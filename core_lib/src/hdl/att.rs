// Minimal ATT client over our own L2CAP ATT socket (fixed CID 4), bypassing bluetoothd's GATT
// client for one connection.
//
// Why: bluetoothd exchanges the ATT MTU the moment an LE link comes up. A Pixel's private GATT
// server (the one behind Quick Share's advertising set) attaches to the connection a few ms later,
// misses the exchange and keeps the default MTU for its indications: anything over 20 bytes fails
// with status 133, so the weave socket ran at 20-byte packets. Android centrals ask for the MTU
// after service discovery; we do the same. When a client ATT socket exists for a link, the kernel
// doesn't hand the ATT channel to bluetoothd (l2cap_le_conn_ready: "Client ATT sockets should
// override the server one"), so the exchange timing is ours.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail};
use bluer::l2cap::{SeqPacket, Socket, SocketAddr};
use bluer::{Address, AddressType, Uuid};
use tokio::sync::{Mutex, mpsc, oneshot};

const INNER_NAME: &str = "Att";

const ATT_CID: u16 = 4;
const ATT_DEFAULT_MTU: u16 = 23;
const OUR_MTU: u16 = 517;
const TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

const OP_ERROR_RSP: u8 = 0x01;
const OP_MTU_REQ: u8 = 0x02;
const OP_MTU_RSP: u8 = 0x03;
const OP_FIND_INFO_REQ: u8 = 0x04;
const OP_READ_BY_TYPE_REQ: u8 = 0x08;
const OP_READ_BY_GROUP_REQ: u8 = 0x10;
const OP_WRITE_REQ: u8 = 0x12;
const OP_NOTIFICATION: u8 = 0x1b;
const OP_INDICATION: u8 = 0x1d;
const OP_CONFIRMATION: u8 = 0x1e;

const ERR_ATTRIBUTE_NOT_FOUND: u8 = 0x0a;
const ERR_REQUEST_NOT_SUPPORTED: u8 = 0x06;

const UUID_PRIMARY_SERVICE: u16 = 0x2800;
const UUID_CHARACTERISTIC: u16 = 0x2803;

/// Bluetooth base UUID with a 16-bit value.
fn uuid16(v: u16) -> Uuid {
    Uuid::from_u128(((v as u128) << 96) | 0x0000_0000_0000_1000_8000_0080_5f9b_34fb)
}

/// ATT UUIDs are little-endian, 2 or 16 bytes.
fn parse_uuid(b: &[u8]) -> Option<Uuid> {
    match b.len() {
        2 => Some(uuid16(u16::from_le_bytes([b[0], b[1]]))),
        16 => {
            let mut a: [u8; 16] = b.try_into().ok()?;
            a.reverse();
            Some(Uuid::from_bytes(a))
        }
        _ => None,
    }
}

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

#[derive(Debug, Clone)]
pub struct Characteristic {
    pub uuid: Uuid,
    pub properties: u8,
    pub value_handle: u16,
}

/// One ATT bearer to a peer. Requests are serialized (ATT allows one outstanding request);
/// notifications/indications arrive on the channel returned by `connect` as (handle, value).
pub struct AttClient {
    sock: Arc<SeqPacket>,
    pending: Arc<Mutex<Option<oneshot::Sender<Vec<u8>>>>>,
    req_lock: Mutex<()>,
}

impl AttClient {
    /// Opens an LE link to `addr` with our ATT socket on it.
    pub async fn connect(
        addr: Address,
        addr_type: AddressType,
    ) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<(u16, Vec<u8>)>), anyhow::Error> {
        let socket = Socket::<SeqPacket>::new_seq_packet()?;
        socket.bind(SocketAddr { addr: Address::any(), addr_type: AddressType::LePublic, psm: 0, cid: ATT_CID })?;
        let sock = tokio::time::timeout(
            TIMEOUT,
            socket.connect(SocketAddr { addr, addr_type, psm: 0, cid: ATT_CID }),
        )
        .await
        .map_err(|_| anyhow!("LE connection to {addr} timed out"))??;
        // A fixed-channel connect returns at once with the link still being set up; the socket
        // reports its MTU once connected.
        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        while sock.send_mtu().is_err() {
            if tokio::time::Instant::now() >= deadline {
                bail!("LE connection to {addr} timed out");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let sock = Arc::new(sock);
        let pending: Arc<Mutex<Option<oneshot::Sender<Vec<u8>>>>> = Arc::default();
        let (tx, rx) = mpsc::unbounded_channel();
        let client = Arc::new(Self {
            sock: sock.clone(),
            pending: pending.clone(),
            req_lock: Mutex::new(()),
        });
        tokio::spawn(Self::reader(sock, pending, tx));
        Ok((client, rx))
    }

    /// Reads PDUs: responses go to the pending request, notifications/indications to `tx`
    /// (indications confirmed), and the peer's own requests get a minimal answer (we serve no
    /// attributes) so its ATT layer doesn't stall.
    async fn reader(
        sock: Arc<SeqPacket>,
        pending: Arc<Mutex<Option<oneshot::Sender<Vec<u8>>>>>,
        tx: mpsc::UnboundedSender<(u16, Vec<u8>)>,
    ) {
        let mut buf = vec![0u8; OUR_MTU as usize + 8];
        loop {
            let n = match sock.recv(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let pdu = &buf[..n];
            let op = pdu[0];
            match op {
                OP_NOTIFICATION | OP_INDICATION if pdu.len() >= 3 => {
                    if op == OP_INDICATION {
                        let _ = sock.send(&[OP_CONFIRMATION]).await;
                    }
                    let _ = tx.send((le16(pdu, 1), pdu[3..].to_vec()));
                }
                OP_MTU_REQ => {
                    let mut rsp = vec![OP_MTU_RSP];
                    rsp.extend_from_slice(&OUR_MTU.to_le_bytes());
                    let _ = sock.send(&rsp).await;
                }
                // Responses (odd opcodes) complete the outstanding request.
                op if op & 1 == 1 && op != OP_INDICATION && op != OP_NOTIFICATION => {
                    if let Some(p) = pending.lock().await.take() {
                        let _ = p.send(pdu.to_vec());
                    }
                }
                // Commands (0x40 bit) need no answer.
                op if op & 0x40 != 0 => {}
                op => {
                    let err = match op {
                        OP_FIND_INFO_REQ | OP_READ_BY_TYPE_REQ | OP_READ_BY_GROUP_REQ | 0x06 => {
                            ERR_ATTRIBUTE_NOT_FOUND
                        }
                        _ => ERR_REQUEST_NOT_SUPPORTED,
                    };
                    let handle = if pdu.len() >= 3 { le16(pdu, 1) } else { 0 };
                    let mut rsp = vec![OP_ERROR_RSP, op];
                    rsp.extend_from_slice(&handle.to_le_bytes());
                    rsp.push(err);
                    let _ = sock.send(&rsp).await;
                }
            }
        }
        debug!("{INNER_NAME}: link closed");
    }

    /// Sends a request and waits for its response (or an Error Response, returned as Err).
    async fn request(&self, pdu: &[u8]) -> Result<Vec<u8>, anyhow::Error> {
        let _g = self.req_lock.lock().await;
        let (tx, rx) = oneshot::channel();
        *self.pending.lock().await = Some(tx);
        self.sock.send(pdu).await?;
        let rsp = tokio::time::timeout(TIMEOUT, rx)
            .await
            .map_err(|_| anyhow!("ATT request {:#04x} timed out", pdu[0]))?
            .map_err(|_| anyhow!("ATT link closed"))?;
        if rsp[0] == OP_ERROR_RSP && rsp.len() >= 5 {
            bail!(AttError { req: rsp[1], handle: le16(&rsp, 2), code: rsp[4] });
        }
        Ok(rsp)
    }

    pub async fn exchange_mtu(&self) -> Result<u16, anyhow::Error> {
        let mut req = vec![OP_MTU_REQ];
        req.extend_from_slice(&OUR_MTU.to_le_bytes());
        let rsp = self.request(&req).await?;
        if rsp[0] != OP_MTU_RSP || rsp.len() < 3 {
            bail!("bad MTU response");
        }
        Ok(le16(&rsp, 1).clamp(ATT_DEFAULT_MTU, OUR_MTU))
    }

    /// All primary services as (start handle, end handle, uuid).
    pub async fn primary_services(&self) -> Result<Vec<(u16, u16, Uuid)>, anyhow::Error> {
        let mut out = Vec::new();
        let mut start = 1u16;
        loop {
            let mut req = vec![OP_READ_BY_GROUP_REQ];
            req.extend_from_slice(&start.to_le_bytes());
            req.extend_from_slice(&0xffffu16.to_le_bytes());
            req.extend_from_slice(&UUID_PRIMARY_SERVICE.to_le_bytes());
            let rsp = match self.request(&req).await {
                Ok(r) => r,
                Err(e) if is_not_found(&e) => break,
                Err(e) => return Err(e),
            };
            let len = rsp[1] as usize;
            if len < 6 {
                break;
            }
            let mut last = start;
            for e in rsp[2..].chunks_exact(len) {
                let (s, end) = (le16(e, 0), le16(e, 2));
                if let Some(u) = parse_uuid(&e[4..]) {
                    out.push((s, end, u));
                }
                last = end;
            }
            if last == 0xffff {
                break;
            }
            start = last + 1;
        }
        Ok(out)
    }

    /// Characteristics declared in a handle range.
    pub async fn characteristics(&self, start: u16, end: u16) -> Result<Vec<Characteristic>, anyhow::Error> {
        let mut out = Vec::new();
        let mut from = start;
        while from <= end {
            let mut req = vec![OP_READ_BY_TYPE_REQ];
            req.extend_from_slice(&from.to_le_bytes());
            req.extend_from_slice(&end.to_le_bytes());
            req.extend_from_slice(&UUID_CHARACTERISTIC.to_le_bytes());
            let rsp = match self.request(&req).await {
                Ok(r) => r,
                Err(e) if is_not_found(&e) => break,
                Err(e) => return Err(e),
            };
            let len = rsp[1] as usize;
            if len < 7 {
                break;
            }
            let mut last = from;
            for e in rsp[2..].chunks_exact(len) {
                // decl handle(2) | properties(1) | value handle(2) | uuid
                last = le16(e, 0);
                if let Some(uuid) = parse_uuid(&e[5..]) {
                    out.push(Characteristic { uuid, properties: e[2], value_handle: le16(e, 3) });
                }
            }
            if last >= end {
                break;
            }
            from = last + 1;
        }
        Ok(out)
    }

    /// Descriptors in a handle range as (handle, uuid).
    pub async fn descriptors(&self, start: u16, end: u16) -> Result<Vec<(u16, Uuid)>, anyhow::Error> {
        let mut out = Vec::new();
        let mut from = start;
        while from <= end {
            let mut req = vec![OP_FIND_INFO_REQ];
            req.extend_from_slice(&from.to_le_bytes());
            req.extend_from_slice(&end.to_le_bytes());
            let rsp = match self.request(&req).await {
                Ok(r) => r,
                Err(e) if is_not_found(&e) => break,
                Err(e) => return Err(e),
            };
            let ulen = if rsp[1] == 1 { 2 } else { 16 };
            let mut last = from;
            for e in rsp[2..].chunks_exact(2 + ulen) {
                last = le16(e, 0);
                if let Some(u) = parse_uuid(&e[2..]) {
                    out.push((last, u));
                }
            }
            if last >= end {
                break;
            }
            from = last + 1;
        }
        Ok(out)
    }

    pub async fn write_request(&self, handle: u16, value: &[u8]) -> Result<(), anyhow::Error> {
        let mut req = vec![OP_WRITE_REQ];
        req.extend_from_slice(&handle.to_le_bytes());
        req.extend_from_slice(value);
        self.request(&req).await?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct AttError {
    pub req: u8,
    pub handle: u16,
    pub code: u8,
}

impl std::fmt::Display for AttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ATT error {:#04x} for request {:#04x} at handle {:#06x}", self.code, self.req, self.handle)
    }
}

impl std::error::Error for AttError {}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<AttError>().is_some_and(|a| a.code == ERR_ATTRIBUTE_NOT_FOUND)
}

pub fn uuid_from_u16(v: u16) -> Uuid {
    uuid16(v)
}
