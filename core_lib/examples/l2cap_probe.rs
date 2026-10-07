// Probe: find a Quick Share receiver over BLE, open an LE L2CAP channel to the PSM in its
// advertisement and send the Nearby "request data connection" command.
use std::time::Duration;

use bluer::l2cap::{SocketAddr, Stream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    let rx = rqs_lib::hdl::discover_receiver(&adapter, None, Duration::from_secs(20)).await?;
    let dev = adapter.device(rx.address)?;
    let at = dev.address_type().await?;
    println!("found {:?} at {} ({at:?}) psm={:?}", rx.name, rx.address, rx.psm);
    let psm = rx.psm.ok_or_else(|| anyhow::anyhow!("no PSM advertised"))?;
    let t = std::time::Instant::now();
    let mut s = tokio::time::timeout(
        Duration::from_secs(20),
        Stream::connect(SocketAddr::new(rx.address, at, psm)),
    )
    .await??;
    for _ in 0..1000 {
        if s.as_ref().send_mtu().is_ok() { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    println!("L2CAP connected in {:?}, send mtu {:?} recv mtu {:?}", t.elapsed(), s.as_ref().send_mtu(), s.as_ref().recv_mtu());
    // [len 4][command 0x03 = request data connection]
    s.write_all(&[0, 0, 0, 1, 0x03]).await?;
    let mut len = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut len)).await??;
    let n = u32::from_be_bytes(len) as usize;
    let mut body = vec![0u8; n.min(4096)];
    s.read_exact(&mut body).await?;
    println!("answer: len {n} {}", hex::encode(&body));
    Ok(())
}
