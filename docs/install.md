# Installation

Calstack is a standalone Wayland application, not an Omarchy plugin. The strip
requires a compositor supporting `zwlr_layer_shell_v1` (including Hyprland and
Sway). X11 and stock GNOME's compositor do not provide this protocol.

## Dependencies

Build: Rust 1.89 or newer, Cargo, a C toolchain, and Make. Use a current stable
Rust release for the dependency versions in Cargo.lock.

Runtime: Wayland, libxkbcommon, fontconfig with installed fonts, and xdg-utils.
Graphical settings also use Python 3, PyGObject, and GTK 4, only while the
settings window is open. The strip can run without those settings dependencies.

On Arch/Omarchy, the runtime packages are `wayland libxkbcommon fontconfig
xdg-utils gtk4 python-gobject`; install `rust` and `base-devel` to build.
On Debian/Ubuntu, the equivalents include `libwayland-client0 libxkbcommon0
fontconfig xdg-utils python3-gi gir1.2-gtk-4.0`.

## Install for yourself

```sh
make install PREFIX="$HOME/.local"
~/.local/bin/calstack --settings
~/.local/bin/calstack
```

Keep `~/.local/bin` on your desktop session's PATH for the launcher entry.
Autostart uses an absolute executable path and does not depend on PATH.
The install includes a launcher entry and the license. To update, run the same
install command, quit the old process, and launch the installed binary again.

For a system installation, use `make install PREFIX=/usr/local` with appropriate
permissions. `DESTDIR` is supported for package staging. `cargo install --path
crates/calstack-app --locked` is also supported, but installs only the executable.

## Start at login

In Settings, enable **Start at login**, or set this in your config:

```toml
[startup]
autostart = true
```

Changes apply when the running app reloads the config. Without a running app:

```sh
calstack --sync-autostart
```

Calstack creates `~/.config/autostart/calstack.desktop` (respecting
`XDG_CONFIG_HOME`) using the standard [XDG autostart mechanism](https://specifications.freedesktop.org/autostart/latest/).
Turning the setting off removes its managed entry. The entry uses the installed
executable and selected config, never `--demo`. A stale entry also checks the
setting before opening the strip. Duplicate launches in one Wayland session
exit without adding a second strip.

Omarchy's UWSM session already starts XDG autostart applications. On a bare
compositor session, your session manager must run XDG autostart entries (for
example via UWSM or dex); Calstack does not modify compositor startup files.
The setting defaults to false. Quit closes the app for this session without
changing whether it starts at the next login.

## Arch package

Build a source archive from this checkout and create a package:

```sh
make dist
cd dist
makepkg -si
```

The generated PKGBUILD includes the source archive's SHA-256 checksum. No AUR
submission or package repository is required. Remove with `pacman -R calstack`.
Packages do not enable autostart for users or overwrite their settings.

## macOS

```sh
make install-macos    # builds and copies Calstack.app to /Applications
```

(`make bundle-macos` builds `dist/Calstack.app` without installing it; `make
uninstall-macos` removes the installed copy.) It's unsigned — no Apple
Developer ID — so the first launch needs a right-click → Open, or System
Settings → Privacy & Security → Open Anyway, past Gatekeeper. Launch it from
Launchpad/Spotlight, or `open /Applications/Calstack.app --args --demo` for a
preview. **Start at login** uses a `launchd` user agent instead of XDG
autostart; the cache lives under `~/Library/Caches/calstack` and the config
under `~/.config/calstack` (same as Linux, not `~/Library/Application
Support`). To update, rebuild and run `make install-macos` again — it
overwrites the existing `/Applications/Calstack.app`; quit the running app
first so the new build isn't blocked by the old one's instance lock.

## Uninstall

First turn **Start at login** off, or set `startup.autostart = false` and run
`calstack --sync-autostart`. Quit the app, then:

```sh
make uninstall PREFIX="$HOME/.local"
```

Use the same prefix as installation. Configuration and cached calendars are
preserved. If the binary has already been removed, delete its managed
`~/.config/autostart/calstack.desktop` entry manually. The cache lives under
`~/.cache/calstack`; the config under `~/.config/calstack` (both respect XDG paths).
