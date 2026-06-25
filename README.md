# hypr-dim

A small Wayland daemon exposing a D-Bus interface (`dev.hyprdim`) for per-display
**brightness**, **gamma**, **colour temperature** and **inversion**, applied
through `wlr-gamma-control-unstable-v1`. Single-threaded, no flicker, zero runtime
dependencies.

It is the gamma backend for **`brightness-sync`**: displays with no hardware
backlight (most external monitors over DisplayPort/HDMI without a DDC/CI backlight
node) are dimmed by adjusting their gamma ramp through this daemon, while real
backlights are driven by `brightnessctl`. See issue #66 of the
[Debian13-Hyprland](https://github.com/tkirkland/Debian13-Hyprland) installer.

## Provenance

hypr-dim is a derivative of
[`wl-gammarelay-rs`](https://github.com/MaxVerevkin/wl-gammarelay-rs) by Max
Verevkin, licensed **GPL-3.0-only** (see [`LICENCE`](LICENCE)). The upstream
copyright and licence are retained. Changes in this fork:

- D-Bus name renamed `rs.wl.gammarelay` → **`dev.hyprdim`**; binary renamed to
  `hypr-dim`.
- Added **`Snapshot` / `Restore`** root methods that save and re-apply every
  output's current values — used to implement an idle-dim that returns each
  display to its exact pre-dim level.

Everything else (the per-output `/outputs/<name>` objects, the temperature/gamma/
inversion controls, the `watch` subcommand) is upstream behaviour, retained.

## D-Bus interface

Service `dev.hyprdim`, interface `dev.hyprdim`.

### Root object `/`

| Member | Kind | Notes |
|---|---|---|
| `Brightness` | property `d` | brightness 0.0–1.0, writable, emits-change (average across outputs) |
| `Temperature` | property `q` | colour temperature (K) |
| `Gamma` | property `d` | gamma |
| `Inverted` | property `b` | colour inversion |
| `UpdateBrightness(d)` | method | relative adjust, applied to every output |
| `UpdateTemperature(n)` / `UpdateGamma(d)` / `ToggleInverted()` | method | relative / toggle |
| `Snapshot()` | method | save every output's current values *(hypr-dim)* |
| `Restore()` | method | re-apply the snapshotted values, per output *(hypr-dim)* |

### Per-output objects `/outputs/<name>`

`<name>` is the Wayland output name with `-` replaced by `_` (e.g. `DP-3` → `DP_3`).
Each carries the same interface as root, scoped to that one display.

```sh
$ busctl --user tree dev.hyprdim
└─ /outputs
  ├─ /outputs/DP_3
  └─ /outputs/eDP_1

# set DP-3 to 60% brightness
busctl --user set-property dev.hyprdim /outputs/DP_3 dev.hyprdim Brightness d 0.6

# save all displays, then restore them (idle dim / restore)
busctl --user call dev.hyprdim / dev.hyprdim Snapshot
busctl --user call dev.hyprdim / dev.hyprdim Restore
```

### Watch for changes

```sh
hypr-dim watch "{t}K {bp}%"   # {t}=temperature, {b}=brightness 0–1, {bp}=brightness %
```

## Build & install

Requires a Rust toolchain new enough for edition 2024 (cargo ≥ 1.85). The lockfile
pins git dependencies, so build with `--locked`.

```sh
cargo install --path . --root ~/.local --locked
# or the helper, which also prints the installed version:
./build-install.sh
```

Run it as a Wayland-session user service:

```ini
[Unit]
PartOf=graphical-session.target
After=graphical-session.target
ConditionEnvironment=WAYLAND_DISPLAY

[Service]
ExecStart=%h/.local/bin/hypr-dim
Restart=always
RestartSec=1

[Install]
WantedBy=graphical-session.target
```

## Licence

GPL-3.0-only. Original work © Max Verevkin and contributors; fork modifications
© Tony Kirkland. The full text is in [`LICENCE`](LICENCE).
