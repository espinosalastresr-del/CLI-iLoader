use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use clap::{Parser, Subcommand};
use idevice::{
    IdeviceService,
    afc::opcode::AfcFopenMode,
    house_arrest::HouseArrestClient,
    installation_proxy::InstallationProxyClient,
    lockdown::LockdownClient,
    provider::UsbmuxdProvider,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection},
};
use isideload::{
    anisette::remote_v3::RemoteV3AnisetteProvider,
    auth::apple_account::AppleAccount,
    dev::developer_session::DeveloperSession,
    sideload::{
        SideloaderBuilder,
        builder::MaxCertsBehavior,
    },
    util::{fs_storage::FsStorage, storage::SideloadingStorage},
};
use tracing::{debug, info, warn};

// ─── CLI Definition ───────────────────────────────────────────────────────────

/// iloader CLI — Instala IPAs en iPhone desde Linux (Alpine/musl compatible)
#[derive(Parser)]
#[command(name = "iloader", version, about, long_about = None)]
struct Cli {
    /// URL del servidor anisette (por defecto: http://localhost:6969)
    #[arg(long, env = "ILOADER_ANISETTE", default_value = "http://localhost:6969")]
    anisette: String,

    /// Directorio de datos (para guardar credenciales/storage)
    #[arg(long, env = "ILOADER_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Nivel de verbosidad (0=error, 1=info, 2=debug)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Listar dispositivos iPhone/iPad conectados por USB
    Devices,

    /// Instalar un IPA en el dispositivo conectado
    Install {
        /// Apple ID (email)
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,

        /// Contraseña de Apple ID
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,

        /// UDID del dispositivo (opcional si solo hay uno conectado)
        #[arg(short, long)]
        udid: Option<String>,

        /// Ruta al IPA, o URL https:// para descargarlo automáticamente
        #[arg(value_name = "IPA")]
        ipa: String,
    },

    /// Instalar SideStore directamente (descarga automática)
    InstallSidestore {
        /// Apple ID (email)
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,

        /// Contraseña de Apple ID
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,

        /// UDID del dispositivo (opcional si solo hay uno conectado)
        #[arg(short, long)]
        udid: Option<String>,

        /// Usar versión nightly en lugar de la estable
        #[arg(long)]
        nightly: bool,

        /// Instalar LiveContainer+SideStore en lugar de SideStore solo
        #[arg(long)]
        live_container: bool,
    },

    /// Generar y colocar el archivo de pairing en una app del dispositivo
    Pairing {
        /// UDID del dispositivo (opcional si solo hay uno conectado)
        #[arg(short, long)]
        udid: Option<String>,

        /// Exportar el pairing file a disco en lugar de colocarlo en una app
        #[arg(long, value_name = "ARCHIVO")]
        export: Option<PathBuf>,

        /// App destino: sidestore | feather | livecont | stikdebug
        /// (ignorado si se usa --export)
        #[arg(long, default_value = "sidestore")]
        app: String,
    },

    /// Listar certificados de desarrollo activos en tu Apple ID
    Certs {
        /// Apple ID (email)
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,

        /// Contraseña de Apple ID
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,
    },

    /// Revocar un certificado de desarrollo por serial
    RevokeCert {
        /// Apple ID (email)
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,

        /// Contraseña de Apple ID
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,

        /// Número de serie del certificado a revocar
        serial: String,
    },

    /// Listar App IDs registrados en tu Apple ID
    AppIds {
        /// Apple ID (email)
        #[arg(short, long, env = "ILOADER_APPLE_ID")]
        apple_id: String,

        /// Contraseña de Apple ID
        #[arg(short, long, env = "ILOADER_APPLE_PASSWORD")]
        password: String,
    },
}

// ─── Error Type ───────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("No se encontró ningún dispositivo conectado")]
    NoDevice,
    #[error("UDID '{0}' no encontrado entre los dispositivos conectados")]
    DeviceNotFound(String),
    #[error("Error de usbmuxd: {0}")]
    Usbmuxd(String),
    #[error("Error de comunicación con el dispositivo: {0}")]
    Device(String),
    #[error("Error de autenticación Apple: {0}")]
    Auth(String),
    #[error("Error de sideloading: {0}")]
    Sideload(String),
    #[error("Error de pairing: {0}")]
    Pairing(String),
    #[error("Error de descarga: {0}")]
    Download(String),
    #[error("Error de E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

impl From<rootcause::Report> for CliError {
    fn from(r: rootcause::Report) -> Self {
        CliError::Other(r.to_string())
    }
}

// ─── App State ────────────────────────────────────────────────────────────────

struct AppState {
    data_dir: PathBuf,
    anisette_url: String,
}

impl AppState {
    fn storage(&self) -> Box<dyn SideloadingStorage> {
        Box::new(FsStorage::new(self.data_dir.clone()))
    }
}

// ─── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Inicializar logging
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

    // Inicializar proveedor crypto (requerido por isideload/rustls)
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");
    isideload::init().expect("Failed to initialize error reporting");

    let data_dir = cli.data_dir.unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("iloader")
    });
    std::fs::create_dir_all(&data_dir).ok();

    let state = Arc::new(AppState {
        data_dir,
        anisette_url: if cli.anisette.starts_with("http") {
            cli.anisette
        } else {
            format!("https://{}", cli.anisette)
        },
    });

    let result = match cli.command {
        Commands::Devices => cmd_devices().await,
        Commands::Install { apple_id, password, udid, ipa } => {
            cmd_install(&state, &apple_id, &password, udid.as_deref(), &ipa).await
        }
        Commands::InstallSidestore { apple_id, password, udid, nightly, live_container } => {
            cmd_install_sidestore(&state, &apple_id, &password, udid.as_deref(), nightly, live_container).await
        }
        Commands::Pairing { udid, export, app } => {
            cmd_pairing(udid.as_deref(), export.as_deref(), &app).await
        }
        Commands::Certs { apple_id, password } => {
            cmd_certs(&state, &apple_id, &password).await
        }
        Commands::RevokeCert { apple_id, password, serial } => {
            cmd_revoke_cert(&state, &apple_id, &password, &serial).await
        }
        Commands::AppIds { apple_id, password } => {
            cmd_app_ids(&state, &apple_id, &password).await
        }
    };

    if let Err(e) = result {
        eprintln!("\n❌ Error: {e}");
        std::process::exit(1);
    }
}

// ─── Comandos ─────────────────────────────────────────────────────────────────

async fn cmd_devices() -> Result<(), CliError> {
    let mut usbmuxd = usbmuxd_connect().await?;
    let devices = usbmuxd
        .get_devices()
        .await
        .map_err(|e| CliError::Usbmuxd(e.to_string()))?;

    if devices.is_empty() {
        println!("No hay dispositivos conectados.");
        println!("Asegúrate de:");
        println!("  1. Conectar el iPhone por USB");
        println!("  2. Desbloquear el iPhone");
        println!("  3. Pulsar 'Confiar' en el diálogo del iPhone");
        return Ok(());
    }

    let usbmuxd_addr = UsbmuxdAddr::from_env_var()
        .map_err(|e| CliError::Usbmuxd(e.to_string()))?;

    println!("Dispositivos encontrados:\n");
    for dev in &devices {
        let provider = dev.to_provider(usbmuxd_addr.clone(), "iloader");
        let conn_type = match dev.connection_type {
            Connection::Usb => "USB",
            Connection::Network(_) => "Red",
            Connection::Unknown(_) => "Desconocido",
        };

        // Obtener nombre e iOS
        let (name, version) = match LockdownClient::connect(&provider).await {
            Ok(mut lc) => {
                let name = lc
                    .get_value(Some("DeviceName"), None)
                    .await
                    .ok()
                    .and_then(|v| v.as_string().map(|s| s.to_string()))
                    .unwrap_or_else(|| "Desconocido".into());
                let ver = lc
                    .get_value(Some("ProductVersion"), None)
                    .await
                    .ok()
                    .and_then(|v| v.as_string().map(|s| s.to_string()))
                    .unwrap_or_else(|| "?".into());
                (name, ver)
            }
            Err(_) => ("(no se pudo conectar al lockdown)".into(), "?".into()),
        };

        println!("  📱 {name}");
        println!("     UDID: {}", dev.udid);
        println!("     iOS:  {version}");
        println!("     Tipo: {conn_type}\n");
    }

    Ok(())
}

async fn cmd_install(
    state: &AppState,
    apple_id: &str,
    password: &str,
    udid: Option<&str>,
    ipa: &str,
) -> Result<(), CliError> {
    // Resolver IPA (local o URL)
    let ipa_path = resolve_ipa(ipa).await?;

    // Conectar dispositivo
    let (provider, device_udid) = connect_device(udid).await?;
    println!("📱 Dispositivo conectado: {device_udid}");

    // Autenticar con Apple
    let sideloader = authenticate(state, apple_id, password).await?;

    // Instalar
    println!("📦 Instalando {}...", ipa_path.display());
    sideloader
        .install_app(&provider, ipa_path.clone(), false)
        .await
        .map_err(|e| CliError::Sideload(e.to_string()))?;

    println!("✅ Instalación completada.");
    println!();
    println!("Pasos finales en el iPhone:");
    println!("  1. Ajustes → General → VPN y gestión de dispositivos");
    println!("  2. Toca tu Apple ID → 'Confiar'");

    // Limpiar IPA temporal si se descargó
    if ipa.starts_with("http") {
        let _ = tokio::fs::remove_file(&ipa_path).await;
    }

    Ok(())
}

async fn cmd_install_sidestore(
    state: &AppState,
    apple_id: &str,
    password: &str,
    udid: Option<&str>,
    nightly: bool,
    live_container: bool,
) -> Result<(), CliError> {
    let url = match (live_container, nightly) {
        (true, true)  => "https://github.com/LiveContainer/LiveContainer/releases/download/nightly/LiveContainer+SideStore.ipa",
        (true, false) => "https://github.com/LiveContainer/LiveContainer/releases/latest/download/LiveContainer+SideStore.ipa",
        (false, true) => "https://github.com/SideStore/SideStore/releases/download/nightly/SideStore.ipa",
        (false, false)=> "https://github.com/SideStore/SideStore/releases/latest/download/SideStore.ipa",
    };

    let label = if live_container { "LiveContainer+SideStore" } else { "SideStore" };
    let version = if nightly { "nightly" } else { "estable" };
    println!("⬇️  Descargando {label} ({version})...");

    let ipa_path = resolve_ipa(url).await?;
    println!("✅ Descarga completa");

    // Conectar dispositivo
    let (provider, device_udid) = connect_device(udid).await?;
    println!("📱 Dispositivo: {device_udid}");

    // Autenticar
    let sideloader = authenticate(state, apple_id, password).await?;

    // Instalar
    println!("📦 Instalando {label}...");
    sideloader
        .install_app(&provider, ipa_path.clone(), false)
        .await
        .map_err(|e| CliError::Sideload(e.to_string()))?;
    println!("✅ IPA instalado");

    // Pairing automático
    println!("🔗 Colocando archivo de pairing en {label}...");
    match place_pairing_auto(&device_udid, live_container).await {
        Ok(_) => println!("✅ Pairing colocado automáticamente"),
        Err(e) => {
            println!("⚠️  No se pudo colocar el pairing automáticamente: {e}");
            println!("   Ejecuta manualmente: iloader pairing --app sidestore");
        }
    }

    let _ = tokio::fs::remove_file(&ipa_path).await;

    println!();
    println!("🎉 ¡Listo! Pasos finales:");
    println!("  1. Ajustes → General → VPN y gestión de dispositivos → Confiar");
    println!("  2. Abre {label} y conecta a Wi-Fi");

    Ok(())
}

async fn cmd_pairing(
    udid: Option<&str>,
    export: Option<&PathBuf>,
    app_name: &str,
) -> Result<(), CliError> {
    let (_, device_udid) = connect_device(udid).await?;

    println!("🔗 Generando pairing file para {}...", device_udid);
    println!("   (Es posible que el iPhone muestre un diálogo — pulsa Confiar)");

    let pairing_bytes = generate_pairing(&device_udid).await?;
    println!("✅ Pairing generado ({} bytes)", pairing_bytes.len());

    if let Some(path) = export {
        tokio::fs::write(path, &pairing_bytes)
            .await
            .map_err(CliError::Io)?;
        println!("💾 Guardado en: {}", path.display());
        return Ok(());
    }

    // Colocar en la app
    let (bundle_id, file_path) = resolve_pairing_app(app_name, &device_udid).await?;
    println!("📲 Colocando en {} ({})...", app_name, bundle_id);

    let provider = get_provider_by_udid(&device_udid).await?;
    place_pairing_file(pairing_bytes, &provider, bundle_id, file_path).await?;
    println!("✅ Pairing colocado correctamente");

    Ok(())
}

async fn cmd_certs(state: &AppState, apple_id: &str, password: &str) -> Result<(), CliError> {
    let mut sideloader = authenticate(state, apple_id, password).await?;
    let team = sideloader.get_team().await.map_err(|e| CliError::Auth(e.to_string()))?;
    let dev_session = sideloader.get_dev_session();

    let certs = dev_session
        .list_all_development_certs(&team, None)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    if certs.is_empty() {
        println!("No hay certificados de desarrollo activos.");
        return Ok(());
    }

    println!("Certificados de desarrollo ({})\n", certs.len());
    for cert in &certs {
        println!("  Serial: {}", cert.serial_number.as_deref().unwrap_or("?"));
        println!("  Nombre: {}", cert.name.as_deref().unwrap_or("?"));
        println!("  Máquina: {}", cert.machine_name.as_deref().unwrap_or("?"));
        println!();
    }

    Ok(())
}

async fn cmd_revoke_cert(
    state: &AppState,
    apple_id: &str,
    password: &str,
    serial: &str,
) -> Result<(), CliError> {
    let mut sideloader = authenticate(state, apple_id, password).await?;
    let team = sideloader.get_team().await.map_err(|e| CliError::Auth(e.to_string()))?;
    let dev_session = sideloader.get_dev_session();

    dev_session
        .revoke_development_cert(&team, serial, None)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    println!("✅ Certificado {serial} revocado");
    Ok(())
}

async fn cmd_app_ids(state: &AppState, apple_id: &str, password: &str) -> Result<(), CliError> {
    let mut sideloader = authenticate(state, apple_id, password).await?;
    let team = sideloader.get_team().await.map_err(|e| CliError::Auth(e.to_string()))?;
    let dev_session = sideloader.get_dev_session();

    let response = dev_session
        .list_app_ids(&team, None)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    println!("App IDs registrados:");
    for app_id in &response.app_ids {
        println!("  {} — {}", app_id.identifier, app_id.name.as_deref().unwrap_or("?"));
    }
    println!("\nUsando {} de {} slots", response.app_ids.len(), response.maximum_app_ids);

    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

async fn usbmuxd_connect() -> Result<UsbmuxdConnection, CliError> {
    UsbmuxdConnection::default()
        .await
        .map_err(|e| CliError::Usbmuxd(e.to_string()))
}

/// Conecta al dispositivo por UDID (o al único disponible)
async fn connect_device(udid: Option<&str>) -> Result<(UsbmuxdProvider, String), CliError> {
    let mut usbmuxd = usbmuxd_connect().await?;
    let devices = usbmuxd
        .get_devices()
        .await
        .map_err(|e| CliError::Usbmuxd(e.to_string()))?;

    if devices.is_empty() {
        return Err(CliError::NoDevice);
    }

    let device = if let Some(udid) = udid {
        devices
            .into_iter()
            .find(|d| d.udid == udid)
            .ok_or_else(|| CliError::DeviceNotFound(udid.to_string()))?
    } else {
        if devices.len() > 1 {
            println!("Múltiples dispositivos encontrados. Usando el primero ({}).", devices[0].udid);
            println!("Usa --udid para seleccionar uno específico.");
        }
        devices.into_iter().next().unwrap()
    };

    let udid_str = device.udid.clone();
    let addr = UsbmuxdAddr::from_env_var().map_err(|e| CliError::Usbmuxd(e.to_string()))?;
    let provider = device.to_provider(addr, "iloader");
    Ok((provider, udid_str))
}

async fn get_provider_by_udid(udid: &str) -> Result<UsbmuxdProvider, CliError> {
    let mut usbmuxd = usbmuxd_connect().await?;
    let devices = usbmuxd
        .get_devices()
        .await
        .map_err(|e| CliError::Usbmuxd(e.to_string()))?;
    let device = devices
        .into_iter()
        .find(|d| d.udid == udid)
        .ok_or_else(|| CliError::DeviceNotFound(udid.to_string()))?;
    let addr = UsbmuxdAddr::from_env_var().map_err(|e| CliError::Usbmuxd(e.to_string()))?;
    Ok(device.to_provider(addr, "iloader"))
}

/// Autentica con Apple y devuelve un Sideloader listo
async fn authenticate(
    state: &AppState,
    email: &str,
    password: &str,
) -> Result<isideload::sideload::sideloader::Sideloader, CliError> {
    println!("🔐 Autenticando con Apple ID {}...", email);

    // Callback de 2FA por terminal
    let tfa_closure = move || -> Option<String> {
        print!("Código 2FA: ");
        let mut code = String::new();
        std::io::stdin().read_line(&mut code).ok()?;
        Some(code.trim().to_string())
    };

    let storage = state.storage();

    let mut account = AppleAccount::builder(&email.to_lowercase())
        .anisette_provider(
            RemoteV3AnisetteProvider::default()
                .map_err(|e| CliError::Auth(e.to_string()))?
                .set_serial_number("0".to_string())
                .set_storage(storage)
                .set_url(&state.anisette_url),
        )
        .login(password, tfa_closure)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    println!("✅ Autenticado");

    let dev_session = DeveloperSession::from_account(&mut account)
        .await
        .map_err(|e| CliError::Auth(e.to_string()))?;

    // Si hay múltiples certificados, mostrarlos y dejar que el usuario elija cuál revocar
    let max_certs_callback = {
        move |certs: &Vec<isideload::dev::certificates::DevelopmentCertificate>| -> Option<Vec<String>> {
            println!("\n⚠️  Has alcanzado el límite de certificados de desarrollo.");
            println!("Certificados actuales:\n");
            for (i, cert) in certs.iter().enumerate() {
                println!(
                    "  [{}] Serial: {}  Máquina: {}",
                    i + 1,
                    cert.serial_number.as_deref().unwrap_or("?"),
                    cert.machine_name.as_deref().unwrap_or("?")
                );
            }
            println!("\nEscribe los números a revocar separados por coma (o Enter para cancelar):");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).ok()?;
            let input = input.trim();
            if input.is_empty() {
                return None;
            }
            let serials: Vec<String> = input
                .split(',')
                .filter_map(|s| {
                    s.trim().parse::<usize>().ok().and_then(|i| {
                        certs.get(i - 1)?.serial_number.clone()
                    })
                })
                .collect();
            if serials.is_empty() { None } else { Some(serials) }
        }
    };

    let sideloader = SideloaderBuilder::new(dev_session, email.to_lowercase())
        .machine_name("iloader".into())
        .storage(state.storage())
        .max_certs_behavior(MaxCertsBehavior::Prompt(Box::new(max_certs_callback)))
        .build();

    Ok(sideloader)
}

/// Resuelve el IPA: si es URL lo descarga a un temp file, si es path local lo devuelve tal cual
async fn resolve_ipa(ipa: &str) -> Result<PathBuf, CliError> {
    if ipa.starts_with("http://") || ipa.starts_with("https://") {
        let filename = ipa.split('/').last().unwrap_or("download.ipa");
        let dest = std::env::temp_dir().join(format!("iloader_{}", filename));
        println!("⬇️  Descargando {}...", filename);
        download(ipa, &dest).await?;
        println!("✅ Descargado en {}", dest.display());
        Ok(dest)
    } else {
        let path = PathBuf::from(ipa);
        if !path.exists() {
            return Err(CliError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Archivo no encontrado: {ipa}"),
            )));
        }
        Ok(path)
    }
}

async fn download(url: &str, dest: &PathBuf) -> Result<(), CliError> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| CliError::Download(e.to_string()))?;

    if !response.status().is_success() {
        return Err(CliError::Download(format!(
            "HTTP {}: {}",
            response.status(),
            url
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| CliError::Download(e.to_string()))?;

    tokio::fs::write(dest, &bytes).await.map_err(CliError::Io)?;
    Ok(())
}

const PAIRING_APPS: &[(&str, &str, &str)] = &[
    ("sidestore",  "ALTPairingFile.mobiledevicepairing",                          "com.SideStore.SideStore"),
    ("livecont",   "SideStore/Documents/ALTPairingFile.mobiledevicepairing",      "io.livecontainer.livecontainer"),
    ("feather",    "pairingFile.plist",                                           "kh.crysalis.feather"),
    ("stikdebug",  "pairingFile.plist",                                           "com.stik.stikdebug"),
    ("sparebox",   "pairingFile.plist",                                           "com.Flintrules.SparseBox"),
];

async fn resolve_pairing_app(
    app_name: &str,
    device_udid: &str,
) -> Result<(String, String), CliError> {
    // Primero intentar encontrar el bundle ID real en el dispositivo
    let provider = get_provider_by_udid(device_udid).await?;
    let mut proxy = InstallationProxyClient::connect(&provider)
        .await
        .map_err(|e| CliError::Device(e.to_string()))?;

    let installed = proxy
        .get_apps(Some("User"), None)
        .await
        .map_err(|e| CliError::Device(e.to_string()))?;

    // Buscar la app destino entre las instaladas
    for (name, file_path, fallback_bundle) in PAIRING_APPS {
        if *name != app_name { continue; }

        // Buscar el bundle_id real en el dispositivo
        for (bundle_id, app_plist) in &installed {
            let display = app_plist
                .as_dictionary()
                .and_then(|d| d.get("CFBundleDisplayName")?.as_string())
                .unwrap_or("");

            if bundle_id.contains(fallback_bundle)
                || display.to_lowercase().contains(&app_name.to_lowercase())
            {
                return Ok((bundle_id.clone(), file_path.to_string()));
            }
        }

        // App no instalada pero se conoce el bundle
        return Err(CliError::Device(format!(
            "La app '{}' no está instalada. Instálala primero.",
            app_name
        )));
    }

    Err(CliError::Other(format!(
        "App '{}' no reconocida. Opciones: sidestore, livecont, feather, stikdebug, sparebox",
        app_name
    )))
}

async fn place_pairing_file(
    pairing: Vec<u8>,
    provider: &UsbmuxdProvider,
    bundle_id: String,
    file_path: String,
) -> Result<(), CliError> {
    let ha = HouseArrestClient::connect(provider)
        .await
        .map_err(|e| CliError::Pairing(e.to_string()))?;

    let mut afc = ha
        .vend_documents(bundle_id)
        .await
        .map_err(|e| CliError::Pairing(e.to_string()))?;

    // Crear directorios padre si existen
    if let Some(parent) = file_path.rsplit_once('/').map(|(p, _)| p) {
        afc.mk_dir(format!("/Documents/{}", parent))
            .await
            .map_err(|e| CliError::Pairing(e.to_string()))?;
    }

    let mut file = afc
        .open(format!("/Documents/{}", file_path), AfcFopenMode::Wr)
        .await
        .map_err(|e| CliError::Pairing(e.to_string()))?;

    file.write_entire(&pairing)
        .await
        .map_err(|e| CliError::Pairing(e.to_string()))?;

    file.close()
        .await
        .map_err(|e| CliError::Pairing(e.to_string()))?;

    Ok(())
}

async fn generate_pairing(udid: &str) -> Result<Vec<u8>, CliError> {
    use idevice::{
        lockdown::LockdownClient,
        remote_pairing::{RemotePairingClient, RpPairingFile},
        core_device_proxy::CoreDeviceProxy,
        rsd::RsdHandshake,
        RemoteXpcClient,
    };

    let provider = get_provider_by_udid(udid).await?;
    let mut usbmuxd = usbmuxd_connect().await?;

    // Obtener pairing record de usbmuxd (lockdown)
    let mut pairing_record = usbmuxd
        .get_pair_record(udid)
        .await
        .map_err(|e| CliError::Pairing(format!("Failed to get pair record: {e}")))?;
    pairing_record.udid = Some(udid.to_string());

    let mut lc = LockdownClient::connect(&provider)
        .await
        .map_err(|e| CliError::Pairing(e.to_string()))?;
    lc.start_session(&pairing_record)
        .await
        .map_err(|e| CliError::Pairing(e.to_string()))?;
    lc.set_value("EnableWifiDebugging", true.into(), Some("com.apple.mobile.wireless_lockdown"))
        .await
        .ok(); // no es fatal

    // Serializar lockdown plist
    let lockdown_xml = pairing_record
        .serialize()
        .map_err(|e| CliError::Pairing(e.to_string()))?;

    // Detectar iOS version para saber si necesitamos RPPairing (17.4+)
    let version = lc
        .get_value(Some("ProductVersion"), None)
        .await
        .ok()
        .and_then(|v| v.as_string().map(|s| s.to_string()))
        .unwrap_or_default();

    let is_below_17_4 = {
        let mut parts = version.split('.');
        let major: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor) < (17, 4)
    };

    if is_below_17_4 {
        // iOS 16 solo necesita lockdown
        return Ok(lockdown_xml);
    }

    // iOS 17.4+: generar RPPairing y combinar
    info!("Generando RPPairing (iOS 17.4+)...");
    println!("   Si aparece un diálogo en el iPhone, pulsa Confiar.");

    let rp_file = generate_rppairing(&provider).await
        .map_err(|e| CliError::Pairing(e.to_string()))?;
    let rp_bytes = rp_file.to_bytes();

    // Combinar lockdown + rppairing en un único plist
    let lockdown_plist = plist::Value::from_reader_xml(std::io::Cursor::new(&lockdown_xml))
        .map_err(|e| CliError::Pairing(e.to_string()))?;
    let rp_plist = plist::Value::from_reader_xml(std::io::Cursor::new(&rp_bytes))
        .map_err(|e| CliError::Pairing(e.to_string()))?;

    let mut combined = plist::Dictionary::new();
    if let Some(dict) = lockdown_plist.as_dictionary() {
        for (k, v) in dict { combined.insert(k.clone(), v.clone()); }
    }
    if let Some(dict) = rp_plist.as_dictionary() {
        for (k, v) in dict { combined.insert(k.clone(), v.clone()); }
    }

    let mut out = Vec::new();
    plist::Value::Dictionary(combined)
        .to_writer_xml(&mut out)
        .map_err(|e| CliError::Pairing(e.to_string()))?;
    Ok(out)
}

async fn generate_rppairing(
    provider: &UsbmuxdProvider,
) -> Result<idevice::remote_pairing::RpPairingFile, idevice::IdeviceError> {
    use idevice::{
        core_device_proxy::CoreDeviceProxy,
        rsd::RsdHandshake,
        RemoteXpcClient,
        remote_pairing::{RemotePairingClient, RpPairingFile},
    };

    let proxy = CoreDeviceProxy::connect(provider).await?;
    let rsd_port = proxy.tunnel_info().server_rsd_port;
    let adapter = proxy.create_software_tunnel()?;
    let mut adapter = adapter.to_async_handle();

    let rsd_stream = adapter.connect(rsd_port).await?;
    let handshake = RsdHandshake::new(rsd_stream).await?;

    let tunnel_svc = handshake
        .services
        .get("com.apple.internal.dt.coredevice.untrusted.tunnelservice")
        .ok_or_else(|| idevice::IdeviceError::InternalError("Untrusted tunnel service not found".into()))?;

    let stream = adapter.connect(tunnel_svc.port).await?;
    let mut xpc = RemoteXpcClient::new(stream).await?;
    xpc.do_handshake().await?;
    let _ = xpc.recv_root().await;

    let mut pairing_file = RpPairingFile::generate("iloader");
    let mut client = RemotePairingClient::new(xpc, "iloader", &mut pairing_file);
    client.connect(async |_| "000000".to_string(), ()).await?;

    // Segunda conexión para confirmar
    let stream2 = adapter.connect(tunnel_svc.port).await?;
    let mut xpc2 = RemoteXpcClient::new(stream2).await?;
    xpc2.do_handshake().await?;
    let _ = xpc2.recv_root().await;
    let mut client2 = RemotePairingClient::new(xpc2, "iloader", &mut pairing_file);
    client2.connect(async |_| "000000".to_string(), ()).await?;

    Ok(pairing_file)
}

async fn place_pairing_auto(device_udid: &str, live_container: bool) -> Result<(), CliError> {
    let app_name = if live_container { "livecont" } else { "sidestore" };
    let pairing = generate_pairing(device_udid).await?;
    let (bundle_id, file_path) = resolve_pairing_app(app_name, device_udid).await?;
    let provider = get_provider_by_udid(device_udid).await?;
    place_pairing_file(pairing, &provider, bundle_id, file_path).await
}
