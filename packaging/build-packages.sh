#!/usr/bin/env bash
# Build quickshare-direct and package it for Debian/Ubuntu (.deb), Fedora (.rpm), Arch (.pkg.tar.zst)
# and other distributions (.tar.gz), into dist/. Used by CI (.github/workflows/packages.yml) on
# Ubuntu 22.04, whose glibc (2.35) keeps the binary runnable on Debian 12, Ubuntu 22.04+,
# Fedora 36+ and Arch.
#
# Needs: cargo (>= 1.85), protoc, libdbus-1-dev, pkg-config, make, fpm (gem), rpm (rpmbuild),
# libarchive-tools (bsdtar) and zstd (pacman packages).
# Asset names carry no version, so https://github.com/<repo>/releases/latest/download/<name> works.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(tr -d ' \n' < VERSION)
ARCH=$(uname -m)
case $ARCH in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64) DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 1 ;;
esac
STAGE=$PWD/build/stage
DIST=$PWD/dist
rm -rf "${STAGE:?}"; mkdir -p "$STAGE" "$DIST"

make build
make install DESTDIR="$STAGE" PREFIX=/usr

URL=https://github.com/papshmeare/quickshare-direct
DESC="Quick Share (Nearby Share) for Linux without a shared Wi-Fi: Bluetooth first contact, then Wi-Fi Direct"
common=(
  -s dir -C "$STAGE" -n quickshare-direct -v "$VERSION" --iteration 1
  --license GPL-3.0-only --url "$URL" --description "$DESC"
  --maintainer "papshmeare <85828434+papshmeare@users.noreply.github.com>"
  --after-install packaging/scripts/postinstall.sh
  --before-remove packaging/scripts/preremove.sh
  --after-remove packaging/scripts/postremove.sh
  --config-files /etc/default/quickshare-direct
  --config-files /etc/ufw/applications.d/quickshare-direct
  -f
)

# Debian 12+, Ubuntu 24.04+ (22.04 works, but its polkit can't let users start the Wi-Fi helpers).
fpm "${common[@]}" -t deb -a "$DEB_ARCH" -p "$DIST/quickshare-direct_$DEB_ARCH.deb" \
  --deb-no-default-config-files \
  -d 'libc6 (>= 2.35)' -d libdbus-1-3 -d libsystemd0 -d systemd -d bluez -d iw -d iproute2 \
  -d wpasupplicant -d 'dnsmasq-base | dnsmasq' -d 'polkitd | policykit-1' \
  --deb-recommends network-manager --deb-recommends libnotify-bin --deb-recommends xdg-utils \
  --deb-recommends busybox --deb-recommends hostapd --deb-recommends 'wl-clipboard | xclip' \
  .

# Fedora.
fpm "${common[@]}" -t rpm -a "$ARCH" -p "$DIST/quickshare-direct.$ARCH.rpm" \
  -d 'glibc >= 2.35' -d dbus-libs -d systemd-libs -d systemd -d bluez -d iw -d iproute \
  -d wpa_supplicant -d dnsmasq -d polkit \
  --rpm-tag 'Recommends: NetworkManager' --rpm-tag 'Recommends: libnotify' \
  --rpm-tag 'Recommends: xdg-utils' --rpm-tag 'Recommends: busybox' --rpm-tag 'Recommends: hostapd' \
  --rpm-tag 'Recommends: wl-clipboard' \
  .

# Arch Linux and derivatives.
fpm "${common[@]}" -t pacman -a "$ARCH" -p "$DIST/quickshare-direct-$ARCH.pkg.tar.zst" \
  --pacman-compression zstd \
  -d glibc -d dbus -d systemd-libs -d systemd -d bluez -d bluez-utils -d iw -d iproute2 \
  -d wpa_supplicant -d dnsmasq -d polkit \
  --pacman-optional-depends 'networkmanager: Wi-Fi management (recommended)' \
  --pacman-optional-depends 'libnotify: notifications' \
  --pacman-optional-depends 'xdg-utils: open received files and links' \
  --pacman-optional-depends 'busybox: DHCP when joining a phone for sending' \
  --pacman-optional-depends 'hostapd: hotspot fallback for receiving' \
  --pacman-optional-depends 'wl-clipboard: copy received text (Wayland)' \
  .

# Everything else: the staged tree plus the package scripts (install.sh uses it).
rm -rf build/tarball && mkdir -p build/tarball
cp -a "$STAGE"/. build/tarball/
install -Dm755 packaging/scripts/postinstall.sh build/tarball/.quickshare-direct/postinstall.sh
install -Dm755 packaging/scripts/preremove.sh build/tarball/.quickshare-direct/preremove.sh
echo "$VERSION" > build/tarball/.quickshare-direct/VERSION
tar -C build/tarball -czf "$DIST/quickshare-direct-$ARCH.tar.gz" .

ls -la "$DIST"
