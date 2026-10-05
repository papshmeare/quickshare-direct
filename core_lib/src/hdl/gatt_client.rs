use std::collections::HashMap;
use std::time::Duration;

use anyhow::anyhow;
use bluer::gatt::remote::Characteristic;
use bluer::{Adapter, Address, Device, Uuid, UuidExt};
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

const INNER_NAME: &str = "WeaveClient";

// Same service/characteristics as the receiver side (gatt.rs), seen from the central.
const QS_GATT_SERVICE: u16 = 0xFEF3;
const QS_WEAVE_TO_PERIPHERAL: &str = "00000100-0004-1000-8000-001a11000101";
const QS_WEAVE_FROM_PERIPHERAL: &str = "00000100-0004-1000-8000-001a11000102";

const WEAVE_CONTROL: u8 = 0b1000_0000;
const WEAVE_CMD_MASK: u8 = 0b0000_1111;
const WEAVE_FIRST_BIT: u8 = 0b0000_1000;
const WEAVE_LAST_BIT: u8 = 0b0000_0100;
const WEAVE_CMD_CONN_REQUEST: u8 = 0;
const WEAVE_CMD_CONN_CONFIRM: u8 = 1;
const WEAVE_CMD_ERROR: u8 = 2;
const WEAVE_PROTOCOL_VERSION: u16 = 1;
// The phone's (private) GATT server keeps the default ATT MTU for its indications even after a
// 517 exchange: anything over 20 bytes fails with status 133 on its side (Pixel 10, Oct 2026).
// So ask for the minimum weave packet; QSD_WEAVE_PACKET overrides it for experiments.
const WEAVE_MAX_PACKET: u16 = 20;

const QS_SVC_HASH: [u8; 3] = [0xfc, 0x9f, 0x5e];
// SocketControlFrame { type: INTRODUCTION, introduction { service_id_hash: fc9f5e,
// socket_version: V2 } } (google/nearby mediums/ble/ble_packet.cc).
const SOCKET_CTRL_INTRODUCTION: [u8; 11] =
    [0x08, 0x01, 0x12, 0x07, 0x0a, 0x03, 0xfc, 0x9f, 0x5e, 0x10, 0x02];
const SOCKET_CTRL_DISCONNECTION: u8 = 2;

/// A Quick Share receiver found over BLE (its 0xFEF3 advertisement).
#[derive(Debug, Clone)]
pub struct BleReceiver {
    pub address: Address,
    pub endpoint_id: [u8; 4],
    pub name: Option<String>,
    pub bt_mac: Option<[u8; 6]>,
}

/// Parses a 0xFEF3 BleAdvertisement for Nearby Sharing (format: docs/BLE_RECEIVER_DISCOVERY.md 4.1).
pub fn parse_receiver_advertisement(address: Address, data: &[u8]) -> Option<BleReceiver> {
    // 48 | fc9f5e | len(4) | 23 fc9f5e eid(4) info_len info... mac(6)
    if data.len() < 8 || data[1..4] != QS_SVC_HASH {
        return None;
    }
    let len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let inner = data.get(8..8 + len)?;
    if inner.len() < 9 || inner[1..4] != QS_SVC_HASH {
        return None;
    }
    let endpoint_id: [u8; 4] = inner[4..8].try_into().ok()?;
    let info_len = inner[8] as usize;
    let info = inner.get(9..9 + info_len)?;
    // endpoint_info: header(1) salt+key(16) name_len(1) name
    let name = info
        .get(17)
        .and_then(|&n| info.get(18..18 + n as usize))
        .map(|n| String::from_utf8_lossy(n).into_owned());
    let bt_mac = inner
        .get(9 + info_len..9 + info_len + 6)
        .and_then(|m| m.try_into().ok());
    Some(BleReceiver {
        address,
        endpoint_id,
        name,
        bt_mac,
    })
}

/// Scans until a receiver advertisement matching `name` (substring, any if None) shows up.
pub async fn discover_receiver(
    adapter: &Adapter,
    name: Option<&str>,
    timeout: Duration,
) -> Result<BleReceiver, anyhow::Error> {
    let svc = Uuid::from_u16(QS_GATT_SERVICE);
    // Keep discovery running while we poll; BlueZ's cache also lists devices from earlier
    // scans (whose random addresses may be gone), so only take ones seen now (RSSI set).
    let _events = adapter.discover_devices().await?;
    let check = async |addr: Address| -> Option<BleReceiver> {
        let dev = adapter.device(addr).ok()?;
        dev.rssi().await.ok()??;
        let sd: HashMap<Uuid, Vec<u8>> = dev.service_data().await.ok()??;
        let r = parse_receiver_advertisement(addr, sd.get(&svc)?)?;
        match (name, &r.name) {
            (None, _) => Some(r),
            (Some(want), Some(n)) if n.to_lowercase().contains(&want.to_lowercase()) => Some(r),
            _ => None,
        }
    };
    tokio::time::timeout(timeout, async {
        loop {
            for addr in adapter.device_addresses().await? {
                if let Some(r) = check(addr).await {
                    return Ok::<_, anyhow::Error>(r);
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("no Quick Share receiver found over BLE"))?
}

async fn find_weave_chars(dev: &Device) -> Result<(Characteristic, Characteristic), anyhow::Error> {
    let to_p: Uuid = QS_WEAVE_TO_PERIPHERAL.parse()?;
    let from_p: Uuid = QS_WEAVE_FROM_PERIPHERAL.parse()?;
    for s in dev.services().await? {
        if s.uuid().await? != Uuid::from_u16(QS_GATT_SERVICE) {
            continue;
        }
        let (mut w, mut n) = (None, None);
        for c in s.characteristics().await? {
            let u = c.uuid().await?;
            if u == to_p {
                w = Some(c);
            } else if u == from_p {
                n = Some(c);
            }
        }
        if let (Some(w), Some(n)) = (w, n) {
            return Ok((w, n));
        }
    }
    Err(anyhow!("no weave characteristics on {}", dev.address()))
}

/// Connects to the receiver's weave socket (we are the GATT client) and returns one side of a
/// duplex carrying the `[len][OfflineFrame]` stream, ready for an `OutboundRequest`.
pub async fn weave_connect(adapter: &Adapter, addr: Address) -> Result<DuplexStream, anyhow::Error> {
    let dev = adapter.device(addr)?;
    // The receiver's GATT server is private to the advertising set the connection came in
    // through: a link left over from an earlier advertisement doesn't show the 0xFEF3 service.
    if dev.is_connected().await? {
        let _ = dev.disconnect().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    info!("{INNER_NAME}: connecting to {addr}");
    dev.connect().await?;
    for _ in 0..100 {
        if dev.is_services_resolved().await? {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (write_c, notify_c) = find_weave_chars(&dev).await?;
    let notes = notify_c.notify().await?;
    let mut notes = Box::pin(notes);

    // 1. Weave connection handshake: we send CONN_REQUEST (counter 0), the peer confirms.
    let mut req = [
        WEAVE_CONTROL | WEAVE_CMD_CONN_REQUEST,
        (WEAVE_PROTOCOL_VERSION >> 8) as u8,
        (WEAVE_PROTOCOL_VERSION & 0xff) as u8,
        (WEAVE_PROTOCOL_VERSION >> 8) as u8,
        (WEAVE_PROTOCOL_VERSION & 0xff) as u8,
        (WEAVE_MAX_PACKET >> 8) as u8,
        (WEAVE_MAX_PACKET & 0xff) as u8,
    ];
    let max_packet: u16 = std::env::var("QSD_WEAVE_PACKET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(WEAVE_MAX_PACKET);
    req[5..7].copy_from_slice(&max_packet.to_be_bytes());
    write_c.write(&req).await?;
    let selected = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(pkt) = notes.next().await {
            debug!("{INNER_NAME}: rx {}", hex::encode(&pkt));
            if pkt.len() >= 5
                && pkt[0] & WEAVE_CONTROL != 0
                && pkt[0] & WEAVE_CMD_MASK == WEAVE_CMD_CONN_CONFIRM
            {
                return Ok(u16::from_be_bytes([pkt[3], pkt[4]]));
            }
        }
        Err(anyhow!("notify stream ended before CONN_CONFIRM"))
    })
    .await
    .map_err(|_| anyhow!("no weave CONN_CONFIRM"))??;
    let max_payload = (selected as usize).saturating_sub(1).max(19);
    info!("{INNER_NAME}: weave: connected (selected={selected})");

    let (or_side, weave_side) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let mut send_counter: u8 = 1; // CONN_REQUEST was counter 0
        let mut send_msg = async |msg: &[u8]| -> Result<(), anyhow::Error> {
            let mut off = 0;
            while off < msg.len() {
                let end = (off + max_payload).min(msg.len());
                let mut h = (send_counter & 0x07) << 4;
                if off == 0 {
                    h |= WEAVE_FIRST_BIT;
                }
                if end == msg.len() {
                    h |= WEAVE_LAST_BIT;
                }
                let mut wp = Vec::with_capacity(1 + end - off);
                wp.push(h);
                wp.extend_from_slice(&msg[off..end]);
                write_c.write(&wp).await?;
                send_counter = send_counter.wrapping_add(1);
                off = end;
            }
            Ok(())
        };

        // 2. BLE socket INTRODUCTION (control packet), then the data stream.
        let mut intro = vec![0, 0, 0];
        intro.extend_from_slice(&SOCKET_CTRL_INTRODUCTION);
        if let Err(e) = send_msg(&intro).await {
            error!("{INNER_NAME}: weave: introduction failed: {e}");
            return;
        }

        let (mut rd, mut wr) = tokio::io::split(weave_side);
        let mut reasm: Vec<u8> = Vec::new();
        let mut out_buf: Vec<u8> = Vec::new();
        let mut rbuf = [0u8; 4096];
        loop {
            tokio::select! {
                pkt = notes.next() => {
                    let Some(pkt) = pkt else { break; };
                    if pkt.is_empty() { continue; }
                    let hdr = pkt[0];
                    if hdr & WEAVE_CONTROL != 0 {
                        if hdr & WEAVE_CMD_MASK == WEAVE_CMD_ERROR {
                            debug!("{INNER_NAME}: weave: peer sent ERROR, closing");
                            break;
                        }
                        continue;
                    }
                    reasm.extend_from_slice(&pkt[1..]);
                    if hdr & WEAVE_LAST_BIT == 0 { continue; }
                    let msg = std::mem::take(&mut reasm);
                    if msg.len() < 3 { continue; }
                    if msg[0..3] == [0, 0, 0] {
                        debug!("{INNER_NAME}: weave: control {}", hex::encode(&msg));
                        if msg.len() >= 5 && msg[3] == 0x08 && msg[4] == SOCKET_CTRL_DISCONNECTION {
                            break;
                        }
                        continue;
                    }
                    if wr.write_all(&msg[3..]).await.is_err() { break; }
                }
                r = rd.read(&mut rbuf) => {
                    match r {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            out_buf.extend_from_slice(&rbuf[..n]);
                            while out_buf.len() >= 4 {
                                let len = u32::from_be_bytes([out_buf[0], out_buf[1], out_buf[2], out_buf[3]]) as usize;
                                if out_buf.len() < 4 + len { break; }
                                let mut msg = QS_SVC_HASH.to_vec();
                                msg.extend(out_buf.drain(0..4 + len));
                                if let Err(e) = send_msg(&msg).await {
                                    error!("{INNER_NAME}: weave: write failed: {e}");
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
        info!("{INNER_NAME}: weave: session ended");
        let _ = dev.disconnect().await;
    });

    Ok(or_side)
}
