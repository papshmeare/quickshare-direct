#!/usr/bin/env bash
# Install test: run install.sh in a clean container of a distribution, with the packages from
# DIST (built by packaging/build-packages.sh), check the result, then uninstall.
#   packaging/ci/test-install.sh debian:12 dist/
set -euo pipefail
IMAGE=$1
DIST=$(cd "$2" && pwd)
REPO=$(cd "$(dirname "$0")/../.." && pwd)

# What the image needs before install.sh can run (as root, so no sudo).
case $IMAGE in
  archlinux*) PREP='pacman -Sy --noconfirm >/dev/null' ;;
  # No package for openSUSE: the generic tarball, with the dependencies installed by hand.
  opensuse*) PREP='zypper -nq in bluez iw iproute2 wpa_supplicant dnsmasq polkit systemd libdbus-1-3 >/dev/null' ;;
  *) PREP=':' ;;
esac

docker run --rm -e DEBIAN_FRONTEND=noninteractive -v "$DIST":/dist:ro -v "$REPO/install.sh":/install.sh:ro "$IMAGE" sh -euc "
  $PREP
  QSD_PACKAGE_DIR=/dist QSD_NO_START=1 sh /install.sh
  echo '--- checks'
  quickshare-direct --help >/dev/null
  b=\$(readlink -f \$(command -v quickshare-direct)); echo \"binary: \$b\"
  case \$b in /usr/local/*) L=/usr/local/lib/quickshare-direct U=/etc/systemd ;; *) L=/usr/lib/quickshare-direct U=/usr/lib/systemd ;; esac
  for f in \$L/quickshare-ap \$L/quickshare-join; do bash -n \"\$f\" && test -x \"\$f\"; done
  for f in \$U/system/quickshare-ap@.service \$U/system/quickshare-join@.service \
           \$U/system/quickshare-bt-connectable.service \$U/user/quickshare-direct.service; do
    test -f \"\$f\" || { echo \"missing \$f\"; exit 1; }
  done
  grep -q \"ExecStart=\$L/quickshare-ap\" \$U/system/quickshare-ap@.service
  grep -q \"ExecStart=\$b\" \$U/user/quickshare-direct.service
  ls /usr/share/polkit-1/rules.d/50-quickshare-direct.rules /etc/polkit-1/rules.d/50-quickshare-direct.rules 2>/dev/null | grep -q .
  for t in iw ip busctl btmgmt dnsmasq; do command -v \$t >/dev/null || { echo \"dependency \$t missing\"; exit 1; }; done
  if command -v systemd-analyze >/dev/null; then
    systemd-analyze verify \$U/system/quickshare-bt-connectable.service
  fi
  echo '--- uninstall'
  sh /install.sh --uninstall
  if [ -e \$b ]; then echo \"\$b still installed\"; exit 1; fi
  echo 'OK $IMAGE'
"
