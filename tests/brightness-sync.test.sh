#!/bin/sh
# Behavioral tests for brightness-sync daemon-snapshot wiring (issue #66).
# Fakes busctl/brightnessctl/hyprctl on PATH and a fake DRM tree; asserts the
# verbs drive the daemon's Snapshot/Restore and that the external reassert is
# gated on the display reporting DPMS on.
set -eu

BS="${BS_BIN:-$HOME/.local/bin/brightness-sync}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

BIN="$TMP/bin"; mkdir -p "$BIN"
LOG="$TMP/calls.log"; : > "$LOG"
cat > "$BIN/busctl" <<EOF
#!/bin/sh
echo "busctl \$*" >> "$LOG"
exit 0
EOF
cat > "$BIN/brightnessctl" <<EOF
#!/bin/sh
echo "brightnessctl \$*" >> "$LOG"
exit 0
EOF
cat > "$BIN/hyprctl" <<EOF
#!/bin/sh
# fake: 'monitors' reports DP-3 with dpmsStatus from \$FAKE_DPMS (default 1=on)
if [ "\$1" = monitors ]; then
  printf 'Monitor DP-3 (ID 1):\n\tdpmsStatus: %s\n' "\${FAKE_DPMS:-1}"
fi
exit 0
EOF
chmod +x "$BIN/busctl" "$BIN/brightnessctl" "$BIN/hyprctl"
PATH="$BIN:$PATH"; export PATH

# Fake DRM: one connected external (DP-3), no internal backlight.
DRM="$TMP/drm"; mkdir -p "$DRM/card1-DP-3"; echo connected > "$DRM/card1-DP-3/status"
BL="$TMP/backlight"; mkdir -p "$BL"
export BS_DRM="$DRM" BS_BL="$BL" BS_LEVEL="$TMP/level" BS_DIMSAVE="$TMP/dimsave"

fail() { echo "FAIL: $1"; exit 1; }

# 1. dim must snapshot via the daemon (once, before dimming externals).
rm -f "$BS_DIMSAVE"
"$BS" dim
grep -q "busctl --user call dev.hyprdim / dev.hyprdim Snapshot" "$LOG" \
    || fail "dim did not call daemon Snapshot"

# 2. restore while DP-3 is OFF must NOT call daemon Restore — the display can't
#    listen yet, so the snapshot is retained for a later (post power-on) restore.
: > "$LOG"; FAKE_DPMS=0 "$BS" restore
grep -q "busctl --user call dev.hyprdim / dev.hyprdim Restore" "$LOG" \
    && fail "restore (DPMS off) wrongly called daemon Restore before the display could listen"

# 3. restore while DP-3 is ON must call daemon Restore, and must NOT set external
#    gamma directly (the daemon owns per-display external restore).
: > "$LOG"; FAKE_DPMS=1 "$BS" restore
grep -q "busctl --user call dev.hyprdim / dev.hyprdim Restore" "$LOG" \
    || fail "restore (DPMS on) did not call daemon Restore"
grep -q "set-property dev.hyprdim /outputs/DP_3" "$LOG" \
    && fail "restore set external gamma directly instead of delegating to daemon Restore"

# 4. restore with NO prior dim but DP-3 on must still call daemon Restore
#    (idempotent; the daemon owns external snapshots).
: > "$LOG"; rm -f "$BS_DIMSAVE"; FAKE_DPMS=1 "$BS" restore
grep -q "busctl --user call dev.hyprdim / dev.hyprdim Restore" "$LOG" \
    || fail "restore without prior dim did not call daemon Restore"

echo "PASS: brightness-sync daemon snapshot/restore wiring"
