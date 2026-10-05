#!/usr/bin/env bash
# Build dependencies for packaging/build-packages.sh on Ubuntu 22.04 (CI runner or container).
set -euo pipefail
SUDO=; [ "$(id -u)" -eq 0 ] || SUDO=sudo
export DEBIAN_FRONTEND=noninteractive
$SUDO apt-get update -q
$SUDO apt-get install -yq --no-install-recommends build-essential pkg-config libdbus-1-dev \
  protobuf-compiler ruby ruby-dev rpm libarchive-tools zstd curl ca-certificates git
$SUDO gem install --no-document fpm
if ! command -v cargo >/dev/null; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
