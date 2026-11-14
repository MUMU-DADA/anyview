//! 设置窗口（docs/SETTINGS.md"设置窗口"）。
//!
//!  重新设计：左边是带图标的分类列表；右边是页的大标题，下面按核心给的分组排成几张圆角卡片，
//! 每行左边是标题（下面可以有一行灰色说明），右边是开关、下拉框等控件；底部是"恢复本页默认"和"取消""确定"。
//! 所有修改只改核心里的草稿，点"确定"才校验、生效、保存；关闭窗口时有没确定的修改就先询问。哪些项能改、
//! 改了以后哪些项跟着灰显、开关是不是翻过来说的，都由核心决定（每次修改后重新取这一页的数据）。

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use anyview_core::actions::Chord;
use anyview_core::i18n::Key;
use anyview_core::settings::{ButtonCommand, Control, Input, ItemView, KeyMappingView, OptionView};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::window::Ui;

/// 每级缩进的宽度（像素）。
const INDENT: i32 = 20;

/// 左侧列表里各页的图标（freedesktop 的图标名）。挑的是 Adwaita（GNOME）和 Breeze（KDE）里都是单色、
/// 互相不重样的：文件类型的图标在 Breeze 里是彩色的，不用。
fn page_icon(id: &str) -> &'static str {
    match id {
        "general" => "preferences-system-symbolic",
        "browse" => "view-reveal-symbolic",
        "image" => "camera-photo-symbolic",
        "formats" => "document-open-symbolic",
        "keyboard" => "input-keyboard-symbolic",
        "mouse" => "input-mouse-symbolic",
        "bookmarks" => "user-bookmarks-symbolic",
        "context_menu" => "view-more-symbolic",
        "misc" => "view-app-grid-symbolic",
        "language" => "preferences-desktop-locale-symbolic",
        _ => "preferences-system-symbolic",
    }
}

pub struct SettingsWindow {
    pub window: gtk::Window,
    ui: Weak<Ui>,
    sidebar: gtk::ListBox,
    page_ids: Vec<String>,
    scroller: gtk::ScrolledWindow,
    page_box: gtk::Box,
    current: RefCell<String>,
    rows: RefCell<Vec<Row>>,
    /// 程序自己在改控件的值（不是用户操作），这时不把变化写进草稿。
    updating: Cell<bool>,
    /// 已经确定或放弃，窗口正在关闭：不再询问。
    closing: Cell<bool>,
    /// 已经安排了重建这一页（快捷键多了或少了一行）。
    rebuild_pending: Cell<bool>,
    /// 刚"添加"的快捷键行（映射表的行号）：重建这一页以后让它的按键框等着按键。
    capture_after_rebuild: Cell<Option<usize>>,
    /// 自测：上一次按钮做完后的说明（格式关联页）。
    last_report: RefCell<Option<String>>,
}

/// 一项在窗口里的控件。
struct Row {
    key: String,
    /// 整行（灰显时整行一起）。
    container: gtk::Widget,
    /// 开关是翻过来说的（只给自测把存的值换成开关的状态）。
    inverted: bool,
    control: RowControl,
}

enum RowControl {
    Toggle(gtk::Switch),
    Choice(gtk::DropDown, Vec<String>),
    Number(gtk::Entry),
    /// 当前颜色和各个色块（选中的色块外面有一圈）。
    Color(Rc<Cell<u32>>, Vec<gtk::DrawingArea>),
    Path(gtk::Entry),
    /// 鼠标映射：一个下拉框和各项的 ID（"default"、空或动作）。
    Mapping(gtk::DropDown, Vec<String>),
    /// 自己设的快捷键：列出来的各行、动作下拉框各项的 ID。
    KeyMappings(Vec<KeyRow>, Vec<String>),
    Other,
}

/// 自己设的快捷键的一行：按键框（点一下后按下组合键录入）、旁边的小按钮（列出不方便按出的键）、
/// 动作下拉框和提示、开关、删除按钮（SETTINGS"键盘"）。
struct KeyRow {
    /// 映射表里的行号。
    row: usize,
    container: gtk::Box,
    switch: gtk::Switch,
    key: gtk::Button,
    more: gtk::MenuButton,
    dropdown: gtk::DropDown,
    note: gtk::Label,
    /// 按键框正在等按键。
    capturing: Rc<Cell<bool>>,
}

impl SettingsWindow {
    fn ui(&self) -> Option<Rc<Ui>> {
        self.ui.upgrade()
    }

    /// 现在显示的页。
    pub fn current_page(&self) -> String {
        self.current.borrow().clone()
    }

    /// 切到一页（左侧列表跟着选中）。
    pub fn select_page(self: &Rc<Self>, id: &str) {
        if let Some(index) = self.page_ids.iter().position(|p| p == id)
            && let Some(row) = self.sidebar.row_at_index(index as i32)
        {
            self.sidebar.select_row(Some(&row));
        }
        self.show_page(id);
    }

    fn show_page(self: &Rc<Self>, id: &str) {
        let Some(ui) = self.ui() else { return };
        let Some(page) = ui.core.settings_page(id, ui.lang()) else {
            return;
        };
        let same_page = *self.current.borrow() == id;
        *self.current.borrow_mut() = id.to_string();
        while let Some(child) = self.page_box.first_child() {
            self.page_box.remove(&child);
        }
        self.updating.set(true);
        let title = gtk::Label::new(Some(page.title));
        title.add_css_class("anyview-settings-title");
        title.set_xalign(0.0);
        self.page_box.append(&title);
        if !page.lead.is_empty() {
            let lead = wrapping_label(page.lead);
            lead.add_css_class("anyview-settings-lead");
            self.page_box.append(&lead);
        }
        // 在不在 Flatpak 里决定显示哪些项（规则在核心）。
        let sandboxed = crate::sandbox::in_flatpak();
        let items: Vec<&ItemView> = page
            .items
            .iter()
            .filter(|item| anyview_core::settings::visible_with_sandbox(item.key, sandboxed))
            .collect();
        let mut rows = Vec::new();
        for (index, caption) in page.sections.iter().enumerate() {
            let members: Vec<&ItemView> =
                items.iter().copied().filter(|i| i.section == index).collect();
            if members.is_empty() {
                continue;
            }
            if !caption.is_empty() {
                let label = gtk::Label::new(Some(caption));
                label.add_css_class("anyview-settings-caption");
                label.set_xalign(0.0);
                self.page_box.append(&label);
            } else {
                let gap = gtk::Box::new(gtk::Orientation::Vertical, 0);
                gap.set_height_request(10);
                self.page_box.append(&gap);
            }
            // 快捷键列表另外排（下面还有"添加快捷键"）。
            if let [item] = members.as_slice()
                && let Control::KeyMappings {
                    rows: mappings,
                    options,
                    add_row,
                } = &item.control
            {
                let row = self.build_key_mappings(item, mappings, options, *add_row);
                self.page_box.append(&row.container);
                rows.push(row);
                continue;
            }
            let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
            card.add_css_class("anyview-settings-card");
            for (i, item) in members.iter().enumerate() {
                if i > 0 {
                    let line = gtk::Separator::new(gtk::Orientation::Horizontal);
                    line.set_margin_start(14 + i32::from(item.indent) * INDENT);
                    card.append(&line);
                }
                let row = self.build_row(item);
                card.append(&row.container);
                rows.push(row);
            }
            self.page_box.append(&card);
        }
        *self.rows.borrow_mut() = rows;
        self.updating.set(false);
        if !same_page {
            self.scroller.vadjustment().set_value(0.0);
        }
        self.refresh_states();
        if let Some(row) = self.capture_after_rebuild.take() {
            self.start_capture(row);
        }
    }

    /// 按核心的页面数据更新各项的可用状态（修改后，依赖它的项可能跟着灰显），以及快捷键各行的按键和提示。
    fn refresh_states(self: &Rc<Self>) {
        let Some(ui) = self.ui() else { return };
        let Some(page) = ui.core.settings_page(&self.current_page(), ui.lang()) else {
            return;
        };
        self.updating.set(true);
        for row in self.rows.borrow().iter() {
            let Some(item) = page.items.iter().find(|i| i.key == row.key) else {
                continue;
            };
            row.container.set_sensitive(item.enabled);
            if let (RowControl::KeyMappings(widgets, ids), Control::KeyMappings { rows, .. }) =
                (&row.control, &item.control)
            {
                // 多了或少了一行：整页重建（等这次事件处理完）。
                if widgets.len() != rows.len() || widgets.iter().zip(rows).any(|(w, r)| w.row != r.row)
                {
                    self.schedule_rebuild();
                    continue;
                }
                for (w, r) in widgets.iter().zip(rows) {
                    update_key_row(&ui, w, r, ids);
                }
            }
        }
        self.updating.set(false);
    }

    fn schedule_rebuild(self: &Rc<Self>) {
        if self.rebuild_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(w) = weak.upgrade() {
                w.rebuild_pending.set(false);
                let page = w.current_page();
                w.show_page(&page);
            }
        });
    }

    /// 把一次修改写进草稿，再刷新各项的可用状态。返回核心是否接受。
    fn set(self: &Rc<Self>, key: &str, input: Input) -> bool {
        if self.updating.get() {
            return false;
        }
        let Some(ui) = self.ui() else { return false };
        let accepted = ui.core.settings_set(key, input);
        self.refresh_states();
        accepted
    }

    /// 快捷键第 row 行录入了一个按键（按下的组合键，或小按钮里选的键；名称为空表示清除）。
    /// 系统占用的按键不接受，在这一行提示（ACTIONS 规则 6）。
    fn record_key(self: &Rc<Self>, row: usize, name: String, mods: u32) {
        let Some(ui) = self.ui() else { return };
        let accepted = self.set(
            anyview_core::settings::KEY_MAPPINGS,
            Input::KeyRowKey {
                row,
                key: name,
                mods,
            },
        );
        if !accepted {
            self.with_key_row(row, |w| {
                w.note.set_text(ui.text(Key::SettingsKeyReserved));
                w.note.set_visible(true);
            });
        }
    }

    fn with_key_row(&self, row: usize, f: impl FnOnce(&KeyRow)) {
        for item in self.rows.borrow().iter() {
            if let RowControl::KeyMappings(widgets, ..) = &item.control
                && let Some(w) = widgets.iter().find(|w| w.row == row)
            {
                f(w);
                return;
            }
        }
    }

    /// 让第 row 行的按键框等着按键（点了按键框，或刚添加了这一行）。
    fn start_capture(self: &Rc<Self>, row: usize) {
        let Some(ui) = self.ui() else { return };
        self.with_key_row(row, |w| {
            w.capturing.set(true);
            w.key.set_label(ui.text(Key::SettingsPressKey));
            w.key.grab_focus();
        });
    }

    /// 一行左边的文字：标题（后面可以跟着灰色的快捷键），下面一行灰色说明。
    fn text_column(item: &ItemView) -> gtk::Box {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        column.set_hexpand(true);
        column.set_valign(gtk::Align::Center);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = wrapping_label(&item.label);
        line.append(&title);
        if !item.shortcut.is_empty() {
            let shortcut = gtk::Label::new(Some(&item.shortcut));
            shortcut.add_css_class("anyview-settings-shortcut");
            shortcut.set_valign(gtk::Align::Center);
            line.append(&shortcut);
        }
        column.append(&line);
        if !item.detail.is_empty() {
            let detail = wrapping_label(&item.detail);
            detail.add_css_class("anyview-settings-detail");
            column.append(&detail);
        }
        column
    }

    fn build_row(self: &Rc<Self>, item: &ItemView) -> Row {
        let key = item.key.to_string();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("anyview-settings-row");
        row.set_margin_start(i32::from(item.indent) * INDENT);
        let weak = Rc::downgrade(self);
        let control = match &item.control {
            Control::Toggle(on) => {
                row.append(&Self::text_column(item));
                let switch = gtk::Switch::new();
                switch.set_valign(gtk::Align::Center);
                switch.set_active(*on);
                let key = key.clone();
                switch.connect_active_notify(move |switch| {
                    if let Some(w) = weak.upgrade() {
                        w.set(&key, Input::Bool(switch.is_active()));
                    }
                });
                row.append(&switch);
                RowControl::Toggle(switch)
            }
            Control::Choice { options, selected } => {
                row.append(&Self::text_column(item));
                let (dropdown, ids) = dropdown(options, selected, 220);
                let (key, list) = (key.clone(), ids.clone());
                dropdown.connect_selected_notify(move |dropdown| {
                    if let (Some(w), Some(id)) =
                        (weak.upgrade(), list.get(dropdown.selected() as usize))
                    {
                        w.set(&key, Input::Choice(id.clone()));
                    }
                });
                row.append(&dropdown);
                RowControl::Choice(dropdown, ids)
            }
            Control::Number { text, unit } => {
                row.append(&Self::text_column(item));
                let entry = gtk::Entry::new();
                entry.set_text(text);
                entry.set_width_chars(6);
                EditableExt::set_alignment(&entry, 1.0);
                entry.set_valign(gtk::Align::Center);
                entry.set_input_purpose(gtk::InputPurpose::Digits);
                let key = key.clone();
                entry.connect_changed(move |entry| {
                    if let Some(w) = weak.upgrade() {
                        w.set(&key, Input::Text(entry.text().to_string()));
                    }
                });
                row.append(&entry);
                let unit = gtk::Label::new(Some(unit));
                unit.add_css_class("anyview-settings-shortcut");
                row.append(&unit);
                RowControl::Number(entry)
            }
            Control::Color {
                value,
                presets,
                choose,
            } => {
                // 名字一行，下面一行是色块和"选取…"：15 个色块和名字排在一行时，
                // 英文和大多数语言的名字只剩几十像素宽，会被拆成几段。
                let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
                column.set_hexpand(true);
                column.append(&Self::text_column(item));
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                column.append(&line);
                row.append(&column);
                let current = Rc::new(Cell::new(*value));
                let strip = gtk::Box::new(gtk::Orientation::Horizontal, 2);
                strip.set_valign(gtk::Align::Center);
                let mut swatches = Vec::new();
                for &preset in presets {
                    let area = color_swatch(preset, Rc::clone(&current));
                    let chip = gtk::Button::new();
                    chip.add_css_class("flat");
                    chip.add_css_class("anyview-swatch");
                    chip.set_child(Some(&area));
                    chip.set_tooltip_text(Some(&anyview_core::settings::color_text(preset)));
                    let (weak, key) = (weak.clone(), key.clone());
                    chip.connect_clicked(move |_| {
                        if let Some(w) = weak.upgrade() {
                            w.set_color(&key, preset);
                        }
                    });
                    strip.append(&chip);
                    swatches.push(area);
                }
                line.append(&strip);
                // "选取…"：系统的取色器（预设里没有的颜色）。
                let button = gtk::Button::with_label(choose);
                button.set_valign(gtk::Align::Center);
                {
                    let key = key.clone();
                    button.connect_clicked(move |_| {
                        if let Some(w) = weak.upgrade() {
                            w.press_button(&key, "choose");
                        }
                    });
                }
                line.append(&button);
                RowControl::Color(current, swatches)
            }
            Control::Path { text, buttons } => {
                if !item.label.is_empty() {
                    let label = gtk::Label::new(Some(&item.label));
                    label.set_xalign(0.0);
                    label.set_width_request(76);
                    row.append(&label);
                }
                let entry = gtk::Entry::new();
                entry.set_text(text);
                entry.set_hexpand(true);
                entry.set_valign(gtk::Align::Center);
                {
                    let (weak, key) = (weak.clone(), key.clone());
                    entry.connect_changed(move |entry| {
                        if let Some(w) = weak.upgrade() {
                            w.set(&key, Input::Text(entry.text().to_string()));
                        }
                    });
                }
                row.append(&entry);
                for (id, title) in buttons {
                    row.append(&self.item_button(&key, id, title));
                }
                RowControl::Path(entry)
            }
            Control::Buttons(buttons) => {
                if item.label.is_empty() {
                    row.set_halign(gtk::Align::Start);
                } else {
                    row.append(&Self::text_column(item));
                }
                for (id, title) in buttons {
                    row.append(&self.item_button(&key, id, title));
                }
                RowControl::Other
            }
            Control::Note => {
                let label = wrapping_label(&item.label);
                label.add_css_class("anyview-settings-note");
                label.set_hexpand(true);
                row.append(&label);
                RowControl::Other
            }
            Control::Mapping { selected, options } => {
                row.append(&Self::text_column(item));
                let (dropdown, ids) = dropdown(options, selected, 220);
                let (key, list) = (key.clone(), ids.clone());
                dropdown.connect_selected_notify(move |dropdown| {
                    if let (Some(w), Some(id)) =
                        (weak.upgrade(), list.get(dropdown.selected() as usize))
                    {
                        w.set(&key, Input::MappingChoice(id.clone()));
                    }
                });
                row.append(&dropdown);
                RowControl::Mapping(dropdown, ids)
            }
            // 快捷键列表在 build_key_mappings 里排。
            Control::KeyMappings { .. } => RowControl::Other,
        };
        Row {
            key,
            container: row.upcast(),
            inverted: item.inverted,
            control,
        }
    }

    /// 设置项里的一个按钮（路径旁的"选取…"、格式关联页的按钮等）。
    fn item_button(self: &Rc<Self>, key: &str, id: &str, title: &str) -> gtk::Button {
        let button = gtk::Button::with_label(title);
        button.set_valign(gtk::Align::Center);
        let (weak, key, id) = (Rc::downgrade(self), key.to_string(), id.to_string());
        button.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.press_button(&key, &id);
            }
        });
        button
    }

    /// 自己设的快捷键：一张卡片列出用过的行，下面一个"添加快捷键"。
    fn build_key_mappings(
        self: &Rc<Self>,
        item: &ItemView,
        mappings: &[KeyMappingView],
        options: &[OptionView],
        add_row: Option<usize>,
    ) -> Row {
        let Some(ui) = self.ui() else {
            return Row {
                key: item.key.to_string(),
                container: gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
                inverted: false,
                control: RowControl::Other,
            };
        };
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let ids: Vec<String> = options.iter().map(|(id, _)| id.clone()).collect();
        let special = ui.core.settings_special_keys(ui.lang());
        let mut widgets = Vec::new();
        if !mappings.is_empty() {
            let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
            card.add_css_class("anyview-settings-card");
            for (i, mapping) in mappings.iter().enumerate() {
                if i > 0 {
                    let line = gtk::Separator::new(gtk::Orientation::Horizontal);
                    line.set_margin_start(14);
                    card.append(&line);
                }
                let w = self.build_key_row(&ui, mapping, options, &special);
                update_key_row(&ui, &w, mapping, &ids);
                card.append(&w.container);
                widgets.push(w);
            }
            outer.append(&card);
        }
        // "添加快捷键"：勾选映射表里的一行空行，它就出现在列表里，按键框等着按键。
        let add = gtk::Button::new();
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&gtk::Image::from_icon_name("list-add-symbolic"));
        content.append(&gtk::Label::new(Some(ui.text(Key::SettingsAddShortcut))));
        add.set_child(Some(&content));
        add.set_halign(gtk::Align::Start);
        add.set_sensitive(add_row.is_some());
        let (weak, key) = (Rc::downgrade(self), item.key.to_string());
        add.connect_clicked(move |_| {
            if let (Some(w), Some(row)) = (weak.upgrade(), add_row)
                && w.set(&key, Input::KeyRowEnabled { row, on: true })
            {
                w.capture_after_rebuild.set(Some(row));
                w.schedule_rebuild();
            }
        });
        outer.append(&add);
        Row {
            key: item.key.to_string(),
            container: outer.upcast(),
            inverted: false,
            control: RowControl::KeyMappings(widgets, ids),
        }
    }

    fn build_key_row(
        self: &Rc<Self>,
        ui: &Rc<Ui>,
        mapping: &KeyMappingView,
        options: &[OptionView],
        special: &[OptionView],
    ) -> KeyRow {
        let row = mapping.row;
        let key = anyview_core::settings::KEY_MAPPINGS;
        let weak = Rc::downgrade(self);
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        container.add_css_class("anyview-settings-row");
        // 按键框：点一下后按下组合键即录入（Esc、Tab 也能录入）。
        let key_button = gtk::Button::with_label("");
        key_button.add_css_class("anyview-key");
        key_button.set_valign(gtk::Align::Center);
        let capturing = Rc::new(Cell::new(false));
        {
            let weak = weak.clone();
            key_button.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.start_capture(row);
                }
            });
        }
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let (weak, capturing) = (weak.clone(), Rc::clone(&capturing));
            keys.connect_key_pressed(move |_, key, _, state| {
                if !capturing.get() {
                    return glib::Propagation::Proceed;
                }
                // 只按了修饰键：继续等。
                let Some(name) = crate::window::key_name(key) else {
                    return glib::Propagation::Stop;
                };
                capturing.set(false);
                if let Some(w) = weak.upgrade() {
                    w.record_key(row, name, crate::window::modifier_bits(state));
                }
                glib::Propagation::Stop
            });
        }
        key_button.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        {
            let (weak, capturing) = (weak.clone(), Rc::clone(&capturing));
            focus.connect_leave(move |_| {
                if capturing.replace(false)
                    && let Some(w) = weak.upgrade()
                {
                    w.refresh_states();
                }
            });
        }
        key_button.add_controller(focus);
        // 小按钮：列出不方便直接按出的键，首项是清除。
        let more = gtk::MenuButton::new();
        more.set_label("…");
        more.set_valign(gtk::Align::Center);
        let popover = gtk::Popover::new();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for (id, title) in special {
            let entry = gtk::Button::with_label(title);
            entry.add_css_class("flat");
            let (weak, popover, id) = (weak.clone(), popover.clone(), id.clone());
            entry.connect_clicked(move |_| {
                popover.popdown();
                if let Some(w) = weak.upgrade() {
                    w.record_key(row, id.clone(), 0);
                }
            });
            list.append(&entry);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(320)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        popover.set_child(Some(&scroller));
        more.set_popover(Some(&popover));
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        cell.append(&key_button);
        cell.append(&more);
        container.append(&cell);
        let arrow = gtk::Label::new(Some("→"));
        arrow.add_css_class("anyview-settings-shortcut");
        container.append(&arrow);
        // 动作下拉框，下面是这一行的提示（将替代某个默认快捷键、和别的行重复）。
        let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
        column.set_hexpand(true);
        column.set_valign(gtk::Align::Center);
        let (dropdown, ids) = dropdown(options, &mapping.action, 236);
        dropdown.set_halign(gtk::Align::Start);
        {
            let weak = weak.clone();
            dropdown.connect_selected_notify(move |dropdown| {
                if let (Some(w), Some(id)) = (weak.upgrade(), ids.get(dropdown.selected() as usize)) {
                    w.set(
                        key,
                        Input::KeyRowAction {
                            row,
                            action: id.clone(),
                        },
                    );
                }
            });
        }
        column.append(&dropdown);
        let note = wrapping_label("");
        note.add_css_class("anyview-settings-detail");
        column.append(&note);
        container.append(&column);
        let switch = gtk::Switch::new();
        switch.set_valign(gtk::Align::Center);
        {
            let weak = weak.clone();
            switch.connect_active_notify(move |switch| {
                if let Some(w) = weak.upgrade() {
                    w.set(
                        key,
                        Input::KeyRowEnabled {
                            row,
                            on: switch.is_active(),
                        },
                    );
                }
            });
        }
        container.append(&switch);
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.add_css_class("flat");
        remove.set_valign(gtk::Align::Center);
        remove.set_tooltip_text(Some(ui.text(Key::SettingsRemoveShortcut)));
        remove.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.set(key, Input::KeyRowRemove { row });
            }
        });
        container.append(&remove);
        KeyRow {
            row,
            container,
            switch,
            key: key_button,
            more,
            dropdown,
            note,
            capturing,
        }
    }

    /// 选了一个颜色（预设色块或取色器）：写进草稿，色块上的选中标记跟着换。
    fn set_color(self: &Rc<Self>, key: &str, color: u32) {
        for row in self.rows.borrow().iter().filter(|r| r.key == key) {
            if let RowControl::Color(current, swatches) = &row.control {
                current.set(color);
                for swatch in swatches {
                    swatch.queue_draw();
                }
            }
        }
        self.set(key, Input::Color(color));
    }

    /// 设置项里的按钮：问核心要做什么，再落到 GTK 上。
    pub fn press_button(self: &Rc<Self>, key: &str, button: &str) {
        let Some(ui) = self.ui() else { return };
        let Some(command) = ui.core.settings_button(key, button) else {
            return;
        };
        match command {
            ButtonCommand::ChooseColor { key } => {
                let initial = self
                    .rows
                    .borrow()
                    .iter()
                    .find_map(|r| match &r.control {
                        RowControl::Color(current, _) if r.key == key => Some(current.get()),
                        _ => None,
                    })
                    .unwrap_or(0x202020);
                let dialog = gtk::ColorDialog::builder()
                    .with_alpha(false)
                    .modal(true)
                    .build();
                let weak = Rc::downgrade(self);
                dialog.choose_rgba(
                    Some(&self.window),
                    Some(&rgba(initial)),
                    gio::Cancellable::NONE,
                    move |result| {
                        if let (Some(w), Ok(color)) = (weak.upgrade(), result) {
                            let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
                            let value = channel(color.red()) << 16
                                | channel(color.green()) << 8
                                | channel(color.blue());
                            w.set_color(&key, value);
                        }
                    },
                );
            }
            ButtonCommand::ChooseFolder { key } => self.choose_path(key, true),
            ButtonCommand::ChooseFile { key } => self.choose_path(key, false),
            // 图像编辑器：选一个程序文件。
            ButtonCommand::ChooseApp { key } => self.choose_path(key, false),
            // 核心改了草稿（如编辑器的"用推荐的"）：重新显示这一页。
            ButtonCommand::Refresh => self.schedule_rebuild(),
            // 书签页的"清除"：先确认。
            ButtonCommand::ConfirmClearArchivePositions => self.confirm_clear_positions(),
            ButtonCommand::OpenFolder { path } => {
                let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
                launcher.launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
            }
            // 格式关联页：写进 mimeapps.list；换回原来的默认程序。
            ButtonCommand::SetDefaultViewer => self.set_default_viewer(),
            ButtonCommand::RestoreDefaultViewer => self.restore_default_viewer(),
            // Flatpak 版让文件管理器用 Anyview 做缩略图。
            ButtonCommand::RegisterThumbnailer => self.register_thumbnailer(),
            ButtonCommand::UnregisterThumbnailer => self.unregister_thumbnailer(),
            // Windows 格式关联页的两个按钮：Linux 的设置窗口里没有。
            ButtonCommand::OpenDefaultApps | ButtonCommand::OpenAssociationsByApp => {}
        }
    }

    /// “登记”：在用户目录登记缩略图生成器，做完后说结果。
    fn register_thumbnailer(self: &Rc<Self>) {
        let Some(ui) = self.ui() else { return };
        let lang = ui.lang();
        let message = match crate::flatpak_thumbnailer::current()
            .and_then(|target| crate::flatpak_thumbnailer::register(&target))
        {
            Ok(count) => anyview_core::i18n::text(Key::SettingsThumbnailerRegistered, lang)
                .replace("{count}", &count.to_string()),
            Err(problem) => thumbnailer_problem(problem, lang),
        };
        self.report(message);
    }

    /// “取消登记”：删掉 Anyview 登记的缩略图生成器。
    fn unregister_thumbnailer(self: &Rc<Self>) {
        let Some(ui) = self.ui() else { return };
        let lang = ui.lang();
        let message = match crate::flatpak_thumbnailer::current()
            .and_then(|target| crate::flatpak_thumbnailer::unregister(&target))
        {
            Ok(true) => anyview_core::i18n::text(Key::SettingsThumbnailerRemoved, lang).to_string(),
            Ok(false) => anyview_core::i18n::text(Key::SettingsThumbnailerNothing, lang).to_string(),
            Err(problem) => thumbnailer_problem(problem, lang),
        };
        self.report(message);
    }

    /// "设为默认"：做完后说结果。
    fn set_default_viewer(self: &Rc<Self>) {
        let Some(ui) = self.ui() else { return };
        let lang = ui.lang();
        let result = match (crate::default_viewer::Locations::current(), std::env::current_exe()) {
            (Some(at), Ok(exe)) => crate::default_viewer::set_default(&at, &exe),
            (_, Err(e)) => Err(e.to_string()),
            (None, _) => Err("HOME".to_string()),
        };
        let message = match result {
            Ok(count) => anyview_core::i18n::text(Key::SettingsDefaultViewerDoneCount, lang)
                .replace("{count}", &count.to_string()),
            Err(reason) if reason == crate::default_viewer::FLATPAK => {
                anyview_core::i18n::text(Key::SettingsDefaultViewerFailed, lang).replace(
                    "{message}",
                    anyview_core::i18n::text(Key::SettingsDefaultViewerFlatpak, lang),
                )
            }
            Err(reason) => anyview_core::i18n::text(Key::SettingsDefaultViewerFailed, lang)
                .replace("{message}", &reason),
        };
        self.report(message);
    }

    /// "恢复原来的"。
    fn restore_default_viewer(self: &Rc<Self>) {
        let Some(ui) = self.ui() else { return };
        let lang = ui.lang();
        let result = crate::default_viewer::Locations::current()
            .ok_or_else(|| "HOME".to_string())
            .and_then(|at| crate::default_viewer::restore(&at));
        let message = match result {
            Ok(Some(count)) => anyview_core::i18n::text(Key::SettingsDefaultViewerRestored, lang)
                .replace("{count}", &count.to_string()),
            Ok(None) => anyview_core::i18n::text(Key::SettingsDefaultViewerNothing, lang).to_string(),
            Err(reason) => anyview_core::i18n::text(Key::SettingsDefaultViewerFailed, lang)
                .replace("{message}", &reason),
        };
        self.report(message);
    }

    /// 按钮做完后的说明：弹一个只有"好"的对话框；自测时不弹，记下来（`take_report`）。
    fn report(self: &Rc<Self>, message: String) {
        let Some(ui) = self.ui() else { return };
        if ui.selftest.borrow().is_some() {
            *self.last_report.borrow_mut() = Some(message);
            return;
        }
        gtk::AlertDialog::builder()
            .message(message.as_str())
            .modal(true)
            .build()
            .show(Some(&self.window));
    }

    /// 自测：上一次按钮做完后的说明。
    pub fn take_report(&self) -> Option<String> {
        self.last_report.borrow_mut().take()
    }

    /// 忘掉所有压缩包内的位置之前先问一下。自测时直接清。
    fn confirm_clear_positions(self: &Rc<Self>) {
        let Some(ui) = self.ui() else { return };
        if ui.selftest.borrow().is_some() {
            ui.core.clear_archive_positions();
            return;
        }
        let lang = ui.lang();
        let dialog = gtk::AlertDialog::builder()
            .message(anyview_core::i18n::text(Key::PromptClearPositions, lang))
            .buttons([
                anyview_core::i18n::text(Key::ButtonDelete, lang),
                anyview_core::i18n::text(Key::ButtonCancel, lang),
            ])
            .cancel_button(1)
            .default_button(1)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |choice| {
            if let (Ok(0), Some(w)) = (choice, weak.upgrade())
                && let Some(ui) = w.ui()
            {
                ui.core.clear_archive_positions();
            }
        });
    }

    /// 选文件或文件夹，选好后填进这一项的输入框（输入框的变化会写进草稿）。
    fn choose_path(self: &Rc<Self>, key: String, folder: bool) {
        let dialog = gtk::FileDialog::builder().modal(true).build();
        let weak = Rc::downgrade(self);
        let done = move |result: Result<gio::File, glib::Error>| {
            let (Some(w), Ok(file)) = (weak.upgrade(), result) else {
                return;
            };
            let Some(path) = file.path() else { return };
            for row in w.rows.borrow().iter().filter(|r| r.key == key) {
                if let RowControl::Path(entry) = &row.control {
                    entry.set_text(&path.display().to_string());
                }
            }
        };
        if folder {
            dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, done);
        } else {
            dialog.open(Some(&self.window), gio::Cancellable::NONE, done);
        }
    }

    /// "恢复本页默认"：这一页恢复默认值（还要点"确定"）。
    pub fn reset(self: &Rc<Self>) {
        let Some(ui) = self.ui() else { return };
        let page = self.current_page();
        ui.core.settings_reset_page(&page);
        self.show_page(&page);
    }

    /// "确定"：校验通过就生效、关窗口；不通过就切到出错的页、标出那一项（快捷键标出那一行）并说明原因。
    /// 返回是否已生效。
    pub fn ok(self: &Rc<Self>) -> bool {
        let Some(ui) = self.ui() else { return false };
        match ui.core.settings_apply() {
            Ok(applied) => {
                self.closing.set(true);
                self.window.close();
                ui.settings_applied(&applied);
                true
            }
            Err(invalid) => {
                self.select_page(invalid.page.id());
                if let Some(row) = self.rows.borrow().iter().find(|r| r.key == invalid.key) {
                    match &row.control {
                        RowControl::Number(entry) | RowControl::Path(entry) => {
                            row.container.add_css_class("anyview-invalid");
                            entry.grab_focus();
                        }
                        RowControl::KeyMappings(widgets, ..) => {
                            if let Some(w) = widgets.iter().find(|w| Some(w.row) == invalid.row) {
                                w.container.add_css_class("anyview-invalid-row");
                            }
                        }
                        _ => row.container.add_css_class("anyview-invalid"),
                    }
                }
                let message = invalid.message(anyview_core::Platform::Linux, ui.lang());
                *ui.settings_error.borrow_mut() = Some(message.clone());
                if ui.selftest.borrow().is_none() {
                    gtk::AlertDialog::builder()
                        .message(message.as_str())
                        .modal(true)
                        .build()
                        .show(Some(&self.window));
                }
                false
            }
        }
    }

    /// 自测：像用户那样改一项（操作对应的控件，走同样的信号）。`value` 按设置文件里存的值写：开关为 true/false
    /// （翻过来说的开关在这里换成开关的状态），下拉框为选项 ID，数字和路径为文字，颜色为 #RRGGBB。
    /// 返回控件是否找到并且接受了这个值。
    pub fn set_for_test(self: &Rc<Self>, key: &str, value: &str) -> bool {
        // 快捷键的一行：`keyboard.mappings#行=on|按键|动作`（按键写法同 `Ctrl+K`，可省略后两段）或 `=off`。
        // 映射表里这一行还没用过时，先像"添加快捷键"那样把它加进列表。
        if let Some((base, index)) = key.split_once('#') {
            let Ok(index) = index.parse::<usize>() else {
                return false;
            };
            let listed = |w: &Self| {
                w.rows.borrow().iter().any(|r| {
                    r.key == base
                        && matches!(&r.control, RowControl::KeyMappings(widgets, ..)
                            if widgets.iter().any(|k| k.row == index))
                })
            };
            if !listed(self.as_ref()) {
                if !self.set(base, Input::KeyRowEnabled { row: index, on: true }) {
                    return false;
                }
                let page = self.current_page();
                self.show_page(&page);
                if !listed(self.as_ref()) {
                    return false;
                }
            }
            let (switch, dropdown, ids) = {
                let rows = self.rows.borrow();
                let Some(RowControl::KeyMappings(widgets, ids)) = rows
                    .iter()
                    .find(|r| r.key == base && r.container.is_sensitive())
                    .map(|r| &r.control)
                else {
                    return false;
                };
                let Some(w) = widgets.iter().find(|w| w.row == index) else {
                    return false;
                };
                (w.switch.clone(), w.dropdown.clone(), ids.clone())
            };
            let mut parts = value.split('|');
            match parts.next() {
                Some("on") => switch.set_active(true),
                Some("off") => switch.set_active(false),
                _ => return false,
            }
            if let Some(spec) = parts.next() {
                let Some(chord) = Chord::parse(spec) else {
                    return false;
                };
                self.record_key(index, chord.key.clone(), chord.mods);
            }
            if let Some(action) = parts.next() {
                let Some(position) = ids.iter().position(|id| id == action) else {
                    return false;
                };
                dropdown.set_selected(position as u32);
            }
            return true;
        }
        let rows = self.rows.borrow();
        let Some(row) = rows.iter().find(|r| r.key == key) else {
            return false;
        };
        if !row.container.is_sensitive() {
            return false;
        }
        match &row.control {
            RowControl::Toggle(switch) => match value {
                "true" | "false" => switch.set_active((value == "true") != row.inverted),
                _ => return false,
            },
            RowControl::Choice(dropdown, ids) => match ids.iter().position(|id| id == value) {
                Some(index) => dropdown.set_selected(index as u32),
                None => return false,
            },
            RowControl::Number(entry) | RowControl::Path(entry) => entry.set_text(value),
            RowControl::Color(..) => {
                let Some(color) = anyview_core::settings::parse_color(value) else {
                    return false;
                };
                drop(rows);
                self.set_color(key, color);
            }
            // 鼠标映射：`on|动作`（动作为空时是"无操作"）或 `off`（下拉框的"默认"）。
            RowControl::Mapping(dropdown, ids) => {
                let mut parts = value.split('|');
                let id = match (parts.next(), parts.next()) {
                    (Some("off"), _) => anyview_core::settings::MAPPING_DEFAULT,
                    (Some("on"), action) => action.unwrap_or(""),
                    _ => return false,
                };
                let Some(position) = ids.iter().position(|i| i == id) else {
                    return false;
                };
                let dropdown = dropdown.clone();
                drop(rows);
                dropdown.set_selected(position as u32);
            }
            RowControl::KeyMappings(..) | RowControl::Other => return false,
        }
        true
    }

    /// 放弃修改并关窗口（"取消"、询问后选了"放弃修改"；自测也用它）。
    pub fn discard(&self) {
        self.closing.set(true);
        self.window.close();
    }

    /// 关闭窗口（×、Esc）：有没确定的修改就先询问；返回窗口是否已经关掉。
    fn close_requested(self: &Rc<Self>) -> bool {
        let Some(ui) = self.ui() else { return true };
        if self.closing.get() || !ui.core.settings_dirty() {
            ui.core.settings_cancel();
            return true;
        }
        let dialog = gtk::AlertDialog::builder()
            .message(ui.text(Key::SettingsDiscardMessage))
            .buttons([
                ui.text(Key::SettingsKeepEditing),
                ui.text(Key::SettingsDiscard),
            ])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |choice| {
            if let (Some(w), Ok(1)) = (weak.upgrade(), choice) {
                w.closing.set(true);
                w.window.close();
            }
        });
        false
    }
}

/// 快捷键一行跟着草稿更新：开关、按键、动作、提示，以及关掉时按键和动作灰显。
fn update_key_row(ui: &Rc<Ui>, w: &KeyRow, r: &KeyMappingView, ids: &[String]) {
    w.switch.set_active(r.enabled);
    if !w.capturing.get() {
        if r.key_label.is_empty() {
            w.key.set_label(ui.text(Key::SettingsKeyUnset));
        } else {
            w.key.set_label(&r.key_label);
        }
    }
    w.note.set_text(&r.note);
    w.note.set_visible(!r.note.is_empty());
    if let Some(index) = ids.iter().position(|id| *id == r.action)
        && w.dropdown.selected() != index as u32
    {
        w.dropdown.set_selected(index as u32);
    }
    w.key.set_sensitive(r.enabled);
    w.more.set_sensitive(r.enabled);
    w.dropdown.set_sensitive(r.enabled);
    w.container.remove_css_class("anyview-invalid-row");
}

/// 能换行的标签（中文没有空格，按字换行），靠左。
fn wrapping_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_natural_wrap_mode(gtk::NaturalWrapMode::Word);
    label
}

/// 下拉框和各项的 ID。
fn dropdown(options: &[OptionView], selected: &str, width: i32) -> (gtk::DropDown, Vec<String>) {
    let titles: Vec<&str> = options.iter().map(|(_, t)| t.as_str()).collect();
    let ids: Vec<String> = options.iter().map(|(id, _)| id.clone()).collect();
    let dropdown = gtk::DropDown::from_strings(&titles);
    dropdown.set_width_request(width);
    dropdown.set_valign(gtk::Align::Center);
    if let Some(index) = ids.iter().position(|id| id == selected) {
        dropdown.set_selected(index as u32);
    }
    (dropdown, ids)
}

fn rgba(color: u32) -> gdk::RGBA {
    let channel = |shift: u32| ((color >> shift) & 0xFF) as f32 / 255.0;
    gdk::RGBA::new(channel(16), channel(8), channel(0), 1.0)
}

/// 一个圆形色块（带细边，浅色在浅色背景上也看得出）；是当前颜色时外面画一圈（颜色同文字）。
fn color_swatch(color: u32, current: Rc<Cell<u32>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(22);
    area.set_content_height(22);
    area.set_draw_func(move |area, cr, w, h| {
        let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
        let c = rgba(color);
        cr.arc(cx, cy, 7.5, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(c.red().into(), c.green().into(), c.blue().into());
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.45);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        if current.get() == color {
            let fg = area.color();
            cr.set_source_rgba(fg.red().into(), fg.green().into(), fg.blue().into(), 0.9);
            cr.set_line_width(2.0);
            cr.arc(cx, cy, 10.0, 0.0, std::f64::consts::TAU);
            let _ = cr.stroke();
        }
    });
    area
}

/// 打开设置窗口；已经打开时放到最前面（草稿保留）。
pub fn show(ui: &Rc<Ui>) -> Rc<SettingsWindow> {
    if let Some(existing) = ui.settings_window.borrow().as_ref() {
        existing.window.present();
        return Rc::clone(existing);
    }
    ui.core.settings_begin();
    let lang = ui.lang();
    let window = gtk::Window::builder()
        .title(ui.text(Key::SettingsTitle))
        .transient_for(&ui.window)
        .default_width(860)
        .default_height(680)
        .build();
    window.add_css_class("anyview-settings");

    // 左边：带图标的分类列表。
    let pages = ui.core.settings_pages(lang);
    let sidebar = gtk::ListBox::new();
    sidebar.set_selection_mode(gtk::SelectionMode::Single);
    sidebar.add_css_class("navigation-sidebar");
    for (id, title) in &pages {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.append(&gtk::Image::from_icon_name(page_icon(id)));
        let label = gtk::Label::new(Some(title));
        label.set_xalign(0.0);
        line.append(&label);
        sidebar.append(&line);
    }
    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.add_css_class("anyview-settings-sidebar");
    left.set_width_request(196);
    left.append(&sidebar);

    // 右边：可以滚动的页，下面是按钮。
    let page_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page_box.add_css_class("anyview-settings-page");
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&page_box)
        .build();
    let reset = gtk::Button::with_label(ui.text(Key::SettingsReset));
    reset.add_css_class("flat");
    let cancel = gtk::Button::with_label(ui.text(Key::ButtonCancel));
    let ok = gtk::Button::with_label(ui.text(Key::SettingsOk));
    ok.add_css_class("suggested-action");
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    footer.add_css_class("anyview-settings-footer");
    footer.append(&reset);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&ok);
    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.set_hexpand(true);
    right.append(&scroller);
    right.append(&footer);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content.append(&left);
    content.append(&right);
    window.set_child(Some(&content));

    let settings = Rc::new(SettingsWindow {
        window,
        ui: Rc::downgrade(ui),
        sidebar,
        page_ids: pages.iter().map(|(id, _)| id.to_string()).collect(),
        scroller,
        page_box,
        current: RefCell::new(String::new()),
        rows: RefCell::new(Vec::new()),
        updating: Cell::new(false),
        closing: Cell::new(false),
        rebuild_pending: Cell::new(false),
        capture_after_rebuild: Cell::new(None),
        last_report: RefCell::new(None),
    });

    let weak = Rc::downgrade(&settings);
    settings.sidebar.connect_row_selected(move |_, row| {
        if let (Some(w), Some(row)) = (weak.upgrade(), row) {
            let id = w.page_ids.get(row.index() as usize).cloned();
            if let Some(id) = id
                && id != w.current_page()
            {
                w.show_page(&id);
            }
        }
    });
    let weak = Rc::downgrade(&settings);
    reset.connect_clicked(move |_| {
        if let Some(w) = weak.upgrade() {
            w.reset();
        }
    });
    let weak = Rc::downgrade(&settings);
    cancel.connect_clicked(move |_| {
        if let Some(w) = weak.upgrade() {
            w.discard();
        }
    });
    let weak = Rc::downgrade(&settings);
    ok.connect_clicked(move |_| {
        if let Some(w) = weak.upgrade() {
            w.ok();
        }
    });
    // Esc 关闭设置窗口，不退出程序。
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(&settings);
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape
            && let Some(w) = weak.upgrade()
        {
            w.window.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    settings.window.add_controller(keys);
    let weak = Rc::downgrade(&settings);
    let ui_weak = Rc::downgrade(ui);
    settings.window.connect_close_request(move |_| {
        let Some(w) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if w.close_requested() {
            if let Some(ui) = ui_weak.upgrade() {
                ui.settings_window.borrow_mut().take();
            }
            glib::Propagation::Proceed
        } else {
            glib::Propagation::Stop
        }
    });

    if let Some(first) = settings.page_ids.first().cloned() {
        settings.select_page(&first);
    }
    *ui.settings_window.borrow_mut() = Some(Rc::clone(&settings));
    settings.window.present();
    settings
}

/// 登记、取消登记缩略图生成器没做成时的说明（文字在核心）。
fn thumbnailer_problem(problem: crate::flatpak_thumbnailer::Problem, lang: anyview_core::Lang) -> String {
    use crate::flatpak_thumbnailer::Problem;
    match problem {
        Problem::NotFlatpak => anyview_core::i18n::text(Key::SettingsThumbnailerNotFlatpak, lang).to_string(),
        Problem::NotOurs(path) => anyview_core::i18n::text(Key::SettingsThumbnailerNotOurs, lang)
            .replace("{path}", &path.display().to_string()),
        Problem::Io(message) => {
            anyview_core::i18n::text(Key::SettingsThumbnailerFailed, lang).replace("{message}", &message)
        }
    }
}
