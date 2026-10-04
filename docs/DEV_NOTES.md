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

## Plan

1. Put the real adapter address in the advertisement; make the BLE weave server handle
   repeated/concurrent connections (first contact must be reliable).
2. Bluetooth Classic (RFCOMM) first contact, the phone's preferred initial medium.
3. Bandwidth upgrade hosted by the laptop: WIFI_DIRECT (group owner) and WIFI_HOTSPOT, so the
   transfer runs at Wi-Fi speed with no shared network.
4. Fixed port for the WIFI_LAN upgrade listener (firewall-friendly) for the same-network case.
