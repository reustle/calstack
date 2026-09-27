"""On-demand GTK settings; the Rust executable validates and writes the config."""
import copy
import json
import re
import subprocess
import sys
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gio, Gtk

BINARY, CONFIG = sys.argv[1:3]
PAYLOAD = json.load(sys.stdin)


def box(orientation=Gtk.Orientation.VERTICAL, spacing=12):
    return Gtk.Box(orientation=orientation, spacing=spacing)


def padded(widget, margin=20):
    for side in ("top", "bottom", "start", "end"):
        getattr(widget, "set_margin_" + side)(margin)
    return widget


def label(text):
    return Gtk.Label(label=text, xalign=0)


def row(text, control):
    line = box(Gtk.Orientation.HORIZONTAL)
    title = label(text)
    title.set_hexpand(True)
    line.append(title)
    line.append(control)
    return line


def entry(value, placeholder=""):
    widget = Gtk.Entry(text=str(value), placeholder_text=placeholder)
    widget.set_hexpand(True)
    return widget


def spin(value, lower, upper, step=1, digits=0):
    widget = Gtk.SpinButton.new_with_range(lower, upper, step)
    widget.set_digits(digits)
    widget.set_value(value)
    return widget


class Settings(Gtk.Application):
    def __init__(self):
        super().__init__(application_id="io.github.reustle.calstack.settings",
                         flags=Gio.ApplicationFlags.NON_UNIQUE)
        self.config = copy.deepcopy(PAYLOAD["config"])
        self.original = PAYLOAD["original"]
        self.connect("activate", self.activate_window)

    def activate_window(self, *_):
        self.window = Gtk.ApplicationWindow(application=self, title="Calstack Settings")
        self.window.set_default_size(620, 650)
        self.window.set_size_request(480, 450)
        header = Gtk.HeaderBar()
        save = Gtk.Button(label="Save")
        save.add_css_class("suggested-action")
        save.connect("clicked", self.save)
        header.pack_end(save)
        self.window.set_titlebar(header)
        content = box(spacing=0)
        notebook = Gtk.Notebook()
        notebook.set_vexpand(True)
        notebook.append_page(self.calendars_page(), Gtk.Label(label="Calendars"))
        notebook.append_page(self.display_page(), Gtk.Label(label="Display & Startup"))
        content.append(notebook)
        self.status = padded(label("Changes apply to the running strip after saving."), 16)
        self.status.set_wrap(True)
        content.append(self.status)
        self.window.set_child(content)
        self.window.present()

    def calendars_page(self):
        page = padded(box())
        hint = label("Add an ICS subscription URL or a local .ics file.")
        hint.set_wrap(True)
        page.append(hint)
        self.feed_list = Gtk.ListBox(selection_mode=Gtk.SelectionMode.NONE)
        self.feed_list.add_css_class("boxed-list")
        scroll = Gtk.ScrolledWindow()
        scroll.set_child(self.feed_list)
        scroll.set_vexpand(True)
        page.append(scroll)
        self.render_feeds()
        add = Gtk.Button(label="Add calendar")
        add.connect("clicked", lambda *_: self.edit_feed(None))
        page.append(add)
        self.refresh_minutes = spin(self.config["calendar"]["refresh_minutes"], 1, 1440)
        page.append(row("Refresh interval (minutes)", self.refresh_minutes))
        refresh = Gtk.Button(label="Refresh saved calendars")
        refresh.connect("clicked", self.refresh)
        page.append(refresh)
        return page

    def render_feeds(self):
        while self.feed_list.get_first_child() is not None:
            self.feed_list.remove(self.feed_list.get_first_child())
        feeds = self.config["calendar"].setdefault("feeds", [])
        if not feeds:
            self.feed_list.append(padded(label("No calendars yet. Add one to get started."), 16))
        for index, feed in enumerate(feeds):
            line = padded(box(Gtk.Orientation.HORIZONTAL), 10)
            enabled = Gtk.Switch(active=feed["enabled"], valign=Gtk.Align.CENTER)
            enabled.set_tooltip_text("Enable " + feed["name"])
            enabled.connect("notify::active", lambda switch, _p, f=feed: f.update(enabled=switch.get_active()))
            line.append(enabled)
            title = label(feed["name"])
            title.set_hexpand(True)
            title.set_wrap(True)
            line.append(title)
            edit = Gtk.Button(label="Edit")
            edit.connect("clicked", lambda _b, i=index: self.edit_feed(i))
            line.append(edit)
            remove = Gtk.Button(icon_name="user-trash-symbolic")
            remove.set_tooltip_text("Remove " + feed["name"])
            remove.connect("clicked", lambda _b, i=index: self.remove_feed(i))
            line.append(remove)
            self.feed_list.append(line)

    def remove_feed(self, index):
        self.config["calendar"]["feeds"].pop(index)
        self.render_feeds()
        self.status.set_text("Calendar removed from this draft. Save to apply, or close to discard.")

    def edit_feed(self, index):
        feed = (self.config["calendar"]["feeds"][index] if index is not None
                else {"name": "", "url": "", "color": None, "enabled": True})
        window = Gtk.Window(title="Edit calendar" if index is not None else "Add calendar",
                            transient_for=self.window, modal=True, default_width=480)
        content = padded(box())
        name = entry(feed["name"])
        source_type = Gtk.DropDown.new_from_strings(["Subscription URL", "Local ICS file"])
        source_type.set_selected(1 if feed.get("path") else 0)
        source = entry(feed.get("path") or feed.get("url") or "", "https://…/calendar.ics")
        color = entry(feed.get("color") or "", "Theme default, or #RRGGBB")
        for title, widget in [("Name", name), ("Source", source_type), ("URL or path", source), ("Color", color)]:
            content.append(label(title))
            content.append(widget)
        message = label("")
        message.set_wrap(True)
        message.add_css_class("error")
        content.append(message)
        buttons = box(Gtk.Orientation.HORIZONTAL)
        buttons.set_halign(Gtk.Align.END)
        cancel = Gtk.Button(label="Cancel")
        cancel.connect("clicked", lambda *_: window.close())
        done = Gtk.Button(label="Done")
        done.add_css_class("suggested-action")

        def accept(*_):
            title, address, tint = name.get_text().strip(), source.get_text().strip(), color.get_text().strip()
            if not title or not address:
                message.set_text("Enter a name and a URL or file path.")
                return
            if any(i != index and f["name"] == title for i, f in enumerate(self.config["calendar"]["feeds"])):
                message.set_text("Choose a unique calendar name.")
                return
            if tint and not re.fullmatch(r"#[0-9a-fA-F]{6}", tint):
                message.set_text("Use a six-digit color such as #7F9BB3, or leave it blank.")
                return
            key = "path" if source_type.get_selected() else "url"
            if key == "url" and not address.startswith(("https://", "http://", "webcal://")):
                message.set_text("Use an HTTP, HTTPS, or webcal subscription URL.")
                return
            updated = {"name": title, key: address, "color": tint or None, "enabled": feed["enabled"]}
            if index is None:
                self.config["calendar"]["feeds"].append(updated)
            else:
                self.config["calendar"]["feeds"][index] = updated
            self.render_feeds()
            self.status.set_text("Calendar updated in this draft. Save to apply.")
            window.close()

        done.connect("clicked", accept)
        buttons.append(cancel)
        buttons.append(done)
        content.append(buttons)
        window.set_child(content)
        window.present()

    def display_page(self):
        display, appearance = self.config["display"], self.config["appearance"]
        page = padded(box(spacing=16))
        self.width = spin(display["width"], 8, 48)
        self.start = entry(display["day_start"])
        self.end = entry(display["day_end"])
        self.monitor = entry(display["monitor"], "primary or connector name")
        self.theme = Gtk.DropDown.new_from_strings(["System", "Dark", "Light"])
        self.theme.set_selected(["auto", "dark", "light"].index(appearance["theme"]))
        self.reserve = Gtk.Switch(active=display["reserve_space"], valign=Gtk.Align.CENTER)
        self.now = Gtk.Switch(active=appearance["show_now_marker"], valign=Gtk.Align.CENTER)
        self.autostart = Gtk.Switch(active=self.config["startup"]["autostart"], valign=Gtk.Align.CENTER)
        self.opacities = {key: spin(appearance[key], 0, 1, .05, 2)
                          for key in ("past_opacity", "future_opacity", "active_opacity")}
        fields = [("Strip width (pixels)", self.width), ("Day starts (HH:MM)", self.start),
                  ("Day ends (HH:MM)", self.end), ("Monitor", self.monitor),
                  ("Appearance", self.theme), ("Reserve screen space", self.reserve),
                  ("Show current time", self.now), ("Past event opacity", self.opacities["past_opacity"]),
                  ("Future event opacity", self.opacities["future_opacity"]),
                  ("Active event opacity", self.opacities["active_opacity"]),
                  ("Start at login", self.autostart)]
        for title, control in fields:
            page.append(row(title, control))
        scroll = Gtk.ScrolledWindow()
        scroll.set_child(page)
        return scroll

    def save(self, *_):
        self.config["display"].update(width=self.width.get_value_as_int(), day_start=self.start.get_text().strip(),
                                      day_end=self.end.get_text().strip(), monitor=self.monitor.get_text().strip(),
                                      reserve_space=self.reserve.get_active())
        self.config["appearance"].update(theme=["auto", "dark", "light"][self.theme.get_selected()],
                                         show_now_marker=self.now.get_active(),
                                         **{key: control.get_value() for key, control in self.opacities.items()})
        self.config["calendar"]["refresh_minutes"] = self.refresh_minutes.get_value_as_int()
        self.config["startup"]["autostart"] = self.autostart.get_active()
        try:
            result = subprocess.run([BINARY, "--config", CONFIG, "--save-settings"],
                                    input=json.dumps({"config": self.config, "original": self.original}),
                                    text=True, capture_output=True, timeout=10)
            if result.returncode:
                self.status.set_text(result.stderr.strip() or "Could not save settings.")
                return
            self.original = Path(CONFIG).read_text()
            self.status.set_text(result.stderr.strip() or "Saved. The running strip will update automatically.")
        except (OSError, subprocess.TimeoutExpired):
            self.status.set_text("Could not save settings. Check that Calstack is still installed.")

    def refresh(self, *_):
        try:
            result = subprocess.run([BINARY, "--config", CONFIG, "--refresh"], capture_output=True, text=True, timeout=10)
            self.status.set_text("Refresh requested." if result.returncode == 0 else result.stderr.strip())
        except (OSError, subprocess.TimeoutExpired):
            self.status.set_text("Could not request a refresh.")


if __name__ == "__main__":
    raise SystemExit(Settings().run([]))
