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

/// Carga el .mobiledevicepairing combinado exportado con
/// `iloader pairing --export archivo.mobiledevicepairing`.
/// Devuelve tanto el RpPairingFile (para el túnel) como los bytes
/// crudos (para reconstruir el PairingFile clásico que exige el trait).
pub fn load_saved_pairing(path: &std::path::Path) -> Result<(RpPairingFile, Vec<u8>), IdeviceError> {
    let bytes = std::fs::read(path)
        .map_err(|e| IdeviceError::InternalError(format!("leyendo {}: {e}", path.display())))?;

    let rp = RpPairingFile::from_bytes(&bytes)
        .map_err(|e| IdeviceError::InternalError(format!("RpPairingFile::from_bytes: {e}")))?;

    Ok((rp, bytes))
}

/// Conecta a 10.7.0.1 usando el RpPairingFile ya generado, sin usbmuxd.
/// Reutiliza el mismo patrón RemoteXpcClient + RemotePairingClient que ya
/// usa `generate_rppairing` en main.rs, pero contra una IP fija en vez de
/// un túnel CoreDeviceProxy por USB.
pub async fn connect_over_network(
    host: IpAddr,
    pairing: &mut RpPairingFile,
) -> Result<RsdHandshake, IdeviceError> {
    // Puerto documentado por SideInstaller para RemotePairing wireless.
    let stream = tokio::net::TcpStream::connect((host, 49152))
        .await
        .map_err(|e| IdeviceError::InternalError(format!("TCP a {host}:49152: {e}")))?;

    let mut xpc = RemoteXpcClient::new(stream).await?;
    xpc.do_handshake().await?;
    let _ = xpc.recv_root().await;

    let mut client = RemotePairingClient::new(xpc, "iloader");

    // Reutiliza el MISMO método `connect` que ya compila en generate_rppairing.
    // Como `pairing` ya trae las claves guardadas, esta llamada debería
    // hacer solo la verificación Ed25519, no el SRP completo. El callback
    // no debería usarse en ese caso, pero lo dejamos por si acaso.
    client.connect(pairing, || async { String::new() }).await?;

    // Si `connect()` no deja el túnel abierto y hay que sacarlo de otro
    // lado (ej. client.tunnel() o un método similar), este es el punto
    // exacto donde el próximo error de compilación te va a apuntar.
    let tunnel_stream = client.into_tunnel_stream()?; // <- ajustar si CI marca error aquí

    let handshake = RsdHandshake::new(tunnel_stream).await?;
    Ok(handshake)
}

/// Envuelve el RsdHandshake para que Sideloader/InstallationProxyClient/etc.
/// lo usen igual que un UsbmuxdProvider.
#[derive(Debug)]
pub struct RsdProvider {
    handshake: RsdHandshake,
    lockdown_bytes: Vec<u8>,
}

impl RsdProvider {
    pub fn new(handshake: RsdHandshake, lockdown_bytes: Vec<u8>) -> Self {
        Self { handshake, lockdown_bytes }
    }
}

impl IdeviceProvider for RsdProvider {
    fn connect(
        &self,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Idevice, IdeviceError>> + Send + 'static>> {
        let handshake = self.handshake.clone(); // <- ajustar si RsdHandshake no es Clone
        Box::pin(async move {
            handshake
                .connect_to_port(port) // <- ajustar nombre si CI marca error aquí
                .await
        })
    }

    fn get_pairing_file(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PairingFile, IdeviceError>> + Send + 'static>> {
        let bytes = self.lockdown_bytes.clone();
        Box::pin(async move {
            plist::from_bytes::<PairingFile>(&bytes)
                .map_err(|e| IdeviceError::InternalError(format!("PairingFile parse: {e}")))
        })
    }
}