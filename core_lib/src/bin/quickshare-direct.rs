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
//   RUST_LOG       log filter (default: info; warn for send/devices)
//
// Sending: `quickshare-direct send [--to NAME] FILE...` finds a phone in receive mode over
// Bluetooth (Quick Share open on "Receive", or visible to everyone), shows the PIN, and sends;
// the transfer moves to the phone's Wi-Fi Direct group when the join helper is installed.
// Without a terminal (e.g. from a file manager) it reports through notifications.
// `quickshare-direct devices` lists receivers nearby.
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

/// file:// URI for a path (percent-encoding everything but unreserved characters and '/').
fn file_uri(path: &std::path::Path) -> String {
    let mut s = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// Show the files selected in the file manager (org.freedesktop.FileManager1.ShowItems: Thunar,
/// Nautilus, Dolphin, ...); falls back to opening the folder.
async fn show_items(dir: &std::path::Path, files: &[String]) {
    let uris: Vec<String> = files.iter().map(|f| file_uri(&dir.join(f))).collect();
    let mut cmd = Command::new("busctl");
    cmd.args([
        "--user", "call", "org.freedesktop.FileManager1", "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1", "ShowItems", "ass",
    ])
    .arg(uris.len().to_string())
    .args(&uris)
    .arg("");
    let ok = !uris.is_empty() && cmd.output().await.map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        open_detached(&dir.to_string_lossy());
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


const USAGE: &str = "usage: quickshare-direct                         run the receiver
       quickshare-direct send [--to NAME] FILE...  send files to a phone in receive mode
       quickshare-direct devices                   list phones in receive mode nearby";

const NO_RECEIVER_HINT: &str = "No phone in receive mode found. On the phone, open Quick Share and tap Receive \
(or set Quick Share to be visible to everyone), keep the screen on, and try again.";

/// Send-side reporting: the terminal, or notifications (updated in place) without one.
struct Reporter {
    tty: bool,
    notif_id: Option<String>,
}

impl Reporter {
    fn new() -> Self {
        use std::io::IsTerminal;
        Self { tty: std::io::stderr().is_terminal(), notif_id: None }
    }

    /// A status line (on a terminal it replaces the previous `progress` line).
    async fn status(&mut self, summary: &str, body: &str) {
        if self.tty {
            eprintln!("\r\x1b[K{summary}{}", if body.is_empty() { String::new() } else { format!(": {body}") });
            return;
        }
        let mut cmd = Command::new("notify-send");
        cmd.args(["--app-name=Quick Share", "--icon=network-wireless", "--print-id"]);
        if let Some(id) = &self.notif_id {
            cmd.arg(format!("--replace-id={id}"));
        }
        cmd.arg(summary).arg(body);
        if let Ok(out) = cmd.output().await {
            let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !id.is_empty() {
                self.notif_id = Some(id);
            }
        }
    }

    fn progress(&self, done: u64, total: u64, rate: f64) {
        if self.tty && total > 0 {
            eprint!(
                "\r\x1b[K{:>3}%  {} of {}  {}/s",
                done * 100 / total,
                human_size(done),
                human_size(total),
                human_size(rate as u64)
            );
        }
    }
}

async fn ble_adapter() -> Result<bluer::Adapter, anyhow::Error> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    if !adapter.is_powered().await? {
        anyhow::bail!("Bluetooth is off (turn it on and try again)");
    }
    Ok(adapter)
}

fn receiver_label(r: &rqs_lib::hdl::BleReceiver) -> String {
    r.name.clone().unwrap_or_else(|| format!("unnamed device {}", String::from_utf8_lossy(&r.endpoint_id)))
}

async fn cmd_devices() -> Result<i32, anyhow::Error> {
    let adapter = ble_adapter().await?;
    eprintln!("Scanning for 8 s...");
    let found = rqs_lib::hdl::discover_receivers(
        &adapter,
        None,
        std::time::Duration::from_secs(8),
        std::time::Duration::from_secs(8),
    )
    .await?;
    if found.is_empty() {
        eprintln!("{NO_RECEIVER_HINT}");
        return Ok(1);
    }
    for r in &found {
        println!("{}", receiver_label(r));
    }
    Ok(0)
}

async fn cmd_send(args: &[String]) -> Result<i32, anyhow::Error> {
    let mut to: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--to" => to = Some(it.next().ok_or_else(|| anyhow::anyhow!("--to needs a name"))?.clone()),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            "--" => files.extend(it.by_ref().cloned()),
            f if f.starts_with("--to=") => to = Some(f["--to=".len()..].to_string()),
            f if f.starts_with('-') && f.len() > 1 => anyhow::bail!("unknown option {f}\n{USAGE}"),
            f => files.push(f.to_string()),
        }
    }
    if files.is_empty() {
        anyhow::bail!("no files given\n{USAGE}");
    }
    let mut paths = Vec::new();
    for f in &files {
        let p = std::fs::canonicalize(f).map_err(|e| anyhow::anyhow!("{f}: {e}"))?;
        if !p.is_file() {
            anyhow::bail!("{f}: not a regular file (folders aren't supported)");
        }
        paths.push(p.to_string_lossy().into_owned());
    }
    let total_size: u64 = paths.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    let what = if paths.len() == 1 {
        let n = std::path::Path::new(&paths[0]).file_name().map(|n| n.to_string_lossy().into_owned());
        format!("{} ({})", n.unwrap_or_default(), human_size(total_size))
    } else {
        format!("{} files ({})", paths.len(), human_size(total_size))
    };

    rqs_lib::set_device_name(&device_name());
    let adapter = ble_adapter().await?;
    let mut rep = Reporter::new();
    rep.status("Quick Share", &format!("Looking for {}...", to.as_deref().unwrap_or("a phone in receive mode"))).await;
    let found = rqs_lib::hdl::discover_receivers(
        &adapter,
        to.as_deref(),
        std::time::Duration::from_secs(30),
        if to.is_some() { std::time::Duration::ZERO } else { std::time::Duration::from_secs(3) },
    )
    .await?;
    let receiver = match found.as_slice() {
        [] => {
            rep.status("Quick Share: nothing sent", NO_RECEIVER_HINT).await;
            return Ok(1);
        }
        [r] => r.clone(),
        many => {
            let names: Vec<String> = many.iter().map(receiver_label).collect();
            rep.status(
                "Quick Share: several phones nearby",
                &format!("pick one with --to NAME: {}", names.join(", ")),
            )
            .await;
            return Ok(2);
        }
    };
    let target = receiver_label(&receiver);
    rep.status(&format!("Sending {what} to {target}"), "connecting...").await;

    let (sender, mut events) = tokio::sync::broadcast::channel::<ChannelMessage>(256);
    let ctk = tokio_util::sync::CancellationToken::new();
    {
        let ctk = ctk.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                ctk.cancel();
            }
        });
    }
    let send = rqs_lib::hdl::send_files_ble(&adapter, &receiver, paths, "send".into(), sender, ctk);
    tokio::pin!(send);

    let mut shown_pin = false;
    let mut started: Option<std::time::Instant> = None;
    let mut last_progress = std::time::Instant::now();
    let result = loop {
        tokio::select! {
            r = &mut send => break r,
            ev = events.recv() => {
                let Ok(cm) = ev else { continue };
                let Message::Client(mc) = cm.msg else { continue };
                let Some(meta) = mc.metadata else { continue };
                match mc.state {
                    Some(TransferState::SentIntroduction) if !shown_pin => {
                        shown_pin = true;
                        let pin = meta.pin_code.map(|p| format!(" (PIN {p})")).unwrap_or_default();
                        rep.status(&format!("Sending {what} to {target}"), &format!("accept on the phone{pin}")).await;
                    }
                    Some(TransferState::SendingFiles) => {
                        let t0 = *started.get_or_insert_with(std::time::Instant::now);
                        if last_progress.elapsed().as_millis() >= 200 {
                            last_progress = std::time::Instant::now();
                            let rate = meta.ack_bytes as f64 / t0.elapsed().as_secs_f64().max(0.001);
                            rep.progress(meta.ack_bytes, meta.total_bytes, rate);
                        }
                    }
                    _ => {}
                }
            }
        }
    };
    let secs = started.map(|t| t.elapsed().as_secs_f64());
    match result {
        Ok(TransferState::Finished) => {
            let rate = secs.map(|s| format!(" in {s:.1} s ({}/s)", human_size((total_size as f64 / s.max(0.001)) as u64))).unwrap_or_default();
            rep.status(&format!("Sent {what} to {target}"), &rate.trim_start().to_string()).await;
            Ok(0)
        }
        Ok(TransferState::Rejected) => {
            rep.status(&format!("{target} declined"), &what).await;
            Ok(1)
        }
        Ok(TransferState::Cancelled) => {
            rep.status("Quick Share: cancelled", &what).await;
            Ok(130)
        }
        Ok(state) => {
            rep.status(&format!("Couldn't send to {target}"), &format!("connection ended ({state:?})")).await;
            Ok(1)
        }
        Err(e) => {
            rep.status(&format!("Couldn't send to {target}"), &e.to_string()).await;
            Ok(1)
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd) = args.first().map(String::as_str) {
        let log = || {
            tracing_subscriber::fmt()
                .with_env_filter(if std::env::var("RUST_LOG").is_ok() {
                    EnvFilter::builder().from_env_lossy()
                } else {
                    EnvFilter::builder().parse_lossy("warn,bluez_async=error,btleplug=error")
                })
                .with_writer(std::io::stderr)
                .init();
        };
        let code = match cmd {
            "send" => {
                log();
                cmd_send(&args[1..]).await
            }
            "devices" => {
                log();
                cmd_devices().await
            }
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                Ok(0)
            }
            other => {
                eprintln!("unknown command {other}\n{USAGE}");
                Ok(2)
            }
        };
        let code = code.unwrap_or_else(|e| {
            eprintln!("quickshare-direct: {e}");
            1
        });
        std::process::exit(code);
    }

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
                                    &[("open", "Show in folder")],
                                    None,
                                )
                                .await
                                .as_deref()
                                    == Some("open")
                                {
                                    let files = match &meta.payload {
                                        Some(TransferPayload::Files(f)) => f.clone(),
                                        _ => Vec::new(),
                                    };
                                    show_items(&dir, &files).await;
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
