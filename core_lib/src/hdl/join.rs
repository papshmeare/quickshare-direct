// Joining the receiver's Wi-Fi Direct group / hotspot for sending (the receiver hosts the upgrade
// link and sends us its credentials over the encrypted Bluetooth channel).
//
// The join runs in the root helper unit (packaging/linux/quickshare-join, default
// `quickshare-join.service`) on a second station interface, so the normal Wi-Fi stays up. We hand
// it the credentials in a 0600 file in our runtime dir and wait for its `joined` file.
// Environment overrides: QSD_JOIN_UNIT, QSD_JOIN_REQUEST, QSD_JOIN_RESULT.

use std::io::Write;
use std::net::Ipv4Addr;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail};
use tokio::process::Command;

const INNER_NAME: &str = "Join";

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn request_path() -> PathBuf {
    if let Ok(p) = std::env::var("QSD_JOIN_REQUEST") {
        return p.into();
    }
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // /proc/self belongs to our uid.
        let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
        format!("/run/user/{uid}")
    });
    PathBuf::from(dir).join("quickshare-join")
}

/// Whether the join helper unit is installed.
pub fn join_available() -> bool {
    let unit = env_or("QSD_JOIN_UNIT", "quickshare-join.service");
    ["/etc/systemd/system", "/run/systemd/system", "/usr/lib/systemd/system", "/lib/systemd/system"]
        .iter()
        .any(|d| std::path::Path::new(d).join(&unit).exists())
}

/// A joined network; leaving it (stopping the helper unit) happens on drop.
#[derive(Debug)]
pub struct JoinGuard {
    unit: String,
    pub iface: String,
    pub ip: Ipv4Addr,
}

impl Drop for JoinGuard {
    fn drop(&mut self) {
        info!("{INNER_NAME}: leaving ({})", self.iface);
        let _ = std::process::Command::new("systemctl")
            .args(["stop", &self.unit])
            .output();
    }
}

/// Join `ssid` (frequency in MHz or -1) through the helper; returns once we have an address.
pub async fn join_network(
    ssid: &str,
    password: &str,
    frequency: i32,
    gateway: &str,
) -> Result<JoinGuard, anyhow::Error> {
    if [ssid, password, gateway].iter().any(|v| v.contains('\n')) {
        bail!("bad credentials");
    }
    let unit = env_or("QSD_JOIN_UNIT", "quickshare-join.service");
    let result = PathBuf::from(env_or("QSD_JOIN_RESULT", "/run/quickshare-join/joined"));
    let req = request_path();
    let _ = std::fs::remove_file(&req);
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&req)?;
        writeln!(f, "ssid={ssid}\npassword={password}\ngateway={gateway}")?;
        if frequency > 0 {
            writeln!(f, "frequency={frequency}")?;
        }
    }

    info!("{INNER_NAME}: joining {ssid} ({frequency} MHz) via {unit}");
    let started = Command::new("systemctl").args(["start", &unit]).output().await;
    let joined = async {
        let out = started?;
        if !out.status.success() {
            bail!("systemctl start {unit}: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        for _ in 0..250 {
            if let Ok(c) = std::fs::read_to_string(&result) {
                let get = |k: &str| c.lines().find_map(|l| l.strip_prefix(&format!("{k}=")));
                if let (Some(iface), Some(ip)) = (get("iface"), get("ip")) {
                    return Ok((iface.to_string(), ip.parse::<Ipv4Addr>()?));
                }
            }
            let state = Command::new("systemctl").args(["is-active", &unit]).output().await?;
            let state = String::from_utf8_lossy(&state.stdout).trim().to_string();
            if state == "failed" || state == "inactive" {
                bail!("{unit} {state} (see journalctl -u {unit})");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(anyhow!("{unit} didn't join within 25 s"))
    }
    .await;
    let _ = std::fs::remove_file(&req); // holds the password
    match joined {
        Ok((iface, ip)) => {
            info!("{INNER_NAME}: on {ssid} via {iface} as {ip}");
            Ok(JoinGuard { unit, iface, ip })
        }
        Err(e) => {
            let _ = Command::new("systemctl").args(["stop", &unit]).output().await;
            Err(e)
        }
    }
}
