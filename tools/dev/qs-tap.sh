#!/usr/bin/env bash
mkdir -p "${QSD_TMP:-/tmp/qsd-dev}"
# qs-tap.sh <target>: tap a device tile in an already open Quick Share screen (USB adb), with logcat
A="adb ${ADB_SERIAL:+-s $ADB_SERIAL}"; T=${QSD_TMP:-/tmp/qsd-dev}
pkill -f "adb ${ADB_SERIAL:+-s $ADB_SERIAL} logcat -v tim[e]" 2>/dev/null; $A logcat -c; ($A logcat -v time > $T/phone-logcat.txt 2>&1 &)
$A shell uiautomator dump /data/local/tmp/ui.xml >/dev/null 2>&1; $A pull /data/local/tmp/ui.xml $T/ui.xml >/dev/null 2>&1
b=$(python3 - "$1" "$T/ui.xml" <<'PY'
import sys,re,xml.etree.ElementTree as E
want=sys.argv[1].lower()
for n in E.parse(sys.argv[2]).iter('node'):
    if (n.get('text','')+' '+n.get('content-desc','')).lower().strip().startswith(want):
        x1,y1,x2,y2=map(int,re.findall(r'\d+',n.get('bounds'))); print((x1+x2)//2,(y1+y2)//2); break
PY
)
[ -n "$b" ] && $A shell input tap $b && echo "tapped $1 at $b" || echo "no $1 tile"
