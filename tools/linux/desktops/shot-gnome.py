#!/usr/bin/env python3
"""GNOME 截全屏：shot-gnome.py <输出的 PNG>。

gnome-shell 只让名单里的程序（如 org.gnome.SettingsDaemon.MediaKeys）调用截图接口。这里在容器自己的
D-Bus 上先占用这个名字再调用；容器里没有真的设置守护进程，不影响别的东西。"""
import sys
from gi.repository import Gio, GLib

bus = Gio.bus_get_sync(Gio.BusType.SESSION)
bus.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName",
              GLib.Variant("(su)", ("org.gnome.SettingsDaemon.MediaKeys", 4)), None, 0, 5000)
# gnome-shell 按名字的变化记下这个名字现在属于谁，稍等一下再调用。
GLib.usleep(500_000)
reply = bus.call_sync("org.gnome.Shell.Screenshot", "/org/gnome/Shell/Screenshot", "org.gnome.Shell.Screenshot",
                      "Screenshot", GLib.Variant("(bbs)", (False, False, sys.argv[1])),
                      GLib.VariantType("(bs)"), 0, 20000)
ok, path = reply.unpack()
print(f"截图：{ok} {path}")
sys.exit(0 if ok else 1)
