# claudeV

Native X11 waveform viewer for VCD files.

![VCD Viewer](docs/screenshot.png)

## Features

- Load VCD files from CLI or in-app file browser.
- Browse module scopes and signal lists.
- Pin/reorder wave rows and expand buses into bit slices.
- Zoom, pan, cursor placement, and A/B marker measurements.
- Signal filtering and `(all scopes)` global selection mode.
- Full-path or short-name signal labels.
- Multi-select signals and waveforms with keyboard or mouse.
- Resizable panels, selectable waveform/font sizes, and four color schemes.

## Requirements

- Linux/Unix desktop with X11 (or Wayland with XWayland).
- Rust toolchain (`cargo`, `rustc`).

## Build

```bash
cargo build
```

Release build:

```bash
cargo build --release
```

## Run

Run with no file:

```bash
cargo run
```

Run with a file:

```bash
cargo run -- path/to/file.vcd
```

Specify X11 display:

```bash
cargo run -- -d :0 path/to/file.vcd
```

Binary usage:

```text
claudeV [-d DISPLAY] [file.vcd]
```

## Quick Controls

- `Tab`: cycle focus `Browser -> Signals -> Wave`
- `a` / `Enter`: add/remove selected signal (Signals focus)
- `Space`, `Ctrl-click`, `Shift-click`: multi-select signals or waveforms
- `J` / `K`: reorder selected wave signal (Wave focus)
- `e`: expand/collapse bus
- `p`: toggle full-path vs signal-name labels
- `z`: cycle size `default -> big -> large -> huge`
- `t`: cycle color scheme
- `+` / `-`: zoom in/out
- `h` / `l`: pan left/right
- `c`: set cursor
- `m`, `M`, `D`: marker set/switch/clear
- `/`, `X`: edit/clear signal filter
- `q` or `Esc`: quit

## Menus

Top `File` menu actions:

- `Open`: browse directories and `.vcd` files.
- `Reload`: reload current file.
- `Exit`: quit application.

Top `View` menu actions:

- `Refresh`: redraw the window.

## Documentation

For full keyboard/mouse controls, workflow, and troubleshooting, see [USERGUIDE.txt](./docs/USERGUIDE.txt).
