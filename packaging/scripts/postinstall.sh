#!/bin/sh
# quickshare-direct package post-install (deb/rpm/pacman). Must not fail the install: no systemd
# in a container/chroot, NetworkManager not running, ... are all fine.
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload >/dev/null 2>&1 || :
  # Phones must be able to connect over Bluetooth.
  systemctl enable --now quickshare-bt-connectable.service >/dev/null 2>&1 || :
  # Polkit picks up new rules itself; NetworkManager needs a reload for its conf.d drop-in.
  systemctl try-reload-or-restart NetworkManager.service >/dev/null 2>&1 || :
fi
# The receiver runs in every user's graphical session (starts at the next login; the installer
# script also starts it for the current user).
systemctl --global enable quickshare-direct.service >/dev/null 2>&1 || :
exit 0
