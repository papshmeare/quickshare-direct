# Development notes (quickshare-direct)

Findings from testing against a **Pixel 10** (Android 16, Quick Share with AirDrop compatibility),
receiver `core_lib/examples/rx_service.rs` on NixOS (BlueZ 5.86, MediaTek MT7922).

## 2026-10-05: baseline with the BLE receiver branch

- **Send 1 (manual, phone off Wi-Fi):** the phone found the laptop over BLE, connected through the
  weave GATT socket, UKEY2 + PIN ok, file received over BLE, byte-identical.
  The receiver's WIFI_LAN upgrade offer failed (phone off Wi-Fi, and the upgrade listener used a
  random port blocked by the firewall); the phone answered `BandwidthUpgradeNegotiation`
  `UPGRADE_FAILURE` and the transfer stayed on BLE.
- **Send 2 (automated over USB adb):** the phone tried to connect and failed:
  - First medium tried: **BLUETOOTH (Classic)** to `XX:XX:XX:XX:17:20`, which is *not* the
    laptop's adapter address (`2C:9C:58:2C:E8:5E`). The advertisement carries a wrong/placeholder
    Bluetooth MAC, so Classic RFCOMM can never work until that is the real address.
  - Then **BLE**: `bleConnectImpl() failed`; the phone gave up after ~7 s (`operationResultCode 504`).
    The weave server handles one connection at a time (see BLE_RECEIVER_DISCOVERY.md §8), so BLE
    first contact is not yet reliable.
- **Mediums the phone offers for the connection** (phone log, `requestConnection`):
  `[WIFI_DIRECT, WIFI_HOTSPOT, WIFI_AWARE, USB, WIFI_LAN, BLE_L2CAP, BLUETOOTH, BLE, NFC]`;
  initial connection attempted over `[BLUETOOTH, BLE]`. Remote capabilities queried:
  `WIFI_DIRECT_GC, WIFI_DIRECT_GO, WIFI_HOTSPOT, WIFI_AWARE, STATION_CHANNEL`.
  So **Wi-Fi Direct and Wi-Fi Hotspot upgrades are on the phone's list**.
- **Laptop Wi-Fi (MT7922):** interface combinations allow `managed + P2P-client/GO` on 2 channels,
  so the laptop can host a Wi-Fi Direct group while staying on its normal Wi-Fi.
- Testing tip: the Pixel drops Wi-Fi when the Quick Share sheet opens, so wireless adb dies;
  use USB adb to drive the share sheet and read `logcat` (tags `NearbyConnections`,
  `NearbyMediums`, `NearbySharing`).

## 2026-10-05: Bluetooth Classic + hotspot upgrade (work in progress)

- Advertisement now carries the adapter's real MAC; `RfcommServer` (rfcomm.rs) registers the
  NearbySharing RFCOMM service (`a82efa21-ae5c-3dde-9bbc-f16da7b16c5a`, insecure, no pairing).
  The phone now pages the right address, but gets `PAGE_TIMEOUT`:
  - BlueZ keeps the adapter **not connectable** unless discoverable (`btmgmt info` lacks
    "connectable"). Fix needs root once: `btmgmt connectable on` (NixOS: a oneshot service).
  - Even when connectable/discoverable, still `PAGE_TIMEOUT` on MT7922; under investigation.
    The phone then falls back to BLE (weave) after ~5 s, which works.
- BLE weave throughput measured: **~5 KB/s** (960 KB in ~3 min). Unusable for real files,
  hence the bandwidth upgrade is essential.
- The phone's ConnectionRequest lists upgrade mediums
  `[WifiLan, WifiDirect, WifiAware, WifiHotspot, WebRtc, BleL2cap, Bluetooth, Ble, Nfc]` and its
  MediumMetadata (Wi-Fi IP, AP frequency, usable channels). A Pixel 10 that dropped Wi-Fi for
  discovery had already rejoined (same /24) by the time it sent the ConnectionRequest; the
  earlier WIFI_LAN upgrade only failed because the upgrade listener used a random port.
- New: `QSD_BWU_PORT` (fixed upgrade port), and a **WIFI_HOTSPOT upgrade** (hotspot.rs):
  temporary WPA2 hotspot via NetworkManager (`shared` mode → 10.42.0.1 + DHCP) on a virtual AP
  interface (`ap0`, created by a root oneshot unit the user may start via polkit), same channel
  as the station connection. Chosen when the phone isn't on our /24 (`QSD_BWU=lan|hotspot`
  forces one).

## 2026-10-05: goal reached, phone off Wi-Fi → laptop hotspot at Wi-Fi speed

End-to-end with a Pixel 10 that had dropped Wi-Fi: BLE discovery + weave first contact, UKEY2/PIN,
receiver starts a hotspot **in the background** (`quickshare-ap` helper: hostapd + dnsmasq on a
virtual `ap0` next to the station, same channel, up in ~120-250 ms), offers `WIFI_HOTSPOT`
credentials over the encrypted BLE channel, the phone joins (associates in ~1 s, TCP after ~4.7 s),
the payload moves to TCP. **75 MB in ~18 s (~4.2 MB/s), byte-identical**; the laptop's own Wi-Fi
stays connected; the hotspot is removed afterwards.

Fixes on the way:
- Starting the hotspot blocked the handshake (NetworkManager AP start took 3-10 s): the phone
  timed out. Now started in a background task right after the ConnectionResponse; offered as soon
  as it is up.
- NetworkManager scans on an idle AP interface delay `AP-ENABLED` by 3-6 s ("Reject scan trigger
  since one is already pending"): replaced by the hostapd helper, NM keeps off `ap0`.
- NixOS restarts wpa_supplicant whenever a Wi-Fi interface appears (udev); that dropped the
  station and the learned regulatory domain → "Failed to start AP functionality" (5 GHz no-IR).
  Fixed with a udev override for `ap0` and an explicit regdomain at boot.
- Upgrade mid-transfer: the drain loop gave up after 16 frames while payload chunks were still
  arriving over BLE, closing the old channel before the phone's LAST_WRITE → upgrade aborted.
  Now time-bounded (15 s) and keeps processing payload frames.
- mDNS and BLE carried different endpoint info (version bits, random vs fixed identity), so a
  phone rejoining Wi-Fi saw a "renamed" endpoint. Now one generator (utils::endpoint_info).
- Bluetooth Classic: the advertisement carried a captured phone's MAC; now opt-in
  (`QSD_BT_CLASSIC=1`) with the real MAC, because RFCOMM still fails ("read failed") and its
  retries delay BLE by ~5 s.

## 2026-10-05: Wi-Fi Direct + notification daemon

- A plain hotspot fails when the phone reconnects to its usual Wi-Fi at the same moment (one
  station radio: joining our hotspot took 12 s, over the phone's 15 s upgrade budget). The helper
  now creates a **Wi-Fi Direct group** (we are GO) via wpa_supplicant D-Bus (`P2PDevice.GroupAdd`
  on the station interface's object), in ~230 ms; phones join it on a separate P2P interface.
  Offered as `WIFI_DIRECT`; hotspot stays as fallback. 75 MB from a phone off Wi-Fi: byte-identical,
  ~5 MB/s; the phone took ~16 s to join but didn't time out.
- Phone on the same Wi-Fi → `WIFI_LAN` upgrade on the fixed port, connects in ~0.2 s.
- Fixed a stall at a random point mid-transfer: `handle()` raced the frame-length `read_exact`
  against the state-update channel in `select!`; a cancelled read lost bytes and desynced the
  stream. The length prefix is now read with cancel-safe single reads.
- `quickshare-direct` binary: receiver + notifications (Accept/Decline with PIN, "Open folder",
  "Open" for links, "Copy" for text) via notify-send actions; tested with swaync.

## 2026-10-05: faster prompt, 80 MHz link

- Consent before the upgrade: the phone holds back the Introduction (file details → our prompt)
  until an offered upgrade completes. The upgrade now runs after Accept (link prepared in the
  background): prompt 0.4-0.5 s after the PIN instead of 5-15 s.
- Wi-Fi Direct groups from D-Bus `GroupAdd` default to 802.11n 20 MHz (MCS 15, 144 Mbit/s,
  ~12 MB/s). `GroupAdd` takes `ht40`/`vht`/`he` booleans: with them the group runs VHT 80 MHz,
  234-390 Mbit/s. 75 MB in 4.4 s (~17 MB/s average, ~30 MB/s peak); phone join ~4.3 s.
- Transfer ids were constant ("ble-weave") for BLE sessions; clients that track ids (our
  notification daemon) skipped every transfer after the first. Now unique per session.

## 2026-10-05: throughput

300 MB from a Pixel 10 over the VHT80 Wi-Fi Direct group (link 585-780 Mbit/s, -50..-54 dBm):
33-37 MB/s average, 56-59 MB/s peaks; ~1 % CPU per MB/s on the receiver (not CPU-bound).
A run where the phone rejoined its home Wi-Fi mid-transfer averaged 16 MB/s (scan/association
dips on the phone side). The phone's home Wi-Fi was on the same channel (5220 MHz), so no
multi-channel hopping. HE (Wi-Fi 6) isn't available for P2P-GO on MT7922 (HE iftypes: managed,
AP only). Wi-Fi power save on the station made no measurable difference.

## 2026-10-05: 160 MHz?

- Wi-Fi Direct at 160 MHz isn't possible on MT7922 in the EU: every 5 GHz 160 MHz channel includes
  DFS channels, and the driver lacks `DFS_CONCURRENT` (GO on a DFS channel while the station is
  associated to an AP there); 6 GHz needs HE, which the chip doesn't offer for P2P-GO; an AP
  (HE-capable) must share the station's channel.
- The shared-router path with both devices at 160 MHz HE (phone 1200 Mbit/s, laptop 576-720
  Mbit/s link rates) gave 2.4 MB/s average for raw TCP phone → router → laptop (300 MB, `nc` from
  an adb shell; bursts to 16 MB/s, long stalls). Two air hops on one channel plus router behaviour:
  far below the direct VHT80 link (33-37 MB/s). Keep preferring Wi-Fi Direct.

## 2026-10-05: sending from Linux over BLE (spike)

First file sent laptop → Pixel 10 with no shared Wi-Fi (7.9 kB PNG, md5 identical, phone prompt
with matching PIN), using `hdl/gatt_client.rs` (now `quickshare-direct send`, `hdl/send.rs`).

- **Discovery.** With the Quick Share Receive screen open ("Temporarily visible to everyone") the
  phone advertises 0xFEF3 service data in the same format as our receiver (endpoint id, plaintext
  name, its BR/EDR MAC), extended + legacy, connectable, `isPrivateGatt=true`. Its advertising
  mediums don't include Bluetooth Classic (`advertisingMediums=[8,3,6,5,11,9,4,7]`), so BLE is the
  way in. The address is random and rotates; BlueZ's cache keeps stale entries, so only devices
  with an RSSI (seen in the current scan) count.
- **GATT.** Service 0xFEF3 with weave `…0101` (write) and `…0102` (**indicate**), plus a second
  0xFEF3 service with advertisement slots `00000000-0000-3000-8000-00000000000{0..4}`. The service
  only shows on a connection made through the current advertising set; a link left from an earlier
  set lists GAP/GATT only, so the client always reconnects fresh.
- **Weave client.** We send CONN_REQUEST (counter 0), the phone confirms, then the BLE-socket
  INTRODUCTION control frame `00 00 00 | 08 01 12 07 0a 03 fc9f5e 10 02`, then `[fc9f5e][len][frame]`
  data. BlueZ sends our packets as Write Command even though the characteristic only lists `write`;
  the phone accepts that.
- **MTU.** Indications from the phone over 20 bytes fail on its side with `failed with status 133`
  (`BleSocketOutputStream failed to write data`), although btmon shows a 517 MTU exchange and
  BlueZ confirming every indication. Likely the private GATT server doesn't pick up the MTU
  BlueZ exchanged right after connecting. Workaround: weave packet size 20 (the minimum), about
  1-2 s per handshake step. Open: get the phone to use a larger MTU.
- **Outbound fix.** rQuickShare's sender sent its encrypted PairedKeyEncryption right after the
  connection responses; the phone only switches its channel to encrypted ~30 ms later, discarded
  the frames ("invalid OfflineFrame … invalid tag (zero)") and then failed on "Incorrect sequence
  number". The sender now sends its PairedKeyEncryption when the phone's arrives.
- **Upgrade.** As receiver the phone initiates the bandwidth upgrade: it offered WIFI_LAN with its
  own ip:port (we only list WIFI_LAN in the ConnectionRequest). Outbound ignores it, so the file
  went over BLE. Without shared Wi-Fi the phone has to host (WIFI_DIRECT / WIFI_HOTSPOT
  credentials) and the laptop has to join as a client: the reverse of the receive side.

- **Upgrade, WIFI_LAN (done).** Outbound now follows the receiver's offer as the responder
  (`OutboundRequest::do_bwu`): TCP connect to the offered ip:port, plaintext CLIENT_INTRODUCTION
  with our endpoint id, read the ACK, encrypted LAST_WRITE on BLE, answer the phone's LAST_WRITE
  with SAFE_TO_CLOSE, stop at its SAFE_TO_CLOSE, plaintext DISCONNECTION, swap the socket. Frames
  that arrive during the drain (the consent response) are processed after the swap, so the file
  goes over Wi-Fi. 1 MB laptop → Pixel on the same Wi-Fi: upgrade ~2 s after the offer, data
  0.2 s, md5 identical. `tools/dev/qs-receive-mode.sh` puts the phone on the Receive screen.

- **Upgrade, WIFI_DIRECT (done).** With WIFI_DIRECT/WIFI_HOTSPOT in our ConnectionRequest
  (+ MediumMetadata `supports_5_ghz`, `ap_frequency` = our station's) the phone hosts a Wi-Fi
  Direct group on our channel (5220 MHz; without the 5 GHz flag it picked 2467) and sends
  ssid `DIRECT-…`, an 8-char passphrase, gateway 192.168.49.1 and a port. The root helper
  `quickshare-join` joins it on a second station interface `qsc0` via wpa_supplicant D-Bus
  (CreateInterface/AddNetwork/SelectNetwork) and gets 192.168.49.x by DHCP; wlp4s0 stays on the
  home Wi-Fi (the chip allows two managed interfaces on two channels). Credentials go to the
  helper in a 0600 file in the user's runtime dir. 300 MB laptop → Pixel 10: join 2 s after the
  offer, data 8.4 s (~36 MB/s, same as receiving), md5 identical. From launch: weave connected
  3.3 s, PIN prompt 8.6 s (BLE at 20-byte packets), upgraded 16.6 s, done 25 s.

- **CLI (done).** `quickshare-direct send [--to NAME] FILE...` / `devices`, and a
  "Send with Quick Share" desktop entry (Open With); terminal progress, or one notification
  updated in place without a terminal. Exit codes: 0 sent, 1 declined/failed, 2 several phones
  (use --to), 130 cancelled. The phone's upgrade offer sometimes arrives *after* its Accept and
  outbound didn't read frames while streaming, so the files went over BLE (~200 KB/s) until the
  phone dropped the link: the send driver now holds the files after Accept for up to 10 s until
  the upgrade is done. 70 MB in two files: 4.1 s over Wi-Fi Direct, md5 identical. A phone
  Decline now ends as Rejected (was Disconnected). GIO ignores MimeType wildcards (image/* etc.),
  so the Open With entry lists types explicitly; Thunar's Send To entry (share/Thunar/sendto, no
  MimeType) covers every file. Launched via `gio launch` like a file manager: 5 MB sent, md5 OK.
  blueman's ConnectionNotifier shows its own "Connected" popup for the BLE link.

- **Weave MTU (done).** Phone log of the first spike: `gatts_process_mtu_req: MTU 517` at
  04.529, `bluetooth_private_gatt::gatt::server: connected on tcb_idx` at 04.532: the private
  GATT server attaches after bluetoothd's immediate MTU exchange and keeps MTU 23. bluetoothd
  can't delay or repeat the exchange, so `hdl/att.rs` runs its own ATT client on an L2CAP
  socket (CID 4); with a client ATT socket on the link the kernel doesn't give the ATT channel
  to bluetoothd. Discovery first, then the MTU exchange (like an Android central): MTU 517,
  weave packets 509, indications fine. The phone also sends us discovery requests on that
  bearer (`08 0100 ffff 002a`), answered with Attribute Not Found. A fixed-channel connect()
  returns at once; the socket is usable once `send_mtu()` succeeds. Same file, same phone:
  weave connected → Accept prompt 1.3 s (bluetoothd path: 5.5 s), start → prompt 8.4 s (12.3 s).
  The remaining ~6 s is LE link setup, the same on both paths. `QSD_WEAVE_ATT=0` uses
  bluetoothd (20-byte packets), which is also the fallback.

## 2026-10-07: BLE listener no longer keeps discovery open (issue #1)

rQuickShare's `BleListener` ran an unfiltered BlueZ discovery for the daemon's lifetime
(`Discovering: yes`); on a MediaTek MT7922 that stops BlueZ's background scan from reconnecting
bonded LE mice/keyboards. Its only use is an mDNS re-announce when a nearby device starts
sharing (helps a phone on the same Wi-Fi a little). Now (`hdl/ble.rs`, bluer instead of btleplug):
a passive advertisement monitor (AdvertisementMonitorManager1, pattern: service data 0xFE2C) when
BlueZ offers it, else no scan at all. BlueZ 5.86 here (and distribution defaults) only offers the
monitor in experimental mode (`RegisterMonitor` → UnknownMethod), so in practice: no scan.
Verified: receiver running → `Discovering: no`. `QSD_BLE_SCAN=discovery` restores the old
behaviour (with a warning), `QSD_BLE_SCAN=off`/`QSD_NO_BLE_SCAN` disables the listener.

## 2026-10-07: station on a DFS channel → no Wi-Fi Direct group

On a new network (router on channel 100, 5500 MHz, DFS in EE) a receive stayed on Bluetooth
(147 KB in ~39 s): `GroupAdd` with frequency 5500 → "Did not receive correct message arguments"
for all three variants, and the hotspot fallback can't start there either (hostapd: channel not
in the list). A group owner/AP is a master device and must do its own radar detection on DFS
channels; the station side is the router's job. The helper now checks `iw phy` for the station
frequency and, if it's marked radar detection / no-IR / disabled, puts the group on the first
usable channel (5180…5240, 5745…5805, then 2.4 GHz); the chip runs it on a second channel next to
the station (#channels <= 2 with P2P-GO). The hotspot fallback needs the station's channel and
now fails at once in that case, and the receiver stops waiting as soon as the helper unit fails
(it used to wait the full 15 s). Credentials report the group's real frequency.

## 2026-10-07: single-channel fallback, scan guard, faster give-up

Follow-up to the DFS case. With the station on channel 100 the group came up on 5180, but phones
never saw it (8 phone scans, no `DIRECT-…`; a 2.4 GHz group next to the 5 GHz station likewise):
the MT7922 reports a P2P-GO on a second channel but never beacons there. As a *client* on a second
channel it works, slowly (sending: phone group on 5765/5805, 2.5-3.5 MB/s instead of ~36).
- `QS_SINGLE_CHANNEL` (helpers; NixOS `singleChannel`): auto (default) = receive: if the station
  channel can't host the link, `nmcli device disconnect` the station for the transfer and host the
  group alone on 5180; send: join next to the station first, and only if that fails disconnect and
  retry; always = disconnect whenever the channels differ (full speed); never.
- Disconnecting made NetworkManager scan all channels (`iw event`: 8 s, 35.5-43.6), which took the
  radio off channel 36 exactly while the phone associated and DHCP'd: Nearby gives a join attempt
  ~4-5 s (associate + DHCP), so it failed, and the next attempt after the scan worked. The helpers
  now abort station scans (`iw event` → `iw dev STA scan abort`) while they hold the radio.
  Result: phone joins on the first attempt, offer → TCP 4.1 s, 480 KB photo done in 4.6 s, station
  back 0.4 s after (internet paused ~6 s). Before: ~2 min over Bluetooth.
- The receiver keeps reading the Bluetooth channel while waiting for the phone's TCP connection and
  stops at the phone's UPGRADE_FAILURE (it waited the full 30 s before).
- The group owner also offers P2P IP address allocation in EAPOL-Key (P2PDeviceConfig IpAddrGo..,
  pool .100-.199, dnsmasq keeps .10-.99); this Pixel doesn't use it (joins with credentials,
  "provisioning mode: 0") and DHCPs anyway.
- Dev loop without reinstalling: `sudo tools/dev/helper-watch.sh` + dev build with
  `QSD_SYSTEMCTL=tools/dev/helper-ctl.sh` runs the repo's helpers. `tools/dev/p2p-go-test.sh FREQ`
  starts a bare group owner for scan tests.
- Seen once after many Wi-Fi mode switches: BlueZ refused discovery (`InProgress`) with nothing
  discovering; `bluetoothctl power off/on` fixed it.

## 2026-10-07: sending over BLE L2CAP; LE connection interval

BLE_L2CAP isn't a bandwidth-upgrade medium (Nearby's UpgradePathInfo has no value 10); it replaces
the GATT/weave socket for the Bluetooth leg. The receiver advertises a PSM in the extra fields of
its 0xFEF3 BleAdvertisement (after the 2-byte device token: mask byte, bit 0 → 2-byte PSM; the
Pixel uses 128 or 161). On the channel every message is [4-byte BE length][payload]: we send
0x03 (request data connection), the phone answers 0x17 (ready); then the BLE socket layer as over
weave ([00 00 00][SocketControlFrame] INTRODUCTION, data packets [fc9f5e][bytes], and a
PACKET_ACKNOWLEDGEMENT control frame for every data packet received). `hdl/l2cap.rs`; `send` uses
it when a PSM is advertised and falls back to weave (QSD_BLE_L2CAP=0 forces weave).
- Link up 0.9-2.7 s (GATT: 6-7 s), PIN prompt 1.6 s after connecting.
- A 512 KB chunk (one frame) stalls the phone (it reads one L2CAP SDU per message; 64 KB > its
  65535 MTU also breaks); 1 KB (Nearby's own size) and 16 KB work; default 16 KB.
- Throughput was ~5 KB/s with 1 KB or 16 KB chunks, headphones on or off: one ~250-byte packet per
  connection event at the kernel's 30-50 ms LE connection interval (debugfs conn_min/max_interval
  24/40). With 6/9 (7.5-11 ms): 46 KB/s (16 KB chunks), 38.5 KB/s (1 KB). The boot unit
  (packaging/linux/quickshare-bt-setup) now sets QS_LE_CONN_INTERVAL="6 12" (NixOS
  leConnInterval); `tools/dev/le-conn-interval.sh` shows/sets it.
- "Sent" used to be reported once everything was buffered; over Bluetooth that's long before the
  phone has it (a 1 MB file arrived with 734 KB). On a Bluetooth link the sender now waits up to
  5 min for the phone to close the connection (which it does once complete).
- With Wi-Fi: L2CAP handshake, then the phone's Wi-Fi Direct group: 30 MB at 6.8 MB/s (joined on a
  second channel next to the station on channel 100).

## Plan

1. Put the real adapter address in the advertisement; make the BLE weave server handle
   repeated/concurrent connections (first contact must be reliable).
2. Bluetooth Classic (RFCOMM) first contact, the phone's preferred initial medium.
3. Bandwidth upgrade hosted by the laptop: WIFI_HOTSPOT (done), WIFI_DIRECT (optional).
4. Fixed port for the WIFI_LAN upgrade listener (firewall-friendly) for the same-network case. (done)
5. App integration: desktop notification when a file arrives, with an "Open folder" action.
6. Sending works (`quickshare-direct send`, file manager entry, weave at MTU 517). Next: LE link
   setup ~6 s (scan/connection parameters?), text/URL payloads, folders, waking a phone that isn't
   on its Receive screen (the "device nearby is sharing" beacon).
7. BLE L2CAP: sending done (above). Receiving: advertise a PSM and listen, so phones can use it
   too (their GATT connect to us is already fast; matters for the no-Wi-Fi fallback).
