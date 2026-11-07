#!/usr/bin/env bash
# System packages for Ratewise development (run as root inside WSL).
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q --no-install-recommends \
  build-essential pkg-config git curl ca-certificates xz-utils unzip \
  libfontconfig1-dev libfreetype-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-dev libx11-xcb-dev libxcursor-dev libxi-dev libxrandr-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libgl1-mesa-dev libegl1-mesa-dev libgbm-dev libdrm-dev \
  xvfb xauth weston mesa-utils \
  patchelf file desktop-file-utils appstream squashfs-tools zsync \
  pulseaudio-utils
echo "ROOT SETUP OK"
