//! 窗口所在显示器的 ICC 配置文件（设置"使用监视器的 ICC 设置"）。
//!
//! - X11：桌面（colord、KDE 等）按 freedesktop 的"ICC Profiles in X"约定把配置文件放在根窗口的属性里：
//!   第一块屏是 `_ICC_PROFILE`，第 n 块是 `_ICC_PROFILE_n`。运行时打开 libX11 读（不链接；Wayland 下用不到）。
//!   屏的序号按 GDK 列出的显示器顺序。
//! - Wayland：桌面自己做色彩管理（如 KDE 的显示设置里给显示器选 ICC 配置文件），程序再转一遍就转了两次，
//!   所以报"没有"。
//! - 自测：环境变量 `ANYVIEW_TEST_MONITOR_ICC` 指向一个配置文件时用它（不管哪种桌面），核对转换的结果。
//!
//! 核心按设置决定用不用（关着时只记下），见 `AppCore::set_display_profile`。

use std::ffi::{c_char, c_int, c_long, c_ulong, c_void};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use libloading::Library;

use crate::window::Ui;

/// 配置文件最大多少字节（再大的不是显示器配置文件）。
const MAX_PROFILE: usize = 16 << 20;

/// 报给核心：窗口出现时、换到别的显示器时。
pub fn connect(ui: &Rc<Ui>) {
    let weak = Rc::downgrade(ui);
    ui.window.connect_realize(move |window| {
        let Some(surface) = window.surface() else { return };
        // 窗口刚碰到另一块屏时（enter）多半还算原来那块（重叠多的那块）；整个挪过去以后只来原来那块的 leave。
        // 两种都重新看一次。
        let weak_enter = weak.clone();
        surface.connect_enter_monitor(move |_, _| {
            if let Some(ui) = weak_enter.upgrade() {
                report(&ui);
            }
        });
        let weak_leave = weak.clone();
        surface.connect_leave_monitor(move |_, _| {
            if let Some(ui) = weak_leave.upgrade() {
                report(&ui);
            }
        });
        if let Some(ui) = weak.upgrade() {
            report(&ui);
        }
        // X11 下窗口停在两块屏之间、慢慢挪过去时不会再来 enter、leave：每 2 秒看一次窗口算在哪块屏上，
        // 变了才重新取配置文件（只比较显示器，不读文件，几乎不占 CPU）。Wayland 由桌面负责，不用看。
        if gtk::prelude::WidgetExt::display(window).type_().name() == "GdkX11Display" {
            let weak_poll = weak.clone();
            let last: std::cell::RefCell<Option<gtk::gdk::Monitor>> = std::cell::RefCell::new(current_monitor(window));
            glib::timeout_add_seconds_local(2, move || {
                let Some(ui) = weak_poll.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let now = current_monitor(&ui.window);
                if *last.borrow() != now {
                    *last.borrow_mut() = now;
                    report(&ui);
                }
                glib::ControlFlow::Continue
            });
        }
    });
}

fn current_monitor(window: &gtk::ApplicationWindow) -> Option<gtk::gdk::Monitor> {
    let surface = window.surface()?;
    gtk::prelude::WidgetExt::display(window).monitor_at_surface(&surface)
}

fn report(ui: &Rc<Ui>) {
    let profile = profile_for(&ui.window);
    ui.core.set_display_profile(profile);
}

/// 窗口所在显示器的配置文件；没有、或由桌面负责时为空。
pub fn profile_for(window: &gtk::ApplicationWindow) -> Option<Vec<u8>> {
    if let Some(path) = std::env::var_os("ANYVIEW_TEST_MONITOR_ICC") {
        return std::fs::read(path).ok();
    }
    let display = gtk::prelude::WidgetExt::display(window);
    if display.type_().name() != "GdkX11Display" {
        return None;
    }
    let surface = window.surface()?;
    let monitor = display.monitor_at_surface(&surface)?;
    let monitors = display.monitors();
    let index = (0..monitors.n_items()).find(|&i| {
        monitors
            .item(i)
            .and_then(|m| m.downcast::<gtk::gdk::Monitor>().ok())
            .is_some_and(|m| m == monitor)
    })?;
    let atom = if index == 0 {
        "_ICC_PROFILE\0".to_string()
    } else {
        format!("_ICC_PROFILE_{index}\0")
    };
    x11_root_property(&atom)
}

/// 读 X11 根窗口上的一个属性（8 位数据）。没有 libX11、连不上、没有这个属性时为空。
fn x11_root_property(name_nul: &str) -> Option<Vec<u8>> {
    type OpenDisplay = unsafe extern "C" fn(*const c_char) -> *mut c_void;
    type CloseDisplay = unsafe extern "C" fn(*mut c_void) -> c_int;
    type DefaultRootWindow = unsafe extern "C" fn(*mut c_void) -> c_ulong;
    type InternAtom = unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_ulong;
    type GetWindowProperty = unsafe extern "C" fn(
        *mut c_void,
        c_ulong,
        c_ulong,
        c_long,
        c_long,
        c_int,
        c_ulong,
        *mut c_ulong,
        *mut c_int,
        *mut c_ulong,
        *mut c_ulong,
        *mut *mut u8,
    ) -> c_int;
    type Free = unsafe extern "C" fn(*mut c_void) -> c_int;

    // SAFETY: 按 Xlib 的头文件里的签名取函数；连接用完就关，属性数据用 XFree 释放。
    unsafe {
        let library = Library::new("libX11.so.6").ok()?;
        let open: OpenDisplay = *library.get(b"XOpenDisplay\0").ok()?;
        let close: CloseDisplay = *library.get(b"XCloseDisplay\0").ok()?;
        let root: DefaultRootWindow = *library.get(b"XDefaultRootWindow\0").ok()?;
        let intern: InternAtom = *library.get(b"XInternAtom\0").ok()?;
        let get: GetWindowProperty = *library.get(b"XGetWindowProperty\0").ok()?;
        let free: Free = *library.get(b"XFree\0").ok()?;
        let dpy = open(std::ptr::null());
        if dpy.is_null() {
            return None;
        }
        // only_if_exists = True：没有这个属性名时不新建。
        let atom = intern(dpy, name_nul.as_ptr().cast(), 1);
        let mut result = None;
        if atom != 0 {
            let (mut kind, mut format, mut items, mut after) = (0, 0, 0, 0);
            let mut data: *mut u8 = std::ptr::null_mut();
            // AnyPropertyType = 0；长度按 32 位为单位。
            let status = get(
                dpy,
                root(dpy),
                atom,
                0,
                (MAX_PROFILE / 4) as c_long,
                0,
                0,
                &mut kind,
                &mut format,
                &mut items,
                &mut after,
                &mut data,
            );
            if status == 0 && !data.is_null() {
                let len = items as usize;
                if format == 8 && len > 128 && len <= MAX_PROFILE {
                    result = Some(std::slice::from_raw_parts(data, len).to_vec());
                }
                free(data.cast());
            }
        }
        close(dpy);
        result
    }
}
