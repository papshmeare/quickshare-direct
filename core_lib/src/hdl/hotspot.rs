// Temporary Wi-Fi hotspot for the bandwidth upgrade (Nearby Connections WIFI_HOTSPOT medium).
//
// When the phone is not on our network (e.g. a Pixel that dropped off Wi-Fi to discover us over
// Bluetooth), the receiver hosts a WPA2 hotspot and tells the phone its SSID/password/gateway over
// the already-encrypted Bluetooth channel; the phone joins it and connects to us over TCP, and the
// payload continues at Wi-Fi speed. No shared network, no internet needed.
//
// The hotspot runs on a separate *virtual* access-point interface so the laptop's normal Wi-Fi
// connection stays up. Most Wi-Fi chips only allow the AP on the same channel as the station
// connection, so we reuse that channel. Requirements (see README):
//   * an AP interface (default `ap0`), or a systemd unit that creates it (default
//     `quickshare-ap.service`, started via `systemctl start` - allow it for the user with polkit);
//   * NetworkManager (the hotspot is an NM connection in `shared` mode: it assigns 10.42.0.1 to
//     the interface and runs DHCP for the phone);
//   * `iw` and `nmcli` on PATH.
// Environment overrides: QSD_AP_IFACE, QSD_AP_UNIT, QSD_STA_IFACE.

use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{anyhow, bail};
use rand::Rng;
use rand::distr::Alphanumeric;
use tokio::process::Command;

const INNER_NAME: &str = "Hotspot";
const CONN_NAME: &str = "quickshare-hotspot";

#[derive(Debug, Clone)]
pub struct Hotspot {
    pub ssid: String,
    pub password: String,
    pub gateway: Ipv4Addr,
    /// Frequency in MHz (sent to the phone so it can find the network quickly), or -1.
    pub frequency: i32,
    iface: String,
    unit: Option<String>,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

async fn run(cmd: &str, args: &[&str]) -> Result<String, anyhow::Error> {
    let out = Command::new(cmd).args(args).output().await?;
    if !out.status.success() {
        bail!(
            "{cmd} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn iface_exists(iface: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{iface}")).exists()
}

/// Whether a hotspot can be offered on this machine (AP interface present or creatable).
pub fn hotspot_available() -> bool {
    let iface = env_or("QSD_AP_IFACE", "ap0");
    if iface_exists(&iface) {
        return true;
    }
    let unit = env_or("QSD_AP_UNIT", "quickshare-ap.service");
    std::path::Path::new("/etc/systemd/system").join(&unit).exists()
}

/// The Wi-Fi station interface and the frequency (MHz) it is connected on, if any.
async fn station_frequency() -> Option<(String, i32)> {
    let ifaces: Vec<String> = match std::env::var("QSD_STA_IFACE") {
        Ok(i) => vec![i],
        Err(_) => std::fs::read_dir("/sys/class/net")
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| std::path::Path::new(&format!("/sys/class/net/{n}/wireless")).exists())
            .collect(),
    };
    for iface in ifaces {
        if let Ok(info) = run("iw", &["dev", &iface, "info"]).await {
            if !info.contains("type managed") {
                continue;
            }
            // "channel 44 (5220 MHz), width: ..."
            if let Some(mhz) = info
                .split("channel ")
                .nth(1)
                .and_then(|s| s.split('(').nth(1))
                .and_then(|s| s.split(' ').next())
                .and_then(|s| s.parse::<i32>().ok())
            {
                return Some((iface, mhz));
            }
        }
    }
    None
}

fn channel_of(mhz: i32) -> Option<(&'static str, i32)> {
    match mhz {
        2412..=2472 => Some(("bg", (mhz - 2407) / 5)),
        2484 => Some(("bg", 14)),
        5000..=5900 => Some(("a", (mhz - 5000) / 5)),
        _ => None,
    }
}

impl Hotspot {
    pub async fn start() -> Result<Self, anyhow::Error> {
        let iface = env_or("QSD_AP_IFACE", "ap0");
        let mut unit = None;
        if !iface_exists(&iface) {
            let u = env_or("QSD_AP_UNIT", "quickshare-ap.service");
            info!("{INNER_NAME}: creating {iface} via {u}");
            run("systemctl", &["start", &u]).await?;
            unit = Some(u);
            for _ in 0..20 {
                if iface_exists(&iface) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if !iface_exists(&iface) {
                bail!("{iface} did not appear after starting {}", unit.as_deref().unwrap_or("?"));
            }
        }

        let suffix: String = rand::rng()
            .sample_iter(Alphanumeric)
            .take(6)
            .map(char::from)
            .collect();
        let ssid = format!("QuickShare-{suffix}");
        let password: String = rand::rng()
            .sample_iter(Alphanumeric)
            .take(16)
            .map(char::from)
            .collect();

        // Same channel as the station connection (single-channel AP+STA concurrency);
        // otherwise let NetworkManager pick a 5 GHz channel.
        let sta = station_frequency().await;
        let (band, channel, frequency) = match sta.as_ref().and_then(|(_, f)| channel_of(*f).map(|c| (c, *f))) {
            Some(((band, ch), f)) => (band.to_string(), Some(ch), f),
            None => ("a".to_string(), None, -1),
        };
        info!(
            "{INNER_NAME}: starting {ssid} on {iface} (band {band}, channel {:?}; station {:?})",
            channel, sta
        );

        let _ = run("nmcli", &["connection", "delete", CONN_NAME]).await; // leftover from a crash
        let channel_s = channel.map(|c| c.to_string());
        let mut args: Vec<&str> = vec![
            "connection", "add", "type", "wifi", "ifname", &iface, "con-name", CONN_NAME,
            "autoconnect", "no", "ssid", &ssid,
            "802-11-wireless.mode", "ap", "802-11-wireless.band", &band,
            "wifi-sec.key-mgmt", "wpa-psk", "wifi-sec.proto", "rsn",
            "wifi-sec.pairwise", "ccmp", "wifi-sec.group", "ccmp", "wifi-sec.psk", &password,
            "ipv4.method", "shared", "ipv6.method", "disabled",
        ];
        if let Some(c) = channel_s.as_deref() {
            args.extend_from_slice(&["802-11-wireless.channel", c]);
        }
        run("nmcli", &args).await?;
        let hs = Self {
            ssid,
            password,
            gateway: Ipv4Addr::new(10, 42, 0, 1),
            frequency,
            iface,
            unit,
        };
        if let Err(e) = run("nmcli", &["--wait", "20", "connection", "up", CONN_NAME]).await {
            hs.stop_blocking();
            return Err(anyhow!("hotspot didn't come up: {e}"));
        }
        // NetworkManager's shared mode gives the interface 10.42.0.1 unless configured otherwise.
        if let Ok(addr) = run("ip", &["-4", "-o", "addr", "show", "dev", &hs.iface]).await {
            if let Some(ip) = addr
                .split_whitespace()
                .skip_while(|w| *w != "inet")
                .nth(1)
                .and_then(|c| c.split('/').next())
                .and_then(|s| s.parse::<Ipv4Addr>().ok())
            {
                return Ok(Self { gateway: ip, ..hs });
            }
        }
        Ok(hs)
    }

    /// Tear the hotspot down (also called from Drop). Synchronous so it works in Drop.
    pub fn stop_blocking(&self) {
        info!("{INNER_NAME}: stopping {}", self.ssid);
        let _ = std::process::Command::new("nmcli")
            .args(["connection", "delete", CONN_NAME])
            .output();
        if let Some(u) = &self.unit {
            let _ = std::process::Command::new("systemctl").args(["stop", u]).output();
        }
    }
}

/// Owns a running hotspot and removes it when dropped (end of the transfer).
#[derive(Debug)]
pub struct HotspotGuard(pub Hotspot);

impl Drop for HotspotGuard {
    fn drop(&mut self) {
        self.0.stop_blocking();
    }
}
