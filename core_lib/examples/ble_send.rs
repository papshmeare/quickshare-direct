// Send test: finds a Quick Share receiver over BLE (0xFEF3 advertisement), connects to its
// weave GATT socket as the central and runs the outbound (sender) handshake over it.
//
//   cargo run --release --example ble_send -- <file> [name-substring]
#[macro_use]
extern crate log;

use std::time::Duration;

use rqs_lib::OutboundPayload;
use rqs_lib::channel::{ChannelMessage, Message};
use rqs_lib::hdl::{OutboundRequest, discover_receiver, weave_connect};
use rqs_lib::utils::{DeviceType, RemoteDeviceInfo};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::builder().parse_lossy("info,rqs_lib=debug"))
        .init();
    let args: Vec<String> = std::env::args().collect();
    let file = std::fs::canonicalize(args.get(1).expect("usage: ble_send <file> [name]"))?;
    let name = args.get(2).map(|s| s.as_str());

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    adapter.set_powered(true).await?;

    let rx = discover_receiver(&adapter, name, Duration::from_secs(30)).await?;
    info!(
        "found {:?} endpoint {} at {} (bt {:02x?})",
        rx.name,
        String::from_utf8_lossy(&rx.endpoint_id),
        rx.address,
        rx.bt_mac
    );
    let stream = weave_connect(&adapter, rx.address).await?;

    let (sender, mut events) = tokio::sync::broadcast::channel::<ChannelMessage>(64);
    tokio::spawn(async move {
        while let Ok(cm) = events.recv().await {
            if let Message::Client(mc) = &cm.msg {
                info!(
                    "EVENT state={:?} pin={:?}",
                    mc.state,
                    mc.metadata.as_ref().and_then(|m| m.pin_code.clone())
                );
            }
        }
    });

    let endpoint_id: [u8; 4] = rand_endpoint_id();
    let mut or = OutboundRequest::new(
        endpoint_id,
        stream,
        "ble-send".into(),
        sender,
        OutboundPayload::Files(vec![file.to_string_lossy().into_owned()]),
        RemoteDeviceInfo {
            name: rx.name.clone().unwrap_or_default(),
            device_type: DeviceType::Phone,
        },
    );
    or.send_connection_request().await?;
    or.send_ukey2_client_init().await?;
    loop {
        if let Err(e) = or.handle().await {
            info!("outbound ended: {e} ({:?})", or.state.state);
            break;
        }
    }
    Ok(())
}

fn rand_endpoint_id() -> [u8; 4] {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut id = [0u8; 4];
    for (i, b) in id.iter_mut().enumerate() {
        let r = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as usize;
        *b = ALPHA[(r / (i + 1) + i * 7) % ALPHA.len()];
    }
    id
}
