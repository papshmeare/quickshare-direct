#!/usr/bin/env bash
mkdir -p "${QSD_TMP:-/tmp/qsd-dev}"
# qs-auto.sh <local-image> <target-name> [receiver-log]: send an image from the phone (USB adb) to
# <target-name> via Quick Share. With receiver-log: don't tap the tile once the receiver already has
# a connection (Quick Share may reconnect by itself, and tapping a connecting tile cancels it).
set -u
A="adb ${ADB_SERIAL:+-s $ADB_SERIAL}"; f=$1; n=$(basename "$f"); target=$2; T=${QSD_TMP:-/tmp/qsd-dev}
RLOG=${3:-}; RSTART=$( [ -n "$RLOG" ] && wc -l < "$RLOG" || echo 0 )
connected() { [ -n "$RLOG" ] && tail -n +"$((RSTART+1))" "$RLOG" | grep -q -E 'weave: connected|RfcommServer: connection'; }
$A shell input keyevent KEYCODE_BACK; $A shell input keyevent KEYCODE_HOME; sleep 1
$A push "$f" "/sdcard/Pictures/$n" >/dev/null
$A shell "am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE -d file:///sdcard/Pictures/$n >/dev/null"; sleep 2
id=$($A shell "content query --uri content://media/external/images/media --projection _id --where \"_display_name='$n'\"" | sed -n 's/.*_id=\([0-9]*\).*/\1/p' | tail -1)
echo "media id $id"
pkill -f "adb ${ADB_SERIAL:+-s $ADB_SERIAL} logcat -v tim[e]" 2>/dev/null; $A logcat -c; ($A logcat -v time > $T/phone-logcat.txt 2>&1 &)
$A shell "am start -a android.intent.action.CHOOSER --eu android.intent.extra.INTENT 'intent:#Intent;action=android.intent.action.SEND;type=image/png;S.android.intent.extra.STREAM=content://media/external/images/media/$id;launchFlags=0x1;end'" >/dev/null
sleep 4
tapnode() { # tap the first UI node whose text/desc contains $1; returns 1 if not found
  $A shell uiautomator dump /data/local/tmp/ui.xml >/dev/null 2>&1; $A pull /data/local/tmp/ui.xml $T/ui.xml >/dev/null 2>&1
  b=$(python3 - "$1" "$T/ui.xml" <<'PY'
import sys,re,xml.etree.ElementTree as E
want=sys.argv[1].lower()
for n in E.parse(sys.argv[2]).iter('node'):
    if want in (n.get('text','')+' '+n.get('content-desc','')).lower():
        x1,y1,x2,y2=map(int,re.findall(r'\d+',n.get('bounds'))); print((x1+x2)//2,(y1+y2)//2); break
PY
)
  [ -n "$b" ] || return 1; $A shell input tap $b; echo "tapped '$1' at $b"
}
tapnode "Quick Share" || { echo "no Quick Share button"; exit 1; }
sleep 8; for i in $(seq 1 15); do sleep 3; if connected; then echo "receiver already connected, not tapping"; break; fi; tapnode "$target" && break; done
