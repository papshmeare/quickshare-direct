// Notices a nearby device starting to share (its Nearby Sharing 0xFE2C advertisement), so the mDNS
// service can re-announce itself right away for a phone on the same Wi-Fi.
//
// This must not keep BlueZ discovery open: a permanent discovery stops BlueZ's background scan
// from reconnecting bonded LE devices (mice, keyboards) on some controllers (issue #1, MediaTek
// MT7922). So it uses BlueZ's passive advertisement monitor (org.bluez.AdvertisementMonitorManager1),
// and without it (BlueZ < 5.65 or not in experimental mode, which is the default on most
// distributions) it does nothing: the signal is optional, phones query mDNS themselves and the
// Bluetooth path doesn't use it.
//
// QSD_BLE_SCAN=discovery restores the old permanent discovery (rQuickShare's behaviour);
// QSD_BLE_SCAN=off (or QSD_NO_BLE_SCAN) disables the listener.

use std::time::{Duration, Instant};

use anyhow::anyhow;
use bluer::monitor::{Monitor, MonitorEvent, Pattern};
use bluer::{AdapterEvent, DiscoveryFilter, DiscoveryTransport, Uuid, UuidExt};
use futures::stream::StreamExt;
use tokio::sync::broadcast::Sender;
use tokio_util::sync::CancellationToken;

const INNER_NAME: &str = "BleListener";

/// Service data AD type (16-bit UUID) and the Nearby Sharing UUID 0xFE2C, little-endian.
const AD_TYPE_SERVICE_DATA16: u8 = 0x16;
const SHARING_UUID16_LE: [u8; 2] = [0x2c, 0xfe];
const SERVICE_UUID_SHARING: u16 = 0xfe2c;

/// Quick Share advertises every few seconds while sharing; one alert per 10 s is plenty.
const ALERT_INTERVAL: Duration = Duration::from_secs(10);

pub struct BleListener {
    adapter: bluer::Adapter,
    sender: Sender<()>,
}

impl BleListener {
    pub async fn new(sender: Sender<()>) -> Result<Self, anyhow::Error> {
        let session = bluer::Session::new().await?;
        let adapter = session
            .default_adapter()
            .await
            .map_err(|e| anyhow!("no bluetooth adapter: {e}"))?;
        Ok(Self { adapter, sender })
    }

    pub async fn run(self, ctk: CancellationToken) -> Result<(), anyhow::Error> {
        let mode = std::env::var("QSD_BLE_SCAN").unwrap_or_default();
        match mode.as_str() {
            "off" => {
                info!("{INNER_NAME}: disabled (QSD_BLE_SCAN=off)");
                Ok(())
            }
            "discovery" => self.run_discovery(ctk).await,
            _ => match self.run_monitor(&ctk).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    info!(
                        "{INNER_NAME}: no passive advertisement monitor ({e}); not scanning \
                         (a permanent discovery would block reconnects of bonded LE devices)"
                    );
                    Ok(())
                }
            },
        }
    }

    fn alert(&self, last: &mut Option<Instant>, what: &str) {
        if last.is_some_and(|t| t.elapsed() < ALERT_INTERVAL) {
            return;
        }
        debug!("{INNER_NAME}: a device is sharing nearby ({what})");
        let _ = self.sender.send(());
        *last = Some(Instant::now());
    }

    /// Passive: BlueZ matches the advertisements (offloaded to the controller where supported)
    /// without holding discovery open.
    async fn run_monitor(&self, ctk: &CancellationToken) -> Result<(), anyhow::Error> {
        let manager = self.adapter.monitor().await?;
        let mut handle = manager
            .register(Monitor {
                patterns: Some(vec![Pattern::new(
                    AD_TYPE_SERVICE_DATA16,
                    0,
                    &SHARING_UUID16_LE,
                )]),
                ..Default::default()
            })
            .await?;
        info!("{INNER_NAME}: watching for sharing devices (passive advertisement monitor)");
        let mut last = None;
        loop {
            tokio::select! {
                _ = ctk.cancelled() => break,
                ev = handle.next() => match ev {
                    Some(MonitorEvent::DeviceFound(id)) => self.alert(&mut last, &id.device.to_string()),
                    Some(_) => {}
                    None => return Err(anyhow!("monitor released by BlueZ")),
                },
            }
        }
        Ok(())
    }

    /// rQuickShare's original approach: LE discovery for as long as the receiver runs.
    async fn run_discovery(&self, ctk: CancellationToken) -> Result<(), anyhow::Error> {
        warn!(
            "{INNER_NAME}: permanent LE discovery (QSD_BLE_SCAN=discovery); this can stop bonded \
             LE mice/keyboards from reconnecting"
        );
        self.adapter
            .set_discovery_filter(DiscoveryFilter {
                transport: DiscoveryTransport::Le,
                duplicate_data: true,
                ..Default::default()
            })
            .await?;
        let events = self.adapter.discover_devices().await?;
        tokio::pin!(events);
        let sharing = <Uuid as UuidExt>::from_u16(SERVICE_UUID_SHARING);
        let mut last = None;
        loop {
            tokio::select! {
                _ = ctk.cancelled() => break,
                ev = events.next() => {
                    let addr = match ev {
                        Some(AdapterEvent::DeviceAdded(a)) => a,
                        Some(AdapterEvent::PropertyChanged(_)) | Some(AdapterEvent::DeviceRemoved(_)) => continue,
                        None => break,
                    };
                    let Ok(dev) = self.adapter.device(addr) else { continue };
                    if let Ok(Some(sd)) = dev.service_data().await {
                        if sd.contains_key::<Uuid>(&sharing) {
                            self.alert(&mut last, &addr.to_string());
                        }
                    }
                }
            }
        }
        info!("{INNER_NAME}: tracker cancelled, breaking");
        Ok(())
    }
}
