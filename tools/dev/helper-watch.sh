#!/usr/bin/env bash
# helper-watch.sh: run the repo's root helpers (packaging/linux/quickshare-ap, quickshare-join) for a
# dev build, without reinstalling. Start it with sudo in a terminal and leave it running:
#   sudo tools/dev/helper-watch.sh
# then run the dev build with QSD_SYSTEMCTL=tools/dev/helper-ctl.sh (receiver or `send`); its
# start/stop/is-active requests for the helper units come here through /run/user/<uid>/qsd-dev.
# Helper output: this terminal and /run/user/<uid>/qsd-dev/<helper>.log. Ctrl-C stops everything.
# Environment passed on to the helpers: QS_SINGLE_CHANNEL, QS_MODE, QS_STA.
set -u
[ "$(id -u)" -eq 0 ] || { echo "run with sudo" >&2; exit 1; }
U=${SUDO_USER:?run it through sudo}
REPO=$(cd "$(dirname "$0")/../.." && pwd)
# NixOS: the helpers' tools aren't on root's PATH.
for t in iw dnsmasq busybox; do
  if ! command -v $t >/dev/null && command -v nix >/dev/null && [ -z "${QSD_WATCH_NIX:-}" ]; then
    exec env QSD_WATCH_NIX=1 nix shell nixpkgs#iw nixpkgs#dnsmasq nixpkgs#busybox nixpkgs#hostapd -c "$0" "$@"
  fi
done
D=/run/user/$(id -u "$U")/qsd-dev
mkdir -p "$D"; chown "$U" "$D"
declare -A PID
rundir() { case $1 in quickshare-ap) echo /run/quickshare ;; quickshare-join) echo /run/quickshare-join ;; esac; }
setstate() { echo "$2" > "$D/state.$1"; chown "$U" "$D/state.$1"; }
finish() { # base status
  local b=$1
  case $b in quickshare-ap) iw dev ap0 del 2>/dev/null ;; quickshare-join) iw dev qsc0 del 2>/dev/null ;; esac
  rm -rf "$(rundir "$b")"
  unset "PID[$b]"
  setstate "$b" "$2"
  echo "[watch] $b $2"
}
stop_all() { for b in "${!PID[@]}"; do kill -TERM "${PID[$b]}" 2>/dev/null; wait "${PID[$b]}" 2>/dev/null; finish "$b" inactive; done; exit 0; }
trap stop_all INT TERM
echo "[watch] serving helper requests from $D for $U (Ctrl-C to stop)"
while :; do
  for req in "$D"/req.*; do
    [ -e "$req" ] || continue
    b=${req##*/req.}; act=$(cat "$req"); rm -f "$req"
    case $b in quickshare-ap|quickshare-join) ;; *) echo "[watch] unknown helper $b"; continue ;; esac
    case $act in
      start)
        [ -n "${PID[$b]:-}" ] && kill -0 "${PID[$b]}" 2>/dev/null && continue
        r=$(rundir "$b"); mkdir -p "$r"
        echo "[watch] start $b"
        RUNTIME_DIRECTORY=$r QS_USER=$U QS_SINGLE_CHANNEL=${QS_SINGLE_CHANNEL:-auto} \
          QS_MODE=${QS_MODE:-p2p} ${QS_STA:+QS_STA=$QS_STA} \
          bash "$REPO/packaging/linux/$b" > >(tee "$D/$b.log" | sed "s/^/[$b] /") 2>&1 &
        PID[$b]=$!
        chown "$U" "$D/$b.log" 2>/dev/null
        setstate "$b" active
        ;;
      stop)
        if [ -n "${PID[$b]:-}" ]; then
          echo "[watch] stop $b"
          kill -TERM "${PID[$b]}" 2>/dev/null; wait "${PID[$b]}" 2>/dev/null
          finish "$b" inactive
        fi
        ;;
    esac
  done
  for b in "${!PID[@]}"; do
    if ! kill -0 "${PID[$b]}" 2>/dev/null; then
      wait "${PID[$b]}"; rc=$?
      finish "$b" "$([ $rc -eq 0 ] && echo inactive || echo failed)"
    fi
  done
  sleep 0.2
done
