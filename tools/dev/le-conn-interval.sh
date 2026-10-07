#!/usr/bin/env bash
# le-conn-interval.sh [MIN MAX]: show (and with arguments set, until reboot) the LE connection
# interval Linux asks for when it connects, in 1.25 ms units (kernel default 24-40 = 30-50 ms).
# Run as root. e.g. sudo tools/dev/le-conn-interval.sh 6 9   (7.5-11.25 ms)
set -euo pipefail
D=/sys/kernel/debug/bluetooth/${HCI:-hci0}
[ -d "$D" ] || { echo "no $D (debugfs not mounted?)" >&2; exit 1; }
echo "before: min $(cat "$D/conn_min_interval") max $(cat "$D/conn_max_interval")"
if [ $# -eq 2 ]; then
  # Lower the minimum first, so min <= max holds at every step.
  if [ "$1" -le "$(cat "$D/conn_max_interval")" ]; then
    echo "$1" > "$D/conn_min_interval"; echo "$2" > "$D/conn_max_interval"
  else
    echo "$2" > "$D/conn_max_interval"; echo "$1" > "$D/conn_min_interval"
  fi
  echo "now:    min $(cat "$D/conn_min_interval") max $(cat "$D/conn_max_interval")"
fi
