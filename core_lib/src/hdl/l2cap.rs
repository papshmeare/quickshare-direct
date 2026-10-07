// BLE L2CAP connection-oriented channel to a Quick Share receiver (Nearby Connections' BLE_L2CAP
// medium): much faster than the GATT/weave socket, and the link comes up sooner. The receiver
// advertises its PSM in the extra fields of its 0xFEF3 advertisement (`BleReceiver::psm`).
//
// On the channel every message is [4-byte big-endian length][payload]
// (google/nearby GNCBLEL2CAPConnection, mediums/ble/ble_l2cap_packet):
//   us → [0x03] request data connection; receiver → [0x17] ready (0x18: failure);
//   then the BLE socket layer as over weave: control packets [00 00 00][SocketControlFrame]
//   (INTRODUCTION first, PACKET_ACKNOWLEDGEMENT for each data packet received) and data packets
//   [service id hash fc9f5e][bytes of the [len][OfflineFrame] stream].

use std::time::Duration;

use anyhow::{anyhow, bail};
use bluer::l2cap::{SocketAddr, Stream};
use bluer::{Adapter, AddressType};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use crate::hdl::BleReceiver;

const INNER_NAME: &str = "L2cap";

const CMD_REQUEST_DATA_CONNECTION: u8 = 0x03;
const CMD_DATA_CONNECTION_READY: u8 = 0x17;
const CMD_DATA_CONNECTION_FAILURE: u8 = 0x18;
const QS_SVC_HASH: [u8; 3] = [0xfc, 0x9f, 0x5e];
const CONTROL_PREFIX: [u8; 3] = [0, 0, 0];
const MAX_MESSAGE: usize = 5 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// SocketControlFrame { type: INTRODUCTION, introduction { service_id_hash, socket_version: V2 } }
const INTRODUCTION: [u8; 11] = [0x08, 0x01, 0x12, 0x07, 0x0a, 0x03, 0xfc, 0x9f, 0x5e, 0x10, 0x02];
const CTRL_DISCONNECTION: u8 = 2;

/// SocketControlFrame { type: PACKET_ACKNOWLEDGEMENT, packet_acknowledgement { hash, size } }.
fn ack_frame(received: usize) -> Vec<u8> {
    let mut size = Vec::new();
    let mut v = received as u64;
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            size.push(b);
            break;
        }
        size.push(b | 0x80);
    }
    let mut inner = vec![0x0a, 0x03, 0xfc, 0x9f, 0x5e, 0x10];
    inner.extend(size);
    let mut f = CONTROL_PREFIX.to_vec();
    f.extend([0x08, 0x03, 0x22, inner.len() as u8]);
    f.extend(inner);
    f
}

async fn write_message(s: &mut (impl AsyncWriteExt + Unpin), payload: &[u8]) -> std::io::Result<()> {
    let mut m = Vec::with_capacity(4 + payload.len());
    m.extend((payload.len() as u32).to_be_bytes());
    m.extend(payload);
    s.write_all(&m).await
}

async fn read_message(s: &mut (impl AsyncReadExt + Unpin)) -> Result<Vec<u8>, anyhow::Error> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_MESSAGE {
        bail!("bad L2CAP message length {len}");
    }
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Opens the receiver's L2CAP channel and returns one side of a duplex carrying the
/// `[len][OfflineFrame]` stream, ready for an `OutboundRequest` (like `weave_connect`).
pub async fn l2cap_connect(adapter: &Adapter, rx: &BleReceiver) -> Result<DuplexStream, anyhow::Error> {
    let psm = rx.psm.ok_or_else(|| anyhow!("receiver advertises no L2CAP PSM"))?;
    let addr_type = adapter
        .device(rx.address)?
        .address_type()
        .await
        .unwrap_or(AddressType::LeRandom);
    info!("{INNER_NAME}: connecting to {} PSM {psm}", rx.address);
    let mut s = tokio::time::timeout(
        CONNECT_TIMEOUT,
        Stream::connect(SocketAddr::new(rx.address, addr_type, psm)),
    )
    .await
    .map_err(|_| anyhow!("L2CAP connection timed out"))??;
    // A connect on an LE socket can return before the channel is up; it's usable once it has
    // a send MTU.
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    while s.as_ref().send_mtu().is_err() {
        if tokio::time::Instant::now() >= deadline {
            bail!("L2CAP connection timed out");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    write_message(&mut s, &[CMD_REQUEST_DATA_CONNECTION]).await?;
    let answer = tokio::time::timeout(Duration::from_secs(10), read_message(&mut s))
        .await
        .map_err(|_| anyhow!("no answer to the L2CAP data connection request"))??;
    match answer.first() {
        Some(&CMD_DATA_CONNECTION_READY) => {}
        Some(&CMD_DATA_CONNECTION_FAILURE) => bail!("receiver refused the L2CAP data connection"),
        other => bail!("unexpected L2CAP answer {other:?}"),
    }
    let mut intro = CONTROL_PREFIX.to_vec();
    intro.extend(INTRODUCTION);
    write_message(&mut s, &intro).await?;
    info!("{INNER_NAME}: data connection ready");

    let (or_side, ours) = tokio::io::duplex(256 * 1024);
    tokio::spawn(async move {
        let (mut l2_rd, mut l2_wr) = tokio::io::split(s);
        let (mut rd, mut wr) = tokio::io::split(ours);
        // Phone → us: acknowledge data packets, drop control packets, forward the data.
        let (ack_tx, mut ack_rx) = tokio::sync::mpsc::unbounded_channel::<usize>();
        let inbound = async move {
            loop {
                let msg = match read_message(&mut l2_rd).await {
                    Ok(m) => m,
                    Err(e) => {
                        debug!("{INNER_NAME}: read ended: {e}");
                        break;
                    }
                };
                if msg.len() < 3 {
                    continue;
                }
                if msg[..3] == CONTROL_PREFIX {
                    if msg.len() >= 5 && msg[3] == 0x08 && msg[4] == CTRL_DISCONNECTION {
                        debug!("{INNER_NAME}: peer DISCONNECTION");
                        break;
                    }
                    continue;
                }
                let _ = ack_tx.send(msg.len() - 3);
                if wr.write_all(&msg[3..]).await.is_err() {
                    break;
                }
            }
        };
        // Us → phone: each [len][frame] the sender writes becomes one data packet; acks go out
        // between them.
        let outbound = async move {
            let mut buf: Vec<u8> = Vec::new();
            let mut rbuf = vec![0u8; 64 * 1024];
            loop {
                tokio::select! {
                    n = rd.read(&mut rbuf) => {
                        let n = match n { Ok(0) | Err(_) => break, Ok(n) => n };
                        buf.extend_from_slice(&rbuf[..n]);
                        while buf.len() >= 4 {
                            let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                            if buf.len() < 4 + len {
                                break;
                            }
                            let mut msg = QS_SVC_HASH.to_vec();
                            msg.extend(buf.drain(..4 + len));
                            if write_message(&mut l2_wr, &msg).await.is_err() {
                                return;
                            }
                        }
                    }
                    Some(size) = ack_rx.recv() => {
                        if write_message(&mut l2_wr, &ack_frame(size)).await.is_err() {
                            return;
                        }
                    }
                }
            }
        };
        tokio::select! {
            _ = inbound => {}
            _ = outbound => {}
        }
        info!("{INNER_NAME}: channel closed");
    });
    Ok(or_side)
}
