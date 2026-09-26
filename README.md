# Calstack

![Calstack calendar strip on the Omarchy desktop](assets/screenshots/fullscreen.png)

![Single-event popup with a Google Meet action](assets/screenshots/single-event.png)

A small native calendar strip for Omarchy/Hyprland. This first prototype
implements milestones 0–2 from [PLAN.md](PLAN.md), plus meeting-link validation
and basic configuration. It uses one **built-in fictional calendar**, repeating
at local wall-clock times every day. ICS ingestion, remote feeds, caching,
autostart installation, and a graphical settings editor are not implemented yet.

## Run

Requires Rust, a Wayland desktop with layer-shell, a C compiler, Wayland and
xkbcommon development libraries, fontconfig, and xdg-utils. On this machine Rust
is installed in `~/.cargo/bin`; source `~/.cargo/env` if Cargo is not on PATH.

```sh
source ~/.cargo/env
cargo run --release -- --demo
```

Or launch the already-built binary:

```sh
./target/release/calstack --demo
```

Only start one instance. The initial desktop launch is a transient user service
named `calstack-demo`; stop it before starting another instance:

```sh
systemctl --user stop calstack-demo
```

This service is not enabled at login. Its logs are available with:

```sh
journalctl --user -u calstack-demo
```

## Interactions

- The 12 logical-pixel strip sits on the right edge, below the desktop bar. It
  uses layer-shell's top layer and reserves desktop space, like the top bar, so
  tiled windows stop before its left edge.
- Events occupy 06:00–24:00. Events remain full-width; two-event overlaps are grey and three-event
  overlaps are darker shades of the theme’s muted color. Past events are muted.
  The red marker updates every 30 seconds. Small, right-aligned numbers label
  every hour (6, 7, 8, …), without tick marks.
- Hover over a colored block for its title, start/end, duration, and calendar.
  Move left into the tooltip to keep it visible. Tooltips wait 150 ms before
  opening and allow 180 ms for crossing between surfaces before dismissing.
- Tooltips show each event in a separate card with its calendar, title, time,
  duration, and action. Overlapping events have visible card boundaries and
  gutters; hover highlights the entire card. Click a card to open that event.
- Click an event to open its recognized meeting link, or to show details.
  **Demo meeting URLs are placeholders, not working invitations.** Clicking
  them can open the provider's website in your browser.
- Click the bottom `⋮` for **Refresh**, **Settings**, or **Quit**. Settings opens
  the TOML config in Omarchy's selected editor (including a terminal window for
  terminal editors), or the default associated application on other desktops; Refresh
  reloads the config and demo schedule. Invalid edits keep the previous config
  and log a warning.

Config is created on first launch at `$XDG_CONFIG_HOME/calstack/config.toml`
(or `~/.config/calstack/config.toml`). Width, day range, opacity, light/dark
appearance, and reserved space can be changed there. A monitor connector name
such as `eDP-1` can be selected; changing monitors requires restarting.
`primary` currently selects the first Wayland output, since Wayland has no
standard primary-output designation. Theme `auto` reads Omarchy's current `colors.toml` and `shell.toml`, plus
`~/.config/omarchy/shell.toml` overrides. Background and text match the bar's
colors; default events use the theme's `muted` color. Cards use the shell's
normal control fill, and the time marker uses its active color. Overlaps blend
the event color toward the theme background. Theme files are checked every two
seconds; invalid/incomplete updates retain the last good palette. Outside
Omarchy, startup falls back to the desktop's light/dark preference.

```sh
./target/release/calstack --config ./my-config.toml --demo
./target/release/calstack --check
./target/release/calstack --log calstack_platform=debug
```

`--check` validates config and prints the demo schedule without a Wayland
connection. A missing config is initialized with defaults. `--demo` is explicit
for clarity; this prototype also defaults to the demo calendar without it.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --release
```

- `calstack-core`: platform-independent config, demo events, clipping, overlap
  segments, hit testing, and meeting-provider validation.
- `calstack-render`: software rasterization and system-font text rendering.
- `calstack-platform`: Wayland shared-memory surfaces, pointer input, desktop
  appearance, URL opening, and the event loop.
- `calstack-app`: CLI and startup.

Rendering runs only for input, surface/config/theme changes, and the clock tick; no
continuous animation or GPU render loop is used. Fonts render at the output's
integer buffer scale, with compositor downscaling on fractional-scale displays.

Typography follows Omarchy's `monospace` fontconfig alias and the theme/user
`[font]` tokens in `shell.toml`: titles use `title`, event times use `subtitle`,
calendar labels use `caption`, actions/durations use `body-small`, and menus
use `body`. The base-size ratios and token overrides match Omarchy's shell.
Popups and their click targets grow with those sizes. Changes to these settings
and Omarchy's fontconfig alias reload within two seconds. On other desktops,
GTK 4/3 `gtk-font-name` or the desktop's `font-name`/`text-scaling-factor` supply
the fallback font and size (point sizes are converted to logical pixels).
The strip's tiny hour numbers deliberately stay at **6 logical pixels**.

[assets/demo.ics](assets/demo.ics) is the companion fixture for future ICS
loading. The prototype uses the equivalent static timed schedule in the core;
edits to that ICS file do not change the running demo. All-day events are not
rendered. Disconnecting the selected monitor closes the prototype. Fullscreen,
suspend/resume, multi-monitor hotplug, and arbitrary ICS timezone/recurrence
behavior still need the later milestones and verification.
