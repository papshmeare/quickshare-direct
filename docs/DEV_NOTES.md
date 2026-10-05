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

## Plan

1. Put the real adapter address in the advertisement; make the BLE weave server handle
   repeated/concurrent connections (first contact must be reliable).
2. Bluetooth Classic (RFCOMM) first contact, the phone's preferred initial medium.
3. Bandwidth upgrade hosted by the laptop: WIFI_HOTSPOT (done), WIFI_DIRECT (optional).
4. Fixed port for the WIFI_LAN upgrade listener (firewall-friendly) for the same-network case. (done)
5. App integration: desktop notification when a file arrives, with an "Open folder" action.
