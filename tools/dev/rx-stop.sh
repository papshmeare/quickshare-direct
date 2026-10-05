#!/usr/bin/env bash
T=${QSD_TMP:-/tmp/qsd-dev}; [ -f $T/rx.pid ] && kill "$(cat $T/rx.pid)" 2>/dev/null && echo stopped; rm -f $T/rx.pid
