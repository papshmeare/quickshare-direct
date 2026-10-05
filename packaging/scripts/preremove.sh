#!/bin/sh
# quickshare-direct package pre-remove. Arguments: deb "remove"/"upgrade"/..., rpm "0" (erase) or
# "1" (upgrade), pacman the old version (only on removal).
case "${1:-remove}" in
  upgrade|1) exit 0 ;;
esac
systemctl --global disable quickshare-direct.service >/dev/null 2>&1 || :
if [ -d /run/systemd/system ]; then
  systemctl disable --now quickshare-bt-connectable.service >/dev/null 2>&1 || :
  systemctl stop 'quickshare-ap@*.service' 'quickshare-join@*.service' >/dev/null 2>&1 || :
fi
exit 0
