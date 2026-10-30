#!/usr/bin/env bash
# Installs the system packages that a Linux build of the desktop app needs.
set -euo pipefail
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  clang cmake pkg-config libasound2-dev libfontconfig-dev libwayland-dev \
  libx11-xcb-dev libxkbcommon-x11-dev libxkbcommon-dev libxcb1-dev libssl-dev \
  libzstd-dev libvulkan-dev libdrm-dev libgbm-dev
