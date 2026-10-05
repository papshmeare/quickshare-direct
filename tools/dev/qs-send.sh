#!/usr/bin/env bash
# qs-send.sh <local-file> : push to the phone, index it, open Quick Share's send sheet for it
set -e
A="adb ${ADB_SERIAL:+-s $ADB_SERIAL}"
f=$1; n=$(basename "$f")
$A push "$f" "/sdcard/Pictures/$n" >/dev/null
$A shell "am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE -d file:///sdcard/Pictures/$n >/dev/null"
sleep 2
id=$($A shell "content query --uri content://media/external/images/media --projection _id --where \"_display_name='$n'\"" | sed -n 's/.*_id=\([0-9]*\).*/\1/p' | tail -1)
echo "media id: $id"
$A shell "am start -a android.intent.action.SEND -t image/png --eu android.intent.extra.STREAM content://media/external/images/media/$id --grant-read-uri-permission -n com.google.android.gms/.nearby.sharing.main.MainActivity" | tail -1
