//! 主窗口：空态、画布、右上角通知、中央倍率提示，以及键盘、鼠标、拖放、对话框。
//! 按键和菜单都交给核心解析、执行；这里只把结果落到 GTK 上。

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyview_core::actions::{ALT, CMD, CTRL, SHIFT};
use anyview_core::settings::{MouseBinding, MouseInput};
use anyview_core::i18n::{self, Key};
use anyview_core::{
    ActionOutcome, AppCore, Command, CoreListener, Event, Lang, Navigation, NoticeKind, Platform,
    SettingsApplied, SwipePhase, UiRequest, UiSettings,
};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::window_state;

use crate::LaunchOptions;
use crate::canvas::ImageCanvas;
use crate::menu;
use crate::overlays::{self, Overlays};
use crate::selftest;
use crate::settings_window::{self, SettingsWindow};

/// 鼠标按键要做的事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseTarget {
    Action(&'static str),
    ContextMenu,
    Nothing,
}

/// 核心在后台线程唤醒；通过通道交给 GTK 主线程上的异步任务处理。
struct ChannelListener(async_channel::Sender<()>);

impl CoreListener for ChannelListener {
    fn wake(&self) {
        let _ = self.0.try_send(());
    }
}

/// 启动时提前建好的核心：命令行给了图片时，建窗口之前就开始读图。
/// 核心送来的事件留在核心的队列里，窗口建好、主循环开始后再取出。
pub struct EarlyCore {
    core: Arc<AppCore>,
    wakes: async_channel::Receiver<()>,
    /// 提前打开的时刻（自测从这时开始计时）。
    opened_at: Option<Instant>,
}

impl EarlyCore {
    /// 建核心、读设置。`settings`：设置文件（自测用自己的文件）；为空时用默认位置。
    /// 先读设置：EXIF 方向、ICC、快速预览等影响第一张图的解码；"不允许多个实例"决定要不要开窗口。
    pub fn new(settings: Option<&Path>) -> Self {
        let (tx, wakes) = async_channel::unbounded::<()>();
        let core = Arc::new(AppCore::new(Arc::new(ChannelListener(tx))));
        core.load_settings(settings.map(Path::to_path_buf));
        Self {
            core,
            wakes,
            opened_at: None,
        }
    }

    /// 设置"不允许多个实例"。
    pub fn reuse_window(&self) -> bool {
        self.core.ui_settings().reuse_window
    }

    /// 开始读命令行给的图片（建窗口之前就开始）。
    pub fn open(&mut self, path: &Path) {
        self.core.dispatch(Command::Open {
            path: path.display().to_string(),
        });
        self.opened_at = Some(Instant::now());
    }
}

pub struct Ui {
    pub core: Arc<AppCore>,
    pub window: gtk::ApplicationWindow,
    pub canvas: ImageCanvas,
    /// 界面语言：设置里选的，或跟随系统；改了设置后立即换。
    lang: Cell<Lang>,
    pub actions: gio::SimpleActionGroup,
    pub menu_model: RefCell<gio::Menu>,
    /// 界面相关的设置（背景色、隐藏控件等），设置生效后重新读。
    pub ui_settings: Cell<UiSettings>,
    /// 背景色（设置里可改）的样式。
    background_css: gtk::CssProvider,
    hint: gtk::Label,
    /// 打开着的设置窗口。
    pub settings_window: RefCell<Option<Rc<SettingsWindow>>>,
    /// 打开着的选择图片窗口。
    pub picker: RefCell<Option<Rc<crate::picker::Picker>>>,
    /// 打开着的帮助窗口。
    pub help_windows: RefCell<Vec<gtk::Window>>,
    /// 最近一次"确定"校验失败的提示（自测记录用）。
    pub settings_error: RefCell<Option<String>>,
    /// 控制栏、边缘箭头、左上角图片信息。
    pub overlays: Overlays,
    empty: gtk::Box,
    status: gtk::Label,
    notice: gtk::Label,
    notice_serial: Rc<Cell<u64>>,
    zoom_label: gtk::Label,
    zoom_serial: Rc<Cell<u64>>,
    /// 指针在画布上的位置（逻辑像素），作为滚轮缩放的中心。
    pointer: Cell<(f64, f64)>,
    /// 高精度滚轮的累计量，满一格翻一页。
    wheel: Cell<f64>,
    generation: Cell<u64>,
    name: RefCell<String>,
    index: Cell<Option<usize>>,
    total: Cell<Option<usize>>,
    ping_sent: Cell<Option<Instant>>,
    pub ping_ms: Cell<Option<f64>>,
    pub selftest: RefCell<Option<selftest::Runner>>,
    /// 切到了新图片：图像送达时按图片调整一次窗口大小。
    fit_pending: Cell<bool>,
    /// 最近一次按图片调整的窗口大小（逻辑像素，自测记录用）。
    pub last_fit: Cell<Option<(i32, i32)>>,
    /// 切图开始的时刻（运行时性能记录）。
    show_started: Cell<Option<Instant>>,
    /// 阻止屏保：向桌面申请"不要空闲"时拿到的号；没有申请时为 0。
    inhibit_cookie: Cell<u32>,
    /// 这一代次的切图要做的翻页效果：`Current` 时向核心要，图像送达时开始。
    pending_turn: Cell<Option<(u64, anyview_core::transition::Transition)>>,
    /// 翻页音效。
    page_sound: crate::sound::PageSound,
    /// 放过几次翻页音效、用的哪种方式（自测记录；自测时不真的放）。
    pub sound_log: RefCell<Vec<String>>,
}

impl Ui {
    pub fn lang(&self) -> Lang {
        self.lang.get()
    }

    pub fn text(&self, key: Key) -> &'static str {
        i18n::text(key, self.lang())
    }

    /// 让界面相关的设置生效：背景色、透明背景、隐藏的控件、提示、字体、通知位置、界面语言。
    /// 设置"看图时阻止屏幕保护程序和显示器睡眠"：打开、窗口里有图片、窗口看得见时向桌面申请不要空闲，
    /// 否则撤销。看不看得见听桌面的（窗口的"挂起"状态：最小化、在别的工作区、整个被盖住；X11 上没有这个状态，
    /// 一直算看得见）。
    pub fn update_inhibit(&self) {
        let wanted = self.ui_settings.get().no_screensaver
            && self.core.current_path().is_some()
            && !self.window.is_suspended();
        let cookie = self.inhibit_cookie.get();
        let Some(app) = self.window.application() else {
            return;
        };
        if wanted && cookie == 0 {
            let cookie = app.inhibit(
                Some(&self.window),
                gtk::ApplicationInhibitFlags::IDLE,
                Some("Anyview"),
            );
            self.inhibit_cookie.set(cookie);
        } else if !wanted && cookie != 0 {
            app.uninhibit(cookie);
            self.inhibit_cookie.set(0);
        }
    }

    /// 自检用：现在有没有在阻止屏保。
    pub fn inhibiting(&self) -> bool {
        self.inhibit_cookie.get() != 0
    }

    pub fn apply_ui_settings(self: &Rc<Self>) {
        let settings = self.core.ui_settings();
        self.ui_settings.set(settings);
        self.background_css.load_from_string(&format!(
            "window.anyview-main {{ background-color: {}; }}",
            anyview_core::settings::color_text(settings.background_color)
        ));
        self.canvas.refresh_alpha_fill();
        // 背景图片：解码好了才有；在后台解码时先用背景色，好了会收到 BackgroundChanged。
        self.canvas.set_background(self.core.background_image().as_ref());
        // 通知在右上角，或在中央。
        if settings.corner_notices {
            self.notice.set_halign(gtk::Align::End);
            self.notice.set_valign(gtk::Align::Start);
            self.notice.set_margin_top(16);
            self.notice.set_margin_end(16);
        } else {
            self.notice.set_halign(gtk::Align::Center);
            self.notice.set_valign(gtk::Align::Center);
            self.notice.set_margin_top(0);
            self.notice.set_margin_end(0);
        }
        if settings.hide_zoom_info {
            self.zoom_label.set_visible(false);
        }
        let lang = self.core.ui_lang(system_lang());
        if lang != self.lang.replace(lang) {
            self.hint
                .set_text(&anyview_core::empty_state_hint(Platform::Linux, lang));
            self.update_title();
        }
        // 语言和键盘映射都可能变了：右键菜单按现在的文字和实际生效的按键重新生成（ACTIONS 规则 7）。
        *self.menu_model.borrow_mut() = menu::build_model(&self.core, lang);
        self.overlays.apply_settings(&self.core, lang, &settings);
        self.refresh_overlays();
        self.canvas.queue_draw();
        self.update_inhibit();
    }

    /// 设置窗口点了"确定"、设置已经生效：刷新界面；写文件失败时提示。
    pub fn settings_applied(self: &Rc<Self>, applied: &SettingsApplied) {
        self.apply_ui_settings();
        if let Some(message) = &applied.save_error {
            self.show_notice(
                &self
                    .text(Key::NoticeSettingsSaveFailed)
                    .replace("{message}", message),
            );
        }
        selftest::on_settings_applied(self, applied);
    }

    pub fn open(&self, path: &Path) {
        self.core.dispatch(Command::Open {
            path: path.display().to_string(),
        });
    }

    /// 别的进程交来的图片（不允许多个实例）：在这个窗口里打开，窗口来到前面。
    pub fn open_forwarded(self: &Rc<Self>, request: crate::instance::Request) {
        if let Some(token) = request.token.as_deref() {
            self.window.set_startup_id(token);
        }
        if let Some(path) = request.open.as_deref() {
            selftest::on_forwarded(self, path);
            self.open(path);
        }
        self.window.present();
    }

    pub fn current_name(&self) -> String {
        self.name.borrow().clone()
    }

    /// 正在显示的通知文字（自测记录用）。
    pub fn notice_text(&self) -> Option<String> {
        self.notice
            .is_visible()
            .then(|| self.notice.text().to_string())
    }

    /// 自测：现在挡在画面上的控件（窗口坐标）：悬浮界面，以及倍率提示和通知。
    pub fn covered_rects(&self) -> Vec<gtk::graphene::Rect> {
        let mut rects = self.overlays.covered_rects(self.window.upcast_ref());
        for label in [&self.zoom_label, &self.notice] {
            if label.is_visible()
                && let Some(bounds) = label.compute_bounds(&self.window)
            {
                rects.push(bounds);
            }
        }
        rects
    }

    /// 鼠标按键（中键单击、右键、侧键、滚轮）要做的事：设置里映射了就按映射，没有映射时按默认
    /// （ACTIONS"鼠标与触控板的默认动作"：右键弹出菜单，侧键和滚轮翻页，中键单击不做事）。
    pub fn mouse_target(&self, input: MouseInput) -> MouseTarget {
        match self.core.mouse_action(input) {
            MouseBinding::Action(id) => MouseTarget::Action(id),
            MouseBinding::Nothing => MouseTarget::Nothing,
            MouseBinding::Default => match input {
                MouseInput::Right => MouseTarget::ContextMenu,
                MouseInput::Back | MouseInput::WheelUp => MouseTarget::Action("prev_image"),
                MouseInput::Forward | MouseInput::WheelDown => MouseTarget::Action("next_image"),
                MouseInput::Left | MouseInput::Middle => MouseTarget::Nothing,
            },
        }
    }

    /// 执行鼠标按键要做的事；右键菜单弹在画布的 (x, y)。
    fn run_mouse(self: &Rc<Self>, input: MouseInput, x: f64, y: f64) {
        match self.mouse_target(input) {
            MouseTarget::Action(id) => {
                self.invoke(id);
            }
            MouseTarget::ContextMenu => menu::popup(self, x, y),
            MouseTarget::Nothing => {}
        }
    }

    /// 点在画布宽度上的位置（0–1），左键单击、双击的规则用。
    fn x_fraction(&self, x: f64) -> f64 {
        x / f64::from(self.canvas.width().max(1))
    }

    /// 跳到指定序号（进度条）。
    /// 这一代次的新图像到了：有翻页效果就开始（先拍下旧画面，再换图）。
    fn start_turn(&self, generation: u64) {
        if let Some((turned, t)) = self.pending_turn.take()
            && turned == generation
        {
            self.canvas.begin_transition(t.effect, t.forward, t.duration_ms);
        }
    }

    /// 翻页音效。自测时只记下来，不出声。
    fn play_page_sound(&self) {
        let used = if self.selftest.borrow().is_some() {
            self.page_sound
                .describe()
                .map(|(backend, _)| format!("{backend}（自测不出声）"))
                .unwrap_or_else(|| "unavailable".into())
        } else {
            self.page_sound.play()
        };
        self.sound_log.borrow_mut().push(used);
    }

    pub fn jump_to(&self, index: usize) {
        self.core
            .dispatch(Command::Navigate(Navigation::Jump(index)));
    }

    /// 悬浮界面跟着核心的状态刷新；开着的“生成信息”面板也按当前图片换内容。
    pub fn refresh_overlays(&self) {
        self.overlays.refresh(self);
        crate::generation_window::refresh(self);
    }

    pub fn current_position(&self) -> (Option<usize>, Option<usize>) {
        (self.index.get(), self.total.get())
    }

    /// 执行动作并把结果落到界面上。
    pub fn invoke(self: &Rc<Self>, id: &str) -> ActionOutcome {
        let outcome = self.core.invoke_action(id);
        self.apply(outcome.clone());
        // 旋转后窗口跟着图片转；缩放、翻转等不改变窗口。
        if self.follows_image()
            && let Some(size) = self
                .core
                .window_size_after_action(id, available_area(&self.window))
        {
            self.set_window_size(size);
        }
        outcome
    }

    /// 窗口跟着图片变大小：普通窗口才跟；最大化、全屏时窗口不变。
    fn follows_image(&self) -> bool {
        self.window_movable()
    }

    /// 窗口现在能不能移动：最大化、全屏时不能。
    pub fn window_movable(&self) -> bool {
        !self.window.is_maximized() && !self.window.is_fullscreen()
    }

    /// 按核心算出的大小（逻辑像素）设置窗口。窗口位置由窗口管理器决定（Wayland 不允许程序摆放窗口）。
    fn set_window_size(&self, (width, height): (f64, f64)) {
        let size = (width.round() as i32, height.round() as i32);
        self.last_fit.set(Some(size));
        self.window.set_default_size(size.0, size.1);
    }

    /// 窗口回到当前图片的大小（退出最大化、全屏时）。
    fn follow_current_image(&self) {
        if self.follows_image()
            && let Some(size) = self.core.window_size_for_view(available_area(&self.window))
        {
            self.set_window_size(size);
        }
    }

    pub fn apply(self: &Rc<Self>, outcome: ActionOutcome) {
        match outcome {
            ActionOutcome::Unavailable | ActionOutcome::Started => {}
            ActionOutcome::ViewChanged { show_zoom } => {
                self.canvas.queue_draw();
                if show_zoom {
                    self.show_zoom();
                }
            }
            ActionOutcome::OverlayChanged => self.refresh_overlays(),
            ActionOutcome::Ui(request) => self.perform(request),
        }
    }

    fn perform(self: &Rc<Self>, request: UiRequest) {
        let window = &self.window;
        match request {
            UiRequest::OpenFileDialog => open_dialog(self, false),
            UiRequest::OpenFolderDialog => open_dialog(self, true),
            // 退出（Esc、X）：先关窗口，让它记下窗口大小，再退出程序。Linux 每个窗口是一个进程，
            // Esc 的"关窗口，最后一个时退出"也就是退出。
            UiRequest::Quit | UiRequest::CloseOrQuit => {
                let app = window.application();
                window.close();
                if let Some(app) = app {
                    app.quit();
                }
            }
            UiRequest::ToggleFullscreen => window.set_fullscreened(!window.is_fullscreen()),
            UiRequest::ToggleMaximize => window.set_maximized(!window.is_maximized()),
            UiRequest::Minimize => window.minimize(),
            UiRequest::CloseWindow => window.close(),
            UiRequest::ShowAbout => show_about(self),
            UiRequest::ShowContextMenu => {
                let (w, h) = (
                    f64::from(self.canvas.width()),
                    f64::from(self.canvas.height()),
                );
                menu::popup(self, w / 2.0, h / 2.0);
            }
            UiRequest::TrashFile {
                path,
                name,
                confirm,
            } => trash_file(self, path, name, confirm),
            UiRequest::ShowSettings => {
                settings_window::show(self);
            }
            UiRequest::ApplySettings => self.apply_ui_settings(),
            UiRequest::RevealFile { path } => {
                // 在文件管理器中显示并选中。
                let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
                launcher.open_containing_folder(Some(window), gio::Cancellable::NONE, |_| {});
            }
            UiRequest::CopyImage { path } => {
                copy_image(self, &path);
            }
            UiRequest::OpenInEditor { path, editor } => open_in_editor(self, &path, &editor),
            UiRequest::SetWallpaper { path } => set_wallpaper(self, &path),
            UiRequest::Print { name } => print_image(self, &name),
            UiRequest::ShowPicker => {
                crate::picker::show(self);
            }
            UiRequest::ShowHelp { kind } => {
                crate::help_window::show(self, kind);
            }
            UiRequest::ShowGenerationInfo => crate::generation_window::show(self),
            // 只在 macOS 上有意义。
            UiRequest::HideApp
            | UiRequest::HideOthers
            | UiRequest::ShowAll
            | UiRequest::ClearRecent => {}
        }
    }

    /// 到达首尾时询问：转到首张（末张）、转到下一个（上一个）文件夹，或取消。
    fn ask_boundary(self: &Rc<Self>, forward: bool, archive: bool) {
        if selftest::on_boundary(self, forward) {
            return;
        }
        let prompt = anyview_core::i18n::boundary_prompt(forward, archive, self.lang());
        let dialog = gtk::AlertDialog::builder()
            .message(prompt.message.as_str())
            .buttons(
                prompt
                    .buttons
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
            .cancel_button(2)
            .default_button(0)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |choice| {
            let Some(ui) = weak.upgrade() else { return };
            let id = match (choice, forward) {
                (Ok(0), true) => "first_image",
                (Ok(0), false) => "last_image",
                (Ok(1), true) => "next_container",
                (Ok(1), false) => "prev_container",
                _ => return,
            };
            ui.invoke(id);
        });
    }

    /// 图片被安全上限拦下了：问用户要不要仍然打开。
    fn ask_limit(self: &Rc<Self>, path: std::path::PathBuf) {
        if selftest::on_limit(self) {
            return;
        }
        let prompt = anyview_core::i18n::limit_prompt(self.lang());
        let dialog = gtk::AlertDialog::builder()
            .message(prompt.message.as_str())
            .buttons(
                prompt
                    .buttons
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
            .cancel_button(1)
            // 默认按钮是"取消"：回车、Esc 都不打开。
            .default_button(1)
            .modal(true)
            .build();
        let weak = Rc::downgrade(self);
        dialog.choose(Some(&self.window), gio::Cancellable::NONE, move |choice| {
            let Some(ui) = weak.upgrade() else { return };
            match choice {
                Ok(0) => ui.core.open_without_limits(&path),
                _ => ui.core.keep_limits(&path),
            }
        });
    }

    pub fn update_title(&self) {
        let name = self.name.borrow();
        let title = if name.is_empty() {
            self.text(Key::AppName).to_string()
        } else {
            match (self.index.get(), self.total.get()) {
                (Some(i), Some(total)) => format!("{name} [{}/{total}]", i + 1),
                _ => name.clone(),
            }
        };
        self.window.set_title(Some(&title));
    }

    pub fn show_notice(&self, text: &str) {
        selftest::note_notice(self, text);
        flash(
            &self.notice,
            &self.notice_serial,
            text,
            Duration::from_secs(2),
        );
    }

    /// 缩放倍率提示：在画面中央显示约 1 秒；设置"不显示缩放信息控制"时不显示。
    fn show_zoom(&self) {
        if self.ui_settings.get().hide_zoom_info {
            return;
        }
        if let Some(layout) = self.core.view_layout() {
            flash(
                &self.zoom_label,
                &self.zoom_serial,
                &anyview_core::zoom_text(layout.zoom),
                Duration::from_secs(1),
            );
        }
    }

    /// 画布上的逻辑像素 → 核心使用的物理像素。
    fn physical(&self, x: f64, y: f64) -> (f64, f64) {
        let s = self.canvas.physical_scale();
        (x * s, y * s)
    }

    fn after_view_change(&self, changed: bool, show_zoom: bool) {
        if changed {
            self.canvas.queue_draw();
            if show_zoom {
                self.show_zoom();
            }
        }
    }
}

/// 显示一段文字，到时自动隐藏；期间再次显示时重新计时。
fn flash(label: &gtk::Label, serial: &Rc<Cell<u64>>, text: &str, duration: Duration) {
    let current = serial.get() + 1;
    serial.set(current);
    label.set_text(text);
    label.set_visible(true);
    let label = label.clone();
    let serial = Rc::clone(serial);
    glib::timeout_add_local_once(duration, move || {
        if serial.get() == current {
            label.set_visible(false);
        }
    });
}

/// 系统的界面语言：偏好列表（LANGUAGE、LC_ALL、LANG 等）里第一种有翻译的。
pub fn system_lang() -> Lang {
    Lang::from_locales(glib::language_names().iter().map(|name| name.as_str()))
}

pub fn build(app: &gtk::Application, options: &LaunchOptions, early: EarlyCore) {
    let lang = early.core.ui_lang(system_lang());
    let canvas = ImageCanvas::default();
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);

    let hint = gtk::Label::new(Some(&anyview_core::empty_state_hint(Platform::Linux, lang)));
    hint.add_css_class("anyview-hint");
    let status = gtk::Label::new(Some(&format!(
        "{} {}",
        i18n::text(Key::AppName, lang),
        anyview_core::version()
    )));
    status.add_css_class("anyview-status");
    let empty = gtk::Box::new(gtk::Orientation::Vertical, 8);
    empty.set_halign(gtk::Align::Center);
    empty.set_valign(gtk::Align::Center);
    empty.set_can_target(false);
    empty.append(&hint);
    empty.append(&status);

    let notice = gtk::Label::new(None);
    notice.add_css_class("anyview-notice");
    notice.set_halign(gtk::Align::End);
    notice.set_valign(gtk::Align::Start);
    notice.set_margin_top(16);
    notice.set_margin_end(16);
    notice.set_can_target(false);
    notice.set_visible(false);

    let zoom_label = gtk::Label::new(None);
    zoom_label.add_css_class("anyview-zoom");
    zoom_label.set_halign(gtk::Align::Center);
    zoom_label.set_valign(gtk::Align::Center);
    zoom_label.set_can_target(false);
    zoom_label.set_visible(false);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&canvas));
    overlay.add_overlay(&empty);
    overlay.add_overlay(&notice);
    overlay.add_overlay(&zoom_label);
    let overlay_widgets = Overlays::new(&overlay);

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(i18n::text(Key::AppName, lang))
        .default_width(900)
        .default_height(600)
        .child(&overlay)
        .build();
    window.add_css_class("anyview-main");

    let EarlyCore {
        core,
        wakes: rx,
        opened_at,
    } = early;
    canvas.set_core(Arc::clone(&core));
    let background_css = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(&window),
        &background_css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
    let ui_settings = core.ui_settings();
    let menu_model = menu::build_model(&core, lang);
    let ui = Rc::new(Ui {
        core,
        window,
        canvas,
        lang: Cell::new(lang),
        actions: gio::SimpleActionGroup::new(),
        menu_model: RefCell::new(menu_model),
        ui_settings: Cell::new(ui_settings),
        background_css,
        hint,
        settings_window: RefCell::new(None),
        picker: RefCell::new(None),
        help_windows: RefCell::new(Vec::new()),
        pending_turn: Cell::new(None),
        page_sound: crate::sound::PageSound::default(),
        sound_log: RefCell::new(Vec::new()),
        settings_error: RefCell::new(None),
        overlays: overlay_widgets,
        empty,
        status,
        notice,
        notice_serial: Rc::new(Cell::new(0)),
        zoom_label,
        zoom_serial: Rc::new(Cell::new(0)),
        pointer: Cell::new((0.0, 0.0)),
        wheel: Cell::new(0.0),
        generation: Cell::new(0),
        name: RefCell::new(String::new()),
        index: Cell::new(None),
        total: Cell::new(None),
        ping_sent: Cell::new(None),
        ping_ms: Cell::new(None),
        selftest: RefCell::new(None),
        fit_pending: Cell::new(false),
        last_fit: Cell::new(None),
        show_started: Cell::new(None),
        inhibit_cookie: Cell::new(0),
    });
    // 记住窗口大小；自测时不用，每次都从 900x600 开始。直接启动（没有图片）时固定 900x600、
    // 位置由窗口管理器决定。
    if options.self_test.is_none() {
        if options.open.is_some() {
            window_state::restore(&ui.window);
        }
        let window = ui.window.clone();
        ui.window.connect_close_request(move |_| {
            window_state::save(&window);
            glib::Propagation::Proceed
        });
    }
    // 窗口看不见了（最小化等）就不再阻止屏保，看得见了再申请。
    let weak = Rc::downgrade(&ui);
    ui.window.connect_suspended_notify(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.update_inhibit();
        }
    });
    // 退出最大化、全屏后，窗口回到当前图片的大小。窗口管理器退出时会先恢复原来的大小，
    // 稍等一下再调，免得被它盖掉。
    for property in ["maximized", "fullscreened"] {
        let weak = Rc::downgrade(&ui);
        ui.window.connect_notify_local(Some(property), move |window, _| {
            if window.is_maximized() || window.is_fullscreen() {
                return;
            }
            let weak = weak.clone();
            glib::timeout_add_local_once(Duration::from_millis(100), move || {
                if let Some(ui) = weak.upgrade() {
                    ui.follow_current_image();
                }
            });
        });
    }
    ui.apply_ui_settings();
    // 退出前记下查看状态（"保存缩放状态"）。
    let core = Arc::clone(&ui.core);
    // 退出前也关掉压缩包（删掉解出的临时文件）。
    app.connect_shutdown(move |_| core.shutdown());
    // 不允许多个实例：接收别的进程交来的图片（先启动的实例占着套接字，退出时交出）。
    if let Some(socket) = crate::instance::socket_path() {
        let (tx, rx) = async_channel::unbounded::<crate::instance::Request>();
        if crate::instance::listen(&socket, move |request| {
            let _ = tx.try_send(request);
        }) {
            let weak = Rc::downgrade(&ui);
            glib::spawn_future_local(async move {
                while let Ok(request) = rx.recv().await {
                    let Some(ui) = weak.upgrade() else { break };
                    ui.open_forwarded(request);
                }
            });
            app.connect_shutdown(move |_| crate::instance::release(&socket));
        }
    }
    menu::install_actions(&ui);
    install_keys(&ui);
    install_pointer(&ui);
    install_drop(&ui);
    overlays::connect(&ui);
    install_hover(&ui);
    crate::monitor_icc::connect(&ui);
    ui.refresh_overlays();

    let ui_events = Rc::clone(&ui);
    glib::spawn_future_local(async move {
        while rx.recv().await.is_ok() {
            for event in ui_events.core.drain_events() {
                handle_event(&ui_events, event);
            }
        }
    });

    ui.ping_sent.set(Some(Instant::now()));
    ui.core.dispatch(Command::Ping { token: 1 });
    ui.window.present();
    // 设置文件坏了时提示一次。
    if let Some(notice) = ui.core.settings_notice(ui.lang()) {
        ui.show_notice(&notice);
    }

    // 命令行给的图片已在启动时打开（EarlyCore）。
    if let Some(plan) = options.self_test.clone() {
        selftest::start(&ui, plan, options.open.clone(), opened_at);
    }
}

/// 运行时性能记录：GLib 调试日志，设置 `G_MESSAGES_DEBUG=anyview-perf` 时输出。
const PERF_LOG: &str = "anyview-perf";

fn log_shown(ui: &Ui, image: &anyview_core::DecodedImage) {
    let Some(started) = ui.show_started.take() else {
        return;
    };
    glib::g_debug!(
        PERF_LOG,
        "image {}x{} shown after {:.1} ms (decode {:.1} ms; {:.1} ms after main)",
        image.width,
        image.height,
        started.elapsed().as_secs_f64() * 1000.0,
        image.info.decode_ms,
        crate::MAIN_START
            .get()
            .map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0)
    );
}

/// 左键拖动这一次做什么。
#[derive(Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Pan,
    /// 要移动窗口，等移动超过门槛再交给窗口管理器。
    MoveWindowPending,
    /// 已交给窗口管理器，之后的拖动不再处理。
    MovingWindow,
}

/// 按下后移动多少逻辑像素才算拖动。
const DRAG_THRESHOLD: f64 = 4.0;

/// 让窗口管理器开始移动窗口（Wayland 也可以），起点是这次拖动按下的位置。
fn begin_window_move(ui: &Ui, gesture: &gtk::GestureDrag) {
    let (Some((sx, sy)), Some(device)) = (gesture.start_point(), gesture.current_event_device())
    else {
        return;
    };
    let Some(toplevel) = ui.window.surface().and_downcast::<gdk::Toplevel>() else {
        return;
    };
    let Some(point) = ui
        .canvas
        .compute_point(&ui.window, &gtk::graphene::Point::new(sx as f32, sy as f32))
    else {
        return;
    };
    let (nx, ny) = ui.window.surface_transform();
    toplevel.begin_move(
        &device,
        gesture.current_button() as i32,
        f64::from(point.x()) + nx,
        f64::from(point.y()) + ny,
        gesture.current_event_time(),
    );
}

/// 窗口跟着图片：切到新图片时窗口变成图片的大小（由核心按显示方式算出），最大化、全屏时不调。
fn fit_window(ui: &Ui, width: u32, height: u32) {
    if !ui.follows_image() {
        return;
    }
    // 关掉"根据图片大小调节窗口大小"时核心不给大小，窗口不变。
    if let Some(size) = ui
        .core
        .window_size_for_image(width, height, available_area(&ui.window))
    {
        ui.set_window_size(size);
    }
}

/// 屏幕可用区域（逻辑像素）。GTK4 拿不到去掉面板后的工作区，按窗口所在显示器大小的 90% 算。
fn available_area(window: &gtk::ApplicationWindow) -> (f64, f64) {
    let display = gtk::prelude::WidgetExt::display(window);
    let monitor = window
        .surface()
        .and_then(|surface| display.monitor_at_surface(&surface))
        .or_else(|| {
            display
                .monitors()
                .item(0)
                .and_then(|m| m.downcast::<gdk::Monitor>().ok())
        });
    monitor.map_or((1280.0, 720.0), |monitor| {
        let geometry = monitor.geometry();
        (
            f64::from(geometry.width()) * 0.9,
            f64::from(geometry.height()) * 0.9,
        )
    })
}

fn handle_event(ui: &Rc<Ui>, event: Event) {
    match event {
        Event::Pong { .. } => {
            if let Some(sent) = ui.ping_sent.take() {
                let ms = sent.elapsed().as_secs_f64() * 1000.0;
                ui.ping_ms.set(Some(ms));
                let link = ui
                    .text(Key::CoreLinkOk)
                    .replace("{ms}", &format!("{ms:.2}"));
                ui.status.set_text(&format!(
                    "{} {} · {link}",
                    ui.text(Key::AppName),
                    anyview_core::version()
                ));
                selftest::on_pong(ui);
            }
        }
        Event::Current {
            generation,
            name,
            index,
            total,
            ..
        } => {
            ui.generation.set(generation);
            // 翻页效果和音效：音效马上放，效果等新图像送达时做。
            let turn = ui.core.transition_for(generation);
            ui.pending_turn.set(turn.map(|t| (generation, t)));
            if turn.is_some_and(|t| t.sound) {
                ui.play_page_sound();
            }
            // 每切到一张新图片，窗口都跟着图片变大小，在图像送达时调。
            ui.fit_pending.set(true);
            ui.show_started.set(Some(Instant::now()));
            *ui.name.borrow_mut() = name;
            ui.index.set(index);
            ui.total.set(total);
            ui.update_title();
            ui.refresh_overlays();
        }
        Event::Collection {
            generation,
            index,
            total,
        } => {
            if generation == ui.generation.get() {
                ui.index.set(index);
                ui.total.set(Some(total));
                ui.update_title();
                ui.refresh_overlays();
            }
            // 排序、文件夹变了：选择图片窗口重新载入列表。
            let picker = ui.picker.borrow().clone();
            if let Some(picker) = picker {
                picker.reload();
            }
        }
        Event::BackgroundChanged => ui.canvas.set_background(ui.core.background_image().as_ref()),
        // 别的进程改了设置文件：像"确定"之后那样刷新。
        Event::SettingsReloaded { changed } => {
            ui.settings_applied(&SettingsApplied {
                changed,
                save_error: None,
            });
        }
        Event::Thumbnail { path, image } => {
            let picker = ui.picker.borrow().clone();
            if let Some(picker) = picker {
                picker.on_thumbnail(&path.display().to_string(), image.as_ref());
            }
        }
        Event::ImageReady {
            generation, image, ..
        } => {
            if generation == ui.generation.get() {
                ui.update_inhibit();
                ui.start_turn(generation);
                if ui.fit_pending.take() {
                    fit_window(ui, image.width, image.height);
                }
                ui.canvas.set_image(Some(&image));
                log_shown(ui, &image);
                ui.empty.set_visible(false);
                ui.refresh_overlays();
                selftest::on_image(ui, &image);
            }
        }
        Event::ImagePreview {
            generation,
            image,
            width,
            height,
            ..
        } => {
            if generation == ui.generation.get() {
                ui.start_turn(generation);
                if ui.fit_pending.take() {
                    fit_window(ui, width, height);
                }
                ui.canvas.set_preview(&image, width, height);
                if let Some(started) = ui.show_started.get() {
                    glib::g_debug!(
                        PERF_LOG,
                        "preview shown after {:.1} ms",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
                ui.empty.set_visible(false);
                ui.refresh_overlays();
                selftest::on_preview(ui);
            }
        }
        Event::ImageUpdated {
            generation, image, ..
        } => {
            if generation == ui.generation.get() {
                ui.canvas.replace_image(&image);
                // 清晰图替换预览后，图片信息才有完整内容。
                ui.refresh_overlays();
                log_shown(ui, &image);
                selftest::on_image(ui, &image);
            }
        }
        // 动图的一帧：只换纹理，更新动画信息控制。
        Event::AnimationFrame {
            generation, image, ..
        } => {
            if generation == ui.generation.get() {
                ui.canvas.replace_image(&image);
                ui.overlays.refresh_animation(ui);
                selftest::on_frame(ui);
            }
        }
        Event::ExactReady {
            generation,
            signature,
            x,
            y,
            image,
            render_ms,
        } => {
            if generation == ui.generation.get() {
                ui.canvas.set_exact(&image, x, y, signature, render_ms);
                glib::g_debug!(PERF_LOG, "exact render {render_ms:.1} ms");
            }
        }
        Event::ImageFailed {
            generation,
            message,
            ..
        } => {
            if generation == ui.generation.get() {
                ui.canvas.set_image(None);
                ui.refresh_overlays();
                ui.show_notice(
                    &ui.text(Key::NoticeImageFailed)
                        .replace("{message}", &message),
                );
                selftest::on_failure(ui, &message);
            }
        }
        Event::ImageBlocked { generation, path } => {
            if generation == ui.generation.get() {
                ui.canvas.set_image(None);
                ui.refresh_overlays();
                ui.ask_limit(path);
            }
        }
        Event::OpenFailed { message, .. } => {
            ui.show_notice(
                &ui.text(Key::NoticeOpenFailed)
                    .replace("{message}", &message),
            );
            selftest::on_failure(ui, &message);
        }
        Event::Notice(kind) => {
            ui.show_notice(&kind.text(ui.lang()));
            // 复制、移动完成是正常的结果；其余提示在自测里算问题。
            if matches!(kind, NoticeKind::Copied { .. } | NoticeKind::Moved { .. }) {
                selftest::on_notice(ui, &kind);
            } else {
                selftest::on_failure(ui, &format!("{kind:?}"));
            }
        }
        Event::Boundary { forward, archive } => ui.ask_boundary(forward, archive),
        Event::Closed { generation } => {
            ui.generation.set(generation);
            ui.canvas.set_image(None);
            ui.update_inhibit();
            ui.empty.set_visible(true);
            ui.name.borrow_mut().clear();
            ui.index.set(None);
            ui.total.set(None);
            ui.update_title();
            ui.refresh_overlays();
        }
    }
}

/// GTK 的按键 → 核心的按键名称（docs/ACTIONS.md；Linux 键位与 Windows 相同）。
pub fn key_name(key: gdk::Key) -> Option<String> {
    use gdk::Key as K;
    let named = match key {
        K::Left | K::KP_Left => "Left",
        K::Right | K::KP_Right => "Right",
        K::Up | K::KP_Up => "Up",
        K::Down | K::KP_Down => "Down",
        K::Home | K::KP_Home => "Home",
        K::End | K::KP_End => "End",
        K::Page_Up | K::KP_Page_Up => "PageUp",
        K::Page_Down | K::KP_Page_Down => "PageDown",
        K::Return | K::KP_Enter | K::ISO_Enter => "Enter",
        K::Tab | K::ISO_Left_Tab | K::KP_Tab => "Tab",
        K::Escape => "Escape",
        K::Delete | K::KP_Delete => "Delete",
        K::BackSpace => "Backspace",
        K::Insert | K::KP_Insert => "Insert",
        K::space | K::KP_Space => "Space",
        K::F1 => "F1",
        K::F2 => "F2",
        K::F3 => "F3",
        K::F4 => "F4",
        K::F5 => "F5",
        K::F6 => "F6",
        K::F7 => "F7",
        K::F8 => "F8",
        K::F9 => "F9",
        K::F10 => "F10",
        K::F11 => "F11",
        K::F12 => "F12",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    let c = key.to_unicode()?;
    (!c.is_control()).then(|| c.to_string())
}

pub fn modifier_bits(state: gdk::ModifierType) -> u32 {
    [
        (gdk::ModifierType::SHIFT_MASK, SHIFT),
        (gdk::ModifierType::CONTROL_MASK, CTRL),
        (gdk::ModifierType::ALT_MASK, ALT),
        (gdk::ModifierType::SUPER_MASK, CMD),
        (gdk::ModifierType::META_MASK, CMD),
    ]
    .into_iter()
    .filter(|(mask, _)| state.contains(*mask))
    .fold(0, |acc, (_, bit)| acc | bit)
}

fn install_keys(ui: &Rc<Ui>) {
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(ui);
    keys.connect_key_pressed(move |_, key, _, state| {
        let (Some(ui), Some(name)) = (weak.upgrade(), key_name(key)) else {
            return glib::Propagation::Proceed;
        };
        match ui.core.action_for_key(&name, modifier_bits(state)) {
            Some(id) => {
                ui.invoke(id);
                glib::Propagation::Stop
            }
            None => glib::Propagation::Proceed,
        }
    });
    // 捕获阶段先处理：Tab 等按键在 GTK 里默认用来切换焦点，要先交给核心按键位解析。
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    ui.window.add_controller(keys);
}

/// 指针在窗口里移动时，按核心的区域规则显示控制栏和边缘箭头。
fn install_hover(ui: &Rc<Ui>) {
    let motion = gtk::EventControllerMotion::new();
    let weak = Rc::downgrade(ui);
    motion.connect_motion(move |controller, x, y| {
        let (Some(ui), Some(widget)) = (weak.upgrade(), controller.widget()) else {
            return;
        };
        ui.overlays.pointer_moved(
            &ui,
            x,
            y,
            f64::from(widget.width()),
            f64::from(widget.height()),
        );
    });
    let weak = Rc::downgrade(ui);
    motion.connect_leave(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.overlays.pointer_left(&ui);
        }
    });
    ui.window.add_controller(motion);

    // 鼠标侧键：后退键为上一张，前进键为下一张。
    let side = gtk::GestureClick::new();
    side.set_button(0);
    let weak = Rc::downgrade(ui);
    side.connect_pressed(move |gesture, _, x, y| {
        let Some(ui) = weak.upgrade() else { return };
        // 侧键：默认后退键为上一张，前进键为下一张；设置里可以映射（"X 键""Y 键"）。
        let input = match gesture.current_button() {
            8 => MouseInput::Back,
            9 => MouseInput::Forward,
            _ => return,
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        ui.run_mouse(input, x, y);
    });
    ui.window.add_controller(side);
}

/// 鼠标与触控板：左键拖动平移、双击最大化、右键菜单、
/// 滚轮翻页（按住 Ctrl 缩放）、触控板双指平移与捏合缩放。
fn install_pointer(ui: &Rc<Ui>) {
    let canvas = &ui.canvas;

    let motion = gtk::EventControllerMotion::new();
    let weak = Rc::downgrade(ui);
    motion.connect_motion(move |_, x, y| {
        if let Some(ui) = weak.upgrade() {
            ui.pointer.set((x, y));
        }
    });
    canvas.add_controller(motion);

    // 左键拖动："拖着图片走"：图片完整显示、窗口能移动时移动窗口（交给窗口管理器，屏幕边缘贴靠照常），
    // 否则平移图片。中键拖动总是平移图片。按下后移动超过几个像素才算拖动，单击、双击不会误挪窗口。
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let last = Rc::new(Cell::new((0.0, 0.0)));
    let mode = Rc::new(Cell::new(DragMode::Pan));
    // 这次按下后移动超过了门槛：松开时不算单击（左键映射、单击两侧翻页，设置"鼠标"页）。
    let moved = Rc::new(Cell::new(false));
    {
        let (last, mode, moved, weak) = (
            Rc::clone(&last),
            Rc::clone(&mode),
            Rc::clone(&moved),
            Rc::downgrade(ui),
        );
        drag.connect_drag_begin(move |_, _, _| {
            last.set((0.0, 0.0));
            moved.set(false);
            if let Some(ui) = weak.upgrade() {
                mode.set(if ui.core.left_drag_moves_window(ui.window_movable()) {
                    DragMode::MoveWindowPending
                } else {
                    DragMode::Pan
                });
            }
        });
    }
    let weak = Rc::downgrade(ui);
    let drag_moved = Rc::clone(&moved);
    drag.connect_drag_update(move |gesture, ox, oy| {
        let Some(ui) = weak.upgrade() else { return };
        if ox.hypot(oy) >= DRAG_THRESHOLD {
            drag_moved.set(true);
        }
        match mode.get() {
            DragMode::MoveWindowPending => {
                if ox.hypot(oy) >= DRAG_THRESHOLD {
                    mode.set(DragMode::MovingWindow);
                    begin_window_move(&ui, gesture);
                }
            }
            DragMode::MovingWindow => {}
            DragMode::Pan => {
                let (lx, ly) = last.replace((ox, oy));
                let (dx, dy) = ui.physical(ox - lx, oy - ly);
                ui.after_view_change(ui.core.view_pan_by(dx, dy).is_some(), false);
            }
        }
    });
    canvas.add_controller(drag);

    let middle = gtk::GestureDrag::new();
    middle.set_button(gdk::BUTTON_MIDDLE);
    let middle_last = Rc::new(Cell::new((0.0, 0.0)));
    let start = Rc::clone(&middle_last);
    middle.connect_drag_begin(move |_, _, _| start.set((0.0, 0.0)));
    let weak = Rc::downgrade(ui);
    middle.connect_drag_update(move |_, ox, oy| {
        let Some(ui) = weak.upgrade() else { return };
        let (lx, ly) = middle_last.replace((ox, oy));
        let (dx, dy) = ui.physical(ox - lx, oy - ly);
        ui.after_view_change(ui.core.view_pan_by(dx, dy).is_some(), false);
    });
    // 中键单击（没有拖动就松开）：映射了就执行（设置"鼠标中键"），默认不做事。
    let weak = Rc::downgrade(ui);
    middle.connect_drag_end(move |gesture, ox, oy| {
        let Some(ui) = weak.upgrade() else { return };
        if ox.hypot(oy) < DRAG_THRESHOLD {
            let (sx, sy) = gesture.start_point().unwrap_or((0.0, 0.0));
            ui.run_mouse(MouseInput::Middle, sx, sy);
        }
    });
    canvas.add_controller(middle);

    // 左键双击：默认最大化、还原；左键单击：映射了左键或打开了"单击屏幕两侧时翻页"时，
    // 没有拖动就松开才算单击（规则由核心给出）。
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    let weak = Rc::downgrade(ui);
    click.connect_pressed(move |_, presses, x, _| {
        if let (Some(ui), 2) = (weak.upgrade(), presses)
            && let Some(id) = ui.core.left_double_click_action(ui.x_fraction(x))
        {
            ui.invoke(id);
        }
    });
    let weak = Rc::downgrade(ui);
    click.connect_released(move |_, _, x, _| {
        let Some(ui) = weak.upgrade() else { return };
        if ui.core.left_click_matters()
            && !moved.get()
            && let Some(id) = ui.core.left_click_action(ui.x_fraction(x))
        {
            ui.invoke(id);
        }
    });
    canvas.add_controller(click);

    let secondary = gtk::GestureClick::new();
    secondary.set_button(gdk::BUTTON_SECONDARY);
    let weak = Rc::downgrade(ui);
    secondary.connect_pressed(move |_, _, x, y| {
        if let Some(ui) = weak.upgrade() {
            // 右键：默认弹出右键菜单；设置里映射了就执行映射的动作。
            ui.run_mouse(MouseInput::Right, x, y);
        }
    });
    canvas.add_controller(secondary);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    let weak = Rc::downgrade(ui);
    scroll.connect_scroll(move |controller, dx, dy| {
        let Some(ui) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let zoom = controller
            .current_event_state()
            .contains(gdk::ModifierType::CONTROL_MASK);
        let (px, py) = ui.pointer.get();
        let (ax, ay) = ui.physical(px, py);
        if controller.unit() == gdk::ScrollUnit::Wheel {
            // 普通滚轮：满一格动作一次；高精度滚轮的小步累计起来。
            let total = ui.wheel.get() + dy;
            let steps = total.trunc();
            ui.wheel.set(total - steps);
            for _ in 0..(steps.abs() as usize) {
                if zoom {
                    let changed = ui
                        .core
                        .view_zoom_step(if steps < 0.0 { 1 } else { -1 }, ax, ay);
                    ui.after_view_change(changed.is_some(), true);
                } else {
                    // 滚轮：默认向上为上一张、向下为下一张；设置里可以映射。
                    let input = if steps > 0.0 {
                        MouseInput::WheelDown
                    } else {
                        MouseInput::WheelUp
                    };
                    ui.run_mouse(input, px, py);
                }
            }
        } else if zoom {
            let changed = ui.core.view_zoom_by((-dy * 0.01).exp(), ax, ay);
            ui.after_view_change(changed.is_some(), true);
        } else {
            // 触控板双指滑动：画面跟着手指走；同时累计位移，供轻扫翻页判断。
            let (sx, sy) = ui.physical(-dx, -dy);
            ui.core.view_swipe(SwipePhase::Changed, sx, sy);
            ui.after_view_change(ui.core.view_pan_by(sx, sy).is_some(), false);
        }
        glib::Propagation::Stop
    });
    // 触控板双指左右轻扫翻页：手势开始、结束由 GTK 报告，是否翻页由核心判断。
    let weak = Rc::downgrade(ui);
    scroll.connect_scroll_begin(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.core.view_swipe(SwipePhase::Began, 0.0, 0.0);
        }
    });
    let weak = Rc::downgrade(ui);
    scroll.connect_scroll_end(move |_| {
        if let Some(ui) = weak.upgrade()
            && let Some(outcome) = ui.core.view_swipe(SwipePhase::Ended, 0.0, 0.0)
        {
            ui.apply(outcome);
        }
    });
    canvas.add_controller(scroll);

    let pinch = gtk::GestureZoom::new();
    let previous = Rc::new(Cell::new(1.0));
    let reset = Rc::clone(&previous);
    pinch.connect_begin(move |_, _| reset.set(1.0));
    let weak = Rc::downgrade(ui);
    pinch.connect_scale_changed(move |gesture, scale| {
        let Some(ui) = weak.upgrade() else { return };
        let factor = scale / previous.replace(scale);
        let (cx, cy) = gesture
            .bounding_box_center()
            .unwrap_or_else(|| ui.pointer.get());
        let (ax, ay) = ui.physical(cx, cy);
        ui.after_view_change(ui.core.view_zoom_by(factor, ax, ay).is_some(), true);
    });
    canvas.add_controller(pinch);
}

fn install_drop(ui: &Rc<Ui>) {
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    let weak = Rc::downgrade(ui);
    drop.connect_drop(move |_, value, _, _| {
        let (Some(ui), Ok(list)) = (weak.upgrade(), value.get::<gdk::FileList>()) else {
            return false;
        };
        match list.files().first().and_then(|file| file.path()) {
            Some(path) => {
                ui.open(&path);
                true
            }
            None => false,
        }
    });
    ui.window.add_controller(drop);
}

fn open_dialog(ui: &Rc<Ui>, folder: bool) {
    let dialog = gtk::FileDialog::builder()
        .title(ui.text(Key::OpenDialogTitle))
        .modal(true)
        .build();
    let weak = Rc::downgrade(ui);
    let done = move |result: Result<gio::File, glib::Error>| {
        if let (Some(ui), Ok(file)) = (weak.upgrade(), result)
            && let Some(path) = file.path()
        {
            ui.open(&path);
        }
    };
    if folder {
        dialog.select_folder(Some(&ui.window), gio::Cancellable::NONE, done);
        return;
    }
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(ui.text(Key::ImageFilterName)));
    // 图片和压缩包。
    for ext in anyview_core::formats::supported_extensions()
        .into_iter()
        .chain(anyview_core::archive::EXTENSIONS.iter().copied())
    {
        filter.add_suffix(ext);
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));
    dialog.open(Some(&ui.window), gio::Cancellable::NONE, done);
}

/// 删除：先确认（设置"删除文件时确认"），再移到回收站；成功后告诉核心，由核心显示下一张。
fn trash_file(ui: &Rc<Ui>, path: String, name: String, confirm: bool) {
    let weak = Rc::downgrade(ui);
    let trash = move || {
        let Some(ui) = weak.upgrade() else { return };
        let file = gio::File::for_path(&path);
        let weak = Rc::downgrade(&ui);
        file.trash_async(
            glib::Priority::DEFAULT,
            gio::Cancellable::NONE,
            move |result| {
                let Some(ui) = weak.upgrade() else { return };
                match result {
                    Ok(()) => ui.core.dispatch(Command::Removed { path }),
                    Err(e) => ui.show_notice(
                        &ui.text(Key::NoticeTrashFailed)
                            .replace("{message}", e.message()),
                    ),
                }
            },
        );
    };
    // 自测时不弹确认框，直接执行。
    if !confirm || ui.selftest.borrow().is_some() {
        trash();
        return;
    }
    let prompt = anyview_core::i18n::trash_prompt(Platform::Linux, ui.lang(), &name);
    let dialog = gtk::AlertDialog::builder()
        .message(prompt.message.as_str())
        .buttons(
            prompt
                .buttons
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )
        .cancel_button(1)
        .default_button(0)
        .modal(true)
        .build();
    dialog.choose(Some(&ui.window), gio::Cancellable::NONE, move |choice| {
        if let Ok(0) = choice {
            trash();
        }
    });
}

/// 复制到剪贴板：原图（按 EXIF 方向，不含反相、Gamma 等效果；超大图也是原图，不是代理纹理）加文件，
/// 粘贴到文件管理器得到文件，粘贴到编辑器得到图像。`path` 是磁盘上的文件（压缩包里的图片是解出的临时文件，核心给出）。
/// 原图还没解码好时不复制。
pub fn copy_image(ui: &Rc<Ui>, path: &str) -> bool {
    let Some(original) = ui.core.original_image() else {
        return false;
    };
    let texture = crate::canvas::texture_from(&original);
    let file = gio::File::for_path(path);
    let provider = gdk::ContentProvider::new_union(&[
        gdk::ContentProvider::for_value(&texture.to_value()),
        gdk::ContentProvider::for_value(&gdk::FileList::from_array(&[file]).to_value()),
    ]);
    if ui.window.clipboard().set_content(Some(&provider)).is_err() {
        return false;
    }
    ui.show_notice(ui.text(Key::NoticeCopied));
    true
}

/// 用图像编辑器打开：设置里的程序带上文件路径启动；没有设置时提示去设置。
/// Flatpak 里看不到系统装的程序，改用系统的“打开方式”对话框（门户），让用户选。
/// 自测时只记下来，不真的启动。
fn open_in_editor(ui: &Rc<Ui>, path: &str, editor: &str) {
    if crate::sandbox::in_flatpak() {
        if selftest::on_external(ui, "editor", &format!("portal {path}")) {
            return;
        }
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
        launcher.set_always_ask(true);
        let weak = Rc::downgrade(ui);
        launcher.launch(Some(&ui.window), gio::Cancellable::NONE, move |result| {
            if let (Some(ui), Err(e)) = (weak.upgrade(), result) {
                // 用户在对话框里取消的不算失败。
                if !e.matches(gtk::DialogError::Dismissed) && !e.matches(gtk::DialogError::Cancelled) {
                    ui.show_notice(&ui.text(Key::NoticeEditorFailed).replace("{message}", e.message()));
                }
            }
        });
        return;
    }
    if editor.is_empty() {
        ui.show_notice(ui.text(Key::NoticeEditorMissing));
        selftest::on_external(ui, "editor", "");
        return;
    }
    if selftest::on_external(ui, "editor", &format!("{editor} {path}")) {
        return;
    }
    if let Err(e) = std::process::Command::new(editor).arg(path).spawn() {
        ui.show_notice(
            &ui.text(Key::NoticeEditorFailed)
                .replace("{message}", &e.to_string()),
        );
    }
}

/// 设为桌面图片：KDE Plasma 使用 plasma-apply-wallpaperimage，
/// GNOME 用 gsettings；Flatpak 里这两个都用不了，用系统的壁纸门户。自测时只记下来，不改你的桌面。
fn set_wallpaper(ui: &Rc<Ui>, path: &str) {
    if selftest::on_external(ui, "wallpaper", path) {
        return;
    }
    if crate::sandbox::in_flatpak() {
        set_wallpaper_by_portal(ui, path);
        return;
    }
    let plasma = std::process::Command::new("plasma-apply-wallpaperimage")
        .arg(path)
        .status()
        .is_ok_and(|s| s.success());
    let ok = plasma || {
        let uri = gio::File::for_path(path).uri();
        ["picture-uri", "picture-uri-dark"].iter().all(|key| {
            std::process::Command::new("gsettings")
                .args(["set", "org.gnome.desktop.background", key, uri.as_str()])
                .status()
                .is_ok_and(|s| s.success())
        })
    };
    if ok {
        ui.show_notice(ui.text(Key::NoticeWallpaperSet));
    } else {
        let message = "plasma-apply-wallpaperimage / gsettings";
        ui.show_notice(&ui.text(Key::NoticeFileFailed).replace("{message}", message));
    }
}

/// 壁纸门户 org.freedesktop.portal.Wallpaper.SetWallpaperFile：把打开的文件交给系统（第一次系统先问用户允不允许）。
/// 结果从这次请求的 Response 信号来（0 成功，1 用户取消，2 其他）：先按门户的约定算出请求的路径、订阅它，
/// 再发请求，免得错过回话。用户在询问里点“不允许”、以前不允许过时门户回的也是 2（xdg-desktop-portal 1.22），
/// 所以 2 提示可能没有允许、到哪里改，不当成程序出错。
fn set_wallpaper_by_portal(ui: &Rc<Ui>, path: &str) {
    fn failed(ui: &Ui, message: &str) {
        ui.show_notice(&ui.text(Key::NoticeFileFailed).replace("{message}", message));
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) => return failed(ui, &e.to_string()),
    };
    let connection = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(connection) => connection,
        Err(e) => return failed(ui, e.message()),
    };
    thread_local! {
        static NEXT: Cell<u32> = const { Cell::new(0) };
    }
    let token = format!("anyview_wallpaper_{}", NEXT.with(|n| {
        n.set(n.get() + 1);
        n.get()
    }));
    let sender = connection
        .unique_name()
        .map(|name| name.trim_start_matches(':').replace('.', "_"))
        .unwrap_or_default();
    let request = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    // 订阅放在自己的回调能拿到的地方，收到回话后丢掉（取消订阅）。
    let subscription: Rc<RefCell<Option<gio::SignalSubscription>>> = Rc::default();
    let keep = subscription.clone();
    let weak = Rc::downgrade(ui);
    *subscription.borrow_mut() = Some(connection.subscribe_to_signal(
        Some("org.freedesktop.portal.Desktop"),
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(&request),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let response = signal.parameters.child_value(0).get::<u32>().unwrap_or(2);
            if let Some(ui) = weak.upgrade() {
                match response {
                    0 => ui.show_notice(ui.text(Key::NoticeWallpaperSet)),
                    1 => {}
                    _ => ui.show_notice(ui.text(Key::NoticeWallpaperNotSet)),
                }
            }
            keep.borrow_mut().take();
        },
    ));
    let options = glib::VariantDict::new(None);
    options.insert_value("handle_token", &token.to_variant());
    options.insert_value("show-preview", &false.to_variant());
    options.insert_value("set-on", &"background".to_variant());
    // (父窗口, 文件, 选项)：父窗口留空。用 tuple_from_iter，免得选项被包成 v。
    let parameters = glib::Variant::tuple_from_iter([
        "".to_variant(),
        glib::variant::Handle(0).to_variant(),
        options.end(),
    ]);
    let fds = gio::UnixFDList::from_array([file]);
    let weak = Rc::downgrade(ui);
    connection.call_with_unix_fd_list(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Wallpaper",
        "SetWallpaperFile",
        Some(&parameters),
        Some(glib::VariantTy::new("(o)").expect("variant type")),
        gio::DBusCallFlags::NONE,
        -1,
        Some(&fds),
        gio::Cancellable::NONE,
        move |result| {
            if let Err(e) = result {
                subscription.borrow_mut().take();
                if let Some(ui) = weak.upgrade() {
                    failed(&ui, e.message());
                }
            }
        },
    );
}

/// 打印：当前图片（按 EXIF 方向，不含反相等效果）按纸张的可打印区域等比放到最大、居中，
/// 用系统的打印对话框。自测时只记下来。
fn print_image(ui: &Rc<Ui>, name: &str) {
    if selftest::on_external(ui, "print", name) {
        return;
    }
    let Some(image) = ui.canvas.image() else {
        return;
    };
    // 按 EXIF 方向的原图（缓存里没有效果的版本），没有时用正在显示的。
    let source = ui.core.original_image().unwrap_or(image);
    let operation = gtk::PrintOperation::new();
    operation.set_n_pages(1);
    operation.set_job_name(name);
    operation.connect_draw_page(move |_, context, _| {
        let width = source.width as i32;
        let height = source.height as i32;
        let row = source.width as usize * 4;
        let mut data = Vec::with_capacity(row * source.height as usize);
        for y in 0..source.height as usize {
            data.extend_from_slice(&source.pixels[y * source.stride..y * source.stride + row]);
        }
        let Ok(surface) = gtk::cairo::ImageSurface::create_for_data(
            data,
            gtk::cairo::Format::ARgb32,
            width,
            height,
            row as i32,
        ) else {
            return;
        };
        let cr = context.cairo_context();
        let (page_w, page_h) = (context.width(), context.height());
        let scale = (page_w / f64::from(width)).min(page_h / f64::from(height));
        cr.translate(
            (page_w - f64::from(width) * scale) / 2.0,
            (page_h - f64::from(height) * scale) / 2.0,
        );
        cr.scale(scale, scale);
        let _ = cr.set_source_surface(&surface, 0.0, 0.0);
        let _ = cr.paint();
    });
    let _ = operation.run(gtk::PrintOperationAction::PrintDialog, Some(&ui.window));
}

/// "关于"：程序名、版本，以及两个帮助窗口的按钮（Linux 没有菜单栏里的"帮助"）。
fn show_about(ui: &Rc<Ui>) {
    crate::help_window::show_about(ui);
}
