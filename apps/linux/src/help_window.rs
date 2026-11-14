//! 帮助窗口："键盘快捷键""支持的格式"：一张两列的表，分组标题加粗，内容由核心给出。
//! "开源许可"没有表，是一段可以选中复制的文字（Anyview 和它包含的第三方软件的许可证原文）。
//! Linux 没有菜单栏，这几个窗口从"关于"窗口的按钮打开（F1 是"关于"）。

use std::rc::Rc;

use anyview_core::help::{HelpKind, HelpRow};
use anyview_core::i18n::Key;
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::window::Ui;

/// 打开帮助窗口；同一种已经开着时放到最前面。
pub fn show(ui: &Rc<Ui>, kind: HelpKind) -> gtk::Window {
    let title = kind.title(ui.lang());
    if let Some(existing) = ui
        .help_windows
        .borrow()
        .iter()
        .find(|w| w.title().is_some_and(|t| t == title))
    {
        existing.present();
        return existing.clone();
    }
    let text = ui.core.help_text(kind, ui.lang());
    // 许可证原文按 80 列排版：等宽字体（text_body），窗口放得下 80 列。
    let width = if text.is_some() { 680 } else { 560 };
    let body: gtk::Widget = match text {
        Some(text) => text_body(&text).upcast(),
        None => table(&ui.core.help_rows(kind, ui.lang())).upcast(),
    };
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&body)
        .build();
    let window = gtk::Window::builder()
        .title(title)
        .transient_for(&ui.window)
        .destroy_with_parent(true)
        .default_width(width)
        .default_height(600)
        .child(&scroller)
        .build();
    let keys = gtk::EventControllerKey::new();
    let weak = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape
            && let Some(w) = weak.upgrade()
        {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    let ui_weak = Rc::downgrade(ui);
    window.connect_close_request(move |w| {
        if let Some(ui) = ui_weak.upgrade() {
            ui.help_windows.borrow_mut().retain(|x| x != w);
        }
        glib::Propagation::Proceed
    });
    ui.help_windows.borrow_mut().push(window.clone());
    window.present();
    window
}

/// 一段只读的文字（开源许可）：能选中复制，按窗口宽度折行。
fn text_body(text: &str) -> gtk::TextView {
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_monospace(true);
    view.add_css_class("anyview-mono");
    view.set_top_margin(12);
    view.set_bottom_margin(12);
    view.set_left_margin(16);
    view.set_right_margin(16);
    view.buffer().set_text(text);
    view
}

/// 两列的表：分组标题加粗、占两列。
fn table(rows: &[HelpRow]) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(24);
    grid.set_row_spacing(4);
    grid.set_margin_top(12);
    grid.set_margin_bottom(12);
    grid.set_margin_start(16);
    grid.set_margin_end(16);
    for (i, row) in rows.iter().enumerate() {
        let i = i as i32;
        if row.heading {
            let label = gtk::Label::new(None);
            label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&row.left)));
            label.set_xalign(0.0);
            if i > 0 {
                label.set_margin_top(10);
            }
            grid.attach(&label, 0, i, 2, 1);
        } else {
            let left = gtk::Label::new(Some(&row.left));
            left.set_xalign(0.0);
            let right = gtk::Label::new(Some(&row.right));
            right.set_xalign(0.0);
            right.set_wrap(true);
            grid.attach(&left, 0, i, 1, 1);
            grid.attach(&right, 1, i, 1, 1);
        }
    }
    grid
}

/// 帮助窗口的行数（自测记录用）。
pub fn row_count(rows: &[HelpRow]) -> (usize, usize) {
    (
        rows.iter().filter(|r| r.heading).count(),
        rows.iter().filter(|r| !r.heading).count(),
    )
}

/// "关于"：程序名、版本、许可证，下面三个按钮打开"键盘快捷键""支持的格式""开源许可"。
pub fn show_about(ui: &Rc<Ui>) {
    let name = gtk::Label::new(None);
    name.set_markup(&format!(
        "<span size=\"x-large\"><b>{}</b></span>",
        glib::markup_escape_text(ui.text(Key::AppName))
    ));
    let version = gtk::Label::new(Some(anyview_core::version()));
    version.add_css_class("dim-label");
    let lang = ui.lang();
    let license = gtk::Label::new(Some(anyview_core::licenses::about_line(lang)));
    license.add_css_class("dim-label");
    let shortcuts = gtk::Button::with_label(HelpKind::Shortcuts.title(lang));
    let formats = gtk::Button::with_label(HelpKind::Formats.title(lang));
    let licenses = gtk::Button::with_label(HelpKind::Licenses.title(lang));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::Center);
    buttons.append(&shortcuts);
    buttons.append(&formats);
    buttons.append(&licenses);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_margin_top(24);
    content.set_margin_bottom(20);
    content.set_margin_start(32);
    content.set_margin_end(32);
    content.append(&name);
    content.append(&version);
    content.append(&license);
    content.append(&buttons);
    let window = gtk::Window::builder()
        .title(ui.text(Key::AppName))
        .transient_for(&ui.window)
        .modal(true)
        .resizable(false)
        .child(&content)
        .build();
    for (button, kind) in [
        (&shortcuts, HelpKind::Shortcuts),
        (&formats, HelpKind::Formats),
        (&licenses, HelpKind::Licenses),
    ] {
        let ui_weak = Rc::downgrade(ui);
        let about = window.downgrade();
        button.connect_clicked(move |_| {
            if let Some(about) = about.upgrade() {
                about.close();
            }
            if let Some(ui) = ui_weak.upgrade() {
                show(&ui, kind);
            }
        });
    }
    let keys = gtk::EventControllerKey::new();
    let weak = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape
            && let Some(w) = weak.upgrade()
        {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    window.present();
}
