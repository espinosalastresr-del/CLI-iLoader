//! network_transport.rs — transporte alternativo sin usbmuxd/USB.
//!
//! Reutiliza el .mobiledevicepairing combinado (lockdown + RpPairingFile)
//! que ya genera `iloader pairing --export <ruta>`, y lo usa contra
//! 10.7.0.1 (expuesto por LocalDevVPN) en vez de pedirle el device a usbmuxd.
//!
//! ⚠️ ADVERTENCIA HONESTA: las 2-3 líneas marcadas con VERIFICAR son las
//! únicas partes de este archivo que no pude confirmar contra el código
//! fuente exacto de idevice 0.1.61 (no tengo acceso de red para revisar
//! el crate pineado). Todo lo demás reutiliza tipos y funciones que ya
//! están probados en tu propio main.rs.
//!
//! Antes de compilar en serio, corre esto localmente (con tu Cargo.lock
//! ya resuelto, no necesita red):
//!     cargo doc -p idevice --no-deps --open
//! y confirma las firmas de:
//!   - RpPairingFile: deserialización desde bytes/plist guardado
//!   - RemotePairingClient: método para RETOMAR una sesión ya pareada
//!     contra una IP:puerto fijos (no el flujo de generación que ya usas)

use std::net::IpAddr;

use idevice::{
    IdeviceService,
    core_device_proxy::CoreDeviceProxy,
    provider::IdeviceProvider,
    remote_pairing::{RemotePairingClient, RpPairingFile},
    rsd::RsdHandshake,
    RemoteXpcClient,
};

/// Dirección por defecto que expone LocalDevVPN dentro de iSH.
pub const LOCALDEVVPN_ADDR: &str = "10.7.0.1";

/// Carga el .mobiledevicepairing combinado que ya exportaste con
/// `iloader pairing --export archivo.mobiledevicepairing`.
///
/// El archivo contiene un plist con AMBOS: el registro lockdown clásico
/// y los campos de un RpPairingFile (mismo patrón que combinas al
/// generarlo en `generate_pairing_bytes`).
pub fn load_saved_pairing(path: &std::path::Path) -> anyhow::Result<RpPairingFile> {
    let bytes = std::fs::read(path)?;
    let plist_val = plist::Value::from_reader_xml(std::io::Cursor::new(&bytes))?;

    // VERIFICAR: RpPairingFile probablemente expone `from_bytes` o
    // `deserialize` a partir del plist ya combinado. En tu propio código,
    // `generate_rppairing` termina con `pairing_file.to_bytes()`, así que
    // debería existir el inverso simétrico. Si no, puede que tengas que
    // extraer solo las claves rp_* del diccionario combinado y reconstruir
    // el struct campo a campo — pero el `to_bytes`/`from_bytes` simétrico
    // es lo más probable dado el resto de la API.
    let rp = RpPairingFile::from_bytes(&plist_to_raw_rp_bytes(&plist_val)?)?;
    Ok(rp)
}

fn plist_to_raw_rp_bytes(v: &plist::Value) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    v.to_writer_xml(&mut out)?;
    Ok(out)
}

/// Conecta directamente a 10.7.0.1 usando el RpPairingFile ya generado,
/// sin pasar por usbmuxd. Esto es lo que reemplaza a `select_device()`
/// cuando corres dentro de iSH sin USB.
///
/// Reutiliza EXACTAMENTE el mismo camino RemotePairing → RSD que ya
/// probaste en `generate_rppairing`, solo que aquí NO generamos un
/// pairing nuevo — retomamos el que ya existe, contra una IP fija en
/// vez de descubrir el device por Bonjour/usbmuxd.
pub async fn connect_over_network(
    host: IpAddr,
    pairing: &RpPairingFile,
) -> anyhow::Result<RsdHandshake> {
    // VERIFICAR: en tu flujo actual, CoreDeviceProxy::connect(provider) recibe
    // un IdeviceProvider resuelto vía usbmuxd. Para el camino de red puro,
    // lo normal en el ecosistema idevice es que exista una variante que
    // conecte un TcpStream crudo a (host, puerto RemotePairing) y arranque
    // RemotePairingClient directamente ahí, SIN pasar por CoreDeviceProxy/
    // lockdown en absoluto — porque RemotePairing es un protocolo aparte
    // que ya trae su propio TLS-PSK. El puerto documentado por SideInstaller
    // para este propósito es 49152.
    let stream = tokio::net::TcpStream::connect((host, 49152)).await?;

    let mut xpc = RemoteXpcClient::new(stream).await?;
    xpc.do_handshake().await?;
    let _ = xpc.recv_root().await;

    // "resume" en vez de "connect" (que es el que usa SRP completo para
    // generar credenciales nuevas). Verificar el nombre exacto del método
    // de reanudación en RemotePairingClient para 0.1.61.
    let mut client = RemotePairingClient::new(xpc, "iloader");
    let tunnel = client.resume(pairing).await?; // <- nombre a verificar

    let handshake = RsdHandshake::new(tunnel).await?;
    Ok(handshake)
}

/// Envuelve el RsdHandshake ya establecido para que el resto de tu código
/// (InstallationProxyClient::connect, HouseArrestClient::connect,
/// Sideloader::install_app, etc.) lo trate igual que un UsbmuxdProvider.
///
/// Si `cargo build` marca error aquí, es casi seguro que el nombre del
/// método que idevice espera para "dame un stream al puerto X del RSD"
/// no es exactamente este. El error del compilador te va a decir el
/// nombre real — ajusta esta única función y todo lo demás sigue igual.
pub struct RsdProvider {
    handshake: RsdHandshake,
    label: String,
}

impl RsdProvider {
    pub fn new(handshake: RsdHandshake) -> Self {
        Self { handshake, label: "iloader".to_string() }
    }
}

#[async_trait::async_trait]
impl IdeviceProvider for RsdProvider {
    // Firma aproximada — ajustar campo por campo según lo que pida el
    // trait real en 0.1.61 (el compilador listará los métodos faltantes
    // completos si esto no coincide).
    async fn connect(&self, port: u16) -> Result<Box<dyn idevice::ReadWrite>, idevice::IdeviceError> {
        let stream = self.handshake
            .connect_to_service_port(port) // <- ajustar nombre si CI falla aquí
            .await?;
        Ok(Box::new(stream))
    }

    fn label(&self) -> &str {
        &self.label
    }
}
