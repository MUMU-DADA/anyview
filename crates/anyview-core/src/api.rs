//! 核心与界面之间的接口（docs/ARCHITECTURE.md"核心与界面的接口"）。
//!
//! - 界面调用 [`AppCore::dispatch`] 发命令，立即返回，不等待读取、解码和扫描。
//! - 核心在后台线程产生事件，放入队列；队列由空变为非空时调用一次 [`CoreListener::wake`]。
//! - 界面收到唤醒后，在自己的主线程调用 [`AppCore::drain_events`] 取出全部事件。
//!
//! 唤醒是合并的：队列非空期间新增的事件不再重复唤醒，由尚未执行的那次读取一并取走，所以不会丢事件。
//!
//! 每次切换当前图片，代次加一（[`Event::Current`]）。图像就绪事件带着发出时的代次；
//! 界面只显示与最近一次 `Current` 代次相同的图像，旧图永远不会覆盖新图。
//! 大 JPEG 先送快速预览（[`Event::ImagePreview`]），清晰图随后以 `ImageUpdated` 送达。
//! 锁的顺序固定为 state → cache → events，避免死锁。查看状态 view 只在界面线程使用，不与其他锁同时持有。
//!
//! 查看计算是同步的：界面调用 `view_*` 方法立即得到新的 [`Layout`]。
//! 菜单、按键都通过 [`AppCore::invoke_action`] 执行动作。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::actions::{self, Chord};
use crate::cache::{DEFAULT_BUDGET, ImageCache};
use crate::collection::{self, Entry, SortKey, SortOrder};
use crate::decode::{self, DecodeOptions, DecodedImage};
use crate::effects::{self, Effects};
use crate::exact;
use crate::i18n::Lang;
use crate::overlay::{self, ImageFacts, InfoSource};
use crate::platform::Platform;
use crate::pool::{Pool, Priority};
use crate::settings::{self, Boundary, Draft, Settings, Value};
use crate::view::{DisplayMode, EdgeMode, Interpolation, Layout, SwipePhase, ViewState};

/// 界面实现的唤醒回调。核心会在任意后台线程调用它，实现方负责转到主线程。
pub trait CoreListener: Send + Sync {
    fn wake(&self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Navigation {
    Next,
    Prev,
    First,
    Last,
    Forward10,
    Back10,
    /// 跳到指定序号（从 0 开始；进度条）。超出范围时停在末张。
    Jump(usize),
}

/// 界面发给核心的命令。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// 让核心在后台线程回一个 `Pong`，用来验证命令、事件和唤醒的完整往返。
    Ping {
        token: u64,
    },
    /// 打开图片文件或文件夹。
    Open {
        path: String,
    },
    Navigate(Navigation),
    /// 关闭当前图片，回到空态。
    Close,
    /// 界面已把文件移到废纸篓或回收站：从浏览集合中去掉，显示下一张（已是最后一张则显示上一张）。
    Removed {
        path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    AtFirst,
    AtLast,
    EmptyFolder,
    /// 没有下一个（上一个）含图片的文件夹了。
    LastFolder,
    FirstFolder,
    /// 没有可以退回的随机跳转。
    NoRandomBack,
    /// 压缩包加了密码，打不开。
    ArchivePassword,
    /// 压缩包里没有可以打开的图片。
    EmptyArchive,
    /// 复制、移动到图片文件夹完成：`folder` 是文件夹名，`name` 是新文件名（重名时加了序号）。
    Copied { folder: String, name: String },
    Moved { folder: String, name: String },
    /// 还没有设置图片文件夹。
    FolderNotSet,
    /// 文件操作失败。
    FileFailed { message: String },
}

impl NoticeKind {
    /// 提示的文字（界面直接显示）。
    pub fn text(&self, lang: Lang) -> String {
        use crate::i18n::{Key, text};
        match self {
            NoticeKind::AtFirst => text(Key::NoticeAtFirst, lang).to_string(),
            NoticeKind::AtLast => text(Key::NoticeAtLast, lang).to_string(),
            NoticeKind::EmptyFolder => text(Key::NoticeEmptyFolder, lang).to_string(),
            NoticeKind::LastFolder => text(Key::NoticeLastFolder, lang).to_string(),
            NoticeKind::FirstFolder => text(Key::NoticeFirstFolder, lang).to_string(),
            NoticeKind::NoRandomBack => text(Key::NoticeNoRandomBack, lang).to_string(),
            NoticeKind::ArchivePassword => text(Key::NoticeArchivePassword, lang).to_string(),
            NoticeKind::EmptyArchive => text(Key::NoticeEmptyArchive, lang).to_string(),
            NoticeKind::Copied { folder, name } => text(Key::NoticeCopiedTo, lang)
                .replace("{folder}", folder)
                .replace("{name}", name),
            NoticeKind::Moved { folder, name } => text(Key::NoticeMoved, lang)
                .replace("{folder}", folder)
                .replace("{name}", name),
            NoticeKind::FolderNotSet => text(Key::NoticeFolderNotSet, lang).to_string(),
            NoticeKind::FileFailed { message } => {
                text(Key::NoticeFileFailed, lang).replace("{message}", message)
            }
        }
    }
}

/// 核心发给界面的事件。
#[derive(Clone, Debug)]
pub enum Event {
    Pong {
        token: u64,
    },
    /// 当前图片变了。图像随后由 `ImageReady` 送达（有缓存时紧跟着送达）。
    /// 文件夹还没扫描完时 `index`、`total` 为空。`opened`：由"打开"引起（不是浏览中切图），
    /// 界面在图像送达时按图片调整窗口大小。
    Current {
        generation: u64,
        path: PathBuf,
        name: String,
        index: Option<usize>,
        total: Option<usize>,
        opened: bool,
    },
    /// 浏览集合扫描完成。
    Collection {
        generation: u64,
        index: Option<usize>,
        total: usize,
    },
    ImageReady {
        generation: u64,
        path: PathBuf,
        image: Arc<DecodedImage>,
    },
    /// 快速预览：`image` 是 EXIF 内嵌的小缩略图，界面把它拉伸到原图尺寸 `width` x `height`
    /// 显示，与 `ImageReady` 一样复位查看状态。清晰图解码完成后以 `ImageUpdated` 送达。
    ImagePreview {
        generation: u64,
        path: PathBuf,
        image: Arc<DecodedImage>,
        width: u32,
        height: u32,
    },
    /// 同一张图片的显示缓冲区换了（反相、Gamma 改变，或清晰图替换快速预览）：界面只换纹理，不复位查看状态。
    ImageUpdated {
        generation: u64,
        path: PathBuf,
        image: Arc<DecodedImage>,
    },
    /// 动图的一帧：界面只换纹理，不复位查看状态。`index` 从 0 起；
    /// `count` 是总帧数，第一遍还没播完时可能为空。
    AnimationFrame {
        generation: u64,
        path: PathBuf,
        image: Arc<DecodedImage>,
        index: usize,
        count: Option<usize>,
    },
    /// 精确重绘完成：`image` 放在视口物理像素 (x, y) 处，盖在交互路径的画面上。
    /// 只有 `signature` 与当前 [`AppCore::view_signature`] 相同时才有效。
    ExactReady {
        generation: u64,
        signature: u64,
        x: i64,
        y: i64,
        image: Arc<DecodedImage>,
        render_ms: f64,
    },
    ImageFailed {
        generation: u64,
        path: PathBuf,
        message: String,
    },
    /// 当前图片被安全上限拦下了（解码内存、解压后的大小等）：界面问用户要不要仍然打开
    /// （[`crate::i18n::limit_prompt`]），按回答调 [`AppCore::open_without_limits`] 或 [`AppCore::keep_limits`]。
    ImageBlocked {
        generation: u64,
        path: PathBuf,
    },
    OpenFailed {
        path: String,
        message: String,
    },
    Notice(NoticeKind),
    /// 到达首尾（当设置为"询问"时）：界面询问转到首张（末张）、下一个（上一个）文件夹，或取消。
    /// `forward` 为真表示在末张继续向后。
    Boundary {
        forward: bool,
        /// 在压缩包里：对话框写"转到下一个压缩包"。
        archive: bool,
    },
    Closed {
        generation: u64,
    },
    /// 选择图片窗口要的缩略图好了（[`AppCore::picker_want_thumbnails`]）；生成失败时 `image` 为空。
    Thumbnail {
        path: PathBuf,
        image: Option<Arc<DecodedImage>>,
    },
    /// 背景图片（设置"更改默认背景图片"）解码好了或换了：界面重新取 [`AppCore::background_image`]。
    BackgroundChanged,
    /// 别的进程（Linux 的另一个窗口）改了设置文件，已经重新读、生效：界面像"确定"之后那样刷新。
    /// `changed` 是改了的项（不含记住的状态）。
    SettingsReloaded {
        changed: Vec<&'static str>,
    },
}

/// 需要界面完成的操作（动作的结果）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiRequest {
    OpenFileDialog,
    OpenFolderDialog,
    Quit,
    /// Esc：开着别的窗口时只关这个窗口，这是最后一个窗口时退出程序。
    CloseOrQuit,
    ToggleFullscreen,
    Minimize,
    ToggleMaximize,
    ShowAbout,
    ShowContextMenu,
    CloseWindow,
    HideApp,
    HideOthers,
    ShowAll,
    /// 把文件移到废纸篓或回收站，成功后发 [`Command::Removed`]。`confirm` 为真时先确认
    /// （设置"删除文件时确认"）。
    TrashFile {
        path: String,
        name: String,
        confirm: bool,
    },
    /// 在访达或文件管理器中显示并选中。
    RevealFile {
        path: String,
    },
    /// 把当前显示的图片（已按 EXIF 方向）和文件一起放进剪贴板。
    CopyImage {
        path: String,
    },
    /// macOS"打开最近使用 › 清除菜单"。
    ClearRecent,
    /// 打开设置窗口（已经打开时放到最前面）。
    ShowSettings,
    /// 界面相关的设置变了（如"总在最前"）：界面重新读 [`AppCore::ui_settings`]。
    ApplySettings,
    /// 用图像编辑器打开：`path` 是磁盘上的文件（压缩包里的图片是解出的副本）；
    /// `editor` 是设置里的编辑器（macOS 是 .app，Linux 是程序），为空时界面提示去设置。
    OpenInEditor {
        path: String,
        editor: String,
    },
    /// 设为桌面图片：`path` 是磁盘上的文件（压缩包里的图片先复制到程序的数据文件夹）。
    SetWallpaper {
        path: String,
    },
    /// 打印：界面打印当前图片（按 EXIF 方向，不含反相等效果），用系统的打印对话框。
    Print {
        name: String,
    },
    /// 打开选择图片窗口；已经打开时放到最前面。列表从 [`AppCore::picker_list`] 取。
    ShowPicker,
    /// 打开帮助窗口：内容从 [`AppCore::help_rows`] 取。
    ShowHelp {
        kind: crate::help::HelpKind,
    },
    /// 打开“生成信息”面板；已经打开时放到最前面。内容从 [`AppCore::generation_info`] 取，换了图片时重新取。
    ShowGenerationInfo,
}

/// 选择图片窗口的列表（[`AppCore::picker_list`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerList {
    pub rows: Vec<crate::picker::Row>,
    /// 当前图片所在的行（打开窗口时选中它）。
    pub current: Option<usize>,
    /// 排序按钮上的文字，如"按名称排序"。
    pub sort_label: String,
    /// 排序按钮的菜单：文件名、修改日期、大小、类型（单选）和降序，与右键菜单"排序方式"是同一组动作。
    pub sort_options: Vec<PickerSortOption>,
}

/// 排序菜单的一项：执行的动作、文字、是否勾选。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerSortOption {
    pub action: String,
    pub title: String,
    pub checked: bool,
}

/// 执行动作的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    /// 现在不能执行：没有图片，或尚未实现。
    Unavailable,
    /// 已开始，结果由事件送回（切换、关闭）。
    Started,
    /// 查看参数变了：界面取 [`AppCore::view_layout`] 重绘，并刷新菜单勾选。
    /// `show_zoom` 为真时在画面中央短暂显示倍率。
    ViewChanged {
        show_zoom: bool,
    },
    /// 悬浮界面的开关变了（图片信息、控制栏锁定）：界面按 `action_state` 更新显示。
    OverlayChanged,
    Ui(UiRequest),
}

/// 菜单项的状态快照（MENUS"启用规则"）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ActionState {
    pub enabled: bool,
    pub checked: bool,
}

/// 一个窗口的核心（macOS 一个程序可以开多个窗口）。同一进程里的各窗口用 [`AppCore::new_window`] 建，
/// 共用设置、图片缓存和后台线程，各自有浏览集合、当前图片和查看状态。
pub struct AppCore {
    shared: Arc<Shared>,
    platform: Platform,
}

/// 设置和设置窗口的草稿。
struct Prefs {
    settings: Settings,
    /// 设置文件；为空时不保存（还没读设置，或取不到主目录）。
    path: Option<PathBuf>,
    /// 读到的设置文件：保存时保留认不出的键。
    table: toml::Table,
    /// 设置窗口打开期间的草稿。
    draft: Option<Draft>,
    /// 启动时读设置的结果；界面提示一次后清空。
    status: Option<settings::LoadStatus>,
    /// 最近一次读、写设置文件后文件的修改时间：变了说明别的进程写过。
    stamp: Option<SystemTime>,
    /// 设置文件里现在的内容（最近一次读到、写下的）。保存以它为底，只盖上本进程改过的项。
    disk: Settings,
    /// 最近一次读、写时本进程的设置：与 `settings` 不同的项就是本进程改过、还没写下的。
    baseline: Settings,
    /// 启动时设置文件在、但读不出来（被占用、没有权限）：现在用的是默认值，文件里的内容还不知道。
    /// 读到之前不写它（不把可能完好的文件照默认值盖掉）。
    unread: bool,
}

impl Prefs {
    /// 写设置文件。没有路径时什么也不做。
    fn save(&mut self, platform: Platform) -> Result<(), String> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if self.unread {
            // 启动时没读出来：现在再读一次。还读不了就不写，把原因报给界面（"无法保存设置"）。
            let again = settings::load(&path);
            if again.unreadable {
                return Err(match again.status {
                    settings::LoadStatus::Recovered { reason, .. } => reason,
                    _ => String::new(),
                });
            }
            self.disk = again.settings;
            self.table = again.table;
            self.unread = false;
        } else if let Some((theirs, table)) = settings::read_file(&path) {
            // 每次都以文件里现在的内容为底：别的进程（Linux 的另一个窗口）写过的话，不会把它改的项写回旧值。
            // （不靠修改时间判断有没有人写过：有的文件系统修改时间只精确到秒。）
            self.disk = theirs;
            self.table = table;
        }
        let merged = settings::overlay(&self.disk, &self.settings, &self.baseline);
        let table = settings::to_table(&merged, &self.table, platform);
        settings::save(&path, &table).map_err(|e| e.to_string())?;
        self.table = table;
        self.baseline = self.settings.clone();
        // 文件里有别的进程改的项、本进程还没跟上时不更新时间：共享设置开着的话下一次检查会重新读进来。
        if settings::differences(&merged, &self.settings).is_empty() {
            self.stamp = modified_time(&path);
        }
        self.disk = merged;
        Ok(())
    }
}

/// 背景图片：在用的文件、解码好的图（缩到长边不超过 [`BACKGROUND_MAX`]）、是否正在解码。
#[derive(Default)]
struct BackgroundSlot {
    path: Option<PathBuf>,
    image: Option<Arc<DecodedImage>>,
}

/// 背景图片的长边上限（像素）：铺满窗口用，4K 屏也够了。
const BACKGROUND_MAX: u32 = 3840;

/// 后台任务要用的设置：从 [`Settings`] 取出，放在 state 锁里。
#[derive(Clone, Copy, Debug)]
struct Runtime {
    decode: DecodeOptions,
    /// 先显示低品质图片。
    fast_preview: bool,
    /// 预先读好接下来的图片。
    prefetch: bool,
    /// 往翻的方向预读几张。
    prefetch_count: usize,
    /// 到达首尾时。
    boundary: Boundary,
    /// 在压缩包里到达首尾时。
    archive_boundary: Boundary,
    /// 记住压缩包里看到哪一张。
    remember_archive: bool,
    /// RAW 文件的筛选。
    raw: collection::RawFilter,
    /// 使用监视器的 ICC 设置。
    monitor_icc: bool,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::from_settings(&Settings::defaults())
    }
}

impl Runtime {
    fn from_settings(s: &Settings) -> Self {
        Self {
            decode: DecodeOptions {
                auto_orient: s.bool(settings::EXIF_ROTATE),
                use_icc: s.bool(settings::FILE_ICC),
                raw_preview: s.bool(settings::RAW_THUMBNAIL),
                generation: true,
            },
            fast_preview: s.bool(settings::FAST_PREVIEW),
            prefetch: s.bool(settings::PREFETCH),
            prefetch_count: s.int(settings::PREFETCH_COUNT).clamp(1, 20) as usize,
            boundary: s.boundary(),
            archive_boundary: s.archive_boundary(),
            remember_archive: s.bool(settings::REMEMBER_ARCHIVE),
            raw: collection::RawFilter {
                skip_raw: s.bool(settings::SKIP_RAW),
                skip_with_jpeg: s.bool(settings::SKIP_RAW_WITH_JPEG),
            },
            monitor_icc: s.bool(settings::MONITOR_ICC),
        }
    }
}

/// 界面要按设置显示的东西（背景色、隐藏控件等）。界面启动时、设置生效后读一次。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiSettings {
    /// 背景色 0xRRGGBB。
    pub background_color: u32,
    /// 32 位图片透明处的背景。
    pub alpha_background: settings::AlphaBackground,
    pub hide_control_bar: bool,
    pub hide_side_arrows: bool,
    /// 不显示缩放倍率提示。
    pub hide_zoom_info: bool,
    pub hide_tooltips: bool,
    /// 用小字体显示图片信息。
    pub small_info_font: bool,
    /// 通知显示在右上角；关掉时显示在中央。
    pub corner_notices: bool,
    /// 双击最大化、还原。
    pub double_click_maximize: bool,
    pub always_on_top: bool,
    /// 不记录最近打开的文件和文件夹。
    pub no_recent: bool,
    /// 一直显示标题栏（macOS；Linux 的标题栏由系统显示）。
    pub show_title_bar: bool,
    /// 不显示动画信息控制。
    pub hide_animation_info: bool,
    /// 看图时阻止屏幕保护程序和显示器睡眠。
    pub no_screensaver: bool,
    /// 拖动窗口时吸附屏幕边缘（macOS）。
    pub snap_to_edges: bool,
    /// 从访达（文件管理器）打开图片时交给已经开着的窗口，不开新窗口。
    pub reuse_window: bool,
    /// 标题栏隐藏时在窗口顶端显示文件名（macOS、Windows）。
    pub title_filename: bool,
    /// 不显示窗口顶端的菜单栏（Windows；别的平台没有这一项，总是 true）。
    pub hide_menu_bar: bool,
    /// 控制栏的悬停效果（macOS；别的平台没有这一项，总是淡色亮底）。
    pub bar_hover_effect: settings::BarHoverEffect,
}

/// 设置窗口的一页。
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsPageView {
    pub id: &'static str,
    /// 左侧列表里的名称，也是页顶上的大标题。
    pub title: &'static str,
    /// 大标题下面的一段说明（键盘、鼠标页）；没有时为空。
    pub lead: &'static str,
    /// 各张卡片上面的组名（没有组名的卡片为空），[`settings::ItemView::section`] 是这里的下标。
    pub sections: Vec<String>,
    pub items: Vec<settings::ItemView>,
}

/// "确定"成功后的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsApplied {
    /// 改了的项。
    pub changed: Vec<&'static str>,
    /// 写设置文件失败的原因（设置已经生效，只是下次启动不会保留）。
    pub save_error: Option<String>,
}

/// 同一进程里各窗口共用的东西：设置、图片缓存、后台线程。
struct Hub {
    platform: Platform,
    /// 设置（docs/SETTINGS.md）。只在界面线程使用；后台任务要用的值另存在各窗口的 [`State::opts`]。
    /// 锁的顺序：prefs 不与其他锁同时持有。
    prefs: Mutex<Prefs>,
    cache: Mutex<ImageCache>,
    pool: Pool,
    /// 压缩包内位置的记忆。锁的顺序：state → positions。
    positions: Mutex<crate::positions::Positions>,
    /// 最近打开的（菜单栏的"打开最近使用"）。锁的顺序：不与其他锁同时持有。
    recent: Mutex<crate::recent::Recent>,
    /// 记不记最近打开的：只在 Windows 上记（macOS 用系统的"最近使用"，Linux 没有菜单栏）。
    track_recent: AtomicBool,
    /// 界面现在用的语言（[`crate::i18n::ALL_LANGS`] 里的位置）：界面每次问 [`AppCore::ui_lang`] 时记下，
    /// 后台线程写出错原因时用（那时不能去锁 prefs）。
    lang: std::sync::atomic::AtomicU8,
    /// 各窗口（设置生效时逐个应用）。窗口关掉后自然失效。
    windows: Mutex<Vec<Weak<Shared>>>,
    next_window: AtomicU64,
    /// 选择图片窗口的缩略图（各窗口共用）。锁的顺序：不与其他锁同时持有。
    thumbs: Mutex<crate::picker::ThumbCache>,
    /// 背景图片。锁的顺序：不与其他锁同时持有。
    background: Mutex<BackgroundSlot>,
    /// 叫醒后台检查线程（[`start_watcher`]）。
    watch: Arc<WatchSignal>,
}

impl Hub {
    /// 界面现在用的语言。
    fn lang(&self) -> Lang {
        crate::i18n::ALL_LANGS
            .get(usize::from(self.lang.load(AtomicOrdering::Relaxed)))
            .copied()
            .unwrap_or(Lang::En)
    }
}

fn lang_index(lang: Lang) -> u8 {
    crate::i18n::ALL_LANGS.iter().position(|l| *l == lang).unwrap_or(0) as u8
}

impl Drop for Hub {
    fn drop(&mut self) {
        // 窗口都关了：后台检查线程不用等到下一轮才发现，现在就结束。
        self.watch.poke();
    }
}

/// 叫醒后台检查线程：平时它每隔 [`WATCH_INTERVAL`] 看一轮；刚显示了图片时叫它马上核对一次文件
/// （显示的可能是缓存里的旧图：解码之后文件被改过）。
#[derive(Default)]
struct WatchSignal {
    poked: Mutex<bool>,
    wake: std::sync::Condvar,
}

impl WatchSignal {
    fn poke(&self) {
        *lock(&self.poked) = true;
        self.wake.notify_one();
    }

    /// 等到被叫醒（返回 true）或到了 `deadline`（返回 false）。
    fn wait_until(&self, deadline: Instant) -> bool {
        let mut poked = lock(&self.poked);
        while !*poked {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            poked = self
                .wake
                .wait_timeout(poked, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        *poked = false;
        true
    }
}

impl Hub {
    /// 还开着的窗口。
    fn live_windows(&self) -> Vec<Arc<Shared>> {
        let mut windows = lock(&self.windows);
        windows.retain(|w| w.strong_count() > 0);
        windows.iter().filter_map(Weak::upgrade).collect()
    }
}

/// 一个窗口的状态（后台任务也要用）。
struct Shared {
    /// 窗口编号（缓存按它登记要保护的图片）。
    id: u64,
    hub: Arc<Hub>,
    events: Mutex<VecDeque<Event>>,
    listener: Arc<dyn CoreListener>,
    state: Mutex<State>,
    view: Mutex<ViewState>,
    /// 正在播放的动图（每个窗口最多一个）。锁的顺序：state → anim。
    anim: Mutex<Option<AnimHandle>>,
    /// 选择图片窗口现在要的缩略图（看得见的几张）和正在生成的。锁的顺序：state → picker。
    picker: Mutex<PickerWants>,
}

/// 选择图片窗口要的缩略图：界面每次给出看得见的那几张，替换上一次的；后台任务开始前看自己还要不要。
#[derive(Default)]
struct PickerWants {
    wanted: HashSet<PathBuf>,
    in_flight: HashSet<PathBuf>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        // 窗口关掉了：它的图片不再受保护，可以被别的窗口挤掉。
        lock(&self.hub.cache).forget(self.id);
        if let Some(handle) = lock(&self.anim).take() {
            handle.control.stop();
        }
    }
}

/// 动图的播放：播放线程和界面（动作）之间共享的控制。
struct AnimHandle {
    /// 属于哪一代次的当前图片。
    generation: u64,
    control: Arc<AnimControl>,
}

#[derive(Default)]
struct AnimControl {
    state: Mutex<AnimState>,
    wake: std::sync::Condvar,
}

#[derive(Default)]
struct AnimState {
    /// 确认是动图（读到了第二帧）。
    animated: bool,
    paused: bool,
    stop: bool,
    /// 暂停时逐帧：+1 下一帧，-1 上一帧。
    step: i32,
    /// 效果（反相、Gamma）变了：按新效果重送当前帧。
    refresh: bool,
    /// 正在显示第几帧、一共几帧（第一遍没播完时为空）。
    index: usize,
    count: Option<usize>,
}

impl AnimControl {
    fn stop(&self) {
        lock(&self.state).stop = true;
        self.wake.notify_all();
    }

    fn change(&self, f: impl FnOnce(&mut AnimState)) {
        f(&mut lock(&self.state));
        self.wake.notify_all();
    }
}

/// 动图的播放状态（动画信息控制）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationInfo {
    /// 正在显示的帧（从 0 起）。
    pub index: usize,
    /// 总帧数；第一遍还没播完时可能为空。
    pub count: Option<usize>,
    pub playing: bool,
}

#[derive(Default)]
struct State {
    generation: u64,
    folder: Option<PathBuf>,
    entries: Vec<Entry>,
    /// 当前文件夹已扫描完成，`entries` 有效。
    scanned: bool,
    /// 每次开始新的扫描加一；旧扫描的结果直接丢弃。
    scan_token: u64,
    /// 每次"打开"（文件、文件夹、压缩包）加一：还在后台读的文件夹、压缩包是更早那次打开的，读好了也不用。
    open_token: u64,
    index: Option<usize>,
    current: Option<PathBuf>,
    /// 排着队或正在解码的文件（每个文件同时只有一次请求）。
    in_flight: HashMap<PathBuf, Flight>,
    /// 解码请求的编号（见 [`Flight::ticket`]）。
    next_ticket: u64,
    /// 排序方式（右键菜单"排序方式"）。
    sort: SortOrder,
    /// 最近看过的图片，随机跳转时避开。
    recent: VecDeque<PathBuf>,
    /// 每次随机跳转前的图片，"向前随机"依次退回。
    random_back: Vec<PathBuf>,
    /// 随机数状态（xorshift），0 表示还没取种子。
    rng: u64,
    /// 反相、Gamma（切图后保持）；显示器配置文件（设置打开时为 `display_profile`）。
    effects: Effects,
    /// 界面报来的这个窗口所在显示器的配置文件；"使用监视器的 ICC 设置"关着时只记下、不用。
    display_profile: Option<effects::DisplayProfileId>,
    /// 动图最近送出的那一帧（不含效果）和它的代次：复制、打印用当前这一帧，不是第一帧。
    current_frame: Option<(u64, Arc<DecodedImage>)>,
    /// 界面最近一次是为哪一代次、按多大的图排的版（显示了图片，或它的快速预览，预览记的是原图尺寸）：
    /// 同一代次、同样尺寸的再送来只换内容（`ImageUpdated`），别的都按新图片显示（`ImageReady`）。
    /// 这一代次报了解码失败（或被上限拦下）时记 (代次, 0, 0)。
    shown: Option<(u64, u32, u32)>,
    /// 后台任务要用的设置。
    opts: Runtime,
    /// 正在浏览的压缩包：浏览集合是它里面的图片，`folder` 是压缩包文件。
    archive: Option<Arc<crate::archive::Archive>>,
    /// 智能双页阅览：阅读方向；关掉时为空。
    two_page: Option<crate::spread::Direction>,
    /// 这一组以当前图片结尾（往回翻、末张时），否则以它开头。
    spread_backward: bool,
    /// 下一次切图时的 `spread_backward`（由导航设定，切图时取走）。
    next_backward: bool,
    /// 双页时这一代次显示着的一组；还没显示时为空。
    spread: Option<Spread>,
    /// 双页时正在后台合成的一组（同一组不重复合成；合成好时对不上就不要了）。
    composing: Option<Composing>,
    /// 这一次切图以来读不了的图片（双页时缺的那一页按单页显示），和读之前文件的修改时间、大小
    /// （文件不在、压缩包里的没有）。每次切到一张图片，它和双页的另一页都重新试。
    failed: HashMap<PathBuf, Option<FileStamp>>,
    /// 被上限拦下、用户选了"取消"的图片：再翻到时不再问，按解码失败提示。
    declined: HashSet<PathBuf>,
    /// 文件夹变化自动刷新：在看的文件夹和上次扫描时它的修改时间。
    watched: Option<(PathBuf, Option<SystemTime>)>,
    /// 还没到装列表那一步的重扫：它是为哪一次 `scan_token` 扫的、它自己的编号。这之前不再排一个。
    rescanning: Option<(u64, u64)>,
    /// 因为文件改了而重新显示的那一代次：这一代次显示出来时不马上再核对，等下一轮
    /// （文件一直在变的话，最多每轮重读一次，不是解多快就重读多快）。
    reloaded: Option<u64>,
    /// 显示着的图片文件（双页时两页都在）和显示的那一份解码时文件的修改时间、大小：文件之后变了就重新读、
    /// 重新显示（如用图像编辑器改完保存）。压缩包里的图片不看；还没显示出来时为空。
    watched_files: Vec<(PathBuf, FileStamp)>,
    /// 下一次切图是翻页（往后翻为真）：由导航在切图前设定，切图时取走。"打开"、刷新、删除后的下一张等不设。
    direction_hint: Option<bool>,
    /// 当前代次是翻页切过来的。界面据此做翻页效果、放音效（[`AppCore::transition_for`]）。
    turn: Option<Turn>,
    /// 往回翻着看（最近一次翻页是往回的）：预读往回的方向。“打开”时回到往后。
    heading_back: bool,
    /// 预读范围最多几张（含当前图片），按缓存大小算（[`preload_limit`]）；为空时不限（还不知道图片多大）。
    preload_limit: Option<usize>,
    /// 这个窗口最近解开的一张图片占多少字节：用来估还没读的图片多大（[`preload_limit`]）。
    last_bytes: usize,
    /// 读的期间文件变了、解不出来而重新读过几次（[`decode_job`]）；每次切图清零。
    rereads: HashMap<PathBuf, u32>,
}

/// 一次解码请求。
struct Flight {
    /// 每次请求一个新编号。解码任务开始、结束时都核对：对不上（文件改了重新请求过、解码方式变了、
    /// 文件删了）就不做、结果也不用，旧的结果不会盖住新的。
    ticket: u64,
    /// 排队用的优先级（请求当预加载排着、后来成了当前图片时，按新的优先级再排一个任务）。
    priority: Priority,
    /// 已经有任务在解了（同一次请求的别的任务轮到时不再做）。
    started: bool,
}

/// 文件的修改时间和大小。
type FileStamp = (Option<SystemTime>, u64);

/// 双页时显示着的一组。
struct Spread {
    /// 这一组的几页（一页或两页，按列表里的先后）。按路径记：列表排序、刷新、删掉文件后序号会变。
    /// 一个代次里显示了哪几页就一直是这几页，列表变了也等翻页时再按新的列表成组。
    paths: Vec<PathBuf>,
    /// 显示的图片（两页时是合成好的）和它用的效果。
    image: Arc<DecodedImage>,
    effects: Effects,
    /// 本来该一起显示、但读不了的另一页，和读之前文件的修改时间、大小：文件之后变了（比如刚才正存到一半）
    /// 就重新成组。
    missing: Option<(PathBuf, FileStamp)>,
}

/// 双页时这一组要显示的东西（[`resolve_spread`]）。
struct Resolved {
    paths: Vec<PathBuf>,
    images: Vec<Arc<DecodedImage>>,
    missing: Option<(PathBuf, FileStamp)>,
}

/// 双页时在后台合成的一组：哪一代次、哪两页、什么效果、什么方向。
#[derive(Clone, PartialEq)]
struct Composing {
    generation: u64,
    paths: Vec<PathBuf>,
    effects: Effects,
    direction: crate::spread::Direction,
}

/// 翻页切过来的那一代次。
struct Turn {
    generation: u64,
    /// 往后翻。
    forward: bool,
    /// 界面已经问过一次（[`AppCore::transition_for`]）：音效只给第一次。
    asked: bool,
}

/// 文件夹变化自动刷新多久看一次：只比较文件夹和当前文件的修改时间，几乎不占 CPU。
const WATCH_INTERVAL: Duration = Duration::from_millis(1500);

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// 后台每隔 `interval` 看一轮：设置文件、各窗口的文件夹和图片文件有没有变；被叫醒时
/// （[`WatchSignal::poke`]，刚显示了图片）马上核对各窗口的图片文件，别的等到点再看。程序的窗口都关了就结束。
/// 这些读磁盘的事都在这个线程上做：界面线程翻页时不查文件。
fn start_watcher(hub: &Arc<Hub>, interval: Duration) {
    let weak = Arc::downgrade(hub);
    let signal = Arc::clone(&hub.watch);
    let _ = thread::Builder::new()
        .name("anyview-watch".into())
        .spawn(move || {
            // 整轮检查按自己的时间走：一直在翻页（一直被叫醒）也照样每隔 `interval` 看一轮。
            let mut next_round = Instant::now() + interval;
            loop {
                let poked = signal.wait_until(next_round);
                let Some(hub) = weak.upgrade() else {
                    return;
                };
                let round = !poked || Instant::now() >= next_round;
                if round {
                    next_round = Instant::now() + interval;
                    check_settings_file(&hub);
                }
                for window in hub.live_windows() {
                    check_window(&window, round);
                }
            }
        });
}

/// 多窗口共享设置（设置 › 书签"多窗口共享设置"）：Linux 每个窗口是一个进程，别的进程写了设置文件时
/// 这里重新读，改了的项像"确定"一样生效，通知界面刷新。设置窗口开着（有草稿）时先不读，关了再说；
/// 关掉这一项时不读（各进程用自己启动时的设置）。记住的状态（state.*）不跟着变。
fn check_settings_file(hub: &Arc<Hub>) {
    let (path, stamp) = {
        let prefs = lock(&hub.prefs);
        if prefs.draft.is_some() || !prefs.settings.bool(settings::SHARE_SETTINGS) {
            return;
        }
        let Some(path) = prefs.path.clone() else {
            return;
        };
        (path, prefs.stamp)
    };
    let now = modified_time(&path);
    if now.is_none() || now == stamp {
        return;
    }
    let Some((fresh, table)) = settings::read_file(&path) else {
        return;
    };
    let (changed, current) = {
        let mut prefs = lock(&hub.prefs);
        if prefs.draft.is_some() {
            return;
        }
        // 文件里的值生效；本进程改了还没存下的项（上次保存失败）留着，下次保存再写。
        // 记住的状态（state.*）不跟着别的窗口变：留本进程的。
        let merged = settings::overlay(&fresh, &prefs.settings, &prefs.baseline);
        let merged = settings::with_state_of(&merged, &prefs.settings);
        let changed = settings::differences(&prefs.settings, &merged);
        prefs.baseline = settings::with_state_of(&fresh, &prefs.baseline);
        prefs.settings = merged;
        prefs.disk = fresh;
        prefs.unread = false;
        prefs.table = table;
        prefs.stamp = now;
        (changed, prefs.settings.clone())
    };
    if changed.is_empty() {
        return;
    }
    apply_runtime_to(hub, &current);
    for window in hub.live_windows() {
        window.push(Event::SettingsReloaded {
            changed: changed.clone(),
        });
    }
}

/// 文件现在的修改时间和大小；读不到（文件不在了：等文件夹刷新处理）时为空。
fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

/// 看一个窗口的文件有没有改过（都在后台检查线程上，读磁盘不占界面线程）：
/// - 显示着的图片：改过就重新读、重新显示；
/// - 缓存里、预加载范围内的图片：改过的丢掉重新读，翻到时直接是新的（不用先显示旧的再换）；
/// - `folders` 为真（整轮检查）时也看文件夹。
fn check_window(shared: &Arc<Shared>, folders: bool) {
    let (folder, shown, nearby, check_shown) = {
        let st = lock(&shared.state);
        let nearby = if st.archive.is_some() {
            Vec::new() // 压缩包里的图片不看
        } else {
            // 只看缓存里有的（没有的翻到时现读，用不着核对）。锁的顺序：state → cache。
            let cache = lock(&shared.hub.cache);
            let mut nearby = st.window();
            nearby.retain(|path| cache.has(path));
            nearby
        };
        (
            folders.then(|| st.watched.clone()).flatten(),
            st.watched_files.clone(),
            nearby,
            // 刚因为文件改了重新显示的：等整轮检查再看（见 [`State::reloaded`]）。
            folders || st.reloaded != Some(st.generation),
        )
    };
    if let Some((dir, seen)) = folder {
        let now = modified_time(&dir);
        if now != seen {
            rescan_folder(shared, dir, now);
        }
    }
    let changed: Vec<(PathBuf, FileStamp, FileStamp)> = shown
        .iter()
        .filter(|_| check_shown)
        .filter_map(|(path, seen)| {
            let now = file_stamp(path)?;
            (now != *seen).then(|| (path.clone(), *seen, now))
        })
        .collect();
    if !changed.is_empty() {
        reload_changed(shared, &changed);
        return; // 重新显示了（新的代次）：它显示出来时会再叫这里核对
    }
    for path in nearby {
        if shown.iter().any(|(p, _)| *p == path) {
            continue;
        }
        if let Some(now) = file_stamp(&path) {
            refresh_cached(shared, &path, now);
        }
    }
}

/// 缓存里这张图片是文件改动之前解的：丢掉；还用得着（当前图片还没显示出来、双页的另一页、预加载范围里）就重新读。
fn refresh_cached(shared: &Arc<Shared>, path: &Path, now: FileStamp) {
    let mut st = lock(&shared.state);
    if !lock(&shared.hub.cache).remove_stale(path, now.0, now.1) {
        return;
    }
    // 正拿着旧的这一份合成双页：那一张不要了，等新的读好再合成。
    if st.composing.as_ref().is_some_and(|c| c.paths.iter().any(|p| p == path)) {
        st.composing = None;
    }
    if !wanted(&st, path) {
        return;
    }
    let page = st.current.as_deref() != Some(path) && is_spread_page(&st, path);
    let priority = if page || st.current.as_deref() == Some(path) {
        Priority::Current
    } else {
        Priority::Prefetch
    };
    // 双页的另一页就在这一会儿又有了（别的窗口共用缓存，刚读好放进去）：没有任务会来叫，这里接着成组。
    if request_display(shared, &mut st, path.to_path_buf(), priority) && page {
        push_spread(shared, &mut st);
    }
}

/// 重扫没走到装列表那一步就结束了（文件夹读不了、任务出错）：也要让之后可以再扫。
/// 只取消自己那一次的记号（按编号认），不碰后来排上的。
struct RescanDone(Arc<Shared>, (u64, u64));

impl Drop for RescanDone {
    fn drop(&mut self) {
        let mut st = lock(&self.0.state);
        if st.rescanning == Some(self.1) {
            st.rescanning = None;
        }
    }
}

/// 文件夹变了：重新扫描，浏览集合换成新的列表；当前图片还在就不动，只更新序号；
/// 被删掉了就显示同一位置的那张（已是最后则显示上一张），没有图片了回到空态。
fn rescan_folder(shared: &Arc<Shared>, dir: PathBuf, stamp: Option<SystemTime>) {
    let (order, token, mine) = {
        let mut st = lock(&shared.state);
        // 上一次重扫还在读文件夹（文件很多、磁盘很慢，一次要好几秒）：不再排一个，免得每隔一会儿多一个、越积越多。
        // 它读完、装列表时用的是到那时的排序方式和 RAW 筛选；期间文件夹又变了的话，它记下的修改时间
        // 是扫描之前的，下一轮检查还会发现。
        if st.rescanning.is_some_and(|(token, _)| token == st.scan_token) {
            return;
        }
        st.next_ticket += 1;
        let mine = (st.scan_token, st.next_ticket);
        st.rescanning = Some(mine);
        (st.sort, st.scan_token, mine)
    };
    let s = Arc::clone(shared);
    shared.hub.pool.submit(Priority::Scan, move || {
        // 先声明、最后才放掉（在状态锁之后）：它放掉时要取状态锁。
        let _done = RescanDone(Arc::clone(&s), mine);
        let Ok(mut entries) = collection::scan_folder(&dir, order) else {
            return;
        };
        let mut st = lock(&s.state);
        // 从这里起用的是现在的排序方式、RAW 筛选；之后再有人要重扫（比如刚好这时改了 RAW 筛选）得另排一个，
        // 所以"还没完"在这同一次加锁里就取消，不等任务结束。
        if st.rescanning == Some(mine) {
            st.rescanning = None;
        }
        // 期间打开了别的文件夹、压缩包：这份旧文件夹的列表不能装进来（会把刚打开的图片关掉）。
        if st.scan_token != token
            || st.watched.as_ref().map(|(d, _)| d) != Some(&dir)
            || st.archive.is_some()
        {
            return;
        }
        // 扫描期间改了排序方式时按新的方式重排。
        if st.sort != order {
            collection::sort_entries(&mut entries, st.sort);
        }
        collection::filter_raw(&mut entries, st.opts.raw, st.current.as_deref());
        st.watched = Some((dir, stamp));
        let same = entries.len() == st.entries.len()
            && entries.iter().zip(&st.entries).all(|(a, b)| a.path == b.path);
        if same {
            return;
        }
        let old_index = st.index;
        st.entries = entries;
        let index = st.current.clone().and_then(|c| st.position(&c));
        if index.is_some() || st.current.is_none() {
            st.index = index;
            regroup_after_list_change(&s, &mut st);
            s.push(Event::Collection {
                generation: st.generation,
                index,
                total: st.entries.len(),
            });
            return;
        }
        // 当前图片被删掉或改名了。
        let next = old_index.map(|i| i.min(st.entries.len().saturating_sub(1)));
        match next.filter(|_| !st.entries.is_empty()) {
            Some(i) => {
                let path = st.entries[i].path.clone();
                set_current(&s, &mut st, path, Some(i), false);
                // 浏览集合也变了：选择图片窗口等要按新的列表重新载入（不然按旧的序号会选到别的图）。
                s.push(Event::Collection {
                    generation: st.generation,
                    index: Some(i),
                    total: st.entries.len(),
                });
            }
            None => {
                drop(st);
                close(&s);
            }
        }
    });
}

/// 显示着的图片的文件改过了（如用图像编辑器改完保存；双页时两页里哪一页改了都算）：重新读、重新显示。
/// `changed` 的每一项：文件、检查时拿来比的那一份（显示的那一份解码时的修改时间、大小）、文件现在的。
fn reload_changed(shared: &Arc<Shared>, changed: &[(PathBuf, FileStamp, FileStamp)]) {
    let mut st = lock(&shared.state);
    // 检查期间已经换了图片，或者已经重新读过了：不用了（不然同一次改动会重读两遍）。
    if !changed
        .iter()
        .all(|(path, seen, _)| st.watched_files.iter().any(|(p, stamp)| p == path && stamp == seen))
    {
        return;
    }
    for (path, _, now) in changed {
        // 缓存里的要是已经是新的（别的窗口重新读过）就留着用。正在解的那一次读的可能是改动之前的：作废，重新请求。
        lock(&shared.hub.cache).remove_stale(path, now.0, now.1);
        st.in_flight.remove(path);
    }
    show_again(shared, &mut st);
    st.reloaded = Some(st.generation);
}

/// 当前图片重新显示一遍（文件改了、解码方式变了、解除了上限、双页时另一页不在了）：新的代次；
/// 双页时照原来的方向成组（不然往回翻出来的"上一张 + 这一张"会变成"这一张 + 下一张"）。
fn show_again(shared: &Arc<Shared>, st: &mut State) {
    let Some(path) = st.current.clone() else {
        return;
    };
    st.next_backward = st.spread_backward;
    let index = st.index;
    set_current(shared, st, path, index, false);
}

/// 最近看过的图片最多记多少张。
const RECENT_LIMIT: usize = 50;
/// 随机跳转的退回记录最多多少条。
const RANDOM_BACK_LIMIT: usize = 100;

impl State {
    fn next_random(&mut self) -> u64 {
        if self.rng == 0 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15);
            self.rng = nanos | 1;
        }
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    fn position(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|e| e.path == path)
    }

    /// 浏览集合要换了、新的列表还没有：清空列表，序号一并作废（等新列表好了再定），
    /// 旧文件夹也不再盯着（它的重扫结果不能再装进来）。
    fn clear_entries(&mut self) {
        self.entries.clear();
        self.index = None;
        self.watched = None;
    }
}

impl State {
    /// 当前图片和要预读的图片，先近后远。缓存放不下时只取前 `preload_limit` 张
    /// （当前图片、双页时一起显示的另一页总在）。
    fn window(&self) -> Vec<PathBuf> {
        self.window_and_keep().0
    }

    /// 同 [`Self::window`]，和其中一定要留的前几张（当前图片、双页时一起显示的另一页）。
    fn window_and_keep(&self) -> (Vec<PathBuf>, usize) {
        let (mut paths, keep) = self.preload_candidates();
        if let Some(limit) = self.preload_limit {
            paths.truncate(limit.max(keep));
        }
        (paths, keep)
    }

    /// 预读的候选（还没按缓存大小截），和其中一定要留的前几张。顺序：当前图片；双页时一起显示的另一页
    /// （显示着的那一组；列表排序、刷新以后它可能已经不挨着当前图片了）；然后往翻的方向一张、反方向一张
    /// （双页时反方向两张，正好一组），再往翻的方向读到第 `prefetch_count` 张。双页时从这一组的两头往外数
    /// （一起显示的另一页不占往前读的张数）。往翻的方向已经到头（翻到第一张、最后一张，切到上一个文件夹时
    /// 从它的第一张看起）时往另一头读。关掉预读时只有前两样。
    fn preload_candidates(&self) -> (Vec<PathBuf>, usize) {
        fn add(paths: &mut Vec<PathBuf>, path: &Path) {
            if !paths.iter().any(|p| p == path) {
                paths.push(path.to_path_buf());
            }
        }
        let mut paths = Vec::new();
        if let Some(current) = &self.current {
            add(&mut paths, current);
        }
        for page in self.spread.iter().flat_map(|s| &s.paths) {
            add(&mut paths, page);
        }
        if let Some(j) = spread_neighbor(self) {
            add(&mut paths, &self.entries[j].path);
        }
        let keep = paths.len();
        if !self.opts.prefetch {
            return (paths, keep);
        }
        if let Some((lo, hi)) = preload_group(self) {
            let (lo, hi, len) = (lo as isize, hi as isize, self.entries.len() as isize);
            let ahead = self.opts.prefetch_count.max(1) as isize;
            let behind: isize = if self.two_page.is_some() { 2 } else { 1 };
            let back = match self.heading_back {
                true => lo > 0,
                false => hi + 1 >= len && lo > 0,
            };
            for k in 1..=ahead.max(behind) {
                let mut at = Vec::with_capacity(2);
                if k <= ahead {
                    at.push(if back { lo - k } else { hi + k });
                }
                if k <= behind {
                    at.push(if back { hi + k } else { lo - k });
                }
                for j in at {
                    if (0..len).contains(&j) {
                        add(&mut paths, &self.entries[j as usize].path);
                    }
                }
            }
        }
        (paths, keep)
    }
}

/// 预读从哪一段往外数：双页时是这一组（显示着的那一组；还没显示时是当前图片和挨着它、要一起显示的那一页），
/// 单页时是当前图片自己。当前图片不在列表里时为空。
fn preload_group(st: &State) -> Option<(usize, usize)> {
    let (lo, hi) = spread_range(st)?;
    match (st.spread.is_none(), spread_neighbor(st)) {
        (true, Some(j)) => Some((lo.min(j), hi.max(j))),
        _ => Some((lo, hi)),
    }
}

/// 预读放得下几张（含当前图片）：在这个窗口分到的缓存里（几个窗口平分“缓存大小”），先近后远一张张加，
/// 加到放不下为止。已经在缓存里的按实际大小算；还没读的按这个窗口最近解开的那张估（开着反相等效果时每张
/// 还要多存一份处理好的，按两倍算；缓存里只有原图的也是）。什么都还不知道时（刚打开、还没解开过）为空：不限。
fn preload_limit(st: &State, cache: &ImageCache, owner: u64) -> Option<usize> {
    let (paths, keep) = st.preload_candidates();
    let copies = if st.effects.is_none() { 1 } else { 2 };
    let sizes = cache.sizes();
    let known: Vec<usize> = paths
        .iter()
        .map(|p| sizes.get(p.as_path()).map_or(0, |&(total, original)| total.max(original * copies)))
        .collect();
    let guess = if st.last_bytes > 0 {
        st.last_bytes * copies
    } else {
        known.iter().copied().max().unwrap_or(0)
    };
    if guess == 0 {
        return None;
    }
    let share = cache.budget() / cache.sharers(owner).max(1);
    let mut used = 0usize;
    let mut count = 0usize;
    for (k, &bytes) in known.iter().enumerate() {
        let need = if bytes > 0 { bytes } else { guess };
        if k >= keep && used + need > share {
            break;
        }
        used += need;
        count += 1;
    }
    Some(count.max(keep))
}

/// 按现在的设置和缓存重新算预读范围，告诉缓存不要挤掉这些图片（锁的顺序：state → cache）。
fn reprotect(shared: &Arc<Shared>, st: &mut State) {
    let mut cache = lock(&shared.hub.cache);
    protect_window(st, &mut cache, shared.id);
}

/// 同 [`reprotect`]，调用方已经锁着缓存。
fn protect_window(st: &mut State, cache: &mut ImageCache, owner: u64) {
    st.preload_limit = preload_limit(st, cache, owner);
    let (paths, keep) = st.window_and_keep();
    cache.protect(owner, paths, keep);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // 持锁期间不会 panic；即使中毒，数据本身仍然有效。
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    fn push(&self, event: Event) {
        let was_empty = {
            let mut queue = lock(&self.events);
            let was_empty = queue.is_empty();
            queue.push_back(event);
            was_empty
        };
        // 在锁外唤醒，避免界面的回调里再取锁时死锁。
        if was_empty {
            self.listener.wake();
        }
    }
}

impl AppCore {
    /// 程序的第一个窗口：同时建好各窗口共用的设置、缓存和后台线程。
    pub fn new(listener: Arc<dyn CoreListener>) -> Self {
        Self::with_watch_interval(listener, WATCH_INTERVAL)
    }

    /// 同 [`AppCore::new`]，后台整轮检查的间隔由调用方给（测试用很长的间隔，看"马上核对"是不是真的发生了）。
    fn with_watch_interval(listener: Arc<dyn CoreListener>, interval: Duration) -> Self {
        let threads = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 4);
        let hub = Arc::new(Hub {
            platform: Platform::current(),
            prefs: Mutex::new(Prefs {
                settings: Settings::defaults(),
                path: None,
                table: toml::Table::new(),
                disk: Settings::defaults(),
                baseline: Settings::defaults(),
                unread: false,
                draft: None,
                status: None,
                stamp: None,
            }),
            cache: Mutex::new(ImageCache::new(DEFAULT_BUDGET)),
            pool: Pool::new(threads),
            positions: Mutex::new(crate::positions::Positions::default()),
            recent: Mutex::new(crate::recent::Recent::default()),
            track_recent: AtomicBool::new(Platform::current() == Platform::Windows),
            lang: std::sync::atomic::AtomicU8::new(lang_index(Lang::En)),
            windows: Mutex::new(Vec::new()),
            next_window: AtomicU64::new(1),
            thumbs: Mutex::new(crate::picker::ThumbCache::default()),
            background: Mutex::new(BackgroundSlot::default()),
            watch: Arc::default(),
        });
        start_watcher(&hub, interval);
        Self::with_hub(hub, listener)
    }

    /// 同一程序里再开一个窗口（从访达打开图片时）：与这个窗口共用设置、缓存和后台线程；
    /// 记住的状态（排序方式、图片信息开关等）和设置按现在的值生效，浏览集合和当前图片是新的。
    pub fn new_window(&self, listener: Arc<dyn CoreListener>) -> Self {
        let core = Self::with_hub(Arc::clone(&self.shared.hub), listener);
        let current = lock(&core.shared.hub.prefs).settings.clone();
        core.restore_state(&current);
        let runtime = Runtime::from_settings(&current);
        apply_view_settings(&core.shared, &current);
        lock(&core.shared.state).opts = runtime;
        core
    }

    fn with_hub(hub: Arc<Hub>, listener: Arc<dyn CoreListener>) -> Self {
        let id = hub.next_window.fetch_add(1, AtomicOrdering::Relaxed);
        let platform = hub.platform;
        let shared = Arc::new(Shared {
            id,
            hub,
            events: Mutex::new(VecDeque::new()),
            listener,
            state: Mutex::new(State::default()),
            view: Mutex::new(ViewState::default()),
            anim: Mutex::new(None),
            picker: Mutex::new(PickerWants::default()),
        });
        lock(&shared.hub.windows).push(Arc::downgrade(&shared));
        Self { shared, platform }
    }

    /// 拖动窗口时吸附屏幕边缘：设置关掉时为空；否则返回窗口新的原点（不用动时与原来相同）。
    /// `frame`、`area`（屏幕可用区域）是 (x, y, 宽, 高)。
    pub fn snap_window(
        &self,
        frame: (f64, f64, f64, f64),
        area: (f64, f64, f64, f64),
    ) -> Option<(f64, f64)> {
        if !lock(&self.shared.hub.prefs)
            .settings
            .bool(settings::SNAP_WINDOW)
        {
            return None;
        }
        Some(crate::view::snap_to_edges(frame, area))
    }

    /// 同一程序里开着的窗口数（含这个）。
    pub fn window_count(&self) -> usize {
        self.shared.hub.live_windows().len()
    }

    /// 提交命令后立即返回；结果通过事件送回。
    pub fn dispatch(&self, command: Command) {
        let shared = &self.shared;
        match command {
            Command::Ping { token } => {
                let s = Arc::clone(shared);
                shared
                    .hub
                    .pool
                    .submit(Priority::Current, move || s.push(Event::Pong { token }));
            }
            Command::Open { path } => open(shared, Path::new(&path)),
            Command::Navigate(navigation) => navigate(shared, navigation),
            Command::Close => close(shared),
            Command::Removed { path } => removed(shared, Path::new(&path)),
        }
    }

    /// 取出当前全部事件。应在界面主线程、收到唤醒后调用。
    pub fn drain_events(&self) -> Vec<Event> {
        lock(&self.shared.events).drain(..).collect()
    }

    /// 用户对被上限拦下的图片（[`Event::ImageBlocked`]）选了"仍然打开"：这个文件（压缩包里的图片是整个
    /// 压缩包）在这一次运行里不再设上限；还是当前图片的话重新读。
    pub fn open_without_limits(&self, path: &Path) {
        decode::trust(path);
        let mut st = lock(&self.shared.state);
        st.failed.remove(path);
        st.declined.remove(path);
        if st.current.as_deref() != Some(path) {
            return;
        }
        lock(&self.shared.hub.cache).remove(path);
        st.in_flight.remove(path);
        show_again(&self.shared, &mut st);
    }

    /// 用户对被上限拦下的图片选了"取消"：这一次运行里再翻到它不再问，按解码失败提示。
    pub fn keep_limits(&self, path: &Path) {
        lock(&self.shared.state).declined.insert(path.to_path_buf());
    }

    /// 当前图片的路径（没有时为空）。
    pub fn current_path(&self) -> Option<PathBuf> {
        lock(&self.shared.state).current.clone()
    }

    fn has_current(&self) -> bool {
        lock(&self.shared.state).current.is_some()
    }

    /// 当前图片在压缩包里。
    fn current_in_archive(&self) -> bool {
        let st = lock(&self.shared.state);
        match (&st.archive, &st.current) {
            (Some(archive), Some(current)) => current.starts_with(&archive.path),
            _ => false,
        }
    }

    /// 当前图片在磁盘上的文件：压缩包里的图片是解出的临时文件。
    fn current_file(&self) -> Option<PathBuf> {
        let current = lock(&self.shared.state).current.clone()?;
        crate::archive::materialize(&current).ok().map(|m| m.file)
    }

    /// 复制、移动到图片文件夹：在后台进行，不覆盖同名文件（加序号）；完成后在角落提示。
    /// 移动后这张图片从浏览集合里去掉，显示下一张。
    fn file_to_folder(&self, id: &str) -> ActionOutcome {
        let key = if id == "copy_to_folder2" {
            settings::FOLDER2
        } else {
            settings::FOLDER1
        };
        let folder = lock(&self.shared.hub.prefs)
            .settings
            .text(key)
            .trim()
            .to_string();
        if folder.is_empty() {
            self.shared.push(Event::Notice(NoticeKind::FolderNotSet));
            return ActionOutcome::Started;
        }
        let Some(current) = lock(&self.shared.state).current.clone() else {
            return ActionOutcome::Unavailable;
        };
        let moving = id == "move_to_folder1";
        let s = Arc::clone(&self.shared);
        self.shared.hub.pool.submit(Priority::Current, move || {
            let folder = PathBuf::from(folder);
            let result = copy_or_move(&current, &folder, moving);
            let folder_name = folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| folder.display().to_string());
            match result {
                Ok(target) => {
                    let name = target
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    s.push(Event::Notice(if moving {
                        NoticeKind::Moved {
                            folder: folder_name,
                            name,
                        }
                    } else {
                        NoticeKind::Copied {
                            folder: folder_name,
                            name,
                        }
                    }));
                    if moving {
                        removed(&s, &current);
                    }
                }
                Err(e) => s.push(Event::Notice(NoticeKind::FileFailed {
                    message: e.to_string(),
                })),
            }
        });
        ActionOutcome::Started
    }

    /// 设为桌面图片用的文件：普通文件就是它自己；压缩包里的图片先复制到设置文件夹旁边的
    /// `wallpaper/` 里（临时文件离开压缩包就删了，桌面图片要一直在）。
    fn wallpaper_file(&self) -> Result<PathBuf, String> {
        let current = lock(&self.shared.state)
            .current
            .clone()
            .ok_or_else(|| "no image".to_string())?;
        if crate::archive::split(&current).is_none() {
            return Ok(current);
        }
        let source = crate::archive::materialize(&current).map_err(|e| e.to_string())?;
        let dir = lock(&self.shared.hub.prefs)
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|d| d.join("wallpaper"))
            .unwrap_or_else(|| std::env::temp_dir().join("anyview-wallpaper"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let name = source
            .file
            .file_name()
            .ok_or_else(|| "no file name".to_string())?;
        let target = dir.join(name);
        std::fs::copy(&source.file, &target).map_err(|e| e.to_string())?;
        Ok(target)
    }

    /// 动作现在是否可用、是否勾选。
    pub fn action_state(&self, id: &str) -> ActionState {
        let Some(def) = actions::find(id) else {
            return ActionState::default();
        };
        // 动图：只有正在显示动图时可用。
        if matches!(id, "anim_toggle" | "anim_next_frame" | "anim_prev_frame") {
            let info = self.animation();
            return ActionState {
                enabled: def.ready && info.is_some(),
                checked: id == "anim_toggle" && info.is_some_and(|i| !i.playing),
            };
        }
        let has_current = self.has_current();
        // 图片文件夹 2 没有设置时，"复制到 图片文件夹 2"不可用。
        if id == "copy_to_folder2"
            && lock(&self.shared.hub.prefs)
                .settings
                .text(settings::FOLDER2)
                .trim()
                .is_empty()
        {
            return ActionState {
                enabled: false,
                checked: false,
            };
        }
        // 生成信息：当前图片带 AI 生成信息时才可用。
        if id == "show_generation_info" {
            return ActionState {
                enabled: def.ready && self.generation_info().is_some(),
                checked: false,
            };
        }
        // 压缩包里的图片不能删除、移动。
        if matches!(id, "delete_file" | "move_to_folder1") && self.current_in_archive() {
            return ActionState {
                enabled: false,
                checked: false,
            };
        }
        let view = lock(&self.shared.view);
        let shown = view.has_image();
        let checked = match id {
            "edge_none" => view.edge == EdgeMode::None,
            "edge_soften" => view.edge == EdgeMode::Soften,
            "edge_soften_sharpen" => view.edge == EdgeMode::SoftenSharpen,
            "interp_bilinear" => view.interpolation == Interpolation::Bilinear,
            "interp_bicubic" => view.interpolation == Interpolation::Bicubic,
            "interp_lanczos" => view.interpolation == Interpolation::Lanczos,
            "double_page_ltr" | "double_page_rtl" => {
                drop(view);
                let two_page = lock(&self.shared.state).two_page;
                let wanted = if id == "double_page_ltr" {
                    crate::spread::Direction::Ltr
                } else {
                    crate::spread::Direction::Rtl
                };
                return ActionState {
                    enabled: def.ready,
                    checked: two_page == Some(wanted),
                };
            }
            "invert" | "gamma" => {
                drop(view);
                let effects = lock(&self.shared.state).effects;
                return ActionState {
                    enabled: def.ready,
                    checked: if id == "invert" {
                        effects.invert
                    } else {
                        effects.gamma
                    },
                };
            }
            "sort_name" | "sort_mtime" | "sort_size" | "sort_type" | "sort_desc" => {
                drop(view);
                let sort = lock(&self.shared.state).sort;
                let checked = match id {
                    "sort_name" => sort.key == SortKey::Name,
                    "sort_mtime" => sort.key == SortKey::Modified,
                    "sort_size" => sort.key == SortKey::Size,
                    "sort_type" => sort.key == SortKey::Type,
                    _ => sort.descending,
                };
                return ActionState {
                    enabled: def.ready && (!def.needs_image || has_current),
                    checked,
                };
            }
            "stretch_small" => view.stretch_small,
            "zoom_original" => view.mode == DisplayMode::Original,
            "fit_window" => view.mode == DisplayMode::FitWindow,
            "fit_width" => view.mode == DisplayMode::FitWidth,
            "zoom_lock" => view.zoom_lock,
            "toggle_info" => view.info_visible,
            "toggle_bar_lock" => {
                // "不显示控制栏"时不能锁定（控制栏完全不显示）。
                let locked = view.bar_locked;
                drop(view);
                let hidden = lock(&self.shared.hub.prefs)
                    .settings
                    .bool(settings::HIDE_CONTROL_BAR);
                return ActionState {
                    enabled: def.ready && !hidden,
                    checked: locked,
                };
            }
            "toggle_title_filename" => {
                // macOS、Windows 有这一项（Linux 的标题栏由窗口管理器一直显示）：按设置表里的平台。
                drop(view);
                return ActionState {
                    enabled: def.ready && self.platform_has(settings::TITLE_FILENAME),
                    checked: lock(&self.shared.hub.prefs)
                        .settings
                        .bool(settings::TITLE_FILENAME),
                };
            }
            "always_on_top" => {
                // macOS、Windows 能置顶（Linux 的 GTK4 没有这个接口）：按设置表里的平台。
                drop(view);
                return ActionState {
                    enabled: self.platform_has(settings::ALWAYS_ON_TOP),
                    checked: lock(&self.shared.hub.prefs)
                        .settings
                        .bool(settings::ALWAYS_ON_TOP),
                };
            }
            "flip_horizontal" => shown && view.flip_horizontal(),
            "flip_vertical" => shown && view.flip_vertical(),
            "rotate_none" => shown && view.rotation() == 0,
            "rotate_cw" => shown && view.rotation() == 90,
            "rotate_180" => shown && view.rotation() == 180,
            "rotate_ccw" => shown && view.rotation() == 270,
            _ => false,
        };
        ActionState {
            enabled: def.ready && (!def.needs_image || has_current),
            checked,
        }
    }

    /// 执行一个动作（菜单点击、按键、控制栏按钮都走这里）。查看类动作以窗口中心为中心。
    /// 改变了"记住的状态"（排序方式、图片信息开关等）时同时写进设置文件。
    pub fn invoke_action(&self, id: &str) -> ActionOutcome {
        if !self.action_state(id).enabled {
            return ActionOutcome::Unavailable;
        }
        let outcome = self.run_action(id);
        self.remember_after(id);
        outcome
    }

    fn run_action(&self, id: &str) -> ActionOutcome {
        let navigate = |navigation| {
            self.dispatch(Command::Navigate(navigation));
            ActionOutcome::Started
        };
        let view = |show_zoom: bool, change: &dyn Fn(&mut ViewState)| {
            change(&mut lock(&self.shared.view));
            ActionOutcome::ViewChanged { show_zoom }
        };
        use ActionOutcome::Ui;
        match id {
            "open_file" => Ui(UiRequest::OpenFileDialog),
            "open_folder" => Ui(UiRequest::OpenFolderDialog),
            "close_image" => {
                self.dispatch(Command::Close);
                ActionOutcome::Started
            }
            "about" => Ui(UiRequest::ShowAbout),
            "settings" => Ui(UiRequest::ShowSettings),
            "select_image" => Ui(UiRequest::ShowPicker),
            "show_generation_info" => Ui(UiRequest::ShowGenerationInfo),
            "help_shortcuts" => Ui(UiRequest::ShowHelp {
                kind: crate::help::HelpKind::Shortcuts,
            }),
            "help_formats" => Ui(UiRequest::ShowHelp {
                kind: crate::help::HelpKind::Formats,
            }),
            "help_licenses" => Ui(UiRequest::ShowHelp {
                kind: crate::help::HelpKind::Licenses,
            }),
            // 标题栏隐藏时显示文件名（⇧Tab）：直接改设置、保存，界面重新读界面设置。
            "toggle_title_filename" => {
                let mut prefs = lock(&self.shared.hub.prefs);
                let on = !prefs.settings.bool(settings::TITLE_FILENAME);
                prefs.settings.set(settings::TITLE_FILENAME, Value::Bool(on));
                let _ = prefs.save(self.platform);
                Ui(UiRequest::ApplySettings)
            }
            "always_on_top" => {
                let mut prefs = lock(&self.shared.hub.prefs);
                let on = !prefs.settings.bool(settings::ALWAYS_ON_TOP);
                prefs.settings.set(settings::ALWAYS_ON_TOP, Value::Bool(on));
                let _ = prefs.save(self.platform);
                Ui(UiRequest::ApplySettings)
            }
            "quit" => Ui(UiRequest::Quit),
            "close_or_quit" => Ui(UiRequest::CloseOrQuit),
            "fullscreen" => Ui(UiRequest::ToggleFullscreen),
            "maximize" => Ui(UiRequest::ToggleMaximize),
            "minimize" => Ui(UiRequest::Minimize),
            "context_menu" => Ui(UiRequest::ShowContextMenu),
            "close_window" => Ui(UiRequest::CloseWindow),
            "hide_app" => Ui(UiRequest::HideApp),
            "hide_others" => Ui(UiRequest::HideOthers),
            "show_all" => Ui(UiRequest::ShowAll),
            "clear_recent" => Ui(UiRequest::ClearRecent),
            "next_image" => navigate(Navigation::Next),
            "prev_image" => navigate(Navigation::Prev),
            "first_image" => navigate(Navigation::First),
            "last_image" => navigate(Navigation::Last),
            "forward_10" => navigate(Navigation::Forward10),
            "back_10" => navigate(Navigation::Back10),
            "edge_none" => view(false, &|v| v.edge = EdgeMode::None),
            "edge_soften" => view(false, &|v| v.edge = EdgeMode::Soften),
            "edge_soften_sharpen" => view(false, &|v| v.edge = EdgeMode::SoftenSharpen),
            "interp_bilinear" => view(false, &|v| v.interpolation = Interpolation::Bilinear),
            "interp_bicubic" => view(false, &|v| v.interpolation = Interpolation::Bicubic),
            "interp_lanczos" => view(false, &|v| v.interpolation = Interpolation::Lanczos),
            "invert" => self.set_effects(|e| e.invert = !e.invert),
            "gamma" => self.set_effects(|e| e.gamma = !e.gamma),
            "sort_name" => self.set_sort(|o| o.key = SortKey::Name),
            "sort_mtime" => self.set_sort(|o| o.key = SortKey::Modified),
            "sort_size" => self.set_sort(|o| o.key = SortKey::Size),
            "sort_type" => self.set_sort(|o| o.key = SortKey::Type),
            "sort_desc" => self.set_sort(|o| o.descending = !o.descending),
            "random_forward" => {
                random_forward(&self.shared);
                ActionOutcome::Started
            }
            "random_back" => {
                random_back(&self.shared);
                ActionOutcome::Started
            }
            "next_container" => {
                navigate_folder(&self.shared, true);
                ActionOutcome::Started
            }
            // 双页阅览：再按一次同一个方向关掉；换方向直接换。当前这一组按新的方式重新显示。
            "double_page_ltr" | "double_page_rtl" => {
                let wanted = if id == "double_page_ltr" {
                    crate::spread::Direction::Ltr
                } else {
                    crate::spread::Direction::Rtl
                };
                let mut st = lock(&self.shared.state);
                st.two_page = if st.two_page == Some(wanted) {
                    None
                } else {
                    Some(wanted)
                };
                // 从这一组的第一页（没有成组时就是当前这张）往后重新成组。
                let first = spread_range(&st).map(|(low, _)| low);
                let target = first.and_then(|i| st.entries.get(i).map(|e| (i, e.path.clone())));
                st.next_backward = false;
                match target {
                    Some((i, path)) => set_current(&self.shared, &mut st, path, Some(i), false),
                    None => {
                        if let Some(path) = st.current.clone() {
                            let index = st.index;
                            set_current(&self.shared, &mut st, path, index, false);
                        }
                    }
                }
                ActionOutcome::Started
            }
            // 映射里的"下一页、上一页（双页阅览）"：与下一张、上一张相同（双页时一次一组）。
            "next_page_double" => navigate(Navigation::Next),
            "prev_page_double" => navigate(Navigation::Prev),
            "prev_container" => {
                navigate_folder(&self.shared, false);
                ActionOutcome::Started
            }
            "copy_to_folder1" | "copy_to_folder2" | "move_to_folder1" => self.file_to_folder(id),
            "open_in_editor" => {
                let Some(path) = self.current_file() else {
                    return ActionOutcome::Unavailable;
                };
                let editor = lock(&self.shared.hub.prefs)
                    .settings
                    .text(settings::EDITOR)
                    .trim()
                    .to_string();
                Ui(UiRequest::OpenInEditor {
                    path: path.display().to_string(),
                    editor,
                })
            }
            "set_wallpaper" => match self.wallpaper_file() {
                Ok(path) => Ui(UiRequest::SetWallpaper {
                    path: path.display().to_string(),
                }),
                Err(message) => {
                    self.shared
                        .push(Event::Notice(NoticeKind::FileFailed { message }));
                    ActionOutcome::Started
                }
            },
            "print" => {
                let Some(current) = lock(&self.shared.state).current.clone() else {
                    return ActionOutcome::Unavailable;
                };
                Ui(UiRequest::Print {
                    name: current
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                })
            }
            "delete_file" | "reveal_file" | "copy_to_clipboard" => {
                let Some(current) = lock(&self.shared.state).current.clone() else {
                    return ActionOutcome::Unavailable;
                };
                let name = current
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // 压缩包里的图片：定位到压缩包文件；复制时用解出的临时文件。
                let path = match (id, crate::archive::split(&current)) {
                    ("reveal_file", Some((archive, _))) => archive,
                    ("copy_to_clipboard", Some(_)) => match self.current_file() {
                        Some(file) => file,
                        None => return ActionOutcome::Unavailable,
                    },
                    _ => current,
                };
                let path = path.display().to_string();
                let confirm = lock(&self.shared.hub.prefs)
                    .settings
                    .bool(settings::CONFIRM_DELETE);
                Ui(match id {
                    "delete_file" => UiRequest::TrashFile {
                        path,
                        name,
                        confirm,
                    },
                    "reveal_file" => UiRequest::RevealFile { path },
                    _ => UiRequest::CopyImage { path },
                })
            }
            "stretch_small" => view(true, &|v| {
                v.toggle_stretch_small();
            }),
            "zoom_original" => view(true, &|v| {
                v.set_mode(DisplayMode::Original);
            }),
            "fit_window" => view(true, &|v| {
                v.set_mode(DisplayMode::FitWindow);
            }),
            "fit_width" => view(true, &|v| {
                v.set_mode(DisplayMode::FitWidth);
            }),
            "zoom_in" => view(true, &|v| {
                v.zoom_step(1, None);
            }),
            "zoom_out" => view(true, &|v| {
                v.zoom_step(-1, None);
            }),
            "zoom_100" => view(true, &|v| {
                v.zoom_actual(None);
            }),
            "zoom_in_1" => view(true, &|v| {
                v.zoom_percent(1);
            }),
            "zoom_out_1" => view(true, &|v| {
                v.zoom_percent(-1);
            }),
            "zoom_lock" => view(false, &|v| {
                v.toggle_zoom_lock();
            }),
            "rotate_none" => view(false, &|v| {
                v.reset_rotation();
            }),
            "rotate_cw" => view(false, &|v| {
                v.rotate_by(90);
            }),
            "rotate_ccw" => view(false, &|v| {
                v.rotate_by(-90);
            }),
            "rotate_180" => view(false, &|v| {
                v.rotate_by(180);
            }),
            "flip_horizontal" => view(false, &|v| {
                v.toggle_flip_horizontal();
            }),
            "flip_vertical" => view(false, &|v| {
                v.toggle_flip_vertical();
            }),
            "pan_up" => view(false, &|v| {
                v.pan_step(0, -1);
            }),
            "pan_down" => view(false, &|v| {
                v.pan_step(0, 1);
            }),
            "pan_left" => view(false, &|v| {
                v.pan_step(-1, 0);
            }),
            "pan_right" => view(false, &|v| {
                v.pan_step(1, 0);
            }),
            "toggle_info" => {
                let mut v = lock(&self.shared.view);
                v.info_visible = !v.info_visible;
                ActionOutcome::OverlayChanged
            }
            "toggle_bar_lock" => {
                let mut v = lock(&self.shared.view);
                v.bar_locked = !v.bar_locked;
                ActionOutcome::OverlayChanged
            }
            // 动图：空格暂停、继续；逐帧时先暂停。
            "anim_toggle" | "anim_next_frame" | "anim_prev_frame" => {
                let generation = lock(&self.shared.state).generation;
                let Some(control) = animation_control(&self.shared, generation) else {
                    return ActionOutcome::Unavailable;
                };
                control.change(|a| match id {
                    "anim_toggle" => a.paused = !a.paused,
                    _ => {
                        a.paused = true;
                        a.step = if id == "anim_next_frame" { 1 } else { -1 };
                    }
                });
                ActionOutcome::OverlayChanged
            }
            _ => ActionOutcome::Unavailable,
        }
    }

    /// 按键：按所在平台的默认键位解析成动作并执行。没有绑定时返回 `None`，界面按平台惯例处理（如提示音）。
    /// `key` 是有名字的按键（如 `Right`、`PageDown`）或按键输入的字符（含 ⇧ 的效果，如 `+`）。
    pub fn invoke_key(&self, key: &str, mods: u32) -> Option<ActionOutcome> {
        let id = self.action_for_key(key, mods)?;
        Some(self.invoke_action(id))
    }

    /// 按键对应的动作 ID（按所在平台的默认键位）；没有绑定时为空。界面先查出动作再执行，
    /// 执行后能按动作决定窗口要不要跟着变（如旋转）。
    pub fn action_for_key(&self, key: &str, mods: u32) -> Option<&'static str> {
        let chord = Chord::new(key, mods)?;
        // 键盘设置里的映射优先于默认键位。
        let bindings = settings::key_bindings(&lock(&self.shared.hub.prefs).settings);
        if let Some((_, id)) = bindings.iter().find(|(c, _)| *c == chord) {
            return Some(id);
        }
        let def = actions::resolve(self.platform, &chord)?;
        // "按 ESC 退出"关掉时 Esc 不做事。
        if def.id == "close_or_quit"
            && chord.key == "Escape"
            && !lock(&self.shared.hub.prefs)
                .settings
                .bool(settings::ESC_QUITS)
        {
            return None;
        }
        Some(def.id)
    }

    /// 一个动作现在实际生效的快捷键（ACTIONS 规则 7）：默认键位里第一个没被键盘映射占用的；
    /// 都被占用（或没有默认键位）时，取映射到这个动作的第一个按键。
    fn effective_shortcut(
        &self,
        def: &actions::ActionDef,
        bindings: &[(Chord, &'static str)],
    ) -> Option<Chord> {
        def.keys_for(self.platform)
            .iter()
            .filter_map(|spec| Chord::parse(spec))
            .find(|chord| !bindings.iter().any(|(c, _)| c == chord))
            .or_else(|| {
                bindings
                    .iter()
                    .find(|(_, id)| *id == def.id)
                    .map(|(c, _)| c.clone())
            })
    }

    /// 菜单数据（右键菜单、macOS 菜单栏）：快捷键按键盘映射显示实际生效的按键（ACTIONS 规则 7）。
    pub fn menu(&self, kind: actions::MenuKind, lang: Lang) -> Vec<actions::MenuEntry> {
        let bindings = settings::key_bindings(&lock(&self.shared.hub.prefs).settings);
        actions::menu_with(kind, self.platform, lang, &|def| {
            self.effective_shortcut(def, &bindings)
        })
    }

    /// 按钮的悬停提示：名称加实际生效的快捷键。
    pub fn tooltip(&self, id: &str, lang: Lang) -> String {
        let bindings = settings::key_bindings(&lock(&self.shared.hub.prefs).settings);
        actions::tooltip_with(id, self.platform, lang, &|def| {
            self.effective_shortcut(def, &bindings)
        })
    }

    // ---- 鼠标（设置"鼠标"页；ACTIONS"鼠标与触控板的默认动作"）----

    /// 中键单击、右键、侧键、滚轮：勾选了映射时执行映射的动作（或屏蔽），否则按默认处理。
    pub fn mouse_action(&self, input: settings::MouseInput) -> settings::MouseBinding {
        settings::mouse_binding(&lock(&self.shared.hub.prefs).settings, input)
    }

    /// 左键单击要等松开才能决定（映射了左键，或打开了"单击屏幕两侧时翻页"）：界面在按下时不马上拖动窗口，
    /// 移动超过几个点才算拖动，没动就松开才算单击。
    pub fn left_click_matters(&self) -> bool {
        let prefs = lock(&self.shared.hub.prefs);
        settings::mouse_binding(&prefs.settings, settings::MouseInput::Left)
            != settings::MouseBinding::Default
            || prefs.settings.bool(settings::CLICK_SIDES)
    }

    /// 左键单击（按下后没有拖动就松开）要执行的动作：映射了左键时为映射的动作；打开了"单击屏幕两侧时翻页"时，
    /// 点在左边三分之一为上一张，右边三分之一为下一张。`x_fraction`：点在画面宽度上的位置（0–1）。
    pub fn left_click_action(&self, x_fraction: f64) -> Option<&'static str> {
        let prefs = lock(&self.shared.hub.prefs);
        match settings::mouse_binding(&prefs.settings, settings::MouseInput::Left) {
            settings::MouseBinding::Action(id) => Some(id),
            settings::MouseBinding::Nothing => None,
            settings::MouseBinding::Default => {
                if !prefs.settings.bool(settings::CLICK_SIDES) {
                    None
                } else if x_fraction < 1.0 / 3.0 {
                    Some("prev_image")
                } else if x_fraction > 2.0 / 3.0 {
                    Some("next_image")
                } else {
                    None
                }
            }
        }
    }

    /// 左键双击（第二次按下时）要执行的动作：默认最大化、还原（设置"双击最大化/还原"）。
    /// 映射了左键时每次单击都执行映射的动作，双击不另外做事；"单击屏幕两侧时翻页"打开时，两侧的每次单击都翻页，
    /// 只有中间三分之一的双击最大化。
    pub fn left_double_click_action(&self, x_fraction: f64) -> Option<&'static str> {
        let prefs = lock(&self.shared.hub.prefs);
        let s = &prefs.settings;
        if settings::mouse_binding(s, settings::MouseInput::Left) != settings::MouseBinding::Default
        {
            return None;
        }
        let on_side = !(1.0 / 3.0..=2.0 / 3.0).contains(&x_fraction);
        if s.bool(settings::CLICK_SIDES) && on_side {
            return None;
        }
        s.bool(settings::DOUBLE_CLICK_MAXIMIZE)
            .then_some("maximize")
    }

    /// 键盘设置里热键旁小按钮的按键列表。
    pub fn settings_special_keys(&self, lang: Lang) -> Vec<settings::OptionView> {
        settings::special_key_options(self.platform, lang)
    }

    // ---- 查看计算（同步）。坐标、尺寸都是视口内的物理像素。----

    /// 切换反相、Gamma（ARCHITECTURE"效果与透明"）：重新生成当前图片的显示缓冲区，以 `ImageUpdated` 送达；
    /// 预加载的图片随后也按新效果准备。
    fn set_effects(&self, change: impl FnOnce(&mut Effects)) -> ActionOutcome {
        let mut st = lock(&self.shared.state);
        change(&mut st.effects);
        let Some(path) = st.current.clone() else {
            return ActionOutcome::OverlayChanged;
        };
        let ready = request_display(&self.shared, &mut st, path, Priority::Current);
        if ready && st.scanned {
            prefetch_neighbors(&self.shared, &mut st);
        }
        // 有图片时，新的显示缓冲区随后以 ImageUpdated 送达。
        ActionOutcome::Started
    }

    /// 画面签名（见 [`ViewState::signature`]），另含效果：效果变了，旧的精确重绘也作废。
    pub fn view_signature(&self) -> u64 {
        let effects = lock(&self.shared.state).effects;
        let strong = lock(&self.shared.hub.prefs)
            .settings
            .bool(settings::STRONG_SHARPEN);
        let view = lock(&self.shared.view).signature();
        view ^ (u64::from(effects.invert) << 62)
            ^ (u64::from(effects.gamma) << 61)
            ^ (u64::from(strong) << 60)
            ^ effects.display.unwrap_or(0).rotate_left(17)
    }

    /// 这个窗口所在显示器的 ICC 配置文件（"使用监视器的 ICC 设置"）：界面在窗口出现、换到别的显示器、
    /// 显示器的配置文件改了时调用；没有配置文件（或桌面自己做色彩管理，如 Wayland、macOS）时传空。
    /// 设置打开时按它转换颜色（与反相、Gamma 一样重新生成显示缓冲区，以 `ImageUpdated` 送达）；关着时只记下。
    /// 返回画面的颜色是否要变。
    pub fn set_display_profile(&self, icc: Option<Vec<u8>>) -> bool {
        let id = icc.as_deref().and_then(effects::register_display_profile);
        // 读设置、记下配置文件、改效果、请求重新显示都在同一次加锁里做：设置同时被别处改了也不会写回旧的结果。
        let mut st = lock(&self.shared.state);
        st.display_profile = id;
        let wanted = if st.opts.monitor_icc { id } else { None };
        if st.effects.display == wanted {
            return false;
        }
        st.effects.display = wanted;
        if let Some(path) = st.current.clone() {
            let ready = request_display(&self.shared, &mut st, path, Priority::Current);
            if ready && st.scanned {
                prefetch_neighbors(&self.shared, &mut st);
            }
        }
        true
    }

    /// 现在是否按显示器的配置文件转换颜色（自测、报告用）。
    pub fn display_profile_active(&self) -> bool {
        lock(&self.shared.state).effects.display.is_some()
    }

    /// 当前图片的原图（按 EXIF 方向，不含反相、Gamma、显示器配置文件等效果）；还没解码好时为空。
    /// 动图是正在显示的那一帧；双页时是当前这一组合成的图（开着效果时用两页的原图重新合成，效果不改变复制的页数）。
    pub fn original_image(&self) -> Option<Arc<DecodedImage>> {
        let st = lock(&self.shared.state);
        if let Some((generation, frame)) = &st.current_frame
            && *generation == st.generation
        {
            return Some(Arc::clone(frame));
        }
        if let (Some(spread), Some(direction)) = (&st.spread, st.two_page)
            && spread.paths.len() == 2
        {
            if spread.effects.is_none() {
                return Some(Arc::clone(&spread.image));
            }
            let paths = spread.paths.clone();
            drop(st);
            let (first, second) = {
                let mut cache = lock(&self.shared.hub.cache);
                (cache.get(&paths[0], Effects::NONE)?, cache.get(&paths[1], Effects::NONE)?)
            };
            return Some(Arc::new(crate::spread::compose(&first, &second, direction)));
        }
        let current = st.current.clone()?;
        drop(st);
        lock(&self.shared.hub.cache).get(&current, Effects::NONE)
    }

    /// 正在显示的动图的播放状态（动画信息控制）；当前图片不是动图时为空。
    pub fn animation(&self) -> Option<AnimationInfo> {
        let generation = lock(&self.shared.state).generation;
        let control = animation_control(&self.shared, generation)?;
        let a = lock(&control.state);
        Some(AnimationInfo {
            index: a.index,
            count: a.count,
            playing: !a.paused,
        })
    }

    /// 透明处怎么画：GIF 按"为 GIF 图片使用透明背景"，其他按"为32位图片使用透明背景"。
    pub fn alpha_fill(&self, image: &DecodedImage) -> settings::AlphaFill {
        let prefs = lock(&self.shared.hub.prefs);
        let s = &prefs.settings;
        if image.info.format == "GIF" {
            return match s.choice(settings::GIF_BACKGROUND).as_str() {
                "white" => settings::AlphaFill::Solid(0xFFFFFF),
                "grid" => settings::AlphaFill::Checker,
                // 默认 GIF 调色板：文件头里的背景色（跟着反相、Gamma、显示器配置文件变，与图像的颜色一致）；
                // 没有时与窗口背景相同。
                _ => settings::AlphaFill::Solid(match image.info.gif_background {
                    Some(rgb) => effects::apply_to_color(rgb, lock(&self.shared.state).effects),
                    None => s.color(settings::BACKGROUND_COLOR),
                }),
            };
        }
        match s.alpha_background() {
            settings::AlphaBackground::Black => settings::AlphaFill::Solid(0x000000),
            settings::AlphaBackground::White => settings::AlphaFill::Solid(0xFFFFFF),
            settings::AlphaBackground::Grid => settings::AlphaFill::Checker,
        }
    }

    fn current_source(&self) -> Option<(PathBuf, u64, Arc<DecodedImage>)> {
        let (path, generation, effects) = {
            let st = lock(&self.shared.state);
            // 双页：精确重绘用合成好的这一组（刚换了效果、新的还没合成好时先不做）。
            if st.two_page.is_some() {
                let spread = st.spread.as_ref().filter(|s| s.effects == st.effects)?;
                return Some((st.current.clone()?, st.generation, Arc::clone(&spread.image)));
            }
            (st.current.clone()?, st.generation, st.effects)
        };
        let source = lock(&self.shared.hub.cache).get(&path, effects)?;
        Some((path, generation, source))
    }

    /// 窗口跟着图片：切到这张新图片时窗口内容区应有的大小（点）。新图片不旋转；
    /// 锁定缩放时按锁定的倍率。`available`：屏幕可用区域（点）。
    /// 关掉"根据图片大小调节窗口大小"时为空：窗口不变。
    pub fn window_size_for_image(
        &self,
        width: u32,
        height: u32,
        available: (f64, f64),
    ) -> Option<(f64, f64)> {
        if !self.window_follows_image() {
            return None;
        }
        Some(lock(&self.shared.view).window_size_for((width, height), false, available))
    }

    /// 当前图片按现在的方向应有的窗口大小（退出最大化、全屏时用）；没有图片，
    /// 或关掉了"根据图片大小调节窗口大小"时为空（界面回到原来的窗口大小）。
    pub fn window_size_for_view(&self, available: (f64, f64)) -> Option<(f64, f64)> {
        if !self.window_follows_image() {
            return None;
        }
        lock(&self.shared.view).window_size_for_current(available)
    }

    /// 窗口是否跟着图片变（设置"根据图片大小调节窗口大小"）。
    pub fn window_follows_image(&self) -> bool {
        lock(&self.shared.hub.prefs)
            .settings
            .bool(settings::FIT_WINDOW)
    }

    /// 执行动作后窗口要不要跟着变：旋转后窗口跟着图片转；缩放、翻转、显示方式等都不改变窗口。
    /// 要变时给出新的大小。
    pub fn window_size_after_action(&self, id: &str, available: (f64, f64)) -> Option<(f64, f64)> {
        if !matches!(
            id,
            "rotate_cw" | "rotate_ccw" | "rotate_180" | "rotate_none"
        ) {
            return None;
        }
        self.window_size_for_view(available)
    }

    /// 左键拖动做什么："拖着图片走"——图片完整显示（不超出画布）、窗口又能移动时，图片带着窗口一起走
    /// （移动窗口）；图片放大到超出画布，或窗口最大化、全屏时，平移图片。`window_movable`：窗口现在能不能移动
    /// （普通窗口才能）。中键拖动总是平移图片，不用问。
    pub fn left_drag_moves_window(&self, window_movable: bool) -> bool {
        window_movable && !lock(&self.shared.view).image_exceeds_viewport()
    }

    /// 窗口所在屏幕的缩放比例（一个点对应的物理像素，Retina 为 2）：100% 按它换算。
    /// 界面在建好画布和换屏幕时调用。
    pub fn view_set_pixels_per_point(&self, pixels_per_point: f64) -> Option<Layout> {
        lock(&self.shared.view).set_pixels_per_point(pixels_per_point)
    }

    /// 图片缓存现在占用的字节数（遵守缓存上限）。
    pub fn cache_bytes(&self) -> u64 {
        lock(&self.shared.hub.cache).used_bytes() as u64
    }

    /// 当前画面是否需要精确重绘（超大图的代理被放大时也需要）。
    pub fn needs_exact(&self) -> bool {
        // 动图播放时帧一直在换，不做精确重绘。
        if self.animation().is_some() {
            return false;
        }
        let Some((_, _, source)) = self.current_source() else {
            return false;
        };
        let view = lock(&self.shared.view);
        view.needs_exact_with(crate::proxy::ratio(&source)) || vector_needs_exact(&source, &view)
    }

    /// 精确重绘：界面在交互停止约 100 ms 后调用。需要时在后台按当前倍率重采样可见区域，
    /// 结果以 `ExactReady` 送达；不需要时（Bilinear、"无"、100%，且不是被放大的代理纹理）什么也不做。
    pub fn request_exact(&self) {
        if self.animation().is_some() {
            return;
        }
        let Some((_, generation, source)) = self.current_source() else {
            return;
        };
        let amount = if lock(&self.shared.hub.prefs)
            .settings
            .bool(settings::STRONG_SHARPEN)
        {
            exact::STRONG_SHARPEN_AMOUNT
        } else {
            exact::SHARPEN_AMOUNT
        };
        let (effects, lifted) = {
            let st = lock(&self.shared.state);
            (st.effects, st.current.as_deref().is_some_and(decode::is_trusted))
        };
        let (layout, viewport, interpolation, sharpen) = {
            let view = lock(&self.shared.view);
            if !view.needs_exact_with(crate::proxy::ratio(&source))
                && !vector_needs_exact(&source, &view)
            {
                return;
            }
            let Some(layout) = view.layout() else { return };
            (
                layout,
                view.viewport(),
                view.interpolation,
                (view.edge == crate::view::EdgeMode::SoftenSharpen).then_some(amount),
            )
        };
        let signature = self.view_signature();
        let s = Arc::clone(&self.shared);
        self.shared.hub.pool.submit(Priority::Current, move || {
            let started = std::time::Instant::now();
            // 矢量图（SVG）按屏幕像素重新渲染，反相、Gamma 照样加上；其他图片从原图重采样。
            let rendered = match &source.vector {
                Some(data) => decode::with_limits_lifted(lifted, || {
                    crate::svg::render_region(data, &source, &layout, viewport)
                })
                .map(|mut r| {
                    if !effects.is_none() {
                        r.image = effects::apply(&r.image, effects);
                    }
                    r
                }),
                None => exact::render(&source, &layout, viewport, interpolation, sharpen),
            };
            let Some(rendered) = rendered else {
                return;
            };
            let render_ms = started.elapsed().as_secs_f64() * 1000.0;
            let st = lock(&s.state);
            if st.generation != generation {
                return;
            }
            s.push(Event::ExactReady {
                generation,
                signature,
                x: rendered.x,
                y: rendered.y,
                image: Arc::new(rendered.image),
                render_ms,
            });
        });
    }

    /// 改变排序方式（右键菜单"排序方式"）：重新排列浏览集合，当前图片不变，送出新的序号（`Collection`）。
    fn set_sort(&self, change: impl FnOnce(&mut SortOrder)) -> ActionOutcome {
        let mut st = lock(&self.shared.state);
        change(&mut st.sort);
        let order = st.sort;
        if st.scanned {
            collection::sort_entries(&mut st.entries, order);
            let index = st.current.clone().and_then(|c| st.position(&c));
            st.index = index;
            regroup_after_list_change(&self.shared, &mut st);
            let event = Event::Collection {
                generation: st.generation,
                index,
                total: st.entries.len(),
            };
            self.shared.push(event);
            if index.is_some() {
                prefetch_neighbors(&self.shared, &mut st);
            }
        }
        ActionOutcome::OverlayChanged
    }

    /// 选择图片窗口的列表：浏览集合的每一项、当前图片所在的行、排序按钮的文字。
    /// 文件夹还没扫描完或是空的时为空。排序、刷新后核心发 `Collection` 事件，界面重新取。
    pub fn picker_list(&self, lang: Lang) -> Option<PickerList> {
        let mut list = {
            let st = lock(&self.shared.state);
            if !st.scanned || st.entries.is_empty() {
                return None;
            }
            let container = st.folder.as_deref();
            PickerList {
                rows: st
                    .entries
                    .iter()
                    .map(|e| crate::picker::row(e, container))
                    .collect(),
                current: st.index,
                sort_label: crate::picker::sort_label(st.sort, lang),
                sort_options: Vec::new(),
            }
        };
        list.sort_options = ["sort_name", "sort_mtime", "sort_size", "sort_type", "sort_desc"]
            .iter()
            .filter_map(|id| {
                let def = actions::find(id)?;
                Some(PickerSortOption {
                    action: id.to_string(),
                    title: def.title(self.platform, lang).to_string(),
                    checked: self.action_state(id).checked,
                })
            })
            .collect();
        Some(list)
    }

    /// 选择图片窗口的"确定"、双击：切到第 `index` 张（与进度条跳转相同，双页时从这一张起成组）。
    pub fn picker_choose(&self, index: usize) {
        self.dispatch(Command::Navigate(Navigation::Jump(index)));
    }

    /// 已经生成好的缩略图；没有时为空（用 [`Self::picker_want_thumbnails`] 要）。
    pub fn picker_thumbnail(&self, path: &str) -> Option<Arc<DecodedImage>> {
        let path = PathBuf::from(path);
        let modified = self.entry_modified(&path)?;
        lock(&self.shared.hub.thumbs).get(&path, modified)
    }

    /// 选择图片窗口现在看得见的那几张：替换上一次要的；没有缩略图的在后台生成，好了发 `Thumbnail` 事件。
    /// 不再要的、还没开始生成的就不生成了。
    pub fn picker_want_thumbnails(&self, paths: Vec<String>) {
        let options = lock(&self.shared.state).opts.decode;
        let wanted: Vec<(PathBuf, Option<SystemTime>)> = paths
            .into_iter()
            .map(PathBuf::from)
            .filter_map(|p| self.entry_modified(&p).map(|m| (p, m)))
            .collect();
        let mut todo = Vec::new();
        {
            let mut picker = lock(&self.shared.picker);
            picker.wanted = wanted.iter().map(|(p, _)| p.clone()).collect();
            let mut thumbs = lock(&self.shared.hub.thumbs);
            for (path, modified) in wanted {
                if picker.in_flight.contains(&path)
                    || thumbs.has_failed(&path, modified)
                    || thumbs.get(&path, modified).is_some()
                {
                    continue;
                }
                picker.in_flight.insert(path.clone());
                todo.push((path, modified));
            }
        }
        for (path, modified) in todo {
            let shared = Arc::clone(&self.shared);
            self.shared.hub.pool.submit(Priority::Thumbnail, move || {
                make_thumbnail(&shared, path, modified, options);
            });
        }
    }

    /// 选择图片窗口是否隐藏缩略图导航（勾选框"隐藏缩略图导航"，随时记住）。
    pub fn picker_hides_thumbnails(&self) -> bool {
        lock(&self.shared.hub.prefs)
            .settings
            .bool(settings::STATE_PICKER_HIDE_THUMBS)
    }

    pub fn set_picker_hides_thumbnails(&self, hide: bool) {
        let mut prefs = lock(&self.shared.hub.prefs);
        if prefs.settings.bool(settings::STATE_PICKER_HIDE_THUMBS) != hide {
            prefs
                .settings
                .set(settings::STATE_PICKER_HIDE_THUMBS, Value::Bool(hide));
            let _ = prefs.save(self.platform);
        }
    }

    /// 这一代次的切图要做的翻页效果和音效（设置"翻页效果""翻页时播放音效"）：
    /// 只有翻页切过来的才有（"打开"、刷新、删除后的下一张没有）；效果为"无"且不放音效时为空。
    /// 界面在 `Current` 事件时放音效，在这一代次的图像送达时做效果。
    /// 音效只在这一代次第一次问时给：双页成组后同一代次还会再来一个 `Current`（标题换成两页的名字），
    /// 界面每个 `Current` 都问一次，不能响两遍。
    pub fn transition_for(&self, generation: u64) -> Option<crate::transition::Transition> {
        let (forward, first) = {
            let mut st = lock(&self.shared.state);
            let turn = st.turn.as_mut().filter(|t| t.generation == generation)?;
            (turn.forward, !std::mem::replace(&mut turn.asked, true))
        };
        let prefs = lock(&self.shared.hub.prefs);
        let effect = crate::transition::PageEffect::from_id(&prefs.settings.choice(settings::PAGE_EFFECT));
        let sound = first && prefs.settings.bool(settings::PAGE_SOUND);
        (effect != crate::transition::PageEffect::None || sound).then_some(crate::transition::Transition {
            effect,
            forward,
            duration_ms: effect.duration_ms(),
            sound,
        })
    }

    /// 背景图片（设置"更改默认背景图片"）：打开了且选了能解码的图片时为它（缩到长边不超过 3840 像素），
    /// 否则为空（用背景色）。第一次要时在后台解码，好了给所有窗口发 `BackgroundChanged`；打不开时用背景色。
    pub fn background_image(&self) -> Option<Arc<DecodedImage>> {
        let wanted = {
            let prefs = lock(&self.shared.hub.prefs);
            let s = &prefs.settings;
            let path = s.text(settings::BACKGROUND_IMAGE_PATH);
            let path = path.trim();
            (s.bool(settings::BACKGROUND_IMAGE) && !path.is_empty()).then(|| PathBuf::from(path))
        }?;
        {
            let mut slot = lock(&self.shared.hub.background);
            if slot.path.as_ref() == Some(&wanted) {
                return slot.image.clone();
            }
            slot.path = Some(wanted.clone());
            slot.image = None;
        }
        let hub = Arc::clone(&self.shared.hub);
        self.shared.hub.pool.submit(Priority::Prefetch, move || {
            let image = crate::decode::decode_file(&wanted)
                .ok()
                .map(|i| Arc::new(crate::picker::downscale(&i, BACKGROUND_MAX)));
            {
                let mut slot = lock(&hub.background);
                if slot.path.as_ref() != Some(&wanted) {
                    return;
                }
                slot.image = image;
            }
            for window in hub.live_windows() {
                window.push(Event::BackgroundChanged);
            }
        });
        None
    }

    /// 帮助窗口里的一段文字：只有"开源许可"有（Anyview 和第三方软件的许可证原文），别的帮助窗口是表。
    pub fn help_text(&self, kind: crate::help::HelpKind, lang: Lang) -> Option<String> {
        (kind == crate::help::HelpKind::Licenses).then(|| crate::licenses::text(lang))
    }

    /// 帮助窗口的内容：键盘快捷键按右键菜单的分组列出实际生效的按键（含键盘映射），菜单里没有的
    /// 放在"其他"；支持的格式按类分组。
    pub fn help_rows(&self, kind: crate::help::HelpKind, lang: Lang) -> Vec<crate::help::HelpRow> {
        use crate::help::{HelpKind, HelpRow, push_group};
        match kind {
            HelpKind::Formats => return crate::help::formats(self.platform, lang),
            HelpKind::Licenses => return Vec::new(),
            HelpKind::Shortcuts => {}
        }
        let bindings = settings::key_bindings(&lock(&self.shared.hub.prefs).settings);
        let shortcut = |def: &actions::ActionDef| self.effective_shortcut(def, &bindings);
        let mut general = Vec::new();
        let mut groups: Vec<(String, Vec<HelpRow>)> = Vec::new();
        let mut listed = HashSet::new();
        let mut depth = 0usize;
        for entry in actions::menu_with(actions::MenuKind::Context, self.platform, lang, &shortcut) {
            match entry {
                actions::MenuEntry::SubmenuStart { title, .. } => {
                    depth += 1;
                    if depth == 1 {
                        groups.push((title, Vec::new()));
                    }
                }
                actions::MenuEntry::SubmenuEnd => depth = depth.saturating_sub(1),
                actions::MenuEntry::Action {
                    id,
                    title,
                    shortcut: Some(chord),
                    ..
                } => {
                    listed.insert(id);
                    // 像素边缘处理的三项在菜单里只写"无""减淡"，单独列出时加上是哪一组。
                    let title = if id.starts_with("edge_") {
                        tr!(lang, "像素边缘：{title}", "Pixel edges: {title}").replace("{title}", &title)
                    } else {
                        title
                    };
                    let row = HelpRow::row(&title, &chord.label(self.platform));
                    match groups.last_mut() {
                        Some((_, rows)) if depth > 0 => rows.push(row),
                        _ => general.push(row),
                    }
                }
                _ => {}
            }
        }
        // 只在部分平台有的动作（按对应设置项的平台）：这个平台没有就不列。
        let available = |id: &str| match id {
            "always_on_top" => self.platform_has(settings::ALWAYS_ON_TOP),
            "toggle_title_filename" => self.platform_has(settings::TITLE_FILENAME),
            _ => true,
        };
        let other: Vec<HelpRow> = actions::ACTIONS
            .iter()
            .filter(|def| def.ready && !listed.contains(def.id))
            .filter(|def| available(def.id))
            .filter_map(|def| {
                shortcut(def).map(|chord| {
                    HelpRow::row(def.title(self.platform, lang), &chord.label(self.platform))
                })
            })
            .collect();
        let (general_title, other_title) = (tr!(lang, "常用", "General"), tr!(lang, "其他", "Other"));
        let mut rows = Vec::new();
        push_group(&mut rows, general_title, general);
        for (title, items) in groups {
            push_group(&mut rows, &title, items);
        }
        push_group(&mut rows, other_title, other);
        rows
    }

    /// 浏览集合里这一项的修改时间（外层为空表示不在集合里）。
    fn entry_modified(&self, path: &Path) -> Option<Option<SystemTime>> {
        let st = lock(&self.shared.state);
        st.entries
            .iter()
            .find(|e| e.path == path)
            .map(|e| e.modified)
    }

    /// 左上角图片信息的各行文字。图片还没解码完时只有文件名一行；没有当前图片时为空。
    pub fn info_lines(&self, lang: Lang) -> Vec<String> {
        let (path, index, total, effects) = {
            let st = lock(&self.shared.state);
            let Some(path) = st.current.clone() else {
                return Vec::new();
            };
            (
                path,
                st.index,
                st.scanned.then_some(st.entries.len()),
                st.effects,
            )
        };
        // 锁的顺序是 state → cache：效果已在上面取好，这里只锁缓存。
        let image = {
            let mut cache = lock(&self.shared.hub.cache);
            cache
                .get(&path, Effects::NONE)
                .or_else(|| cache.get(&path, effects))
        };
        let source = InfoSource {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            index,
            total,
            image: image.map(|image| ImageFacts {
                width: image.width,
                height: image.height,
                file_size: image.info.file_size,
                modified: image.info.modified,
                format: image.info.format.clone(),
                pixel_format: image.info.pixel_format.clone(),
                shooting: image.info.shooting.clone(),
                generation: image.generation.as_ref().map(|g| g.tool.clone()),
                generation_key: image
                    .generation
                    .is_some()
                    .then(|| self.shortcut_label("show_generation_info"))
                    .flatten(),
            }),
        };
        overlay::info_lines(&source, lang)
    }

    /// 当前图片里的 AI 生成信息；没有，或者还没解码好时为空。
    pub fn generation_info(&self) -> Option<Arc<crate::generation::GenerationInfo>> {
        let (path, effects) = {
            let st = lock(&self.shared.state);
            (st.current.clone()?, st.effects)
        };
        let mut cache = lock(&self.shared.hub.cache);
        cache
            .get(&path, Effects::NONE)
            .or_else(|| cache.get(&path, effects))?
            .generation
            .clone()
    }

    /// 当前图片还在读：还没解码好，也没有读不了。“生成信息”面板这时不换内容，等读好了再换（不然换图片时先闪一下
    /// “没有生成信息”，选中的文字、滚动的位置也丢了）。没有当前图片时为假。
    pub fn generation_pending(&self) -> bool {
        let (path, effects) = {
            let st = lock(&self.shared.state);
            match &st.current {
                Some(path) if !st.failed.contains_key(path) => (path.clone(), st.effects),
                _ => return false,
            }
        };
        let cache = lock(&self.shared.hub.cache);
        !cache.contains(&path, Effects::NONE) && !cache.contains(&path, effects)
    }

    /// 一个动作现在实际生效的快捷键的文字（按键盘映射，ACTIONS 规则 7）；没有快捷键时为空。
    fn shortcut_label(&self, id: &str) -> Option<String> {
        let def = actions::find(id)?;
        let bindings = settings::key_bindings(&lock(&self.shared.hub.prefs).settings);
        self.effective_shortcut(def, &bindings).map(|chord| chord.label(self.platform))
    }

    /// 触控板双指滑动：手势结束时判断是否翻页；翻页时返回执行结果。
    /// dx、dy 为画面移动方向的位移（物理像素）。
    pub fn view_swipe(&self, phase: SwipePhase, dx: f64, dy: f64) -> Option<ActionOutcome> {
        let direction = lock(&self.shared.view).swipe(phase, dx, dy);
        match direction {
            1 => Some(self.invoke_action("next_image")),
            -1 => Some(self.invoke_action("prev_image")),
            _ => None,
        }
    }

    pub fn view_layout(&self) -> Option<Layout> {
        lock(&self.shared.view).layout()
    }

    /// 视口（画布）大小变了，包括窗口缩放和换到不同缩放比例的屏幕。
    pub fn view_set_viewport(&self, width: f64, height: f64) -> Option<Layout> {
        lock(&self.shared.view).set_viewport(width, height)
    }

    /// 界面开始显示一张新图片。
    pub fn view_show_image(&self, width: u32, height: u32) -> Option<Layout> {
        lock(&self.shared.view).set_image(width, height)
    }

    /// 界面不再显示图片（关闭、失败）。
    pub fn view_clear(&self) {
        lock(&self.shared.view).clear_image();
    }

    /// 拖动、触控板双指滑动平移。
    pub fn view_pan_by(&self, dx: f64, dy: f64) -> Option<Layout> {
        lock(&self.shared.view).pan_by(dx, dy)
    }

    /// 捏合缩放，以指针为中心。
    pub fn view_zoom_by(&self, factor: f64, x: f64, y: f64) -> Option<Layout> {
        lock(&self.shared.view).zoom_by(factor, Some((x, y)))
    }

    /// 按住 ⌘/Ctrl 滚动滚轮：按档位缩放，以指针为中心。
    pub fn view_zoom_step(&self, direction: i32, x: f64, y: f64) -> Option<Layout> {
        lock(&self.shared.view).zoom_step(direction, Some((x, y)))
    }

    /// 触控板双指轻点两下。
    pub fn view_toggle_fit_actual(&self, x: f64, y: f64) -> Option<Layout> {
        lock(&self.shared.view).toggle_fit_actual(Some((x, y)))
    }

    pub fn view_image_to_screen(&self, u: f64, v: f64) -> Option<(f64, f64)> {
        let view = lock(&self.shared.view);
        let (w, h) = view.image_size()?;
        Some(view.layout()?.image_to_screen(w, h, u, v))
    }

    pub fn view_screen_to_image(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let view = lock(&self.shared.view);
        let (w, h) = view.image_size()?;
        Some(view.layout()?.screen_to_image(w, h, x, y))
    }
}

/// 设置（docs/SETTINGS.md）。
impl AppCore {
    /// 读设置文件并应用。启动时、打开第一张图片之前调用一次。`path` 为空时用平台的默认位置；
    /// 文件坏了时备份旧文件、使用默认值，界面随后用 [`AppCore::settings_notice`] 提示。
    pub fn load_settings(&self, path: Option<PathBuf>) -> settings::LoadStatus {
        let path = path.or_else(|| settings::default_path(self.platform));
        let loaded = match &path {
            Some(path) => settings::load(path),
            None => settings::Loaded {
                settings: Settings::defaults(),
                table: toml::Table::new(),
                status: settings::LoadStatus::Missing,
                unreadable: false,
            },
        };
        let status = loaded.status.clone();
        let current = loaded.settings.clone();
        let unreadable = loaded.unreadable;
        // 压缩包内的位置记在设置文件旁边。
        *lock(&self.shared.hub.positions) = crate::positions::Positions::load(
            path.as_ref()
                .and_then(|p| p.parent())
                .map(|dir| dir.join(crate::positions::FILE_NAME)),
        );
        // 最近打开的也记在设置文件旁边。
        *lock(&self.shared.hub.recent) = crate::recent::Recent::load(
            path.as_ref()
                .and_then(|p| p.parent())
                .map(|dir| dir.join(crate::recent::FILE_NAME)),
        );
        {
            let mut prefs = lock(&self.shared.hub.prefs);
            prefs.disk = loaded.settings.clone();
            prefs.baseline = loaded.settings.clone();
            prefs.settings = loaded.settings;
            prefs.table = loaded.table;
            // 文件在、但没读出来（被占用、没有权限）：不记修改时间，后台每隔一会儿再读，读到了像别的窗口改了设置
            // 一样生效；读到之前保存会先再读一次，读不了就不写（见 Prefs::save）。
            prefs.unread = unreadable;
            prefs.stamp = if unreadable {
                None
            } else {
                path.as_deref().and_then(modified_time)
            };
            prefs.path = path;
            prefs.draft = None;
            prefs.status = Some(status.clone());
        }
        self.restore_state(&current);
        self.apply_runtime(&current);
        status
    }

    /// 启动时读设置出了问题（文件坏了）的提示；只给一次。
    pub fn settings_notice(&self, lang: Lang) -> Option<String> {
        let status = lock(&self.shared.hub.prefs).status.take()?;
        let settings::LoadStatus::Recovered { backup, .. } = status else {
            return None;
        };
        use crate::i18n::{Key, text};
        Some(match backup.as_ref().and_then(|b| b.file_name()) {
            Some(name) => {
                text(Key::NoticeSettingsRecovered, lang).replace("{name}", &name.to_string_lossy())
            }
            None => text(Key::NoticeSettingsUnreadable, lang).to_string(),
        })
    }

    /// 设置文件的位置（还没读设置或取不到主目录时为空）。
    pub fn settings_path(&self) -> Option<PathBuf> {
        lock(&self.shared.hub.prefs).path.clone()
    }

    /// 界面语言：设置为"跟随系统"时用 `system`。
    pub fn ui_lang(&self, system: Lang) -> Lang {
        let lang = lock(&self.shared.hub.prefs).settings.lang(system);
        self.shared.hub.lang.store(lang_index(lang), AtomicOrdering::Relaxed);
        lang
    }

    /// 这一项设置在这个平台显示、已经做好（设置表里的平台）。
    fn platform_has(&self, key: &str) -> bool {
        settings::find(key).is_some_and(|d| d.visible_on(self.platform) && d.ready_on(self.platform))
    }

    /// 界面要按设置显示的东西。只在部分平台有的项（总在最前、显示标题栏、吸附边缘、标题栏隐藏时显示文件名）
    /// 按设置表里的平台判断：这个平台不显示、还没做好的项当作关着（以前写死成只有 macOS，Windows 上这几项都不起作用）。
    pub fn ui_settings(&self) -> UiSettings {
        let prefs = lock(&self.shared.hub.prefs);
        let s = &prefs.settings;
        let here = |key: &str| s.bool(key) && self.platform_has(key);
        UiSettings {
            background_color: s.color(settings::BACKGROUND_COLOR),
            alpha_background: s.alpha_background(),
            hide_control_bar: s.bool(settings::HIDE_CONTROL_BAR),
            hide_side_arrows: s.bool(settings::HIDE_SIDE_ARROWS),
            hide_zoom_info: s.bool(settings::HIDE_ZOOM_INFO),
            hide_tooltips: s.bool(settings::HIDE_TOOLTIPS),
            small_info_font: s.bool(settings::SMALL_INFO_FONT),
            corner_notices: s.bool(settings::CORNER_NOTICES),
            double_click_maximize: s.bool(settings::DOUBLE_CLICK_MAXIMIZE),
            always_on_top: here(settings::ALWAYS_ON_TOP),
            no_recent: s.bool(settings::NO_RECENT),
            show_title_bar: here(settings::SHOW_TITLE_BAR),
            hide_animation_info: s.bool(settings::HIDE_ANIMATION_INFO),
            no_screensaver: s.bool(settings::NO_SCREENSAVER),
            snap_to_edges: here(settings::SNAP_WINDOW),
            reuse_window: s.bool(settings::SINGLE_INSTANCE),
            title_filename: here(settings::TITLE_FILENAME),
            hide_menu_bar: !self.platform_has(settings::HIDE_MENU_BAR)
                || s.bool(settings::HIDE_MENU_BAR),
            bar_hover_effect: if self.platform_has(settings::BAR_HOVER_EFFECT) {
                s.bar_hover_effect()
            } else {
                settings::BarHoverEffect::Highlight
            },
        }
    }

    /// 一项设置的当前值（自检和界面的少数地方用）。
    pub fn setting(&self, key: &str) -> Option<Value> {
        lock(&self.shared.hub.prefs).settings.get(key).cloned()
    }

    /// 打开设置窗口：把当前设置复制成草稿。窗口已经打开（已有草稿）时保留草稿。
    pub fn settings_begin(&self) {
        let mut prefs = lock(&self.shared.hub.prefs);
        if prefs.draft.is_none() {
            prefs.draft = Some(Draft::new(&prefs.settings));
        }
    }

    /// 关闭设置窗口而不确定：丢掉草稿（未保存修改的确认由界面在调用前完成）。
    pub fn settings_cancel(&self) {
        lock(&self.shared.hub.prefs).draft = None;
    }

    /// 左侧分类列表：(页 ID, 名称)，只含这个平台有设置项的页。
    pub fn settings_pages(&self, lang: Lang) -> Vec<(&'static str, &'static str)> {
        settings::visible_pages(self.platform)
            .into_iter()
            .map(|page| (page.id(), page.title(lang)))
            .collect()
    }

    /// 一页的内容（按草稿的值；没有草稿时按当前设置）。
    pub fn settings_page(&self, page: &str, lang: Lang) -> Option<SettingsPageView> {
        let page = settings::Page::from_id(page)?;
        let prefs = lock(&self.shared.hub.prefs);
        let values = prefs
            .draft
            .as_ref()
            .map(Draft::values)
            .unwrap_or(&prefs.settings);
        let content = settings::page_content(page, values, self.platform, lang);
        Some(SettingsPageView {
            id: page.id(),
            title: page.title(lang),
            lead: page.lead(lang),
            sections: content.sections,
            items: content.items,
        })
    }

    /// 改草稿里的一项；返回是否接受。界面随后重新取这一页，更新各项的可用状态。
    pub fn settings_set(&self, key: &str, input: settings::Input) -> bool {
        let platform = self.platform;
        lock(&self.shared.hub.prefs)
            .draft
            .as_mut()
            .is_some_and(|draft| draft.set(key, input, platform))
    }

    /// "重置"：这一页的草稿恢复为默认值。
    pub fn settings_reset_page(&self, page: &str) {
        let Some(page) = settings::Page::from_id(page) else {
            return;
        };
        let platform = self.platform;
        if let Some(draft) = lock(&self.shared.hub.prefs).draft.as_mut() {
            draft.reset_page(page, platform);
        }
    }

    /// 有没有没确定的修改（关闭窗口时询问）。
    pub fn settings_dirty(&self) -> bool {
        lock(&self.shared.hub.prefs)
            .draft
            .as_ref()
            .is_some_and(Draft::is_dirty)
    }

    /// "确定"：校验草稿；通过后把改过的项写回、生效、保存，并结束草稿。
    /// 校验失败时草稿保留，返回出错的页和项。
    pub fn settings_apply(&self) -> Result<SettingsApplied, settings::Invalid> {
        let zoom_state = self.zoom_state_values();
        let (current, changed, save_error) = {
            let mut prefs = lock(&self.shared.hub.prefs);
            let Some(draft) = prefs.draft.take() else {
                return Ok(SettingsApplied::default());
            };
            if let Err(invalid) = draft.validate(self.platform) {
                prefs.draft = Some(draft);
                return Err(invalid);
            }
            let changes = draft.changes();
            let changed: Vec<&'static str> = changes.iter().map(|(key, _)| *key).collect();
            for (key, value) in changes {
                prefs.settings.set(key, value);
            }
            // 刚打开"保存缩放状态"：把现在的显示方式记下来。
            if changed.contains(&settings::SAVE_ZOOM_STATE)
                && prefs.settings.bool(settings::SAVE_ZOOM_STATE)
            {
                for (key, value) in zoom_state {
                    prefs.settings.set(key, value);
                }
            }
            let save_error = prefs.save(self.platform).err();
            (prefs.settings.clone(), changed, save_error)
        };
        self.apply_runtime(&current);
        Ok(SettingsApplied {
            changed,
            save_error,
        })
    }

    /// 设置窗口里的按钮：返回要界面做的事；项不可用时为空。
    pub fn settings_button(&self, key: &str, button: &str) -> Option<settings::ButtonCommand> {
        let mut prefs = lock(&self.shared.hub.prefs);
        // 图像编辑器的"预设"：填上这个平台常见的编辑器，界面重新显示这一页。
        if key == settings::EDITOR && button == "presets" {
            let preset = settings::preset_editor(self.platform);
            let platform = self.platform;
            let draft = prefs.draft.as_mut()?;
            draft.set(key, settings::Input::Text(preset), platform);
            return Some(settings::ButtonCommand::Refresh);
        }
        let values = prefs
            .draft
            .as_ref()
            .map(Draft::values)
            .unwrap_or(&prefs.settings);
        settings::button_command(key, button, values, self.platform)
    }

    /// 退出前调用：打开了"保存缩放状态"时记下显示方式、缩放锁定和锁定的倍率。
    pub fn remember_view_state(&self) {
        // 压缩包内的位置也在这时写进文件。
        let _ = lock(&self.shared.hub.positions).save();
        let values = self.zoom_state_values();
        let mut prefs = lock(&self.shared.hub.prefs);
        if !prefs.settings.bool(settings::SAVE_ZOOM_STATE) {
            return;
        }
        let before = prefs.settings.clone();
        for (key, value) in values {
            prefs.settings.set(key, value);
        }
        if prefs.settings != before {
            let _ = prefs.save(self.platform);
        }
    }

    /// 设置 › 书签的"删除"：忘掉所有压缩包内的位置（界面先确认）。
    pub fn clear_archive_positions(&self) {
        let mut positions = lock(&self.shared.hub.positions);
        positions.clear();
        let _ = positions.save();
    }

    /// 记住了几个压缩包内的位置（自检用）。
    pub fn archive_positions_count(&self) -> usize {
        lock(&self.shared.hub.positions).len()
    }

    /// 最近打开的文件、文件夹、压缩包（菜单栏的"打开最近使用"），最近的在最前。只在 Windows 上记；
    /// 设置"不保存浏览过的文件夹路径"打开时是空的，已经记下的也在这时清掉。
    pub fn recent_files(&self) -> Vec<String> {
        let hub = &self.shared.hub;
        let off = lock(&hub.prefs).settings.bool(settings::NO_RECENT);
        if off {
            self.clear_recent();
            return Vec::new();
        }
        lock(&hub.recent).list().to_vec()
    }

    /// "打开最近使用"里的"清除菜单"。
    pub fn clear_recent(&self) {
        if lock(&self.shared.hub.recent).clear() {
            save_recent_later(&self.shared);
        }
    }

    /// 程序退出前：记下查看状态，关掉所有压缩包（删掉解出的临时文件）。
    pub fn shutdown(&self) {
        self.remember_view_state();
        crate::archive::shutdown();
    }

    /// 退出时要保存的查看状态（“保存缩放状态”）。
    fn zoom_state_values(&self) -> Vec<(&'static str, Value)> {
        let view = lock(&self.shared.view);
        let zoom = if view.zoom_lock && view.has_image() {
            view.layout().map(|l| l.zoom)
        } else {
            None
        };
        let mut values = vec![
            (
                settings::STATE_MODE,
                Value::Choice(mode_id(view.mode).to_string()),
            ),
            (settings::STATE_STRETCH, Value::Bool(view.stretch_small)),
            (settings::STATE_ZOOM_LOCK, Value::Bool(view.zoom_lock)),
        ];
        if let Some(zoom) = zoom {
            let percent = (zoom * 100.0).round() as i64;
            values.push((settings::STATE_ZOOM, Value::Int(percent.clamp(1, 3200))));
        }
        values
    }

    /// 动作改变了"记住的状态"时写进设置文件（排序方式、图片信息、控制栏锁定、像素边缘、
    /// 菜单里的插值算法；打开"保存缩放状态"时还有显示方式）。
    fn remember_after(&self, id: &str) {
        let mut values: Vec<(&'static str, Value)> = Vec::new();
        match id {
            "sort_name" | "sort_mtime" | "sort_size" | "sort_type" | "sort_desc" => {
                let sort = lock(&self.shared.state).sort;
                values.push((
                    settings::STATE_SORT,
                    Value::Choice(sort_id(sort.key).to_string()),
                ));
                values.push((settings::STATE_SORT_DESC, Value::Bool(sort.descending)));
            }
            "toggle_info" => {
                let shown = lock(&self.shared.view).info_visible;
                values.push((settings::STATE_INFO, Value::Bool(shown)));
            }
            "toggle_bar_lock" => {
                let locked = lock(&self.shared.view).bar_locked;
                values.push((settings::STATE_BAR_LOCKED, Value::Bool(locked)));
            }
            "double_page_ltr" | "double_page_rtl" => {
                let two_page = lock(&self.shared.state).two_page;
                values.push((
                    settings::STATE_TWO_PAGE,
                    Value::Choice(two_page.map_or("off", |d| d.id()).to_string()),
                ));
            }
            "edge_none" | "edge_soften" | "edge_soften_sharpen" => {
                let edge = lock(&self.shared.view).edge;
                values.push((
                    settings::STATE_EDGE,
                    Value::Choice(edge_id(edge).to_string()),
                ));
            }
            "interp_bilinear" | "interp_bicubic" | "interp_lanczos" => {
                let interpolation = lock(&self.shared.view).interpolation;
                values.push((
                    settings::INTERPOLATION,
                    Value::Choice(interpolation_id(interpolation).to_string()),
                ));
            }
            "stretch_small" | "zoom_original" | "fit_window" | "fit_width" | "zoom_lock" => {
                if !lock(&self.shared.hub.prefs)
                    .settings
                    .bool(settings::SAVE_ZOOM_STATE)
                {
                    return;
                }
                values = self.zoom_state_values();
            }
            _ => return,
        }
        let mut prefs = lock(&self.shared.hub.prefs);
        let before = prefs.settings.clone();
        for (key, value) in values {
            prefs.settings.set(key, value);
        }
        if prefs.settings != before {
            let _ = prefs.save(self.platform);
        }
    }

    /// 启动时恢复记住的状态。
    fn restore_state(&self, s: &Settings) {
        {
            let mut st = lock(&self.shared.state);
            st.sort = SortOrder {
                key: sort_key(&s.choice(settings::STATE_SORT)),
                descending: s.bool(settings::STATE_SORT_DESC),
            };
            st.two_page = crate::spread::Direction::from_id(&s.choice(settings::STATE_TWO_PAGE));
        }
        let mut view = lock(&self.shared.view);
        view.info_visible = s.bool(settings::STATE_INFO);
        view.bar_locked = s.bool(settings::STATE_BAR_LOCKED);
        view.edge = edge_mode(&s.choice(settings::STATE_EDGE));
        if s.bool(settings::SAVE_ZOOM_STATE) {
            view.mode = display_mode(&s.choice(settings::STATE_MODE));
            view.stretch_small = s.bool(settings::STATE_STRETCH);
            if s.bool(settings::STATE_ZOOM_LOCK) {
                view.restore_zoom_lock(s.int(settings::STATE_ZOOM) as f64 / 100.0);
            }
        }
    }

    /// 让设置生效（各窗口都生效）：后台任务的选项、缓存上限、查看状态；解码方式变了时清掉缓存，
    /// 各窗口按新方式重新显示当前图片。
    fn apply_runtime(&self, s: &Settings) {
        apply_runtime_to(&self.shared.hub, s);
    }
}

/// 让设置生效（各窗口都生效）：后台任务的选项、缓存上限、查看状态；解码方式变了时清掉缓存，
/// 各窗口按新方式重新显示当前图片。
fn apply_runtime_to(hub: &Arc<Hub>, s: &Settings) {
    {
        let runtime = Runtime::from_settings(s);
        let budget = (s.int(settings::CACHE_MB).max(1) as usize) * 1024 * 1024;
        // 缓存变大了：各窗口可以多预读几张。
        let budget_grew = budget > lock(&hub.cache).budget();
        // 先让每个窗口排着队、正在解的作废，再清缓存：旧方式解出的结果不会在清空之后又放进去。
        let mut reload = Vec::new();
        for window in hub.live_windows() {
            apply_view_settings(&window, s);
            let (decoding_changed, prefetch_on, rescan, display_changed) = {
                let mut st = lock(&window.state);
                let decoding_changed = st.opts.decode != runtime.decode;
                // 打开了预读，或者预读的张数加多了：把多出来的读进来。
                let prefetch_on = runtime.prefetch
                    && (!st.opts.prefetch || runtime.prefetch_count > st.opts.prefetch_count);
                // RAW 的筛选变了：重新扫描在看的文件夹（放开锁以后再做）。
                let rescan = (st.opts.raw != runtime.raw && st.archive.is_none())
                    .then(|| st.watched.clone())
                    .flatten();
                st.opts = runtime;
                if decoding_changed {
                    // 排着队、正在解的都作废（按旧方式解的结果不用，也不放进缓存）。
                    st.in_flight.clear();
                }
                // 使用监视器的 ICC 设置：打开、关掉时按记下的显示器配置文件改效果（像切换反相一样，
                // 缓存里的原图照用），当前图片随后以 ImageUpdated 重新显示。
                let display = if runtime.monitor_icc { st.display_profile } else { None };
                let display_changed = st.effects.display != display;
                st.effects.display = display;
                (decoding_changed, prefetch_on || display_changed, rescan, display_changed)
            };
            if let Some((dir, stamp)) = rescan {
                rescan_folder(&window, dir, stamp);
            }
            reload.push((window, decoding_changed, prefetch_on, display_changed));
        }
        {
            let mut cache = lock(&hub.cache);
            cache.set_budget(budget);
            if reload.iter().any(|(_, changed, _, _)| *changed) {
                cache.clear();
            }
        }
        for (window, decoding_changed, prefetch_on, display_changed) in &reload {
            let mut st = lock(&window.state);
            // 预读的张数、缓存大小可能变了：按新的重新算预读范围；少了的不再保护，缓存会照常挤掉。
            reprotect(window, &mut st);
            if *decoding_changed {
                show_again(window, &mut st);
                continue;
            }
            if *display_changed && let Some(path) = st.current.clone() {
                request_display(window, &mut st, path, Priority::Current);
            }
            if (*prefetch_on || budget_grew) && st.scanned && st.current.is_some() {
                prefetch_neighbors(window, &mut st);
            }
        }
    }
}

/// 设置里影响查看的项（"上部优先"、默认的插值算法）。
fn apply_view_settings(window: &Shared, s: &Settings) {
    let mut view = lock(&window.view);
    view.top_first = s.bool(settings::TOP_FIRST);
    view.interpolation = interpolation(&s.choice(settings::INTERPOLATION));
}

fn sort_id(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "name",
        SortKey::Modified => "modified",
        SortKey::Size => "size",
        SortKey::Type => "type",
    }
}

fn sort_key(id: &str) -> SortKey {
    match id {
        "modified" => SortKey::Modified,
        "size" => SortKey::Size,
        "type" => SortKey::Type,
        _ => SortKey::Name,
    }
}

fn edge_id(edge: EdgeMode) -> &'static str {
    match edge {
        EdgeMode::None => "none",
        EdgeMode::Soften => "soften",
        EdgeMode::SoftenSharpen => "soften_sharpen",
    }
}

fn edge_mode(id: &str) -> EdgeMode {
    match id {
        "none" => EdgeMode::None,
        "soften_sharpen" => EdgeMode::SoftenSharpen,
        _ => EdgeMode::Soften,
    }
}

fn interpolation_id(interpolation: Interpolation) -> &'static str {
    match interpolation {
        Interpolation::Bilinear => "bilinear",
        Interpolation::Bicubic => "bicubic",
        Interpolation::Lanczos => "lanczos",
    }
}

fn interpolation(id: &str) -> Interpolation {
    match id {
        "bicubic" => Interpolation::Bicubic,
        "lanczos" => Interpolation::Lanczos,
        _ => Interpolation::Bilinear,
    }
}

fn mode_id(mode: DisplayMode) -> &'static str {
    match mode {
        DisplayMode::FitWindow => "fit_window",
        DisplayMode::Original => "original",
        DisplayMode::FitWidth => "fit_width",
    }
}

fn display_mode(id: &str) -> DisplayMode {
    match id {
        "original" => DisplayMode::Original,
        "fit_width" => DisplayMode::FitWidth,
        _ => DisplayMode::FitWindow,
    }
}

/// 把改过的"最近打开的"写进文件：放到后台，调用的线程（界面线程）不等磁盘。
fn save_recent_later(shared: &Arc<Shared>) {
    let s = Arc::clone(shared);
    shared.hub.pool.submit(Priority::Scan, move || {
        let _ = lock(&s.hub.recent).save();
    });
}

/// 用户打开了这个文件、文件夹或压缩包：记进"打开最近使用"。只在 Windows 上记，
/// 设置"不保存浏览过的文件夹路径"打开时不记。
fn note_recent(shared: &Arc<Shared>, path: &Path) {
    let hub = &shared.hub;
    if !hub.track_recent.load(AtomicOrdering::Relaxed) {
        return;
    }
    let off = lock(&hub.prefs).settings.bool(settings::NO_RECENT);
    if off {
        return;
    }
    let changed = lock(&hub.recent).note(path);
    if changed {
        save_recent_later(shared);
    }
}

fn open(shared: &Arc<Shared>, path: &Path) {
    let canonical = match crate::paths::canonical(path) {
        Ok(p) => p,
        Err(e) => {
            shared.push(Event::OpenFailed {
                path: path.display().to_string(),
                message: e.to_string(),
            });
            return;
        }
    };
    note_recent(shared, &canonical);
    if canonical.is_dir() {
        open_folder(shared, canonical);
    } else if crate::archive::kind(&canonical).is_some() {
        open_archive(shared, canonical);
    } else {
        open_file(shared, canonical);
    }
}

/// 打开压缩包：在后台列出里面的图片，从上次看到的那一张或第一张开始。
/// 7Z、RAR 同时在后台依次解出。读好、里面有图片才换过去；打不开（有密码、坏了）或没有图片时只提示，
/// 别的都不变：正在看的图片和浏览集合照旧，还能接着翻。
fn open_archive(shared: &Arc<Shared>, archive: PathBuf) {
    let token = {
        let mut st = lock(&shared.state);
        st.open_token += 1;
        st.open_token
    };
    let s = Arc::clone(shared);
    shared.hub.pool.submit(Priority::Scan, move || {
        let result = crate::archive::open(&archive);
        let mut st = lock(&s.state);
        if st.open_token != token {
            return; // 期间又打开了别的
        }
        match result {
            Ok(handle) => {
                let mut entries = archive_entries(&handle, st.sort);
                collection::filter_raw(&mut entries, st.opts.raw, None);
                if entries.is_empty() {
                    s.push(Event::Notice(NoticeKind::EmptyArchive));
                } else {
                    enter_archive(&s, &mut st, archive, handle, entries, true);
                }
            }
            Err(crate::archive::ArchiveError::Password) => {
                s.push(Event::Notice(NoticeKind::ArchivePassword));
            }
            Err(e) => s.push(Event::OpenFailed {
                path: archive.display().to_string(),
                message: e.message().text(s.hub.lang()),
            }),
        }
    });
}

/// 压缩包里的图片作为浏览集合的条目（虚拟路径，名字是包内路径，按当前排序方式排好）。
fn archive_entries(handle: &crate::archive::Archive, order: SortOrder) -> Vec<Entry> {
    let mut entries: Vec<Entry> = handle
        .items
        .iter()
        .map(|item| Entry {
            path: crate::archive::entry_path(&handle.path, &item.name),
            name: item.name.clone(),
            modified: item.modified,
            size: item.size,
        })
        .collect();
    collection::sort_entries(&mut entries, order);
    entries
}

/// 进入压缩包：浏览集合换成包里的图片，从上次看到的那一张或第一张开始。
/// `entries` 不能是空的（没有图片的压缩包不进，由调用方提示）。
fn enter_archive(
    shared: &Arc<Shared>,
    st: &mut State,
    archive: PathBuf,
    handle: Arc<crate::archive::Archive>,
    entries: Vec<Entry>,
    opened: bool,
) {
    if entries.is_empty() {
        return;
    }
    leave_archive(shared, st);
    st.folder = Some(archive.clone());
    st.entries = entries;
    st.scanned = true;
    st.scan_token += 1;
    st.archive = Some(handle);
    st.watched = None;
    let remembered = st
        .opts
        .remember_archive
        .then(|| lock(&shared.hub.positions).get(&archive).map(str::to_string))
        .flatten();
    let index = remembered
        .and_then(|name| st.entries.iter().position(|e| e.name == name))
        .unwrap_or(0);
    let path = st.entries[index].path.clone();
    set_current(shared, st, path, Some(index), opened);
}

/// 离开压缩包（换了浏览集合）：记下的位置写进文件，放掉压缩包（临时文件随之删掉）。
fn leave_archive(shared: &Shared, st: &mut State) {
    if st.archive.take().is_some() {
        let _ = lock(&shared.hub.positions).save();
    }
}

fn open_file(shared: &Arc<Shared>, file: PathBuf) {
    let folder = file.parent().map(Path::to_path_buf);
    let mut st = lock(&shared.state);
    st.open_token += 1;
    let index = if st.scanned && st.folder == folder {
        st.entries.iter().position(|e| e.path == file)
    } else {
        None
    };
    if index.is_none() {
        // 换了文件夹（或文件不在已扫描的列表里）：先显示这张，同时在后台扫描文件夹。
        leave_archive(shared, &mut st);
        st.folder = folder.clone();
        st.clear_entries();
        st.scanned = false;
        st.scan_token += 1;
        if let Some(dir) = folder {
            submit_scan(shared, dir, st.scan_token, st.sort);
        }
    }
    set_current(shared, &mut st, file, index, true);
}

/// 打开文件夹：在后台扫描，里面有图片才换过去，从第一张开始；读不了或没有图片时只提示，别的都不变。
fn open_folder(shared: &Arc<Shared>, dir: PathBuf) {
    let (token, order) = {
        let mut st = lock(&shared.state);
        st.open_token += 1;
        (st.open_token, st.sort)
    };
    let s = Arc::clone(shared);
    shared.hub.pool.submit(Priority::Scan, move || {
        // 先记下文件夹的修改时间再扫描：扫描期间的变化下一次检查时还能发现。
        let stamp = modified_time(&dir);
        let result = collection::scan_folder(&dir, order);
        let mut st = lock(&s.state);
        if st.open_token != token {
            return; // 期间又打开了别的
        }
        let mut entries = match result {
            Ok(entries) => entries,
            Err(e) => {
                s.push(Event::OpenFailed {
                    path: dir.display().to_string(),
                    message: e.to_string(),
                });
                return;
            }
        };
        // 扫描期间改了排序方式时按新的方式重排。
        if st.sort != order {
            collection::sort_entries(&mut entries, st.sort);
        }
        collection::filter_raw(&mut entries, st.opts.raw, None);
        if entries.is_empty() {
            s.push(Event::Notice(NoticeKind::EmptyFolder));
            return;
        }
        leave_archive(&s, &mut st);
        st.folder = Some(dir.clone());
        st.entries = entries;
        st.scanned = true;
        st.scan_token += 1;
        st.watched = Some((dir, stamp));
        let path = st.entries[0].path.clone();
        set_current(&s, &mut st, path, Some(0), true);
    });
}

fn close(shared: &Arc<Shared>) {
    let mut st = lock(&shared.state);
    st.generation += 1;
    stop_animation(shared);
    st.current_frame = None;
    st.current = None;
    st.index = None;
    st.spread = None;
    st.composing = None;
    st.watched_files.clear();
    lock(&shared.hub.cache).protect(shared.id, Vec::new(), 0);
    shared.push(Event::Closed {
        generation: st.generation,
    });
}

/// 矢量图（SVG）不是按 100% 的物理像素显示时，都要按屏幕像素重新渲染才清晰。
fn vector_needs_exact(source: &DecodedImage, view: &ViewState) -> bool {
    source.vector.is_some() && view.layout().is_some_and(|l| (l.scale - 1.0).abs() > 1e-9)
}

/// 后台生成一张缩略图（选择图片窗口）。开始前看界面还要不要；缓存里有原图就直接缩小。
fn make_thumbnail(
    shared: &Arc<Shared>,
    path: PathBuf,
    modified: Option<SystemTime>,
    options: crate::decode::DecodeOptions,
) {
    let still_wanted = lock(&shared.picker).wanted.contains(&path);
    if !still_wanted {
        lock(&shared.picker).in_flight.remove(&path);
        return;
    }
    let original = lock(&shared.hub.cache).get(&path, Effects::NONE);
    let result = crate::picker::make_thumbnail(&path, options, original);
    let image = match result {
        Ok(thumb) => {
            let thumb = Arc::new(thumb);
            lock(&shared.hub.thumbs).insert(path.clone(), modified, Arc::clone(&thumb));
            Some(thumb)
        }
        Err(_) => {
            lock(&shared.hub.thumbs).mark_failed(path.clone(), modified);
            None
        }
    };
    lock(&shared.picker).in_flight.remove(&path);
    shared.push(Event::Thumbnail { path, image });
}

fn navigate(shared: &Arc<Shared>, navigation: Navigation) {
    let mut st = lock(&shared.state);
    if !st.scanned || st.entries.is_empty() {
        return; // 文件夹还没扫描完，先忽略这次按键
    }
    if st.two_page.is_some() {
        navigate_spread(shared, &mut st, navigation);
        return;
    }
    let last = st.entries.len() - 1;
    let target = match (navigation, st.index) {
        (Navigation::First, _) => 0,
        (Navigation::Last, _) => last,
        (Navigation::Next | Navigation::Forward10, Some(i)) if i >= last => {
            match at_boundary(shared, &st, true) {
                Some(target) => target,
                None => return,
            }
        }
        (Navigation::Prev | Navigation::Back10, Some(0)) => match at_boundary(shared, &st, false) {
            Some(target) => target,
            None => return,
        },
        (Navigation::Next, Some(i)) => i + 1,
        (Navigation::Prev, Some(i)) => i - 1,
        (Navigation::Jump(i), _) => i.min(last),
        // 跳 10 张越界时停在首张或末张。
        (Navigation::Forward10, Some(i)) => (i + 10).min(last),
        (Navigation::Back10, Some(i)) => i.saturating_sub(10),
        (Navigation::Next | Navigation::Forward10, None) => 0,
        (Navigation::Prev | Navigation::Back10, None) => last,
    };
    if st.index == Some(target) {
        return;
    }
    // 翻页的方向：下一张、跳 10 张、末张往后；跳转按位置比较；到末张后回到首张（循环）也算往后。
    let forward = match navigation {
        Navigation::Next | Navigation::Forward10 | Navigation::Last => true,
        Navigation::Prev | Navigation::Back10 | Navigation::First => false,
        Navigation::Jump(_) => st.index.is_none_or(|i| target > i),
    };
    st.direction_hint = Some(forward);
    let path = st.entries[target].path.clone();
    set_current(shared, &mut st, path, Some(target), false);
}

/// 双页时的翻页：一次翻一组。往后从这一组的下一张开始；往回到这一组前面那张为止
/// （那一组以它结尾）；首张往后、末张往回成组；跳转、跳 10 张从目标往后成组。
/// 当前图片不在列表里时与单页一样：往后到首张，往回到末张。
fn navigate_spread(shared: &Arc<Shared>, st: &mut State, navigation: Navigation) {
    let last = st.entries.len() - 1;
    let range = spread_range(st);
    let (target, backward) = match navigation {
        Navigation::First => (0, false),
        Navigation::Last => (last, true),
        Navigation::Jump(i) => (i.min(last), false),
        Navigation::Next | Navigation::Forward10 => {
            let step = if navigation == Navigation::Next { 1 } else { 10 };
            match range {
                None => (0, false),
                Some((_, high)) if high >= last => match at_boundary(shared, st, true) {
                    Some(target) => (target, false),
                    None => return,
                },
                Some((_, high)) => ((high + step).min(last), false),
            }
        }
        Navigation::Prev | Navigation::Back10 => match range {
            None => (last, true),
            Some((0, _)) => match at_boundary(shared, st, false) {
                Some(target) => (target, true),
                None => return,
            },
            Some((low, _)) if navigation == Navigation::Prev => (low - 1, true),
            // 往回跳 10 张和往后跳一样，从目标往后成组（往回成组的话和原来的分组错开一页）。
            Some((low, _)) => (low.saturating_sub(10), false),
        },
    };
    if st.index == Some(target) && st.spread_backward == backward && st.spread.is_some() {
        return;
    }
    st.next_backward = backward;
    st.direction_hint = Some(match navigation {
        Navigation::Next | Navigation::Forward10 | Navigation::Last => true,
        Navigation::Prev | Navigation::Back10 | Navigation::First => false,
        Navigation::Jump(i) => st.index.is_none_or(|current| i > current),
    });
    let path = st.entries[target].path.clone();
    set_current(shared, st, path, Some(target), false);
}

/// 到达首尾时（设置"到达首尾图片时执行"）：询问、提示、循环或转到相邻文件夹。
/// 循环时返回要转到的序号；其余情况已经处理完，返回空。
fn at_boundary(shared: &Arc<Shared>, st: &State, forward: bool) -> Option<usize> {
    // 压缩包里按"到达压缩包内首尾图片时执行"。
    let archive = st.archive.is_some();
    let rule = if archive {
        st.opts.archive_boundary
    } else {
        st.opts.boundary
    };
    match rule {
        Boundary::Ask => shared.push(Event::Boundary { forward, archive }),
        Boundary::Nothing => shared.push(Event::Notice(if forward {
            NoticeKind::AtLast
        } else {
            NoticeKind::AtFirst
        })),
        Boundary::Loop => {
            return Some(if forward { 0 } else { st.entries.len() - 1 });
        }
        Boundary::NextFolder => {
            // navigate_folder 在后台任务里取 state 锁，这里持着锁提交也不会死锁。
            navigate_folder_from(shared, st, forward);
        }
    }
    None
}

/// 切换当前图片。`opened` 为真表示由"打开"（文件、文件夹）引起，界面据此按图片调整窗口大小；浏览中切图为假。
/// 可能在界面线程上调用：这里不读磁盘（文件有没有改过由后台检查线程核对，见 [`show`]）。
fn set_current(
    shared: &Arc<Shared>,
    st: &mut State,
    path: PathBuf,
    index: Option<usize>,
    opened: bool,
) {
    st.generation += 1;
    let generation = st.generation;
    let hint = st.direction_hint.take();
    // 预读跟着翻页的方向走；“打开”从头往后看。刷新、删除后显示下一张等不改方向。
    if let Some(forward) = hint {
        st.heading_back = !forward;
    } else if opened {
        st.heading_back = false;
    }
    st.turn = hint.map(|forward| Turn {
        generation,
        forward,
        asked: false,
    });
    stop_animation(shared);
    st.current_frame = None;
    st.index = index;
    st.current = Some(path.clone());
    st.spread_backward = std::mem::take(&mut st.next_backward);
    st.rereads.clear();
    st.spread = None;
    st.composing = None;
    // 这一代次还没显示出来：显示着的文件等显示时再记。
    st.watched_files.clear();
    // 缓存不挤掉这个窗口的当前图片和预读的图片（锁的顺序：state → cache）。
    reprotect(shared, st);
    st.recent.retain(|p| p != &path);
    st.recent.push_back(path.clone());
    if st.recent.len() > RECENT_LIMIT {
        st.recent.pop_front();
    }
    // 压缩包里的图片：记下看到哪一张。
    if st.opts.remember_archive
        && let Some(archive) = &st.archive
        && let Ok(inner) = path.strip_prefix(&archive.path)
    {
        let inner = inner
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        lock(&shared.hub.positions).set(&archive.path, &inner);
    }
    let total = st.scanned.then_some(st.entries.len());
    shared.push(Event::Current {
        generation,
        path: path.clone(),
        name: title(st, std::slice::from_ref(&path)),
        index,
        total,
        opened,
    });
    // 上次没读出来的这次重新试（文件可能已经好了）；双页的另一页也是，要在请求当前图片之前：
    // 当前图片在缓存里时马上就要看另一页。
    let other = spread_neighbor(st).map(|j| st.entries[j].path.clone());
    st.failed.remove(&path);
    if let Some(other) = &other {
        st.failed.remove(other);
    }
    let ready_now = request_display(shared, st, path, Priority::Current);
    // 双页：同一组的另一页也马上读。
    if let Some(other) = other {
        request_display(shared, st, other, Priority::Current);
    }
    // 当前图片已经就绪才开始预加载，不和它抢 CPU；否则等它解码完再预加载。
    if ready_now && st.scanned {
        prefetch_neighbors(shared, st);
    }
}

/// 标题、图片信息里的名字：文件名，双页成组时是两页的名字；压缩包里的图片前面加上压缩包的名字。
fn title(st: &State, pages: &[PathBuf]) -> String {
    let name = pages
        .iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let archive = st
        .archive
        .as_ref()
        .filter(|a| pages.iter().all(|p| p.starts_with(&a.path)))
        .and_then(|a| a.path.file_name());
    match archive {
        Some(file) => format!("{} › {}", file.to_string_lossy(), name),
        None => name,
    }
}

/// 双页时与当前图片可能成一组的另一页：往后翻时是下一张，往回翻时是上一张。
fn spread_neighbor(st: &State) -> Option<usize> {
    st.two_page?;
    let i = st.index.filter(|&i| i < st.entries.len())?;
    if st.spread_backward {
        i.checked_sub(1)
    } else {
        (i + 1 < st.entries.len()).then_some(i + 1)
    }
}

/// 双页时这张图片是不是和当前图片一起显示的另一页（或当前图片自己）：这一代次已经显示了一组就看那一组，
/// 还没有就看列表里挨着的那张。
fn is_spread_page(st: &State, path: &Path) -> bool {
    if st.two_page.is_none() {
        return false;
    }
    match &st.spread {
        Some(shown) => shown.paths.iter().any(|p| p == path),
        None => spread_neighbor(st).is_some_and(|j| st.entries[j].path == path),
    }
}

/// 显示着的这一组现在占列表里的哪一段（最小、最大序号）；没有成组时就是当前图片自己。当前图片不在列表里时为空。
/// 每次按路径现找：列表排序、刷新、删掉文件后序号会变；另一页已经不挨着当前图片了就只算当前这一张。
fn spread_range(st: &State) -> Option<(usize, usize)> {
    let i = st.index.filter(|&i| i < st.entries.len())?;
    let other = st
        .spread
        .as_ref()
        .filter(|shown| shown.paths.len() == 2)
        .and_then(|shown| shown.paths.iter().find(|p| Some(*p) != st.current.as_ref()));
    if let Some(other) = other {
        if st.entries.get(i + 1).is_some_and(|e| e.path == *other) {
            return Some((i, i + 1));
        }
        if i > 0 && st.entries[i - 1].path == *other {
            return Some((i - 1, i));
        }
    }
    Some((i, i))
}

/// 列表变了（排序、文件夹刷新、删了文件）以后双页要做的事：
/// - 显示着的另一页不在了：这一组重新成；
/// - 这一代次还没显示出来：按新的列表再看一次能不能成组。挨着当前图片的可能换了一张，原来等的那一张
///   读好了不会再来叫，不看的话就一直不显示。
///
/// 已经显示着、两页都还在的不动（见 [`Spread::paths`]）。
fn regroup_after_list_change(shared: &Arc<Shared>, st: &mut State) {
    if st.two_page.is_none() {
        return;
    }
    if spread_page_gone(st) {
        show_again(shared, st);
    } else if st.spread.is_none() {
        push_spread(shared, st);
    }
}

/// 双页时显示着（或正在合成）的另一页已经不在列表里了（文件被删掉、改名）。
fn spread_page_gone(st: &State) -> bool {
    let shown = st.spread.iter().flat_map(|s| &s.paths);
    let composing = st.composing.iter().flat_map(|c| &c.paths);
    shown
        .chain(composing)
        .any(|p| Some(p) != st.current.as_ref() && st.position(p).is_none())
}

/// 双页里一页按现在效果的图片。缓存里没有、`request` 为真时去要（读好了会再来成组）；要的那一下发现
/// 已经有了（别的窗口共用缓存，刚放进去）就直接用——那种情况没有任务会再来叫。
fn spread_page(
    shared: &Arc<Shared>,
    st: &mut State,
    path: &Path,
    request: bool,
) -> Option<Arc<DecodedImage>> {
    let effects = st.effects;
    let cached = lock(&shared.hub.cache).get(path, effects);
    if cached.is_some() || !request {
        return cached;
    }
    if request_display(shared, st, path.to_path_buf(), Priority::Current) {
        return lock(&shared.hub.cache).get(path, effects);
    }
    None
}

/// 双页：这一组显示哪几页、各自的图片（按当前效果）。还缺解码结果时为空（解码好了会再来）。
/// 当前图片是横图、没有另一页、另一页是横图或解码失败时只显示当前这一张。
fn resolve_spread(shared: &Arc<Shared>, st: &mut State) -> Option<Resolved> {
    // 这一代次已经显示了一组：页不换，只按现在的效果取图（换了效果）。另一页缺处理好的版本时去要
    // （当前图片的由改效果的一方要过了）；读不了的就算了（保持原样）。
    if let Some(shown) = &st.spread {
        let (paths, missing) = (shown.paths.clone(), shown.missing.clone());
        let mut images = Vec::with_capacity(paths.len());
        for path in &paths {
            let request = st.current.as_ref() != Some(path) && !st.failed.contains_key(path);
            images.push(spread_page(shared, st, path, request)?);
        }
        return Some(Resolved {
            paths,
            images,
            missing,
        });
    }
    let current = st.current.clone()?;
    let single = |current: PathBuf, image: Arc<DecodedImage>, missing| Resolved {
        paths: vec![current],
        images: vec![image],
        missing,
    };
    let Some(i) = st.index.filter(|&i| i < st.entries.len()) else {
        // 列表还没扫描好：等它（那时才知道另一页是谁，扫描完会再来；先单独显示再换成两页的话，
        // 一个代次里显示的页就变了）。
        if !st.scanned {
            return None;
        }
        // 列表好了、当前图片不在里面（读不了的文件夹，列表不收的文件）：只显示这一张。
        let image = spread_page(shared, st, &current, false)?;
        return Some(single(current, image, None));
    };
    let anchor = spread_page(shared, st, &current, false)?;
    if crate::spread::is_landscape(&anchor) {
        return Some(single(current, anchor, None));
    }
    let Some(j) = spread_neighbor(st) else {
        return Some(single(current, anchor, None));
    };
    let other_path = st.entries[j].path.clone();
    // 另一页读不了：单独显示当前这一张，同时盯着那个文件，它变了再重新成组。
    if let Some(stamp) = st.failed.get(&other_path) {
        let missing = stamp.map(|stamp| (other_path, stamp));
        return Some(single(current, anchor, missing));
    }
    // 另一页还没好时去要（有效果时可能只缺处理好的版本）。
    let other = spread_page(shared, st, &other_path, true)?;
    if crate::spread::is_landscape(&other) {
        return Some(single(current, anchor, None));
    }
    Some(if j < i {
        Resolved {
            paths: vec![other_path, current],
            images: vec![other, anchor],
            missing: None,
        }
    } else {
        Resolved {
            paths: vec![current, other_path],
            images: vec![anchor, other],
            missing: None,
        }
    })
}

/// 双页：这一组能显示了就显示。两页成一组时在后台合成一张（要复制、缩放两整页的像素，
/// 几十到几百毫秒：界面线程和等状态锁的别的线程都不等它），合成好了再送。
fn push_spread(shared: &Arc<Shared>, st: &mut State) {
    let Some(direction) = st.two_page else {
        return;
    };
    let effects = st.effects;
    // 这一代次的这一组已经按这个效果显示着了。
    if st.spread.as_ref().is_some_and(|shown| shown.effects == effects) {
        return;
    }
    let Some(Resolved {
        paths,
        images,
        missing,
    }) = resolve_spread(shared, st)
    else {
        return;
    };
    let [first, second] = images.as_slice() else {
        // 只有一页：和单页一样显示。
        st.composing = None;
        let image = Arc::clone(&images[0]);
        let stamps = vec![stamp_of(&paths[0], &image)];
        show_spread(shared, st, paths, image, effects, stamps, missing);
        return;
    };
    let request = Composing {
        generation: st.generation,
        paths,
        effects,
        direction,
    };
    if st.composing.as_ref() == Some(&request) {
        return; // 这一组正在合成
    }
    st.composing = Some(request.clone());
    let (first, second) = (Arc::clone(first), Arc::clone(second));
    let s = Arc::clone(shared);
    shared.hub.pool.submit(Priority::Current, move || {
        let composed = no_panic(|| crate::spread::compose(&first, &second, direction));
        let mut st = lock(&s.state);
        // 合成期间翻走了，或者换了效果、方向（那一组另有任务在合成）：这一张不要了。
        if st.composing.as_ref() != Some(&request) {
            return;
        }
        st.composing = None;
        let stamps = vec![
            stamp_of(&request.paths[0], &first),
            stamp_of(&request.paths[1], &second),
        ];
        match composed {
            Some(image) => {
                show_spread(&s, &mut st, request.paths, Arc::new(image), effects, stamps, None);
            }
            // 合成出错了：不能没有下文，单独显示当前这一张。
            None => {
                let at = usize::from(st.current.as_ref() == Some(&request.paths[1]));
                let image = if at == 0 { first } else { second };
                let (path, stamp) = (request.paths[at].clone(), stamps[at].clone());
                show_spread(&s, &mut st, vec![path], image, effects, vec![stamp], None);
            }
        }
    });
}

/// 运行 `f`；里面出错（panic）时为空。后台任务出了错不能就这么没了下文：等它结果的一方要有个交代
/// （线程池只保证线程还在）。
fn no_panic<T>(f: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok()
}

/// 一页的文件和这一份解码时文件的修改时间、大小（之后核对文件有没有改过）。
fn stamp_of(path: &Path, image: &DecodedImage) -> (PathBuf, FileStamp) {
    (path.to_path_buf(), (image.info.modified, image.info.file_size))
}

/// 双页：把这一组（一页，或合成好的两页）交给界面。两页成一组时另送一个 `Current`，标题是两页的名字。
fn show_spread(
    shared: &Arc<Shared>,
    st: &mut State,
    paths: Vec<PathBuf>,
    image: Arc<DecodedImage>,
    effects: Effects,
    mut stamps: Vec<(PathBuf, FileStamp)>,
    missing: Option<(PathBuf, FileStamp)>,
) {
    let Some(path) = st.current.clone() else {
        return;
    };
    let first_time = st.spread.is_none();
    let paired = paths.len() == 2;
    let name = (paired && first_time).then(|| title(st, &paths));
    // 读不了的另一页也盯着：它的文件变了就重新成组。
    stamps.extend(missing.clone());
    st.spread = Some(Spread {
        paths,
        image: Arc::clone(&image),
        effects,
        missing,
    });
    if let Some(name) = name {
        shared.push(Event::Current {
            generation: st.generation,
            path: path.clone(),
            name,
            index: spread_range(st).map(|(low, _)| low),
            total: st.scanned.then_some(st.entries.len()),
            opened: false,
        });
    }
    if paired {
        show(shared, st, path, image, stamps);
    } else {
        show_single(shared, st, path, image, stamps);
    }
}

/// 送出当前图片的显示缓冲区（按当前效果）。
fn push_display(shared: &Arc<Shared>, st: &mut State, path: PathBuf, image: Arc<DecodedImage>) {
    // 双页：合成这一组再送。
    if st.two_page.is_some() {
        push_spread(shared, st);
        return;
    }
    let stamps = vec![stamp_of(&path, &image)];
    show_single(shared, st, path, image, stamps);
}

/// 把一张图片（不是合成的两页）交给界面；可能是动图时开始播放。
fn show_single(
    shared: &Arc<Shared>,
    st: &mut State,
    path: PathBuf,
    image: Arc<DecodedImage>,
    stamps: Vec<(PathBuf, FileStamp)>,
) {
    // 正在播放的动图换了效果：由播放线程按新效果重送当前帧（不要退回第一帧）。
    if let Some(control) = animation_control(shared, st.generation) {
        control.change(|a| a.refresh = true);
        return;
    }
    start_animation(shared, st, &path, &image);
    show(shared, st, path, image, stamps);
}

/// 把图片交给界面，并记下显示的是文件的哪一份（`stamps`：显示着的文件和这一份解码时的修改时间、大小）。
///
/// 界面在这一代次已经按这个尺寸排过版（显示过这张图，或它的快速预览）时只换内容（`ImageUpdated`，不复位
/// 查看状态）；否则按新图片显示（`ImageReady`）。只看这一条，不看调用的一方是不是"只是换了效果"：换效果的
/// 结果送到时用户可能已经翻走又翻回来了，这一代次界面还什么都没显示过，只换内容的话不会重新排版。
fn show(
    shared: &Arc<Shared>,
    st: &mut State,
    path: PathBuf,
    image: Arc<DecodedImage>,
    stamps: Vec<(PathBuf, FileStamp)>,
) {
    let generation = st.generation;
    let size = (generation, image.width, image.height);
    let again = st.shown == Some(size);
    st.shown = Some(size);
    // 文件改过后重新显示；压缩包里的不看。显示的可能是缓存里的旧图（解码之后文件被改过）：
    // 叫后台检查线程马上核对，不等下一轮。翻页常在界面线程上，这里不读磁盘。
    st.watched_files = if st.archive.is_some() { Vec::new() } else { stamps };
    if !st.watched_files.is_empty() {
        shared.hub.watch.poke();
    }
    shared.push(if again {
        Event::ImageUpdated {
            generation,
            path,
            image,
        }
    } else {
        Event::ImageReady {
            generation,
            path,
            image,
        }
    });
}

/// 切走了：停下正在播放的动图（暂停中的播放线程也结束）。
fn stop_animation(shared: &Shared) {
    if let Some(handle) = lock(&shared.anim).take() {
        handle.control.stop();
    }
}

/// 这一代次正在播放的动图的控制（确认是动图的才算）。
fn animation_control(shared: &Shared, generation: u64) -> Option<Arc<AnimControl>> {
    let slot = lock(&shared.anim);
    let handle = slot.as_ref().filter(|h| h.generation == generation)?;
    lock(&handle.control.state)
        .animated
        .then(|| Arc::clone(&handle.control))
}

/// 当前图片可能是动图（GIF、APNG、WebP 等）时开始播放（每代次一次）；之前的动画停下。
/// 是不是动图要读文件才知道，由播放线程去看：这里可能是界面线程、还占着状态锁，不能读磁盘
/// （压缩包里的图片还可能要等后台解压到它）。
fn start_animation(shared: &Arc<Shared>, st: &State, path: &Path, image: &DecodedImage) {
    let generation = st.generation;
    let control = {
        let mut slot = lock(&shared.anim);
        if slot.as_ref().is_some_and(|h| h.generation == generation) {
            return;
        }
        if let Some(old) = slot.take() {
            old.control.stop();
        }
        // 超大图（有代理纹理）不放动画；JPEG 没有动图，不用起线程去看。
        if image.proxy.is_some() || image.info.format == "JPEG" {
            return;
        }
        let control = Arc::new(AnimControl::default());
        *slot = Some(AnimHandle {
            generation,
            control: Arc::clone(&control),
        });
        control
    };
    let player = Player {
        shared: Arc::downgrade(shared),
        path: path.to_path_buf(),
        generation,
        control,
        size: (image.width, image.height),
        info: image.info.clone(),
    };
    let _ = thread::Builder::new()
        .name("anyview-anim".into())
        .spawn(move || player.run());
}

/// 播放线程：按帧的时长送出帧，暂停时等着；切到别的图片（代次变了）或窗口关掉时结束。
struct Player {
    shared: Weak<Shared>,
    path: PathBuf,
    generation: u64,
    control: Arc<AnimControl>,
    size: (u32, u32),
    /// 第一帧的图片信息：各帧沿用（格式、ICC 等）。
    info: crate::decode::ImageInfo,
}

/// 读出来、转成显示格式的一帧（还没加效果）和它的时长。
type PlayerFrame = (Arc<DecodedImage>, u32);

/// 播放线程取帧：整段动画不大时全部留在内存里，太大时边播边解（往回时从头再读）。
struct FrameSource {
    /// 读帧用的文件（压缩包里的图片是解出的临时文件）。
    file: PathBuf,
    reader: crate::anim::FrameReader,
    cache: Vec<PlayerFrame>,
    cached_bytes: usize,
    caching: bool,
    /// 这一遍已经从读取器里读了几帧。
    read: usize,
    /// 总帧数（读完一遍后才知道）。
    count: Option<usize>,
}

impl FrameSource {
    /// 第 n 帧；超出最后一帧（这时记下总帧数）或读不了时为空。
    fn get(&mut self, player: &Player, n: usize) -> Option<PlayerFrame> {
        if self.caching && n < self.cache.len() {
            return Some(self.cache[n].clone());
        }
        if self.count.is_some_and(|c| n >= c) {
            return None;
        }
        if n < self.read {
            self.reader = crate::anim::FrameReader::open(&self.file)?;
            self.read = 0;
        }
        loop {
            let Some(frame) = player.read(&mut self.reader) else {
                if self.read > 0 {
                    self.count = Some(self.read);
                }
                return None;
            };
            let i = self.read;
            self.read += 1;
            if self.caching && i == self.cache.len() {
                self.cached_bytes += frame.0.byte_len();
                if self.cached_bytes > crate::anim::FRAME_CACHE_BYTES {
                    self.caching = false;
                    self.cache.clear();
                } else {
                    self.cache.push(frame.clone());
                }
            }
            if i == n {
                return Some(frame);
            }
        }
    }
}

impl Player {
    fn run(self) {
        // 起线程的这一会儿已经翻走了：不用看了（压缩包可能已经关了，别为它再打开一次）。
        if lock(&self.control.state).stop {
            return;
        }
        // 压缩包里的图片读解出的临时文件（可能要等后台解压到它；拿着 `opened` 的时候压缩包不关、文件不删）。
        let Ok(opened) = crate::archive::materialize(&self.path) else {
            return;
        };
        if !crate::anim::may_animate(&opened.file) {
            return;
        }
        let Some(reader) = crate::anim::FrameReader::open(&opened.file) else {
            return;
        };
        let mut source = FrameSource {
            file: opened.file.clone(),
            reader,
            cache: Vec::new(),
            cached_bytes: 0,
            caching: true,
            read: 0,
            count: None,
        };
        // 读到第二帧才算动图（只有一帧的 GIF 照常是静态图片）；尺寸要与已经显示的图片相同。
        let Some(mut current) = source.get(&self, 0) else {
            return;
        };
        if source.get(&self, 1).is_none() {
            return;
        }
        lock(&self.control.state).animated = true;
        let mut shown = 0usize;
        if !self.deliver(&current, shown, source.count) {
            return;
        }
        let mut due = Instant::now() + Duration::from_millis(u64::from(current.1));
        loop {
            // 等到该换下一帧（暂停时一直等），或者要逐帧、重送、结束。
            let step = {
                let mut a = lock(&self.control.state);
                loop {
                    if a.stop {
                        return;
                    }
                    if a.refresh {
                        a.refresh = false;
                        break 0;
                    }
                    if a.step != 0 {
                        let step = a.step;
                        a.step = 0;
                        break step;
                    }
                    if a.paused {
                        a = self
                            .control
                            .wake
                            .wait(a)
                            .unwrap_or_else(|p| p.into_inner());
                        continue;
                    }
                    let now = Instant::now();
                    if now >= due {
                        break 1;
                    }
                    a = self
                        .control
                        .wake
                        .wait_timeout(a, due - now)
                        .unwrap_or_else(|p| p.into_inner())
                        .0;
                }
            };
            if step != 0 {
                let wanted = if step > 0 {
                    shown + 1
                } else if shown > 0 {
                    shown - 1
                } else {
                    // 从第一帧往回到最后一帧：总帧数还不知道时先读完一遍。
                    if source.count.is_none() {
                        let _ = source.get(&self, usize::MAX - 1);
                    }
                    source.count.map_or(0, |c| c - 1)
                };
                let (index, frame) = match source.get(&self, wanted) {
                    Some(frame) => (wanted, frame),
                    // 播完一遍：从第一帧接着播（循环）。
                    None => match source.get(&self, 0) {
                        Some(frame) => (0, frame),
                        None => return,
                    },
                };
                shown = index;
                current = frame;
            }
            if !self.deliver(&current, shown, source.count) {
                return;
            }
            // 帧都留在内存里时先读好下一帧：播放更稳，读完一遍时早一点知道总帧数（动画信息控制里显示）。
            if source.caching {
                let _ = source.get(&self, shown + 1);
                lock(&self.control.state).count = source.count;
            }
            // 下一帧的时间：落后太多（解码慢）时不追赶，从现在算；逐帧以后也从现在算。
            let now = Instant::now();
            let delay = Duration::from_millis(u64::from(current.1));
            due = if step == 1 && due + delay > now {
                due + delay
            } else {
                now + delay
            };
        }
    }

    /// 读下一帧并转成显示格式；读完一遍、出错或尺寸不对时为空。
    fn read(&self, reader: &mut crate::anim::FrameReader) -> Option<PlayerFrame> {
        let frame = reader.next_frame().ok()??;
        if (frame.width, frame.height) != self.size {
            return None;
        }
        let mut pixels = frame.rgba;
        if !frame.bgra {
            crate::decode::rgba_to_bgra_premultiplied(&mut pixels);
        }
        let image = DecodedImage::new(frame.width, frame.height, pixels, self.info.clone());
        Some((Arc::new(image), frame.delay_ms))
    }

    /// 送出一帧（按当前效果）。图片已经换了或窗口关了时返回 false。
    fn deliver(&self, frame: &PlayerFrame, index: usize, count: Option<usize>) -> bool {
        let Some(shared) = self.shared.upgrade() else {
            return false;
        };
        let effects = {
            let st = lock(&shared.state);
            if st.generation != self.generation {
                return false;
            }
            st.effects
        };
        let image = if effects.is_none() {
            Arc::clone(&frame.0)
        } else {
            Arc::new(effects::apply(&frame.0, effects))
        };
        {
            let mut a = lock(&self.control.state);
            if a.stop {
                return false;
            }
            a.index = index;
            a.count = count;
        }
        let mut st = lock(&shared.state);
        if st.generation != self.generation {
            return false;
        }
        st.current_frame = Some((self.generation, Arc::clone(&frame.0)));
        shared.push(Event::AnimationFrame {
            generation: self.generation,
            path: self.path.clone(),
            image,
            index,
            count,
        });
        true
    }
}

/// 请求一张图片的显示缓冲区：有缓存时立即送达（返回 true）；只有原图时在后台加效果；都没有时安排解码。
fn request_display(shared: &Arc<Shared>, st: &mut State, path: PathBuf, priority: Priority) -> bool {
    let effects = st.effects;
    let (cached, original) = {
        let mut cache = lock(&shared.hub.cache);
        let cached = cache.get(&path, effects);
        let original = if cached.is_none() && !effects.is_none() {
            cache.get(&path, Effects::NONE)
        } else {
            None
        };
        (cached, original)
    };
    if let Some(image) = cached {
        if st.current.as_ref() == Some(&path) {
            push_display(shared, st, path, image);
        }
        return true;
    }
    if let Some(original) = original {
        let s = Arc::clone(shared);
        shared.hub.pool.submit(priority, move || {
            let Some(processed) = no_panic(|| effects::apply(&original, effects)) else {
                // 加效果出错了：等着它显示的当前图片按解码失败报，不能一直没有下文。
                let mut st = lock(&s.state);
                if st.current.as_ref() == Some(&path) && st.effects == effects {
                    fail_current(&s, &mut st, path, "处理图像时出错".to_string(), None, false);
                }
                return;
            };
            let processed = Arc::new(processed);
            let mut st = lock(&s.state);
            let same = {
                let mut cache = lock(&s.hub.cache);
                // 处理期间缓存里这张图片的原图换了一份或者没了（文件改了重新读过、解码方式变了、别的窗口
                // 又读了一遍、文件删了）：手上这一份是按旧的做的，不能放进去、更不能显示。
                let same = cache
                    .get(&path, Effects::NONE)
                    .is_some_and(|now| Arc::ptr_eq(&now, &original));
                if same {
                    protect_window(&mut st, &mut cache, s.id);
                    cache.insert(path.clone(), effects, Arc::clone(&processed));
                }
                same
            };
            if !same {
                // 还等着它显示的话按现在的那一份再要一次（不然换了一份以后没有人再来送）。
                if wanted(&st, &path) && st.effects == effects {
                    request_display(&s, &mut st, path, priority);
                }
                return;
            }
            if st.current.as_ref() == Some(&path) {
                if st.effects == effects {
                    push_display(&s, &mut st, path, processed);
                }
            } else if is_spread_page(&st, &path) {
                push_spread(&s, &mut st);
            }
        });
        return false;
    }
    // 当前图片要等解码、这一代次界面还什么都没显示：先在另一个任务里读快速预览，与解码同时进行。
    if priority == Priority::Current
        && st.opts.fast_preview
        && st.two_page.is_none()
        && st.current.as_ref() == Some(&path)
        && st.shown.is_none_or(|(generation, ..)| generation != st.generation)
    {
        let s = Arc::clone(shared);
        let generation = st.generation;
        let target = path.clone();
        shared.hub.pool.submit(Priority::Current, move || {
            preview_job(&s, target, generation)
        });
    }
    let ticket = match st.in_flight.get_mut(&path) {
        None => {
            st.next_ticket += 1;
            let ticket = st.next_ticket;
            st.in_flight.insert(
                path.clone(),
                Flight {
                    ticket,
                    priority,
                    started: false,
                },
            );
            Some(ticket)
        }
        // 已经排着队、但排在后面（当预加载请求的，现在成了当前图片）：按现在的优先级再排一个任务。
        // 两个任务谁先轮到谁解，另一个轮到时看到已经有人在解（或解完了）就不做。
        Some(flight) if !flight.started && priority < flight.priority => {
            flight.priority = priority;
            Some(flight.ticket)
        }
        Some(_) => None,
    };
    if let Some(ticket) = ticket {
        let s = Arc::clone(shared);
        shared
            .hub
            .pool
            .submit(priority, move || decode_job(&s, path, ticket));
    }
    false
}

/// 快速预览：读 EXIF 内嵌的缩略图，清晰图还没送出时先送它。
fn preview_job(shared: &Arc<Shared>, path: PathBuf, generation: u64) {
    let options = {
        let st = lock(&shared.state);
        if st.generation != generation {
            return; // 已经翻走了
        }
        st.opts.decode
    };
    // 压缩包里的图片读解出的临时文件。
    let Ok(source) = crate::archive::materialize(&path) else {
        return;
    };
    let Some(preview) = crate::jpeg::preview_with(&source.file, options) else {
        return;
    };
    let effects = lock(&shared.state).effects;
    let image = if effects.is_none() {
        preview.image
    } else {
        effects::apply(&preview.image, effects)
    };
    let mut st = lock(&shared.state);
    if st.generation != generation || st.effects != effects {
        return;
    }
    // 这一代次已经给过界面东西了（清晰图、另一份预览，或者已经报了解码失败）：不再送预览。
    if st.shown.is_some_and(|(shown, ..)| shown == generation) {
        return;
    }
    // 清晰图已经解码好（马上送出）也不送，免得盖住清晰图。锁的顺序：state → cache。
    {
        let cache = lock(&shared.hub.cache);
        if cache.contains(&path, Effects::NONE) || cache.contains(&path, effects) {
            return;
        }
    }
    st.shown = Some((generation, preview.width, preview.height));
    shared.push(Event::ImagePreview {
        generation,
        path,
        image: Arc::new(image),
        width: preview.width,
        height: preview.height,
    });
}

fn prefetch_neighbors(shared: &Arc<Shared>, st: &mut State) {
    if !st.opts.prefetch {
        return; // 设置“预先读好接下来的图片”关掉了
    }
    // 列表（扫描完、刷新、排序）、方向、缓存里的东西都可能变了：先按现在的重新算预读范围。
    // 不然当前图片解开时列表还没扫描完，算出来只有它自己一张，扫描完了也一张都不预读。
    reprotect(shared, st);
    for path in st.window() {
        if st.current.as_ref() != Some(&path) {
            request_display(shared, st, path, Priority::Prefetch);
        }
    }
}

/// 这张图片现在还用得着：是当前图片、双页时一起显示的另一页，或者在预加载范围里（开着预加载时）。
fn wanted(st: &State, path: &Path) -> bool {
    st.current.as_deref() == Some(path)
        || is_spread_page(st, path)
        || (st.opts.prefetch && st.window().iter().any(|p| p == path))
}

/// 解不出来是被上限拦下的（由界面问用户），不是普通的解码失败：还设着上限（`lifted` 为假），而且解码器
/// 记了一笔（`hit`）或错误本身就是超限。已经不设上限还报超限的不算：再问一次结果也一样，只会问个没完。
fn stopped_by_limit(lifted: bool, hit: bool, error: &decode::DecodeError) -> bool {
    !lifted && (hit || error.is_limit())
}

/// 解码一张图片（`ticket`：这次请求的编号，见 [`Flight`]）。
fn decode_job(shared: &Arc<Shared>, path: PathBuf, ticket: u64) {
    let (options, in_archive) = {
        let mut st = lock(&shared.state);
        // 这次请求作废了，或者同一次请求的另一个任务已经在解了。
        if !st
            .in_flight
            .get(&path)
            .is_some_and(|flight| flight.ticket == ticket && !flight.started)
        {
            return;
        }
        // 轮到的时候已经用不着了就不解：按住翻页键一路翻过去时，路过的图片各排了一个任务，
        // 都解的话停下来的那一张要等前面的全解完。
        if !wanted(&st, &path) {
            st.in_flight.remove(&path);
            return;
        }
        if let Some(flight) = st.in_flight.get_mut(&path) {
            flight.started = true;
        }
        (st.opts.decode, st.archive.is_some())
    };
    // 读内容之前文件的修改时间、大小：解不出来时按它记（之后文件变了再试）；压缩包里的图片不看。
    let before = if in_archive { None } else { file_stamp(&path) };
    // 用户对这个文件选过"仍然打开"的不设上限；被上限拦下的不当成普通的解码失败，由界面问用户。
    // 解码、加效果万一出错（panic）也当成解不出来：不然这次请求一直挂着，这张图片不显示、不报错、也不再试。
    let broken = || decode::DecodeError::Corrupt(msg!("解码器出错", "The decoder failed"));
    decode::take_limit_hit();
    let lifted = decode::is_trusted(&path);
    let result = no_panic(|| {
        decode::with_limits_lifted(lifted, || decode::decode_file_with(&path, options))
    })
    .unwrap_or_else(|| Err(broken()));
    let hit = decode::take_limit_hit();
    let blocked = result.as_ref().err().is_some_and(|e| stopped_by_limit(lifted, hit, e));
    // 需要效果时在锁外处理（按解码完成时的效果；期间又改了效果，由那次请求另行处理）。
    let effects = lock(&shared.state).effects;
    let result = result.and_then(|image| {
        let image = Arc::new(image);
        let processed = if effects.is_none() {
            None
        } else {
            Some(Arc::new(no_panic(|| effects::apply(&image, effects)).ok_or_else(broken)?))
        };
        Ok((image, processed))
    });
    #[cfg(test)]
    tests::after_decode(&path);
    // 解不出来，而读的期间文件变了（别的程序正在存、刚存好这个文件）：这次的失败不算数，按现在的文件再读。
    // 不然预读时读到了旧的或写了一半的文件，翻到这一张时还按失败处理（双页时另一页单独显示）。
    let changed_while_reading =
        result.is_err() && !in_archive && before.is_some() && file_stamp(&path) != before;
    let mut st = lock(&shared.state);
    // 解的期间这次请求作废了（文件改了重新请求过、解码方式变了、文件删了）：结果不用，也不放进缓存。
    if !st.in_flight.get(&path).is_some_and(|flight| flight.ticket == ticket) {
        return;
    }
    st.in_flight.remove(&path);
    let is_current = st.current.as_ref() == Some(&path);
    if changed_while_reading && (is_current || wanted(&st, &path)) {
        // 文件一直在变的话最多重读 3 次，之后照常按失败处理（之后再变由后台检查接着管）。
        let tries = st.rereads.entry(path.clone()).or_insert(0);
        *tries += 1;
        if *tries <= 3 {
            let priority = if is_current || is_spread_page(&st, &path) {
                Priority::Current
            } else {
                Priority::Prefetch
            };
            request_display(shared, &mut st, path, priority);
            return;
        }
    }
    match result {
        Ok((image, processed)) => {
            // 预读范围放得下的张数比原来多了（图片比估的小）：当前图片已经好了的话，把多出来的也读进来。
            let (grew, current_ready) = {
                let mut cache = lock(&shared.hub.cache);
                // 按刚解开的这张估还没读的有多大，重新算预读范围，再放进缓存。
                st.last_bytes = image.byte_len();
                let before = st.preload_limit;
                protect_window(&mut st, &mut cache, shared.id);
                let grew = match (before, st.preload_limit) {
                    (Some(a), Some(b)) => b > a,
                    (Some(_), None) => true,
                    _ => false,
                };
                cache.insert(path.clone(), Effects::NONE, Arc::clone(&image));
                if let Some(processed) = &processed {
                    cache.insert(path.clone(), effects, Arc::clone(processed));
                }
                let current_ready = st
                    .current
                    .as_ref()
                    .is_some_and(|c| cache.contains(c, st.effects));
                (grew, current_ready)
            };
            st.failed.remove(&path);
            if is_current {
                if st.effects == effects {
                    push_display(shared, &mut st, path, processed.unwrap_or(image));
                } else {
                    // 解码期间改了效果：按新效果再处理一次。
                    request_display(shared, &mut st, path, Priority::Current);
                }
                if st.scanned {
                    prefetch_neighbors(shared, &mut st);
                }
            } else {
                if is_spread_page(&st, &path) {
                    // 双页：另一页读好了，看这一组能不能显示了（效果对不上时它会再去要处理好的版本）。
                    push_spread(shared, &mut st);
                }
                if grew && current_ready && st.scanned {
                    prefetch_neighbors(shared, &mut st);
                }
            }
        }
        Err(e) => {
            st.failed.insert(path.clone(), before);
            if is_current {
                let ask = blocked && !st.declined.contains(&path);
                let message = e.message().text(shared.hub.lang());
                fail_current(shared, &mut st, path, message, before, ask);
            } else if is_spread_page(&st, &path) {
                // 双页：另一页读不了时当前这一张单独显示。
                push_spread(shared, &mut st);
            }
        }
    }
}

/// 当前图片显示不出来：告诉界面（`ask` 为真时是被上限拦下的，由界面问用户，；否则报解码失败）。
/// `before` 是读之前文件的修改时间、大小：之后文件变了再试。
fn fail_current(
    shared: &Arc<Shared>,
    st: &mut State,
    path: PathBuf,
    message: String,
    before: Option<FileStamp>,
    ask: bool,
) {
    let generation = st.generation;
    st.shown = Some((generation, 0, 0));
    st.watched_files = match before {
        Some(stamp) if st.archive.is_none() => vec![(path.clone(), stamp)],
        _ => Vec::new(),
    };
    shared.push(if ask {
        Event::ImageBlocked { generation, path }
    } else {
        Event::ImageFailed {
            generation,
            path,
            message,
        }
    });
}

/// 向后随机：从当前集合中随机选一张，避开最近看过的；全都看过时只避开当前这张。
fn random_forward(shared: &Arc<Shared>) {
    let mut st = lock(&shared.state);
    if !st.scanned || st.entries.len() < 2 {
        return;
    }
    let pick = |st: &State, avoid_recent: bool| -> Vec<usize> {
        (0..st.entries.len())
            .filter(|&i| Some(i) != st.index)
            .filter(|&i| !avoid_recent || !st.recent.contains(&st.entries[i].path))
            .collect()
    };
    let mut pool = pick(&st, true);
    if pool.is_empty() {
        pool = pick(&st, false);
    }
    let target = pool[(st.next_random() % pool.len() as u64) as usize];
    if let Some(current) = st.current.clone() {
        st.random_back.push(current);
        if st.random_back.len() > RANDOM_BACK_LIMIT {
            st.random_back.remove(0);
        }
    }
    let path = st.entries[target].path.clone();
    st.direction_hint = Some(true);
    set_current(shared, &mut st, path, Some(target), false);
}

/// 向前随机：退回上一次随机跳转前的图片；已不在集合中的跳过。
fn random_back(shared: &Arc<Shared>) {
    let mut st = lock(&shared.state);
    while let Some(path) = st.random_back.pop() {
        if let Some(index) = st.position(&path) {
            st.direction_hint = Some(false);
            set_current(shared, &mut st, path, Some(index), false);
            return;
        }
    }
    shared.push(Event::Notice(NoticeKind::NoRandomBack));
}

/// 下一个、上一个文件夹：在上级文件夹中按当前排序方式移动，跳过不含图片的文件夹，打开目标里的第一张。
/// 在后台扫描；期间打开了别的文件夹则放弃。
fn navigate_folder(shared: &Arc<Shared>, forward: bool) {
    let st = lock(&shared.state);
    navigate_folder_from(shared, &st, forward);
}

fn navigate_folder_from(shared: &Arc<Shared>, st: &State, forward: bool) {
    let Some(folder) = st.folder.clone() else {
        return;
    };
    let order = st.sort;
    let raw = st.opts.raw;
    let s = Arc::clone(shared);
    shared.hub.pool.submit(Priority::Current, move || {
        let target = sibling_with_images(&folder, order, raw, forward);
        let mut st = lock(&s.state);
        if st.folder.as_ref() != Some(&folder) {
            return;
        }
        // 换文件夹、压缩包也是翻页：下一个往后，上一个往前。
        if target.is_some() {
            st.direction_hint = Some(forward);
        }
        match target {
            Some((dir, entries, Some(handle))) => {
                enter_archive(&s, &mut st, dir, handle, entries, false);
            }
            Some((dir, entries, None)) => {
                leave_archive(&s, &mut st);
                st.watched = Some((dir.clone(), modified_time(&dir)));
                st.folder = Some(dir);
                st.entries = entries;
                st.scanned = true;
                st.scan_token += 1;
                let path = st.entries[0].path.clone();
                set_current(&s, &mut st, path, Some(0), false);
            }
            None => s.push(Event::Notice(if forward {
                NoticeKind::LastFolder
            } else {
                NoticeKind::FirstFolder
            })),
        }
    });
}

/// 上级文件夹里下一个（上一个）含图片的文件夹或压缩包：文件夹的图片，或压缩包的图片和打开的压缩包。
type Container = (PathBuf, Vec<Entry>, Option<Arc<crate::archive::Archive>>);

fn sibling_with_images(
    folder: &Path,
    order: SortOrder,
    raw: collection::RawFilter,
    forward: bool,
) -> Option<Container> {
    let parent = folder.parent()?;
    let siblings = collection::sibling_folders(parent, order).ok()?;
    let position = siblings.iter().position(|d| d == folder)?;
    let candidates: Vec<&PathBuf> = if forward {
        siblings[position + 1..].iter().collect()
    } else {
        siblings[..position].iter().rev().collect()
    };
    candidates.into_iter().find_map(|dir| {
        if dir.is_dir() {
            let mut entries = collection::scan_folder(dir, order).ok()?;
            collection::filter_raw(&mut entries, raw, None);
            (!entries.is_empty()).then(|| (dir.clone(), entries, None))
        } else {
            let handle = crate::archive::open(dir).ok()?;
            let mut entries = archive_entries(&handle, order);
            collection::filter_raw(&mut entries, raw, None);
            (!entries.is_empty()).then(|| (dir.clone(), entries, Some(handle)))
        }
    })
}

/// 复制或移动到文件夹：不覆盖同名文件，在文件名后加" (2)"等序号；返回新文件。
/// 压缩包里的图片复制解出的文件，不能移动。
fn copy_or_move(current: &Path, folder: &Path, moving: bool) -> std::io::Result<PathBuf> {
    let source = crate::archive::materialize(current).map_err(std::io::Error::other)?;
    if moving && source.entry.is_some() {
        return Err(std::io::Error::other("cannot move out of an archive"));
    }
    if !folder.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not a folder", folder.display()),
        ));
    }
    let name = source
        .file
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?;
    let target = unique_destination(folder, Path::new(name));
    if moving {
        // 同一个磁盘上直接改名；跨磁盘时先复制再删掉原文件。
        if std::fs::rename(&source.file, &target).is_err() {
            std::fs::copy(&source.file, &target)?;
            std::fs::remove_file(&source.file)?;
        }
    } else {
        std::fs::copy(&source.file, &target)?;
    }
    Ok(target)
}

/// 文件夹里不重名的文件名：`a.jpg` 已有时依次试 `a (2).jpg`、`a (3).jpg`……
fn unique_destination(folder: &Path, name: &Path) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let stem = name
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = name
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// 界面已把文件移到废纸篓：从集合中去掉；是当前图片时显示下一张，已是最后一张则显示上一张，没有了就回到空态。
fn removed(shared: &Arc<Shared>, path: &Path) {
    // 核心里的路径都是规范路径（打开时 canonicalize）。文件已经不在了，只能把所在文件夹换成规范路径再接上文件名，
    // 例如 macOS 的 /var/… 实际是 /private/var/…。
    let normalized = match (
        path.parent().and_then(|p| crate::paths::canonical(p).ok()),
        path.file_name(),
    ) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    };
    let path = normalized.as_path();
    let mut st = lock(&shared.state);
    st.in_flight.remove(path);
    lock(&shared.hub.cache).remove(path);
    st.recent.retain(|p| p != path);
    st.random_back.retain(|p| p != path);
    let position = st.position(path);
    if let Some(i) = position {
        st.entries.remove(i);
    }
    if st.current.as_deref() != Some(path) {
        if position.is_some() {
            let index = st.current.clone().and_then(|c| st.position(&c));
            st.index = index;
            regroup_after_list_change(shared, &mut st);
            shared.push(Event::Collection {
                generation: st.generation,
                index,
                total: st.entries.len(),
            });
        }
        return;
    }
    let next = position.and_then(|i| {
        if i < st.entries.len() {
            Some(i)
        } else {
            i.checked_sub(1)
        }
    });
    match next {
        Some(i) => {
            let path = st.entries[i].path.clone();
            set_current(shared, &mut st, path, Some(i), false);
            // 浏览集合也变了（少了一张）：选择图片窗口等按新的列表重新载入。
            shared.push(Event::Collection {
                generation: st.generation,
                index: Some(i),
                total: st.entries.len(),
            });
        }
        None => {
            drop(st);
            close(shared);
        }
    }
}

/// 在后台扫描刚打开的图片所在的文件夹（图片先显示，列表随后到）。`order` 由调用方传入（调用方正持有状态锁）。
fn submit_scan(shared: &Arc<Shared>, dir: PathBuf, token: u64, order: SortOrder) {
    let s = Arc::clone(shared);
    shared.hub.pool.submit(Priority::Scan, move || {
        // 先记下文件夹的修改时间再扫描：扫描期间的变化下一次检查时还能发现。
        let stamp = modified_time(&dir);
        let result = collection::scan_folder(&dir, order);
        let mut st = lock(&s.state);
        if st.scan_token != token {
            return; // 期间又打开了别的文件夹
        }
        st.watched = result.is_ok().then(|| (dir.clone(), stamp));
        st.scanned = true;
        let index = match result {
            Ok(mut entries) => {
                // 扫描期间改了排序方式时按新的方式重排。
                if st.sort != order {
                    collection::sort_entries(&mut entries, st.sort);
                }
                collection::filter_raw(&mut entries, st.opts.raw, st.current.as_deref());
                st.entries = entries;
                st.current.clone().and_then(|c| st.position(&c))
            }
            // 读不了文件夹：只显示打开的这一张。
            Err(_) => None,
        };
        st.index = index;
        s.push(Event::Collection {
            generation: st.generation,
            index,
            total: st.entries.len(),
        });
        let current_ready = st
            .current
            .as_ref()
            .is_some_and(|c| lock(&s.hub.cache).contains(c, st.effects));
        if current_ready {
            // 双页：列表好了才知道另一页是谁，当前图片早解好了的现在成组、显示（见 resolve_spread）。
            if st.two_page.is_some() {
                push_spread(&s, &mut st);
            }
            if index.is_some() {
                prefetch_neighbors(&s, &mut st);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    struct ChannelListener(Mutex<mpsc::Sender<()>>);

    impl CoreListener for ChannelListener {
        fn wake(&self) {
            let _ = self.0.lock().unwrap().send(());
        }
    }

    struct Harness {
        core: AppCore,
        wakes: mpsc::Receiver<()>,
        seen: Vec<Event>,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_watch_interval(WATCH_INTERVAL)
        }

        /// 后台整轮检查隔 `interval` 才来一次的核心。隔很久的话，测试里等到的重新读取就只能是
        /// "显示时马上核对"做的，不是碰巧赶上了下一轮。
        fn with_watch_interval(interval: Duration) -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                core: AppCore::with_watch_interval(Arc::new(ChannelListener(Mutex::new(tx))), interval),
                wakes: rx,
                seen: Vec::new(),
            }
        }

        /// 丢掉已经收到的事件，之后只看新事件。
        fn clear(&mut self) {
            let _ = self.core.drain_events();
            self.seen.clear();
        }

        /// 收集事件，直到出现满足条件的事件；返回它。
        fn wait_for(&mut self, what: &str, pred: impl Fn(&Event) -> bool) -> Event {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                for event in self.core.drain_events() {
                    self.seen.push(event);
                }
                if let Some(pos) = self.seen.iter().position(&pred) {
                    return self.seen.remove(pos);
                }
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "没等到 {what}，已收到：{:?}", self.seen);
                let _ = self
                    .wakes
                    .recv_timeout(left.min(Duration::from_millis(200)));
            }
        }
    }

    /// 把后台线程占住，直到开闸：期间排进去的任务都等着，测试可以决定它们什么时候开始。
    struct Gate(Arc<(Mutex<(bool, usize)>, std::sync::Condvar)>);

    impl Gate {
        /// 排进 `count` 个等开闸的任务。
        fn block(core: &AppCore, priority: Priority, count: usize) -> Self {
            let gate = Arc::new((Mutex::new((false, 0)), std::sync::Condvar::new()));
            for _ in 0..count {
                let gate = Arc::clone(&gate);
                core.shared.hub.pool.submit(priority, move || {
                    let (state, wake) = &*gate;
                    let mut state = state.lock().unwrap();
                    state.1 += 1;
                    wake.notify_all();
                    while !state.0 {
                        state = wake.wait(state).unwrap();
                    }
                });
            }
            Self(gate)
        }

        /// 把后台线程全部占住（等到每个线程都停在闸上才返回）。
        fn block_all(core: &AppCore) -> Self {
            let threads = core.shared.hub.pool.threads();
            let gate = Self::block(core, Priority::Current, threads);
            let (state, wake) = &*gate.0;
            let mut state = state.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while state.1 < threads {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "后台线程没有都停下");
                state = wake.wait_timeout(state, left).unwrap().0;
            }
            drop(state);
            gate
        }

        fn open(&self) {
            self.0.0.lock().unwrap().0 = true;
            self.0.1.notify_all();
        }
    }

    impl Drop for Gate {
        // 测试中途失败也开闸：不然后台线程一直停着，核心收不了尾。
        fn drop(&mut self) {
            self.open();
        }
    }

    /// 等到条件成立（最多 10 秒）。
    fn eventually(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "没等到：{what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn in_cache(h: &Harness, path: &Path) -> bool {
        lock(&h.core.shared.hub.cache).contains(path, Effects::NONE)
    }

    /// 一个文件夹的竖图（6×8，红色分量依次是 `reds`），文件名是 a.png、b.png……
    fn folder_with_pages(tag: &str, reds: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anyview-api-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (i, r) in reds.iter().enumerate() {
            let name = format!("{}.png", (b'a' + i as u8) as char);
            image::RgbImage::from_pixel(6, 8, image::Rgb([*r, 0, 0]))
                .save(dir.join(name))
                .unwrap();
        }
        crate::paths::canonical(&dir).unwrap()
    }

    /// 显示出来的图片的宽度和左上、右下两点的红色分量（BGRA：第 3 个字节）。
    fn corners(image: &DecodedImage) -> (u32, u8, u8) {
        let at = |x: usize, y: usize| image.pixels[y * image.stride + x * 4 + 2];
        (image.width, at(0, 0), at(image.width as usize - 1, image.height as usize - 1))
    }

    /// 等到一张这个样子的图片作为新图片显示出来（之前别的样子的不算）。
    fn wait_shown(h: &mut Harness, what: &str, wanted: (u32, u8, u8)) {
        h.wait_for(what, |e| matches!(e, Event::ImageReady { image, .. } if corners(image) == wanted));
    }

    fn folder_with_images(tag: &str, names: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anyview-api-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        for (i, name) in names.iter().enumerate() {
            let shade = (i * 40) as u8;
            image::RgbImage::from_pixel(8, 6, image::Rgb([shade, 0, 0]))
                .save(dir.join(name))
                .unwrap();
        }
        dir
    }

    fn is_ready(event: &Event, name: &str) -> bool {
        matches!(event, Event::ImageReady { path, .. } if path.file_name().unwrap() == name)
    }

    #[test]
    fn picker_lists_the_collection_and_makes_thumbnails_on_request() {
        let dir = folder_with_images("picker", &["b10.png", "b2.png", "a.png"]);
        let mut h = Harness::new();
        assert!(h.core.picker_list(Lang::ZhHans).is_none());
        assert!(!h.core.action_state("select_image").enabled);
        h.core.dispatch(Command::Open {
            path: dir.join("b2.png").display().to_string(),
        });
        h.wait_for("扫描", |e| matches!(e, Event::Collection { .. }));
        assert!(h.core.action_state("select_image").enabled);
        assert_eq!(h.core.invoke_action("select_image"), ActionOutcome::Ui(UiRequest::ShowPicker));
        let list = h.core.picker_list(Lang::ZhHans).unwrap();
        let folder = dir.file_name().unwrap().to_string_lossy().into_owned();
        let names: Vec<_> = list.rows.iter().map(|r| r.name.clone()).collect();
        assert_eq!(
            names,
            [format!("{folder}/a.png"), format!("{folder}/b2.png"), format!("{folder}/b10.png")]
        );
        assert_eq!(list.current, Some(1));
        assert_eq!(list.sort_label, "按名称排序");
        let options: Vec<_> = list
            .sort_options
            .iter()
            .map(|o| (o.action.as_str(), o.checked))
            .collect();
        assert_eq!(
            options,
            [
                ("sort_name", true),
                ("sort_mtime", false),
                ("sort_size", false),
                ("sort_type", false),
                ("sort_desc", false)
            ]
        );
        assert_eq!(list.sort_options[0].title, "文件名");
        assert!(list.rows[0].size.ends_with(" B"));
        assert!(!list.rows[0].modified.is_empty());
        // 缩略图：要了才生成，好了发事件；之后直接取得到。
        let a = list.rows[0].path.clone();
        assert!(h.core.picker_thumbnail(&a).is_none());
        h.core.picker_want_thumbnails(vec![a.clone(), list.rows[2].path.clone()]);
        let Event::Thumbnail { image, .. } = h.wait_for("缩略图", |e| {
            matches!(e, Event::Thumbnail { path, .. } if path.file_name().unwrap() == "a.png")
        }) else {
            unreachable!()
        };
        let thumb = image.expect("应生成缩略图");
        assert_eq!((thumb.width, thumb.height), (8, 6));
        assert!(h.core.picker_thumbnail(&a).is_some());
        // 不在浏览集合里的路径不理。
        assert!(h.core.picker_thumbnail("/没有/这个.png").is_none());
        // 降序后列表跟着变；选中第 0 行切到 b10。
        h.core.invoke_action("sort_desc");
        let list = h.core.picker_list(Lang::En).unwrap();
        assert_eq!(list.sort_label, "Sort by Name (Descending)");
        assert!(list.rows[0].name.ends_with("b10.png"));
        h.core.picker_choose(0);
        h.wait_for("b10", |e| is_ready(e, "b10.png"));
        // 隐藏缩略图导航：记住。
        assert!(!h.core.picker_hides_thumbnails());
        h.core.set_picker_hides_thumbnails(true);
        assert!(h.core.picker_hides_thumbnails());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_turns_carry_a_direction_and_follow_the_settings() {
        let dir = folder_with_images("turn", &["1.png", "2.png", "3.png"]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("2.png").display().to_string(),
        });
        let Event::Current { generation, .. } = h.wait_for("打开", |e| matches!(e, Event::Current { .. })) else {
            unreachable!()
        };
        h.wait_for("扫描", |e| matches!(e, Event::Collection { .. }));
        // 默认：效果"无"、不放音效。
        assert_eq!(h.core.transition_for(generation), None);
        {
            let mut prefs = lock(&h.core.shared.hub.prefs);
            prefs
                .settings
                .set(settings::PAGE_EFFECT, Value::Choice("slide".into()));
            prefs.settings.set(settings::PAGE_SOUND, Value::Bool(true));
        }
        // "打开"不算翻页。
        assert_eq!(h.core.transition_for(generation), None);
        let current = |h: &mut Harness, name: &'static str| {
            let Event::Current { generation, .. } = h.wait_for(name, move |e| {
                matches!(e, Event::Current { path, .. } if path.file_name().unwrap() == name)
            }) else {
                unreachable!()
            };
            generation
        };
        h.core.dispatch(Command::Navigate(Navigation::Next));
        let g = current(&mut h, "3.png");
        let t = h.core.transition_for(g).expect("翻页应有效果");
        assert_eq!(
            (t.effect, t.forward, t.sound, t.duration_ms),
            (crate::transition::PageEffect::Slide, true, true, 300)
        );
        assert_eq!(h.core.transition_for(g - 1), None, "旧代次没有");
        h.core.dispatch(Command::Navigate(Navigation::First));
        let g = current(&mut h, "1.png");
        assert!(!h.core.transition_for(g).unwrap().forward);
        h.core.picker_choose(2);
        let g = current(&mut h, "3.png");
        assert!(h.core.transition_for(g).unwrap().forward, "跳到后面是往后翻");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn left_drag_moves_the_window_only_while_the_image_fits() {
        let mut h = Harness::new();
        // 空窗口：能移动就移动窗口（拖着空窗口走）。
        assert!(h.core.left_drag_moves_window(true));
        let dir = folder_with_images("drag", &["a.png"]);
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.core.view_set_viewport(100.0, 100.0);
        h.core.view_show_image(8, 6);
        // 图片完整显示：移动窗口；窗口不能移动（最大化、全屏）时平移。
        assert!(h.core.left_drag_moves_window(true));
        assert!(!h.core.left_drag_moves_window(false));
        // 放大到超出画布（8 像素宽的图放大到 1600% 以上，超过 100 像素的画布）：平移图片。
        for _ in 0..10 {
            h.core.invoke_action("zoom_in");
        }
        assert!(!h.core.left_drag_moves_window(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn large_jpeg_shows_preview_first_then_the_sharp_image_as_update() {
        use crate::jpeg::insert_exif_thumbnail;
        use crate::jpeg::tests::encode_jpeg;
        let dir = folder_with_images("preview", &["small.png"]);
        let main = encode_jpeg(3000, 2000, 70);
        let thumb = encode_jpeg(150, 100, 70);
        // EXIF 方向 6：原图 3000x2000 摆正后是 2000x3000。
        std::fs::write(
            dir.join("big.jpg"),
            insert_exif_thumbnail(&main, &thumb, 6, false),
        )
        .unwrap();
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("big.jpg").display().to_string(),
        });
        let Event::ImagePreview {
            generation,
            image,
            width,
            height,
            ..
        } = h.wait_for("preview", |e| matches!(e, Event::ImagePreview { .. }))
        else {
            unreachable!()
        };
        assert_eq!((width, height), (2000, 3000));
        assert_eq!((image.width, image.height), (100, 150));
        let Event::ImageUpdated {
            generation: sharp_generation,
            image,
            ..
        } = h.wait_for("sharp", |e| matches!(e, Event::ImageUpdated { .. }))
        else {
            unreachable!()
        };
        assert_eq!(sharp_generation, generation);
        assert_eq!((image.width, image.height), (2000, 3000));
        assert!(
            !h.seen.iter().any(|e| matches!(e, Event::ImageReady { .. })),
            "预览之后清晰图应以 ImageUpdated 送达"
        );
        // 小图（PNG）没有预览，照常以 ImageReady 送达。
        h.clear();
        h.core.dispatch(Command::Open {
            path: dir.join("small.png").display().to_string(),
        });
        h.wait_for("small", |e| is_ready(e, "small.png"));
        assert!(
            !h.seen
                .iter()
                .any(|e| matches!(e, Event::ImagePreview { .. }))
        );
        // 再回到大图：已在缓存里，直接 ImageReady，没有预览。
        h.clear();
        h.core.dispatch(Command::Open {
            path: dir.join("big.jpg").display().to_string(),
        });
        h.wait_for("cached", |e| is_ready(e, "big.jpg"));
        assert!(
            !h.seen
                .iter()
                .any(|e| matches!(e, Event::ImagePreview { .. }))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn actions_follow_enable_rules_and_drive_the_view() {
        use crate::actions::ALT;
        let mut h = Harness::new();
        // 没有图片：需要图片的动作和尚未实现的动作不可用，全局选项可用。
        assert_eq!(h.core.invoke_action("zoom_in"), ActionOutcome::Unavailable);
        assert!(!h.core.action_state("next_image").enabled);
        assert!(!h.core.action_state("print").enabled);
        assert!(h.core.action_state("stretch_small").enabled);
        assert!(h.core.action_state("fit_window").checked);
        assert!(h.core.action_state("edge_soften").checked);
        assert_eq!(
            h.core.invoke_action("no_such_action"),
            ActionOutcome::Unavailable
        );
        assert_eq!(
            h.core.invoke_action("open_file"),
            ActionOutcome::Ui(UiRequest::OpenFileDialog)
        );

        let dir = folder_with_images("actions", &["a.png", "b.png"]);
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("collection", |e| matches!(e, Event::Collection { .. }));
        h.core.view_set_viewport(100.0, 100.0);
        assert_eq!(h.core.view_show_image(8, 6).unwrap().scale, 1.0);

        let changed = ActionOutcome::ViewChanged { show_zoom: false };
        assert_eq!(h.core.invoke_action("rotate_cw"), changed);
        assert!(h.core.action_state("rotate_cw").checked);
        // ⌥→ 在三个平台上都是顺时针旋转。
        assert_eq!(h.core.invoke_key("Right", ALT), Some(changed));
        assert!(h.core.action_state("rotate_180").checked);
        assert_eq!(
            h.core.invoke_action("zoom_in"),
            ActionOutcome::ViewChanged { show_zoom: true }
        );
        assert_eq!(h.core.view_layout().unwrap().scale, 1.5);
        assert_eq!(
            h.core.invoke_key("Z", 0),
            Some(ActionOutcome::ViewChanged { show_zoom: true })
        );
        assert!(h.core.action_state("stretch_small").checked);
        assert_eq!(h.core.invoke_key("F24", 0), None);

        assert_eq!(h.core.invoke_key("Right", 0), Some(ActionOutcome::Started));
        h.wait_for("b.png", |e| is_ready(e, "b.png"));
        // 换图后旋转复位。
        h.core.view_show_image(8, 6);
        assert!(h.core.action_state("rotate_none").checked);
    }

    #[test]
    fn jump_info_and_overlay_toggles() {
        let mut h = Harness::new();
        assert!(h.core.info_lines(Lang::ZhHans).is_empty());
        assert_eq!(
            h.core.invoke_action("toggle_bar_lock"),
            ActionOutcome::OverlayChanged
        );
        assert!(h.core.action_state("toggle_bar_lock").checked);
        let dir = folder_with_images("jump", &["a.png", "b.png", "c.png", "d.png"]);
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("collection", |e| matches!(e, Event::Collection { .. }));
        let lines = h.core.info_lines(Lang::ZhHans);
        assert_eq!(lines[0], "文件名: a.png [1/4]");
        assert!(
            lines[1].starts_with("图片大小: ") && lines[1].ends_with('B'),
            "{lines:?}"
        );
        assert!(lines[2].starts_with("修改日期: 20"), "{lines:?}");
        assert_eq!(lines[3], "图片信息: 8x6 (PNG,RGB24)");
        assert_eq!(
            h.core.invoke_action("toggle_info"),
            ActionOutcome::OverlayChanged
        );
        assert!(h.core.action_state("toggle_info").checked);
        h.core.dispatch(Command::Navigate(Navigation::Jump(2)));
        h.wait_for("c.png", |e| is_ready(e, "c.png"));
        h.core.dispatch(Command::Navigate(Navigation::Jump(99)));
        h.wait_for("d.png", |e| is_ready(e, "d.png"));
        assert_eq!(h.core.info_lines(Lang::En)[0], "File: d.png [4/4]");
    }

    /// 执行一个动作，返回随后切换到的图片名称。
    fn go(h: &mut Harness, action: impl FnOnce(&AppCore)) -> String {
        h.clear();
        action(&h.core);
        match h.wait_for("current", |e| matches!(e, Event::Current { .. })) {
            Event::Current { name, .. } => name,
            _ => unreachable!(),
        }
    }

    fn act(id: &'static str) -> impl FnOnce(&AppCore) {
        move |core| {
            core.invoke_action(id);
        }
    }

    fn nav(navigation: Navigation) -> impl FnOnce(&AppCore) {
        move |core| core.dispatch(Command::Navigate(navigation))
    }

    #[test]
    fn sorting_reorders_and_keeps_the_current_image() {
        let mut h = Harness::new();
        let dir = folder_with_images("sort", &["a.png", "b.png", "c.png"]);
        // 让 a 最大、c 最新，便于区分大小和修改日期排序。
        image::RgbImage::from_pixel(64, 64, image::Rgb([9, 9, 9]))
            .save(dir.join("a.png"))
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        image::RgbImage::from_pixel(8, 6, image::Rgb([1, 2, 3]))
            .save(dir.join("c.png"))
            .unwrap();
        h.core.dispatch(Command::Open {
            path: dir.join("b.png").display().to_string(),
        });
        h.wait_for("b", |e| is_ready(e, "b.png"));
        h.wait_for("scan", |e| {
            matches!(
                e,
                Event::Collection {
                    index: Some(1),
                    total: 3,
                    ..
                }
            )
        });
        assert_eq!(
            h.core.invoke_action("sort_desc"),
            ActionOutcome::OverlayChanged
        );
        assert!(h.core.action_state("sort_desc").checked);
        h.wait_for("desc", |e| {
            matches!(e, Event::Collection { index: Some(1), .. })
        });
        assert_eq!(go(&mut h, nav(Navigation::First)), "c.png");
        h.core.invoke_action("sort_desc");
        h.core.invoke_action("sort_size");
        assert!(
            h.core.action_state("sort_size").checked && !h.core.action_state("sort_name").checked
        );
        // 大小升序：a（64x64）最大，排在最后。
        assert_eq!(go(&mut h, nav(Navigation::Last)), "a.png");
        h.core.invoke_action("sort_mtime");
        assert_eq!(go(&mut h, nav(Navigation::Last)), "c.png");
    }

    #[test]
    fn random_avoids_recent_and_goes_back() {
        let mut h = Harness::new();
        let names = ["1.png", "2.png", "3.png", "4.png", "5.png", "6.png"];
        let dir = folder_with_images("random", &names);
        h.core.dispatch(Command::Open {
            path: dir.join("1.png").display().to_string(),
        });
        h.wait_for("scan", |e| matches!(e, Event::Collection { total: 6, .. }));
        let mut seen = vec!["1.png".to_string()];
        for _ in 0..5 {
            let name = go(&mut h, act("random_forward"));
            assert!(!seen.contains(&name), "随机跳回了看过的 {name}：{seen:?}");
            seen.push(name);
        }
        // 全都看过以后仍能跳（只避开当前这张）。
        assert_ne!(go(&mut h, act("random_forward")), seen[5]);
        assert_eq!(go(&mut h, act("random_back")), seen[5]);
        assert_eq!(go(&mut h, act("random_back")), seen[4]);
    }

    #[test]
    fn folders_skip_empty_ones_and_stop_at_the_ends() {
        let mut h = Harness::new();
        let root = std::env::temp_dir().join(format!("anyview-api-{}-folders", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (folder, files) in [
            ("a", &["1.png"][..]),
            ("b", &[][..]),
            ("c", &["x.png", "y.png"][..]),
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            for file in files {
                image::RgbImage::from_pixel(4, 4, image::Rgb([0, 0, 0]))
                    .save(root.join(folder).join(file))
                    .unwrap();
            }
        }
        std::fs::write(root.join("b/readme.txt"), b"no images").unwrap();
        h.core.dispatch(Command::Open {
            path: root.join("a/1.png").display().to_string(),
        });
        h.wait_for("a/1", |e| is_ready(e, "1.png"));
        h.core.invoke_action("next_container");
        h.wait_for("c/x", |e| is_ready(e, "x.png"));
        h.core.invoke_action("next_container");
        h.wait_for("last folder", |e| {
            matches!(e, Event::Notice(NoticeKind::LastFolder))
        });
        h.core.invoke_action("prev_container");
        h.wait_for("back to a", |e| is_ready(e, "1.png"));
        h.core.invoke_action("prev_container");
        h.wait_for("first folder", |e| {
            matches!(e, Event::Notice(NoticeKind::FirstFolder))
        });
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn removed_files_show_the_next_or_previous() {
        let mut h = Harness::new();
        let dir = folder_with_images("removed", &["a.png", "b.png", "c.png"]);
        h.core.dispatch(Command::Open {
            path: dir.join("b.png").display().to_string(),
        });
        h.wait_for("scan", |e| matches!(e, Event::Collection { total: 3, .. }));
        match h.core.invoke_action("delete_file") {
            ActionOutcome::Ui(UiRequest::TrashFile {
                path,
                name,
                confirm,
            }) => {
                assert_eq!(name, "b.png");
                assert!(path.ends_with("b.png"));
                assert!(confirm, "默认先确认");
            }
            other => panic!("{other:?}"),
        }
        // 通过符号链接的文件夹路径告诉核心（不是规范路径，与 macOS 的 /var → /private/var 相同的情形）。
        let link = dir.with_extension("link");
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        #[cfg(not(unix))]
        let link = dir.clone();
        let remove = |name: &str| {
            let path = link.join(name).display().to_string();
            move |core: &AppCore| core.dispatch(Command::Removed { path })
        };
        assert_eq!(go(&mut h, remove("b.png")), "c.png");
        h.wait_for("少了一张", |e| matches!(e, Event::Collection { total: 2, index: Some(1), .. }));
        // 删除最后一张后显示上一张。
        assert_eq!(go(&mut h, remove("c.png")), "a.png");
        h.clear();
        h.core.dispatch(Command::Removed {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("closed", |e| matches!(e, Event::Closed { .. }));
    }

    #[test]
    fn effects_update_the_current_image_and_follow_navigation() {
        let mut h = Harness::new();
        let dir = folder_with_images("effects", &["a.png", "b.png"]);
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        // 先等第一张显示出来，再切换效果（解码完成前切换时，第一次送达的 ImageReady 就已带着效果）。
        h.wait_for("a", |e| is_ready(e, "a.png"));
        h.wait_for("scan", |e| matches!(e, Event::Collection { total: 2, .. }));
        h.clear();
        // 有图片时切换效果：新的显示缓冲区随后以 ImageUpdated 送达。
        assert_eq!(h.core.invoke_action("invert"), ActionOutcome::Started);
        assert!(h.core.action_state("invert").checked);
        // folder_with_images 的第一张是纯黑（BGRA 0,0,0），反相后是纯白。
        let updated = h.wait_for("updated", |e| matches!(e, Event::ImageUpdated { .. }));
        let Event::ImageUpdated { image, .. } = updated else {
            unreachable!()
        };
        assert_eq!(&image.pixels[..4], &[255, 255, 255, 255]);
        h.clear();
        h.core.dispatch(Command::Navigate(Navigation::Next));
        let ready = h.wait_for("b", |e| is_ready(e, "b.png"));
        let Event::ImageReady { image, .. } = ready else {
            unreachable!()
        };
        // b 的红色分量为 40，反相后为 215。
        assert_eq!(image.pixels[2], 215);
        h.clear();
        h.core.invoke_action("invert");
        let updated = h.wait_for("restored", |e| matches!(e, Event::ImageUpdated { .. }));
        let Event::ImageUpdated { image, .. } = updated else {
            unreachable!()
        };
        assert_eq!(image.pixels[2], 40);
    }

    /// 使用监视器的 ICC 设置：设置关着时界面报来的配置文件只记下；打开后按它转换（缓存里的原图照用），
    /// 翻页后的图片也转；关掉后换回原来的颜色。macOS 不显示这一项（系统负责），跳过。
    #[test]
    fn monitor_profile_converts_colors_only_when_the_setting_is_on() {
        use settings::Input::Bool;
        if Platform::current() == Platform::Macos {
            return;
        }
        let mut h = Harness::new();
        let dir = folder_with_images("monitor-icc", &["a.png", "b.png"]);
        h.core.dispatch(Command::Open {
            path: dir.join("b.png").display().to_string(),
        });
        h.wait_for("b", |e| is_ready(e, "b.png"));
        h.wait_for("scan", |e| matches!(e, Event::Collection { total: 2, .. }));
        h.clear();
        // sRGB 的 40 转到线性 Gamma 的"显示器"约为 5。
        assert!(!h.core.set_display_profile(Some(crate::color::linear_srgb_test_profile())));
        assert!(!h.core.display_profile_active());
        apply(&h.core, &[(settings::MONITOR_ICC, Bool(true))]);
        assert!(h.core.display_profile_active());
        let updated = h.wait_for("converted", |e| matches!(e, Event::ImageUpdated { .. }));
        let Event::ImageUpdated { image, .. } = updated else {
            unreachable!()
        };
        assert!((4..=7).contains(&image.pixels[2]), "{:?}", &image.pixels[..4]);
        // 原图（复制、打印用）不转。
        assert_eq!(h.core.original_image().unwrap().pixels[2], 40);
        h.clear();
        h.core.dispatch(Command::Navigate(Navigation::Prev));
        let ready = h.wait_for("a", |e| is_ready(e, "a.png"));
        let Event::ImageReady { image, .. } = ready else {
            unreachable!()
        };
        assert_eq!(&image.pixels[..3], &[0, 0, 0]);
        h.clear();
        // 没有配置文件（换到没设配置文件的显示器）：不转。
        assert!(h.core.set_display_profile(None));
        h.wait_for("plain", |e| matches!(e, Event::ImageUpdated { .. }));
        assert!(!h.core.display_profile_active());
        // 与 sRGB 一样的配置文件也不用转。
        assert!(!h.core.set_display_profile(Some(crate::color::srgb_test_profile())));
        h.clear();
        h.core.dispatch(Command::Navigate(Navigation::Next));
        h.wait_for("b again", |e| is_ready(e, "b.png"));
        h.core.set_display_profile(Some(crate::color::linear_srgb_test_profile()));
        h.wait_for("converted again", |e| matches!(e, Event::ImageUpdated { .. }));
        h.clear();
        apply(&h.core, &[(settings::MONITOR_ICC, Bool(false))]);
        let updated = h.wait_for("restored", |e| matches!(e, Event::ImageUpdated { .. }));
        let Event::ImageUpdated { image, .. } = updated else {
            unreachable!()
        };
        assert_eq!(image.pixels[2], 40);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exact_rendering_arrives_with_the_current_signature() {
        let mut h = Harness::new();
        let dir = folder_with_images("exact", &["a.png"]);
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a", |e| is_ready(e, "a.png"));
        h.core.view_set_viewport(100.0, 100.0);
        h.core.view_show_image(8, 6);
        // 默认 Bilinear：不需要精确重绘。
        h.core.view_zoom_by(3.0, 50.0, 50.0);
        h.core.request_exact();
        h.core.invoke_action("interp_lanczos");
        assert!(h.core.action_state("interp_lanczos").checked);
        h.clear();
        h.core.request_exact();
        let ready = h.wait_for("exact", |e| matches!(e, Event::ExactReady { .. }));
        let Event::ExactReady {
            signature,
            image,
            x,
            y,
            ..
        } = ready
        else {
            unreachable!()
        };
        assert_eq!(signature, h.core.view_signature());
        // 8x6 放大 3 倍 = 24x18，居中在 100x100 的视口里。
        assert_eq!((image.width, image.height, x, y), (24, 18, 38, 41));
        h.core.view_pan_by(1.0, 0.0);
        h.core.invoke_action("interp_bilinear");
        assert_ne!(signature, h.core.view_signature());
    }

    #[test]
    fn ping_produces_pong_and_wakes_listener() {
        let mut h = Harness::new();
        h.core.dispatch(Command::Ping { token: 42 });
        h.wait_for("pong", |e| matches!(e, Event::Pong { token: 42 }));
    }

    #[test]
    fn coalesced_wakes_never_lose_events() {
        let mut h = Harness::new();
        const N: u64 = 200;
        for token in 0..N {
            h.core.dispatch(Command::Ping { token });
        }
        for token in 0..N {
            h.wait_for(
                "all pongs",
                |e| matches!(e, Event::Pong { token: t } if *t == token),
            );
        }
    }

    /// "打开最近使用"：用户打开的文件、文件夹记下来，最近的在最前，同一个不重复；翻页不算打开；
    /// 设置"不保存浏览过的文件夹路径"打开以后不记，已经记下的清掉。只在 Windows 上记（这里手动打开）。
    /// 记是在交出"打开"的那一刻做的，不用等图片显示出来。
    #[test]
    fn opened_paths_are_remembered_newest_first() {
        let folder = crate::paths::canonical(&folder_with_images("recent", &["a.png", "b.png"])).unwrap();
        let a = folder.join("a.png");
        let settings_dir = std::env::temp_dir().join(format!("anyview-api-{}-recent-settings", std::process::id()));
        let _ = std::fs::remove_dir_all(&settings_dir);
        std::fs::create_dir_all(&settings_dir).unwrap();
        let h = Harness::new();
        h.core.load_settings(Some(settings_dir.join("settings.toml")));
        let open = |path: &Path| {
            h.core.dispatch(Command::Open {
                path: path.display().to_string(),
            });
        };
        open(&a);
        assert!(h.core.recent_files().is_empty(), "别的平台不记");
        h.core
            .shared
            .hub
            .track_recent
            .store(true, AtomicOrdering::Relaxed);
        open(&a);
        h.core.dispatch(Command::Navigate(Navigation::Next));
        assert_eq!(h.core.recent_files(), [a.display().to_string()], "翻页不算打开");
        open(&folder);
        open(&a);
        assert_eq!(
            h.core.recent_files(),
            [a.display().to_string(), folder.display().to_string()]
        );
        // 写进了设置文件旁边的 recent.toml（后台写，等一下）。
        let file = settings_dir.join(crate::recent::FILE_NAME);
        eventually("recent.toml 写出来", || {
            std::fs::read_to_string(&file).is_ok_and(|t| t.contains("a.png"))
        });
        // "清除菜单"。
        h.core.clear_recent();
        assert!(h.core.recent_files().is_empty());
        eventually("recent.toml 删掉", || !file.exists());
        // 关掉"记住最近打开的文件和文件夹"（设置文件里对应反向布尔键"不保存浏览过的文件夹路径"）：
        // 之后打开的不记，已经记下的清掉。
        open(&a);
        assert_eq!(h.core.recent_files().len(), 1);
        h.core.settings_begin();
        assert!(h.core.settings_set(settings::NO_RECENT, settings::Input::Bool(false)));
        assert!(h.core.settings_apply().is_ok());
        assert!(h.core.recent_files().is_empty());
        eventually("recent.toml 再删掉", || !file.exists());
        open(&folder);
        assert!(h.core.recent_files().is_empty());
        h.core.shutdown();
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_dir_all(&settings_dir);
    }

    #[test]
    fn open_folder_shows_first_image_in_natural_order() {
        let dir = folder_with_images("folder", &["10.png", "2.png", "1.png"]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.display().to_string(),
        });
        let current = h.wait_for("current", |e| matches!(e, Event::Current { .. }));
        let Event::Current {
            name, index, total, ..
        } = current
        else {
            unreachable!()
        };
        assert_eq!((name.as_str(), index, total), ("1.png", Some(0), Some(3)));
        let ready = h.wait_for("image 1", |e| is_ready(e, "1.png"));
        let Event::ImageReady { image, .. } = ready else {
            unreachable!()
        };
        assert_eq!((image.width, image.height), (8, 6));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_file_then_navigate_with_bounds_and_prefetch() {
        let dir = folder_with_images("file", &["a.png", "b.png", "c.png"]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("b.png").display().to_string(),
        });
        h.wait_for("image b", |e| is_ready(e, "b.png"));
        h.wait_for("collection", |e| {
            matches!(
                e,
                Event::Collection {
                    index: Some(1),
                    total: 3,
                    ..
                }
            )
        });

        // 预加载完成后，切到下一张应立即从缓存送达（在 dispatch 里同步推入事件）。
        std::thread::sleep(Duration::from_millis(300));
        h.core.dispatch(Command::Navigate(Navigation::Next));
        let events = h.core.drain_events();
        assert!(
            events.iter().any(|e| is_ready(e, "c.png")),
            "下一张没有预加载：{events:?}"
        );

        // 到达首尾：默认直接回到首张、往回到末张，不问。
        h.clear();
        h.core.dispatch(Command::Navigate(Navigation::Next));
        h.wait_for("回到首张", |e| matches!(e, Event::Current { index: Some(0), .. }));
        h.core.dispatch(Command::Navigate(Navigation::Prev));
        h.wait_for("往回到末张", |e| matches!(e, Event::Current { index: Some(2), .. }));
        let events = h.core.drain_events();
        assert!(!h.seen.iter().chain(&events).any(|e| matches!(e, Event::Boundary { .. })));
        // 设置成"询问"时照旧问。
        apply(&h.core, &[(settings::AT_BOUNDARY, settings::Input::Choice("ask".into()))]);
        h.core.dispatch(Command::Navigate(Navigation::Next));
        h.wait_for("at last", |e| {
            matches!(e, Event::Boundary { forward: true, .. })
        });
        h.core.dispatch(Command::Navigate(Navigation::First));
        h.wait_for("image a", |e| is_ready(e, "a.png"));
        h.core.dispatch(Command::Navigate(Navigation::Back10));
        h.wait_for("at first", |e| {
            matches!(e, Event::Boundary { forward: false, .. })
        });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_results_never_replace_newer_ones() {
        let dir = folder_with_images("stale", &["1.png", "2.png", "3.png", "4.png", "5.png"]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.display().to_string(),
        });
        h.wait_for("first image", |e| is_ready(e, "1.png"));
        // 快速连翻，最后停在 5.png；之后收到的图像事件，代次必须是最后一次 Current 的代次。
        for _ in 0..4 {
            h.core.dispatch(Command::Navigate(Navigation::Next));
        }
        let ready = h.wait_for("image 5", |e| is_ready(e, "5.png"));
        let Event::ImageReady {
            generation: ready_gen,
            ..
        } = ready
        else {
            unreachable!()
        };
        let last_current = h
            .seen
            .iter()
            .rev()
            .find_map(|e| match e {
                Event::Current { generation, .. } => Some(*generation),
                _ => None,
            })
            .unwrap();
        assert_eq!(ready_gen, last_current);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_path_and_empty_folder_are_reported() {
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: "/definitely/not/here.png".into(),
        });
        h.wait_for("open failed", |e| matches!(e, Event::OpenFailed { .. }));
        let dir = folder_with_images("empty", &[]);
        h.core.dispatch(Command::Open {
            path: dir.display().to_string(),
        });
        h.wait_for("empty folder", |e| {
            matches!(e, Event::Notice(NoticeKind::EmptyFolder))
        });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ---- 设置 ----

    /// 用草稿改设置并确定（与设置窗口的流程相同）。
    fn apply(core: &AppCore, changes: &[(&str, settings::Input)]) -> SettingsApplied {
        core.settings_begin();
        for (key, input) in changes {
            assert!(core.settings_set(key, input.clone()), "{key} 没被接受");
        }
        core.settings_apply().expect("校验应通过")
    }

    #[test]
    fn windows_share_settings_and_cache_but_not_images() {
        use settings::Input::{Bool, Choice};
        // macOS 的临时目录 /var 是 /private/var 的链接，核心记的是真实路径。
        let dir = crate::paths::canonical(&folder_with_images("windows", &["a.png", "b.png"])).unwrap();
        let mut first = Harness::new();
        apply(
            &first.core,
            &[(settings::INTERPOLATION, Choice("lanczos".into()))],
        );
        // 第二个窗口：按现在的设置开始，浏览集合和当前图片是自己的。
        let (tx, rx) = mpsc::channel();
        let mut second = Harness {
            core: first
                .core
                .new_window(Arc::new(ChannelListener(Mutex::new(tx)))),
            wakes: rx,
            seen: Vec::new(),
        };
        assert_eq!(first.core.window_count(), 2);
        assert!(second.core.action_state("interp_lanczos").checked);
        first.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        second.core.dispatch(Command::Open {
            path: dir.join("b.png").display().to_string(),
        });
        first.wait_for("a.png", |e| is_ready(e, "a.png"));
        second.wait_for("b.png", |e| is_ready(e, "b.png"));
        assert_eq!(first.core.current_path(), Some(dir.join("a.png")));
        assert_eq!(second.core.current_path(), Some(dir.join("b.png")));
        // 两个窗口的事件各走各的：第一个窗口没收到 b.png。
        assert!(!first.seen.iter().any(|e| is_ready(e, "b.png")));
        // 缓存共用。
        assert!(first.core.cache_bytes() > 0);
        assert_eq!(first.core.cache_bytes(), second.core.cache_bytes());
        // 在第二个窗口改设置，第一个窗口也生效。
        apply(
            &second.core,
            &[
                (settings::INTERPOLATION, Choice("bicubic".into())),
                (settings::SINGLE_INSTANCE, Bool(true)),
            ],
        );
        assert!(first.core.action_state("interp_bicubic").checked);
        assert!(first.core.ui_settings().reuse_window);
        // 关掉第二个窗口：后台任务放下它以后，只剩一个窗口。
        drop(second);
        let deadline = Instant::now() + Duration::from_secs(5);
        while first.core.window_count() != 1 {
            assert!(Instant::now() < deadline, "关掉的窗口还在");
            thread::sleep(Duration::from_millis(10));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 4 帧的动图（每帧一种颜色，时长 30 ms）。
    fn animated_gif(path: &Path) {
        use image::codecs::gif::{GifEncoder, Repeat};
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = GifEncoder::new(file);
        encoder.set_repeat(Repeat::Infinite).unwrap();
        let colors = [[220u8, 40, 40], [40, 170, 70], [40, 90, 220], [240, 200, 40]];
        let frames = colors.iter().map(|c| {
            let buffer = image::RgbaImage::from_pixel(8, 6, image::Rgba([c[0], c[1], c[2], 255]));
            image::Frame::from_parts(buffer, 0, 0, image::Delay::from_numer_denom_ms(30, 1))
        });
        encoder.encode_frames(frames).unwrap();
    }

    #[test]
    fn animations_play_pause_and_step() {
        let dir = crate::paths::canonical(&folder_with_images("anim", &["b.png"])).unwrap();
        let gif = dir.join("a.gif");
        animated_gif(&gif);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: gif.display().to_string(),
        });
        h.wait_for("a.gif", |e| is_ready(e, "a.gif"));
        // 播放：帧依次送来，播完一遍后知道总帧数，再从头循环。
        for wanted in [1usize, 2, 3] {
            h.wait_for("下一帧", |e| {
                matches!(e, Event::AnimationFrame { index, .. } if *index == wanted)
            });
        }
        h.wait_for("循环", |e| {
            matches!(e, Event::AnimationFrame { index: 0, count: Some(4), .. })
        });
        let info = h.core.animation().expect("是动图");
        assert_eq!(info.count, Some(4));
        assert!(info.playing);
        assert!(!h.core.needs_exact());
        // 暂停：不再送帧。
        assert_eq!(h.core.invoke_action("anim_toggle"), ActionOutcome::OverlayChanged);
        assert!(h.core.action_state("anim_toggle").checked);
        std::thread::sleep(Duration::from_millis(100));
        h.clear();
        let paused_at = h.core.animation().unwrap().index;
        std::thread::sleep(Duration::from_millis(150));
        assert!(h.core.drain_events().is_empty(), "暂停后还在送帧");
        // 逐帧：下一帧、上一帧。
        h.core.invoke_action("anim_next_frame");
        let next = (paused_at + 1) % 4;
        let stepped = h.wait_for("逐帧", |e| matches!(e, Event::AnimationFrame { index, .. } if *index == next));
        // 复制、打印的原图是现在显示的这一帧，不是第一帧。
        let Event::AnimationFrame { image, .. } = stepped else {
            unreachable!()
        };
        assert_eq!(h.core.original_image().unwrap().pixels, image.pixels);
        h.core.invoke_action("anim_prev_frame");
        h.wait_for("往回", |e| {
            matches!(e, Event::AnimationFrame { index, .. } if *index == paused_at)
        });
        // 切到静态图片：动画停下，动作不可用。
        h.core.invoke_action("next_image");
        h.wait_for("b.png", |e| is_ready(e, "b.png"));
        assert!(h.core.animation().is_none());
        assert!(!h.core.action_state("anim_toggle").enabled);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn transparent_areas_follow_the_two_background_settings() {
        use settings::Input::Choice;
        let h = Harness::new();
        let info = |format: &str, gif_background| crate::decode::ImageInfo {
            format: format.into(),
            pixel_format: "RGBA32".into(),
            file_size: 0,
            icc_profile: None,
            modified: None,
            has_alpha: true,
            decode_ms: 0.0,
            shooting: None,
            gif_background,
        };
        let png = DecodedImage::new(1, 1, vec![0; 4], info("PNG", None));
        let gif = DecodedImage::new(1, 1, vec![0; 4], info("GIF", Some(0xFF00FF)));
        let plain_gif = DecodedImage::new(1, 1, vec![0; 4], info("GIF", None));
        use settings::AlphaFill::{Checker, Solid};
        assert_eq!(h.core.alpha_fill(&png), Checker);
        assert_eq!(h.core.alpha_fill(&gif), Solid(0xFF00FF));
        assert_eq!(h.core.alpha_fill(&plain_gif), Solid(0x202020));
        apply(
            &h.core,
            &[
                (settings::GIF_BACKGROUND, Choice("grid".into())),
                (settings::ALPHA_BACKGROUND, Choice("white".into())),
            ],
        );
        assert_eq!(h.core.alpha_fill(&gif), Checker);
        assert_eq!(h.core.alpha_fill(&png), Solid(0xFFFFFF));
    }

    /// 在 dir 里做一个 ZIP：每个名字一张 8x6 的 PNG（不压缩）。
    fn zip_with_images(path: &Path, names: &[&str]) {
        zip_with_sized_images(path, names, 8, 6);
    }

    fn zip_with_sized_images(path: &Path, names: &[&str], width: u32, height: u32) {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (i, name) in names.iter().enumerate() {
            let mut png = Vec::new();
            image::RgbImage::from_pixel(width, height, image::Rgb([(i * 40) as u8, 100, 0]))
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            zip.start_file(*name, options).unwrap();
            zip.write_all(&png).unwrap();
        }
        zip.finish().unwrap();
    }

    fn is_current(event: &Event, wanted: &str) -> bool {
        matches!(event, Event::Current { name, .. } if name == wanted)
    }

    #[test]
    fn archives_browse_like_folders_and_remember_the_position() {
        let dir = crate::paths::canonical(&folder_with_images("archives", &["x.png"])).unwrap();
        let book = dir.join("book.cbz");
        zip_with_images(&book, &["ch2/1.png", "ch1/10.png", "ch1/2.png", "__MACOSX/ch1/._2.png"]);
        zip_with_images(&dir.join("next.zip"), &["only.png"]);
        let settings_file = settings_file("archives");
        let mut h = Harness::new();
        h.core.load_settings(Some(settings_file.clone()));
        // 这里看"询问"的写法（默认是直接回到首张）。
        apply(&h.core, &[(settings::AT_ARCHIVE_BOUNDARY, settings::Input::Choice("ask".into()))]);
        h.core.dispatch(Command::Open {
            path: book.display().to_string(),
        });
        // 包内路径按自然排序：ch1/2、ch1/10、ch2/1；跳过 __MACOSX。
        h.wait_for("第一张", |e| is_current(e, "book.cbz › 2.png"));
        h.wait_for("出图", |e| matches!(e, Event::ImageReady { .. }));
        assert!(!h.core.action_state("delete_file").enabled, "压缩包里不能删除");
        assert!(!h.core.action_state("move_to_folder1").enabled);
        h.core.invoke_action("next_image");
        h.wait_for("下一张", |e| is_current(e, "book.cbz › 10.png"));
        h.wait_for("出图", |e| matches!(e, Event::ImageReady { .. }));
        // 到末尾：询问时说"压缩包"。
        h.core.invoke_action("last_image");
        h.wait_for("末张", |e| is_current(e, "book.cbz › 1.png"));
        h.core.invoke_action("next_image");
        h.wait_for("询问", |e| matches!(e, Event::Boundary { forward: true, archive: true }));
        // 下一个压缩包：同一文件夹里按名字排在后面的 next.zip。
        h.core.invoke_action("next_container");
        h.wait_for("下一个压缩包", |e| is_current(e, "next.zip › only.png"));
        // 回到 book.cbz：从上次看到的那一张（ch2/1.png）开始。
        h.core.dispatch(Command::Open {
            path: book.display().to_string(),
        });
        h.wait_for("记住的位置", |e| is_current(e, "book.cbz › 1.png"));
        // 复制到图片文件夹：两次，第二次加序号。
        let target = dir.join("copies");
        std::fs::create_dir_all(&target).unwrap();
        apply(
            &h.core,
            &[(settings::FOLDER1, settings::Input::Text(target.display().to_string()))],
        );
        h.clear();
        for _ in 0..2 {
            h.core.invoke_action("copy_to_folder1");
            h.wait_for("复制完", |e| matches!(e, Event::Notice(NoticeKind::Copied { .. })));
        }
        assert!(target.join("1.png").is_file() && target.join("1 (2).png").is_file());
        h.core.shutdown();
        assert!(settings_file
            .parent()
            .unwrap()
            .join(crate::positions::FILE_NAME)
            .is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn virtual_paths_split_into_archive_and_entry() {
        let dir = crate::paths::canonical(&folder_with_images("split", &["x.png"])).unwrap();
        let book = dir.join("b.zip");
        zip_with_images(&book, &["a/b.png"]);
        let inner = crate::archive::entry_path(&book, "a/b.png");
        assert_eq!(
            crate::archive::split(&inner),
            Some((book.clone(), "a/b.png".to_string()))
        );
        assert_eq!(crate::archive::split(&dir.join("x.png")), None);
        let m = crate::archive::materialize(&inner).unwrap();
        assert!(m.file.is_file());
        assert_eq!(m.file.file_name().unwrap(), "b.png");
        assert!(crate::decode::decode_file(&inner).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_page_view_pairs_portrait_pages_and_keeps_landscape_ones_alone() {
        let dir = std::env::temp_dir().join(format!("anyview-api-{}-spread", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // a、b 竖图，c 横图，d、e 竖图；颜色各不相同。
        for (name, w, h, r) in [("a", 6, 8, 10u8), ("b", 6, 8, 60), ("c", 8, 6, 110), ("d", 6, 8, 160), ("e", 6, 8, 210)] {
            image::RgbImage::from_pixel(w, h, image::Rgb([r, 0, 0]))
                .save(dir.join(format!("{name}.png")))
                .unwrap();
        }
        let dir = crate::paths::canonical(&dir).unwrap();
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 5, .. }));
        let shown = |h: &mut Harness, what: &str| -> (u32, u32, u8, u8) {
            let event = h.wait_for(what, |e| matches!(e, Event::ImageReady { .. }));
            let Event::ImageReady { image, .. } = event else { unreachable!() };
            // 左上、右下两点的红色分量（BGRA：第 3 个字节）。
            let at = |x: usize, y: usize| image.pixels[y * image.stride + x * 4 + 2];
            let (w, h) = (image.width as usize, image.height as usize);
            (image.width, image.height, at(0, 0), at(w - 1, h - 1))
        };
        h.core.invoke_action("double_page_ltr");
        assert!(h.core.action_state("double_page_ltr").checked);
        h.wait_for("两页的名字", |e| matches!(e, Event::Current { name, .. } if name == "a.png | b.png"));
        assert_eq!(shown(&mut h, "a+b"), (12, 8, 10, 60));
        // 开着反相复制：原图仍是两页合成的、没有反相（显示效果不改变复制的页数）。
        h.core.invoke_action("invert");
        h.wait_for("反相后的一组", |e| matches!(e, Event::ImageUpdated { .. }));
        let original = h.core.original_image().unwrap();
        assert_eq!((original.width, original.height), (12, 8));
        let red = |x: usize, y: usize| original.pixels[y * original.stride + x * 4 + 2];
        assert_eq!((red(0, 0), red(11, 7)), (10, 60));
        h.core.invoke_action("invert");
        h.wait_for("反相关掉", |e| matches!(e, Event::ImageUpdated { .. }));
        h.core.invoke_action("next_image");
        assert_eq!(shown(&mut h, "c 单独"), (8, 6, 110, 110));
        h.core.invoke_action("next_image");
        assert_eq!(shown(&mut h, "d+e"), (12, 8, 160, 210));
        h.core.invoke_action("prev_image");
        assert_eq!(shown(&mut h, "回到 c"), (8, 6, 110, 110));
        h.core.invoke_action("prev_image");
        assert_eq!(shown(&mut h, "回到 a+b"), (12, 8, 10, 60));
        h.core.invoke_action("last_image");
        assert_eq!(shown(&mut h, "末尾一组"), (12, 8, 160, 210));
        // 左←右：前一页在右边。
        h.core.invoke_action("double_page_rtl");
        assert_eq!(shown(&mut h, "左←右"), (12, 8, 210, 160));
        // 再按一次关掉：只显示当前这一张。
        h.core.invoke_action("double_page_rtl");
        assert!(!h.core.action_state("double_page_rtl").checked);
        let (w, _, _, _) = shown(&mut h, "关掉双页");
        assert_eq!(w, 6);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_changes_refresh_the_collection() {
        let dir = crate::paths::canonical(&folder_with_images("watch", &["a.png", "b.png"])).unwrap();
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 2, .. }));
        // 加了一张：几秒内浏览集合变成 3 张，当前图片不变。
        std::thread::sleep(Duration::from_millis(1100)); // 修改时间按秒记录的文件系统上也能看出变化
        std::fs::copy(dir.join("b.png"), dir.join("c.png")).unwrap();
        h.wait_for("多了一张", |e| matches!(e, Event::Collection { total: 3, index: Some(0), .. }));
        // 当前图片被删掉：显示同一位置的那一张。
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::remove_file(dir.join("a.png")).unwrap();
        h.wait_for("换成 b.png", |e| is_current(e, "b.png"));
        // 浏览集合也跟着报一次（选择图片窗口按新的列表重新载入，不会按旧序号选到别的图）。
        h.wait_for("少了一张", |e| matches!(e, Event::Collection { total: 2, index: Some(0), .. }));
        // 当前图片的文件改了：重新读、重新显示。
        h.clear();
        std::thread::sleep(Duration::from_millis(1100));
        image::RgbImage::from_pixel(10, 7, image::Rgb([1, 2, 3]))
            .save(dir.join("b.png"))
            .unwrap();
        h.wait_for("重新显示", |e| {
            matches!(e, Event::ImageReady { image, .. } if (image.width, image.height) == (10, 7))
        });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn snapping_follows_the_setting() {
        let h = Harness::new();
        let frame = (5.0, 100.0, 600.0, 400.0);
        let area = (0.0, 0.0, 1440.0, 900.0);
        assert_eq!(h.core.snap_window(frame, area), None);
        lock(&h.core.shared.hub.prefs)
            .settings
            .set(settings::SNAP_WINDOW, Value::Bool(true));
        assert_eq!(h.core.snap_window(frame, area), Some((0.0, 100.0)));
    }

    fn settings_file(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("anyview-api-{}-settings-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.toml")
    }

    #[test]
    fn background_image_is_decoded_in_the_background_and_follows_the_setting() {
        let dir = folder_with_images("background", &["bg.png"]);
        let mut h = Harness::new();
        assert!(h.core.background_image().is_none(), "默认用背景色");
        {
            let mut prefs = lock(&h.core.shared.hub.prefs);
            prefs.settings.set(settings::BACKGROUND_IMAGE, Value::Bool(true));
            prefs.settings.set(
                settings::BACKGROUND_IMAGE_PATH,
                Value::Text(dir.join("bg.png").display().to_string()),
            );
        }
        assert!(h.core.background_image().is_none(), "第一次要时在后台解码");
        h.wait_for("背景图片", |e| matches!(e, Event::BackgroundChanged));
        let image = h.core.background_image().expect("应解码好了");
        assert_eq!((image.width, image.height), (8, 6));
        // 打不开的文件：用背景色。
        lock(&h.core.shared.hub.prefs).settings.set(
            settings::BACKGROUND_IMAGE_PATH,
            Value::Text(dir.join("没有.png").display().to_string()),
        );
        assert!(h.core.background_image().is_none());
        h.wait_for("换了", |e| matches!(e, Event::BackgroundChanged));
        assert!(h.core.background_image().is_none());
        lock(&h.core.shared.hub.prefs)
            .settings
            .set(settings::BACKGROUND_IMAGE, Value::Bool(false));
        assert!(h.core.background_image().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn help_lists_effective_shortcuts_and_formats() {
        // 按键随平台（测试在 Linux 和 macOS 上都跑）。
        let mac = Platform::current() == Platform::Macos;
        let h = Harness::new();
        let rows = h.core.help_rows(crate::help::HelpKind::Shortcuts, Lang::ZhHans);
        assert_eq!(rows[0], crate::help::HelpRow::heading("常用"));
        let open = rows.iter().find(|r| r.left == "打开图片").expect("有打开图片");
        assert_eq!(open.right, if mac { "⌘O" } else { "F2" });
        assert!(rows.iter().any(|r| r.heading && r.left == "浏览导航"));
        assert!(rows.iter().any(|r| r.heading && r.left == "其他"));
        assert!(rows.iter().any(|r| r.left == "像素边缘：减淡"));
        if !mac {
            assert!(rows.iter().any(|r| r.left == "关于" && r.right == "F1"));
        }
        // 标题栏隐藏时显示文件名只在 macOS、Windows 有（Linux 的标题栏由窗口管理器一直显示）。
        let title_filename = matches!(Platform::current(), Platform::Macos | Platform::Windows);
        assert_eq!(rows.iter().any(|r| r.left.contains("显示文件名")), title_filename);
        let formats = h.core.help_rows(crate::help::HelpKind::Formats, Lang::En);
        assert_eq!(formats[0], crate::help::HelpRow::heading("Common formats"));
        assert_eq!(
            h.core.invoke_action("help_formats"),
            ActionOutcome::Ui(UiRequest::ShowHelp {
                kind: crate::help::HelpKind::Formats
            })
        );
        assert_eq!(h.core.action_state("toggle_title_filename").enabled, title_filename);
        assert!(!h.core.ui_settings().title_filename, "默认关");
    }

    #[test]
    fn another_process_changing_the_settings_file_is_picked_up() {
        let file = settings_file("shared");
        let mut h = Harness::new();
        h.core.load_settings(Some(file.clone()));
        // 自己保存一次（不算别人改的）。
        lock(&h.core.shared.hub.prefs)
            .settings
            .set(settings::BACKGROUND_COLOR, Value::Color(0x112233));
        lock(&h.core.shared.hub.prefs).save(Platform::current()).unwrap();
        check_settings_file(&h.core.shared.hub);
        h.clear();
        // 别的进程写了新的背景色（文件的修改时间要变：等一下再写）。
        std::thread::sleep(Duration::from_millis(20));
        let text = std::fs::read_to_string(&file).unwrap().replace("#112233", "#336699");
        std::fs::write(&file, text).unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(2);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later)
            .unwrap();
        check_settings_file(&h.core.shared.hub);
        let Event::SettingsReloaded { changed } =
            h.wait_for("重新读", |e| matches!(e, Event::SettingsReloaded { .. }))
        else {
            unreachable!()
        };
        assert_eq!(changed, [settings::BACKGROUND_COLOR]);
        assert_eq!(h.core.ui_settings().background_color, 0x336699);
        // 关掉"多窗口共享设置"后不再跟着变。
        lock(&h.core.shared.hub.prefs)
            .settings
            .set(settings::SHARE_SETTINGS, Value::Bool(false));
        let text = std::fs::read_to_string(&file).unwrap().replace("#336699", "#445566");
        std::fs::write(&file, text).unwrap();
        check_settings_file(&h.core.shared.hub);
        assert_eq!(h.core.ui_settings().background_color, 0x336699);
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn boundary_setting_controls_what_happens_at_the_ends() {
        use settings::Input::Choice;
        let dir = folder_with_images("boundary-setting", &["a.png", "b.png"]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("b.png").display().to_string(),
        });
        h.wait_for("scan", |e| matches!(e, Event::Collection { total: 2, .. }));
        // 无操作：提示"已是最后一张"。
        apply(&h.core, &[(settings::AT_BOUNDARY, Choice("none".into()))]);
        h.clear();
        h.core.dispatch(Command::Navigate(Navigation::Next));
        h.wait_for("notice", |e| matches!(e, Event::Notice(NoticeKind::AtLast)));
        // 转到首个文件：循环。
        apply(&h.core, &[(settings::AT_BOUNDARY, Choice("loop".into()))]);
        assert_eq!(
            go(&mut h, |c| c.dispatch(Command::Navigate(Navigation::Next))),
            "a.png"
        );
        assert_eq!(
            go(&mut h, |c| c
                .dispatch(Command::Navigate(Navigation::Back10))),
            "b.png"
        );
        // 询问（默认）。
        apply(&h.core, &[(settings::AT_BOUNDARY, Choice("ask".into()))]);
        h.clear();
        h.core.dispatch(Command::Navigate(Navigation::Next));
        h.wait_for("boundary", |e| {
            matches!(e, Event::Boundary { forward: true, .. })
        });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn boundary_setting_can_continue_in_the_next_folder() {
        let root =
            std::env::temp_dir().join(format!("anyview-api-{}-boundary-next", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (folder, name) in [("1", "x.png"), ("2", "y.png")] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            image::RgbImage::from_pixel(4, 4, image::Rgb([9, 9, 9]))
                .save(root.join(folder).join(name))
                .unwrap();
        }
        let mut h = Harness::new();
        apply(
            &h.core,
            &[(
                settings::AT_BOUNDARY,
                settings::Input::Choice("next_folder".into()),
            )],
        );
        h.core.dispatch(Command::Open {
            path: root.join("1").join("x.png").display().to_string(),
        });
        h.wait_for("scan", |e| matches!(e, Event::Collection { total: 1, .. }));
        assert_eq!(
            go(&mut h, |c| c.dispatch(Command::Navigate(Navigation::Next))),
            "y.png"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn esc_delete_confirm_and_window_settings_take_effect() {
        use settings::Input::Bool;
        let h = Harness::new();
        assert_eq!(h.core.action_for_key("Escape", 0), Some("close_or_quit"));
        assert!(
            h.core
                .window_size_for_image(800, 600, (1600.0, 1000.0))
                .is_some()
        );
        let applied = apply(
            &h.core,
            &[
                (settings::ESC_QUITS, Bool(false)),
                (settings::CONFIRM_DELETE, Bool(false)),
                (settings::FIT_WINDOW, Bool(false)),
                // 界面上是"控制栏"开关（翻过来说的）：关掉就是不显示控制栏。
                (settings::HIDE_CONTROL_BAR, Bool(false)),
            ],
        );
        assert_eq!(applied.changed.len(), 4);
        assert_eq!(applied.save_error, None);
        assert_eq!(h.core.action_for_key("Escape", 0), None);
        // 其他退出键不受影响。
        let quit = if Platform::current() == Platform::Macos {
            h.core.action_for_key("q", actions::CMD)
        } else {
            h.core.action_for_key("x", 0)
        };
        assert_eq!(quit, Some("quit"));
        assert!(
            h.core
                .window_size_for_image(800, 600, (1600.0, 1000.0))
                .is_none()
        );
        assert!(
            h.core
                .window_size_after_action("rotate_cw", (1600.0, 1000.0))
                .is_none()
        );
        assert!(!h.core.action_state("toggle_bar_lock").enabled);
        let ui = h.core.ui_settings();
        assert!(ui.hide_control_bar && !ui.hide_side_arrows);
        assert!(h.core.action_state("settings").enabled);
        assert_eq!(
            h.core.invoke_action("settings"),
            ActionOutcome::Ui(UiRequest::ShowSettings)
        );
        let dir = folder_with_images("confirm", &["a.png"]);
        let mut h = h;
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("ready", |e| is_ready(e, "a.png"));
        assert!(matches!(
            h.core.invoke_action("delete_file"),
            ActionOutcome::Ui(UiRequest::TrashFile { confirm: false, .. })
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_draft_is_reported_and_kept() {
        let h = Harness::new();
        h.core.settings_begin();
        assert!(
            h.core
                .settings_set(settings::CACHE_MB, settings::Input::Text("12".into()))
        );
        assert!(h.core.settings_dirty());
        let invalid = h.core.settings_apply().unwrap_err();
        assert_eq!(invalid.key, settings::CACHE_MB);
        assert_eq!(invalid.page, settings::Page::Image);
        // 草稿还在，改对了就能确定。
        assert!(
            h.core
                .settings_set(settings::CACHE_MB, settings::Input::Text("100".into()))
        );
        let page = h.core.settings_page("image", Lang::ZhHans).unwrap();
        assert_eq!(page.title, "图像处理");
        assert_eq!(page.sections, ["显示", "画质", "颜色与透明", "缓存"]);
        let cache = page
            .items
            .iter()
            .find(|i| i.key == settings::CACHE_MB)
            .unwrap();
        assert_eq!(
            cache.control,
            settings::Control::Number {
                text: "100".into(),
                unit: "MB".into()
            }
        );
        h.core.settings_apply().unwrap();
        assert!(!h.core.settings_dirty());
        assert_eq!(h.core.setting(settings::CACHE_MB), Some(Value::Int(100)));
        assert_eq!(lock(&h.core.shared.hub.cache).budget(), 100 * 1024 * 1024);
        // 取消：草稿丢掉，设置不变。
        h.core.settings_begin();
        h.core
            .settings_set(settings::CACHE_MB, settings::Input::Text("200".into()));
        h.core.settings_cancel();
        assert_eq!(h.core.setting(settings::CACHE_MB), Some(Value::Int(100)));
    }

    #[test]
    fn remembered_state_and_settings_survive_a_restart() {
        let path = settings_file("restart");
        let h = Harness::new();
        assert_eq!(
            h.core.load_settings(Some(path.clone())),
            settings::LoadStatus::Missing
        );
        h.core.invoke_action("sort_size");
        h.core.invoke_action("sort_desc");
        h.core.invoke_action("toggle_info");
        h.core.invoke_action("edge_none");
        h.core.invoke_action("interp_lanczos");
        apply(
            &h.core,
            &[
                (settings::BACKGROUND_COLOR, settings::Input::Color(0x336699)),
                (settings::LANGUAGE, settings::Input::Choice("en".into())),
                (settings::SAVE_ZOOM_STATE, settings::Input::Bool(true)),
            ],
        );
        h.core.invoke_action("fit_width");
        assert!(path.exists());
        drop(h);
        let h = Harness::new();
        assert_eq!(
            h.core.load_settings(Some(path.clone())),
            settings::LoadStatus::Loaded
        );
        assert_eq!(h.core.ui_settings().background_color, 0x336699);
        assert_eq!(h.core.ui_lang(Lang::ZhHans), Lang::En);
        assert!(h.core.action_state("sort_size").checked);
        assert!(h.core.action_state("sort_desc").checked);
        assert!(h.core.action_state("toggle_info").checked);
        assert!(h.core.action_state("edge_none").checked);
        assert!(h.core.action_state("interp_lanczos").checked);
        assert!(
            h.core.action_state("fit_width").checked,
            "保存缩放状态"
        );
        // 设置窗口里的插值算法与菜单是同一个值。
        let page = h.core.settings_page("image", Lang::En).unwrap();
        let interp = page
            .items
            .iter()
            .find(|i| i.key == settings::INTERPOLATION)
            .unwrap();
        assert!(
            matches!(&interp.control, settings::Control::Choice { selected, .. } if selected == "lanczos")
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_broken_settings_file_gives_one_notice() {
        let path = settings_file("broken");
        std::fs::write(&path, "[general\n").unwrap();
        let h = Harness::new();
        assert!(matches!(
            h.core.load_settings(Some(path.clone())),
            settings::LoadStatus::Recovered { .. }
        ));
        let notice = h.core.settings_notice(Lang::ZhHans).expect("应提示");
        assert!(notice.contains("settings-broken-"), "{notice}");
        assert_eq!(h.core.settings_notice(Lang::ZhHans), None, "只提示一次");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn changing_decode_settings_reloads_the_current_image() {
        let dir = folder_with_images("reload", &["a.png"]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("ready", |e| is_ready(e, "a.png"));
        h.clear();
        apply(
            &h.core,
            &[(settings::EXIF_ROTATE, settings::Input::Bool(false))],
        );
        h.wait_for(
            "current again",
            |e| matches!(e, Event::Current { name, .. } if name == "a.png"),
        );
        h.wait_for("decoded again", |e| is_ready(e, "a.png"));
        // 与解码无关的设置不重新打开。
        h.clear();
        apply(
            &h.core,
            // "鼠标悬停时的提示"开关关掉（存的是"不显示鼠标悬浮工具提示"）。
            &[(settings::HIDE_TOOLTIPS, settings::Input::Bool(false))],
        );
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            h.core
                .drain_events()
                .iter()
                .all(|e| !matches!(e, Event::Current { .. })),
            "不应重新打开"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn key_mappings_override_defaults_and_show_in_menus() {
        use settings::Input;
        let h = Harness::new();
        let p = Platform::current();
        // PageDown、PageUp 来自默认勾选的两行映射。
        assert_eq!(h.core.action_for_key("PageDown", 0), Some("next_container"));
        assert_eq!(h.core.action_for_key("]", 0), Some("next_container"));
        let shortcut = |id: &str| {
            h.core
                .menu(actions::MenuKind::Context, Lang::ZhHans)
                .into_iter()
                .find_map(|e| match e {
                    actions::MenuEntry::Action {
                        id: i, shortcut, ..
                    } if i == id => Some(shortcut),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(
            shortcut("next_image").map(|c| c.key),
            Some("Right".to_string())
        );
        apply(
            &h.core,
            &[
                (
                    settings::KEY_MAPPINGS,
                    Input::KeyRowEnabled { row: 0, on: false },
                ),
                (
                    settings::KEY_MAPPINGS,
                    Input::KeyRowEnabled { row: 2, on: true },
                ),
                (
                    settings::KEY_MAPPINGS,
                    Input::KeyRowKey {
                        row: 2,
                        key: "q".into(),
                        mods: 0,
                    },
                ),
                (
                    settings::KEY_MAPPINGS,
                    Input::KeyRowAction {
                        row: 2,
                        action: "zoom_in".into(),
                    },
                ),
                (
                    settings::KEY_MAPPINGS,
                    Input::KeyRowEnabled { row: 3, on: true },
                ),
                (
                    settings::KEY_MAPPINGS,
                    Input::KeyRowKey {
                        row: 3,
                        key: "Right".into(),
                        mods: 0,
                    },
                ),
                (
                    settings::KEY_MAPPINGS,
                    Input::KeyRowAction {
                        row: 3,
                        action: "zoom_out".into(),
                    },
                ),
            ],
        );
        assert_eq!(
            h.core.action_for_key("PageDown", 0),
            None,
            "第 1 行取消了勾选"
        );
        assert_eq!(h.core.action_for_key("q", 0), Some("zoom_in"));
        assert_eq!(h.core.action_for_key("Right", 0), Some("zoom_out"));
        // 菜单显示实际生效的按键：下一张的 → 被占用了；缩小多了一个映射，但菜单仍显示默认的第一个键位。
        assert_eq!(shortcut("next_image"), None);
        assert_eq!(shortcut("zoom_out").map(|c| c.key), Some("-".to_string()));
        let tip = h.core.tooltip("next_image", Lang::ZhHans);
        assert_eq!(tip, "下一张图片");
        let _ = p;
    }

    #[test]
    fn mouse_settings_decide_clicks_wheel_and_buttons() {
        use settings::{Input, MouseBinding, MouseInput};
        let h = Harness::new();
        // 默认：单击不用等，双击最大化，滚轮、侧键、右键按默认处理。
        assert!(!h.core.left_click_matters());
        assert_eq!(h.core.left_double_click_action(0.5), Some("maximize"));
        assert_eq!(h.core.left_click_action(0.1), None);
        assert_eq!(
            h.core.mouse_action(MouseInput::WheelUp),
            MouseBinding::Default
        );
        // 单击两侧翻页：两侧单击翻页，中间双击最大化。
        apply(&h.core, &[(settings::CLICK_SIDES, Input::Bool(true))]);
        assert!(h.core.left_click_matters());
        assert_eq!(h.core.left_click_action(0.1), Some("prev_image"));
        assert_eq!(h.core.left_click_action(0.5), None);
        assert_eq!(h.core.left_click_action(0.9), Some("next_image"));
        assert_eq!(h.core.left_double_click_action(0.9), None);
        assert_eq!(h.core.left_double_click_action(0.5), Some("maximize"));
        // 映射左键：每次单击执行映射的动作，双击不另外做事；双击最大化和两侧翻页灰显。
        apply(
            &h.core,
            &[
                (
                    settings::MOUSE_LEFT,
                    Input::MappingChoice("toggle_info".into()),
                ),
                (
                    settings::MOUSE_WHEEL_DOWN,
                    Input::MappingChoice("zoom_out".into()),
                ),
                (settings::MOUSE_RIGHT, Input::MappingChoice(String::new())),
            ],
        );
        assert_eq!(h.core.left_click_action(0.5), Some("toggle_info"));
        assert_eq!(h.core.left_double_click_action(0.5), None);
        assert_eq!(
            h.core.mouse_action(MouseInput::WheelDown),
            MouseBinding::Action("zoom_out")
        );
        assert_eq!(
            h.core.mouse_action(MouseInput::Right),
            MouseBinding::Nothing
        );
        let page = h.core.settings_page("mouse", Lang::ZhHans).unwrap();
        let double = page
            .items
            .iter()
            .find(|i| i.key == settings::DOUBLE_CLICK_MAXIMIZE)
            .unwrap();
        assert!(!double.enabled, "映射了左键时双击最大化灰显");
    }

    /// 两张竖图 a（红 10）、b（红 60）的文件夹，外加没有图片的压缩包、坏的压缩包、空文件夹。
    fn folder_with_bad_containers(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anyview-api-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("nothing")).unwrap();
        for (name, r) in [("a", 10u8), ("b", 60)] {
            image::RgbImage::from_pixel(6, 8, image::Rgb([r, 0, 0]))
                .save(dir.join(format!("{name}.png")))
                .unwrap();
        }
        zip_with_images(&dir.join("empty.zip"), &["readme.txt"]);
        std::fs::write(dir.join("broken.zip"), b"this is not a zip archive").unwrap();
        crate::paths::canonical(&dir).unwrap()
    }

    /// 打开没有图片的压缩包、坏的压缩包、空文件夹：只提示，正在看的图片和浏览集合不动，还能接着翻。
    /// （以前先把列表清空了：提示之后翻页键全部失灵，双页时切换效果还会按旧序号取空列表而崩溃。）
    #[test]
    fn a_failed_open_leaves_the_image_and_the_collection_alone() {
        let dir = folder_with_bad_containers("failed-open");
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 2, .. }));
        for (name, what) in [("empty.zip", "空压缩包"), ("broken.zip", "坏压缩包"), ("nothing", "空文件夹")] {
            h.clear();
            h.core.dispatch(Command::Open {
                path: dir.join(name).display().to_string(),
            });
            let event = h.wait_for(what, |e| matches!(e, Event::Notice(_) | Event::OpenFailed { .. }));
            match name {
                "empty.zip" => assert!(matches!(event, Event::Notice(NoticeKind::EmptyArchive)), "{event:?}"),
                "broken.zip" => assert!(matches!(event, Event::OpenFailed { .. }), "{event:?}"),
                _ => assert!(matches!(event, Event::Notice(NoticeKind::EmptyFolder)), "{event:?}"),
            }
            assert_eq!(h.core.current_path().as_deref(), Some(dir.join("a.png").as_path()), "{what}");
            let list = h.core.picker_list(Lang::ZhHans).expect("浏览集合还在");
            assert_eq!(list.rows.len(), 2, "{what}");
            assert!(h.core.action_state("delete_file").enabled, "{what}：不是在压缩包里");
            // 还能接着翻。
            h.core.invoke_action("next_image");
            h.wait_for("b.png", |e| is_ready(e, "b.png"));
            h.core.invoke_action("prev_image");
            h.wait_for("回到 a.png", |e| is_ready(e, "a.png"));
        }
        // 双页：a、b 成一组显示着，打开失败后切换效果，还是两页的那一组。
        h.core.invoke_action("double_page_ltr");
        h.wait_for("a+b", |e| matches!(e, Event::ImageReady { image, .. } if image.width == 12));
        for name in ["empty.zip", "broken.zip", "nothing"] {
            h.clear();
            h.core.dispatch(Command::Open {
                path: dir.join(name).display().to_string(),
            });
            h.wait_for(name, |e| matches!(e, Event::Notice(_) | Event::OpenFailed { .. }));
            h.core.invoke_action("invert");
            let Event::ImageUpdated { image, .. } =
                h.wait_for("换了效果的一组", |e| matches!(e, Event::ImageUpdated { .. }))
            else {
                unreachable!()
            };
            assert_eq!((image.width, image.height), (12, 8), "{name}");
        }
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页时打开图片：列表扫描好之前不显示（不知道另一页是谁），好了之后第一次显示就是成组的两页——
    /// 不会先单独显示一页、再以"同一张图片更新"换成两页（界面不会为它重新排版）。关着预加载也一样。
    #[test]
    fn two_page_view_waits_for_the_folder_scan() {
        for prefetch in [true, false] {
            let dir = std::env::temp_dir()
                .join(format!("anyview-api-{}-spread-scan-{prefetch}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            for (name, r) in [("a", 10u8), ("b", 60)] {
                image::RgbImage::from_pixel(6, 8, image::Rgb([r, 0, 0]))
                    .save(dir.join(format!("{name}.png")))
                    .unwrap();
            }
            // 再放几千个文件：扫描比解一张 6×8 的图片慢得多，图片先解好。
            for i in 0..4000 {
                std::fs::write(dir.join(format!("z{i:04}.png")), b"").unwrap();
            }
            let dir = crate::paths::canonical(&dir).unwrap();
            let mut h = Harness::new();
            apply(&h.core, &[(settings::PREFETCH, settings::Input::Bool(prefetch))]);
            h.core.invoke_action("double_page_ltr");
            h.clear();
            h.core.dispatch(Command::Open {
                path: dir.join("a.png").display().to_string(),
            });
            let first = h.wait_for("第一次显示", |e| {
                matches!(e, Event::ImageReady { .. } | Event::ImageUpdated { .. })
            });
            let Event::ImageReady { image, .. } = first else {
                panic!("第一次显示应当是 ImageReady：{first:?}")
            };
            assert_eq!((image.width, image.height), (12, 8), "预加载 {prefetch}：第一次显示就是两页");
            h.wait_for("两页的名字", |e| matches!(e, Event::Current { name, .. } if name == "a.png | b.png"));
            drop(h);
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// 看别的图片时这一张被改了：翻回来显示新的。文件有没有改过由后台线程核对（界面线程翻页时不读磁盘）：
    /// 缓存里旧的可能先出来一下，但马上换成新的，不用等下一轮检查。
    #[test]
    fn a_file_changed_while_viewing_another_is_read_again() {
        let dir = crate::paths::canonical(&folder_with_images("changed-away", &["a.png", "b.png"])).unwrap();
        // 整轮检查隔一小时：下面等到的只能是显示时马上核对的结果。
        let mut h = Harness::with_watch_interval(Duration::from_secs(3600));
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 2, .. }));
        h.core.invoke_action("next_image");
        h.wait_for("b.png", |e| is_ready(e, "b.png"));
        std::thread::sleep(Duration::from_millis(1100)); // 修改时间按秒记录的文件系统上也能看出变化
        image::RgbImage::from_pixel(10, 7, image::Rgb([1, 2, 3]))
            .save(dir.join("a.png"))
            .unwrap();
        h.clear();
        h.core.invoke_action("prev_image");
        h.wait_for("改过的 a.png", |e| {
            matches!(e, Event::ImageReady { path, image, .. }
                if path.file_name().unwrap() == "a.png" && (image.width, image.height) == (10, 7))
        });
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 缓存着、就在旁边的图片（预加载范围里）被改了：后台核对时换成新的，翻过去时第一次显示就是新的。
    #[test]
    fn nearby_cached_images_are_refreshed_before_they_are_shown() {
        let dir = crate::paths::canonical(&folder_with_images("changed-nearby", &["a.png", "b.png"])).unwrap();
        let a = dir.join("a.png");
        let mut h = Harness::with_watch_interval(Duration::from_secs(3600));
        h.core.dispatch(Command::Open {
            path: a.display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 2, .. }));
        h.core.invoke_action("next_image");
        h.wait_for("b.png", |e| is_ready(e, "b.png"));
        std::thread::sleep(Duration::from_millis(1100));
        image::RgbImage::from_pixel(10, 7, image::Rgb([1, 2, 3])).save(&a).unwrap();
        // 让 b 再显示一次（换个效果）：显示时后台核对，顺带看到旁边的 a 改过了。
        h.core.invoke_action("invert");
        eventually("缓存里的 a 换成新的", || {
            lock(&h.core.shared.hub.cache)
                .get(&a, Effects::NONE)
                .is_some_and(|image| image.width == 10)
        });
        h.clear();
        h.core.invoke_action("prev_image");
        let first = h.wait_for("a.png", |e| {
            matches!(e, Event::ImageReady { path, .. } | Event::ImageUpdated { path, .. }
                if path.file_name().unwrap() == "a.png")
        });
        let Event::ImageReady { image, .. } = first else {
            panic!("{first:?}")
        };
        assert_eq!((image.width, image.height), (10, 7), "第一次显示就是新的");
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页时也一样：这一组两页都被改过，翻回来（很快）显示的是新的两页。
    #[test]
    fn two_page_view_reads_a_changed_neighbor_again() {
        let dir = folder_with_pages("spread-stale", &[10, 60, 110, 160]);
        let mut h = Harness::with_watch_interval(Duration::from_secs(3600));
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 4, .. }));
        h.core.invoke_action("double_page_ltr");
        wait_shown(&mut h, "a+b", (12, 10, 60));
        h.core.invoke_action("next_image");
        wait_shown(&mut h, "c+d", (12, 110, 160));
        // 往回翻时这一组以 b 结尾：b 是当前图片，a 是另一页。两张都改掉。
        std::thread::sleep(Duration::from_millis(1100)); // 修改时间按秒记录的文件系统上也能看出变化
        for (name, r) in [("a", 200u8), ("b", 250)] {
            image::RgbImage::from_pixel(6, 8, image::Rgb([r, 0, 0]))
                .save(dir.join(format!("{name}.png")))
                .unwrap();
        }
        h.clear();
        h.core.invoke_action("prev_image");
        wait_shown(&mut h, "改过的 a+b", (12, 200, 250));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页时显示着的两页，哪一页的文件改了都重新显示，而且还是原来那两页：往回翻出来的
    /// "上一张 + 这一张"不会变成"这一张 + 下一张"。
    #[test]
    fn two_page_view_reloads_whichever_page_changes() {
        let dir = folder_with_pages("spread-edit", &[10, 60, 110, 160]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 4, .. }));
        h.core.invoke_action("double_page_ltr");
        wait_shown(&mut h, "a+b", (12, 10, 60));
        h.core.invoke_action("next_image");
        wait_shown(&mut h, "c+d", (12, 110, 160));
        h.core.invoke_action("prev_image");
        wait_shown(&mut h, "a+b（b 是当前图片）", (12, 10, 60));
        assert_eq!(h.core.current_path().unwrap().file_name().unwrap(), "b.png");
        std::thread::sleep(Duration::from_millis(1100));
        // 另一页（a）改了。
        image::RgbImage::from_pixel(6, 8, image::Rgb([200, 0, 0]))
            .save(dir.join("a.png"))
            .unwrap();
        wait_shown(&mut h, "a 改了", (12, 200, 60));
        std::thread::sleep(Duration::from_millis(1100));
        // 当前图片（b）改了。
        image::RgbImage::from_pixel(6, 8, image::Rgb([250, 0, 0]))
            .save(dir.join("b.png"))
            .unwrap();
        wait_shown(&mut h, "b 改了", (12, 200, 250));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 自己保存的设置读回来写法不同（键盘映射的空行不写进文件）：不能当成别的窗口改过，再"重新读"一遍。
    #[test]
    fn our_own_saves_are_not_taken_for_someone_elses_changes() {
        let file = settings_file("own-saves");
        let mut h = Harness::new();
        h.core.load_settings(Some(file.clone()));
        // 第 5 行设一个映射，前面留着空行。
        apply(
            &h.core,
            &[
                (
                    settings::KEY_MAPPINGS,
                    settings::Input::KeyRowKey { row: 5, key: "q".into(), mods: 0 },
                ),
                (
                    settings::KEY_MAPPINGS,
                    settings::Input::KeyRowAction { row: 5, action: "zoom_in".into() },
                ),
                (settings::KEY_MAPPINGS, settings::Input::KeyRowEnabled { row: 5, on: true }),
            ],
        );
        // 再存一次别的。
        let applied = apply(&h.core, &[(settings::SAVE_ZOOM_STATE, settings::Input::Bool(true))]);
        assert_eq!(applied.save_error, None);
        h.clear();
        check_settings_file(&h.core.shared.hub);
        let events = h.core.drain_events();
        assert!(
            !events.iter().any(|e| matches!(e, Event::SettingsReloaded { .. })),
            "{events:?}"
        );
        assert_eq!(h.core.action_for_key("q", 0), Some("zoom_in"));
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    /// 两个进程（Linux 的两个窗口）各改各的设置：后保存的不把先保存的改动写回旧值，关着"多窗口共享设置"也一样。
    #[test]
    fn saving_keeps_what_another_process_changed() {
        let file = settings_file("merge");
        let a = Harness::new();
        let b = Harness::new();
        a.core.load_settings(Some(file.clone()));
        b.core.load_settings(Some(file.clone()));
        let set = |h: &Harness, key: &str, value: Value| {
            let mut prefs = lock(&h.core.shared.hub.prefs);
            prefs.settings.set(key, value);
            prefs.save(Platform::current()).unwrap();
        };
        let touch = |seconds: u64| {
            let later = std::time::SystemTime::now() + Duration::from_secs(seconds);
            std::fs::File::options().write(true).open(&file).unwrap().set_modified(later).unwrap();
        };
        set(&a, settings::SHARE_SETTINGS, Value::Bool(false));
        touch(2);
        set(&b, settings::SHARE_SETTINGS, Value::Bool(false));
        touch(4);
        set(&a, settings::BACKGROUND_COLOR, Value::Color(0x112233));
        touch(6);
        set(&b, settings::CACHE_MB, Value::Int(100));
        touch(8);
        // a 再存一项别的：b 改的缓存大小还在。
        set(&a, settings::SAVE_ZOOM_STATE, Value::Bool(true));
        let (on_disk, _) = settings::read_file(&file).unwrap();
        assert_eq!(on_disk.color(settings::BACKGROUND_COLOR), 0x112233);
        assert_eq!(on_disk.int(settings::CACHE_MB), 100);
        assert!(on_disk.bool(settings::SAVE_ZOOM_STATE));
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    /// 旧的 Windows 工具打的包：包里的名字用反斜杠。列得出来，也打得开。
    #[test]
    fn zip_entries_named_with_backslashes_open() {
        let dir = crate::paths::canonical(&folder_with_images("backslash", &["x.png"])).unwrap();
        let book = dir.join("old.zip");
        zip_with_images(&book, &["ch1\\1.png", "ch1\\2.png"]);
        let names: Vec<String> = {
            let file = std::fs::File::open(&book).unwrap();
            let zip = zip::ZipArchive::new(std::io::BufReader::new(file)).unwrap();
            zip.file_names().map(str::to_string).collect()
        };
        assert!(names.iter().all(|n| n.contains('\\')), "测试用的包里应当是反斜杠：{names:?}");
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: book.display().to_string(),
        });
        h.wait_for("第一张", |e| is_ready(e, "1.png"));
        h.core.invoke_action("next_image");
        h.wait_for("第二张", |e| is_ready(e, "2.png"));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 启动时设置文件读不出来（这里去掉读的权限；所在的文件夹照常能写）：用默认值、提示一次，文件不动；
    /// 读到之前保存会报错，不把它照默认值盖掉；之后读得出来了，后台读进来像别的窗口改了设置一样生效，
    /// 保存只写改过的项。
    #[cfg(unix)]
    #[test]
    fn an_unreadable_settings_file_is_never_overwritten_with_defaults() {
        use std::os::unix::fs::PermissionsExt;
        const ORIGINAL: &str = "version = 1\n[browse]\nbackground_color = \"#336699\"\n";
        let lock_up = |file: &Path| {
            std::fs::write(file, ORIGINAL).unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o000)).unwrap();
            // 管理员什么都读得了：这个测试做不了。
            std::fs::read(file).is_err()
        };
        let unlock = |file: &Path| {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
        };

        let file = settings_file("unreadable");
        if !lock_up(&file) {
            return;
        }
        let mut h = Harness::new();
        assert!(matches!(
            h.core.load_settings(Some(file.clone())),
            settings::LoadStatus::Recovered { backup: None, .. }
        ));
        assert_eq!(h.core.settings_path().as_deref(), Some(file.as_path()), "设置文件的位置还在（窗口位置等也靠它）");
        let notice = h.core.settings_notice(Lang::ZhHans).expect("应提示");
        assert!(!notice.contains("broken"), "{notice}");
        // 还读不了：改的设置在这一次运行里生效，但没有存下，界面得到"无法保存"的原因；文件原样。
        let applied = apply(&h.core, &[(settings::CACHE_MB, settings::Input::Text("100".into()))]);
        assert!(applied.save_error.is_some());
        assert_eq!(h.core.setting(settings::CACHE_MB), Some(Value::Int(100)));
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["settings.toml"], "没有改名备份，也没有留下临时文件");
        unlock(&file);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL, "文件没有被动过");
        // 现在读得出来了：后台读进来，文件里的背景色生效，没存下的那一项还在。
        h.clear();
        check_settings_file(&h.core.shared.hub);
        let Event::SettingsReloaded { changed } =
            h.wait_for("读到了", |e| matches!(e, Event::SettingsReloaded { .. }))
        else {
            unreachable!()
        };
        assert_eq!(changed, [settings::BACKGROUND_COLOR]);
        assert_eq!(h.core.ui_settings().background_color, 0x336699);
        assert_eq!(h.core.setting(settings::CACHE_MB), Some(Value::Int(100)));
        // 再改一项：文件里原来的背景色还在，新改的和上次没存下的都写下了。
        let applied = apply(&h.core, &[(settings::SAVE_ZOOM_STATE, settings::Input::Bool(true))]);
        assert_eq!(applied.save_error, None);
        let (on_disk, _) = settings::read_file(&file).unwrap();
        assert_eq!(on_disk.color(settings::BACKGROUND_COLOR), 0x336699);
        assert!(on_disk.bool(settings::SAVE_ZOOM_STATE));
        assert_eq!(on_disk.int(settings::CACHE_MB), 100);
        let _ = std::fs::remove_dir_all(file.parent().unwrap());

        // 后台还没来得及读、用户先点了"确定"：保存时先读一次，同样不盖掉文件里原来的设置。
        let file = settings_file("unreadable-save");
        assert!(lock_up(&file));
        let h = Harness::new();
        h.core.load_settings(Some(file.clone()));
        unlock(&file);
        let applied = apply(&h.core, &[(settings::SAVE_ZOOM_STATE, settings::Input::Bool(true))]);
        assert_eq!(applied.save_error, None);
        let (on_disk, _) = settings::read_file(&file).unwrap();
        assert_eq!(on_disk.color(settings::BACKGROUND_COLOR), 0x336699);
        assert!(on_disk.bool(settings::SAVE_ZOOM_STATE));
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    /// 被安全上限拦下的图片不是直接报"解码失败"，而是让界面问用户；选"仍然打开"就对这个文件解除上限、
    /// 重新读；选"取消"后再翻到它不再问。
    #[test]
    fn a_file_stopped_by_a_limit_can_be_opened_anyway() {
        use std::io::Write;
        let dir = crate::paths::canonical(&folder_with_images("limit", &["a.png"])).unwrap();
        // 解压后 65 MB 的 SVGZ（上限 64 MB）：一张 4x4 的红色方块，后面跟一大段注释。压缩后只有几十 KB。
        let big = dir.join("b.svgz");
        {
            let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(&big).unwrap(), flate2::Compression::fast());
            gz.write_all(br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/><!--"#).unwrap();
            gz.write_all(&vec![b' '; 65 << 20]).unwrap();
            gz.write_all(b"--></svg>").unwrap();
            gz.finish().unwrap();
        }
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 2, .. }));
        // 第一次翻到：问。
        h.clear();
        h.core.invoke_action("next_image");
        let blocked = h.wait_for("被拦下", |e| matches!(e, Event::ImageBlocked { .. } | Event::ImageFailed { .. }));
        assert!(matches!(&blocked, Event::ImageBlocked { path, .. } if path == &big), "{blocked:?}");
        // 取消：再翻到它不再问，按解码失败提示。
        h.core.keep_limits(&big);
        h.core.invoke_action("prev_image");
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.clear();
        h.core.invoke_action("next_image");
        let failed = h.wait_for("不再问", |e| matches!(e, Event::ImageBlocked { .. } | Event::ImageFailed { .. }));
        assert!(matches!(failed, Event::ImageFailed { .. }), "{failed:?}");
        // 仍然打开：解除上限，重新读，显示出来。
        h.clear();
        h.core.open_without_limits(&big);
        let Event::ImageReady { image, .. } = h.wait_for("打开了", |e| is_ready(e, "b.svgz")) else {
            unreachable!()
        };
        assert_eq!((image.width, image.height), (4, 4));
        // 别的文件照旧有上限：只对这一个文件解除。
        assert!(decode::is_trusted(&big) && !decode::is_trusted(&dir.join("a.png")));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 已经不设上限还报超限的是普通的解码失败：不再问（不然"仍然打开"之后还是同一个询问，问个没完）。
    #[test]
    fn a_trusted_file_that_still_fails_is_a_plain_failure() {
        let corrupt = decode::DecodeError::Corrupt(crate::i18n::Message::raw("坏了"));
        let limits = decode::DecodeError::Image(image::ImageError::Limits(
            image::error::LimitError::from_kind(image::error::LimitErrorKind::DimensionError),
        ));
        assert!(stopped_by_limit(false, true, &corrupt), "解码器记了一笔");
        assert!(stopped_by_limit(false, false, &limits), "错误本身是超限");
        assert!(!stopped_by_limit(false, false, &corrupt));
        assert!(!stopped_by_limit(true, true, &corrupt));
        assert!(!stopped_by_limit(true, false, &limits));
    }

    /// 按住翻页键一路翻过去：路过的图片不解码（各排过一个任务，轮到时已经用不着了），停下的那一张不用等它们。
    #[test]
    fn images_passed_over_are_not_decoded() {
        let names: Vec<String> = (0..12).map(|i| format!("{i:02}.png")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let dir = crate::paths::canonical(&folder_with_images("passed-over", &names)).unwrap();
        let mut h = Harness::new();
        apply(&h.core, &[(settings::PREFETCH, settings::Input::Bool(false))]);
        h.core.dispatch(Command::Open {
            path: dir.join("00.png").display().to_string(),
        });
        h.wait_for("00.png", |e| is_ready(e, "00.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 12, .. }));
        // 后台线程都占着的时候翻 10 张：10 个解码任务排着队。
        let gate = Gate::block_all(&h.core);
        for _ in 0..10 {
            h.core.dispatch(Command::Navigate(Navigation::Next));
        }
        gate.open();
        h.wait_for("10.png", |e| is_ready(e, "10.png"));
        eventually("排着的任务都轮到过了", || lock(&h.core.shared.state).in_flight.is_empty());
        for i in 1..10 {
            assert!(!in_cache(&h, &dir.join(format!("{i:02}.png"))), "路过的 {i:02}.png 不该解码");
        }
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 先当预加载排着队的图片成了当前图片：按当前图片的优先级解，不排在扫描、别的预加载后面。
    #[test]
    fn a_queued_prefetch_moves_ahead_when_it_becomes_current() {
        let dir = crate::paths::canonical(&folder_with_images(
            "promote",
            &["a.png", "b.png", "c.png", "d.png"],
        ))
        .unwrap();
        let (b, c, d) = (dir.join("b.png"), dir.join("c.png"), dir.join("d.png"));
        let mut h = Harness::new();
        // 只往后预读 2 张：在 a 时读 b、c，翻到 b 时 d 才进预读范围。
        apply(&h.core, &[(settings::PREFETCH_COUNT, settings::Input::Text("2".into()))]);
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 4, .. }));
        eventually("b、c 预加载好", || in_cache(&h, &b) && in_cache(&h, &c));
        let first = Gate::block_all(&h.core);
        // 翻到 b（缓存里有，马上显示）：d 进了预加载范围，线程都占着，排队。
        h.core.dispatch(Command::Navigate(Navigation::Next));
        h.wait_for("b.png", |e| is_ready(e, "b.png"));
        assert!(
            lock(&h.core.shared.state)
                .in_flight
                .get(&d)
                .is_some_and(|flight| !flight.started && flight.priority == Priority::Prefetch)
        );
        // 再排上一批"扫描"优先级的任务（第二道闸）：比预加载先，比当前图片后。
        let second = Gate::block(&h.core, Priority::Scan, h.core.shared.hub.pool.threads());
        h.core.dispatch(Command::Navigate(Navigation::Jump(3)));
        first.open();
        // 第二道闸还关着也能等到 d：它是按当前图片的优先级解的（照预加载排的话在第二道闸后面，等不到）。
        h.wait_for("d.png", |e| is_ready(e, "d.png"));
        second.open();
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 正在读的文件被换掉、重新请求了：先开始的那一次读完得再晚，结果也不用（旧的不盖住新的）。
    #[cfg(unix)]
    #[test]
    fn a_decode_that_was_superseded_is_thrown_away() {
        use std::io::Write;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("anyview-api-{}-superseded", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = crate::paths::canonical(&dir).unwrap().join("a.png");
        // 拿管道当图片文件：读它的解码任务停在"读"上，这边写进东西它才往下走。
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let mut h = Harness::new();
        apply(
            &h.core,
            &[
                (settings::FAST_PREVIEW, settings::Input::Bool(false)),
                (settings::PREFETCH, settings::Input::Bool(false)),
            ],
        );
        h.core.dispatch(Command::Open {
            path: path.display().to_string(),
        });
        // 等解码任务打开了管道（没有人在读时，不等待地打开写端会失败）。
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut pipe = loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
            {
                Ok(pipe) => break pipe,
                Err(_) => {
                    assert!(Instant::now() < deadline, "解码任务没有来读");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        // 文件换成一张真的图片，重新请求（借"仍然打开"：它让正在读的那一次作废、重新读）。
        std::fs::remove_file(&path).unwrap();
        image::RgbImage::from_pixel(10, 7, image::Rgb([1, 2, 3])).save(&path).unwrap();
        h.core.open_without_limits(&path);
        let Event::ImageReady { image, .. } = h.wait_for("新的", |e| is_ready(e, "a.png")) else {
            unreachable!()
        };
        assert_eq!((image.width, image.height), (10, 7));
        let shown = Arc::clone(&image);
        h.clear();
        // 现在让先开始的那一次读完。
        let mut old = Vec::new();
        image::RgbImage::from_pixel(8, 6, image::Rgb([9, 9, 9]))
            .write_to(&mut std::io::Cursor::new(&mut old), image::ImageFormat::Png)
            .unwrap();
        pipe.write_all(&old).unwrap();
        drop(pipe);
        std::thread::sleep(Duration::from_millis(500));
        let later: Vec<Event> = h.core.drain_events();
        assert!(
            !later.iter().any(|e| matches!(
                e,
                Event::ImageReady { .. } | Event::ImageUpdated { .. } | Event::ImageFailed { .. }
            )),
            "作废的那一次不该再送东西：{later:?}"
        );
        let cached = lock(&h.core.shared.hub.cache).get(&path, Effects::NONE).unwrap();
        assert!(Arc::ptr_eq(&cached, &shown), "缓存里还是新的那一份");
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页合成一组（复制、缩放两整页的像素）在后台做：翻页的调用马上返回，不在调用的线程上合成。
    #[test]
    fn spreads_are_composed_in_the_background() {
        let dir = folder_with_pages("compose", &[10, 60, 110, 160]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 4, .. }));
        h.core.invoke_action("double_page_ltr");
        wait_shown(&mut h, "a+b", (12, 10, 60));
        eventually("c、d 预加载好", || in_cache(&h, &dir.join("c.png")) && in_cache(&h, &dir.join("d.png")));
        let gate = Gate::block_all(&h.core);
        h.clear();
        h.core.invoke_action("next_image");
        // 调用已经返回：换到了 c；两页都在缓存里，但合成要等后台（线程都占着，所以还没有）。
        let events = h.core.drain_events();
        assert!(events.iter().any(|e| matches!(e, Event::Current { .. })), "{events:?}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::ImageReady { .. } | Event::ImageUpdated { .. })),
            "合成不该在调用的线程上做"
        );
        gate.open();
        wait_shown(&mut h, "c+d", (12, 110, 160));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页时列表重新排了序（序号全变了）：翻页从这一组现在在列表里的位置算，不按排序之前的序号。
    #[test]
    fn two_page_navigation_follows_the_list_after_sorting() {
        let dir = folder_with_pages("spread-sort", &[10, 40, 70, 100, 130, 160]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 6, .. }));
        h.core.invoke_action("double_page_ltr");
        h.wait_for("a | b", |e| matches!(e, Event::Current { name, .. } if name == "a.png | b.png"));
        wait_shown(&mut h, "a+b", (12, 10, 40));
        // 降序：f e d c b a。显示着的 a、b 到了列表末尾（b 在 a 前面）。
        h.core.invoke_action("sort_desc");
        h.wait_for("排序", |e| matches!(e, Event::Collection { index: Some(5), .. }));
        // 往回：这一组现在是第 5、6 张（b、a），前面那一组是 d、c。按排序之前的序号（第 1、2 张）算的话，
        // 会当成已经在开头。
        h.clear();
        h.core.invoke_action("prev_image");
        wait_shown(&mut h, "d+c", (12, 100, 70));
        h.clear();
        h.core.invoke_action("next_image");
        wait_shown(&mut h, "b+a", (12, 40, 10));
        // 这一组在末尾：再往后是"到达末尾"（默认回到首张 f）。
        h.clear();
        h.core.invoke_action("next_image");
        let Event::Current { path, .. } = h.wait_for("下一组", |e| matches!(e, Event::Current { .. })) else {
            unreachable!()
        };
        assert_eq!(path.file_name().unwrap(), "f.png");
        wait_shown(&mut h, "f+e", (12, 160, 130));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页时当前图片不在列表里（扩展名不认识、按内容认出来的图片）：与单页一样，往后翻到列表的第一张。
    #[test]
    fn two_page_navigation_from_an_unlisted_image_starts_at_the_first() {
        let dir = folder_with_pages("spread-unlisted", &[10, 60, 110]);
        std::fs::copy(dir.join("a.png"), dir.join("x.dat")).unwrap();
        let mut h = Harness::new();
        h.core.invoke_action("double_page_ltr");
        h.core.dispatch(Command::Open {
            path: dir.join("x.dat").display().to_string(),
        });
        h.wait_for("扫描", |e| matches!(e, Event::Collection { index: None, total: 3, .. }));
        wait_shown(&mut h, "x 单独", (6, 10, 10));
        h.clear();
        h.core.invoke_action("next_image");
        let Event::Current { path, index, .. } = h.wait_for("第一张", |e| matches!(e, Event::Current { .. })) else {
            unreachable!()
        };
        assert_eq!((path.file_name().unwrap().to_str(), index), (Some("a.png"), Some(0)));
        wait_shown(&mut h, "a+b", (12, 10, 60));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 上一次重扫还没完时不再排一个：文件多、磁盘慢，一次扫描比检查的间隔还长时，扫描不会越积越多。
    #[test]
    fn folder_rescans_do_not_pile_up() {
        let dir = crate::paths::canonical(&folder_with_images("rescans", &["a.png", "b.png"])).unwrap();
        let mut h = Harness::with_watch_interval(Duration::from_secs(3600));
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 2, .. }));
        eventually("b 预加载好", || in_cache(&h, &dir.join("b.png")));
        let shared = Arc::clone(&h.core.shared);
        let gate = Gate::block_all(&h.core);
        assert_eq!(shared.hub.pool.queued(), 0);
        let stamp = modified_time(&dir);
        rescan_folder(&shared, dir.clone(), stamp);
        rescan_folder(&shared, dir.clone(), stamp);
        rescan_folder(&shared, dir.clone(), stamp);
        assert_eq!(shared.hub.pool.queued(), 1, "只排一个");
        gate.open();
        // 扫完了可以再扫。
        eventually("重扫结束", || lock(&shared.state).rescanning.is_none());
        std::fs::write(dir.join("c.png"), std::fs::read(dir.join("a.png")).unwrap()).unwrap();
        rescan_folder(&shared, dir.clone(), modified_time(&dir));
        h.wait_for("新的列表", |e| matches!(e, Event::Collection { total: 3, .. }));
        drop(shared);
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页成组时同一代次有两个 `Current`（第二个把标题换成两页的名字），界面每个都问一次：翻页音效只响一次。
    #[test]
    fn the_page_sound_plays_once_per_turn() {
        let dir = folder_with_pages("turn-sound", &[10, 60, 110, 160]);
        let mut h = Harness::new();
        lock(&h.core.shared.hub.prefs)
            .settings
            .set(settings::PAGE_SOUND, Value::Bool(true));
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 4, .. }));
        h.core.invoke_action("double_page_ltr");
        wait_shown(&mut h, "a+b", (12, 10, 60));
        h.clear();
        h.core.invoke_action("next_image");
        let Event::Current { generation, .. } =
            h.wait_for("c", |e| matches!(e, Event::Current { name, .. } if name == "c.png"))
        else {
            unreachable!()
        };
        assert!(h.core.transition_for(generation).is_some_and(|t| t.sound && t.forward));
        h.wait_for("c | d", |e| {
            matches!(e, Event::Current { generation: g, name, .. } if *g == generation && name == "c.png | d.png")
        });
        assert!(h.core.transition_for(generation).is_none_or(|t| !t.sound), "第二个 Current 不再放音效");
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 压缩包里双页成组：标题仍然带着压缩包的名字。
    #[test]
    fn two_pages_in_an_archive_keep_the_archive_name_in_the_title() {
        let dir = std::env::temp_dir().join(format!("anyview-api-{}-spread-zip", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let zip = dir.join("book.zip");
        zip_with_sized_images(&zip, &["p1.png", "p2.png"], 6, 8);
        let mut h = Harness::new();
        h.core.invoke_action("double_page_ltr");
        h.core.dispatch(Command::Open {
            path: zip.display().to_string(),
        });
        h.wait_for("第一页", |e| matches!(e, Event::Current { name, .. } if name == "book.zip › p1.png"));
        h.wait_for("两页", |e| matches!(e, Event::Current { name, .. } if name == "book.zip › p1.png | p2.png"));
        h.core.dispatch(Command::Close);
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 界面只在同一代次、同样尺寸已经排过版时才收到"只换内容"；这一代次还没显示过、尺寸变了、
    /// 报过解码失败的，都按新图片显示（不然界面不会重新排版）。
    #[test]
    fn an_update_is_only_sent_for_an_image_the_ui_has_laid_out() {
        let h = Harness::new();
        let shared = &h.core.shared;
        let push = |width: u32, height: u32| -> &'static str {
            let info = crate::decode::ImageInfo {
                format: "PNG".into(),
                pixel_format: "RGBA32".into(),
                file_size: 0,
                icc_profile: None,
                modified: None,
                has_alpha: false,
                decode_ms: 0.0,
                shooting: None,
                gif_background: None,
            };
            let image = Arc::new(DecodedImage::new(
                width,
                height,
                vec![0; (width * height * 4) as usize],
                info,
            ));
            show(shared, &mut lock(&shared.state), PathBuf::from("x.png"), image, Vec::new());
            match h.core.drain_events().as_slice() {
                [Event::ImageReady { .. }] => "新图片",
                [Event::ImageUpdated { .. }] => "只换内容",
                other => panic!("{other:?}"),
            }
        };
        lock(&shared.state).generation = 5;
        assert_eq!(push(8, 6), "新图片");
        assert_eq!(push(8, 6), "只换内容");
        assert_eq!(push(9, 6), "新图片", "尺寸变了");
        lock(&shared.state).generation = 6;
        assert_eq!(push(9, 6), "新图片", "新的代次还没显示过");
        lock(&shared.state).shown = Some((6, 0, 0));
        assert_eq!(push(9, 6), "新图片", "这一代次报过解码失败");
    }

    /// 双页时和当前图片一起显示的另一页被删掉了：这一组重新成（当前图片和它现在的下一张）。
    #[test]
    fn two_page_view_regroups_when_the_other_page_is_deleted() {
        let dir = folder_with_pages("spread-gone", &[10, 60, 110, 160]);
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 4, .. }));
        h.core.invoke_action("double_page_ltr");
        wait_shown(&mut h, "a+b", (12, 10, 60));
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::remove_file(dir.join("b.png")).unwrap();
        wait_shown(&mut h, "a+c", (12, 10, 110));
        h.wait_for("新的列表", |e| matches!(e, Event::Collection { index: Some(0), total: 3, .. }));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 加效果的任务做完时，缓存里这张图片的原图已经换了一份（文件改了重新读过、别的窗口又读了一遍）：
    /// 按旧的那一份做出来的不放进缓存、不显示；还等着显示的，按新的那一份再做。
    #[test]
    fn an_effect_applied_to_a_replaced_image_is_thrown_away() {
        let dir = crate::paths::canonical(&folder_with_images("stale-effect", &["a.png"])).unwrap();
        let a = dir.join("a.png");
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: a.display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 1, .. }));
        let gate = Gate::block_all(&h.core);
        h.clear();
        // 加效果的任务排着队的时候，缓存里的原图换成了另一份（9×9；文件的时间、大小照原来的记，
        // 后台核对时不会当成文件改过）。
        h.core.invoke_action("invert");
        let inverted = lock(&h.core.shared.state).effects;
        {
            let mut cache = lock(&h.core.shared.hub.cache);
            let old = cache.get(&a, Effects::NONE).unwrap();
            let newer = DecodedImage::new(9, 9, vec![0; 9 * 9 * 4], old.info.clone());
            cache.insert(a.clone(), Effects::NONE, Arc::new(newer));
        }
        gate.open();
        let shown = h.wait_for("按新的那一份做的", |e| {
            matches!(e, Event::ImageReady { .. } | Event::ImageUpdated { .. })
        });
        assert!(
            matches!(&shown, Event::ImageReady { image, .. } if (image.width, image.height) == (9, 9)),
            "旧的那一份（8×6）不该再送出来：{shown:?}"
        );
        let cached = lock(&h.core.shared.hub.cache).get(&a, inverted).unwrap();
        assert_eq!((cached.width, cached.height), (9, 9));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页时正等着另一页，列表变了（这里是换了排序），挨着当前图片的换成了别的图片：按新的列表成组显示。
    /// 原来等的那一页读好了不会再来叫，不重新看的话画面就一直不出来。
    #[test]
    fn two_page_view_regroups_when_the_list_changes_before_it_is_shown() {
        let dir = folder_with_pages("spread-pending", &[10, 40, 70, 100, 130, 160]);
        let mut h = Harness::new();
        // 往后预读 2 张（双页时从这一组的后一页往外数）：a、b 这一组的预读范围到 d。
        apply(&h.core, &[(settings::PREFETCH_COUNT, settings::Input::Text("2".into()))]);
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 6, .. }));
        h.core.invoke_action("double_page_ltr");
        wait_shown(&mut h, "a+b", (12, 10, 40));
        // a 的预加载范围到 d：c、d 在缓存里，e 不在。
        eventually("c、d 预加载好", || in_cache(&h, &dir.join("c.png")) && in_cache(&h, &dir.join("d.png")));
        assert!(!in_cache(&h, &dir.join("e.png")));
        let gate = Gate::block_all(&h.core);
        // 跳到 d：d 在缓存里，另一页 e 要读（排着队）。
        h.core.dispatch(Command::Navigate(Navigation::Jump(3)));
        // 降序：f e d c b a。d 后面那张现在是 c（在缓存里）。
        h.core.invoke_action("sort_desc");
        gate.open();
        wait_shown(&mut h, "d+c", (12, 100, 70));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页时另一页读不了（比如正好读到存了一半的文件）：先单独显示当前这一张；那个文件之后变了
    /// （存好了）就重新成组，不用用户翻走再翻回来。
    #[test]
    fn two_page_view_pairs_up_once_the_other_page_becomes_readable() {
        let dir = folder_with_pages("spread-heals", &[10, 60, 110, 160]);
        std::fs::write(dir.join("b.png"), b"not an image yet").unwrap();
        let mut h = Harness::new();
        h.core.invoke_action("double_page_ltr");
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        wait_shown(&mut h, "a 单独", (6, 10, 10));
        std::thread::sleep(Duration::from_millis(1100));
        image::RgbImage::from_pixel(6, 8, image::Rgb([60, 0, 0]))
            .save(dir.join("b.png"))
            .unwrap();
        wait_shown(&mut h, "a+b", (12, 10, 60));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 上次没读出来的另一页，再翻到这一组时重新试：文件已经好了的话，第一次显示就是成组的两页。
    #[test]
    fn two_page_view_tries_an_unreadable_page_again_on_the_next_visit() {
        let dir = folder_with_pages("spread-retry", &[10, 60, 110, 160]);
        std::fs::write(dir.join("b.png"), b"not an image yet").unwrap();
        // 整轮检查隔一小时：下面成组靠的是翻回来时重新试，不是后台发现文件变了。
        let mut h = Harness::with_watch_interval(Duration::from_secs(3600));
        h.core.invoke_action("double_page_ltr");
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        wait_shown(&mut h, "a 单独", (6, 10, 10));
        h.core.dispatch(Command::Navigate(Navigation::Jump(2)));
        wait_shown(&mut h, "c+d", (12, 110, 160));
        image::RgbImage::from_pixel(6, 8, image::Rgb([60, 0, 0]))
            .save(dir.join("b.png"))
            .unwrap();
        h.clear();
        h.core.dispatch(Command::Navigate(Navigation::Jump(0)));
        let first = h.wait_for("翻回来", |e| matches!(e, Event::ImageReady { .. } | Event::ImageUpdated { .. }));
        let Event::ImageReady { image, .. } = &first else {
            panic!("{first:?}")
        };
        assert_eq!(corners(image), (12, 10, 60), "第一次显示就是两页");
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 显示着的文件一直在变：因为文件改了重新显示以后，不马上再核对（那样解多快就重读多快，画面闪个不停），
    /// 等下一轮检查。
    #[test]
    fn a_file_that_keeps_changing_is_reloaded_once_per_round() {
        let dir = crate::paths::canonical(&folder_with_images("keeps-changing", &["a.png", "b.png"])).unwrap();
        let a = dir.join("a.png");
        // 整轮检查隔一小时：测试期间不会有"下一轮"。
        let mut h = Harness::with_watch_interval(Duration::from_secs(3600));
        h.core.dispatch(Command::Open {
            path: a.display().to_string(),
        });
        h.wait_for("a.png", |e| is_ready(e, "a.png"));
        h.wait_for("扫描", |e| matches!(e, Event::Collection { total: 2, .. }));
        h.core.invoke_action("next_image");
        h.wait_for("b.png", |e| is_ready(e, "b.png"));
        std::thread::sleep(Duration::from_millis(1100));
        image::RgbImage::from_pixel(10, 7, image::Rgb([1, 2, 3])).save(&a).unwrap();
        h.core.invoke_action("prev_image");
        // 缓存里旧的先出来，核对后重新读（这一次是因为文件改了重新显示的）。
        h.wait_for("改过的 a.png", |e| matches!(e, Event::ImageReady { image, .. } if image.width == 10));
        // 文件又变了；这时再显示一次（换个效果）会叫后台核对，但这一代次不再重读。
        std::thread::sleep(Duration::from_millis(1100));
        image::RgbImage::from_pixel(12, 9, image::Rgb([4, 5, 6])).save(&a).unwrap();
        h.clear();
        h.core.invoke_action("invert");
        h.wait_for("换了效果", |e| matches!(e, Event::ImageUpdated { image, .. } if image.width == 10));
        std::thread::sleep(Duration::from_millis(400));
        let later = h.core.drain_events();
        assert!(
            !later.iter().chain(&h.seen).any(|e| matches!(e, Event::Current { .. } | Event::ImageReady { .. })),
            "不到下一轮不该又重读：{later:?}"
        );
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 双页时往回跳 10 张和往后跳一样，从目标往后成组：和原来的分组对得上，不错开一页。
    #[test]
    fn two_page_jumps_of_ten_group_forward_from_the_target() {
        let reds: Vec<u8> = (0..14).map(|i| 10 + i * 10).collect();
        let dir = folder_with_pages("spread-ten", &reds);
        let mut h = Harness::new();
        h.core.invoke_action("double_page_ltr");
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        wait_shown(&mut h, "第 1、2 张", (12, 10, 20));
        h.core.dispatch(Command::Navigate(Navigation::Jump(12)));
        wait_shown(&mut h, "第 13、14 张", (12, 130, 140));
        h.core.dispatch(Command::Navigate(Navigation::Back10));
        wait_shown(&mut h, "第 3、4 张", (12, 30, 40));
        let turn = lock(&h.core.shared.state).turn.as_ref().map(|t| t.forward);
        assert_eq!(turn, Some(false), "往回跳是往前翻");
        h.core.dispatch(Command::Navigate(Navigation::Forward10));
        wait_shown(&mut h, "第 14 张单独（后面没有了）", (6, 140, 140));
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_background_job_that_panics_is_reported_not_lost() {
        assert_eq!(no_panic(|| 7), Some(7));
        assert_eq!(no_panic(|| -> u8 { panic!("测试") }), None);
    }

    /// 测试用：解码完、交回结果之前对某个文件做的事（模拟“读的期间文件被改了”）。按文件路径登记，只做一次。
    type AfterDecode = (PathBuf, Box<dyn FnOnce() + Send>);
    static AFTER_DECODE: Mutex<Vec<AfterDecode>> = Mutex::new(Vec::new());

    pub(super) fn after_decode(path: &Path) {
        let hook = {
            let mut hooks = lock(&AFTER_DECODE);
            let at = hooks.iter().position(|(p, _)| p == path);
            at.map(|at| hooks.remove(at).1)
        };
        if let Some(hook) = hook {
            hook();
        }
    }

    /// 预读时读到的是旧的（坏的）文件、读完还没交回结果时文件已经存好了：这次的失败不算数，按新的文件再读，
    /// 双页时直接成组显示（以前会先单独显示当前这一页；Mac 上一键测试偶然失败过一次， 查明）。
    #[test]
    fn a_page_saved_while_it_was_being_read_is_read_again() {
        let dir = crate::paths::canonical(&folder_with_pages("reread", &[10, 60])).unwrap();
        let b = dir.join("b.png");
        std::fs::write(&b, b"not an image yet").unwrap();
        let fixed = b.clone();
        lock(&AFTER_DECODE).push((
            b.clone(),
            Box::new(move || {
                image::RgbImage::from_pixel(6, 8, image::Rgb([60, 0, 0]))
                    .save(&fixed)
                    .unwrap();
            }),
        ));
        let mut h = Harness::with_watch_interval(Duration::from_secs(3600));
        h.core.invoke_action("double_page_ltr");
        h.core.dispatch(Command::Open {
            path: dir.join("a.png").display().to_string(),
        });
        let first = h.wait_for("第一次显示", |e| matches!(e, Event::ImageReady { .. } | Event::ImageUpdated { .. }));
        let Event::ImageReady { image, .. } = &first else {
            panic!("{first:?}")
        };
        assert_eq!(corners(image), (12, 10, 60), "读的期间存好了的另一页重新读，第一次显示就是两页");
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 生成信息：带生成信息的图片上“生成信息”可用、打开面板，图片信息里多一行；没有的图片上不可用。
    #[test]
    fn generation_info_follows_the_current_image() {
        let dir = std::env::temp_dir().join(format!("anyview-api-{}-generation", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in crate::generation::samples::all() {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        let mut h = Harness::new();
        h.core.dispatch(Command::Open {
            path: dir.join("text.png").display().to_string(),
        });
        h.wait_for("text.png", |e| is_ready(e, "text.png"));
        let info = h.core.generation_info().expect("text.png 带生成信息");
        assert_eq!(info.prompt, crate::generation::samples::PROMPT);
        assert!(h.core.action_state("show_generation_info").enabled);
        assert_eq!(h.core.invoke_action("show_generation_info"), ActionOutcome::Ui(UiRequest::ShowGenerationInfo));
        let lines = h.core.info_lines(Lang::ZhHans);
        assert!(
            lines.iter().any(|l| l == "AI 生成: Stable Diffusion WebUI (按 P 看提示词)"),
            "{lines:?}"
        );
        assert!(!h.core.generation_pending(), "读好了");
        // 换到一张还没读好的：这时面板不换内容（不先闪一下“没有生成信息”），读好了再换。
        let gate = Gate::block_all(&h.core);
        lock(&h.core.shared.hub.cache).clear();
        h.core.dispatch(Command::Open {
            path: dir.join("plain.png").display().to_string(),
        });
        assert!(h.core.generation_pending(), "还在读");
        gate.open();
        h.wait_for("plain.png", |e| is_ready(e, "plain.png"));
        assert!(!h.core.generation_pending());
        assert!(h.core.generation_info().is_none());
        assert!(!h.core.action_state("show_generation_info").enabled);
        assert_eq!(h.core.invoke_action("show_generation_info"), ActionOutcome::Unavailable);
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 预读用的：n 张图片的列表，看着第 index 张，设置都是默认的。
    fn preload_state(n: usize, index: usize) -> State {
        let entries: Vec<Entry> = (0..n)
            .map(|i| Entry {
                path: PathBuf::from(format!("/preload/{i}.png")),
                name: format!("{i}.png"),
                modified: None,
                size: 0,
            })
            .collect();
        State {
            current: Some(entries[index].path.clone()),
            index: Some(index),
            entries,
            scanned: true,
            ..State::default()
        }
    }

    fn stems(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn pixels(bytes: usize) -> Arc<DecodedImage> {
        Arc::new(DecodedImage::new(
            1,
            1,
            vec![0; bytes],
            crate::decode::ImageInfo {
                format: "PNG".into(),
                pixel_format: "RGBA32".into(),
                file_size: 0,
                icc_profile: None,
                modified: None,
                has_alpha: true,
                decode_ms: 0.0,
                shooting: None,
                gif_background: None,
            },
        ))
    }

    /// 预读的顺序：先近后远，往翻的方向读设好的张数，反方向一张；双页时反方向两张；不越过头尾。
    #[test]
    fn preload_reads_ahead_in_the_direction_of_flipping() {
        let mut st = preload_state(20, 10);
        assert_eq!(st.opts.prefetch_count, 5, "默认预读 5 张");
        assert_eq!(stems(&st.window()), ["10", "11", "9", "12", "13", "14", "15"]);
        st.heading_back = true;
        assert_eq!(stems(&st.window()), ["10", "9", "11", "8", "7", "6", "5"]);
        st.opts.prefetch_count = 1;
        assert_eq!(stems(&st.window()), ["10", "9", "11"]);
        st.opts.prefetch = false;
        assert_eq!(stems(&st.window()), ["10"], "关掉预读时只有当前图片");
        let st = preload_state(3, 2);
        assert_eq!(stems(&st.window()), ["2", "1", "0"], "末张往后没有了：往回读");
        let mut st = preload_state(20, 10);
        st.two_page = Some(crate::spread::Direction::Ltr);
        assert_eq!(
            stems(&st.window()),
            ["10", "11", "12", "9", "13", "8", "14", "15", "16"],
            "双页：一起显示的另一页在前；从这一组的两头往外数，往前 5 张、反方向两张"
        );
        st.opts.prefetch_count = 1;
        assert_eq!(stems(&st.window()), ["10", "11", "12", "9", "8"], "往前 1 张也是下一组的");
    }

    /// 往翻的方向已经到头时往另一头读：翻到第一张（算往回翻）、切到上一个文件夹（从它的第一张看起）、
    /// 翻到最后一张。
    #[test]
    fn preload_turns_around_at_the_ends_of_the_list() {
        let mut st = preload_state(20, 0);
        st.heading_back = true;
        assert_eq!(stems(&st.window()), ["0", "1", "2", "3", "4", "5"]);
        let mut st = preload_state(20, 19);
        assert_eq!(stems(&st.window()), ["19", "18", "17", "16", "15", "14"]);
        st.heading_back = true;
        assert_eq!(stems(&st.window()), ["19", "18", "17", "16", "15", "14"]);
        // 双页：第一组（0、1）往回翻过来的。
        let mut st = preload_state(20, 0);
        st.two_page = Some(crate::spread::Direction::Ltr);
        st.heading_back = true;
        assert_eq!(stems(&st.window()), ["0", "1", "2", "3", "4", "5", "6"]);
        let st = preload_state(1, 0);
        assert_eq!(stems(&st.window()), ["0"], "只有一张");
    }

    /// 预读放不下时少读几张：按缓存大小（几个窗口平分）先近后远地加；缓存里已有的按实际大小算。
    #[test]
    fn preload_stops_where_the_cache_is_full() {
        let mut st = preload_state(20, 10);
        let mut cache = ImageCache::new(1000);
        assert_eq!(preload_limit(&st, &cache, 1), None, "还不知道图片多大时不限");
        st.last_bytes = 300;
        assert_eq!(preload_limit(&st, &cache, 1), Some(3), "1000 字节放得下 3 张 300 的");
        st.preload_limit = Some(3);
        assert_eq!(stems(&st.window()), ["10", "11", "9"]);
        cache.insert(st.entries[11].path.clone(), Effects::NONE, pixels(100));
        assert_eq!(preload_limit(&st, &cache, 1), Some(4), "11 只占 100：300 + 100 + 300 + 300");
        cache.protect(2, vec![PathBuf::from("/other.png")], 1);
        assert_eq!(preload_limit(&st, &cache, 1), Some(2), "两个窗口各分 500");
        cache.forget(2);
        st.last_bytes = 5000;
        assert_eq!(preload_limit(&st, &cache, 1), Some(1), "当前图片自己放不下也留着");
        st.effects.invert = true;
        st.last_bytes = 300;
        assert_eq!(
            preload_limit(&st, &cache, 1),
            Some(2),
            "开着反相时没读的每张按两份算，缓存里只有原图的也是：600 + 200，再加 600 就放不下"
        );
        st.effects.invert = false;
        st.last_bytes = 150;
        cache.insert(st.entries[11].path.clone(), Effects { invert: true, ..Effects::NONE }, pixels(500));
        assert_eq!(preload_limit(&st, &cache, 1), Some(3), "缓存里已有的按所有版本加起来：150 + 600 + 150，再加 150 就放不下");
    }

    /// 在真的文件夹里：打开第一张后读好后面 5 张，第 6 张不读；往回翻以后往回读 5 张。
    #[test]
    fn preload_follows_the_flipping_direction_in_a_real_folder() {
        let names: Vec<String> = (0..20).map(|i| format!("{i:02}.png")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let dir = folder_with_images("preload-direction", &refs);
        let mut h = Harness::new();
        let cached = |h: &Harness, i: usize| lock(&h.core.shared.hub.cache).has(&dir.join(&names[i]));
        let wait_cached = |h: &Harness, i: usize| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !cached(h, i) {
                assert!(Instant::now() < deadline, "第 {i} 张没有预读");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        h.core.dispatch(Command::Open {
            path: dir.join(&names[0]).display().to_string(),
        });
        h.wait_for("第一张", |e| is_ready(e, "00.png"));
        for i in 1..=5 {
            wait_cached(&h, i);
        }
        std::thread::sleep(Duration::from_millis(200));
        assert!(!cached(&h, 6), "只往后读 5 张");
        h.core.dispatch(Command::Navigate(Navigation::Last));
        h.wait_for("末张", |e| is_ready(e, "19.png"));
        h.core.dispatch(Command::Navigate(Navigation::Prev));
        h.wait_for("倒数第二张", |e| is_ready(e, "18.png"));
        for i in 13..=17 {
            wait_cached(&h, i);
        }
        std::thread::sleep(Duration::from_millis(200));
        assert!(!cached(&h, 12), "往回翻时往回读 5 张");
        drop(h);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
