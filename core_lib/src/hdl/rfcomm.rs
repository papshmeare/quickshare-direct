// Quick Share over Bluetooth Classic (Nearby Connections "BLUETOOTH" medium).
//
// When a phone picks us as a target it first tries Bluetooth Classic: it connects an insecure
// RFCOMM socket to the bluetooth MAC carried in our BLE advertisement (see `blea.rs`), looking
// up the service by UUID over SDP. The UUID is the name-based (v3) UUID of the service id
// "NearbySharing". On that socket the phone speaks exactly the same `[len(4)][OfflineFrame]`
// stream as over Wi-Fi LAN TCP (Nearby's BluetoothEndpointChannel is a BaseEndpointChannel), so
// the regular inbound handshake runs on it unchanged. Afterwards the connection is upgraded to
// Wi-Fi (bandwidth upgrade), exactly like the BLE weave path in `gatt.rs`.

use bluer::rfcomm::{Profile, Role};
use bluer::{Session, Uuid};
use futures::StreamExt;
use tokio::sync::broadcast::Sender;
use tokio_util::sync::CancellationToken;

use crate::channel::ChannelMessage;
use crate::errors::AppError;
use crate::hdl::InboundRequest;

const INNER_NAME: &str = "RfcommServer";

/// SDP service UUID the phone connects to: UUID v3 (MD5, name-based) of "NearbySharing".
/// Seen in the phone log as `connectSocket: ... uuid=a82efa21-ae5c-3dde-9bbc-f16da7b16c5a`.
pub const NEARBY_SHARING_RFCOMM_UUID: &str = "a82efa21-ae5c-3dde-9bbc-f16da7b16c5a";

pub struct RfcommServer {
    session: Session,
    sender: Sender<ChannelMessage>,
    tcp_port: u16,
}

impl RfcommServer {
    pub async fn new(sender: Sender<ChannelMessage>, tcp_port: u16) -> Result<Self, anyhow::Error> {
        let session = Session::new().await?;
        session.default_adapter().await?.set_powered(true).await?;
        Ok(Self {
            session,
            sender,
            tcp_port,
        })
    }

    pub async fn run(&self, ctk: CancellationToken) -> Result<(), anyhow::Error> {
        let uuid: Uuid = NEARBY_SHARING_RFCOMM_UUID.parse()?;
        let profile = Profile {
            uuid,
            name: Some("NearbySharing".to_string()),
            service: Some(uuid),
            role: Some(Role::Server),
            // Nearby uses insecure RFCOMM sockets: no pairing, no authorization prompt.
            require_authentication: Some(false),
            require_authorization: Some(false),
            ..Default::default()
        };
        let mut handle = self.session.register_profile(profile).await?;
        info!("{INNER_NAME}: listening for Bluetooth Classic connections (RFCOMM {uuid})");

        let mut conn_no: u32 = 0;
        loop {
            tokio::select! {
                _ = ctk.cancelled() => {
                    info!("{INNER_NAME}: tracker cancelled, returning");
                    return Ok(());
                }
                req = handle.next() => {
                    let Some(req) = req else {
                        warn!("{INNER_NAME}: profile handle closed");
                        return Ok(());
                    };
                    let device = req.device();
                    let stream = match req.accept() {
                        Ok(s) => s,
                        Err(e) => {
                            warn!("{INNER_NAME}: accepting connection from {device} failed: {e}");
                            continue;
                        }
                    };
                    conn_no += 1;
                    let id = format!("bt-{conn_no}");
                    info!("{INNER_NAME}: connection from {device} ({id})");
                    let sender = self.sender.clone();
                    let tcp_port = self.tcp_port;
                    tokio::spawn(async move { serve(stream, id, sender, tcp_port).await });
                }
            }
        }
    }
}

/// Runs the inbound handshake over the RFCOMM stream (bridged through an in-memory duplex so
/// the stream can later be swapped for the upgraded Wi-Fi socket).
async fn serve(
    mut stream: bluer::rfcomm::Stream,
    id: String,
    sender: Sender<ChannelMessage>,
    tcp_port: u16,
) {
    let (inbound_side, mut bt_side) = tokio::io::duplex(64 * 1024);
    let bridge_id = id.clone();
    let bridge = tokio::spawn(async move {
        if let Err(e) = tokio::io::copy_bidirectional(&mut stream, &mut bt_side).await {
            debug!("{INNER_NAME}: {bridge_id}: bridge ended: {e}");
        }
    });

    let mut ir = InboundRequest::new(
        crate::hdl::MigratableStream::Ble(inbound_side),
        id.clone(),
        sender,
    );
    ir.set_bwu_tcp_port(tcp_port);
    loop {
        if let Err(e) = ir.handle().await {
            if !matches!(e.downcast_ref(), Some(AppError::NotAnError)) {
                debug!("{INNER_NAME}: {id}: inbound ended: {e}");
            }
            break;
        }
        // Once the encrypted connection is up, move the payload to a faster medium.
        if ir.take_bwu_pending() {
            if let Err(e) = ir.do_bwu().await {
                warn!("{INNER_NAME}: {id}: bandwidth upgrade failed, staying on Bluetooth: {e}");
            }
        }
    }
    bridge.abort();
    info!("{INNER_NAME}: {id}: session ended");
}
