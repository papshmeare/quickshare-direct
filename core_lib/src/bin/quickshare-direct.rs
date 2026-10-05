// quickshare-direct: headless Quick Share receiver with desktop notifications.
//
// Runs the receiver (mDNS + BLE discovery, Bluetooth first contact, Wi-Fi LAN / hotspot upgrade)
// and asks through notifications: Accept/Decline for each incoming transfer (with the PIN shown on
// the phone), then "Open folder" when files arrive, "Open" for links, "Copy" for text.
// Notifications use `notify-send` (libnotify >= 0.8, with actions); opening uses `xdg-open`;
// copying uses `wl-copy` (Wayland) or `xclip`.
//
// Environment:
//   QSD_DIR        download folder (default: XDG Downloads, else ~/Downloads)
//   QSD_NAME       name shown on the phone (default: hostname)
//   QSD_PORT       TCP port for incoming transfers (default: random)
//   QSD_BWU_PORT   TCP port for the Wi-Fi upgrade (default: random)
//   QSD_CONSENT_TIMEOUT  seconds to answer the Accept/Decline notification (default 60)
//   RUST_LOG       log filter (default: info)
#[macro_use]
extern crate log;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rqs_lib::channel::{ChannelMessage, Message, TransferAction, TransferKind};
use rqs_lib::hdl::info::{TransferMetadata, TransferPayload};
use rqs_lib::{RQS, TransferState, Visibility};
use tokio::process::Command;
use tracing_subscriber::EnvFilter;

fn human_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1000.0 && i < U.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

fn download_dir() -> PathBuf {
    if let Ok(d) = std::env::var("QSD_DIR") {
        return PathBuf::from(d);
    }
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads"))
}

fn device_name() -> String {
    std::env::var("QSD_NAME").ok().unwrap_or_else(|| {
        std::fs::read_to_string("/etc/hostname")
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Linux".to_string())
    })
}

/// Show a notification and wait for the chosen action (None if dismissed or timed out).
async fn notify(summary: &str, body: &str, actions: &[(&str, &str)], timeout_s: Option<u64>) -> Option<String> {
    let mut cmd = Command::new("notify-send");
    cmd.args(["--app-name=Quick Share", "--icon=network-wireless"]);
    for (name, label) in actions {
        cmd.arg(format!("--action={name}={label}"));
    }
    if let Some(t) = timeout_s {
        cmd.arg(format!("--expire-time={}", t * 1000));
    }
    cmd.arg(summary).arg(body);
    let fut = cmd.output();
    let out = match timeout_s {
        Some(t) => tokio::time::timeout(std::time::Duration::from_secs(t + 2), fut).await.ok()?,
        None => fut.await,
    }
    .ok()?;
    let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!chosen.is_empty()).then_some(chosen)
}

fn sender_name(meta: &TransferMetadata) -> String {
    meta.source
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "a nearby device".to_string())
}

fn describe(meta: &TransferMetadata) -> String {
    match &meta.payload {
        Some(TransferPayload::Files(files)) => {
            let names = if files.len() <= 3 {
                files.join(", ")
            } else {
                format!("{}, … ({} files)", files[..2].join(", "), files.len())
            };
            format!("{names} ({})", human_size(meta.total_bytes))
        }
        Some(TransferPayload::Text(_)) => "Text".to_string(),
        Some(TransferPayload::Url(u)) => u.clone(),
        Some(TransferPayload::Wifi { ssid, .. }) => format!("Wi-Fi network {ssid}"),
        None => human_size(meta.total_bytes),
    }
}

/// Open a file/folder/URL with the desktop's default app, outside this service's process group
/// (so the app survives service restarts) and with the user's normal PATH (a systemd user
/// service has a minimal one, so xdg-open found Thunar's .desktop file but not `thunar`).
fn open_detached(target: &str) {
    let mut path = std::env::var("PATH").unwrap_or_default();
    if let Ok(user) = std::env::var("USER") {
        path.push_str(&format!(":/etc/profiles/per-user/{user}/bin"));
    }
    path.push_str(":/run/current-system/sw/bin:/usr/local/bin:/usr/bin:/bin");
    let scoped = std::process::Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "--collect", "xdg-open", target])
        .env("PATH", &path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if scoped.is_err() {
        let _ = std::process::Command::new("xdg-open").arg(target).env("PATH", &path).spawn();
    }
}

async fn copy_to_clipboard(text: &str) {
    use tokio::io::AsyncWriteExt;
    let mut cmd = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Command::new("wl-copy")
    } else {
        let mut c = Command::new("xclip");
        c.args(["-selection", "clipboard"]);
        c
    };
    if let Ok(mut child) = cmd.stdin(std::process::Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes()).await;
        }
        let _ = child.wait().await;
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(if std::env::var("RUST_LOG").is_ok() {
            EnvFilter::builder().from_env_lossy()
        } else {
            EnvFilter::builder()
                .parse_lossy("info,mdns_sd=error,polling=error,neli=error,bluez_async=error,btleplug=error")
        })
        .init();

    let dir = download_dir();
    std::fs::create_dir_all(&dir).ok();
    let name = device_name();
    let port: Option<u32> = std::env::var("QSD_PORT").ok().and_then(|p| p.parse().ok());
    let consent_timeout: u64 = std::env::var("QSD_CONSENT_TIMEOUT")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(60);

    let mut rqs = RQS::new(Visibility::Visible, port, Some(dir.clone()), Some(name.clone()));
    rqs.run().await?;
    info!("Quick Share receiver '{name}' running; files go to {}", dir.display());

    let sender = rqs.message_sender.clone();
    let mut rx = rqs.message_sender.subscribe();
    // Transfers we already asked about / reported (the library repeats state updates).
    let asked: Arc<Mutex<HashSet<String>>> = Arc::default();
    let done: Arc<Mutex<HashSet<String>>> = Arc::default();

    tokio::spawn(async move {
        loop {
            // Progress updates are frequent; if we fall behind, skip them instead of stopping.
            let cm = match rx.recv().await {
                Ok(cm) => cm,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    debug!("skipped {n} state updates");
                    continue;
                }
                Err(_) => break,
            };
            let Message::Client(mc) = &cm.msg else { continue };
            if mc.kind != TransferKind::Inbound {
                continue;
            }
            let Some(meta) = mc.metadata.clone() else { continue };
            match mc.state {
                Some(TransferState::WaitingForUserConsent) => {
                    if !asked.lock().unwrap().insert(cm.id.clone()) {
                        continue;
                    }
                    let (sender, id, timeout) = (sender.clone(), cm.id.clone(), consent_timeout);
                    tokio::spawn(async move {
                        let pin = meta.pin_code.clone().map(|p| format!(" · PIN {p}")).unwrap_or_default();
                        let summary = format!("Quick Share from {}{pin}", sender_name(&meta));
                        let choice = notify(
                            &summary,
                            &describe(&meta),
                            &[("accept", "Accept"), ("decline", "Decline")],
                            Some(timeout),
                        )
                        .await;
                        let action = if choice.as_deref() == Some("accept") {
                            info!("{id}: accepted");
                            TransferAction::ConsentAccept
                        } else {
                            info!("{id}: declined ({})", choice.as_deref().unwrap_or("no answer"));
                            TransferAction::ConsentDecline
                        };
                        let _ = sender.send(ChannelMessage { id, msg: Message::Lib { action } });
                    });
                }
                Some(TransferState::Rejected | TransferState::Cancelled | TransferState::Disconnected) => {
                    asked.lock().unwrap().remove(&cm.id);
                    done.lock().unwrap().remove(&cm.id);
                }
                Some(TransferState::Finished) => {
                    asked.lock().unwrap().remove(&cm.id);
                    if !done.lock().unwrap().insert(cm.id.clone()) {
                        continue;
                    }
                    let dir = dir.clone();
                    tokio::spawn(async move {
                        let from = sender_name(&meta);
                        match meta.payload.clone() {
                            Some(TransferPayload::Text(text)) => {
                                let preview: String = text.chars().take(200).collect();
                                if notify(&format!("Text from {from}"), &preview, &[("copy", "Copy")], None)
                                    .await
                                    .as_deref()
                                    == Some("copy")
                                {
                                    copy_to_clipboard(&text).await;
                                }
                            }
                            Some(TransferPayload::Url(url)) => {
                                if notify(&format!("Link from {from}"), &url, &[("open", "Open")], None)
                                    .await
                                    .as_deref()
                                    == Some("open")
                                {
                                    open_detached(&url);
                                }
                            }
                            _ => {
                                if notify(
                                    &format!("Received from {from}"),
                                    &describe(&meta),
                                    &[("open", "Open folder")],
                                    None,
                                )
                                .await
                                .as_deref()
                                    == Some("open")
                                {
                                    open_detached(&dir.to_string_lossy());
                                }
                            }
                        }
                    });
                }
                _ => {}
            }
        }
    });

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    info!("Stopping.");
    rqs.stop().await;
    Ok(())
}
