#!/bin/sh
set -eu
PREFIX="${PREFIX:-/opt/asterism}"
install -d "$PREFIX/data/blobs"
install -m 0755 ./target/release/asterism-hub "$PREFIX/asterism-hub"
if [ ! -f "$PREFIX/data/config.toml" ]; then
  "$PREFIX/asterism-hub" init --data-dir "$PREFIX/data"
fi
echo "installed to $PREFIX"
echo "systemctl enable --now asterism-hub  # after copying deploy/systemd/asterism-hub.service"
