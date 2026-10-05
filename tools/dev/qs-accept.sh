#!/usr/bin/env bash
mkdir -p "${QSD_TMP:-/tmp/qsd-dev}"
# qs-accept.sh [tries]: wait for the phone's (USB adb) incoming-transfer prompt and tap Accept
# (QS_BUTTON=Decline taps that instead).
A="adb ${ADB_SERIAL:+-s $ADB_SERIAL}"; T=${QSD_TMP:-/tmp/qsd-dev}
for i in $(seq 1 "${1:-30}"); do
  rm -f $T/ui.xml # never act on a leftover dump
  $A shell uiautomator dump /data/local/tmp/ui.xml >/dev/null 2>&1; $A pull /data/local/tmp/ui.xml $T/ui.xml >/dev/null 2>&1 || { sleep 2; continue; }
  b=$(python3 - "$T/ui.xml" "${QS_BUTTON:-Accept}" <<'PY'
import sys,re,xml.etree.ElementTree as E
for n in E.parse(sys.argv[1]).iter('node'):
    if n.get('text','')==sys.argv[2]:
        x1,y1,x2,y2=map(int,re.findall(r'\d+',n.get('bounds'))); print((x1+x2)//2,(y1+y2)//2); break
PY
)
  [ -n "$b" ] && { $A shell input tap $b; echo "tapped ${QS_BUTTON:-Accept}"; exit 0; }
  sleep 2
done
echo "no ${QS_BUTTON:-Accept} button"; exit 1
