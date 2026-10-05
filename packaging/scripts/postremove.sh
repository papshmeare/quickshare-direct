#!/bin/sh
# quickshare-direct package post-remove: drop the removed units and NetworkManager drop-in.
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload >/dev/null 2>&1 || :
  systemctl try-reload-or-restart NetworkManager.service >/dev/null 2>&1 || :
fi
exit 0
