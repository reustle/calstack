# Calstack

![Calstack on the Omarchy desktop](assets/screenshots/fullscreen.png)

![Event popup with a Google Meet link](assets/screenshots/single-event.png)

A small native calendar strip for Wayland. See today's events at the edge of
your screen, hover for details, and click to join a meeting.

- Local ICS files and HTTP/HTTPS/webcal subscriptions
- Recurring events, multiple calendars, and offline caching
- Native settings, theme integration, and optional start at login
- 13-pixel strip; no browser engine or continuous render loop

Works with layer-shell compositors such as Hyprland and Sway. Omarchy gets
matching colors and fonts; no Omarchy plugin is required.

## Install

With Rust and the [runtime dependencies](docs/install.md) installed:

```sh
make install PREFIX="$HOME/.local"
~/.local/bin/calstack --settings
~/.local/bin/calstack
```

Add your ICS subscription in Settings. Use the bottom `⋮` menu for Settings,
Refresh, or Quit. Changes apply automatically; **Start at login** is optional.

For a preview without a calendar: `calstack --demo`.

## More

[Install, autostart & uninstall](docs/install.md) ·
[Configuration & calendar support](docs/configuration.md) ·
[Development & verification](docs/development.md) ·
[Build plan](PLAN.md)

MIT licensed.
