//! 悬浮界面（docs/MENUS.md）：底部控制栏、左右切图箭头、左上角图片信息。
//! 显示规则（指针区域、时间）由核心给出，这里只负责 GTK 控件和动画。

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use anyview_core::i18n::{self, Key};
use anyview_core::overlay::{self as rules, HoverZone};
use anyview_core::{AppCore, Lang, UiSettings};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::window::Ui;

/// 淡入淡出：用逐帧回调改不透明度；隐藏完成后把控件设为不可见，不再接收指针。
#[derive(Clone)]
pub struct Fader {
    widget: gtk::Widget,
    serial: Rc<Cell<u64>>,
    shown: Rc<Cell<bool>>,
}

impl Fader {
    pub fn new(widget: &impl IsA<gtk::Widget>) -> Self {
        let widget = widget.clone().upcast::<gtk::Widget>();
        widget.set_opacity(0.0);
        widget.set_visible(false);
        Self {
            widget,
            serial: Rc::new(Cell::new(0)),
            shown: Rc::new(Cell::new(false)),
        }
    }

    pub fn is_shown(&self) -> bool {
        self.shown.get()
    }

    pub fn show(&self) {
        if !self.shown.replace(true) {
            self.widget.set_visible(true);
            self.animate(1.0, rules::FADE_IN_MS);
        }
    }

    pub fn hide(&self) {
        if self.shown.replace(false) {
            self.animate(0.0, rules::FADE_OUT_MS);
        }
    }

    fn animate(&self, target: f64, duration_ms: u32) {
        let serial = self.serial.get() + 1;
        self.serial.set(serial);
        let current = Rc::clone(&self.serial);
        let from = self.widget.opacity();
        let start = Cell::new(None::<i64>);
        let duration = f64::from(duration_ms) * 1000.0;
        self.widget.add_tick_callback(move |widget, clock| {
            if current.get() != serial {
                return glib::ControlFlow::Break;
            }
            let now = clock.frame_time();
            let begun = start.get().unwrap_or(now);
            start.set(Some(begun));
            let progress = ((now - begun) as f64 / duration).clamp(0.0, 1.0);
            widget.set_opacity(from + (target - from) * progress);
            if progress < 1.0 {
                return glib::ControlFlow::Continue;
            }
            if target == 0.0 {
                widget.set_visible(false);
            }
            glib::ControlFlow::Break
        });
    }
}

/// 进度条的状态：当前序号、总数，以及拖动中尚未松开的目标序号。
#[derive(Default)]
pub struct StripState {
    pub index: Cell<Option<usize>>,
    pub total: Cell<Option<usize>>,
    pub pending: Cell<Option<usize>>,
}

/// 进度条两端留给滑块的边距（点）。
const STRIP_MARGIN: f64 = 8.0;

pub struct Overlays {
    pub bar: gtk::Box,
    pub bar_fade: Fader,
    pub strip: gtk::DrawingArea,
    pub strip_state: Rc<StripState>,
    lock_dot: gtk::Label,
    pub left: gtk::Button,
    pub right: gtk::Button,
    pub left_fade: Fader,
    pub right_fade: Fader,
    pub info: gtk::Box,
    pub info_label: gtk::Label,
    buttons: Vec<(gtk::Button, &'static str)>,
    info_close: gtk::Button,
    info_copy: gtk::Button,
    lock: gtk::Button,
    lock_label: gtk::Label,
    prev: gtk::Button,
    next: gtk::Button,
    open: gtk::Button,
    /// 设置"不显示控制栏""不显示侧边箭头"（完全不显示，不同于自动隐藏）。
    bar_hidden: Cell<bool>,
    arrows_hidden: Cell<bool>,
    /// 设置"不显示鼠标悬浮工具提示"。
    tooltips_hidden: Rc<Cell<bool>>,
    /// 最近一次指针所在的区域。
    zone: Cell<HoverZone>,
    pointer_on_bar: Rc<Cell<bool>>,
    bar_hide_serial: Rc<Cell<u64>>,
    /// 已经安排了淡出、还在等待（1 秒延时）。
    bar_hide_pending: Rc<Cell<bool>>,
    arrow_serial: Rc<Cell<u64>>,
    /// 动画信息控制：左下角的播放、暂停按钮和"第几帧 / 共几帧"。
    pub anim: gtk::Box,
    pub anim_button: gtk::Button,
    pub anim_label: gtk::Label,
    /// 设置"不显示动画信息控制"。
    anim_hidden: Cell<bool>,
}

fn icon_button(icon: &str, class: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class(class);
    button.set_focusable(false);
    button
}

fn separator() -> gtk::Separator {
    let line = gtk::Separator::new(gtk::Orientation::Vertical);
    line.add_css_class("anyview-bar-separator");
    line
}

impl Overlays {
    /// 建立控件并放进画面的叠加层。文字和提示由 [`Overlays::apply_settings`] 按语言填上，回调在 [`connect`] 中接上。
    pub fn new(overlay: &gtk::Overlay) -> Self {
        // 控制栏：左边两个大按钮，中间进度条和一排小按钮，右边两个大按钮（MENUS"底部控制栏"）。
        let open = icon_button("media-eject-symbolic", "anyview-bar-big");
        let select = icon_button("view-list-symbolic", "anyview-bar-big");
        let strip = gtk::DrawingArea::new();
        strip.set_content_height(18);
        strip.set_hexpand(true);
        strip.set_has_tooltip(true);
        let zoom_in = icon_button("zoom-in-symbolic", "anyview-bar-small");
        let zoom_out = icon_button("zoom-out-symbolic", "anyview-bar-small");
        let rotate_ccw = icon_button("object-rotate-left-symbolic", "anyview-bar-small");
        let rotate_cw = icon_button("object-rotate-right-symbolic", "anyview-bar-small");
        let lock = gtk::Button::new();
        lock.add_css_class("anyview-bar-lock");
        lock.set_focusable(false);
        let lock_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let lock_label = gtk::Label::new(None);
        lock_box.append(&lock_label);
        let lock_dot = gtk::Label::new(Some("●"));
        lock_dot.add_css_class("anyview-lock-dot");
        lock_box.append(&lock_dot);
        lock.set_child(Some(&lock_box));
        let prev = icon_button("go-previous-symbolic", "anyview-bar-big");
        let next = icon_button("go-next-symbolic", "anyview-bar-big");

        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        tools.set_halign(gtk::Align::Center);
        tools.append(&zoom_in);
        tools.append(&zoom_out);
        tools.append(&separator());
        tools.append(&rotate_ccw);
        tools.append(&rotate_cw);
        tools.append(&separator());
        tools.append(&lock);
        let middle = gtk::Box::new(gtk::Orientation::Vertical, 2);
        middle.set_valign(gtk::Align::Center);
        middle.set_size_request(220, -1);
        middle.append(&strip);
        middle.append(&tools);

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("anyview-bar");
        bar.set_halign(gtk::Align::Center);
        bar.set_valign(gtk::Align::End);
        bar.set_margin_bottom(16);
        bar.append(&open);
        bar.append(&select);
        bar.append(&middle);
        bar.append(&prev);
        bar.append(&next);
        overlay.add_overlay(&bar);
        let bar_fade = Fader::new(&bar);

        // 左右切图箭头：浅灰细线 ‹ ›，贴近窗口边缘、垂直居中，没有底板（MENUS"边缘切图箭头"）。
        let arrow = |icon: &str, align: gtk::Align| {
            let button = gtk::Button::from_icon_name(icon);
            button.add_css_class("anyview-arrow");
            button.set_focusable(false);
            button.set_halign(align);
            button.set_valign(gtk::Align::Center);
            if let Some(image) = button.child().and_then(|c| c.downcast::<gtk::Image>().ok()) {
                image.set_pixel_size(48);
            }
            overlay.add_overlay(&button);
            button
        };
        let left = arrow("go-previous-symbolic", gtk::Align::Start);
        left.set_margin_start(12);
        let right = arrow("go-next-symbolic", gtk::Align::End);
        right.set_margin_end(12);
        let left_fade = Fader::new(&left);
        let right_fade = Fader::new(&right);

        // 左上角图片信息：关闭、复制两个按钮，下面是白字深色描边的文字。
        let info_close = icon_button("window-close-symbolic", "anyview-info-button");
        let info_copy = icon_button("edit-copy-symbolic", "anyview-info-button");
        let info_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        info_buttons.append(&info_close);
        info_buttons.append(&info_copy);
        let info_label = gtk::Label::new(None);
        info_label.add_css_class("anyview-info-text");
        info_label.set_xalign(0.0);
        info_label.set_wrap(true);
        info_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        info_label.set_max_width_chars(48);
        let info = gtk::Box::new(gtk::Orientation::Vertical, 10);
        info.add_css_class("anyview-info");
        info.set_halign(gtk::Align::Start);
        info.set_valign(gtk::Align::Start);
        info.set_margin_start(16);
        info.set_margin_top(16);
        info.append(&info_buttons);
        info.append(&info_label);
        info.set_visible(false);
        overlay.add_overlay(&info);

        // 动画信息控制：左下角，控制栏出来时让到它上面。
        let anim_button = icon_button("media-playback-pause-symbolic", "anyview-anim-button");
        let anim_label = gtk::Label::new(None);
        anim_label.add_css_class("anyview-anim-text");
        let anim = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        anim.add_css_class("anyview-anim");
        anim.set_halign(gtk::Align::Start);
        anim.set_valign(gtk::Align::End);
        anim.set_margin_start(16);
        anim.set_margin_bottom(16);
        anim.append(&anim_button);
        anim.append(&anim_label);
        anim.set_visible(false);
        overlay.add_overlay(&anim);

        Self {
            bar,
            bar_fade,
            strip,
            strip_state: Rc::new(StripState::default()),
            lock_dot,
            left,
            right,
            left_fade,
            right_fade,
            info,
            info_label,
            buttons: vec![
                (select, "select_image"),
                (zoom_in, "zoom_in"),
                (zoom_out, "zoom_out"),
                (rotate_ccw, "rotate_ccw"),
                (rotate_cw, "rotate_cw"),
            ],
            info_close,
            info_copy,
            lock,
            lock_label,
            prev,
            next,
            open,
            bar_hidden: Cell::new(false),
            arrows_hidden: Cell::new(false),
            tooltips_hidden: Rc::new(Cell::new(false)),
            zone: Cell::new(HoverZone::None),
            pointer_on_bar: Rc::new(Cell::new(false)),
            bar_hide_serial: Rc::new(Cell::new(0)),
            bar_hide_pending: Rc::new(Cell::new(false)),
            arrow_serial: Rc::new(Cell::new(0)),
            anim,
            anim_button,
            anim_label,
            anim_hidden: Cell::new(false),
        }
    }

    /// 动画信息控制：正在显示动图时出现（设置"不显示动画信息控制"时不出现）；控制栏显示时让到它上面。
    pub fn refresh_animation(&self, ui: &Ui) {
        let info = ui.core.animation();
        self.anim
            .set_visible(info.is_some() && !self.anim_hidden.get());
        if let Some(info) = info {
            let count = info
                .count
                .map_or_else(|| "?".to_string(), |c| c.to_string());
            self.anim_label
                .set_text(&format!("{} / {}", info.index + 1, count));
            self.anim_button.set_icon_name(if info.playing {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            });
        }
        let lift = if self.bar_fade.is_shown() {
            self.bar.height() + 16 + 8
        } else {
            16
        };
        self.anim.set_margin_bottom(lift);
    }

    /// 动画信息控制的文字（自测用）；不显示时为空。
    pub fn animation_text(&self) -> Option<String> {
        self.anim
            .is_visible()
            .then(|| self.anim_label.text().to_string())
    }

    /// 按设置和语言填上文字与悬停提示，隐藏设置要求不显示的控件（设置"浏览"页）。
    pub fn apply_settings(&self, core: &AppCore, lang: Lang, settings: &UiSettings) {
        let show_tips = !settings.hide_tooltips;
        self.tooltips_hidden.set(!show_tips);
        let tip = |text: String| show_tips.then_some(text);
        // 提示里的快捷键按键盘映射（ACTIONS 规则 7）。
        let action_tip = |id: &str| tip(core.tooltip(id, lang));
        self.open.set_tooltip_text(action_tip("open_file").as_deref());
        for (button, id) in &self.buttons {
            button.set_tooltip_text(action_tip(id).as_deref());
        }
        self.lock.set_tooltip_text(action_tip("toggle_bar_lock").as_deref());
        self.lock_label.set_text(i18n::text(Key::BarLock, lang));
        self.prev
            .set_tooltip_text(tip(i18n::text(Key::BarPrevTip, lang).to_string()).as_deref());
        self.next
            .set_tooltip_text(tip(i18n::text(Key::BarNextTip, lang).to_string()).as_deref());
        self.info_close
            .set_tooltip_text(tip(i18n::text(Key::InfoClose, lang).to_string()).as_deref());
        self.info_copy
            .set_tooltip_text(tip(i18n::text(Key::InfoCopy, lang).to_string()).as_deref());
        self.strip.set_has_tooltip(show_tips);
        if settings.small_info_font {
            self.info_label.add_css_class("anyview-info-small");
        } else {
            self.info_label.remove_css_class("anyview-info-small");
        }
        self.bar_hidden.set(settings.hide_control_bar);
        self.anim_hidden.set(settings.hide_animation_info);
        self.anim_button
            .set_tooltip_text(action_tip("anim_toggle").as_deref());
        self.arrows_hidden.set(settings.hide_side_arrows);
        if settings.hide_control_bar {
            self.bar_fade.hide();
        }
        if settings.hide_side_arrows {
            self.left_fade.hide();
            self.right_fade.hide();
        }
    }

    /// 按核心的状态刷新：控制栏锁定、按钮可用、进度条、图片信息（MENUS 各节）。
    pub fn refresh(&self, ui: &Ui) {
        let locked = ui.core.action_state("toggle_bar_lock").checked;
        if locked {
            self.lock.add_css_class("locked");
        } else {
            self.lock.remove_css_class("locked");
        }
        self.lock_dot.set_opacity(if locked { 1.0 } else { 0.35 });
        self.open
            .set_sensitive(ui.core.action_state("open_file").enabled);
        for (button, id) in &self.buttons {
            button.set_sensitive(ui.core.action_state(id).enabled);
        }
        let has_image = ui.core.action_state("next_image").enabled;
        self.prev.set_sensitive(has_image);
        self.next.set_sensitive(has_image);
        let (index, total) = ui.current_position();
        self.strip_state.index.set(index);
        self.strip_state.total.set(total);
        self.strip.queue_draw();

        let lines = ui.core.info_lines(ui.lang());
        let show_info = ui.core.action_state("toggle_info").checked && !lines.is_empty();
        self.info_label.set_text(&lines.join("\n"));
        self.info.set_visible(show_info);
        if !has_image {
            self.left_fade.hide();
            self.right_fade.hide();
        }
        self.update_bar(locked);
        self.refresh_animation(ui);
    }

    /// 控制栏：在控制栏区域、指针在控制栏上或已锁定时显示；否则离开 1 秒后淡出。
    fn update_bar(&self, locked: bool) {
        if self.bar_hidden.get() {
            self.bar_fade.hide();
            return;
        }
        let wanted = locked || self.zone.get() == HoverZone::Bar || self.pointer_on_bar.get();
        if wanted {
            self.bar_hide_serial.set(self.bar_hide_serial.get() + 1);
            self.bar_hide_pending.set(false);
            self.bar_fade.show();
            return;
        }
        if !self.bar_fade.is_shown() || self.bar_hide_pending.get() {
            return;
        }
        let serial = self.bar_hide_serial.get() + 1;
        self.bar_hide_serial.set(serial);
        self.bar_hide_pending.set(true);
        let current = Rc::clone(&self.bar_hide_serial);
        let pending = Rc::clone(&self.bar_hide_pending);
        let fade = self.bar_fade.clone();
        glib::timeout_add_local_once(
            Duration::from_millis(u64::from(rules::BAR_HIDE_DELAY_MS)),
            move || {
                if current.get() == serial {
                    pending.set(false);
                    fade.hide();
                }
            },
        );
    }

    /// 指针移动（窗口内容区的点坐标）：按核心的区域规则显示控制栏和箭头。
    pub fn pointer_moved(&self, ui: &Ui, x: f64, y: f64, width: f64, height: f64) {
        let zone = rules::hover_zone(x, y, width, height);
        self.zone.set(zone);
        let has_image =
            ui.core.action_state("next_image").enabled && !self.arrows_hidden.get();
        match zone {
            HoverZone::Left if has_image => {
                self.left_fade.show();
                self.right_fade.hide();
            }
            HoverZone::Right if has_image => {
                self.right_fade.show();
                self.left_fade.hide();
            }
            _ => {
                self.left_fade.hide();
                self.right_fade.hide();
            }
        }
        // 静止 2 秒后箭头淡出；每次移动重新计时。
        let serial = self.arrow_serial.get() + 1;
        self.arrow_serial.set(serial);
        let current = Rc::clone(&self.arrow_serial);
        let (left, right) = (self.left_fade.clone(), self.right_fade.clone());
        glib::timeout_add_local_once(
            Duration::from_millis(u64::from(rules::ARROW_IDLE_MS)),
            move || {
                if current.get() == serial {
                    left.hide();
                    right.hide();
                }
            },
        );
        self.update_bar(ui.core.action_state("toggle_bar_lock").checked);
    }

    pub fn pointer_left(&self, ui: &Ui) {
        self.zone.set(HoverZone::None);
        self.left_fade.hide();
        self.right_fade.hide();
        self.update_bar(ui.core.action_state("toggle_bar_lock").checked);
    }

    /// 把进度条上的位置（0–1）换成序号并跳转（与松开鼠标时的处理相同；自测也用它）。
    pub fn jump_to_fraction(&self, ui: &Ui, fraction: f64) -> Option<usize> {
        let total = self.strip_state.total.get()?;
        let index = rules::progress_index(fraction, total)?;
        ui.jump_to(index);
        Some(index)
    }

    /// 自测：当前可见的悬浮控件在窗口中的范围（逻辑像素），核对画面时跳过这些区域。
    pub fn covered_rects(&self, window: &gtk::Window) -> Vec<gtk::graphene::Rect> {
        let mut rects = Vec::new();
        let widgets: [&gtk::Widget; 4] = [
            self.bar.upcast_ref(),
            self.left.upcast_ref(),
            self.right.upcast_ref(),
            self.info.upcast_ref(),
        ];
        for widget in widgets {
            if widget.is_visible()
                && let Some(bounds) = widget.compute_bounds(window)
            {
                rects.push(bounds);
            }
        }
        rects
    }

    /// 控制栏正在等待淡出（自测用）。
    pub fn bar_hide_pending(&self) -> bool {
        self.bar_hide_pending.get()
    }

    /// 进度条滑块现在的位置（0–1），自测用。
    pub fn knob_fraction(&self) -> Option<f64> {
        let total = self.strip_state.total.get()?;
        let index = self
            .strip_state
            .pending
            .get()
            .or(self.strip_state.index.get())?;
        Some(rules::progress_fraction(index, total))
    }
}

/// 进度条上 x（点）对应的位置（0–1）。
fn strip_fraction(x: f64, width: f64) -> f64 {
    let usable = (width - 2.0 * STRIP_MARGIN).max(1.0);
    ((x - STRIP_MARGIN) / usable).clamp(0.0, 1.0)
}

/// 接上回调：按钮执行动作，进度条悬停提示与拖动跳转，信息的关闭与复制。
pub fn connect(ui: &Rc<Ui>) {
    let o = &ui.overlays;

    let weak = Rc::downgrade(ui);
    o.open.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.invoke("open_file");
        }
    });
    for (button, id) in &o.buttons {
        let weak = Rc::downgrade(ui);
        let id = *id;
        button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.invoke(id);
            }
        });
    }
    let weak = Rc::downgrade(ui);
    o.anim_button.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.invoke("anim_toggle");
        }
    });
    let weak = Rc::downgrade(ui);
    o.lock.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.invoke("toggle_bar_lock");
        }
    });
    // 翻页按钮：左键翻 1 张，右键翻 10 张。
    for (button, click, right_click) in [
        (&o.prev, "prev_image", "back_10"),
        (&o.next, "next_image", "forward_10"),
        (&o.left, "prev_image", "back_10"),
        (&o.right, "next_image", "forward_10"),
    ] {
        let weak = Rc::downgrade(ui);
        button.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.invoke(click);
            }
        });
        let secondary = gtk::GestureClick::new();
        secondary.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(ui);
        secondary.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(ui) = weak.upgrade() {
                ui.invoke(right_click);
            }
        });
        button.add_controller(secondary);
    }

    // 指针停在控制栏上时不隐藏。
    let on_bar = gtk::EventControllerMotion::new();
    let flag = Rc::clone(&o.pointer_on_bar);
    on_bar.connect_enter(move |_, _, _| flag.set(true));
    let flag = Rc::clone(&o.pointer_on_bar);
    on_bar.connect_leave(move |_| flag.set(false));
    o.bar.add_controller(on_bar);

    // 进度条：画轨道和滑块；悬停提示"序号/总数"；单击或拖动，松开后跳转。
    let state = Rc::clone(&o.strip_state);
    o.strip.set_draw_func(move |_, cr, width, height| {
        let (w, h) = (f64::from(width), f64::from(height));
        let y = h / 2.0;
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_width(4.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.25);
        cr.move_to(STRIP_MARGIN, y);
        cr.line_to(w - STRIP_MARGIN, y);
        let _ = cr.stroke();
        if let Some(total) = state.total.get()
            && let Some(index) = state.pending.get().or(state.index.get())
        {
            let x =
                STRIP_MARGIN + rules::progress_fraction(index, total) * (w - 2.0 * STRIP_MARGIN);
            cr.set_source_rgba(0.95, 0.95, 0.95, 1.0);
            cr.arc(x, y, 6.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    });
    let weak = Rc::downgrade(ui);
    o.strip
        .connect_query_tooltip(move |strip, x, _, _, tooltip| {
            let Some(ui) = weak.upgrade() else {
                return false;
            };
            let state = &ui.overlays.strip_state;
            let text = match state.total.get() {
                Some(total) if total > 0 => {
                    let fraction = strip_fraction(f64::from(x), f64::from(strip.width()));
                    match state
                        .pending
                        .get()
                        .or_else(|| rules::progress_index(fraction, total))
                    {
                        Some(index) => rules::position_text(index, total),
                        None => return false,
                    }
                }
                _ => i18n::text(Key::BarScanning, ui.lang()).to_string(),
            };
            tooltip.set_text(Some(&text));
            true
        });
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let start_x = Rc::new(Cell::new(0.0));
    let weak = Rc::downgrade(ui);
    let begin_x = Rc::clone(&start_x);
    drag.connect_drag_begin(move |gesture, x, _| {
        let Some(ui) = weak.upgrade() else { return };
        begin_x.set(x);
        update_pending(&ui, gesture, x);
    });
    let weak = Rc::downgrade(ui);
    let update_x = Rc::clone(&start_x);
    drag.connect_drag_update(move |gesture, dx, _| {
        if let Some(ui) = weak.upgrade() {
            update_pending(&ui, gesture, update_x.get() + dx);
        }
    });
    let weak = Rc::downgrade(ui);
    drag.connect_drag_end(move |_, _, _| {
        let Some(ui) = weak.upgrade() else { return };
        let state = &ui.overlays.strip_state;
        if let Some(index) = state.pending.take() {
            ui.jump_to(index);
        }
        ui.overlays.strip.queue_draw();
    });
    o.strip.add_controller(drag);

    let weak = Rc::downgrade(ui);
    o.info_close.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.invoke("toggle_info");
        }
    });
    let weak = Rc::downgrade(ui);
    o.info_copy.connect_clicked(move |button| {
        if let Some(ui) = weak.upgrade() {
            button
                .clipboard()
                .set_text(&ui.core.info_lines(ui.lang()).join("\n"));
        }
    });
}

fn update_pending(ui: &Ui, gesture: &gtk::GestureDrag, x: f64) {
    let state = &ui.overlays.strip_state;
    let Some(total) = state.total.get() else {
        return;
    };
    let Some(strip) = gesture.widget() else {
        return;
    };
    state.pending.set(rules::progress_index(
        strip_fraction(x, f64::from(strip.width())),
        total,
    ));
    strip.queue_draw();
    strip.trigger_tooltip_query();
}
