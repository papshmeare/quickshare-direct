# quickshare-direct: notes for Claude Code sessions

Quick Share (Nearby Share) receiver for Linux that works without a shared Wi-Fi: first contact over
Bluetooth LE, then the transfer moves to a Wi-Fi Direct group the laptop hosts (fallbacks: hotspot,
shared LAN). Public repo github.com/papshmeare/quickshare-direct (GPL-3.0, fork of rQuickShare with
martinalderson's BLE receiver branch). Commits use the GitHub noreply address (set in this repo's
git config); don't commit with the owner's Gmail.

Read first: README.md (status, install), docs/DEV_NOTES.md (all findings, phone-log evidence,
measurements, open plan), docs/BLE_RECEIVER_DISCOVERY.md (BLE/weave protocol).

## Layout
- `core_lib/src/hdl/inbound.rs`: receive state machine + bandwidth upgrade (`do_bwu`, background
  `hotspot_task`, consent-before-upgrade gate in `take_bwu_pending`, cancel-safe frame reads).
- `core_lib/src/hdl/hotspot.rs`: starts the root helper unit, reads `/run/quickshare/credentials`
  (mode p2p → WIFI_DIRECT offer, ap → WIFI_HOTSPOT); NetworkManager fallback.
- `core_lib/src/hdl/{gatt,blea,rfcomm}.rs`: BLE GATT/weave server, BLE advertisement, Bluetooth
  Classic RFCOMM (opt-in `QSD_BT_CLASSIC=1`, still fails with "read failed").
- Sending: `hdl/send.rs` (driver), `hdl/gatt_client.rs` (discovery, weave client),
  `hdl/att.rs` (own ATT bearer on an L2CAP socket so the MTU exchange comes after the phone's
  private GATT server attaches), `hdl/outbound.rs` (sender state machine, BWU as responder),
  `hdl/join.rs` + `packaging/linux/quickshare-join` (join the phone's Wi-Fi Direct group),
  `hdl/l2cap.rs` (BLE L2CAP channel to the phone's advertised PSM, preferred over weave).
- `core_lib/src/bin/quickshare-direct.rs`: the daemon (notifications via notify-send actions:
  Accept/Decline with PIN, "Show in folder" via org.freedesktop.FileManager1.ShowItems).
- `packaging/linux/quickshare-ap`: root helper (Wi-Fi Direct GO via wpa_supplicant D-Bus GroupAdd
  with ht40/vht/he, or hostapd AP; dnsmasq; temporary nft rule).
- `flake.nix`: package + NixOS module `services.quickshare-direct` + dev shell (NixOS uses plain
  units `quickshare-ap.service`/`quickshare-join.service` for one configured user).
- Other distros: `Makefile` (`make install DESTDIR= PREFIX=`) installs everything from
  `packaging/linux/` (template units `quickshare-ap@USER`/`quickshare-join@USER` + polkit rule
  letting each active local user start their own, NM drop-in, firewalld/ufw, bt-connectable,
  user unit); `packaging/build-packages.sh` makes .deb/.rpm/.pkg.tar.zst/.tar.gz with fpm;
  `install.sh` is the one-line installer (picks the package by /etc/os-release).
  `hotspot::helper_unit` finds either unit flavour at runtime.
- CI: `.github/workflows/packages.yml` builds (Ubuntu 22.04, x86_64 + aarch64), install-tests
  in Debian/Ubuntu/Fedora/Arch/openSUSE containers (`packaging/ci/test-install.sh`), and
  publishes release `qsd-v<VERSION>` when `VERSION` changes on main (upstream rQuickShare tags
  v0.x already exist, hence the prefix). Bump `VERSION` to release.
- Not ours / unused: `app/` (rQuickShare's Tauri GUI; depends on core_lib, nothing depends on
  it; LAN-only sending; not built or tested, may not compile against today's core_lib), `snap/`,
  `BUILD.md`, `CHANGELOG.md`, release-please, `build.yml`/`lint.yml` (upstream `master` only).
  The README below "Repository layout" is upstream's text about that app.
- `tools/dev/`: test harness (USB adb drives the phone's share sheet; see below).

## Dev loop
- `nix develop`, then in `core_lib`: `cargo build --release --bin quickshare-direct`.
- The owner's laptop runs the installed service (`systemctl --user {stop,start} quickshare-direct`,
  logs `journalctl --user -u quickshare-direct`, helper logs `journalctl -u quickshare-ap`). Stop it
  before running a dev build (ports 46257/46258, BLE advertising).
- Changes to the helper/module only take effect after pushing, `nix flake update quickshare-direct`
  in /data/nixos-config and the owner running `nrs` (you can't sudo). During development use dev
  builds instead: the owner runs `sudo tools/dev/helper-watch.sh` once, then the dev build with
  `QSD_SYSTEMCTL=$PWD/tools/dev/helper-ctl.sh` runs the repo's helpers through it (receiver: stop
  the installed user service first). Release + flake update + nrs only when done.
- Test phone: Pixel 10 on USB adb (`ADB_SERIAL=...`). The Pixel drops Wi-Fi while Quick Share
  discovers devices, so wireless adb dies mid-test: use USB. `tools/dev/qs-auto.sh <image> <name>
  [receiver-log]` pushes an image, opens the share sheet → Quick Share → taps the device unless the
  receiver is already connected (Quick Share auto-connects to known devices, and a tap on a
  connecting tile cancels it). Accept prompts with `swaync-client -a 0`. Phone logs:
  `$QSD_TMP/phone-logcat.txt` (tags NearbyConnections/NearbyMediums/NearbySharing).
  Remove test images from the phone afterwards (MediaStore delete).
- Sending tests: `tools/dev/qs-receive-mode.sh` puts the phone on Quick Share's Receive screen,
  `tools/dev/qs-accept.sh` taps Accept (`QS_BUTTON=Decline`), then
  `core_lib/target/release/quickshare-direct send FILE...` (run it under `script -qec` to get the
  terminal output; without a tty it reports through notifications). Files land in the phone's
  `Download/Quick Share/`.

## State (2026-10-05)
Works end to end with a Pixel 10: prompt 0.4 s after the PIN, Wi-Fi Direct VHT80 at 33-37 MB/s
(300 MB), files land in ~/Downloads, "Show in folder" selects them. Sending works too
(`quickshare-direct send`, phone on its Receive screen → BLE → phone's Wi-Fi Direct group joined by
`quickshare-join` on qsc0, ~36 MB/s). Open items: see the Plan in docs/DEV_NOTES.md (Bluetooth
Classic, phone join ~4 s, LE link setup ~6 s when sending, upstreaming).
