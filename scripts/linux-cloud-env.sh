#!/bin/bash
# Linux (cloud container) build environment for Sieve. The app targets macOS; this lets
# `cargo test` / `cargo clippy` / `pnpm build` / Playwright run on Ubuntu 24.04.
#   source scripts/linux-cloud-env.sh          # env only
#   bash scripts/linux-cloud-env.sh --install  # one-time: apt deps, TurboJPEG 3, ONNX Runtime
# Distro libjpeg-turbo is 2.1 (no tj3 API) -> build 3.1 into /opt/ljt3. The ort crate's prebuilt
# download (cdn.pyke.io) may be blocked -> ONNX Runtime 1.28 from GitHub releases, linked dynamically.
# Tests run as root here, so the two permission tests in xmp::tests fail (permissions are not enforced).
if [ "$1" = "--install" ]; then
  set -e
  apt-get update -q && apt-get install -y -q libraw-dev libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev libimage-exiftool-perl cmake
  apt-get remove -y -q libturbojpeg0-dev || true  # its libturbojpeg.so (2.1) would shadow /opt/ljt3
  if [ ! -f /opt/ljt3/lib/libturbojpeg.so ]; then
    t=$(mktemp -d); curl -sSL https://github.com/libjpeg-turbo/libjpeg-turbo/releases/download/3.1.2/libjpeg-turbo-3.1.2.tar.gz | tar xz -C "$t"
    cmake -S "$t"/libjpeg-turbo-3.1.2 -B "$t"/b -DCMAKE_INSTALL_PREFIX=/opt/ljt3 -DCMAKE_INSTALL_LIBDIR=lib -DWITH_SIMD=0 -DCMAKE_BUILD_TYPE=Release >/dev/null
    cmake --build "$t"/b -j"$(nproc)" >/dev/null && cmake --install "$t"/b >/dev/null; rm -rf "$t"
  fi
  if [ ! -f /opt/ort/lib/libonnxruntime.so ]; then
    t=$(mktemp -d); curl -sSL https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-x64-1.28.0.tgz | tar xz -C "$t"
    mkdir -p /opt/ort && cp -r "$t"/onnxruntime-linux-x64-1.28.0/* /opt/ort/; rm -rf "$t"
  fi
fi
export TURBOJPEG_DIR=/opt/ljt3
export ORT_LIB_PATH=/opt/ort/lib
export ORT_PREFER_DYNAMIC_LINK=1
export LD_LIBRARY_PATH=/opt/ljt3/lib:/opt/ort/lib:${LD_LIBRARY_PATH}
export PATH=$HOME/.cargo/bin:$PATH
# Disk: the per-session allowance is small; several worktrees share one CARGO_TARGET_DIR.
export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/home/user/Sieve/src-tauri/target}
# Playwright: the image ships Chromium 1194, @playwright/test wants a newer build.
export PW_CHROMIUM=/opt/pw-browsers/chromium_headless_shell-1194/chrome-linux/headless_shell
