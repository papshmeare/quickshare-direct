#!/usr/bin/env bash
mkdir -p "${QSD_TMP:-/tmp/qsd-dev}"
# rx-start.sh <logname>: start the Quick Share test receiver in the background (pid in rx.pid)
T=${QSD_TMP:-/tmp/qsd-dev}; cd /data/code/quickshare-direct/core_lib
[ -f $T/rx.pid ] && kill "$(cat $T/rx.pid)" 2>/dev/null; sleep 1
QSD_DIR=$HOME/Downloads/quickshare-test QSD_PORT=46257 QSD_BWU_PORT=46258 QSD_NAME=mepc RUST_LOG="${RUST_LOG:-info,rqs_lib=debug,mdns_sd=error,polling=error,neli=error,bluez_async=error,btleplug=error}" \
  setsid ./target/debug/examples/rx_service > "$T/$1" 2>&1 < /dev/null &
echo $! > $T/rx.pid; echo "started pid $!"
