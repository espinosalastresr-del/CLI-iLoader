//! network_transport.rs — transporte alternativo sin usbmuxd/USB.
//! Reutiliza el .mobiledevicepairing combinado (lockdown + RpPairingFile)
//! que ya genera `iloader pairing --export <ruta>`, contra 10.7.0.1
//! (LocalDevVPN) en vez de usbmuxd.

use std::{future::Future, net::IpAddr, pin::Pin};

use idevice::{
    Idevice, RemoteXpcClient,
    pairing_file::PairingFile,
    provider::IdeviceProvider,
    remote_pairing::{RemotePairingClient, RpPairingFile},
    rsd::RsdHandshake,
    IdeviceError,
};

pub const LOCALDEVVPN_ADDR: &str = "10.7.0.1";

/// Carga el .mobiledevicepairing combinado. Devuelve el RpPairingFile
/// (para el túnel) y los bytes crudos (para reconstruir el PairingFile
/// clásico que exige el trait IdeviceProvider vía get_pairing_file).
pub fn load_saved_pairing(path: &std::path::Path) -> Result<(RpPairingFile, Vec<u8>), IdeviceError> {
    let bytes = std::fs::read(path)
        .map_err(|e| IdeviceError::InternalError(format!("leyendo {}: {e}", path.display())))?;
    let rp = RpPairingFile::from_bytes(&bytes)
        .map_err(|e| IdeviceError::InternalError(format!("RpPairingFile::from_bytes: {e}")))?;
    Ok((rp, bytes))
}

/// Conecta a 10.7.0.1 usando el RpPairingFile ya generado, sin usbmuxd.
///
/// NOTA: `into_tunnel_stream()` fue mi segunda adivinanza fallida en CI.
/// En vez de una tercera adivinanza, este build.yml ahora vuelca el código
/// fuente real de idevice 0.1.65 (remote_pairing, rsd.rs, provider.rs) si
/// la compilación falla, así que si esto vuelve a fallar aquí, pega ese
/// dump y lo cierro sin más intentos a ciegas.
pub async fn connect_over_network(
    host: IpAddr,
    pairing: &mut RpPairingFile,
) -> Result<RsdHandshake, IdeviceError> {
    let stream = tokio::net::TcpStream::connect((host, 49152))
        .await
        .map_err(|e| IdeviceError::InternalError(format!("TCP a {host}:49152: {e}")))?;

    let mut xpc = RemoteXpcClient::new(stream).await?;
    xpc.do_handshake().await?;
    let _ = xpc.recv_root().await;

    let mut client = RemotePairingClient::new(xpc, "iloader");

    // Se asume que connect() devuelve el stream/túnel ya negociado
    // (a diferencia de generate_rppairing, que descarta el resultado
    // porque solo necesitaba generar el pairing file, no el túnel).
    let tunnel_stream = client.connect(pairing, || async { String::new() }).await?;

    let handshake = RsdHandshake::new(tunnel_stream).await?;
    Ok(handshake)
}

/// Envuelve el RsdHandshake para que Sideloader/InstallationProxyClient/etc.
/// lo usen igual que un UsbmuxdProvider.
#[derive(Debug)]
pub struct RsdProvider {
    handshake: RsdHandshake,
    lockdown_bytes: Vec<u8>,
    label: String,
}

impl RsdProvider {
    pub fn new(handshake: RsdHandshake, lockdown_bytes: Vec<u8>) -> Self {
        Self { handshake, lockdown_bytes, label: "iloader".to_string() }
    }
}

impl IdeviceProvider for RsdProvider {
    fn connect(
        &self,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Idevice, IdeviceError>> + Send + 'static>> {
        let handshake = self.handshake.clone(); // <- si RsdHandshake no es Clone, CI lo dirá
        Box::pin(async move { handshake.connect(port).await }) // <- mismo método que ya usa `handle.connect(port)` en generate_rppairing
    }

    fn get_pairing_file(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PairingFile, IdeviceError>> + Send + 'static>> {
        let bytes = self.lockdown_bytes.clone();
        Box::pin(async move {
            plist::from_reader_xml::<_, PairingFile>(std::io::Cursor::new(&bytes))
                .map_err(|e| IdeviceError::InternalError(format!("PairingFile parse: {e}")))
        })
    }

    fn label(&self) -> &str {
        &self.label
    }
}
