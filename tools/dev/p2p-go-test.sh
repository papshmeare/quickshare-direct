#!/usr/bin/env bash
# p2p-go-test.sh FREQ [SECONDS] [STA]: start a Wi-Fi Direct group owner on FREQ (MHz) through
# wpa_supplicant for SECONDS (default 40), then remove it. Run as root. Checks whether a phone can
# see a group on a given channel next to the station connection (scan from the phone meanwhile:
# adb shell cmd wifi start-scan; adb shell cmd wifi list-scan-results).
set -euo pipefail
# NixOS: iw isn't on root's PATH.
if ! command -v iw >/dev/null && command -v nix >/dev/null; then exec nix shell nixpkgs#iw -c "$0" "$@"; fi
FREQ=$1; SECS=${2:-40}
STA=${3:-$(iw dev | awk '/Interface/{i=$2} /type managed/{print i; exit}')}
WPAS=fi.w1.wpa_supplicant1
obj() {
  local p
  for p in $(busctl get-property $WPAS /fi/w1/wpa_supplicant1 $WPAS Interfaces | grep -o '"/[^"]*"' | tr -d '"'); do
    [ "$(busctl get-property $WPAS "$p" $WPAS.Interface Ifname 2>/dev/null)" = "s \"$1\"" ] && { echo "$p"; return 0; }
  done
  return 1
}
DEV=$(obj "$STA")
before=$(ls /sys/class/net)
busctl call $WPAS "$DEV" $WPAS.Interface.P2PDevice GroupAdd 'a{sv}' 2 persistent b false frequency i "$FREQ" >/dev/null
sleep 2
IF=$(comm -13 <(echo "$before" | sort) <(ls /sys/class/net | sort) | grep '^p2p-' | head -1)
GOBJ=$(obj "$IF")
trap 'busctl call $WPAS "$GOBJ" $WPAS.Interface.P2PDevice Disconnect >/dev/null 2>&1 || true; echo "group removed"' EXIT
iw dev "$IF" info | grep -E "ssid|channel|txpower"
iw dev "$STA" info | grep channel | sed 's/^/station: /'
echo "group up for $SECS s"
sleep "$SECS"
