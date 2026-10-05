#!/bin/sh
# quickshare-direct installer: Quick Share (Nearby Share) for Linux, no shared Wi-Fi needed.
#
#   curl -fsSL https://raw.githubusercontent.com/papshmeare/quickshare-direct/main/install.sh | sh
#
# Installs the latest release with the system package manager (Debian/Ubuntu .deb, Fedora .rpm,
# Arch .pkg.tar.zst; other distributions: the generic tarball), opens the Quick Share ports if a
# firewall is active, and starts the receiver for the current user.
#   sh install.sh --uninstall     remove it again
# Environment: QSD_VERSION (release tag, default latest), QSD_FORMAT (deb|rpm|pacman|tar),
#              QSD_NO_FIREWALL=1 (leave the firewall alone), QSD_NO_START=1 (don't start it),
#              QSD_PACKAGE_DIR (take the package files from this directory; CI tests).
set -eu

REPO=papshmeare/quickshare-direct
say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

if [ "$(id -u)" -eq 0 ]; then
  SUDO=
else
  command -v sudo >/dev/null || die "needs root: install sudo or run as root"
  SUDO=sudo
fi

[ "$(uname -s)" = Linux ] || die "Linux only"
case $(uname -m) in
  x86_64 | amd64) ARCH=x86_64 DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64 DEB_ARCH=arm64 ;;
  *) die "unsupported CPU architecture $(uname -m) (x86_64 and aarch64 have packages)" ;;
esac

# shellcheck disable=SC1091
[ -r /etc/os-release ] && . /etc/os-release
ID=${ID:-unknown}
LIKE=" $ID ${ID_LIKE:-} "
if [ "$ID" = nixos ]; then
  die "NixOS: use the flake's module instead, see https://github.com/$REPO#install-nixos"
fi
FORMAT=${QSD_FORMAT:-}
if [ -z "$FORMAT" ]; then
  case $LIKE in
    *" debian "* | *" ubuntu "*) FORMAT=deb ;;
    *" arch "*) FORMAT=pacman ;;
    *" fedora "* | *" rhel "* | *" centos "*) FORMAT=rpm ;;
    *) FORMAT=tar ;;
  esac
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

if [ "${1:-}" = --uninstall ]; then
  systemctl --user disable --now quickshare-direct.service 2>/dev/null || :
  case $FORMAT in
    deb) $SUDO apt-get remove -y quickshare-direct ;;
    rpm) $SUDO dnf remove -y quickshare-direct ;;
    pacman) $SUDO pacman -R --noconfirm quickshare-direct ;;
    tar)
      if [ -r /usr/local/share/quickshare-direct/files ]; then
        $SUDO sh /usr/local/share/quickshare-direct/preremove.sh remove || :
        # shellcheck disable=SC2046
        $SUDO rm -f $(cat /usr/local/share/quickshare-direct/files)
        $SUDO rm -rf /usr/local/share/quickshare-direct /usr/local/lib/quickshare-direct
        $SUDO systemctl daemon-reload || :
      else
        die "no tarball install found"
      fi
      ;;
  esac
  say "quickshare-direct removed"
  exit 0
fi

[ -d /run/systemd/system ] || warn "systemd isn't running here: the packages install, but nothing starts (quickshare-direct needs systemd)"
if [ "$(id -u)" -eq 0 ] && [ -z "${SUDO_USER:-}" ] && [ -z "${QSD_NO_START:-}" ]; then
  warn "running as root: the receiver is started for no user (log in as a normal user and run 'systemctl --user enable --now quickshare-direct')"
fi

fetch() { # url dest
  if [ -n "${QSD_PACKAGE_DIR:-}" ]; then cp "$QSD_PACKAGE_DIR/$(basename "$1")" "$2"; return; fi
  if command -v curl >/dev/null; then curl -fL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null; then wget -O "$2" "$1"
  else die "needs curl or wget"; fi
}
if [ -n "${QSD_VERSION:-}" ]; then
  BASE=https://github.com/$REPO/releases/download/$QSD_VERSION
else
  BASE=https://github.com/$REPO/releases/latest/download
fi

case $FORMAT in
  deb)
    FILE=quickshare-direct_$DEB_ARCH.deb
    say "downloading $FILE"
    fetch "$BASE/$FILE" "$TMP/$FILE"
    chmod 644 "$TMP/$FILE" && chmod 755 "$TMP" # apt's sandbox user must read it
    say "installing with apt"
    $SUDO apt-get update -q || warn "apt-get update failed; trying the install anyway"
    $SUDO apt-get install -y "$TMP/$FILE"
    ;;
  rpm)
    FILE=quickshare-direct.$ARCH.rpm
    say "downloading $FILE"
    fetch "$BASE/$FILE" "$TMP/$FILE"
    say "installing with dnf"
    if command -v dnf >/dev/null; then $SUDO dnf install -y "$TMP/$FILE"
    else $SUDO yum install -y "$TMP/$FILE"; fi
    ;;
  pacman)
    FILE=quickshare-direct-$ARCH.pkg.tar.zst
    say "downloading $FILE"
    fetch "$BASE/$FILE" "$TMP/$FILE"
    say "installing with pacman"
    $SUDO pacman -U --noconfirm --needed "$TMP/$FILE"
    ;;
  tar)
    FILE=quickshare-direct-$ARCH.tar.gz
    say "no package for '$ID'; installing the generic build to /usr/local"
    warn "install these yourself if missing: bluez (with btmgmt), iw, iproute2, wpa_supplicant, dnsmasq, polkit (>= 0.106), libdbus, NetworkManager, libnotify (notify-send), xdg-utils, busybox (optional)"
    fetch "$BASE/$FILE" "$TMP/$FILE"
    mkdir "$TMP/root"
    tar -C "$TMP/root" -xzf "$TMP/$FILE"
    ROOT=$TMP/root
    # The tarball is built for /usr; put it under /usr/local, except files whose location is
    # fixed (systemd, polkit, NetworkManager, firewalld, /etc).
    $SUDO install -d /usr/local/bin /usr/local/lib/quickshare-direct /usr/local/share/quickshare-direct
    LIST=$TMP/files
    : > "$LIST"
    put() { # src dest mode
      $SUDO install -Dm"$3" "$1" "$2" && echo "$2" >> "$LIST"
    }
    put "$ROOT/usr/bin/quickshare-direct" /usr/local/bin/quickshare-direct 755
    for f in quickshare-ap quickshare-join; do
      put "$ROOT/usr/lib/quickshare-direct/$f" "/usr/local/lib/quickshare-direct/$f" 755
    done
    for f in "$ROOT"/usr/lib/systemd/system/*; do
      sed -e 's|/usr/lib/quickshare-direct|/usr/local/lib/quickshare-direct|' "$f" > "$f.new"
      put "$f.new" "/etc/systemd/system/$(basename "$f")" 644
    done
    sed -e 's|/usr/bin/quickshare-direct|/usr/local/bin/quickshare-direct|' \
      "$ROOT/usr/lib/systemd/user/quickshare-direct.service" > "$TMP/user.service"
    put "$TMP/user.service" /etc/systemd/user/quickshare-direct.service 644
    put "$ROOT/usr/share/polkit-1/rules.d/50-quickshare-direct.rules" /etc/polkit-1/rules.d/50-quickshare-direct.rules 644
    put "$ROOT/usr/lib/NetworkManager/conf.d/quickshare-direct.conf" /etc/NetworkManager/conf.d/quickshare-direct.conf 644
    put "$ROOT/usr/lib/firewalld/services/quickshare-direct.xml" /etc/firewalld/services/quickshare-direct.xml 644
    put "$ROOT/usr/share/applications/quickshare-direct-send.desktop" /usr/local/share/applications/quickshare-direct-send.desktop 644
    put "$ROOT/usr/share/Thunar/sendto/quickshare-direct.desktop" /usr/local/share/Thunar/sendto/quickshare-direct.desktop 644
    [ -e /etc/default/quickshare-direct ] || put "$ROOT/etc/default/quickshare-direct" /etc/default/quickshare-direct 644
    [ -d /etc/ufw/applications.d ] && put "$ROOT/etc/ufw/applications.d/quickshare-direct" /etc/ufw/applications.d/quickshare-direct 644
    $SUDO install -m644 "$LIST" /usr/local/share/quickshare-direct/files
    $SUDO install -m755 "$ROOT/.quickshare-direct/preremove.sh" /usr/local/share/quickshare-direct/preremove.sh
    $SUDO sh "$ROOT/.quickshare-direct/postinstall.sh"
    ;;
  *) die "unknown QSD_FORMAT '$FORMAT'" ;;
esac

# Ports for transfers from a phone on the same Wi-Fi (the direct Wi-Fi link opens its own
# interface while it exists). Only touches a firewall that is running.
if [ -z "${QSD_NO_FIREWALL:-}" ]; then
  if command -v firewall-cmd >/dev/null && $SUDO firewall-cmd --state >/dev/null 2>&1; then
    say "opening the Quick Share ports in firewalld"
    $SUDO firewall-cmd -q --reload 2>/dev/null || :
    $SUDO firewall-cmd -q --permanent --add-service=quickshare-direct && $SUDO firewall-cmd -q --reload || warn "couldn't open the ports in firewalld"
  elif command -v ufw >/dev/null && $SUDO ufw status 2>/dev/null | grep -q "Status: active"; then
    say "opening the Quick Share ports in ufw"
    $SUDO ufw allow quickshare-direct >/dev/null || warn "couldn't open the ports in ufw"
  fi
fi

# Linux won't start a Wi-Fi Direct group or hotspot on 5 GHz with the world regulatory domain.
if command -v iw >/dev/null && iw reg get 2>/dev/null | grep -q "country 00"; then
  warn "Wi-Fi regulatory domain is not set (country 00), so direct Wi-Fi links stay on 2.4 GHz. Set yours, e.g. 'sudo iw reg set DE' (persist it with your distribution's wireless-regdb/crda setting)."
fi

# Start the receiver for the user who ran the installer.
RUN_USER=$(id -un)
[ -n "${SUDO_USER:-}" ] && [ "$(id -u)" -eq 0 ] && RUN_USER=$SUDO_USER
if [ -z "${QSD_NO_START:-}" ] && [ "$RUN_USER" != root ]; then
  if [ "$(id -un)" = "$RUN_USER" ]; then
    usercmd() { systemctl --user "$@"; }
  else
    usercmd() { sudo -u "$RUN_USER" XDG_RUNTIME_DIR="/run/user/$(id -u "$RUN_USER")" systemctl --user "$@"; }
  fi
  usercmd daemon-reload 2>/dev/null || :
  if usercmd enable --now quickshare-direct.service 2>/dev/null; then
    say "receiver running for $RUN_USER (it starts with every graphical login)"
  else
    warn "couldn't start the receiver now; it starts at your next graphical login (or: systemctl --user enable --now quickshare-direct)"
  fi
fi

say "done. Receive: pick '$(uname -n)' in Quick Share on the phone. Send: 'quickshare-direct send FILE' or the file manager's \"Send with Quick Share\" (phone on its Quick Share Receive screen)."
