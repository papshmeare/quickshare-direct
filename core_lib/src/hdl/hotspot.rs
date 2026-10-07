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
//   * an AP interface (default `ap0`), or the root helper unit that creates it
//     (`quickshare-ap.service` from the NixOS module, or our `quickshare-ap@USER.service` from
//     the packages; started via `systemctl start`, which polkit allows the user);
//   * NetworkManager (the hotspot is an NM connection in `shared` mode: it assigns 10.42.0.1 to
//     the interface and runs DHCP for the phone);
//   * `nmcli` on PATH.
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
    /// Started via the root helper unit (hostapd + dnsmasq), which also removes it.
    helper: bool,
    /// The helper made a Wi-Fi Direct group (offer WIFI_DIRECT) rather than a hotspot.
    pub wifi_direct: bool,
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

/// The program that starts/stops the root helper units: systemctl, or QSD_SYSTEMCTL for
/// development (tools/dev/helper-ctl.sh runs the repo's helpers through a root watcher, so helper
/// changes can be tried without reinstalling).
pub(crate) fn systemctl() -> String {
    std::env::var("QSD_SYSTEMCTL").unwrap_or_else(|_| "systemctl".to_string())
}

fn iface_exists(iface: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{iface}")).exists()
}

fn unit_installed(unit: &str) -> bool {
    [
        "/etc/systemd/system",
        "/run/systemd/system",
        "/usr/local/lib/systemd/system",
        "/usr/lib/systemd/system",
        "/lib/systemd/system",
    ]
    .iter()
    .any(|d| std::path::Path::new(d).join(unit).exists())
}

fn current_user() -> Option<String> {
    if let Some(u) = std::env::var("USER").ok().filter(|u| !u.is_empty()) {
        return Some(u);
    }
    let out = std::process::Command::new("id").arg("-un").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|u| !u.is_empty())
}

/// The root helper unit `base` ("quickshare-ap", "quickshare-join"): `base.service` as the NixOS
/// module installs it (for one configured user), or our instance `base@USER.service` of the
/// template that distribution packages install (polkit lets each user start their own).
/// `env_key` (QSD_AP_UNIT, QSD_JOIN_UNIT) overrides it.
pub(crate) fn helper_unit(base: &str, env_key: &str) -> Option<String> {
    if let Ok(u) = std::env::var(env_key) {
        return Some(u);
    }
    if std::env::var_os("QSD_SYSTEMCTL").is_some() {
        return Some(format!("{base}.service"));
    }
    let plain = format!("{base}.service");
    if unit_installed(&plain) {
        return Some(plain);
    }
    if unit_installed(&format!("{base}@.service")) {
        return Some(format!("{base}@{}.service", current_user()?));
    }
    None
}

/// Whether a hotspot can be offered on this machine (helper unit installed, or an AP interface
/// that NetworkManager can use).
pub fn hotspot_available() -> bool {
    helper_unit("quickshare-ap", "QSD_AP_UNIT").is_some()
        || iface_exists(&env_or("QSD_AP_IFACE", "ap0"))
}

/// The frequency (MHz) of the Wi-Fi network we are connected to as a station, if any
/// (from NetworkManager: `nmcli -g active,chan,freq dev wifi list` → "yes:44:5220 MHz").
pub(crate) async fn station_frequency() -> Option<i32> {
    let out = run("nmcli", &["-g", "active,chan,freq", "device", "wifi", "list", "--rescan", "no"])
        .await
        .ok()?;
    out.lines()
        .find(|l| l.starts_with("yes:"))
        .and_then(|l| l.rsplit(':').next())
        .and_then(|f| f.split_whitespace().next())
        .and_then(|f| f.parse().ok())
}

/// Wait until NetworkManager manages a freshly created interface (state no longer "unmanaged").
async fn wait_nm_managed(iface: &str) -> bool {
    for _ in 0..50 {
        if let Ok(out) = run("nmcli", &["-g", "DEVICE,STATE", "device"]).await {
            if out
                .lines()
                .any(|l| l.starts_with(&format!("{iface}:")) && !l.ends_with(":unmanaged"))
            {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Last resort: ask NetworkManager to manage it.
    run("nmcli", &["device", "set", iface, "managed", "yes"]).await.is_ok()
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
        if let Some(unit) = helper_unit("quickshare-ap", "QSD_AP_UNIT") {
            return Self::start_helper(unit).await;
        }
        Self::start_nm().await
    }

    /// Root helper (packaging/linux/quickshare-ap): starts hostapd + dnsmasq on a virtual AP
    /// interface and writes `ssid=/password=/frequency=/gateway=` lines to a credentials file.
    async fn start_helper(unit: String) -> Result<Self, anyhow::Error> {
        let creds_path = env_or("QSD_AP_CREDENTIALS", "/run/quickshare/credentials");
        info!("{INNER_NAME}: starting {unit}");
        run(&systemctl(), &["start", &unit]).await?;
        let mut creds = String::new();
        for i in 0..150 {
            if let Ok(c) = std::fs::read_to_string(&creds_path) {
                if c.contains("password=") {
                    creds = c;
                    break;
                }
            }
            // Give up as soon as the helper gave up (e.g. no usable channel), not after 15 s.
            if i % 5 == 4 {
                // (is-active exits non-zero for exactly these states, so read its output.)
                let out = Command::new(systemctl()).args(["is-active", &unit]).output().await?;
                let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if matches!(state.as_str(), "failed" | "inactive") {
                    bail!("{unit} {state} (see journalctl -u {unit})");
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if creds.is_empty() {
            let _ = run(&systemctl(), &["stop", &unit]).await;
            bail!("{unit} didn't write {creds_path} within 15 s (see journalctl -u {unit})");
        }
        let get = |k: &str| {
            creds
                .lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")))
                .map(str::to_string)
        };
        let hs = Self {
            ssid: get("ssid").ok_or_else(|| anyhow!("no ssid in {creds_path}"))?,
            password: get("password").ok_or_else(|| anyhow!("no password in {creds_path}"))?,
            gateway: get("gateway")
                .and_then(|g| g.parse().ok())
                .unwrap_or(Ipv4Addr::new(10, 42, 0, 1)),
            frequency: get("frequency").and_then(|f| f.parse().ok()).unwrap_or(-1),
            iface: env_or("QSD_AP_IFACE", "ap0"),
            unit: Some(unit),
            helper: true,
            wifi_direct: get("mode").as_deref() == Some("p2p"),
        };
        info!("{INNER_NAME}: {} up (gateway {}, {} MHz)", hs.ssid, hs.gateway, hs.frequency);
        Ok(hs)
    }

    /// Without the helper: a NetworkManager hotspot (`shared` mode) on an existing AP interface.
    async fn start_nm() -> Result<Self, anyhow::Error> {
        let iface = env_or("QSD_AP_IFACE", "ap0");
        // Read the station channel first: creating the AP interface can briefly disturb the
        // station connection on some systems.
        let sta = station_frequency().await;
        let mut unit = None;
        if !iface_exists(&iface) {
            let u = helper_unit("quickshare-ap", "QSD_AP_UNIT")
                .unwrap_or_else(|| "quickshare-ap.service".to_string());
            info!("{INNER_NAME}: creating {iface} via {u}");
            run(&systemctl(), &["start", &u]).await?;
            unit = Some(u);
            for _ in 0..20 {
                if iface_exists(&iface) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if !iface_exists(&iface) {
                bail!(
                    "{iface} did not appear after starting {}",
                    unit.as_deref().unwrap_or("?")
                );
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
        if !wait_nm_managed(&iface).await {
            warn!("{INNER_NAME}: NetworkManager doesn't manage {iface}");
        }
        let (band, channel, frequency) = match sta.and_then(|f| channel_of(f).map(|c| (c, f))) {
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
            "connection",
            "add",
            "type",
            "wifi",
            "ifname",
            &iface,
            "con-name",
            CONN_NAME,
            "autoconnect",
            "no",
            "ssid",
            &ssid,
            "802-11-wireless.mode",
            "ap",
            "802-11-wireless.band",
            &band,
            "wifi-sec.key-mgmt",
            "wpa-psk",
            "wifi-sec.proto",
            "rsn",
            "wifi-sec.pairwise",
            "ccmp",
            "wifi-sec.group",
            "ccmp",
            "wifi-sec.psk",
            &password,
            "ipv4.method",
            "shared",
            "ipv6.method",
            "disabled",
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
            helper: false,
            wifi_direct: false,
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
        if !self.helper {
            let _ = std::process::Command::new("nmcli")
                .args(["connection", "delete", CONN_NAME])
                .output();
        }
        if let Some(u) = &self.unit {
            let _ = std::process::Command::new(systemctl())
                .args(["stop", u])
                .output();
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
