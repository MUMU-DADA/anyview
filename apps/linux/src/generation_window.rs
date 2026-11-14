//! “生成信息”面板：AI 生成的图片里记的提示词、反向提示词、参数，每一段都能复制，最后可以复制文件里的原文。
//! 内容由核心读出（anyview_core::generation）；面板开着时跟着当前图片换内容（[`refresh`]，悬浮界面刷新时一起调），
//! 换到还没读好的图片时先不换（等读好了），界面语言变了也重新填。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyview_core::generation::{GenerationInfo, PanelTexts};
use anyview_core::i18n::Lang;
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::window::Ui;

/// 开着的面板和它现在显示的内容。
#[derive(Default)]
pub struct Panel {
    window: Option<gtk::Window>,
    /// 放内容的盒子（换图片时整个换掉）。
    content: Option<gtk::Box>,
    shown: Option<Option<Arc<GenerationInfo>>>,
    /// 填内容时用的语言。
    lang: Option<Lang>,
}

thread_local! {
    static PANEL: RefCell<Panel> = RefCell::new(Panel::default());
}

/// 打开面板；已经开着时放到最前面。
pub fn show(ui: &Rc<Ui>) {
    let existing = PANEL.with(|p| p.borrow().window.clone());
    if let Some(window) = existing {
        refresh(ui);
        window.present();
        return;
    }
    let texts = anyview_core::generation::panel_texts(ui.lang());
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(14);
    content.set_margin_bottom(14);
    content.set_margin_start(16);
    content.set_margin_end(16);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .build();
    let window = gtk::Window::builder()
        .title(&texts.title)
        .transient_for(&ui.window)
        .destroy_with_parent(true)
        .default_width(520)
        .default_height(560)
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
    window.connect_close_request(|_| {
        PANEL.with(|p| *p.borrow_mut() = Panel::default());
        glib::Propagation::Proceed
    });
    PANEL.with(|p| {
        let mut p = p.borrow_mut();
        p.window = Some(window.clone());
        p.content = Some(content);
        p.shown = None;
    });
    refresh(ui);
    window.present();
}

/// 面板开着时按当前图片重新填内容（内容没变时不动，免得选中的文字被清掉；当前图片还在读时也不动）。
pub fn refresh(ui: &Ui) {
    let Some(content) = PANEL.with(|p| p.borrow().content.clone()) else {
        return;
    };
    if ui.core.generation_pending() {
        return;
    }
    let info = ui.core.generation_info();
    let lang = ui.lang();
    let same = PANEL.with(|p| {
        let p = p.borrow();
        p.lang == Some(lang)
            && p.shown.as_ref().is_some_and(|shown| match (shown, &info) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
                (None, None) => true,
                _ => false,
            })
    });
    if same {
        return;
    }
    while let Some(child) = content.first_child() {
        content.remove(&child);
    }
    let texts = anyview_core::generation::panel_texts(lang);
    if let Some(window) = PANEL.with(|p| p.borrow().window.clone()) {
        window.set_title(Some(&texts.title));
    }
    match &info {
        Some(info) => fill(&content, info, &texts),
        None => {
            let empty = gtk::Label::new(Some(&texts.empty));
            empty.add_css_class("dim-label");
            empty.set_xalign(0.0);
            content.append(&empty);
        }
    }
    PANEL.with(|p| {
        let mut p = p.borrow_mut();
        p.shown = Some(info);
        p.lang = Some(lang);
    });
}

/// 面板开着没有、显示的是什么（自测记录用）：(开着, 生成工具, 提示词)。
pub fn state() -> (bool, Option<(String, String)>) {
    PANEL.with(|p| {
        let p = p.borrow();
        let shown = p
            .shown
            .clone()
            .flatten()
            .map(|g| (g.tool.clone(), g.prompt.clone()));
        (p.window.is_some(), shown)
    })
}

/// 开着的面板（自测截图用）。
pub fn window() -> Option<gtk::Window> {
    PANEL.with(|p| p.borrow().window.clone())
}

/// 关掉面板（自测用）。
pub fn close() {
    if let Some(window) = PANEL.with(|p| p.borrow().window.clone()) {
        window.close();
    }
}

fn fill(content: &gtk::Box, info: &GenerationInfo, texts: &PanelTexts) {
    // 这一行不能选中：GTK 打开窗口时把焦点给第一个能选中的标签，会把它整行选上；这样焦点落在第一个“复制”按钮上。
    let tool = gtk::Label::new(Some(&texts.tool.replace("{tool}", &info.tool)));
    tool.add_css_class("dim-label");
    tool.set_xalign(0.0);
    content.append(&tool);
    section(content, &texts.prompt, &info.prompt, texts);
    if !info.negative.is_empty() {
        section(content, &texts.negative, &info.negative, texts);
    }
    if !info.params.is_empty() {
        content.append(&heading(&texts.params, None));
        let grid = gtk::Grid::new();
        grid.set_column_spacing(16);
        grid.set_row_spacing(4);
        for (i, (name, value)) in info.params.iter().enumerate() {
            let name = gtk::Label::new(Some(name));
            name.add_css_class("dim-label");
            name.set_xalign(0.0);
            name.set_yalign(0.0);
            let value = gtk::Label::new(Some(value));
            value.set_xalign(0.0);
            value.set_wrap(true);
            value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            value.set_selectable(true);
            value.set_hexpand(true);
            grid.attach(&name, 0, i as i32, 1, 1);
            grid.attach(&value, 1, i as i32, 1, 1);
        }
        content.append(&grid);
    }
    let raw = copy_button(&texts.copy_raw, &info.raw, texts);
    raw.set_halign(gtk::Align::End);
    raw.set_margin_top(10);
    content.append(&raw);
}

/// 一段文字：标题、右边的“复制”，下面是可以选中的文字。
fn section(content: &gtk::Box, title: &str, text: &str, texts: &PanelTexts) {
    content.append(&heading(title, Some(copy_button(&texts.copy, text, texts))));
    let body = gtk::Label::new(Some(text));
    body.set_xalign(0.0);
    body.set_wrap(true);
    body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    body.set_selectable(true);
    body.set_margin_bottom(6);
    content.append(&body);
}

fn heading(title: &str, button: Option<gtk::Button>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(6);
    let label = gtk::Label::new(None);
    label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(title)));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    row.append(&label);
    if let Some(button) = button {
        row.append(&button);
    }
    row
}

/// “复制”按钮：按下后把文字放进剪贴板，按钮上的字短暂换成“已复制”。
fn copy_button(label: &str, text: &str, texts: &PanelTexts) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    let text = text.to_string();
    let (normal, copied) = (label.to_string(), texts.copied.clone());
    button.connect_clicked(move |button| {
        button.clipboard().set_text(&text);
        button.set_label(&copied);
        let weak = button.downgrade();
        let normal = normal.clone();
        glib::timeout_add_local_once(Duration::from_millis(1200), move || {
            if let Some(button) = weak.upgrade() {
                button.set_label(&normal);
            }
        });
    });
    button
}
