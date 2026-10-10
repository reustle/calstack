# Calstack — Omarchy Build Plan

Current implementation includes milestones 0–8: the native strip, local and
remote ICS subscriptions, caching, meeting links, desktop integration, XDG
autostart, Arch packaging, and graphical settings. Embedded VTIMEZONE rules,
Windows timezone names, THISANDFUTURE, RDATE periods, and EXRULE are supported.
See [calendar support](docs/configuration.md) for bounds and exclusions and
[verification](docs/development.md) for completed checks and remaining manual tests.

The sections below preserve the original design direction. Implemented choices
include a blocking HTTP worker thread (not Tokio) and an on-demand Python/GTK
settings window; the strip, calendar engine, and renderer remain Rust.

## 1. Goal

Build a very lightweight vertical calendar strip for Omarchy/Hyprland that sits on the far-right edge of the screen and behaves more like a desktop panel than a normal app window.

The initial Linux build should be designed so the calendar/data/rendering core can later be reused for a native macOS version.

### Core behavior

- ~13 px wide by default
- pinned to the right edge
- top-to-bottom height of the usable display
- borderless: no title bar, window controls, frame, or normal app chrome
- always above normal windows
- reserves desktop space by default, like the top menu bar
- shows:
  - current-time marker
  - compact colored event blocks
  - full-width overlaps with progressively darker grey shading
- hover over an event:
  - show a tooltip outside the strip
  - event name
  - start time
  - duration in minutes
  - calendar name
- click an event:
  - if it contains a recognized Google Meet / Zoom / Microsoft Teams / other meeting URL, open that call in the default browser
  - otherwise show event details
- bottom menu button:
  - Refresh
  - Settings
  - Quit
- follows OS light/dark mode
- reads one or more ICS feeds
- uses local cache so startup is immediate
- refreshes feeds in the background

---

## 2. Initial scope

### V1

Implement only what is required to make the strip useful every day:

1. Wayland/Hyprland panel surface
2. ICS calendar loading
3. normalized events
4. current-day event layout
5. moving current-time marker
6. hover tooltip
7. click-to-join meeting links
8. simple settings/config file
9. background refresh
10. launch-at-login support
11. packaging for Omarchy/Arch

### Explicit non-goals for V1

- editing calendar events
- Google OAuth
- Microsoft Graph integration
- CalDAV writes
- notifications
- task management
- agenda/list views
- week/month UI
- full calendar replacement
- Electron/WebView UI
- macOS implementation itself

---

## 3. Technology choice

Use Rust for the entire application.

Primary reasons:

- low memory footprint
- negligible idle CPU
- fast startup
- easy single-binary deployment
- strong cross-platform core
- no embedded browser runtime
- good fit for Wayland/layer-shell
- straightforward later FFI/native-shell integration on macOS

The Linux-specific window layer should remain thin.

---

## 4. High-level architecture

```text
calstack/
├── Cargo.toml
├── README.md
├── PLAN.md
├── crates/
│   ├── calstack-core/
│   │   ├── calendar/
│   │   ├── ics/
│   │   ├── recurrence/
│   │   ├── meeting_links/
│   │   ├── layout/
│   │   ├── config/
│   │   ├── cache/
│   │   └── theme/
│   │
│   ├── calstack-render/
│   │   ├── strip.rs
│   │   ├── tooltip.rs
│   │   ├── hit_test.rs
│   │   └── primitives.rs
│   │
│   ├── calstack-platform/
│   │   ├── lib.rs
│   │   └── linux/
│   │       ├── layer_shell.rs
│   │       ├── popup.rs
│   │       ├── browser.rs
│   │       ├── appearance.rs
│   │       └── autostart.rs
│   │
│   └── calstack/
│       ├── main.rs
│       ├── state.rs
│       └── event_loop.rs
│
├── assets/
└── packaging/
    ├── arch/
    └── systemd/
```

The key design rule:

> `calstack-core` must contain no Wayland- or Linux-specific logic.

That is what makes the later macOS port clean.

---

## 5. Platform abstraction

Define a small platform interface rather than letting Linux APIs leak throughout the application.

Example conceptual interface:

```rust
trait Platform {
    fn create_strip_window(&self, config: StripWindowConfig) -> Result<WindowId>;
    fn create_tooltip(&self) -> Result<TooltipId>;
    fn screen_geometry(&self) -> ScreenGeometry;
    fn appearance(&self) -> Appearance;
    fn open_url(&self, url: &Url) -> Result<()>;
    fn set_autostart(&self, enabled: bool) -> Result<()>;
}
```

Linux implements this first.

Later:

```text
calstack-platform/
├── linux/
└── macos/
```

The macOS implementation can use AppKit `NSPanel` without changing the calendar or rendering logic.

---

## 6. Linux / Omarchy windowing

Omarchy runs Hyprland on Wayland, so use the Wayland layer-shell protocol rather than a normal desktop window.

Recommended behavior:

```text
layer: top
anchor: top + right + bottom
width: 13 px
exclusive_zone: strip width
keyboard_interactivity: none
```

### Why layer-shell

It gives the app the correct semantics:

- attached to screen edge
- visible above ordinary windows
- not managed like a normal tiled/floating app
- no title bar
- no window controls
- stable positioning
- independent of Hyprland window rules in normal use

### Default behavior

`exclusive_zone = strip width`

Normal tiled windows stop at the strip instead of extending behind it.

The optional setting is:

```toml
reserve_space = true
```

Set `reserve_space = false` to opt into overlay behavior (`exclusive_zone = 0`).

---

## 7. Strip dimensions and screen behavior

Defaults:

```toml
side = "right"
width = 13
day_start = "06:00"
day_end = "24:00"
monitor = "primary"
reserve_space = true
```

Potential future options:

- 8–24 px width
- left/right side
- per-monitor selection
- all monitors
- 24-hour day mode
- configurable top/bottom margins

V1 should support one selected monitor only.

---

## 8. Rendering

Do not use a WebView.

Use a lightweight Rust 2D renderer.

Good candidates:

- `tiny-skia`
- `softbuffer`
- `wgpu` only if a later need justifies it

Start with the smallest practical option.

### Rendered primitives

The strip requires only:

- background rectangle
- event rectangles
- current-time line
- current-time notch
- menu button
- hover state
- optional separators

This should remain extremely cheap to render.

### Refresh rate

The strip does not need continuous animation.

Suggested approach:

- redraw on state changes
- update current-time line once per 15–30 seconds
- redraw on hover enter/leave
- redraw after calendar refresh
- redraw on theme change

Avoid a 60 FPS loop.

---

## 9. Day mapping

Default visible day:

```text
06:00 → top
24:00 → bottom
```

For an event timestamp:

```text
fraction =
    (event_time - day_start)
    / (day_end - day_start)

y = fraction * drawable_height
```

Clamp events crossing the visible boundaries.

Examples:

- event begins 05:30 → draw from top
- event ends 00:30 next day → draw to bottom
- all-day event → special handling

---

## 10. Current-time marker

Draw a thin horizontal marker at the current time.

Recommended:

- 1–2 px line
- small protruding notch extending left of the strip
- visually stronger than event blocks
- follows theme but can retain a muted red accent

Update approximately every 15–30 seconds.

The marker should not require continuous animation.

---

## 11. Event blocks

Events have no text inside the strip.

Each visible event becomes a colored vertical block.

Properties:

```rust
struct EventBlock {
    event_id: EventId,
    rect: Rect,
    color: Color,
    state: EventVisualState,
}
```

Visual states:

- upcoming
- active
- past
- hovered

Suggested defaults:

```text
upcoming: normal muted calendar color
active: slightly stronger opacity
past: reduced opacity
hovered: subtle contrast increase
```

Avoid strong borders.

Use a tiny radius only if it remains visually clean at ~13 px width.

---

## 12. Overlapping events

Updated after the first desktop prototype on September 26, 2026:

- Keep every event full-width; do not split overlaps into side-by-side lanes.
- Split the vertical timeline at start/end boundaries.
- A single event uses its muted calendar color.
- Two simultaneous events use grey; three or more use progressively darker grey.
- Hovering an overlapping interval lists all its events in a separate popup.
- Clicking an event in that popup opens its meeting link or details.

The core layout returns each interval's rectangle and all active event IDs,
independently of the renderer.

---

## 13. Hover tooltip

The tooltip must be a separate popup surface, not constrained to the 13 px strip.

Default content:

```text
Design Review
14:30–15:15
45 minutes
Work
```

Optional later fields:

- location
- organizer
- meeting provider
- attendees

### Hover timing

Suggested behavior:

- show after ~150 ms hover
- dismiss immediately when pointer leaves both event and tooltip
- do not steal keyboard focus

Position tooltip immediately to the left of the strip.

Keep the tooltip entirely on-screen.

---

## 14. Click behavior

Click handling should prioritize joining calls.

### Precedence

When the user clicks an event:

1. find recognized meeting URL
2. if found:
   - open URL in system default browser
3. otherwise:
   - show a richer event-details popup

The event details popup can later include a "Join" button as well.

---

## 15. Meeting-link detection

Search these event fields:

1. structured conference data, if available
2. URL field
3. location
4. description/body

Recognize at minimum:

### Google Meet

```text
meet.google.com/*
```

### Zoom

```text
zoom.us/*
*.zoom.us/*
```

### Microsoft Teams

```text
teams.microsoft.com/*
teams.live.com/*
```

### Other common providers

Potential V1 support:

```text
whereby.com
webex.com
meet.jit.si
gotomeeting.com
chime.aws
```

### Selection rules

If more than one meeting URL exists:

1. prefer recognized provider URLs
2. prefer structured conference/location field over body text
3. prefer HTTPS
4. take the first valid candidate after normalization

Do not automatically open arbitrary links from descriptions unless they match a known conferencing provider in V1.

That reduces accidental clicks.

---

## 16. URL opening

Linux implementation should call the desktop-standard URL opener rather than hard-code a browser.

Conceptually:

```text
xdg-open <url>
```

Prefer a Rust crate or portal API that delegates to the user's configured browser.

No browser-specific logic should exist in `calstack-core`.

---

## 17. ICS input

V1 supports:

- remote HTTP/HTTPS ICS URLs
- `webcal://`
- local `.ics` files

Example config:

```toml
[[calendar.feeds]]
name = "Work"
url = "https://example.com/work.ics"
color = "#7F9BB3"
enabled = true

[[calendar.feeds]]
name = "Personal"
path = "/home/user/calendars/personal.ics"
color = "#91AA8A"
enabled = true
```

Convert `webcal://` internally to HTTPS where appropriate.

---

## 18. ICS parsing requirements

Use an existing Rust iCalendar parser/recurrence implementation where practical.

Must handle:

- `VEVENT`
- `DTSTART`
- `DTEND`
- duration-only events
- `UID`
- `SUMMARY`
- `DESCRIPTION`
- `LOCATION`
- `URL`
- `STATUS`
- `RRULE`
- `RDATE`
- `EXDATE`
- recurrence overrides
- timezone definitions
- UTC events
- floating/local timestamps
- all-day events
- cancellations

Do not implement recurrence rules from scratch unless existing crates prove insufficient.

---

## 19. Normalized event model

All calendar sources should normalize into one internal type.

Example:

```rust
struct CalendarEvent {
    id: EventId,
    calendar_id: CalendarId,
    uid: String,

    title: String,

    start: DateTime,
    end: DateTime,

    all_day: bool,
    cancelled: bool,

    location: Option<String>,
    description: Option<String>,
    url: Option<Url>,

    meeting: Option<MeetingLink>,

    color: Color,
}
```

Once parsed, the renderer should know nothing about ICS.

---

## 20. Calendar refresh and cache

On startup:

```text
1. read config
2. load cached normalized events
3. render immediately
4. refresh feeds in background
5. update cache
6. redraw
```

This prevents startup from depending on network latency.

### Refresh interval

Default:

```toml
refresh_minutes = 10
```

Allow configuration later.

### HTTP behavior

Support:

- ETag
- Last-Modified
- request timeout
- graceful offline behavior
- per-feed failure isolation

One broken feed should not break the app.

---

## 21. Cache strategy

Store application data using XDG paths.

Recommended:

```text
~/.config/calstack/config.toml
~/.cache/calstack/
~/.local/state/calstack/
```

Example:

```text
~/.cache/calstack/
├── calendars/
│   ├── <calendar-id>.ics
│   └── <calendar-id>.json
└── metadata.json
```

Keep raw downloaded ICS plus normalized/cache metadata if useful for debugging.

---

## 22. Configuration

Initial config can be TOML.

Example:

```toml
[display]
side = "right"
width = 13
monitor = "primary"
day_start = "06:00"
day_end = "24:00"
reserve_space = true

[appearance]
past_opacity = 0.35
future_opacity = 0.78
active_opacity = 1.0
show_now_marker = true

[calendar]
refresh_minutes = 10

[[calendar.feeds]]
name = "Work"
url = "https://..."
color = "#7F9BB3"

[[calendar.feeds]]
name = "Personal"
url = "https://..."
color = "#91AA8A"
```

Settings UI can edit this later.

For V1, manual config editing is acceptable until the strip itself works.

---

## 23. Theme handling

Follow the OS/desktop light/dark preference.

Keep the strip visually minimal.

### Dark

```text
background: dark, slightly translucent
events: muted colors
tooltip: near-black
text: soft white
```

### Light

```text
background: soft off-white
events: muted colors
tooltip: near-white
text: dark gray
```

No elaborate shadows or gradients.

The strip should visually recede until needed.

---

## 24. Bottom menu

Reserve the bottom ~16–20 px for an interaction target larger than the visual icon itself.

Visual:

```text
⋮
```

Menu:

```text
Refresh
Settings…
────────
Quit
```

Possible future items:

```text
Hide until tomorrow
Pause calendar
Open calendar
```

The interaction hitbox can be wider internally than the visible icon by allowing the popup/menu to extend left.

---

## 25. Input model

Normal strip behavior:

- pointer hover → identify event by hit-testing
- hover event → show tooltip
- left click event → meeting URL or details
- click bottom button → menu
- no keyboard focus in ordinary use

Do not make the entire surface keyboard-interactive.

Settings can be a separate normal window if necessary.

---

## 26. Hit-testing

Keep event geometry generated by the layout engine.

Example:

```rust
struct HitRegion {
    event_id: EventId,
    rect: Rect,
}
```

Pointer motion only needs a small linear scan because visible events per day are typically few.

If needed later, switch to a spatial index, but that is unnecessary for V1.

---

## 27. State model

Suggested top-level state:

```rust
struct AppState {
    config: Config,
    calendars: Vec<Calendar>,
    events: Vec<CalendarEvent>,
    positioned_events: Vec<PositionedEvent>,
    hovered_event: Option<EventId>,
    appearance: Appearance,
    now: DateTime,
}
```

Events that mutate state:

```text
Startup
CalendarRefreshCompleted
ClockTick
PointerMoved
PointerLeft
MouseClicked
AppearanceChanged
ConfigChanged
MonitorChanged
Shutdown
```

Keep state transitions predictable and testable.

---

## 28. Async model

Use Tokio for:

- calendar downloads
- refresh timers
- cache I/O where appropriate

Do not put the renderer itself inside complex async code.

Recommended split:

```text
Wayland event loop
    ↕ channel
application state
    ↕ channel
async calendar worker
```

The UI should never block on network calls.

---

## 29. Error handling

Errors should not create intrusive dialogs during normal operation.

Examples:

- feed download failed → keep cached version
- malformed event → skip event, log warning
- one feed invalid → continue other feeds
- no network → continue cached state
- meeting URL malformed → event click shows details instead

Expose feed health later in Settings.

---

## 30. Logging

Use structured Rust logging:

- `tracing`
- `tracing-subscriber`

Default normal logs should be quiet.

Useful levels:

```text
ERROR
WARN
INFO
DEBUG
TRACE
```

Run manually with a verbose flag during development.

Potential:

```bash
calstack --log debug
```

---

## 31. Launch at login

Use the standard XDG autostart mechanism, controlled by `startup.autostart`
in configuration or the Settings window. This works with session managers such
as UWSM without editing Hyprland configuration or installing an Omarchy plugin.
Calstack writes a managed desktop entry using the absolute installed executable
and config paths. Autostart is opt-in; duplicate strip launches exit harmlessly.

---

## 32. Process/resource targets

Desired idle characteristics:

```text
CPU: effectively 0%
RAM: tens of MB, not hundreds
GPU: effectively idle
network: refresh only
```

Avoid:

- Chromium
- Electron
- embedded browser engines
- constant animation loops
- unnecessary GPU rendering

Measure actual usage before optimizing further.

---

## 33. Security/privacy

ICS URLs can contain private tokens.

Therefore:

- never print full feed URLs to normal logs
- redact query strings/tokens in error output
- config file permissions should be user-only where practical
- do not send event data anywhere
- no analytics in V1

Potential future improvement:

- store remote calendar secrets in desktop secret storage rather than plaintext config

Not required for first prototype.

---

## 34. All-day events

All-day events do not naturally map into the vertical time strip.

V1 choice:

- exclude them from the timed strip by default

Optional small indicator near the top later:

```text
■■
```

Configuration:

```toml
show_all_day = false
```

This avoids distorting the core visualization.

---

## 35. Multi-day events

For events spanning midnight:

- clip to current visible day's range
- continue showing the relevant portion today
- hover tooltip shows actual event start/end

Do not treat them as all-day unless the ICS explicitly marks them that way.

---

## 36. Timezones

Internally normalize timestamps carefully.

Use:

- event timezone if declared
- calendar timezone where applicable
- system timezone as display timezone

Render according to the user's current local timezone.

Timezone behavior belongs in core and must be platform-independent.

---

## 37. Prototype milestones

### Milestone 0 — repository skeleton

- Cargo workspace
- crate boundaries
- logging
- config loading
- basic test setup

### Milestone 1 — strip surface

- Wayland layer-shell window
- 13 px wide
- right edge
- full height
- no decorations
- exclusive zone equal to the strip width
- simple background
- quit path

Success criterion:

> A stable 13 px vertical bar remains on top while using Omarchy normally.

### Milestone 2 — static visual prototype

Hard-code several fake events.

Implement:

- event blocks
- now line
- overlap shading
- hover hit-testing
- tooltip
- click handling

Success criterion:

> The visual interaction feels correct before calendar parsing is introduced.

### Milestone 3 — ICS core

Implement:

- local ICS parsing
- event normalization
- recurring events
- timezone handling
- today's event query

Success criterion:

> Real local calendar files produce the same layout as the fake event data.

### Milestone 4 — remote feeds

Implement:

- HTTPS download
- caching
- background refresh
- ETag/Last-Modified
- offline startup

### Milestone 5 — meeting links

Implement provider detection and click-to-open:

- Google Meet
- Zoom
- Teams
- selected additional providers

Test URLs from:

- URL property
- location
- description

### Milestone 6 — configuration

Implement:

- multiple feeds
- colors
- width
- visible day range
- refresh interval
- monitor
- past opacity

### Milestone 7 — desktop integration

Implement:

- dark/light appearance
- autostart
- Arch packaging
- install/uninstall instructions

### Milestone 8 — settings UI

Only after the core experience is proven.

Implement:

- feed management
- display options
- startup toggle
- refresh
- quit

---

## 38. Testing strategy

### Unit tests

Core:

- date-to-Y mapping
- clipping
- full-width overlap segmentation and event hit testing
- meeting URL extraction
- meeting provider detection
- recurrence expansion
- timezone conversion
- configuration parsing

### Fixture tests

Create ICS fixtures for:

- normal event
- recurring event
- recurrence exception
- canceled event
- all-day event
- timezone event
- Google Meet
- Zoom
- Teams
- malformed input
- multi-day event

### Integration tests

Test:

```text
ICS → normalization → today's events → layout
```

This path can remain completely platform-independent.

### Manual Linux tests

Verify on Omarchy:

- tiled apps
- floating apps
- multiple workspaces
- full-screen windows
- monitor changes
- suspend/resume
- theme changes
- network loss
- Hyprland restart

---

## 39. Packaging

Initial options:

1. local Cargo install during development
2. simple release binary
3. Arch `PKGBUILD`
4. optional AUR package later

Target executable:

```text
calstack
```

Potential commands:

```bash
calstack
calstack --config ~/.config/calstack/config.toml
calstack --refresh
calstack --log debug
```

Avoid a complicated installer.

---

## 40. macOS future plan

Do not try to share the Linux window implementation.

Share:

- ICS parsing
- recurrence
- normalized event model
- cache
- meeting-link extraction
- configuration model
- day layout
- overlap logic
- visual state
- rendering primitives where practical

Replace only the native platform layer.

Linux:

```text
Wayland layer-shell
```

macOS:

```text
AppKit NSPanel
```

Expected macOS-specific responsibilities:

- borderless floating panel
- screen-edge placement
- all-Spaces behavior
- full-screen auxiliary behavior
- tooltip/popup panel
- appearance detection
- default-browser opening
- login-item management

The core should compile on macOS long before the macOS window implementation exists.

Use `cfg(target_os = "...")` only inside the platform crate wherever possible.

---

## 41. Suggested first implementation sequence

Build in this exact order:

```text
1. Cargo workspace
2. Wayland layer-shell strip
3. fake event rendering
4. now marker
5. hover detection
6. tooltip popup
7. click handling
8. meeting-link detection
9. local ICS parsing
10. recurrence/timezones
11. remote ICS fetch
12. cache
13. multiple calendars/colors
14. theme following
15. autostart
16. packaging
17. settings UI
```

This minimizes risk.

The most important unknown is the exact feel of a 10–13 px interactive strip. Prove that interaction model first before investing heavily in calendar ingestion.

---

## 42. V1 acceptance criteria

The first usable release is complete when:

- launching `calstack` creates a ~13 px strip on the right edge in Omarchy
- it remains above normal applications
- there is no standard window chrome
- it reserves screen space by default, like desktop chrome
- current time is visibly indicated
- today's timed events appear as colored blocks
- overlapping intervals become progressively darker, with every event accessible in the popup
- hover shows name/start/duration/calendar
- clicking Google Meet, Zoom, or Teams events opens the call
- ICS feeds continue to work offline from cache
- remote feeds refresh automatically
- multiple calendars are supported
- appearance follows light/dark mode
- a bottom menu allows refresh/settings/quit
- the app can launch automatically at login
- idle resource usage remains very low
- core calendar/layout logic is not coupled to Linux APIs

---

## 43. Working name

Use **Calstack** for the repository/app unless renamed later.

Suggested binary:

```text
calstack
```

Suggested config directory:

```text
~/.config/calstack/
```

Suggested repository:

```text
~/sync/projects/calstack
```
