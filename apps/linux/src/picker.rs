//! 选择图片窗口。
//!
//! 上面是缩略图导航（所选的一张在中间、放大、加框），下面是列表（名称、修改日期、大小），底部是排序按钮、
//! "隐藏缩略图导航"和"确定"。单击、方向键只移动选择，主窗口不切换；双击、回车或"确定"切到所选图片并关掉
//! 这个窗口；Esc 或关闭按钮不切换。列表内容、文字和缩略图都由核心给出（[`anyview_core::picker`]）。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use anyview_core::DecodedImage;
use anyview_core::actions;
use anyview_core::i18n::Key;
use anyview_core::picker::Row;
use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};

use crate::window::Ui;

/// 缩略图带的高度和格子（逻辑像素）：普通格子放 64x48 的缩略图，所选的放 96x72，外加 3 点的框。
const STRIP_HEIGHT: i32 = 112;
const SMALL: (f64, f64) = (64.0, 48.0);
const BIG: (f64, f64) = (96.0, 72.0);
const GAP: f64 = 8.0;
const FRAME: f64 = 3.0;
/// 所选那一格的框色（高亮橙色）。
const FRAME_RGB: (f64, f64, f64) = (1.0, 0.6, 0.1);

enum Thumb {
    Ready(cairo::ImageSurface),
    Failed,
}

pub struct Picker {
    pub window: gtk::Window,
    ui: Weak<Ui>,
    strip: gtk::DrawingArea,
    view: gtk::ColumnView,
    selection: gtk::SingleSelection,
    model: gtk::StringList,
    rows: Rc<RefCell<Vec<Row>>>,
    sort_button: gtk::MenuButton,
    hide: gtk::CheckButton,
    thumbs: RefCell<HashMap<String, Thumb>>,
    /// 缩略图带上现在画出来的格子：(序号, 左边 x, 宽度)，点击时找是哪一张。
    cells: RefCell<Vec<(usize, f64, f64)>>,
    /// 程序自己在改选择（重新载入列表），这时不当作用户操作。
    updating: Cell<bool>,
}

impl Picker {
    fn ui(&self) -> Option<Rc<Ui>> {
        self.ui.upgrade()
    }

    /// 选中的行。
    pub fn selected(&self) -> Option<usize> {
        let position = self.selection.selected();
        (position != gtk::INVALID_LIST_POSITION).then_some(position as usize)
    }

    pub fn row_count(&self) -> usize {
        self.rows.borrow().len()
    }

    pub fn row(&self, index: usize) -> Option<Row> {
        self.rows.borrow().get(index).cloned()
    }

    pub fn sort_label(&self) -> String {
        self.sort_button.label().map(|l| l.to_string()).unwrap_or_default()
    }

    pub fn strip_visible(&self) -> bool {
        self.strip.is_visible()
    }

    /// 缩略图带上看得见的几张里，已经有缩略图的张数和总张数（自测用）。
    pub fn thumbnails_loaded(&self) -> (usize, usize) {
        let cells = self.cells.borrow();
        let rows = self.rows.borrow();
        let thumbs = self.thumbs.borrow();
        let ready = cells
            .iter()
            .filter(|(i, _, _)| {
                rows.get(*i)
                    .is_some_and(|r| matches!(thumbs.get(&r.path), Some(Thumb::Ready(_))))
            })
            .count();
        (ready, cells.len())
    }

    /// 选中第 `index` 行并滚到能看见（单击、方向键的效果）。
    pub fn select(&self, index: usize) {
        if index >= self.row_count() {
            return;
        }
        self.selection.set_selected(index as u32);
        self.view.scroll_to(index as u32, None, gtk::ListScrollFlags::NONE, None);
    }

    /// 从核心重新取列表（打开时、排序后、文件夹变了）。原来选中的文件还在就继续选中它，否则选中当前图片。
    pub fn reload(&self) {
        let Some(ui) = self.ui() else { return };
        let Some(list) = ui.core.picker_list(ui.lang()) else {
            return;
        };
        let keep = self
            .selected()
            .and_then(|i| self.rows.borrow().get(i).map(|r| r.path.clone()));
        let target = keep
            .and_then(|path| list.rows.iter().position(|r| r.path == path))
            .or(list.current)
            .unwrap_or(0);
        let count = list.rows.len();
        *self.rows.borrow_mut() = list.rows;
        // 文件夹变了：文件可能换过内容，画好的缩略图不留（核心按修改时间认，会给新的）。
        self.thumbs.borrow_mut().retain(|_, thumb| matches!(thumb, Thumb::Failed));
        self.sort_button.set_label(&list.sort_label);
        self.updating.set(true);
        let ids: Vec<String> = (0..count).map(|i| i.to_string()).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.model.splice(0, self.model.n_items(), &ids);
        self.updating.set(false);
        self.select(target);
        self.refresh_strip();
    }

    /// 缩略图好了（核心的 `Thumbnail` 事件）。
    pub fn on_thumbnail(&self, path: &str, image: Option<&Arc<DecodedImage>>) {
        let thumb = match image.and_then(|i| surface_from(i)) {
            Some(surface) => Thumb::Ready(surface),
            None => Thumb::Failed,
        };
        self.thumbs.borrow_mut().insert(path.to_string(), thumb);
        self.strip.queue_draw();
    }

    /// "确定"、双击、回车：切到所选图片，关掉窗口。
    pub fn choose(&self) {
        if let (Some(ui), Some(index)) = (self.ui(), self.selected()) {
            ui.core.picker_choose(index);
        }
        self.window.close();
    }

    pub fn set_hide_thumbnails(&self, hide: bool) {
        self.hide.set_active(hide);
    }

    /// 按缩略图带的宽度算出看得见的格子，向核心要还没有的缩略图，重画。
    fn refresh_strip(&self) {
        let cells = self.layout_cells(f64::from(self.strip.width().max(1)));
        let paths: Vec<String> = {
            let rows = self.rows.borrow();
            cells
                .iter()
                .filter_map(|(i, _, _)| rows.get(*i).map(|r| r.path.clone()))
                .collect()
        };
        *self.cells.borrow_mut() = cells;
        // 只留看得见的那几张（每张约 150 KB；不然在几千张的文件夹里一路翻过去，看过的全留在内存里）。
        // 核心另有一份有上限的缓存，翻回来时从那里取。生成失败的只是个记号，留着（核心不会再报一次）。
        self.thumbs
            .borrow_mut()
            .retain(|path, thumb| matches!(thumb, Thumb::Failed) || paths.contains(path));
        if let Some(ui) = self.ui()
            && self.strip.is_visible()
        {
            // 已经生成好的直接取；其余交给核心在后台生成。
            let mut missing = Vec::new();
            for path in paths {
                if self.thumbs.borrow().contains_key(&path) {
                    continue;
                }
                match ui.core.picker_thumbnail(&path) {
                    Some(image) => self.on_thumbnail(&path, Some(&image)),
                    None => missing.push(path),
                }
            }
            ui.core.picker_want_thumbnails(missing);
        }
        self.strip.queue_draw();
    }

    /// 所选的一张在正中间，两边依次排开，排到边上为止。
    fn layout_cells(&self, width: f64) -> Vec<(usize, f64, f64)> {
        let count = self.row_count();
        let Some(center) = self.selected().filter(|&i| i < count) else {
            return Vec::new();
        };
        let big = BIG.0 + 2.0 * FRAME;
        let mut cells = vec![(center, (width - big) / 2.0, big)];
        let mut x = (width - big) / 2.0;
        let mut i = center;
        while i > 0 && x > 0.0 {
            i -= 1;
            x -= GAP + SMALL.0;
            cells.push((i, x, SMALL.0));
        }
        let mut x = (width + big) / 2.0;
        let mut i = center;
        while i + 1 < count && x < width {
            i += 1;
            cells.push((i, x + GAP, SMALL.0));
            x += GAP + SMALL.0;
        }
        cells.sort_by_key(|c| c.0);
        cells
    }

    fn draw_strip(&self, cr: &cairo::Context, width: i32, height: i32) {
        cr.set_source_rgb(0.0, 0.0, 0.0);
        let _ = cr.paint();
        let selected = self.selected();
        let rows = self.rows.borrow();
        let thumbs = self.thumbs.borrow();
        let mid = f64::from(height) / 2.0;
        for &(index, x, w) in self.cells.borrow().iter() {
            let is_selected = Some(index) == selected;
            let (bw, bh) = if is_selected { BIG } else { SMALL };
            let bx = if is_selected { x + FRAME } else { x };
            let by = mid - bh / 2.0;
            if is_selected {
                cr.set_source_rgb(FRAME_RGB.0, FRAME_RGB.1, FRAME_RGB.2);
                cr.rectangle(x, by - FRAME, w, bh + 2.0 * FRAME);
                let _ = cr.fill();
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.rectangle(bx, by, bw, bh);
                let _ = cr.fill();
            }
            let thumb = rows.get(index).and_then(|r| thumbs.get(&r.path));
            match thumb {
                Some(Thumb::Ready(surface)) => {
                    let (sw, sh) = (f64::from(surface.width()), f64::from(surface.height()));
                    // 按比例放进格子（很小的图也放大到格子大小，方便看清）。
                    let scale = (bw / sw).min(bh / sh);
                    let (dw, dh) = (sw * scale, sh * scale);
                    let _ = cr.save();
                    cr.translate(bx + (bw - dw) / 2.0, by + (bh - dh) / 2.0);
                    cr.scale(scale, scale);
                    let _ = cr.set_source_surface(surface, 0.0, 0.0);
                    cr.source().set_filter(cairo::Filter::Good);
                    let _ = cr.paint();
                    let _ = cr.restore();
                }
                // 还没生成好：深灰的格子；生成失败：更暗的格子。
                Some(Thumb::Failed) => {
                    cr.set_source_rgb(0.12, 0.12, 0.12);
                    cr.rectangle(bx, by, bw, bh);
                    let _ = cr.fill();
                }
                None => {
                    cr.set_source_rgb(0.2, 0.2, 0.2);
                    cr.rectangle(bx, by, bw, bh);
                    let _ = cr.fill();
                }
            }
        }
        let _ = width;
    }

    fn cell_at(&self, x: f64) -> Option<usize> {
        self.cells
            .borrow()
            .iter()
            .find(|(_, left, w)| x >= *left && x < left + w)
            .map(|c| c.0)
    }
}

/// 缩略图（BGRA 预乘）→ cairo 的 ARGB32 表面（小端机器上内存排列相同）。
fn surface_from(image: &DecodedImage) -> Option<cairo::ImageSurface> {
    let stride = cairo::Format::ARgb32.stride_for_width(image.width).ok()? as usize;
    let row = image.width as usize * 4;
    let mut data = vec![0u8; stride * image.height as usize];
    for y in 0..image.height as usize {
        let src = image.pixels.get(y * image.stride..y * image.stride + row)?;
        data[y * stride..y * stride + row].copy_from_slice(src);
    }
    cairo::ImageSurface::create_for_data(
        data,
        cairo::Format::ARgb32,
        image.width as i32,
        image.height as i32,
        stride as i32,
    )
    .ok()
}

/// 打开选择图片窗口；已经打开时放到最前面。
pub fn show(ui: &Rc<Ui>) -> Rc<Picker> {
    if let Some(existing) = ui.picker.borrow().as_ref() {
        existing.window.present();
        return Rc::clone(existing);
    }
    let window = gtk::Window::builder()
        .title(ui.text(Key::PickerTitle))
        .transient_for(&ui.window)
        .destroy_with_parent(true)
        .default_width(760)
        .default_height(560)
        .build();

    let strip = gtk::DrawingArea::new();
    strip.set_content_height(STRIP_HEIGHT);
    strip.set_hexpand(true);

    let rows: Rc<RefCell<Vec<Row>>> = Rc::new(RefCell::new(Vec::new()));
    let model = gtk::StringList::new(&[]);
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    let view = gtk::ColumnView::new(Some(selection.clone()));
    view.set_show_column_separators(true);
    let columns = [
        (Key::PickerColumnName, 0usize),
        (Key::PickerColumnModified, 1),
        (Key::PickerColumnSize, 2),
    ];
    for (title, field) in columns {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(move |_, item| {
            let label = gtk::Label::new(None);
            label.set_xalign(if field == 2 { 1.0 } else { 0.0 });
            label.set_margin_start(4);
            label.set_margin_end(8);
            if field == 0 {
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            }
            if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
                item.set_child(Some(&label));
            }
        });
        let rows_for_bind = Rc::clone(&rows);
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let index = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .and_then(|s| s.string().parse::<usize>().ok());
            let (Some(index), Some(label)) = (index, item.child().and_downcast::<gtk::Label>())
            else {
                return;
            };
            let rows = rows_for_bind.borrow();
            let text = rows.get(index).map(|r| match field {
                0 => r.name.as_str(),
                1 => r.modified.as_str(),
                _ => r.size.as_str(),
            });
            label.set_text(text.unwrap_or(""));
        });
        let column = gtk::ColumnViewColumn::new(Some(ui.text(title)), Some(factory));
        match field {
            0 => column.set_expand(true),
            1 => column.set_fixed_width(170),
            _ => column.set_fixed_width(90),
        }
        view.append_column(&column);
    }
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&view)
        .build();
    scroller.set_has_frame(true);

    // 底部：排序按钮（显示现在的排序）、"隐藏缩略图导航"、"确定"。
    let sort_button = gtk::MenuButton::new();
    sort_button.set_width_request(180);
    let hide = gtk::CheckButton::with_label(ui.text(Key::PickerHideThumbnails));
    let ok = gtk::Button::with_label(ui.text(Key::PickerOk));
    ok.set_width_request(96);
    ok.add_css_class("suggested-action");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bottom.append(&sort_button);
    bottom.append(&hide);
    bottom.append(&spacer);
    bottom.append(&ok);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.append(&strip);
    content.append(&scroller);
    content.append(&bottom);
    window.set_child(Some(&content));

    let picker = Rc::new(Picker {
        window,
        ui: Rc::downgrade(ui),
        strip,
        view,
        selection,
        model,
        rows,
        sort_button,
        hide,
        thumbs: RefCell::new(HashMap::new()),
        cells: RefCell::new(Vec::new()),
        updating: Cell::new(false),
    });

    // 排序菜单：与右键菜单"排序方式"是同一组动作，改了以后核心发 Collection 事件，列表重新载入。
    install_sort_menu(ui, &picker);

    let weak = Rc::downgrade(&picker);
    picker.strip.set_draw_func(move |_, cr, width, height| {
        if let Some(p) = weak.upgrade() {
            p.draw_strip(cr, width, height);
        }
    });
    let weak = Rc::downgrade(&picker);
    picker.strip.connect_resize(move |_, _, _| {
        if let Some(p) = weak.upgrade() {
            p.refresh_strip();
        }
    });
    // 单击缩略图只选中（这一格移到正中）；双击打开第一下选中的那一张：第一下以后格子重排了，
    // 第二下落在别的格子上。
    let click = gtk::GestureClick::new();
    let weak = Rc::downgrade(&picker);
    // 上一次单击点到了格子：双击的第二下照样打开那一张（第二下可能落在重排后的别的格子或空白处）。
    let pressed_cell = std::cell::Cell::new(false);
    click.connect_pressed(move |_, n_press, x, _| {
        let Some(p) = weak.upgrade() else { return };
        if n_press >= 2 {
            if pressed_cell.get() {
                p.choose();
            }
            return;
        }
        match p.cell_at(x) {
            Some(index) => {
                pressed_cell.set(true);
                p.select(index);
            }
            None => pressed_cell.set(false),
        }
    });
    picker.strip.add_controller(click);
    // 在缩略图带上滚动滚轮：一格移一张。
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
    );
    let weak = Rc::downgrade(&picker);
    scroll.connect_scroll(move |_, dx, dy| {
        let Some(p) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let step = if dy.abs() >= dx.abs() { dy } else { dx };
        if let Some(i) = p.selected() {
            let target = if step > 0.0 { i + 1 } else { i.saturating_sub(1) };
            p.select(target.min(p.row_count().saturating_sub(1)));
        }
        glib::Propagation::Stop
    });
    picker.strip.add_controller(scroll);

    let weak = Rc::downgrade(&picker);
    picker.selection.connect_selected_notify(move |_| {
        if let Some(p) = weak.upgrade()
            && !p.updating.get()
        {
            p.refresh_strip();
        }
    });
    let weak = Rc::downgrade(&picker);
    picker.view.connect_activate(move |_, position| {
        if let Some(p) = weak.upgrade() {
            p.select(position as usize);
            p.choose();
        }
    });
    let weak = Rc::downgrade(&picker);
    ok.connect_clicked(move |_| {
        if let Some(p) = weak.upgrade() {
            p.choose();
        }
    });
    let weak = Rc::downgrade(&picker);
    picker.hide.connect_toggled(move |check| {
        if let Some(p) = weak.upgrade() {
            let hide = check.is_active();
            p.strip.set_visible(!hide);
            if let Some(ui) = p.ui() {
                ui.core.set_picker_hides_thumbnails(hide);
            }
            p.refresh_strip();
        }
    });
    // Esc 关窗口，不切换。
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(&picker);
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape
            && let Some(p) = weak.upgrade()
        {
            p.window.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    picker.window.add_controller(keys);
    let ui_weak = Rc::downgrade(ui);
    picker.window.connect_close_request(move |_| {
        if let Some(ui) = ui_weak.upgrade() {
            ui.picker.borrow_mut().take();
            // 不再要缩略图了。
            ui.core.picker_want_thumbnails(Vec::new());
        }
        glib::Propagation::Proceed
    });

    let hidden = ui.core.picker_hides_thumbnails();
    picker.hide.set_active(hidden);
    picker.strip.set_visible(!hidden);
    picker.reload();
    *ui.picker.borrow_mut() = Some(Rc::clone(&picker));
    picker.window.present();
    picker.view.grab_focus();
    picker
}

/// 排序按钮的菜单：文件名、修改日期、大小、类型（单选）和降序，文字取自核心的动作目录。
fn install_sort_menu(ui: &Rc<Ui>, picker: &Rc<Picker>) {
    let group = gio::SimpleActionGroup::new();
    let ids = ["sort_name", "sort_mtime", "sort_size", "sort_type"];
    let checked = ids
        .iter()
        .find(|id| ui.core.action_state(id).checked)
        .copied()
        .unwrap_or("sort_name");
    let sort = gio::SimpleAction::new_stateful(
        "sort",
        Some(glib::VariantTy::STRING),
        &checked.to_variant(),
    );
    let ui_weak = Rc::downgrade(ui);
    sort.connect_activate(move |action, target| {
        let (Some(ui), Some(id)) = (ui_weak.upgrade(), target.and_then(|t| t.get::<String>()))
        else {
            return;
        };
        action.set_state(&id.to_variant());
        ui.invoke(&id);
    });
    group.add_action(&sort);
    let desc = gio::SimpleAction::new_stateful(
        "desc",
        None,
        &ui.core.action_state("sort_desc").checked.to_variant(),
    );
    let ui_weak = Rc::downgrade(ui);
    desc.connect_activate(move |action, _| {
        let Some(ui) = ui_weak.upgrade() else { return };
        ui.invoke("sort_desc");
        action.set_state(&ui.core.action_state("sort_desc").checked.to_variant());
    });
    group.add_action(&desc);
    picker.window.insert_action_group("picker", Some(&group));

    let menu = gio::Menu::new();
    let platform = anyview_core::Platform::current();
    for id in ids {
        if let Some(def) = actions::find(id) {
            let item = gio::MenuItem::new(Some(def.title(platform, ui.lang())), None);
            item.set_action_and_target_value(Some("picker.sort"), Some(&id.to_variant()));
            menu.append_item(&item);
        }
    }
    let section = gio::Menu::new();
    if let Some(def) = actions::find("sort_desc") {
        section.append(Some(def.title(platform, ui.lang())), Some("picker.desc"));
    }
    menu.append_section(None, &section);
    picker.sort_button.set_menu_model(Some(&menu));
}

/// 排序菜单里的一项（自测用）：与用户在菜单里点它相同。
pub fn activate_sort(picker: &Picker, id: &str) {
    let target = if id == "sort_desc" { None } else { Some(id.to_variant()) };
    let name = if id == "sort_desc" { "picker.desc" } else { "picker.sort" };
    let _ = picker
        .window
        .activate_action(name, target.as_ref());
}
