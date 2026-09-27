#!/usr/bin/env python3
"""Exercise GTK controls and the actual Rust save path using a disposable config.

Run with a desktop session, or: xvfb-run -a python3 scripts/test-settings.py
"""
import io
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import tomllib

repo = Path(__file__).resolve().parents[1]
binary = repo / 'target/debug/calstack'
assert binary.exists(), 'Run cargo build first'
with tempfile.TemporaryDirectory(prefix='calstack-settings-test-') as temp:
    root = Path(temp)
    os.environ['XDG_CONFIG_HOME'] = str(root / 'xdg')
    os.environ['XDG_CACHE_HOME'] = str(root / 'cache')
    config = root / 'config.toml'
    subprocess.run([str(binary), '--config', str(config), '--check', '--demo'], check=True, stdout=subprocess.DEVNULL)
    original = config.read_text()
    data = tomllib.loads(original)
    data['calendar'].setdefault('feeds', [])
    sys.argv = ['settings.py', str(binary), str(config)]
    sys.stdin = io.StringIO(json.dumps({'config': data, 'original': original}))
    module = runpy.run_path(str(repo / 'crates/calstack-app/src/settings.py'), run_name='settings_test')
    Gtk = module['Gtk']
    from gi.repository import GLib

    app = module['Settings']()
    app.register(None)
    app.activate()

    def flush():
        context = GLib.MainContext.default()
        while context.pending():
            context.iteration(False)

    def widgets(widget):
        yield widget
        child = widget.get_first_child()
        while child:
            yield from widgets(child)
            child = child.get_next_sibling()

    flush()
    app.edit_feed(None)
    flush()
    dialog = next(w for w in Gtk.Window.list_toplevels() if w != app.window)
    entries = [w for w in widgets(dialog) if isinstance(w, Gtk.Entry)]
    assert len(entries) == 3
    for control, value in zip(entries, ['Test calendar', 'https://example.com/private.ics', '#7F9BB3']):
        control.set_text(value)
    next(w for w in widgets(dialog) if isinstance(w, Gtk.Button) and w.get_label() == 'Done').emit('clicked')
    flush()
    assert len(app.config['calendar']['feeds']) == 1
    assert config.read_text() == original, 'Editing a draft must not write settings'
    app.width.set_value(15)
    app.autostart.set_active(True)
    app.save()
    saved = tomllib.loads(config.read_text())
    assert saved['display']['width'] == 15
    assert saved['calendar']['feeds'][0]['name'] == 'Test calendar'
    assert saved['startup']['autostart']
    startup = root / 'xdg/autostart/calstack.desktop'
    assert startup.exists()
    before = config.read_text()
    app.end.set_text('05:00')
    app.save()
    assert config.read_text() == before, 'Invalid values must not overwrite valid settings'
    assert 'day_start must precede' in app.status.get_text()
    app.end.set_text('24:00')
    app.autostart.set_active(False)
    app.remove_feed(0)
    app.save()
    assert not tomllib.loads(config.read_text())['calendar'].get('feeds')
    assert not startup.exists()
    before = config.read_text() + '\n# External change\n'
    config.write_text(before)
    app.width.set_value(16)
    app.save()
    assert config.read_text() == before
    assert 'changed outside' in app.status.get_text()
    app.window.destroy()
    flush()
    print('PASS: GTK add/remove feed, draft editing, display changes, startup toggle, validation, and concurrent edit protection')
