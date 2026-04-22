// wdotool bridge — exposes a small window-management D-Bus interface on the
// session bus so the wdotool CLI can list / focus / close GNOME Shell windows.
//
// GNOME Shell doesn't expose a generic external window API (unlike KWin's
// scripting interface or the wlroots foreign-toplevel protocol), so a small
// companion extension is the standard pattern here — same shape as the
// KDE path in `src/backend/kde.rs`. Window identity uses stable_sequence,
// which is unique per window for the lifetime of the Shell session.
//
// The extension takes no configuration, runs no timers, and performs no
// background work — `enable()` registers the D-Bus object and `disable()`
// tears it down.

import Gio from 'gi://Gio';
import Shell from 'gi://Shell';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'org.wdotool.GnomeShellBridge';
const OBJECT_PATH = '/org/wdotool/GnomeShellBridge';

const IFACE_XML = `
<node>
  <interface name="org.wdotool.GnomeShellBridge">
    <method name="ListWindows">
      <arg type="s" direction="out" name="json"/>
    </method>
    <method name="GetActiveWindow">
      <arg type="s" direction="out" name="json"/>
    </method>
    <method name="ActivateWindow">
      <arg type="s" direction="in" name="id"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
    <method name="CloseWindow">
      <arg type="s" direction="in" name="id"/>
      <arg type="b" direction="out" name="ok"/>
    </method>
  </interface>
</node>`;

function windowId(w) {
    // get_stable_sequence() is unique per MetaWindow for the lifetime of the
    // Shell session and is the id GNOME's own debug tooling uses.
    return String(w.get_stable_sequence());
}

function windowJson(w) {
    const tracker = Shell.WindowTracker.get_default();
    const app = tracker.get_window_app(w);
    return {
        id: windowId(w),
        title: w.get_title() || '',
        app_id: app ? app.get_id() : null,
        pid: w.get_pid() || null,
    };
}

function allWindows() {
    // Override-redirect windows (tooltips, popups) aren't useful automation
    // targets and muddle list output — drop them.
    return global
        .get_window_actors()
        .map((a) => a.meta_window)
        .filter((w) => w && !w.is_override_redirect());
}

function findById(id) {
    for (const w of allWindows()) {
        if (windowId(w) === id) return w;
    }
    return null;
}

export default class WdotoolExtension extends Extension {
    enable() {
        this._impl = Gio.DBusExportedObject.wrapJSObject(IFACE_XML, this);
        this._impl.export(Gio.DBus.session, OBJECT_PATH);
        this._busOwnerId = Gio.bus_own_name(
            Gio.BusType.SESSION,
            BUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            null,
            null,
            null
        );
    }

    disable() {
        if (this._impl) {
            this._impl.unexport();
            this._impl = null;
        }
        if (this._busOwnerId) {
            Gio.bus_unown_name(this._busOwnerId);
            this._busOwnerId = 0;
        }
    }

    ListWindows() {
        return JSON.stringify(allWindows().map(windowJson));
    }

    GetActiveWindow() {
        const w = global.display.focus_window;
        return w ? JSON.stringify(windowJson(w)) : 'null';
    }

    ActivateWindow(id) {
        const w = findById(id);
        if (!w) return false;
        w.activate(global.get_current_time());
        return true;
    }

    CloseWindow(id) {
        const w = findById(id);
        if (!w) return false;
        w.delete(global.get_current_time());
        return true;
    }
}
