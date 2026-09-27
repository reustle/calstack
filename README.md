# Calstack

![Calstack calendar strip on the Omarchy desktop](assets/screenshots/fullscreen.png)

![Single-event popup with a Google Meet action](assets/screenshots/single-event.png)

A small native calendar strip for Omarchy/Hyprland. Load local ICS files or
remote HTTP/HTTPS/webcal subscriptions, view today's events, and open meeting
links from their cards. Downloads run in the background, with last-good disk
caching for offline use. Use `--demo` for the fictional calendar in the screenshots.

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
  reloads the config and refreshes calendars. Invalid edits keep the previous config
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

`--check` validates config, fetches the configured calendars (falling back to
cache), and prints today's events without a Wayland connection. Add `--demo`
to check the fictional schedule instead. A missing config is initialized with
defaults. Without `--demo`, an empty feed list shows an empty strip.

## Calendar subscriptions

Add named feeds to `~/.config/calstack/config.toml`. Use the provider's ICS
subscription URL, rather than its calendar webpage. These are read-only
subscriptions; CalDAV accounts, OAuth, and calendar editing are not supported.

```toml
[calendar]
refresh_minutes = 10

[[calendar.feeds]]
name = "Work"
url = "https://example.com/private/calendar.ics"
color = "#7F9BB3"
enabled = true

[[calendar.feeds]]
name = "Personal"
url = "webcal://example.com/personal.ics"
color = "#91AA8A"

[[calendar.feeds]]
name = "Local"
path = "/home/you/calendars/personal.ics"
enabled = false
```

Each entry needs a unique name and exactly one `url` or `path`. Colors are
optional; enabled defaults to true. Relative paths resolve against the config
file's directory. Merge with any existing `[calendar]` section rather than
adding it twice. Choose **Refresh** after editing. Launch without `--demo` to
use your feeds:

```sh
./target/release/calstack --check
./target/release/calstack
```

`webcal://` is fetched over HTTPS. Requests have a 20-second timeout, a 10 MiB
size limit, and support ETag/Last-Modified revalidation. Cached ICS data lives
under `$XDG_CACHE_HOME/calstack/calendars` (normally `~/.cache/calstack/calendars`),
with private file permissions and hashed filenames. Feed URLs and event content
are omitted from feed-error logs. Failed downloads or unsupported responses
keep the last good data for that calendar; other feeds can still update.
Refresh intervals range from 1 to 1440 minutes, and the day is recalculated at
local midnight, including after resume.

Supported ICS features include folded/escaped text, UTC and floating local times,
IANA `TZID` timezones, `DTEND` or `DURATION`, `RRULE`, `RDATE`, `EXDATE`, individual
`RECURRENCE-ID` overrides, cancellations, and multi-day timed events. Meeting
links are extracted from URL, location, then description. All-day entries are
intentionally hidden from the timed strip.

This is not full RFC 5545 coverage: custom `VTIMEZONE` definitions, Windows
TZID names, `RANGE=THISANDFUTURE`, `RDATE` periods, and `EXRULE` are unsupported.
IANA timezone names use the bundled timezone database. A feed requiring an
unsupported feature retains its last good cache and logs a warning. Recurrence
expansion is bounded; very dense/long-running rules may exceed the safety limit.
Event durations longer than 366 days are rejected.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --release
```

- `calstack-core`: config, ICS parsing/recurrence, feed downloads/cache, demo
  events, clipping, overlap segments, hit testing, and meeting-provider validation.
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

[assets/demo.ics](assets/demo.ics) is a local ICS fixture you can load using a
feed `path`. `--demo` still uses its built-in static schedule and does not read
that file. Disconnecting the selected monitor closes the app. Fullscreen,
suspend/resume, and multi-monitor hotplug need broader desktop verification.
Autostart installation, packaging, and a graphical settings editor remain on
the [roadmap](PLAN.md).
