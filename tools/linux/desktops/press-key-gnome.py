#!/usr/bin/env python3
"""GNOME 测试用：像用户在键盘上按一下那样，按一个键（参数是 X 的键码名对应的数值，如 0xff1b 是 Esc）。

用 mutter 的远程桌面接口（org.gnome.Mutter.RemoteDesktop）：会话要在同一个 D-Bus 连接上建、用、关，
连接一断 mutter 就关掉会话，所以不能分几次用 gdbus 调用。只在桌面检查的容器里用（tools/linux/desktops）。
"""
import sys
import time

from gi.repository import Gio, GLib

keysym = int(sys.argv[1], 0)
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)


def call(path, iface, method, params=None, reply=None):
    return bus.call_sync("org.gnome.Mutter.RemoteDesktop", path, iface, method, params, reply,
                         Gio.DBusCallFlags.NONE, 5000, None)


session = call("/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop", "CreateSession",
               None, GLib.VariantType("(o)")).unpack()[0]
S = "org.gnome.Mutter.RemoteDesktop.Session"
call(session, S, "Start")
time.sleep(0.3)
call(session, S, "NotifyKeyboardKeysym", GLib.Variant("(ub)", (keysym, True)))
call(session, S, "NotifyKeyboardKeysym", GLib.Variant("(ub)", (keysym, False)))
time.sleep(0.3)
call(session, S, "Stop")
print(f"按了键 {keysym:#x}")
