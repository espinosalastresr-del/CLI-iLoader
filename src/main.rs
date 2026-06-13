//! iloader CLI — instala IPAs en iPhone desde Alpine Linux (musl)
//!
//! Basado directamente en el backend de iloader (nab138/iloader),
//! adaptado para terminal sin Tauri.

use std::{
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};

use clap::{Parser, Subcommand};
use idevice::{
    IdeviceService, RemoteXpcClient,
    afc::opcode::AfcFopenMode,
    core_device_proxy::CoreDeviceProxy,
    house_arrest::HouseArrestClient,
    installation_proxy::InstallationProxyClient,
    lockdown::LockdownClient,
    provider::IdeviceProvider,
    remote_pairing::{RemotePairingClient, RpPairingFile},
    rsd::RsdHandshake,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection},
};
use isideload::{
    anisette::remote_v3::RemoteV3AnisetteProvider,
    auth::apple_account::AppleAccount,
    dev::{
        app_ids::AppIdsApi,
        certificates::CertificatesApi,
        developer_session::DeveloperSession,
    },
    sideload::{SideloaderBuilder, builder::MaxCertsBehavior, sideloader::Sideloader},
    util::{
        fs_storage::FsStorage,
        storage::SideloadingStorage,
    },
};
use tracing::{info, warn};

// ─── CLI ──────────────────────────────────────────────────────────────────────

/// iloader CLI — instala IPAs en iPhone desde Alpine Linux
#[derive(Parser)]
#[command(name = "iloader", version, about)]
struct Cli {
    /// URL del servidor anisette (requiere: docker run -d -p 6969:6969 dadoum/anisette-v3-server)
    #[arg(long, env = "ILOADER_ANISETTE", default_value = "http://localhost:6969")]
    anisette: String,

    /// Directorio de datos (certificados, anisette cache)
    #[arg(long, env = "ILOADER_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// -v info, -vv debug
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Listar iPhones/iPads conectados por USB
    Devices,

    /// Instalar un IPA en el dispositivo (ruta local o URL https://)
    Install {
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,
        /// UDID del dispositivo (opcional si solo hay uno)
        #[arg(short, long)]
        udid: Option<String>,
        /// Ruta al .ipa o URL de descarga directa
        #[arg(value_name = "IPA")]
        ipa: String,
    },

    /// Descargar e instalar SideStore automáticamente
    InstallSidestore {
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,
        #[arg(short, long)]
        udid: Option<String>,
        /// Usar versión nightly en vez de la estable
        #[arg(long)]
        nightly: bool,
        /// Instalar LiveContainer+SideStore
        #[arg(long)]
        live_container: bool,
    },

    /// Generar y colocar el pairing file en una app del dispositivo
    Pairing {
        #[arg(short, long)]
        udid: Option<String>,
        /// App destino: sidestore | feather | livecont | stikdebug | sparebox | protokolle | antrag
        #[arg(long, default_value = "sidestore")]
        app: String,
        /// Exportar a disco en vez de colocarlo en la app
        #[arg(long, value_name = "RUTA")]
        export: Option<PathBuf>,
    },

    /// Ver certificados de desarrollo del Apple ID
    Certs {
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,
    },

    /// Revocar un certificado de desarrollo por serial
    RevokeCert {
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,
        /// Número de serie (obtenido con `iloader certs`)
        serial: String,
    },

    /// Ver App IDs registrados en el Apple ID
    AppIds {
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,
    },
}

// ─── Errores ──────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("No hay dispositivos conectados. Conecta el iPhone y pulsa 'Confiar'.")]
    NoDevice,
    #[error("UDID '{0}' no encontrado entre los dispositivos conectados")]
    DeviceNotFound(String),
    #[error("usbmuxd: {0}")]
    Usbmuxd(String),
    #[error("Dispositivo: {0}")]
    Device(String),
    #[error("Apple ID: {0}")]
    Auth(String),
    #[error("Sideloading: {0}")]
    Sideload(String),
    #[error("Pairing: {0}")]
    Pairing(String),
    #[error("Descarga: {0}")]
    Download(String),
    #[error("E/S: {0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Other(String),
}

// ─── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_target(false)
        .compact()
        .init();

    // isideload requiere esto para el reporte de errores de red
    isideload::init().expect("isideload::init() failed");

    let anisette_url = normalize_url(&cli.anisette);

    let data_dir = cli.data_dir.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".local")
            .join("share")
            .join("iloader")
    });
    std::fs::create_dir_all(&data_dir).ok();

    let result = match cli.command {
        Commands::Devices => cmd_devices().await,

        Commands::Install { apple_id, password, udid, ipa } => {
            cmd_install(&apple_id, &password, udid.as_deref(), &ipa, &anisette_url, &data_dir).await
        }

        Commands::InstallSidestore { apple_id, password, udid, nightly, live_container } => {
            cmd_install_sidestore(
                &apple_id, &password, udid.as_deref(),
                nightly, live_container, &anisette_url, &data_dir,
            ).await
        }

        Commands::Pairing { udid, app, export } => {
            cmd_pairing(udid.as_deref(), &app, export.as_ref()).await
        }

        Commands::Certs { apple_id, password } => {
            cmd_certs(&apple_id, &password, &anisette_url, &data_dir).await
        }

        Commands::RevokeCert { apple_id, password, serial } => {
            cmd_revoke_cert(&apple_id, &password, &serial, &anisette_url, &data_dir).await
        }

        Commands::AppIds { apple_id, password } => {
            cmd_app_ids(&apple_id, &password, &anisette_url, &data_dir).await
        }
    };

    if let Err(e) = result {
        eprintln!("\n❌  {e}");
        std::process::exit(1);
    }
}

// ─── Comandos ─────────────────────────────────────────────────────────────────

async fn cmd_devices() -> Result<(), CliError> {
    let mut conn = usbmuxd_conn().await?;
    let devs = conn.get_devices().await
        .map_err(|e| CliError::Usbmuxd(e.to_string()))?;

    if devs.is_empty() {
        println!("Sin dispositivos. Conecta el iPhone y pulsa 'Confiar'.");
        return Ok(());
    }

    let addr = usbmuxd_addr()?;
    println!("Dispositivos encontrados:\n");
    for (i, dev) in devs.iter().enumerate() {
        let conn_type = match dev.connection_type {
            Connection::Usb => "USB",
            Connection::Network(_) => "Red",
            Connection::Unknown(_) => "Desconocido",
        };
        let provider = dev.to_provider(addr.clone(), i as u32, "iloader");
        let (name, ver) = lockdown_info(&provider).await;
        println!("  📱 {name}  (iOS {ver})");
        println!("     UDID: {}", dev.udid);
        println!("     Tipo: {conn_type}\n");
    }
    Ok(())
}

async fn cmd_install(
    apple_id: &str,
    password: &str,
    udid: Option<&str>,
    ipa: &str,
    anisette_url: &str,
    data_dir: &PathBuf,
) -> Result<(), CliError> {
    let ipa_path = resolve_ipa(ipa).await?;
    let (provider, dev_udid) = select_device(udid).await?;
    println!("📱 Dispositivo: {dev_udid}");

    let mut sideloader = authenticate(apple_id, password, anisette_url, data_dir).await?;

    println!("📦 Instalando {}…", ipa_path.display());
    sideloader
        .install_app(&provider, ipa_path.clone(), false)
        .await
        .map_err(|e| CliError::Sideload(e.to_string()))?;

    if ipa.starts_with("http") {
        let _ = tokio::fs::remove_file(&ipa_path).await;
    }
    println!("✅ Instalado.");
    println!("   Ajustes → General → VPN y gestión de dispositivos → Confiar");
    Ok(())
}

async fn cmd_install_sidestore(
    apple_id: &str,
    password: &str,
    udid: Option<&str>,
    nightly: bool,
    live_container: bool,
    anisette_url: &str,
    data_dir: &PathBuf,
) -> Result<(), CliError> {
    let (filename, url) = match (live_container, nightly) {
        (true,  true)  => ("LiveContainerSideStore-Nightly.ipa", "https://github.com/LiveContainer/LiveContainer/releases/download/nightly/LiveContainer+SideStore.ipa"),
        (true,  false) => ("LiveContainerSideStore.ipa",         "https://github.com/LiveContainer/LiveContainer/releases/latest/download/LiveContainer+SideStore.ipa"),
        (false, true)  => ("SideStore-Nightly.ipa",              "https://github.com/SideStore/SideStore/releases/download/nightly/SideStore.ipa"),
        (false, false) => ("SideStore.ipa",                      "https://github.com/SideStore/SideStore/releases/latest/download/SideStore.ipa"),
    };
    let label = if live_container { "LiveContainer+SideStore" } else { "SideStore" };

    println!("⬇️  Descargando {label}…");
    let dest = std::env::temp_dir().join(filename);
    http_download(url, &dest).await?;
    println!("✅ Descarga completa");

    let (provider, dev_udid) = select_device(udid).await?;
    println!("📱 Dispositivo: {dev_udid}");

    let mut sideloader = authenticate(apple_id, password, anisette_url, data_dir).await?;

    println!("📦 Instalando {label}…");
    sideloader
        .install_app(&provider, dest.clone(), false)
        .await
        .map_err(|e| CliError::Sideload(e.to_string()))?;
    println!("✅ IPA instalado");

    let _ = tokio::fs::remove_file(&dest).await;

    // Colocar pairing automáticamente (igual que el backend de iloader)
    let app_key = if live_container { "livecont" } else { "sidestore" };
    println!("🔗 Colocando pairing file en {label}…");
    match place_pairing_auto(&dev_udid, app_key).await {
        Ok(_)  => println!("✅ Pairing colocado"),
        Err(e) => {
            println!("⚠️  Pairing automático falló: {e}");
            println!("   Ejecuta después: iloader pairing --app {app_key}");
        }
    }

    println!("\n🎉 Listo.");
    println!("   Ajustes → General → VPN y gestión de dispositivos → Confiar");
    Ok(())
}

async fn cmd_pairing(
    udid: Option<&str>,
    app_name: &str,
    export: Option<&PathBuf>,
) -> Result<(), CliError> {
    let (_, dev_udid) = select_device(udid).await?;
    println!("🔗 Generando pairing para {dev_udid}…");
    println!("   (Si el iPhone muestra un diálogo, pulsa Confiar)");

    let bytes = generate_pairing_bytes(&dev_udid).await?;
    println!("✅ Pairing generado ({} bytes)", bytes.len());

    if let Some(path) = export {
        tokio::fs::write(path, &bytes).await?;
        println!("💾 Guardado en {}", path.display());
        return Ok(());
    }

    place_pairing_auto(&dev_udid, app_name).await?;
    println!("✅ Pairing colocado en '{app_name}'");
    Ok(())
}

async fn cmd_certs(
    apple_id: &str,
    password: &str,
    anisette_url: &str,
    data_dir: &PathBuf,
) -> Result<(), CliError> {
    let mut sideloader = authenticate(apple_id, password, anisette_url, data_dir).await?;
    let team = sideloader.get_team().await
        .map_err(|e| CliError::Auth(e.to_string()))?;
    let dev_session = sideloader.get_dev_session();

    let certs = dev_session
        .list_all_development_certs(&team, None)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    if certs.is_empty() {
        println!("No hay certificados de desarrollo activos.");
        return Ok(());
    }
    println!("Certificados ({}):\n", certs.len());
    for c in &certs {
        println!("  Serial:  {}", c.serial_number.as_deref().unwrap_or("?"));
        println!("  Nombre:  {}", c.name.as_deref().unwrap_or("?"));
        println!("  Máquina: {}\n", c.machine_name.as_deref().unwrap_or("?"));
    }
    Ok(())
}

async fn cmd_revoke_cert(
    apple_id: &str,
    password: &str,
    serial: &str,
    anisette_url: &str,
    data_dir: &PathBuf,
) -> Result<(), CliError> {
    let mut sideloader = authenticate(apple_id, password, anisette_url, data_dir).await?;
    let team = sideloader.get_team().await
        .map_err(|e| CliError::Auth(e.to_string()))?;
    let dev_session = sideloader.get_dev_session();

    dev_session
        .revoke_development_cert(&team, serial, None)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    println!("✅ Certificado {serial} revocado");
    Ok(())
}

async fn cmd_app_ids(
    apple_id: &str,
    password: &str,
    anisette_url: &str,
    data_dir: &PathBuf,
) -> Result<(), CliError> {
    let mut sideloader = authenticate(apple_id, password, anisette_url, data_dir).await?;
    let team = sideloader.get_team().await
        .map_err(|e| CliError::Auth(e.to_string()))?;
    let dev_session = sideloader.get_dev_session();

    let resp = dev_session
        .list_app_ids(&team, None)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    println!("App IDs ({} de {} slots):\n",
        resp.app_ids.len(), resp.maximum_app_ids);
    for app in &resp.app_ids {
        println!("  {}  —  {}",
            app.identifier,
            app.name.as_deref().unwrap_or("?"));
    }
    Ok(())
}

// ─── Auth ─────────────────────────────────────────────────────────────────────

/// Autentica con Apple y construye el Sideloader (igual que `login()` en account.rs del backend)
async fn authenticate(
    email: &str,
    password: &str,
    anisette_url: &str,
    data_dir: &PathBuf,
) -> Result<Sideloader, CliError> {
    println!("🔐 Autenticando {}…", email);

    // Callback de 2FA por terminal
    let tfa_closure = || -> Option<String> {
        print!("Código 2FA: ");
        io::stdout().flush().ok();
        let mut code = String::new();
        io::stdin().read_line(&mut code).ok()?;
        let code = code.trim().to_string();
        if code.is_empty() { None } else { Some(code) }
    };

    let storage = make_storage(data_dir);

    let mut account = AppleAccount::builder(&email.to_lowercase())
        .anisette_provider(
            RemoteV3AnisetteProvider::default()
                .map_err(|e| CliError::Auth(e.to_string()))?
                .set_serial_number("0".to_string())
                .set_storage(storage)
                .set_url(anisette_url),
        )
        .login(password, tfa_closure)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    info!("Logged in to Apple ID");

    let dev_session = DeveloperSession::from_account(&mut account)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    info!("Developer session created");

    // Callback para límite de certificados — pregunta por terminal qué revocar
    let max_certs_callback = |certs: &Vec<isideload::dev::certificates::DevelopmentCertificate>|
        -> Option<Vec<String>>
    {
        println!("\n⚠️  Límite de certificados alcanzado.");
        println!("Certificados activos:\n");
        for (i, c) in certs.iter().enumerate() {
            println!("  [{}] Serial: {}  Máquina: {}",
                i + 1,
                c.serial_number.as_deref().unwrap_or("?"),
                c.machine_name.as_deref().unwrap_or("?"),
            );
        }
        print!("\nNúmeros a revocar (separados por coma, Enter para cancelar): ");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin().read_line(&mut input).ok()?;
        let input = input.trim();
        if input.is_empty() { return None; }

        let serials: Vec<String> = input
            .split(',')
            .filter_map(|s| {
                s.trim().parse::<usize>().ok()
                    .and_then(|i| certs.get(i - 1))
                    .and_then(|c| c.serial_number.clone())
            })
            .collect();
        if serials.is_empty() { None } else { Some(serials) }
    };

    let sideloader = SideloaderBuilder::new(dev_session, email.to_lowercase())
        .machine_name("iloader".into())
        .storage(make_storage(data_dir))
        .max_certs_behavior(MaxCertsBehavior::Prompt(Box::new(max_certs_callback)))
        .build();

    println!("✅ Autenticado");
    Ok(sideloader)
}

fn make_storage(data_dir: &PathBuf) -> Box<dyn SideloadingStorage> {
    Box::new(FsStorage::new(data_dir.clone()))
}

// ─── Device ───────────────────────────────────────────────────────────────────

async fn usbmuxd_conn() -> Result<UsbmuxdConnection, CliError> {
    UsbmuxdConnection::default().await
        .map_err(|e| CliError::Usbmuxd(e.to_string()))
}

fn usbmuxd_addr() -> Result<UsbmuxdAddr, CliError> {
    UsbmuxdAddr::from_env_var()
        .map_err(|e| CliError::Usbmuxd(e.to_string()))
}

async fn select_device(
    udid: Option<&str>,
) -> Result<(impl IdeviceProvider + Clone, String), CliError> {
    let mut conn = usbmuxd_conn().await?;
    let devs = conn.get_devices().await
        .map_err(|e| CliError::Usbmuxd(e.to_string()))?;

    if devs.is_empty() {
        return Err(CliError::NoDevice);
    }

    let (idx, dev) = if let Some(u) = udid {
        devs.iter().enumerate()
            .find(|(_, d)| d.udid == u)
            .map(|(i, d)| (i, d.clone()))
            .ok_or_else(|| CliError::DeviceNotFound(u.to_string()))?
    } else {
        if devs.len() > 1 {
            println!("⚠️  Múltiples dispositivos. Usando {} (usa --udid para elegir).", devs[0].udid);
        }
        (0, devs[0].clone())
    };

    let udid_str = dev.udid.clone();
    let provider = dev.to_provider(usbmuxd_addr()?, idx as u32, "iloader");
    Ok((provider, udid_str))
}

async fn lockdown_info(provider: &impl IdeviceProvider) -> (String, String) {
    let Ok(mut lc) = LockdownClient::connect(provider).await else {
        return ("Desconocido".into(), "?".into());
    };
    let name = lc.get_value("DeviceName").await
        .ok().and_then(|v| v.as_string().map(|s| s.to_string()))
        .unwrap_or_else(|| "Desconocido".into());
    let ver = lc.get_value("ProductVersion").await
        .ok().and_then(|v| v.as_string().map(|s| s.to_string()))
        .unwrap_or_else(|| "?".into());
    (name, ver)
}

// ─── IPA ──────────────────────────────────────────────────────────────────────

async fn resolve_ipa(ipa: &str) -> Result<PathBuf, CliError> {
    if ipa.starts_with("https://") || ipa.starts_with("http://") {
        let fname = ipa.split('/').next_back().unwrap_or("app.ipa");
        let dest  = std::env::temp_dir().join(format!("iloader_{fname}"));
        println!("⬇️  Descargando {fname}…");
        http_download(ipa, &dest).await?;
        Ok(dest)
    } else {
        let p = PathBuf::from(ipa);
        if !p.exists() {
            return Err(CliError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Archivo no encontrado: {ipa}"),
            )));
        }
        Ok(p)
    }
}

async fn http_download(url: &str, dest: &PathBuf) -> Result<(), CliError> {
    let bytes = reqwest::get(url).await
        .map_err(|e| CliError::Download(e.to_string()))?
        .error_for_status()
        .map_err(|e| CliError::Download(e.to_string()))?
        .bytes().await
        .map_err(|e| CliError::Download(e.to_string()))?;
    tokio::fs::write(dest, &bytes).await.map_err(CliError::Io)?;
    Ok(())
}

// ─── Pairing ──────────────────────────────────────────────────────────────────

// (clave_cli, nombre_display, fragmento_bundle_id, path_en_Documents)
const PAIRING_APPS: &[(&str, &str, &str, &str)] = &[
    ("sidestore", "SideStore",  "SideStore",          "ALTPairingFile.mobiledevicepairing"),
    ("livecont",  "LiveContainer", "livecontainer",   "SideStore/Documents/ALTPairingFile.mobiledevicepairing"),
    ("feather",   "Feather",    "crysalis.feather",   "pairingFile.plist"),
    ("stikdebug", "StikDebug",  "stik.stikdebug",     "pairingFile.plist"),
    ("sparebox",  "SparseBox",  "Flintrules.SparseBox","pairingFile.plist"),
    ("protokolle","Protokolle", "khcrysalis.Protokolle","pairingFile.plist"),
    ("antrag",    "Antrag",     "antrag",              "pairingFile.plist"),
];

/// Genera el pairing file para el dispositivo dado su UDID.
/// Para iOS < 17.4 devuelve solo el lockdown plist.
/// Para iOS >= 17.4 combina lockdown + RPPairing (igual que pairing.rs del backend).
async fn generate_pairing_bytes(udid: &str) -> Result<Vec<u8>, CliError> {
    let mut conn = usbmuxd_conn().await?;

    // Obtener pairing record de usbmuxd
    let mut rec = conn.get_pair_record(udid).await
        .map_err(|e| CliError::Pairing(format!("get_pair_record: {e}")))?;
    rec.udid = Some(udid.to_string());

    let devs = conn.get_devices().await
        .map_err(|e| CliError::Usbmuxd(e.to_string()))?;
    let (idx, dev) = devs.iter().enumerate()
        .find(|(_, d)| d.udid == udid)
        .map(|(i, d)| (i, d.clone()))
        .ok_or_else(|| CliError::DeviceNotFound(udid.to_string()))?;
    let provider = dev.to_provider(usbmuxd_addr()?, idx as u32, "iloader");

    // Conectar lockdown, iniciar sesión y habilitar wifi debugging
    let mut lc = LockdownClient::connect(&provider).await
        .map_err(|e| CliError::Pairing(format!("lockdown: {e}")))?;
    lc.start_session(&rec).await
        .map_err(|e| CliError::Pairing(format!("start_session: {e}")))?;
    let _ = lc.set_value("EnableWifiDebugging", true.into(),
        Some("com.apple.mobile.wireless_lockdown")).await;

    // Versión iOS
    let ver_str = lc.get_value("ProductVersion").await
        .ok().and_then(|v| v.as_string().map(|s| s.to_string()))
        .unwrap_or_default();

    // Serializar lockdown plist
    let lockdown_xml = rec.serialize()
        .map_err(|e| CliError::Pairing(format!("serialize: {e}")))?;
    let lockdown_plist = plist::Value::from_reader_xml(io::Cursor::new(&lockdown_xml))
        .map_err(|e| CliError::Pairing(e.to_string()))?;

    // iOS < 17.4 → solo lockdown
    if is_ios_below(&ver_str, 17, 4) {
        info!("iOS {ver_str} < 17.4, usando solo lockdown plist");
        let dict = lockdown_plist.as_dictionary()
            .ok_or_else(|| CliError::Pairing("Lockdown plist no es un diccionario".into()))?;
        let mut out = Vec::new();
        plist::Value::Dictionary(dict.clone()).to_writer_xml(&mut out)
            .map_err(|e| CliError::Pairing(e.to_string()))?;
        return Ok(out);
    }

    // iOS 17.4+ → generar RPPairing y combinar
    info!("iOS {ver_str} >= 17.4, generando RPPairing…");
    println!("   Si el iPhone muestra un diálogo, pulsa Confiar.");

    let rp_bytes = generate_rppairing(&provider, "iloader").await
        .map_err(|e| CliError::Pairing(e.to_string()))?
        .to_bytes();

    let rp_plist = plist::Value::from_reader_xml(io::Cursor::new(&rp_bytes))
        .map_err(|e| CliError::Pairing(e.to_string()))?;

    // Combinar los dos plists en uno (igual que plist!(dict {:<, :<}) del backend)
    let mut combined = plist::Dictionary::new();
    if let Some(d) = lockdown_plist.as_dictionary() {
        for (k, v) in d { combined.insert(k.clone(), v.clone()); }
    }
    if let Some(d) = rp_plist.as_dictionary() {
        for (k, v) in d { combined.insert(k.clone(), v.clone()); }
    }
    let mut out = Vec::new();
    plist::Value::Dictionary(combined).to_writer_xml(&mut out)
        .map_err(|e| CliError::Pairing(e.to_string()))?;
    Ok(out)
}

/// Genera un RpPairingFile conectándose al CoreDeviceProxy (iOS 17.4+).
/// Idéntico a `generate_rppairing()` de pairing.rs del backend.
async fn generate_rppairing(
    provider: &impl IdeviceProvider,
    hostname: &str,
) -> Result<RpPairingFile, idevice::IdeviceError> {
    info!("Connecting to CoreDeviceProxy…");
    let proxy = CoreDeviceProxy::connect(provider).await?;
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    info!("CDTunnel established, RSD port {rsd_port}");

    let adapter = proxy.create_software_tunnel()?;
    let mut adapter = adapter.to_async_handle();

    info!("Performing RSD handshake…");
    let rsd_stream = adapter.connect(rsd_port).await?;
    let handshake = RsdHandshake::new(rsd_stream).await?;
    info!("RSD: {} services", handshake.services.len());

    let tunnel_svc = handshake.services
        .get("com.apple.internal.dt.coredevice.untrusted.tunnelservice")
        .ok_or_else(|| idevice::IdeviceError::InternalError(
            "Untrusted tunnel service not found".into()
        ))?;

    let mut pairing_file = RpPairingFile::generate(hostname);

    // Primera conexión — iniciar el pairing
    {
        let stream = adapter.connect(tunnel_svc.port).await?;
        let mut xpc = RemoteXpcClient::new(stream).await?;
        xpc.do_handshake().await?;
        let _ = xpc.recv_root().await;
        let mut client = RemotePairingClient::new(xpc, hostname, &mut pairing_file);
        client.connect(async |_| "000000".to_string(), ()).await?;
    }

    // Segunda conexión — confirmar en keychain del dispositivo
    {
        let stream = adapter.connect(tunnel_svc.port).await?;
        let mut xpc = RemoteXpcClient::new(stream).await?;
        xpc.do_handshake().await?;
        let _ = xpc.recv_root().await;
        let mut client = RemotePairingClient::new(xpc, hostname, &mut pairing_file);
        client.connect(async |_| "000000".to_string(), ()).await?;
    }

    Ok(pairing_file)
}

/// Detecta el bundle_id de la app en el dispositivo y coloca el pairing file.
/// Idéntico al flujo de `place_pairing_cmd` + `installed_pairing_apps` del backend.
async fn place_pairing_auto(dev_udid: &str, app_key: &str) -> Result<(), CliError> {
    let entry = PAIRING_APPS.iter()
        .find(|(k, _, _, _)| *k == app_key)
        .ok_or_else(|| CliError::Other(format!(
            "App '{app_key}' desconocida. Opciones: {}",
            PAIRING_APPS.iter().map(|(k, _, _, _)| *k).collect::<Vec<_>>().join(", ")
        )))?;

    let (_key, display_name, bundle_fragment, file_path) = entry;

    let pairing = generate_pairing_bytes(dev_udid).await?;

    let (provider, _) = select_device(Some(dev_udid)).await?;

    // Buscar bundle_id real via InstallationProxy
    let mut proxy = InstallationProxyClient::connect(&provider).await
        .map_err(|e| CliError::Device(e.to_string()))?;
    let apps = proxy.get_apps(Some("User"), None).await
        .map_err(|e| CliError::Device(e.to_string()))?;

    // Buscar por display name (igual que el backend) o por bundle fragment
    let bundle_id = apps.iter()
        .find(|(bid, app)| {
            let name = app.as_dictionary()
                .and_then(|d| d.get("CFBundleDisplayName")?.as_string())
                .unwrap_or("");
            name == *display_name || bid.contains(bundle_fragment)
        })
        .map(|(bid, _)| bid.clone())
        .ok_or_else(|| CliError::Device(format!(
            "'{display_name}' no está instalado. Instálalo primero."
        )))?;

    // Colocar el archivo via HouseArrest + AFC (idéntico a place_file() del backend)
    place_file(pairing, &provider, bundle_id, file_path.to_string()).await
}

/// Coloca un archivo en el contenedor Documents de una app via HouseArrest.
/// Idéntico a `place_file()` de pairing.rs del backend.
async fn place_file(
    pairing: Vec<u8>,
    provider: &impl IdeviceProvider,
    bundle_id: String,
    path: String,
) -> Result<(), CliError> {
    let ha = HouseArrestClient::connect(provider).await
        .map_err(|e| CliError::Pairing(format!("HouseArrest connect: {e}")))?;
    let mut afc = ha.vend_documents(bundle_id).await
        .map_err(|e| CliError::Pairing(format!("vend_documents: {e}")))?;

    // Crear directorio padre si es necesario
    afc.mk_dir(format!(
        "/Documents/{}",
        path.rsplit_once('/').map(|x| x.0).unwrap_or("")
    )).await
    .map_err(|e| CliError::Pairing(format!("mk_dir: {e}")))?;

    let mut file = afc.open(format!("/Documents/{path}"), AfcFopenMode::Wr).await
        .map_err(|e| CliError::Pairing(format!("open: {e}")))?;
    file.write_entire(&pairing).await
        .map_err(|e| CliError::Pairing(format!("write: {e}")))?;
    file.close().await
        .map_err(|e| CliError::Pairing(format!("close: {e}")))?;

    Ok(())
}

// ─── Utils ────────────────────────────────────────────────────────────────────

fn normalize_url(url: &str) -> String {
    if url.starts_with("http") { url.to_string() } else { format!("https://{url}") }
}

fn is_ios_below(version: &str, target_major: u32, target_minor: u32) -> bool {
    let mut parts = version.split('.');
    let maj: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let min: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (maj, min) < (target_major, target_minor)
}
