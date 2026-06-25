#!/bin/sh
# Build the locally-patched hypr-dim (was wl-gammarelay-rs) and install it.
# D-Bus name is now dev.hyprdim; package/binary is hypr-dim. Issue #66.
set -eu
CARGO_HOME="$HOME/.cargo" cargo install --path /home/me/src/gamma-fix --root "$HOME/.local" --force
echo "installed: $("$HOME/.local/bin/hypr-dim" --version)"
