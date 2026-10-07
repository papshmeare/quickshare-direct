// Sending files to a phone without a shared network: BLE discovery + weave socket for the
// handshake, then whatever upgrade the phone offers (its Wi-Fi Direct group via the join helper,
// or the shared Wi-Fi). Progress goes out as `ChannelMessage`s with id `send_id`, kind Outbound.

use std::time::Duration;

use anyhow::anyhow;
use rand::Rng;
use tokio::sync::broadcast::Sender;
use tokio_util::sync::CancellationToken;

use crate::channel::ChannelMessage;
use crate::errors::AppError;
use crate::hdl::{
    BleReceiver, MigratableStream, OutboundPayload, OutboundRequest, TransferState, l2cap_connect,
    weave_connect,
};
use crate::utils::{DeviceType, RemoteDeviceInfo};

/// A random endpoint id (4 characters from Nearby Connections' alphabet).
fn random_endpoint_id() -> [u8; 4] {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    std::array::from_fn(|_| ALPHA[rng.random_range(0..ALPHA.len())])
}

/// Send `files` to `receiver` (found with `discover_receivers`). Returns the final state:
/// Finished, Rejected (declined on the phone), Cancelled (`ctk`) or Disconnected.
pub async fn send_files_ble(
    adapter: &bluer::Adapter,
    receiver: &BleReceiver,
    files: Vec<String>,
    send_id: String,
    sender: Sender<ChannelMessage>,
    ctk: CancellationToken,
) -> Result<TransferState, anyhow::Error> {
    // Bluetooth link: the receiver's L2CAP channel when it advertises one (faster, sets up
    // sooner), else (or if that fails) its GATT/weave socket. QSD_BLE_L2CAP=0 forces weave.
    let connect = async {
        let l2cap = std::env::var("QSD_BLE_L2CAP").map(|v| v != "0").unwrap_or(true);
        if l2cap && receiver.psm.is_some() {
            match l2cap_connect(adapter, receiver).await {
                Ok(s) => return Ok((s, true)),
                Err(e) => warn!("L2CAP to the receiver failed ({e}); using GATT"),
            }
        }
        weave_connect(adapter, receiver.address).await.map(|s| (s, false))
    };
    let (stream, over_l2cap) = tokio::select! {
        s = connect => s?,
        _ = ctk.cancelled() => return Ok(TransferState::Cancelled),
    };
    let mut or = OutboundRequest::new(
        random_endpoint_id(),
        MigratableStream::Ble(stream),
        send_id,
        sender,
        OutboundPayload::Files(files),
        RemoteDeviceInfo {
            name: receiver.name.clone().unwrap_or_default(),
            device_type: DeviceType::Phone,
        },
    );
    or.set_hold_files_for_upgrade(true);
    or.set_slow_link(true);
    if over_l2cap {
        let chunk = std::env::var("QSD_L2CAP_CHUNK").ok().and_then(|c| c.parse().ok());
        or.set_chunk_size(chunk.unwrap_or(L2CAP_CHUNK_SIZE));
    }
    or.send_connection_request().await?;
    or.send_ukey2_client_init().await?;

    // After Accept the receiver usually offers its Wi-Fi link within a few seconds (sometimes
    // before its Accept, sometimes after); hold the files that long, then stream them anyway.
    let mut upgrade_settled = false;
    let mut held_since: Option<tokio::time::Instant> = None;
    loop {
        if or.files_held() {
            let since = *held_since.get_or_insert_with(tokio::time::Instant::now);
            if upgrade_settled || since.elapsed() >= UPGRADE_WAIT {
                if !upgrade_settled {
                    info!("no upgrade offer; sending over Bluetooth");
                }
                let r = tokio::select! {
                    r = or.send_held_files() => r,
                    _ = ctk.cancelled() => return Ok(TransferState::Cancelled),
                };
                if let Err(e) = r {
                    return finish(&or, e);
                }
                continue;
            }
        }
        let wait = match held_since {
            Some(t) if or.files_held() => UPGRADE_WAIT.saturating_sub(t.elapsed()),
            _ => Duration::from_secs(3600),
        };
        let r = tokio::select! {
            r = tokio::time::timeout(wait, or.handle()) => match r {
                Ok(r) => r,
                Err(_) => continue, // upgrade wait over; the files go out above
            },
            _ = ctk.cancelled() => {
                // Dropping `or` closes the link and leaves a joined network.
                return Ok(TransferState::Cancelled);
            }
        };
        if let Err(e) = r {
            return finish(&or, e);
        }
        if let Some(offer) = or.take_bwu_offer() {
            if let Err(e) = or.do_bwu(offer).await {
                warn!("upgrade failed, continuing over Bluetooth: {e}");
            }
            upgrade_settled = true;
        }
    }
}

/// File data per frame over a BLE L2CAP channel. Each frame must fit one L2CAP SDU (the Pixel
/// drops the link on a 64 KB chunk); 16 KB measured 46 KB/s against 38 KB/s with Nearby's 1 KB.
const L2CAP_CHUNK_SIZE: usize = 16 * 1024;

/// How long to hold the files after Accept for the receiver's upgrade offer.
const UPGRADE_WAIT: Duration = Duration::from_secs(10);

fn finish(
    or: &OutboundRequest<MigratableStream>,
    e: anyhow::Error,
) -> Result<TransferState, anyhow::Error> {
    let state = or.state.state.clone();
    match state {
        TransferState::Finished | TransferState::Rejected | TransferState::Cancelled => Ok(state),
        _ if matches!(e.downcast_ref(), Some(AppError::NotAnError)) => {
            Ok(TransferState::Disconnected)
        }
        _ => Err(anyhow!("{e} (in {state:?})")),
    }
}
