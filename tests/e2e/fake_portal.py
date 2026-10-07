#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""The harness's org.freedesktop.portal.Desktop for the Linux journeys.

Hosted runners and WSL have no desktop portal, so the journey harness
(xtask/src/journey/linux.rs) runs the editor on a private session bus with
this stand-in. It speaks the real protocol: FileChooser.OpenFile and SaveFile
return a Request handle and answer later through its Response signal, and
Settings.ReadOne and Read answer the appearance keys with "no preference".

The harness stages each answer before it runs the command that opens a
chooser: it writes the chosen paths, one per line, to FAKE_PORTAL_ANSWERS. A
request consumes the staged answer (a request with none is cancelled, as if
the user pressed Cancel) and appends one JSON line to FAKE_PORTAL_LOG once the
Response was sent, which is how the harness sees that the editor asked.

Environment:
  DBUS_SESSION_BUS_ADDRESS  the private bus to own the portal name on
  FAKE_PORTAL_ANSWERS       the staged answer file
  FAKE_PORTAL_LOG           the request log (JSON lines)
  FAKE_PORTAL_DELAY         seconds until a chooser answers (default 0.3)
"""
import json
import os
import pathlib
import signal
import sys

from gi.repository import Gio, GLib

DESKTOP = "/org/freedesktop/portal/desktop"
CHOOSER = "org.freedesktop.portal.FileChooser"
SETTINGS = "org.freedesktop.portal.Settings"
XML = """<node>
<interface name="org.freedesktop.portal.FileChooser">
  <method name="OpenFile"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
  <method name="SaveFile"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
  <property name="version" type="u" access="read"/>
</interface>
<interface name="org.freedesktop.portal.Settings">
  <method name="ReadOne"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
  <method name="Read"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
  <signal name="SettingChanged"><arg type="s"/><arg type="s"/><arg type="v"/></signal>
  <property name="version" type="u" access="read"/>
</interface>
</node>"""


def staged_answer():
    """The staged paths as file URIs, consumed by this request; None cancels."""
    path = os.environ.get("FAKE_PORTAL_ANSWERS")
    if not path:
        return None
    try:
        text = pathlib.Path(path).read_text(encoding="utf-8")
        os.remove(path)
    except OSError:
        return None
    lines = [line for line in text.splitlines() if line]
    return [pathlib.Path(line).absolute().as_uri() for line in lines] or None


def log(entry):
    path = os.environ.get("FAKE_PORTAL_LOG")
    if path:
        with open(path, "a", encoding="utf-8") as file:
            file.write(json.dumps(entry, ensure_ascii=False) + "\n")
    print(f"[fake-portal] {json.dumps(entry, ensure_ascii=False)}", flush=True)


def on_call(connection, sender, path, interface, method, parameters, invocation):
    if interface == CHOOSER:
        parent, title, options = parameters.unpack()
        token = options.get("handle_token", "bareline")
        handle = f"{DESKTOP}/request/{sender[1:].replace('.', '_')}/{token}"
        invocation.return_value(GLib.Variant("(o)", (handle,)))
        uris = staged_answer()
        entry = {
            "method": method,
            "parent": parent,
            "title": title,
            "options": sorted(options.keys()),
            "directory": bool(options.get("directory", False)),
            "uris": uris or [],
            "response": 0 if uris else 1,
        }
        delay = int(float(os.environ.get("FAKE_PORTAL_DELAY", "0.3")) * 1000)

        def respond():
            results = {"uris": GLib.Variant("as", uris or [])}
            connection.emit_signal(
                sender,
                handle,
                "org.freedesktop.portal.Request",
                "Response",
                GLib.Variant("(ua{sv})", (entry["response"], results)),
            )
            log(entry)
            return False

        GLib.timeout_add(delay, respond)
        return
    if interface == SETTINGS:
        namespace, key = parameters.unpack()
        if namespace == "org.freedesktop.appearance" and key in ("color-scheme", "contrast"):
            value = GLib.Variant("u", 0)
        else:
            invocation.return_dbus_error("org.freedesktop.portal.Error.NotFound", "Requested setting not found")
            return
        if method == "ReadOne":
            invocation.return_value(GLib.Variant("(v)", (value,)))
        else:
            invocation.return_value(GLib.Variant("(v)", (GLib.Variant("v", value),)))


def on_property(connection, sender, path, interface, name):
    return GLib.Variant("u", 4 if interface == CHOOSER else 2)


def main():
    address = os.environ["DBUS_SESSION_BUS_ADDRESS"]
    connection = Gio.DBusConnection.new_for_address_sync(
        address,
        Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
        None,
        None,
    )
    info = Gio.DBusNodeInfo.new_for_xml(XML)
    for interface in info.interfaces:
        connection.register_object(DESKTOP, interface, on_call, on_property, None)
    Gio.bus_own_name_on_connection(connection, "org.freedesktop.portal.Desktop", Gio.BusNameOwnerFlags.NONE, None, None)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, lambda *_: sys.exit(0))
    print(f"[fake-portal] serving on {address}", flush=True)
    GLib.MainLoop().run()


if __name__ == "__main__":
    main()
