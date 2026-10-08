# Configuration and calendars

Use `calstack --settings`, or edit `~/.config/calstack/config.toml`.
`XDG_CONFIG_HOME` and `--config /path/to/config.toml` are supported. Changes reload
automatically within two seconds; invalid edits retain the last valid settings.

```toml
[display]
width = 13
monitor = "primary"
day_start = "06:00"
day_end = "24:00"
reserve_space = true

[appearance]
theme = "auto"
past_opacity = 0.35
future_opacity = 0.78
active_opacity = 1.0
show_now_marker = true

[startup]
autostart = false

[calendar]
refresh_minutes = 10

[[calendar.feeds]]
name = "Work"
url = "https://example.com/private/calendar.ics"
color = "#7F9BB3"
enabled = true

[[calendar.feeds]]
name = "Local"
path = "/home/you/calendars/personal.ics"
enabled = false
```

Each feed needs a unique name and exactly one `url` or `path`. Color is optional;
enabled defaults to true. Relative paths resolve against the config directory.
Use a provider's ICS subscription URL, not its calendar webpage. `webcal://`
uses HTTPS. These subscriptions are read-only; CalDAV login, OAuth and event
editing are outside the scope of this app.

`primary` means the current available output, initially the first Wayland output.
You can use a connector name such as `eDP-1`. If that output disappears, Calstack
uses an available output and returns when the selected one reconnects. Monitor
selection can change without restarting. With no outputs, it waits for one.
The top layer stays above normal windows; fullscreen behavior follows the
compositor's top-layer policy rather than forcibly drawing over fullscreen apps.
Run `calstack --list-displays` to see each display's name (what `monitor` matches
against), size, and position — on either OS.

Omarchy themes supply colors and typography, including user overrides. Other
Linux desktops use fontconfig/GTK preferences and a built-in light/dark palette.
The tiny hour labels remain six logical pixels. Font rendering follows output
scale, including compositor downscaling at fractional scales.

## macOS

The strip pins to the right edge of the configured display, below the menu bar,
and always floats above other windows — there's no Wayland-style exclusive zone
on macOS, so `reserve_space` is accepted but ignored; other windows can still
extend behind the strip. Dark/light for `theme = "auto"` follows System
Settings' appearance; there's no Omarchy-style live theme file, so custom
accent colors aren't picked up. Typography uses a bundled font (Inter) rather
than the system font, and doesn't track live font-size changes — Core Text
integration is a possible future improvement, not currently implemented.
`calstack --settings` opens a native window (built with egui, not GTK) with the
same calendar/display/startup fields as Linux's settings, plus a monitor picker
populated from the real display list instead of a free-text field. "Start at
login" uses a `launchd` user agent instead of a freedesktop autostart entry;
the calendar cache lives under `~/Library/Caches/calstack/calendars` instead of
`$XDG_CACHE_HOME`.

## Supported ICS

- Folded/escaped text, UTC, floating local timestamps, IANA and Windows TZIDs
- Calendar-supplied `VTIMEZONE` standard/daylight observances
- `DTEND` and `DURATION`, including nominal days across DST changes
- `RRULE`, `RDATE` (including periods), `EXDATE`, and legacy `EXRULE`
- Individual and `RANGE=THISANDFUTURE` overrides; cancellations and revisions
- Multi-day timed events; all-day events are intentionally hidden
- Meeting links from URL, then location, then description: Google Meet, Zoom,
  Teams, Webex, Whereby, Jitsi Meet, Amazon Chime, and GoTo Meeting

Recurrence uses the rrule library; timezone mapping uses chrono-tz and the CLDR
Windows timezone list. Expansion, downloads, and durations have safety bounds.
Very dense/old recurrence sets can exceed those bounds; durations and series
shifts over 366 days are rejected. This is not a claim of complete RFC coverage:
non-Gregorian recurrence scales and scheduling/invitation workflows are outside
scope. Unrecognized data keeps the feed's last good cache and produces a warning.

## Refresh and offline operation

```sh
calstack --refresh              # Ask the running app to reload and refresh
calstack --check                # Fetch/validate and print today's events
calstack --demo                 # Fictional built-in calendar
```

Downloads happen on a worker thread, every 10 minutes by default (configurable
from 1–1440). Requests use timeouts, a 10 MiB cap, ETag/Last-Modified revalidation,
and bounded redirects. A failing feed does not clear other calendars.

Last-good raw ICS data is cached under `$XDG_CACHE_HOME/calstack/calendars`
(normally `~/.cache/calstack/calendars`) with private permissions and hashed
filenames. It is available on offline startup and recalculated for the current
day. Feed-error logs omit subscription URLs and calendar content. Day rollover
and detected clock/suspend changes trigger recalculation and refresh.

The [demo ICS fixture](../assets/demo.ics) can be loaded with a local feed path.
`--demo` uses the equivalent hardcoded schedule instead.
