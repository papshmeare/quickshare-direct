#!/usr/bin/env bash
mkdir -p "${QSD_TMP:-/tmp/qsd-dev}"
# qs-receive-mode.sh: put the phone (USB adb) on the Quick Share Receive screen, which makes it
# advertise as a receiver ("Temporarily visible to everyone"). Exits 1 if it doesn't get there.
A="adb ${ADB_SERIAL:+-s $ADB_SERIAL}"; T=${QSD_TMP:-/tmp/qsd-dev}
ui() { $A shell uiautomator dump /data/local/tmp/ui.xml >/dev/null 2>&1; $A pull /data/local/tmp/ui.xml $T/ui.xml >/dev/null 2>&1; }
tap() { # tap the first node whose text is exactly $1
  b=$(python3 - "$1" "$T/ui.xml" <<'PY'
import sys,re,xml.etree.ElementTree as E
for n in E.parse(sys.argv[2]).iter('node'):
    if n.get('text','')==sys.argv[1]:
        x1,y1,x2,y2=map(int,re.findall(r'\d+',n.get('bounds'))); print((x1+x2)//2,(y1+y2)//2); break
PY
)
  [ -n "$b" ] && $A shell input tap $b
}
$A shell input keyevent KEYCODE_WAKEUP; $A shell wm dismiss-keyguard >/dev/null 2>&1
$A shell am start -W -a com.google.android.gms.nearby.sharing.QUICK_SETTINGS >/dev/null 2>&1
for i in 1 2 3 4 5; do
  sleep 1.5; ui
  grep -q 'text="Sharing with you"' $T/ui.xml && { echo "phone ready to receive"; exit 0; }
  tap Receive
done
echo "phone not on the Receive screen"; exit 1
