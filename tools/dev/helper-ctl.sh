#!/usr/bin/env bash
# helper-ctl.sh {start|stop|is-active} UNIT: stands in for systemctl in a dev build
# (QSD_SYSTEMCTL=tools/dev/helper-ctl.sh) and hands the request to tools/dev/helper-watch.sh.
set -u
act=$1; unit=${2:?unit}
b=${unit%.service}; b=${b%%@*}
D=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/qsd-dev
[ -d "$D" ] || { echo "helper-watch.sh isn't running (sudo tools/dev/helper-watch.sh)" >&2; exit 1; }
state() { cat "$D/state.$b" 2>/dev/null || echo inactive; }
case $act in
  start) rm -f "$D/state.$b"; echo start > "$D/req.$b"
         for _ in $(seq 50); do [ -e "$D/state.$b" ] && break; sleep 0.1; done
         [ -e "$D/state.$b" ] || { echo "helper-watch.sh didn't answer" >&2; exit 1; } ;;
  stop)  echo stop > "$D/req.$b"
         for _ in $(seq 100); do [ "$(state)" = active ] || break; sleep 0.1; done ;;
  is-active) s=$(state); echo "$s"; [ "$s" = active ] ;;
  *) echo "unsupported: $act" >&2; exit 1 ;;
esac
