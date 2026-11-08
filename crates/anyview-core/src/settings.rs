//! 设置（docs/SETTINGS.md）。
//!
//! 全部设置项登记在 [`DEFS`]：所在的页和分组、控件种类、默认值、在哪些平台显示、是否已经实现、界面文字。
//! 三端的设置窗口都按这份数据生成；读取、保存、校验也都按它进行，界面不重复这些规则。
//!
//! - [`Settings`]：一组取值，每个有值的设置项都有一个值。
//! - [`Draft`]：设置窗口里的草稿。打开窗口时复制当前设置，修改只改草稿；点"确定"后校验，
//!   只把改过的项写回。
//! - 页为空的项是"记住的状态"（排序方式、图片信息开关等）：不在设置窗口里显示，由菜单和按键改变，
//!   同样保存在设置文件里。
//! - 文件格式为 TOML，带版本号；读不出来时备份旧文件、使用默认值。
//!   写文件时保留认不出的键（更新版本写进去的），只覆盖本版本管理的键。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::actions;
use crate::i18n::Lang;
use crate::platform::Platform;

/// 设置文件的格式版本。改变已有键的含义或默认值时加一，并在 [`migrate`] 里迁移旧文件。
pub const VERSION: i64 = 3;

/// 平台掩码。
pub const MAC: u8 = 1;
pub const WIN: u8 = 2;
pub const LINUX: u8 = 4;
pub const ALL: u8 = MAC | WIN | LINUX;
const NONE: u8 = 0;

pub fn platform_mask(platform: Platform) -> u8 {
    match platform {
        Platform::Macos => MAC,
        Platform::Windows => WIN,
        Platform::Linux => LINUX,
    }
}

/// 表里的中英文按界面语言取一句（其他语言查翻译表）。
fn pick(lang: Lang, zh: &'static str, en: &'static str) -> &'static str {
    crate::i18n::translate(lang, zh, en)
}

// ---- 页 ----

/// 设置窗口左侧的分类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    General,
    Browse,
    Image,
    Formats,
    Keyboard,
    Mouse,
    Bookmarks,
    /// 资源管理器上下文菜单：仅在已安装的 Windows 环境下提供，未安装时不显示。
    ContextMenu,
    Misc,
    Language,
}

pub const PAGES: &[Page] = &[
    Page::General,
    Page::Browse,
    Page::Image,
    Page::Formats,
    Page::Keyboard,
    Page::Mouse,
    Page::Bookmarks,
    Page::ContextMenu,
    Page::Misc,
    Page::Language,
];

impl Page {
    pub fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Browse => "browse",
            Self::Image => "image",
            Self::Formats => "formats",
            Self::Keyboard => "keyboard",
            Self::Mouse => "mouse",
            Self::Bookmarks => "bookmarks",
            Self::ContextMenu => "context_menu",
            Self::Misc => "misc",
            Self::Language => "language",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        PAGES.iter().copied().find(|p| p.id() == id)
    }

    /// 左侧列表里的名称。
    pub fn title(self, lang: Lang) -> &'static str {
        match self {
            Self::General => pick(lang, "一般", "General"),
            Self::Browse => pick(lang, "浏览", "Browsing"),
            Self::Image => pick(lang, "图像处理", "Image"),
            Self::Formats => pick(lang, "格式关联", "File Types"),
            Self::Keyboard => pick(lang, "键盘", "Keyboard"),
            Self::Mouse => pick(lang, "鼠标", "Mouse"),
            Self::Bookmarks => pick(lang, "书签", "Bookmarks"),
            Self::ContextMenu => pick(lang, "上下文菜单", "Context Menu"),
            Self::Misc => pick(lang, "杂项", "Other"),
            Self::Language => pick(lang, "语言", "Language"),
        }
    }

    /// 页顶上标题下面的一段说明（键盘、鼠标页）；没有时为空。
    pub fn lead(self, lang: Lang) -> &'static str {
        match self {
            Self::Keyboard => pick(
                lang,
                "自己设的快捷键优先：和默认快捷键用同一个键时，用这里设的。默认的快捷键可以在“键盘快捷键”窗口里查看。",
                "Your own shortcuts come first: when one uses the same key as a default shortcut, yours wins. The defaults are listed in the Keyboard Shortcuts window.",
            ),
            Self::Mouse => pick(
                lang,
                "选了“默认”的按键照常用原来的功能；选了别的，单击这个按键就执行所选的动作。",
                "Buttons set to “Default” keep their usual function; otherwise a click runs the chosen action.",
            ),
            _ => "",
        }
    }
}

// ---- 设置项的定义 ----

/// 下拉框的一个选项。
#[derive(Clone, Copy, Debug)]
pub struct Opt {
    pub id: &'static str,
    pub zh: &'static str,
    pub en: &'static str,
}

const fn opt(id: &'static str, zh: &'static str, en: &'static str) -> Opt {
    Opt { id, zh, en }
}

/// 设置项里的按钮（路径旁的"浏览…"、格式关联页的按钮等）。
#[derive(Clone, Copy, Debug)]
pub struct ButtonDef {
    pub id: &'static str,
    pub zh: &'static str,
    pub en: &'static str,
}

const fn button(id: &'static str, zh: &'static str, en: &'static str) -> ButtonDef {
    ButtonDef { id, zh, en }
}

/// 控件种类。
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// 勾选框。
    Toggle,
    /// 下拉框。
    Choice(&'static [Opt]),
    /// 整数输入框加单位（如缓存大小）。草稿里允许暂时不合法，点"确定"时校验。
    Number {
        min: i64,
        max: i64,
        unit: &'static str,
    },
    /// 颜色：显示当前颜色的"选择…"按钮（打开系统取色器）加一排预设色块。
    Color(&'static [u32]),
    /// 路径输入框加按钮。
    Path(&'static [ButtonDef]),
    /// 一排按钮，点击即执行，不属于草稿。
    Buttons(&'static [ButtonDef]),
    /// 一段说明文字（文字就是标签）。
    Note,
    /// 小标题（文字就是标签）。
    Heading,
    /// 鼠标映射：勾选启用，右边选动作。
    Mapping,
    /// 键盘映射表。
    KeyMappings,
}

impl Kind {
    /// 有没有取值（说明、小标题、按钮没有）。
    pub fn has_value(self) -> bool {
        !matches!(self, Kind::Buttons(_) | Kind::Note | Kind::Heading)
    }
}

/// 默认值。
#[derive(Clone, Copy, Debug)]
pub enum Initial {
    None,
    Bool(bool),
    Choice(&'static str),
    Int(i64),
    Color(u32),
    Text(&'static str),
    /// 系统的"图片"文件夹。
    PicturesFolder,
    Mapping(bool, &'static str),
    KeyMappings(&'static [(bool, &'static str, &'static str)]),
}

/// 一个设置项。
#[derive(Clone, Copy, Debug)]
pub struct Def {
    /// `页.名称`，也是设置文件里的 `[页] 名称`；记住的状态用 `state.名称`。
    pub key: &'static str,
    /// 所在的页；为空表示不在设置窗口里显示（记住的状态）。
    pub page: Option<Page>,
    /// 缩进层级（用于从属设置项缩进）。
    pub indent: u8,
    pub kind: Kind,
    pub initial: Initial,
    /// 标签。`{key:动作}` 替换为该动作在所在平台的快捷键（如"总在最前 (⇧⌘A)"）。
    pub zh: &'static str,
    pub en: &'static str,
    /// 某些平台改用的标签：(平台掩码, 中文, 英文)。
    pub alt: Option<(u8, &'static str, &'static str)>,
    /// 标题下面的一行灰色说明：(中文, 英文)。
    pub detail: Option<(&'static str, &'static str)>,
    /// 某些平台改用的说明：(平台掩码, 中文, 英文)。
    pub detail_alt: Option<(u8, &'static str, &'static str)>,
    /// 标题后面灰色显示的快捷键：这个动作在所在平台的第一个快捷键（如"总在最前"后面的 ⇧⌘A）。
    pub shortcut: Option<&'static str>,
    /// 开关在界面上反过来说：设置文件里存的是"不显示……"，界面上是"显示……"。
    pub invert: bool,
    /// 在哪些平台显示。
    pub platforms: u8,
    /// 在哪些平台已经实现。尚未实现的项灰显而不隐藏（与菜单相同）。
    pub ready: u8,
    /// 只有另一项开关（或映射的勾选）为指定值时才可用，如"双击最大化"要在"鼠标左键"未启用时。
    pub requires: Option<(&'static str, bool)>,
    /// 和“程序是不是用安装程序装的”的关系（只在 Windows 上起作用）。
    pub gate: Gate,
}

/// 设置项和“程序是不是用安装程序装的”的关系（Windows）：资源管理器的右键菜单、缩略图、格式关联
/// 都是安装时向系统登记的，没装的版本（测试用的、以后的免安装版）这些用不了。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// 没有关系。
    Any,
    /// 装好后才显示（上下文菜单页）。
    ShownWhenInstalled,
    /// 装好后才可用，没装时灰显（格式关联的两个按钮）。
    EnabledWhenInstalled,
    /// 没装时才显示（说明为什么按钮是灰的）。
    ShownWhenNotInstalled,
}

/// Windows：程序是不是用安装程序装的（它所在的文件夹就是登记在系统里的安装位置）。界面启动时告诉核心。
static WINDOWS_INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Windows 版界面启动时调用：程序是不是用安装程序装的。
pub fn set_windows_installed(installed: bool) {
    WINDOWS_INSTALLED.store(installed, std::sync::atomic::Ordering::Relaxed);
}

pub fn windows_installed() -> bool {
    WINDOWS_INSTALLED.load(std::sync::atomic::Ordering::Relaxed)
}

const fn item(
    key: &'static str,
    page: Option<Page>,
    kind: Kind,
    initial: Initial,
    zh: &'static str,
    en: &'static str,
) -> Def {
    Def {
        key,
        page,
        indent: 0,
        kind,
        initial,
        zh,
        en,
        alt: None,
        detail: None,
        detail_alt: None,
        shortcut: None,
        invert: false,
        platforms: ALL,
        ready: NONE,
        requires: None,
        gate: Gate::Any,
    }
}

impl Def {
    const fn indent(mut self, indent: u8) -> Self {
        self.indent = indent;
        self
    }

    const fn alt(mut self, platforms: u8, zh: &'static str, en: &'static str) -> Self {
        self.alt = Some((platforms, zh, en));
        self
    }

    const fn detail(mut self, zh: &'static str, en: &'static str) -> Self {
        self.detail = Some((zh, en));
        self
    }

    const fn detail_alt(mut self, platforms: u8, zh: &'static str, en: &'static str) -> Self {
        self.detail_alt = Some((platforms, zh, en));
        self
    }

    const fn shortcut(mut self, action: &'static str) -> Self {
        self.shortcut = Some(action);
        self
    }

    const fn invert(mut self) -> Self {
        self.invert = true;
        self
    }

    const fn only(mut self, platforms: u8) -> Self {
        self.platforms = platforms;
        self
    }

    const fn ready(mut self, platforms: u8) -> Self {
        self.ready = platforms;
        self
    }

    const fn requires(mut self, key: &'static str, value: bool) -> Self {
        self.requires = Some((key, value));
        self
    }

    const fn gate(mut self, gate: Gate) -> Self {
        self.gate = gate;
        self
    }

    /// 这个平台有没有这一项（设置文件里读写它也按这个，与程序装没装无关）。
    pub fn visible_on(&self, platform: Platform) -> bool {
        self.platforms & platform_mask(platform) != 0
    }

    /// 设置窗口里显不显示：这个平台有，并且合乎“装没装”的条件（[`Gate`]，只在 Windows 上看）。
    pub fn shown(&self, platform: Platform, installed: bool) -> bool {
        self.visible_on(platform)
            && (platform != Platform::Windows
                || match self.gate {
                    Gate::ShownWhenInstalled => installed,
                    Gate::ShownWhenNotInstalled => !installed,
                    Gate::Any | Gate::EnabledWhenInstalled => true,
                })
    }

    pub fn ready_on(&self, platform: Platform) -> bool {
        self.ready & platform_mask(platform) != 0
    }

    /// 设置文件里的 (节, 键)。
    fn file_key(&self) -> (&'static str, &'static str) {
        self.key.split_once('.').unwrap_or(("", self.key))
    }

    /// 标签，按平台措辞，并把 `{key:动作}` 换成快捷键。
    pub fn label(&self, platform: Platform, lang: Lang) -> String {
        let (zh, en) = match self.alt {
            Some((mask, zh, en)) if mask & platform_mask(platform) != 0 => (zh, en),
            _ => (self.zh, self.en),
        };
        expand_keys(pick(lang, zh, en), platform)
    }

    /// 标题下面的说明，按平台措辞；没有时为空。
    pub fn detail_text(&self, platform: Platform, lang: Lang) -> String {
        let text = match (self.detail_alt, self.detail) {
            (Some((mask, zh, en)), _) if mask & platform_mask(platform) != 0 => Some((zh, en)),
            (_, other) => other,
        };
        text.map(|(zh, en)| expand_keys(pick(lang, zh, en), platform))
            .unwrap_or_default()
    }

    /// 标题后面显示的快捷键；没有时为空。
    pub fn shortcut_label(&self, platform: Platform) -> String {
        self.shortcut
            .and_then(actions::find)
            .and_then(|def| def.shortcut(platform))
            .map(|chord| chord.label(platform))
            .unwrap_or_default()
    }
}

/// 卡片上面的组名（[`Kind::Heading`]）。
const fn heading(key: &'static str, page: Page, zh: &'static str, en: &'static str) -> Def {
    item(key, Some(page), Kind::Heading, Initial::None, zh, en)
}

/// 把文字里的 `{key:动作}` 换成该动作在所在平台的第一个快捷键。
fn expand_keys(text: &str, platform: Platform) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{key:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 5..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let shortcut = actions::find(&after[..end])
            .and_then(|def| def.shortcut(platform))
            .map(|chord| chord.label(platform))
            .unwrap_or_default();
        out.push_str(&shortcut);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

// 设置项的键。核心和界面用这些常量取值，测试保证它们都在 DEFS 里。
pub const SNAP_WINDOW: &str = "general.snap_window";
pub const NO_RECENT: &str = "general.no_recent";
pub const HIDE_MENU_BAR: &str = "browse.hide_menu_bar";
pub const SINGLE_INSTANCE: &str = "general.single_instance";
pub const CONFIRM_DELETE: &str = "general.confirm_delete";
pub const CORNER_NOTICES: &str = "general.corner_notices";
pub const ALWAYS_ON_TOP: &str = "general.always_on_top";
pub const ESC_QUITS: &str = "general.esc_quits";
pub const AT_BOUNDARY: &str = "general.at_boundary";
pub const AT_ARCHIVE_BOUNDARY: &str = "general.at_archive_boundary";
pub const REMEMBER_ARCHIVE: &str = "bookmarks.remember_archive";
pub const SHOW_TITLE_BAR: &str = "browse.show_main_window";
pub const SAVE_ZOOM_STATE: &str = "browse.save_zoom_state";
pub const HIDE_CONTROL_BAR: &str = "browse.hide_control_bar";
/// 指针移到控制栏的按钮上时的样子（macOS 26 起；只有 macOS 有这一项）。
pub const BAR_HOVER_EFFECT: &str = "browse.bar_hover_effect";
pub const HIDE_ANIMATION_INFO: &str = "browse.hide_animation_info";
pub const HIDE_SIDE_ARROWS: &str = "browse.hide_side_arrows";
pub const HIDE_ZOOM_INFO: &str = "browse.hide_zoom_info";
pub const HIDE_TOOLTIPS: &str = "browse.hide_tooltips";
pub const SMALL_INFO_FONT: &str = "browse.small_info_font";
pub const BACKGROUND_COLOR: &str = "browse.background_color";
pub const EXIF_ROTATE: &str = "image.exif_rotate";
pub const STRONG_SHARPEN: &str = "image.strong_sharpen";
pub const PREFETCH: &str = "image.prefetch";
pub const PREFETCH_COUNT: &str = "image.prefetch_count";
pub const CACHE_MB: &str = "image.cache_mb";
pub const TOP_FIRST: &str = "image.top_first";
pub const FAST_PREVIEW: &str = "image.fast_preview";
pub const FILE_ICC: &str = "image.file_icc";
pub const MONITOR_ICC: &str = "image.monitor_icc";
pub const ALPHA_BACKGROUND: &str = "image.alpha_background";
pub const GIF_BACKGROUND: &str = "image.gif_background";
pub const INTERPOLATION: &str = "image.interpolation";
pub const FORMAT_BUTTONS_MAC: &str = "formats.buttons_mac";
/// Linux 的 Flatpak 版：让文件管理器用 Anyview 做缩略图、取消登记的两个按钮。
pub const FORMAT_BUTTONS_FLATPAK_THUMBNAILS: &str = "formats.buttons_flatpak_thumbnails";
const FORMAT_NOTE_FLATPAK_THUMBNAILS: &str = "formats.note_flatpak_thumbnails";
pub const KEY_MAPPINGS: &str = "keyboard.mappings";
pub const MOUSE_LEFT: &str = "mouse.left";
pub const DOUBLE_CLICK_MAXIMIZE: &str = "mouse.double_click_maximize";
pub const CLICK_SIDES: &str = "mouse.click_sides";
pub const MOUSE_MIDDLE: &str = "mouse.middle";
pub const MOUSE_RIGHT: &str = "mouse.right";
pub const MOUSE_BACK: &str = "mouse.x_button";
pub const MOUSE_FORWARD: &str = "mouse.y_button";
pub const MOUSE_WHEEL_UP: &str = "mouse.wheel_up";
pub const MOUSE_WHEEL_DOWN: &str = "mouse.wheel_down";
pub const FIT_WINDOW: &str = "misc.fit_window";
pub const FOLDER1: &str = "misc.folder1";
pub const FOLDER2: &str = "misc.folder2";
pub const EDITOR: &str = "misc.editor";
pub const NO_SCREENSAVER: &str = "misc.no_screensaver";
pub const TITLE_FILENAME: &str = "browse.title_filename";
pub const BACKGROUND_IMAGE: &str = "browse.background_image";
pub const BACKGROUND_IMAGE_PATH: &str = "browse.background_image_path";
pub const SHARE_SETTINGS: &str = "bookmarks.share";
pub const PAGE_SOUND: &str = "misc.page_sound";
pub const PAGE_EFFECT: &str = "image.page_effect";
pub const SKIP_RAW: &str = "misc.skip_raw";
pub const SKIP_RAW_WITH_JPEG: &str = "misc.skip_raw_with_jpeg";
pub const RAW_THUMBNAIL: &str = "misc.raw_thumbnail";
pub const LANGUAGE: &str = "language.language";
/// 上下文菜单页（只在 Windows）。
pub const CONTEXT_MENU: &str = "context_menu.enabled";
pub const CONTEXT_MENU_ICON: &str = "context_menu.icon";
pub const THUMB_WEBP: &str = "context_menu.thumb_webp";
pub const THUMB_PSD: &str = "context_menu.thumb_psd";
pub const THUMB_PSB: &str = "context_menu.thumb_psb";
pub const THUMB_OTHER: &str = "context_menu.thumb_other";
// 记住的状态
pub const STATE_SORT: &str = "state.sort";
pub const STATE_SORT_DESC: &str = "state.sort_descending";
pub const STATE_INFO: &str = "state.info_visible";
pub const STATE_BAR_LOCKED: &str = "state.bar_locked";
pub const STATE_EDGE: &str = "state.pixel_edges";
pub const STATE_MODE: &str = "state.display_mode";
pub const STATE_STRETCH: &str = "state.stretch_small";
pub const STATE_TWO_PAGE: &str = "state.two_page";
pub const STATE_PICKER_HIDE_THUMBS: &str = "state.picker_hide_thumbnails";
pub const STATE_ZOOM_LOCK: &str = "state.zoom_lock";
/// 锁定的倍率（百分数，随缩放锁定状态一起保存）。
pub const STATE_ZOOM: &str = "state.locked_zoom_percent";

/// 控制栏的悬停效果：液面鼓起、玻璃水珠、淡色亮底。
const BAR_HOVER_OPTIONS: &[Opt] = &[
    opt("bulge", "液面鼓起", "Liquid bulge"),
    opt("droplet", "玻璃水珠", "Glass droplet"),
    opt("highlight", "淡色亮底", "Soft highlight"),
];

/// 到达首尾时的选项，两个下拉框共用。
const BOUNDARY_OPTIONS: &[Opt] = &[
    opt("none", "停在这一张", "Stay there"),
    opt("ask", "询问", "Ask"),
    opt("loop", "转到另一头", "Wrap around"),
    opt(
        "next_folder",
        "转到下一个或上一个文件夹",
        "Go to the next or previous folder",
    ),
];

/// 语言下拉框：跟随系统，然后是各种语言，写成各自的语言（不翻译）；ID 是语言代码，
/// 顺序同 [`crate::i18n::ALL_LANGS`]。
const LANGUAGE_OPTIONS: &[Opt] = &[
    opt("system", "跟随系统", "Same as the system"),
    opt("zh-Hans", "中文(简体)", "中文(简体)"),
    opt("zh-Hant", "中文(繁體)", "中文(繁體)"),
    opt("en", "English", "English"),
    opt("ja", "日本語", "日本語"),
    opt("ko", "한국어", "한국어"),
    opt("es", "Español", "Español"),
    opt("fr", "Français", "Français"),
    opt("de", "Deutsch", "Deutsch"),
    opt("pt-BR", "Português (Brasil)", "Português (Brasil)"),
    opt("ru", "Русский", "Русский"),
    opt("it", "Italiano", "Italiano"),
    opt("tr", "Türkçe", "Türkçe"),
    opt("pl", "Polski", "Polski"),
    opt("nl", "Nederlands", "Nederlands"),
    opt("vi", "Tiếng Việt", "Tiếng Việt"),
    opt("id", "Bahasa Indonesia", "Bahasa Indonesia"),
    opt("th", "ไทย", "ไทย"),
    opt("uk", "Українська", "Українська"),
    opt("hi", "हिन्दी", "हिन्दी"),
];

/// 背景色预设色板，首项为默认背景色 #202020。
pub const BACKGROUND_PRESETS: &[u32] = &[
    0x202020, 0x8ACFFF, 0xF598D6, 0xF3D240, 0xADD85F, 0x78D9D9, 0xFFAF51, 0xFF6F6F, 0xF359A8,
    0x47CF74, 0xB784FD, 0x58B1FC, 0x9898FF, 0xC3B5A8, 0xABABAB,
];

const PATH_FOLDER_BUTTONS: &[ButtonDef] = &[
    button("browse", "选取…", "Choose…"),
    button("open", "打开", "Open"),
];

/// 键盘页的默认映射：PageDown、PageUp 换压缩包或文件夹，其余 9 行为空。
const DEFAULT_KEY_MAPPINGS: &[(bool, &str, &str)] = &[
    (true, "PageDown", "next_container"),
    (true, "PageUp", "prev_container"),
    (false, "", ""),
    (false, "", ""),
    (false, "", ""),
    (false, "", ""),
    (false, "", ""),
    (false, "", ""),
    (false, "", ""),
    (false, "", ""),
    (false, "", ""),
];

use Kind::{
    Buttons, Choice, Color, KeyMappings, Mapping, Note, Number, Path as PathKind, Toggle,
};
use Page::{Bookmarks, Browse, ContextMenu, Formats, General, Image, Keyboard, Language, Misc, Mouse};

const fn on(page: Page) -> Option<Page> {
    Some(page)
}

/// 全部设置项，顺序就是界面上的顺序（SETTINGS.md 逐项规定）。
///
/// 设置窗口卡片分组结构：每页按用途分为若干圆角卡片，[`Heading`] 标识卡片标题
/// 组名（不单独占一行）。每行左边是标题（下面可以有一行灰色说明 `detail`），右边是控件。反着说的开关
/// （设置文件里存的是"不显示……"）用 `invert` 在界面上翻过来说。
pub static DEFS: &[Def] = &[
    // 1. 一般
    heading("general.window_heading", General, "窗口", "Window"),
    // 默认每次从访达（文件管理器）打开都开新窗口；打开后在当前窗口里换图。
    // Linux 每个窗口是一个进程，打开后新打开的图片交给已经开着的那个。
    item(
        SINGLE_INSTANCE,
        on(General),
        Toggle,
        Initial::Bool(false),
        "从文件管理器打开图片时用当前窗口",
        "Reuse the window for images opened from the file manager",
    )
    .alt(
        MAC,
        "从访达打开图片时用当前窗口",
        "Reuse the window for images opened from Finder",
    )
    .detail(
        "不另开新窗口，直接在开着的窗口里换图",
        "Shows them in the open window instead of a new one",
    )
    .ready(ALL),
    // 配置键对应窗口吸附。Linux 由窗口管理器负责吸附，不显示。
    item(
        SNAP_WINDOW,
        on(General),
        Toggle,
        Initial::Bool(false),
        "拖动窗口时吸附屏幕边缘",
        "Snap to screen edges when dragging",
    )
    .only(MAC | WIN)
    .ready(MAC | WIN),
    // Linux 的 GTK4 没有"置顶"接口（Wayland 不允许程序自己置顶），不显示。
    item(
        ALWAYS_ON_TOP,
        on(General),
        Toggle,
        Initial::Bool(false),
        "总在最前",
        "Always on top",
    )
    .shortcut("always_on_top")
    .only(MAC | WIN)
    .ready(MAC | WIN),
    heading(
        "general.boundary_heading",
        General,
        "翻到第一张或最后一张时",
        "At the first or last image",
    ),
    item(
        AT_BOUNDARY,
        on(General),
        Choice(BOUNDARY_OPTIONS),
        Initial::Choice("loop"),
        "文件夹里",
        "In a folder",
    )
    .ready(ALL),
    item(
        AT_ARCHIVE_BOUNDARY,
        on(General),
        Choice(BOUNDARY_OPTIONS),
        Initial::Choice("loop"),
        "压缩包里",
        "In an archive",
    )
    .ready(ALL),
    heading("general.other_heading", General, "其他", "Other"),
    item(
        NO_RECENT,
        on(General),
        Toggle,
        Initial::Bool(false),
        "记住最近打开的文件和文件夹",
        "Remember recently opened files and folders",
    )
    .invert()
    .ready(ALL),
    item(
        CONFIRM_DELETE,
        on(General),
        Toggle,
        Initial::Bool(true),
        "删除文件前先确认",
        "Confirm before deleting files",
    )
    .ready(ALL),
    item(
        CORNER_NOTICES,
        on(General),
        Toggle,
        Initial::Bool(true),
        "通知显示在角落",
        "Show notices in a corner",
    )
    .detail("关掉时显示在画面中央", "Otherwise they appear in the middle")
    .ready(ALL),
    item(
        ESC_QUITS,
        on(General),
        Toggle,
        Initial::Bool(true),
        "按 Esc 退出",
        "Press Esc to quit",
    )
    .ready(ALL),
    item(
        "general.check_updates",
        on(General),
        Toggle,
        Initial::Bool(false),
        "检查更新",
        "Check for updates",
    ),
    // 2. 浏览
    heading("browse.window_heading", Browse, "窗口", "Window"),
    // Windows 的菜单栏：默认不显示，打开这一项时在窗口顶端显示，内容与 macOS 的菜单栏相同。
    item(
        HIDE_MENU_BAR,
        on(Browse),
        Toggle,
        Initial::Bool(true),
        "显示菜单栏",
        "Show the menu bar",
    )
    .invert()
    .only(WIN)
    .ready(WIN),
    // 配置键对应显示标题栏：不打开时无边框。Linux 的标题栏由系统显示、一直可见，不显示此项。
    item(
        SHOW_TITLE_BAR,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "显示标题栏",
        "Show the title bar",
    )
    .detail(
        "一直显示窗口边框，像普通的看图软件",
        "Always show the window frame, like an ordinary viewer",
    )
    .only(MAC | WIN)
    .ready(MAC | WIN),
    // Linux 的标题栏由窗口管理器显示，一直可见，不需要此项。
    item(
        TITLE_FILENAME,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "顶端菜单隐藏时显示文件名",
        "Show the file name when the menu is hidden",
    )
    .alt(
        MAC,
        "标题栏隐藏时显示文件名",
        "Show the file name when the title bar is hidden",
    )
    .shortcut("toggle_title_filename")
    .only(MAC | WIN)
    .ready(MAC | WIN),
    item(
        SAVE_ZOOM_STATE,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "记住缩放方式",
        "Remember the zoom settings",
    )
    .detail(
        "下次启动时恢复显示方式、拉伸小图和缩放锁定",
        "Restores the view mode, small-image stretching and zoom lock next time",
    )
    .ready(ALL),
    heading("browse.overlay_heading", Browse, "画面上显示", "Show on screen"),
    item(
        HIDE_CONTROL_BAR,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "控制栏",
        "Control bar",
    )
    .invert()
    .ready(ALL),
    item(
        BAR_HOVER_EFFECT,
        on(Browse),
        Choice(BAR_HOVER_OPTIONS),
        Initial::Choice("bulge"),
        "悬停效果",
        "Hover effect",
    )
    .detail(
        "指针移到控制栏的按钮上时的样子（macOS 26 起；更早的系统是淡色亮底）",
        "How a control bar button reacts to the pointer (macOS 26 and later; earlier versions use a soft highlight)",
    )
    .indent(1)
    .requires(HIDE_CONTROL_BAR, false)
    .only(MAC)
    .ready(MAC),
    item(
        HIDE_SIDE_ARROWS,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "左右两侧的翻页箭头",
        "Page arrows on the sides",
    )
    .invert()
    .ready(ALL),
    item(
        HIDE_ANIMATION_INFO,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "动图的帧数和播放按钮",
        "Frame number and play button of animations",
    )
    .detail(
        "关掉后播放动图时左下角不显示（空格照常暂停）",
        "When off, Space still pauses",
    )
    .invert()
    .ready(ALL),
    item(
        HIDE_ZOOM_INFO,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "缩放比例",
        "Zoom percentage",
    )
    .invert()
    .ready(ALL),
    item(
        HIDE_TOOLTIPS,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "鼠标悬停时的提示",
        "Tooltips",
    )
    .invert()
    .ready(ALL),
    item(
        SMALL_INFO_FONT,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "图片信息用小字",
        "Small text for image info",
    )
    .ready(ALL),
    heading("browse.background_heading", Browse, "背景", "Background"),
    item(
        BACKGROUND_COLOR,
        on(Browse),
        Color(BACKGROUND_PRESETS),
        Initial::Color(0x202020),
        "背景颜色",
        "Background color",
    )
    .ready(ALL),
    item(
        BACKGROUND_IMAGE,
        on(Browse),
        Toggle,
        Initial::Bool(false),
        "背景图片",
        "Background image",
    )
    .detail(
        "显示在图片四周和空窗口里",
        "Shown around the image and in an empty window",
    )
    .ready(ALL),
    item(
        BACKGROUND_IMAGE_PATH,
        on(Browse),
        PathKind(&[
            button("browse", "选取…", "Choose…"),
            button("open", "打开文件夹", "Open Folder"),
        ]),
        Initial::Text(""),
        "",
        "",
    )
    .indent(1)
    .requires(BACKGROUND_IMAGE, true)
    .ready(ALL),
    // 3. 图像处理
    heading("image.display_heading", Image, "显示", "Display"),
    item(
        EXIF_ROTATE,
        on(Image),
        Toggle,
        Initial::Bool(true),
        "按照片里记录的方向自动摆正",
        "Turn photos the way they were taken",
    )
    .detail("EXIF 方向", "EXIF orientation")
    .ready(ALL),
    item(
        TOP_FIRST,
        on(Image),
        Toggle,
        Initial::Bool(true),
        "图片比窗口高时先显示顶部",
        "Start at the top of images taller than the window",
    )
    .ready(ALL),
    item(
        FAST_PREVIEW,
        on(Image),
        Toggle,
        Initial::Bool(true),
        "先显示低清预览，再换成清晰图",
        "Show a quick preview first",
    )
    .detail("打开大照片时更快看到画面", "Large photos appear sooner")
    .ready(ALL),
    item(
        PAGE_EFFECT,
        on(Image),
        Choice(&[
            opt("none", "无", "None"),
            opt("page", "翻页", "Page turn"),
            opt("fade", "淡出", "Fade"),
            opt("slide", "滑动", "Slide"),
        ]),
        Initial::Choice("none"),
        "翻页效果",
        "Page transition",
    )
    .ready(ALL),
    heading("image.quality_heading", Image, "画质", "Quality"),
    // 与右键菜单"插值算法"是同一个值：菜单改了，这里也跟着变。
    item(
        INTERPOLATION,
        on(Image),
        Choice(&[
            opt("bilinear", "Bilinear（快）", "Bilinear (fast)"),
            opt("bicubic", "Bicubic（慢，画质好）", "Bicubic (slow, high quality)"),
            opt("lanczos", "Lanczos（慢，画质好）", "Lanczos (slow, high quality)"),
        ]),
        Initial::Choice("bilinear"),
        "缩放算法",
        "Scaling filter",
    )
    .ready(ALL),
    item(
        STRONG_SHARPEN,
        on(Image),
        Toggle,
        Initial::Bool(false),
        "更强的锐化",
        "Stronger sharpening",
    )
    .detail(
        "像素边缘为“减淡+锐化”时生效",
        "Applies when pixel edges are set to Soften+Sharpen",
    )
    .ready(ALL),
    heading("image.color_heading", Image, "颜色与透明", "Color and transparency"),
    item(
        FILE_ICC,
        on(Image),
        Toggle,
        Initial::Bool(true),
        "使用图片自带的颜色配置（ICC）",
        "Use the image's color profile (ICC)",
    )
    .ready(ALL),
    // macOS 由系统负责显示器的色彩匹配，不显示。Windows 按窗口所在显示器在"颜色管理"里的配置文件，
    // Linux 只在 X11 下读桌面设的配置文件（_ICC_PROFILE）；Wayland 的桌面自己做色彩管理。
    item(
        MONITOR_ICC,
        on(Image),
        Toggle,
        Initial::Bool(false),
        "使用显示器的颜色配置（ICC）",
        "Use the monitor's color profile (ICC)",
    )
    .detail(
        "按窗口所在显示器的配置文件显示颜色，给校色过的显示器用",
        "Colors follow the profile of the monitor the window is on; for calibrated monitors",
    )
    .detail_alt(
        LINUX,
        "按窗口所在显示器的配置文件显示颜色，给校色过的显示器用。Wayland 桌面由系统负责，这一项不起作用",
        "Colors follow the profile of the monitor the window is on; for calibrated monitors. Wayland desktops do this themselves, so it has no effect there",
    )
    .only(WIN | LINUX)
    .ready(WIN | LINUX),
    item(
        GIF_BACKGROUND,
        on(Image),
        Choice(&[
            opt("palette", "GIF 自带的背景色", "The GIF's own background"),
            opt("white", "白色", "White"),
            opt("grid", "网格", "Checkerboard"),
        ]),
        Initial::Choice("palette"),
        "GIF 的透明处",
        "Transparent parts of GIFs",
    )
    .ready(ALL),
    item(
        ALPHA_BACKGROUND,
        on(Image),
        Choice(&[
            opt("black", "黑色", "Black"),
            opt("white", "白色", "White"),
            opt("grid", "网格", "Checkerboard"),
        ]),
        Initial::Choice("grid"),
        "PNG 等图片的透明处",
        "Transparent parts of other images",
    )
    .ready(ALL),
    heading("image.cache_heading", Image, "缓存", "Cache"),
    item(
        PREFETCH,
        on(Image),
        Toggle,
        Initial::Bool(true),
        "预先读好接下来的图片",
        "Preload upcoming images",
    )
    .ready(ALL),
    // 预读张数：沿当前翻页方向提前解码缓存的张数。
    item(
        PREFETCH_COUNT,
        on(Image),
        Number {
            min: 1,
            max: 20,
            unit: "张",
        },
        Initial::Int(5),
        "预读",
        "Preload",
    )
    .detail(
        "往你翻的方向读这么多张，反方向读 1 张；缓存放不下时少读几张",
        "This many in the direction you’re flipping, one the other way; fewer if they don’t fit in the cache",
    )
    .indent(1)
    .requires(PREFETCH, true)
    .ready(ALL),
    // 默认 1 GB（可完整缓存多张高像素照片）。
    item(
        CACHE_MB,
        on(Image),
        Number {
            min: 100,
            max: 4096,
            unit: "MB",
        },
        Initial::Int(1024),
        "缓存大小",
        "Cache size",
    )
    .indent(1)
    .ready(ALL),
    // 4. 格式关联（各平台不同）
    heading("formats.default_heading", Formats, "默认看图软件", "Default image viewer"),
    item(
        FORMAT_BUTTONS_MAC,
        on(Formats),
        Buttons(&[
            button("set_default", "设为默认", "Make Default"),
            button("open_finder", "打开访达", "Open Finder"),
        ]),
        Initial::None,
        "把 Anyview 设为它支持的所有图片类型的默认程序",
        "Make Anyview the default app for every image type it supports",
    )
    .detail(
        "也可以在访达里逐个类型设置：选中一张图片 › 文件 › 显示简介 › 打开方式 › 选 Anyview › 全部更改…",
        "Or one type at a time in Finder: select an image › File › Get Info › Open with › Anyview › Change All…",
    )
    .only(MAC)
    .ready(MAC),
    // 两个按钮：“默认应用”打开系统设置的“默认应用”；“按扩展名设置”打开按程序逐个扩展名选的那一页
    // （Windows 11 直接到 Anyview 那一页，Windows 10 只能到“默认应用”页，说明里写了怎么走）。
    // 系统不让程序自己改默认程序，只能带用户到设置里选。程序不是用安装程序装的时候系统里没有登记它，按钮灰显。
    item(
        "formats.buttons_windows",
        on(Formats),
        Buttons(&[
            button("associate", "默认应用", "Default Apps"),
            button("classic", "按扩展名设置", "By Extension"),
        ]),
        Initial::None,
        "在系统设置里选 Anyview",
        "Choose Anyview in Windows Settings",
    )
    .detail(
        "“默认应用”里点“照片查看器”，选 Anyview。PSD、WebP、RAW 等不归“照片查看器”管的格式，点“按扩展名设置”逐个选（Windows 10 在打开的页面最下面点“按应用设置默认值” › Anyview › 管理）",
        "In Default apps, click “Photo viewer” and choose Anyview. For formats outside “Photo viewer” (PSD, WebP, RAW…), click “By Extension” and pick them one by one (on Windows 10: bottom of the page › “Set defaults by app” › Anyview › Manage)",
    )
    .only(WIN)
    .ready(WIN)
    .gate(Gate::EnabledWhenInstalled),
    item(
        "formats.note_windows_not_installed",
        on(Formats),
        Note,
        Initial::None,
        "这个 Anyview 不是用安装程序装的，系统里没有登记它，上面的两个按钮不能用。",
        "This copy of Anyview wasn’t installed with its setup program, so Windows doesn’t know it and the two buttons above are unavailable.",
    )
    .only(WIN)
    .ready(WIN)
    .gate(Gate::ShownWhenNotInstalled),
    // Linux：写进你的 mimeapps.list（KDE、GNOME 等桌面都按它），设之前的默认程序记下来，可以换回去。
    item(
        "formats.buttons_linux",
        on(Formats),
        Buttons(&[
            button("set_default", "设为默认", "Make Default"),
            button("restore_default", "恢复原来的", "Restore Previous"),
        ]),
        Initial::None,
        "把 Anyview 设为它支持的图片类型的默认程序",
        "Make Anyview the default app for the image types it supports",
    )
    .detail(
        "文件管理器里双击图片就用 Anyview 打开；“恢复原来的”换回设之前各种图片的默认程序",
        "Double-clicking an image in the file manager then opens it; “Restore Previous” switches each type back",
    )
    .only(LINUX)
    .ready(LINUX),
    // 访达里的缩略图和空格预览：系统只用装在“应用程序”等正式位置的程序带的扩展。
    heading(
        "formats.finder_heading",
        Formats,
        "访达里的缩略图和预览",
        "Thumbnails and previews in Finder",
    )
    .only(MAC),
    item(
        "formats.note_finder",
        on(Formats),
        Note,
        Initial::None,
        "把 Anyview 放进“应用程序”并打开过一次以后，系统自己不认识的格式（如 QOI、CLIP、JPEG XR、PCX）在访达里也会显示缩略图，选中后按空格可以预览。",
        "Once Anyview is in the Applications folder and has been opened once, formats macOS doesn’t know (such as QOI, CLIP, JPEG XR, PCX) get thumbnails in Finder, and pressing Space previews them.",
    )
    .only(MAC)
    .ready(MAC),
    heading(
        "formats.explorer_heading",
        Formats,
        "资源管理器的缩略图",
        "Thumbnails in File Explorer",
    )
    .only(WIN)
    .gate(Gate::ShownWhenInstalled),
    item(
        "formats.note_windows_thumbnails",
        on(Formats),
        Note,
        Initial::None,
        "系统自己不认识的格式（如 QOI、PSD、SVG、RAW）由 Anyview 做缩略图，不用把 Anyview 设成默认程序；可以在“上下文菜单”页里按格式关掉。",
        "Anyview makes the thumbnails for formats Windows doesn’t know (such as QOI, PSD, SVG, RAW), whether or not it is the default app; you can turn formats off on the Context Menu page.",
    )
    .only(WIN)
    .ready(WIN)
    .gate(Gate::ShownWhenInstalled),
    // Linux Flatpak 版：沙盒不自动导出缩略图生成器，点击按钮后按需在用户目录登记。
    // 登记一个用 flatpak run 调用 Anyview 的；KDE 的 Dolphin 等直接运行缩略图生成器的文件管理器就用它，
    // GNOME 的“文件”把生成器关在自己的沙盒里，用不了。只在 Flatpak 里显示（visible_with_sandbox）。
    heading(
        "formats.thumbnailer_heading",
        Formats,
        "文件管理器的缩略图",
        "File manager thumbnails",
    )
    .only(LINUX),
    item(
        FORMAT_NOTE_FLATPAK_THUMBNAILS,
        on(Formats),
        Note,
        Initial::None,
        "Flatpak 版没法自带缩略图生成器。登记一个以后，KDE 的 Dolphin 等文件管理器会给系统不认识的图片（如 PSD、DDS、各家 RAW）做缩略图，每张要多等一会儿；GNOME 的“文件”用不了。",
        "The Flatpak version can’t bring its own thumbnailer. Once you register one, file managers such as KDE’s Dolphin make thumbnails for images your system doesn’t know (such as PSD, DDS and camera RAW), a little more slowly. GNOME Files can’t use it.",
    )
    .only(LINUX)
    .ready(LINUX),
    item(
        FORMAT_BUTTONS_FLATPAK_THUMBNAILS,
        on(Formats),
        Buttons(&[
            button("register_thumbnailer", "登记", "Register"),
            button("unregister_thumbnailer", "取消登记", "Unregister"),
        ]),
        Initial::None,
        "让文件管理器用 Anyview 做缩略图",
        "Let the file manager use Anyview",
    )
    .only(LINUX)
    .ready(LINUX),
    // 5. 键盘：自己设的快捷键，优先于默认键位。只列用过的行，下面"添加快捷键"。
    item(
        KEY_MAPPINGS,
        on(Keyboard),
        KeyMappings,
        Initial::KeyMappings(DEFAULT_KEY_MAPPINGS),
        "",
        "",
    )
    .ready(ALL),
    // 6. 鼠标：每个按键一个下拉框，首项"默认"（照常用原来的功能，设置文件里是没勾选），其次"无操作"（屏蔽）。
    heading("mouse.left_heading", Mouse, "左键", "Left button"),
    item(
        MOUSE_LEFT,
        on(Mouse),
        Mapping,
        Initial::Mapping(false, ""),
        "单击",
        "Click",
    )
    .detail(
        "拖动照常移动窗口或平移图片",
        "Dragging still moves the window or pans",
    )
    .ready(ALL),
    item(
        DOUBLE_CLICK_MAXIMIZE,
        on(Mouse),
        Toggle,
        Initial::Bool(true),
        "双击最大化/还原",
        "Double-click to maximize or restore",
    )
    .indent(1)
    .requires(MOUSE_LEFT, false)
    .ready(ALL),
    item(
        CLICK_SIDES,
        on(Mouse),
        Toggle,
        Initial::Bool(false),
        "单击窗口两侧翻页",
        "Click the sides to turn pages",
    )
    .detail(
        "左边三分之一翻到上一张，右边三分之一翻到下一张",
        "The left third goes back, the right third goes forward",
    )
    .indent(1)
    .requires(MOUSE_LEFT, false)
    .ready(ALL),
    heading("mouse.other_heading", Mouse, "其他按键", "Other buttons"),
    item(
        MOUSE_MIDDLE,
        on(Mouse),
        Mapping,
        Initial::Mapping(false, ""),
        "中键",
        "Middle button",
    )
    .detail("拖动照常平移图片", "Dragging still pans")
    .ready(ALL),
    item(
        MOUSE_RIGHT,
        on(Mouse),
        Mapping,
        Initial::Mapping(false, ""),
        "右键",
        "Right button",
    )
    .detail("默认：打开右键菜单", "Default: opens the context menu")
    .detail_alt(
        MAC,
        "默认：打开右键菜单。按住 ⌃ 点按总是打开右键菜单",
        "Default: opens the context menu. Control-click always opens it",
    )
    .ready(ALL),
    item(
        MOUSE_BACK,
        on(Mouse),
        Mapping,
        Initial::Mapping(false, ""),
        "侧键 1（后退）",
        "Side button 1 (back)",
    )
    .detail("默认：上一张", "Default: previous image")
    .ready(ALL),
    item(
        MOUSE_FORWARD,
        on(Mouse),
        Mapping,
        Initial::Mapping(false, ""),
        "侧键 2（前进）",
        "Side button 2 (forward)",
    )
    .detail("默认：下一张", "Default: next image")
    .ready(ALL),
    item(
        MOUSE_WHEEL_UP,
        on(Mouse),
        Mapping,
        Initial::Mapping(false, ""),
        "滚轮向上",
        "Wheel up",
    )
    .detail(
        "默认：上一张。按住 Ctrl 滚动照常缩放",
        "Default: previous image. Ctrl+wheel still zooms",
    )
    .detail_alt(
        MAC,
        "默认：上一张。按住 ⌘ 滚动照常缩放",
        "Default: previous image. ⌘+wheel still zooms",
    )
    .ready(ALL),
    item(
        MOUSE_WHEEL_DOWN,
        on(Mouse),
        Mapping,
        Initial::Mapping(false, ""),
        "滚轮向下",
        "Wheel down",
    )
    .detail(
        "默认：下一张。按住 Ctrl 滚动照常缩放",
        "Default: next image. Ctrl+wheel still zooms",
    )
    .detail_alt(
        MAC,
        "默认：下一张。按住 ⌘ 滚动照常缩放",
        "Default: next image. ⌘+wheel still zooms",
    )
    .ready(ALL),
    // 7. 书签：不做书签，只保留压缩包内位置的记忆。
    heading("bookmarks.archive_heading", Bookmarks, "压缩包", "Archives"),
    item(
        REMEMBER_ARCHIVE,
        on(Bookmarks),
        Toggle,
        Initial::Bool(true),
        "记住在压缩包里看到哪一张",
        "Remember where you were in each archive",
    )
    .detail(
        "再打开同一个压缩包时，从上次看到的那张开始",
        "Reopening an archive continues where you left off",
    )
    .ready(ALL),
    item(
        "bookmarks.clear_archive",
        on(Bookmarks),
        Buttons(&[button("clear", "清除", "Clear")]),
        Initial::None,
        "忘掉所有记住的位置",
        "Forget all remembered positions",
    )
    .indent(1)
    .ready(ALL),
    // macOS、Windows 是一个程序多个窗口，天然共享，不显示。
    heading("bookmarks.windows_heading", Bookmarks, "多个窗口", "Windows").only(LINUX),
    item(
        SHARE_SETTINGS,
        on(Bookmarks),
        Toggle,
        Initial::Bool(true),
        "多个窗口共用设置",
        "Share settings between windows",
    )
    .detail(
        "在一个窗口里改的设置，别的窗口也跟着用",
        "Changes made in one window apply to the others",
    )
    .only(LINUX)
    .ready(LINUX),
    // 8. 上下文菜单：只在 Windows，而且装好后才显示（右键菜单和缩略图是安装时向系统登记的）。
    // 这一页的开关点“确定”后，装好的程序马上按新的开关重新登记。不提供图片转换功能、
    // “上下文菜单预览”“显示图片信息”需要向资源管理器进程注入代码，出于安全与稳定性考虑不提供。
    heading(
        "context_menu.menu_heading",
        ContextMenu,
        "资源管理器的右键菜单",
        "Right-click menu in File Explorer",
    )
    .only(WIN)
    .gate(Gate::ShownWhenInstalled),
    item(
        CONTEXT_MENU,
        on(ContextMenu),
        Toggle,
        Initial::Bool(false),
        "在右键菜单里加“用 Anyview 浏览图片”",
        "Add “Browse with Anyview” to the right-click menu",
    )
    .detail(
        "右键单击一张图片或压缩包时出现",
        "Appears when you right-click an image or archive",
    )
    .only(WIN)
    .ready(WIN)
    .gate(Gate::ShownWhenInstalled),
    item(
        CONTEXT_MENU_ICON,
        on(ContextMenu),
        Toggle,
        Initial::Bool(true),
        "带上 Anyview 的图标",
        "Show the Anyview icon",
    )
    .indent(1)
    .requires(CONTEXT_MENU, true)
    .only(WIN)
    .ready(WIN)
    .gate(Gate::ShownWhenInstalled),
    heading(
        "context_menu.thumbs_heading",
        ContextMenu,
        "资源管理器的缩略图",
        "Thumbnails in File Explorer",
    )
    .only(WIN)
    .gate(Gate::ShownWhenInstalled),
    item(THUMB_WEBP, on(ContextMenu), Toggle, Initial::Bool(true), "WebP", "WebP")
        .only(WIN)
        .ready(WIN)
        .gate(Gate::ShownWhenInstalled),
    item(THUMB_PSD, on(ContextMenu), Toggle, Initial::Bool(true), "PSD", "PSD")
        .only(WIN)
        .ready(WIN)
        .gate(Gate::ShownWhenInstalled),
    item(THUMB_PSB, on(ContextMenu), Toggle, Initial::Bool(true), "PSB", "PSB")
        .only(WIN)
        .ready(WIN)
        .gate(Gate::ShownWhenInstalled),
    item(
        THUMB_OTHER,
        on(ContextMenu),
        Toggle,
        Initial::Bool(true),
        "其他格式",
        "Other formats",
    )
    .detail("QOI、TGA、SVG、RAW 等", "QOI, TGA, SVG, RAW and more")
    .only(WIN)
    .ready(WIN)
    .gate(Gate::ShownWhenInstalled),
    item(
        "context_menu.thumbs_note",
        on(ContextMenu),
        Note,
        Initial::None,
        "关掉的格式，缩略图交还给系统或别的程序。资源管理器会存着做过的缩略图，已经显示过的文件不会马上变。",
        "For a format you turn off, thumbnails go back to Windows or another program. Explorer keeps the thumbnails it has made, so files already shown won’t change right away.",
    )
    .only(WIN)
    .ready(WIN)
    .gate(Gate::ShownWhenInstalled),
    // 9. 杂项
    heading("misc.folders_heading", Misc, "图片文件夹", "Image folders"),
    item(
        "misc.folders_note",
        on(Misc),
        Note,
        Initial::None,
        "“复制到图片文件夹”“移动到图片文件夹”把图片放到这里。",
        "Copy to Image Folder and Move to Image Folder put images here.",
    )
    .ready(ALL),
    item(
        FOLDER1,
        on(Misc),
        PathKind(PATH_FOLDER_BUTTONS),
        Initial::PicturesFolder,
        "文件夹 1",
        "Folder 1",
    )
    .ready(ALL),
    item(
        FOLDER2,
        on(Misc),
        PathKind(PATH_FOLDER_BUTTONS),
        Initial::Text(""),
        "文件夹 2",
        "Folder 2",
    )
    .ready(ALL),
    heading("misc.editor_heading", Misc, "图像编辑器", "Image editor"),
    item(
        "misc.editor_note",
        on(Misc),
        Note,
        Initial::None,
        "“用图像编辑器打开”（{key:open_in_editor}）用这个程序打开图片。",
        "Open in Image Editor ({key:open_in_editor}) opens the image with this app.",
    )
    .ready(ALL),
    item(
        EDITOR,
        on(Misc),
        PathKind(&[
            button("browse", "选取…", "Choose…"),
            button("presets", "用推荐的", "Use Suggested"),
        ]),
        Initial::Text(""),
        "程序",
        "App",
    )
    .ready(ALL),
    heading("misc.raw_heading", Misc, "RAW 照片", "RAW photos"),
    item(
        SKIP_RAW,
        on(Misc),
        Toggle,
        Initial::Bool(false),
        "打开 RAW 照片",
        "Open RAW photos",
    )
    .invert()
    .ready(ALL),
    item(
        SKIP_RAW_WITH_JPEG,
        on(Misc),
        Toggle,
        Initial::Bool(true),
        "有同名的 JPEG 时跳过 RAW",
        "Skip RAW files that have a JPEG with the same name",
    )
    .indent(1)
    .requires(SKIP_RAW, false)
    .ready(ALL),
    item(
        RAW_THUMBNAIL,
        on(Misc),
        Toggle,
        Initial::Bool(true),
        "先显示 RAW 里相机做的预览图",
        "Show the camera's preview first",
    )
    .detail(
        "关掉时每次都完整解码，慢一些",
        "Otherwise every RAW file is fully decoded, which is slower",
    )
    .indent(1)
    .requires(SKIP_RAW, false)
    .ready(ALL),
    heading("misc.other_heading", Misc, "其他", "Other"),
    item(
        FIT_WINDOW,
        on(Misc),
        Toggle,
        Initial::Bool(true),
        "窗口大小跟着图片",
        "Resize the window to fit the image",
    )
    .ready(ALL),
    item(
        PAGE_SOUND,
        on(Misc),
        Toggle,
        Initial::Bool(false),
        "翻页时播放音效",
        "Play a sound when turning pages",
    )
    .ready(ALL),
    item(
        NO_SCREENSAVER,
        on(Misc),
        Toggle,
        Initial::Bool(false),
        "看图时不让屏幕变暗或睡眠",
        "Keep the screen on while viewing",
    )
    .detail(
        "窗口里有图片时，屏幕保护程序和显示器睡眠都不启动",
        "No screen saver or display sleep while a window shows an image",
    )
    .ready(ALL),
    // 10. 语言
    item(
        LANGUAGE,
        on(Language),
        Choice(LANGUAGE_OPTIONS),
        Initial::Choice("system"),
        "界面语言",
        "Language",
    )
    .detail(
        "点“确定”后菜单和窗口马上换成新的语言",
        "Menus and windows switch as soon as you click OK",
    )
    .ready(ALL),
    // 记住的状态：由菜单、按键改变，随时保存。
    item(
        STATE_SORT,
        None,
        Choice(&[
            opt("name", "", ""),
            opt("modified", "", ""),
            opt("size", "", ""),
            opt("type", "", ""),
        ]),
        Initial::Choice("name"),
        "",
        "",
    )
    .ready(ALL),
    item(STATE_SORT_DESC, None, Toggle, Initial::Bool(false), "", "").ready(ALL),
    item(STATE_INFO, None, Toggle, Initial::Bool(false), "", "").ready(ALL),
    item(STATE_BAR_LOCKED, None, Toggle, Initial::Bool(false), "", "").ready(ALL),
    item(
        STATE_EDGE,
        None,
        Choice(&[
            opt("none", "", ""),
            opt("soften", "", ""),
            opt("soften_sharpen", "", ""),
        ]),
        Initial::Choice("soften"),
        "",
        "",
    )
    .ready(ALL),
    item(
        STATE_MODE,
        None,
        Choice(&[
            opt("fit_window", "", ""),
            opt("original", "", ""),
            opt("fit_width", "", ""),
        ]),
        Initial::Choice("fit_window"),
        "",
        "",
    )
    .ready(ALL),
    item(STATE_STRETCH, None, Toggle, Initial::Bool(false), "", "").ready(ALL),
    // 智能双页阅览：关、左→右、左←右，随时记住。
    item(
        STATE_TWO_PAGE,
        None,
        Choice(&[
            opt("off", "", ""),
            opt("ltr", "", ""),
            opt("rtl", "", ""),
        ]),
        Initial::Choice("off"),
        "",
        "",
    )
    .ready(ALL),
    // 选择图片窗口的"隐藏缩略图导航"（随时记住）。
    item(STATE_PICKER_HIDE_THUMBS, None, Toggle, Initial::Bool(false), "", "").ready(ALL),
    item(STATE_ZOOM_LOCK, None, Toggle, Initial::Bool(false), "", "").ready(ALL),
    item(
        STATE_ZOOM,
        None,
        Number {
            min: 1,
            max: 3200,
            unit: "%",
        },
        Initial::Int(100),
        "",
        "",
    )
    .ready(ALL),
];

pub fn find(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

/// 设置窗口里显示的页：至少有一项在这个平台显示的页。
pub fn visible_pages(platform: Platform) -> Vec<Page> {
    visible_pages_with(platform, windows_installed())
}

/// 同 [`visible_pages`]，“程序是不是用安装程序装的”由调用方给（测试用，不碰全局的那个开关）。
pub fn visible_pages_with(platform: Platform, installed: bool) -> Vec<Page> {
    PAGES
        .iter()
        .copied()
        .filter(|&page| {
            DEFS.iter()
                .any(|d| d.page == Some(page) && d.shown(platform, installed))
        })
        .collect()
}

// ---- 取值 ----

/// 键盘映射表至少显示的行数。
pub const KEY_ROWS: usize = 11;

/// 键盘映射表的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyMapping {
    pub enabled: bool,
    /// 按键，写法同动作目录（如 `PageDown`、`Ctrl+K`）；空表示还没设。
    pub key: String,
    /// 动作 ID；空表示还没选。
    pub action: String,
}

impl KeyMapping {
    /// 没用过的空行。
    pub fn is_empty(&self) -> bool {
        !self.enabled && self.key.is_empty() && self.action.is_empty()
    }

    /// 按键、动作都没设（刚"添加"、还没填的行也算）：不写进设置文件。
    pub fn is_unused(&self) -> bool {
        self.key.is_empty() && self.action.is_empty()
    }
}

/// 映射表至少 11 行，全部用完时末尾自动多一行空行。
fn pad_key_rows(rows: &mut Vec<KeyMapping>) {
    let blank = || KeyMapping {
        enabled: false,
        key: String::new(),
        action: String::new(),
    };
    while rows.len() < KEY_ROWS {
        rows.push(blank());
    }
    if rows.last().is_some_and(|row| !row.is_empty()) {
        rows.push(blank());
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    /// 选项 ID。
    Choice(String),
    Int(i64),
    /// 0xRRGGBB。
    Color(u32),
    Text(String),
    /// 鼠标映射：是否启用、动作 ID。
    Mapping {
        enabled: bool,
        action: String,
    },
    KeyMappings(Vec<KeyMapping>),
    /// 草稿里暂时不合法的输入（如缓存大小写成了字母）；点"确定"时校验失败。
    Invalid(String),
}

impl Initial {
    fn value(self) -> Option<Value> {
        Some(match self {
            Initial::None => return None,
            Initial::Bool(b) => Value::Bool(b),
            Initial::Choice(id) => Value::Choice(id.to_string()),
            Initial::Int(n) => Value::Int(n),
            Initial::Color(c) => Value::Color(c),
            Initial::Text(t) => Value::Text(t.to_string()),
            Initial::PicturesFolder => Value::Text(pictures_folder()),
            Initial::Mapping(enabled, action) => Value::Mapping {
                enabled,
                action: action.to_string(),
            },
            Initial::KeyMappings(rows) => Value::KeyMappings(
                rows.iter()
                    .map(|&(enabled, key, action)| KeyMapping {
                        enabled,
                        key: key.to_string(),
                        action: action.to_string(),
                    })
                    .collect(),
            ),
        })
    }
}

/// 系统的"图片"文件夹：macOS、Linux 为 `~/Pictures`，Windows 为 `%USERPROFILE%\Pictures`。
fn pictures_folder() -> String {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    home.map(|h| PathBuf::from(h).join("Pictures").display().to_string())
        .unwrap_or_default()
}

/// 一组完整的设置取值。
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    values: BTreeMap<&'static str, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self::defaults()
    }
}

impl Settings {
    /// 全部默认值。
    pub fn defaults() -> Self {
        let values = DEFS
            .iter()
            .filter_map(|d| d.initial.value().map(|v| (d.key, v)))
            .collect();
        Self { values }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// 写入一个值（不校验；调用方保证合法）。不认识的键忽略。
    pub fn set(&mut self, key: &str, value: Value) {
        if let Some(def) = find(key)
            && def.kind.has_value()
        {
            self.values.insert(def.key, value);
        }
    }

    fn initial(key: &str) -> Option<Value> {
        find(key).and_then(|d| d.initial.value())
    }

    pub fn bool(&self, key: &str) -> bool {
        match self.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Mapping { enabled, .. }) => *enabled,
            _ => matches!(Self::initial(key), Some(Value::Bool(true))),
        }
    }

    pub fn choice(&self, key: &str) -> String {
        match self.get(key) {
            Some(Value::Choice(id)) => id.clone(),
            _ => match Self::initial(key) {
                Some(Value::Choice(id)) => id,
                _ => String::new(),
            },
        }
    }

    pub fn int(&self, key: &str) -> i64 {
        match self.get(key) {
            Some(Value::Int(n)) => *n,
            _ => match Self::initial(key) {
                Some(Value::Int(n)) => n,
                _ => 0,
            },
        }
    }

    pub fn color(&self, key: &str) -> u32 {
        match self.get(key) {
            Some(Value::Color(c)) => *c,
            _ => match Self::initial(key) {
                Some(Value::Color(c)) => c,
                _ => 0,
            },
        }
    }

    pub fn text(&self, key: &str) -> String {
        match self.get(key) {
            Some(Value::Text(t)) => t.clone(),
            _ => match Self::initial(key) {
                Some(Value::Text(t)) => t,
                _ => String::new(),
            },
        }
    }

    /// 到达首尾时的行为。
    /// 在压缩包里到达首尾时。
    pub fn archive_boundary(&self) -> Boundary {
        boundary_from(&self.choice(AT_ARCHIVE_BOUNDARY))
    }

    pub fn boundary(&self) -> Boundary {
        boundary_from(&self.choice(AT_BOUNDARY))
    }
}

fn boundary_from(choice: &str) -> Boundary {
    match choice {
        "none" => Boundary::Nothing,
        "loop" => Boundary::Loop,
        "next_folder" => Boundary::NextFolder,
        _ => Boundary::Ask,
    }
}

impl Settings {
    pub fn alpha_background(&self) -> AlphaBackground {
        match self.choice(ALPHA_BACKGROUND).as_str() {
            "black" => AlphaBackground::Black,
            "white" => AlphaBackground::White,
            _ => AlphaBackground::Grid,
        }
    }

    /// 控制栏的悬停效果。
    pub fn bar_hover_effect(&self) -> BarHoverEffect {
        match self.choice(BAR_HOVER_EFFECT).as_str() {
            "droplet" => BarHoverEffect::Droplet,
            "highlight" => BarHoverEffect::Highlight,
            _ => BarHoverEffect::Bulge,
        }
    }

    /// 界面语言：跟随系统时用 `system`。
    pub fn lang(&self, system: Lang) -> Lang {
        Lang::from_code(&self.choice(LANGUAGE)).unwrap_or(system)
    }
}

/// 到达首尾时的行为。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Boundary {
    /// 停在边界，在角落提示。
    Nothing,
    /// 询问（默认）。
    #[default]
    Ask,
    /// 循环。
    Loop,
    /// 转到下一个、上一个文件夹。
    NextFolder,
}

/// 32 位图片透明处的背景。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaBackground {
    Black,
    White,
    /// 棋盘格。
    Grid,
}

/// 指针移到控制栏的按钮上时的样子（macOS 26 起，更早的系统一律是淡色亮底）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHoverEffect {
    /// 控制栏的玻璃在按钮处鼓起一个圆包，图标浮起一点。
    Bulge,
    /// 一颗透明的玻璃水珠盖在按钮上，图标在里面放大。
    Droplet,
    /// 按钮下面一块淡色的底（和 macOS 工具栏按钮一样）。
    Highlight,
}

/// 一张图片的透明处画什么（[`crate::AppCore::alpha_fill`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaFill {
    /// 棋盘格。
    Checker,
    /// 纯色 0xRRGGBB。
    Solid(u32),
}

// ---- 设置文件 ----

/// Windows 免安装版的标记文件：在程序旁边时，设置文件放在程序旁边的 `data` 文件夹里。
pub const PORTABLE_MARKER: &str = "portable.txt";

/// Windows 免安装版：程序所在的文件夹里有 [`PORTABLE_MARKER`] 时，设置文件在旁边的 `data\settings.toml`
/// （最近打开的、压缩包里看到的位置、桌面图片的副本都放在设置文件旁边，一起在那里），不写进系统的用户文件夹；
/// 没有时为空（用 [`default_path`]）。
pub fn portable_path(program_dir: &Path) -> Option<PathBuf> {
    program_dir
        .join(PORTABLE_MARKER)
        .is_file()
        .then(|| program_dir.join("data").join("settings.toml"))
}

/// 各平台的设置文件位置。取不到主目录时为空（不保存）。
pub fn default_path(platform: Platform) -> Option<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    match platform {
        Platform::Macos => {
            env("HOME").map(|h| h.join("Library/Application Support/Anyview/settings.toml"))
        }
        Platform::Windows => env("APPDATA").map(|d| d.join("Anyview").join("settings.toml")),
        Platform::Linux => env("XDG_CONFIG_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".config")))
            .map(|d| d.join("anyview").join("settings.toml")),
    }
}

/// 读设置文件的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    /// 读到了。
    Loaded,
    /// 还没有设置文件，使用默认值。
    Missing,
    /// 文件坏了，使用默认值；旧文件已改名备份（备份失败时为空）。界面在启动后提示一次。
    Recovered {
        backup: Option<PathBuf>,
        reason: String,
    },
}

/// 读出的设置，以及原始的表（保存时保留认不出的键）。
pub struct Loaded {
    pub settings: Settings,
    pub table: toml::Table,
    pub status: LoadStatus,
    /// 文件在、但这一次没读出来（被占用、没有权限、磁盘出错）：没有改名备份，内容多半是好的。
    /// 调用方先用默认值，不能照默认值把它盖掉。
    pub unreadable: bool,
}

/// 读设置文件。文件不存在时用默认值；内容坏了（解析失败、不是文字）时备份旧文件、用默认值；
/// 只是这一次读不出来（被占用、没有权限）时不动文件，用默认值并标出 `unreadable`。
/// 个别键的值不合法（类型不对、不在范围内）时只有这一项用默认值。
pub fn load(path: &Path) -> Loaded {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Loaded {
                settings: Settings::defaults(),
                table: toml::Table::new(),
                status: LoadStatus::Missing,
                unreadable: false,
            };
        }
        // 内容不是文字：文件坏了，备份后用默认值。
        Err(e) if e.kind() == io::ErrorKind::InvalidData => return recovered(path, e.to_string()),
        // 这一次读不出来（被别的程序占用、没有权限、磁盘出错）：文件多半是好的，不改名，先用默认值。
        Err(e) => {
            return Loaded {
                settings: Settings::defaults(),
                table: toml::Table::new(),
                status: LoadStatus::Recovered {
                    backup: None,
                    reason: e.to_string(),
                },
                unreadable: true,
            };
        }
    };
    match text.parse::<toml::Table>() {
        Ok(mut table) => {
            let version = table
                .get("version")
                .and_then(toml::Value::as_integer)
                .unwrap_or(VERSION);
            migrate(&mut table, version);
            Loaded {
                settings: from_table(&table),
                table,
                status: LoadStatus::Loaded,
                unreadable: false,
            }
        }
        Err(e) => recovered(path, e.message().to_string()),
    }
}

fn recovered(path: &Path, reason: String) -> Loaded {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings".into());
    let backup = path.with_file_name(format!("{stem}-broken-{stamp}.toml"));
    let backup = fs::rename(path, &backup).ok().map(|_| backup);
    Loaded {
        settings: Settings::defaults(),
        table: toml::Table::new(),
        status: LoadStatus::Recovered { backup, reason },
        unreadable: false,
    }
}

/// 把旧版本的设置文件迁移到当前版本。
///
/// 第 1 版 → 第 2 版：到达首尾时的默认行为从"询问"调整为"转到首个文件"。旧版配置全量写出默认值
/// 都写着（包括没改过的默认值），分不出"询问"是用户选的还是当时的默认值：一律换成新的默认值。
///
/// 第 2 版 → 第 3 版：默认缓存大小从 300 MB 调整为 1024 MB。
/// 用户自己填的：写着 300 的换成新的默认值，别的数照旧。
fn migrate(table: &mut toml::Table, from: i64) {
    if from < 2
        && let Some(toml::Value::Table(general)) = table.get_mut("general")
    {
        for key in ["at_boundary", "at_archive_boundary"] {
            if general.get(key).and_then(toml::Value::as_str) == Some("ask") {
                general.remove(key);
            }
        }
    }
    if from < 3
        && let Some(toml::Value::Table(image)) = table.get_mut("image")
        && image.get("cache_mb").and_then(toml::Value::as_integer) == Some(300)
    {
        image.remove("cache_mb");
    }
}

/// 读设置文件，不备份、不改名：别的进程（Linux 的另一个窗口）改了设置文件时重新读用。
/// 读不了、内容坏了时为空（下一次再试）。
pub fn read_file(path: &Path) -> Option<(Settings, toml::Table)> {
    let text = fs::read_to_string(path).ok()?;
    let mut table = text.parse::<toml::Table>().ok()?;
    let version = table
        .get("version")
        .and_then(toml::Value::as_integer)
        .unwrap_or(VERSION);
    migrate(&mut table, version);
    Some((from_table(&table), table))
}

/// 两份设置里值不同的项，不含记住的状态（state.*：各窗口自己的排序、开关等不跟着别的窗口变）。
/// 保存时写下的内容：以设置文件里现在的 `disk` 为底，只盖上本进程改过的项（`ours` 与 `baseline` 不同的）。
/// 别的进程改的其他项不会被写回旧值。
pub fn overlay(disk: &Settings, ours: &Settings, baseline: &Settings) -> Settings {
    let mut merged = disk.clone();
    for def in DEFS.iter().filter(|d| d.kind.has_value()) {
        if !same_in_file(ours.get(def.key), baseline.get(def.key))
            && let Some(value) = ours.get(def.key)
        {
            merged.values.insert(def.key, value.clone());
        }
    }
    merged
}

/// `fresh` 的设置，记住的状态（state.*）换成 `ours` 的。
pub fn with_state_of(fresh: &Settings, ours: &Settings) -> Settings {
    let mut merged = fresh.clone();
    for (key, value) in ours.values.iter().filter(|(k, _)| k.starts_with("state.")) {
        merged.values.insert(key, value.clone());
    }
    merged
}

pub fn differences(a: &Settings, b: &Settings) -> Vec<&'static str> {
    DEFS.iter()
        .map(|d| d.key)
        .filter(|k| !k.starts_with("state.") && !same_in_file(a.get(k), b.get(k)))
        .collect()
}

/// 两个值写进设置文件是不是一样。按文件里的写法比：键盘映射的空行不写进文件，内存里多几行空行、
/// 空行在中间的，写下去再读回来就不一样了，但它们是同一份设置（不能当成"别的窗口改过"）。
fn same_in_file(a: Option<&Value>, b: Option<&Value>) -> bool {
    a.and_then(to_toml) == b.and_then(to_toml)
}

fn from_table(table: &toml::Table) -> Settings {
    let mut settings = Settings::defaults();
    for def in DEFS.iter().filter(|d| d.kind.has_value()) {
        let (section, name) = def.file_key();
        let Some(raw) = table
            .get(section)
            .and_then(toml::Value::as_table)
            .and_then(|t| t.get(name))
        else {
            continue;
        };
        if let Some(value) = parse_value(def, raw) {
            settings.values.insert(def.key, value);
        }
    }
    settings
}

/// 设置文件里的一个值；不合法时为空（这一项用默认值）。
fn parse_value(def: &Def, raw: &toml::Value) -> Option<Value> {
    match def.kind {
        Kind::Toggle => raw.as_bool().map(Value::Bool),
        Kind::Choice(options) => {
            let id = raw.as_str()?;
            options
                .iter()
                .any(|o| o.id == id)
                .then(|| Value::Choice(id.to_string()))
        }
        Kind::Number { min, max, .. } => {
            let n = raw.as_integer()?;
            (min..=max).contains(&n).then_some(Value::Int(n))
        }
        Kind::Color(_) => parse_color(raw.as_str()?).map(Value::Color),
        Kind::Path(_) => raw.as_str().map(|s| Value::Text(s.to_string())),
        Kind::Mapping => {
            let t = raw.as_table()?;
            let enabled = t.get("enabled")?.as_bool()?;
            let action = t.get("action").and_then(toml::Value::as_str).unwrap_or("");
            mappable(action).then(|| Value::Mapping {
                enabled,
                action: action.to_string(),
            })
        }
        Kind::KeyMappings => {
            let rows = raw.as_array()?;
            // 一行认不出时只动这一行，别的映射留着（以前整张表退回默认）：动作认不出（新版本才有的）的丢掉
            // 这一行；按键认不出（写坏了）的只清掉按键，动作留着等用户重新设。
            let rows = rows
                .iter()
                .filter_map(|row| {
                    let t = row.as_table()?;
                    let key = t.get("key").and_then(toml::Value::as_str).unwrap_or("");
                    let action = t.get("action").and_then(toml::Value::as_str).unwrap_or("");
                    let key = if actions::Chord::parse(key).is_some() { key } else { "" };
                    mappable(action).then(|| KeyMapping {
                        enabled: t
                            .get("enabled")
                            .and_then(toml::Value::as_bool)
                            .unwrap_or(false),
                        key: key.to_string(),
                        action: action.to_string(),
                    })
                })
                .collect::<Vec<_>>();
            let mut rows = rows;
            pad_key_rows(&mut rows);
            Some(Value::KeyMappings(rows))
        }
        Kind::Buttons(_) | Kind::Note | Kind::Heading => None,
    }
}

/// `#RRGGBB`（不区分大小写）。
pub fn parse_color(text: &str) -> Option<u32> {
    let hex = text.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

pub fn color_text(color: u32) -> String {
    format!("#{:06X}", color & 0xFF_FFFF)
}

fn to_toml(value: &Value) -> Option<toml::Value> {
    Some(match value {
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Choice(id) => toml::Value::String(id.clone()),
        Value::Int(n) => toml::Value::Integer(*n),
        Value::Color(c) => toml::Value::String(color_text(*c)),
        Value::Text(t) => toml::Value::String(t.clone()),
        Value::Mapping { enabled, action } => {
            let mut t = toml::Table::new();
            t.insert("enabled".into(), toml::Value::Boolean(*enabled));
            t.insert("action".into(), toml::Value::String(action.clone()));
            toml::Value::Table(t)
        }
        // 只写用过的行；读的时候补足空行。
        Value::KeyMappings(rows) => toml::Value::Array(
            rows.iter()
                .filter(|row| !row.is_unused())
                .map(|row| {
                    let mut t = toml::Table::new();
                    t.insert("enabled".into(), toml::Value::Boolean(row.enabled));
                    t.insert("key".into(), toml::Value::String(row.key.clone()));
                    t.insert("action".into(), toml::Value::String(row.action.clone()));
                    toml::Value::Table(t)
                })
                .collect(),
        ),
        Value::Invalid(_) => return None,
    })
}

/// 生成要写进文件的表：从读到的表出发（保留认不出的键），写入本平台显示且已经实现的设置项和记住的状态。
/// 没实现的项不写，以免让人以为改了文件就能生效。
pub fn to_table(settings: &Settings, base: &toml::Table, platform: Platform) -> toml::Table {
    let mut table = base.clone();
    let version = base
        .get("version")
        .and_then(toml::Value::as_integer)
        .unwrap_or(VERSION)
        .max(VERSION);
    table.insert("version".into(), toml::Value::Integer(version));
    for def in DEFS
        .iter()
        .filter(|d| d.kind.has_value() && d.visible_on(platform) && d.ready_on(platform))
    {
        let Some(value) = settings.get(def.key).and_then(to_toml) else {
            continue;
        };
        let (section, name) = def.file_key();
        let entry = table
            .entry(section.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::Table::new());
        }
        if let toml::Value::Table(t) = entry {
            t.insert(name.to_string(), value);
        }
    }
    table
}

const FILE_HEADER: &str =
    "# Anyview 设置（docs/SETTINGS.md）。由程序写入；手动修改前请先退出 Anyview。\n\n";

/// 写设置文件：先写临时文件再改名，写到一半断电也不会留下半个文件。
pub fn save(path: &Path, table: &toml::Table) -> io::Result<()> {
    let text = toml::to_string(table).map_err(|e| io::Error::other(e.to_string()))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // 临时文件名带进程号：两个窗口（Linux 每个窗口一个进程）同时保存时不会写到同一个文件上。
    let temp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
    let written = (|| {
        use io::Write;
        let mut file = fs::File::create(&temp)?;
        file.write_all(format!("{FILE_HEADER}{text}").as_bytes())?;
        // 落盘之后再改名：不然断电后改好名的文件可能是空的。
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

// ---- 草稿 ----

/// 界面传来的修改。
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Bool(bool),
    Choice(String),
    /// 数字、路径输入框里的文字。
    Text(String),
    Color(u32),
    /// 鼠标映射的下拉框：[`MAPPING_DEFAULT`]（没勾选，照常用原来的功能）、空（"无操作"）或动作 ID。
    MappingChoice(String),
    /// 键盘映射表第 row 行（从 0 开始）的勾选。
    KeyRowEnabled {
        row: usize,
        on: bool,
    },
    /// 第 row 行录入的按键：按键名称（与按键解析相同，如 `PageDown`、`k`）和修饰键；名称为空表示清除。
    KeyRowKey {
        row: usize,
        key: String,
        mods: u32,
    },
    /// 第 row 行选的动作；空表示还没选。
    KeyRowAction {
        row: usize,
        action: String,
    },
    /// 删掉第 row 行（变回空行）。
    KeyRowRemove {
        row: usize,
    },
}

/// 鼠标映射下拉框首项"默认"的 ID：选它就是不勾选这个按键的映射（照常用原来的功能）。
pub const MAPPING_DEFAULT: &str = "default";

/// 校验失败：哪一页、哪一项（映射表还有哪一行，从 0 开始）、为什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid {
    pub page: Page,
    pub key: &'static str,
    pub row: Option<usize>,
    pub reason: InvalidReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidReason {
    /// 不是整数，或不在范围内。
    OutOfRange { min: i64, max: i64 },
    /// 键盘映射：勾选了、设了按键，但没选动作。
    NoAction,
    /// 键盘映射：和前面第 `first` 行（从 0 开始）用了同一个按键。
    DuplicateKey { first: usize, key: String },
}

impl Invalid {
    /// 提示文字，如"缓存大小须是 100 到 4096 之间的整数。"
    pub fn message(&self, platform: Platform, lang: Lang) -> String {
        let label = find(self.key)
            .map(|d| d.label(platform, lang))
            .unwrap_or_default();
        let label = label.trim_end_matches(['：', ':']).trim();
        let row = self.row.map_or(0, |r| r + 1);
        match &self.reason {
            InvalidReason::OutOfRange { min, max } => tr!(
                lang,
                "{label}须是 {min} 到 {max} 之间的整数。",
                "{label} must be a whole number from {min} to {max}."
            )
            .replace("{label}", label)
            .replace("{min}", &min.to_string())
            .replace("{max}", &max.to_string()),
            InvalidReason::NoAction => {
                tr!(lang, "第 {row} 行还没有选择操作。", "Choose an action for row {row}.")
                    .replace("{row}", &row.to_string())
            }
            InvalidReason::DuplicateKey { first, key } => {
                let key = actions::Chord::parse(key)
                    .map(|c| c.label(platform))
                    .unwrap_or_else(|| key.clone());
                let first = first + 1;
                tr!(
                    lang,
                    "第 {first} 行和第 {row} 行用了同一个按键 {key}。",
                    "Rows {first} and {row} use the same key {key}."
                )
                .replace("{first}", &first.to_string())
                .replace("{row}", &row.to_string())
                .replace("{key}", &key)
            }
        }
    }
}

/// 设置窗口的草稿。
#[derive(Clone, Debug)]
pub struct Draft {
    original: Settings,
    values: Settings,
}

impl Draft {
    pub fn new(current: &Settings) -> Self {
        Self {
            original: current.clone(),
            values: current.clone(),
        }
    }

    pub fn values(&self) -> &Settings {
        &self.values
    }

    /// 改一项。返回是否接受（项不存在、不可用或输入种类不对时不接受）。
    pub fn set(&mut self, key: &str, input: Input, platform: Platform) -> bool {
        let Some(def) = find(key) else { return false };
        if def.page.is_none() || !self.enabled(def, platform) {
            return false;
        }
        let value = match (def.kind, input) {
            // 界面上的开关是翻过来说的（"显示控制栏"），存的是"不显示控制栏"。
            (Kind::Toggle, Input::Bool(b)) => Value::Bool(b != def.invert),
            (Kind::Choice(options), Input::Choice(id)) => {
                if !options.iter().any(|o| o.id == id) {
                    return false;
                }
                Value::Choice(id)
            }
            (Kind::Number { min, max, .. }, Input::Text(text)) => {
                match text.trim().parse::<i64>() {
                    Ok(n) if (min..=max).contains(&n) => Value::Int(n),
                    _ => Value::Invalid(text),
                }
            }
            (Kind::Color(_), Input::Color(c)) => Value::Color(c & 0xFF_FFFF),
            (Kind::Path(_), Input::Text(text)) => Value::Text(text),
            // "默认"只是不勾选，选过的动作留着（再选回来时不用重选）。
            (Kind::Mapping, Input::MappingChoice(id)) => {
                let kept = match self.values.get(def.key) {
                    Some(Value::Mapping { action, .. }) => action.clone(),
                    _ => return false,
                };
                if id == MAPPING_DEFAULT {
                    Value::Mapping {
                        enabled: false,
                        action: kept,
                    }
                } else if mappable(&id) {
                    Value::Mapping {
                        enabled: true,
                        action: id,
                    }
                } else {
                    return false;
                }
            }
            (
                Kind::KeyMappings,
                input @ (Input::KeyRowEnabled { .. }
                | Input::KeyRowKey { .. }
                | Input::KeyRowAction { .. }
                | Input::KeyRowRemove { .. }),
            ) => {
                let Some(Value::KeyMappings(rows)) = self.values.get(def.key) else {
                    return false;
                };
                let mut rows = rows.clone();
                if !edit_key_row(&mut rows, input, platform) {
                    return false;
                }
                pad_key_rows(&mut rows);
                Value::KeyMappings(rows)
            }
            _ => return false,
        };
        self.values.values.insert(def.key, value);
        true
    }

    /// 这一项现在能不能改：所在平台已经实现，依赖的开关也满足。
    pub fn enabled(&self, def: &Def, platform: Platform) -> bool {
        item_enabled(def, &self.values, platform)
    }

    /// "重置"：把这一页的草稿恢复为默认值；还要点"确定"才生效。只重置能改的项。
    pub fn reset_page(&mut self, page: Page, platform: Platform) {
        let defaults = Settings::defaults();
        for def in DEFS.iter().filter(|d| {
            d.page == Some(page)
                && d.kind.has_value()
                && d.visible_on(platform)
                && d.ready_on(platform)
        }) {
            if let Some(value) = defaults.get(def.key) {
                self.values.values.insert(def.key, value.clone());
            }
        }
    }

    /// 有没有没确定的修改。
    pub fn is_dirty(&self) -> bool {
        self.values != self.original
    }

    /// 校验：只校验这个平台显示、能改的项。
    pub fn validate(&self, platform: Platform) -> Result<(), Invalid> {
        for def in DEFS.iter() {
            let Some(page) = def.page else { continue };
            if !def.visible_on(platform) || !self.enabled(def, platform) {
                continue;
            }
            if let (Kind::Number { min, max, .. }, Some(Value::Invalid(_))) =
                (def.kind, self.values.get(def.key))
            {
                return Err(Invalid {
                    page,
                    key: def.key,
                    row: None,
                    reason: InvalidReason::OutOfRange { min, max },
                });
            }
            if let Some(Value::KeyMappings(rows)) = self.values.get(def.key) {
                check_key_rows(rows).map_err(|(row, reason)| Invalid {
                    page,
                    key: def.key,
                    row: Some(row),
                    reason,
                })?;
            }
        }
        Ok(())
    }

    /// 改过的项（与打开窗口时相比）。"确定"只写回这些，菜单在此期间改的其他值不会被草稿盖掉。
    pub fn changes(&self) -> Vec<(&'static str, Value)> {
        self.values
            .values
            .iter()
            .filter(|(key, value)| self.original.get(key) != Some(*value))
            .filter(|(_, value)| !matches!(value, Value::Invalid(_)))
            .map(|(key, value)| (*key, value.clone()))
            .collect()
    }
}

fn item_enabled(def: &Def, values: &Settings, platform: Platform) -> bool {
    item_enabled_with(def, values, platform, windows_installed())
}

fn item_enabled_with(def: &Def, values: &Settings, platform: Platform, installed: bool) -> bool {
    if !def.ready_on(platform) {
        return false;
    }
    if platform == Platform::Windows && def.gate == Gate::EnabledWhenInstalled && !installed {
        return false;
    }
    match def.requires {
        Some((key, expected)) => values.bool(key) == expected,
        None => true,
    }
}

/// 改映射表的一行。行号不对、按键认不出或被系统占用（ACTIONS 规则 6）、动作不在下拉框里时不接受。
fn edit_key_row(rows: &mut [KeyMapping], input: Input, platform: Platform) -> bool {
    match input {
        Input::KeyRowEnabled { row, on } => match rows.get_mut(row) {
            Some(r) => r.enabled = on,
            None => return false,
        },
        Input::KeyRowKey { row, key, mods } => {
            let Some(r) = rows.get_mut(row) else {
                return false;
            };
            if key.is_empty() {
                r.key.clear();
            } else {
                let Some(chord) = actions::Chord::new(&key, mods) else {
                    return false;
                };
                if actions::reserved(platform, &chord) {
                    return false;
                }
                r.key = chord.spec();
            }
        }
        Input::KeyRowAction { row, action } => {
            if !mappable(&action) {
                return false;
            }
            match rows.get_mut(row) {
                Some(r) => r.action = action,
                None => return false,
            }
        }
        Input::KeyRowRemove { row } => match rows.get_mut(row) {
            Some(r) => {
                r.enabled = false;
                r.key.clear();
                r.action.clear();
            }
            None => return false,
        },
        _ => return false,
    }
    true
}

/// 映射表的校验：勾选了、设了按键的行必须选了动作；两行不能用同一个按键。返回出错的行和原因。
fn check_key_rows(rows: &[KeyMapping]) -> Result<(), (usize, InvalidReason)> {
    let mut seen: Vec<(actions::Chord, usize)> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if !row.enabled || row.key.is_empty() {
            continue;
        }
        if row.action.is_empty() {
            return Err((i, InvalidReason::NoAction));
        }
        let Some(chord) = actions::Chord::parse(&row.key) else {
            continue;
        };
        if let Some((_, first)) = seen.iter().find(|(c, _)| *c == chord) {
            return Err((
                i,
                InvalidReason::DuplicateKey {
                    first: *first,
                    key: row.key.clone(),
                },
            ));
        }
        seen.push((chord, i));
    }
    Ok(())
}

/// 键盘、鼠标映射下拉框里的动作
/// （设置 键盘/鼠标 操作可映射功能的下拉菜单.png；ACTIONS 中入口带 K 的动作）：(动作, 中文, 英文)。
pub const MAPPABLE: &[(&str, &str, &str)] = &[
    ("fullscreen", "全屏", "Full screen"),
    ("maximize", "最大化", "Maximize"),
    ("minimize", "最小化", "Minimize"),
    ("select_image", "选择图片", "Select image"),
    ("quit", "退出", "Quit"),
    ("open_file", "打开文件", "Open file"),
    ("open_folder", "打开文件夹", "Open folder"),
    (
        "next_page_double",
        "下一页（双页阅览）",
        "Next page (two-page view)",
    ),
    (
        "prev_page_double",
        "上一页（双页阅览）",
        "Previous page (two-page view)",
    ),
    ("next_image", "下一张", "Next image"),
    ("prev_image", "上一张", "Previous image"),
    ("random_forward", "向后随机转到某张图片", "Random image"),
    (
        "random_back",
        "向前随机转到某张图片",
        "Back from random image",
    ),
    ("first_image", "首张图片", "First image"),
    ("last_image", "末尾图片", "Last image"),
    ("forward_10", "向后10张图片", "Forward 10 images"),
    ("back_10", "向前10张图片", "Back 10 images"),
    ("pan_up", "上移", "Pan up"),
    ("pan_down", "下移", "Pan down"),
    ("pan_left", "左移", "Pan left"),
    ("pan_right", "右移", "Pan right"),
    ("zoom_in", "放大", "Zoom in"),
    ("zoom_out", "缩小", "Zoom out"),
    (
        "copy_to_folder1",
        "复制到 图片文件夹",
        "Copy to image folder",
    ),
    (
        "move_to_folder1",
        "移动到 图片文件夹",
        "Move to image folder",
    ),
    (
        "copy_to_folder2",
        "复制到 图片文件夹 2",
        "Copy to image folder 2",
    ),
    (
        "next_container",
        "下一个压缩包/文件夹",
        "Next archive/folder",
    ),
    (
        "prev_container",
        "上一个压缩包/文件夹",
        "Previous archive/folder",
    ),
    ("context_menu", "上下文菜单", "Context menu"),
    (
        "toggle_info",
        "显示文件的EXIF信息",
        "Show the file's EXIF info",
    ),
];

/// 映射可以选的动作：空（键盘为"还没选"，鼠标为"无操作"）或下拉框里的动作。
fn mappable(action: &str) -> bool {
    action.is_empty() || MAPPABLE.iter().any(|(id, ..)| *id == action)
}

fn mappable_id(action: &str) -> Option<&'static str> {
    MAPPABLE
        .iter()
        .find(|(id, ..)| *id == action)
        .map(|(id, ..)| *id)
}

/// 键盘映射里生效的行（勾选了，按键和动作都设了），按行的顺序：(按键, 动作)。
pub fn key_bindings(values: &Settings) -> Vec<(actions::Chord, &'static str)> {
    let Some(Value::KeyMappings(rows)) = values.get(KEY_MAPPINGS) else {
        return Vec::new();
    };
    rows.iter()
        .filter(|row| row.enabled)
        .filter_map(|row| Some((actions::Chord::parse(&row.key)?, mappable_id(&row.action)?)))
        .collect()
}

/// 鼠标的按键和滚轮（设置"鼠标"页）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseInput {
    Left,
    Middle,
    Right,
    /// 侧键 1（后退键，"X 键"）。
    Back,
    /// 侧键 2（前进键，"Y 键"）。
    Forward,
    WheelUp,
    WheelDown,
}

impl MouseInput {
    pub fn key(self) -> &'static str {
        match self {
            Self::Left => MOUSE_LEFT,
            Self::Middle => MOUSE_MIDDLE,
            Self::Right => MOUSE_RIGHT,
            Self::Back => MOUSE_BACK,
            Self::Forward => MOUSE_FORWARD,
            Self::WheelUp => MOUSE_WHEEL_UP,
            Self::WheelDown => MOUSE_WHEEL_DOWN,
        }
    }
}

/// 一个鼠标按键现在做什么。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseBinding {
    /// 没有勾选：默认行为（ACTIONS"鼠标与触控板的默认动作"）。
    Default,
    /// 勾选了，执行这个动作。
    Action(&'static str),
    /// 勾选了"无操作"：屏蔽这个按键。
    Nothing,
}

pub fn mouse_binding(values: &Settings, input: MouseInput) -> MouseBinding {
    match values.get(input.key()) {
        Some(Value::Mapping {
            enabled: true,
            action,
        }) => mappable_id(action).map_or(MouseBinding::Nothing, MouseBinding::Action),
        _ => MouseBinding::Default,
    }
}

// ---- 设置窗口的页面数据 ----

/// 一项在设置窗口里的样子：一张卡片里的一行，左边标题（下面可以有一行灰色说明），右边控件。
#[derive(Clone, Debug, PartialEq)]
pub struct ItemView {
    pub key: &'static str,
    pub label: String,
    /// 标题下面的灰色说明；没有时为空。
    pub detail: String,
    /// 标题后面灰色显示的快捷键；没有时为空。
    pub shortcut: String,
    /// 在第几张卡片里（页内从 0 数，[`PageContent::sections`] 的下标）。
    pub section: usize,
    /// 缩进层级（从属于上一项的行）。
    pub indent: u8,
    pub enabled: bool,
    /// 开关是翻过来说的（设置文件里存的是"不显示……"）。界面不用管，只给自检把存的值换成开关的状态。
    pub inverted: bool,
    pub control: Control,
}

/// 设置窗口一页的内容。
#[derive(Clone, Debug, PartialEq)]
pub struct PageContent {
    /// 各张卡片上面的组名（没有组名的卡片为空）。
    pub sections: Vec<String>,
    pub items: Vec<ItemView>,
}

/// 选项：(ID, 文字)。
pub type OptionView = (String, String);

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Toggle(bool),
    Choice {
        options: Vec<OptionView>,
        selected: String,
    },
    Number {
        text: String,
        unit: String,
    },
    Color {
        value: u32,
        presets: Vec<u32>,
        /// "选择…"按钮的文字。
        choose: String,
    },
    Path {
        text: String,
        buttons: Vec<OptionView>,
    },
    /// 一排按钮（在这一行的右边），点击即执行。
    Buttons(Vec<OptionView>),
    /// 一段说明文字（文字就是标签），占卡片里的一整行。
    Note,
    /// 鼠标映射：一个下拉框。首项"默认"（ID 为 [`MAPPING_DEFAULT`]），其次"无操作"（ID 为空），其余是动作。
    Mapping {
        selected: String,
        options: Vec<OptionView>,
    },
    /// 自己设的快捷键：只列用过的行（勾选了，或设了按键、动作）。
    KeyMappings {
        rows: Vec<KeyMappingView>,
        options: Vec<OptionView>,
        /// "添加快捷键"用的空行（勾选它就出现在列表里）；没有空行时为空（不会出现：末尾总留一行空行）。
        add_row: Option<usize>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyMappingView {
    /// 在映射表里是第几行（从 0 开始），改这一行时传回。
    pub row: usize,
    pub enabled: bool,
    /// 按键的显示文字；还没设时为空。
    pub key_label: String,
    pub action: String,
    /// 这一行的提示：盖掉了某个默认键位时为"将替代：某动作"，与前面某行重复时为"与第 N 行相同"；否则为空。
    pub note: String,
}

fn options_view(options: &[Opt], lang: Lang) -> Vec<OptionView> {
    options
        .iter()
        .map(|o| (o.id.to_string(), pick(lang, o.zh, o.en).to_string()))
        .collect()
}

fn buttons_view(buttons: &[ButtonDef], lang: Lang) -> Vec<OptionView> {
    buttons
        .iter()
        .map(|b| (b.id.to_string(), pick(lang, b.zh, b.en).to_string()))
        .collect()
}

/// 映射下拉框的选项：键盘的首项是空白（未选择）；鼠标的首项是"默认"（照常用原来的功能），
/// 其次"无操作"（屏蔽这个按键）。其余为 [`MAPPABLE`]。
fn action_options(mouse: bool, lang: Lang) -> Vec<OptionView> {
    let mut options = Vec::new();
    if mouse {
        options.push((
            MAPPING_DEFAULT.to_string(),
            tr!(lang, "默认", "Default").to_string(),
        ));
        options.push((String::new(), tr!(lang, "无操作", "No action").to_string()));
    } else {
        options.push((String::new(), String::new()));
    }
    options.extend(
        MAPPABLE
            .iter()
            .map(|&(id, zh, en)| (id.to_string(), pick(lang, zh, en).to_string())),
    );
    options
}

/// 映射表各行的提示（[`KeyMappingView::note`]）。
fn key_row_notes(rows: &[KeyMapping], platform: Platform, lang: Lang) -> Vec<String> {
    let mut seen: Vec<(actions::Chord, usize)> = Vec::new();
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let Some(chord) = actions::Chord::parse(&row.key).filter(|_| row.enabled) else {
                return String::new();
            };
            if let Some((_, first)) = seen.iter().find(|(c, _)| *c == chord) {
                let first = first + 1;
                return tr!(lang, "与第 {first} 行相同", "Same key as row {first}")
                    .replace("{first}", &first.to_string());
            }
            seen.push((chord.clone(), i));
            match actions::resolve(platform, &chord) {
                Some(def) if def.id != row.action => {
                    let title = def.title(platform, lang);
                    tr!(lang, "将替代：{title}", "Replaces: {title}").replace("{title}", title)
                }
                _ => String::new(),
            }
        })
        .collect()
}

/// 键盘设置里热键旁的小按钮列出的按键：首项"清除"，其余为不方便直接按出的键（ID 为按键名称，文字按平台）。
pub fn special_key_options(platform: Platform, lang: Lang) -> Vec<OptionView> {
    let mut options = vec![(String::new(), tr!(lang, "清除", "Clear").to_string())];
    options.extend(actions::SPECIAL_KEYS.iter().filter_map(|key| {
        let chord = actions::Chord::new(key, 0)?;
        Some((key.to_string(), chord.label(platform)))
    }));
    options
}

/// 一页在设置窗口里的各项（按草稿的值）。
pub fn page_view(page: Page, values: &Settings, platform: Platform, lang: Lang) -> Vec<ItemView> {
    page_content(page, values, platform, lang).items
}

/// 同 [`page_view`]，“程序是不是用安装程序装的”由调用方给（测试用）。
pub fn page_view_with(
    page: Page,
    values: &Settings,
    platform: Platform,
    lang: Lang,
    installed: bool,
) -> Vec<ItemView> {
    page_content_with(page, values, platform, lang, installed).items
}

/// 一页在设置窗口里的内容：分成几张卡片，各项和它在哪张卡片里。
pub fn page_content(page: Page, values: &Settings, platform: Platform, lang: Lang) -> PageContent {
    page_content_with(page, values, platform, lang, windows_installed())
}

/// 同 [`page_content`]，“程序是不是用安装程序装的”由调用方给（测试用）。
///
/// 组名（[`Kind::Heading`]）开始一张新卡片，第一个组名之前的项在一张没有组名的卡片里。组名下面这个平台
/// 一项也没有时，这个组名不出现。
pub fn page_content_with(
    page: Page,
    values: &Settings,
    platform: Platform,
    lang: Lang,
    installed: bool,
) -> PageContent {
    let mut sections: Vec<String> = Vec::new();
    let mut items = Vec::new();
    let mut caption: Option<String> = None;
    for def in DEFS
        .iter()
        .filter(|d| d.page == Some(page) && d.shown(platform, installed))
    {
        if matches!(def.kind, Kind::Heading) {
            caption = Some(def.label(platform, lang));
            continue;
        }
        if caption.is_some() || sections.is_empty() {
            sections.push(caption.take().unwrap_or_default());
        }
        items.push(ItemView {
            key: def.key,
            label: def.label(platform, lang),
            detail: def.detail_text(platform, lang),
            shortcut: def.shortcut_label(platform),
            section: sections.len() - 1,
            indent: def.indent,
            enabled: item_enabled_with(def, values, platform, installed),
            inverted: def.invert,
            control: control_view(def, values, platform, lang),
        });
    }
    PageContent { sections, items }
}

fn control_view(def: &Def, values: &Settings, platform: Platform, lang: Lang) -> Control {
    let value = values.get(def.key);
    match def.kind {
        Kind::Toggle => Control::Toggle(values.bool(def.key) != def.invert),
        Kind::Choice(options) => Control::Choice {
            options: options_view(options, lang),
            selected: values.choice(def.key),
        },
        Kind::Number { unit, .. } => Control::Number {
            text: match value {
                Some(Value::Invalid(text)) => text.clone(),
                _ => values.int(def.key).to_string(),
            },
            // MB、% 各种语言都一样；“张”要翻译。
            unit: match unit {
                "张" => tr!(lang, "张", "images").to_string(),
                other => other.to_string(),
            },
        },
        Kind::Color(presets) => Control::Color {
            value: values.color(def.key),
            presets: presets.to_vec(),
            choose: tr!(lang, "选取…", "Choose…").to_string(),
        },
        Kind::Path(buttons) => Control::Path {
            text: values.text(def.key),
            buttons: buttons_view(buttons, lang),
        },
        Kind::Buttons(buttons) => Control::Buttons(buttons_view(buttons, lang)),
        Kind::Note | Kind::Heading => Control::Note,
        Kind::Mapping => {
            let selected = match value {
                Some(Value::Mapping {
                    enabled: true,
                    action,
                }) => action.clone(),
                _ => MAPPING_DEFAULT.to_string(),
            };
            Control::Mapping {
                selected,
                options: action_options(true, lang),
            }
        }
        Kind::KeyMappings => {
            let rows = match value {
                Some(Value::KeyMappings(rows)) => rows.clone(),
                _ => Vec::new(),
            };
            let notes = key_row_notes(&rows, platform, lang);
            Control::KeyMappings {
                rows: rows
                    .iter()
                    .zip(notes)
                    .enumerate()
                    .filter(|(_, (row, _))| !row.is_empty())
                    .map(|(i, (row, note))| KeyMappingView {
                        row: i,
                        enabled: row.enabled,
                        key_label: actions::Chord::parse(&row.key)
                            .map(|c| c.label(platform))
                            .unwrap_or_default(),
                        action: row.action.clone(),
                        note,
                    })
                    .collect(),
                options: action_options(false, lang),
                add_row: rows.iter().position(KeyMapping::is_empty),
            }
        }
    }
}

/// 设置窗口里的按钮要界面做的事（点击即执行，不属于草稿）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonCommand {
    /// 把本程序设为支持的图片类型的默认程序（macOS"设为默认看图软件"）。
    SetDefaultViewer,
    /// 在访达或文件管理器中打开文件夹。
    OpenFolder { path: String },
    /// 选文件夹，选好后把路径写进这一项的草稿。
    ChooseFolder { key: String },
    /// 选文件（背景图片），选好后写进草稿。
    ChooseFile { key: String },
    /// 打开系统取色器，选好后写进草稿。
    ChooseColor { key: String },
    /// 选程序（图像编辑器）：macOS 选 .app，Linux 选可执行文件；选好后写进草稿。
    ChooseApp { key: String },
    /// 核心已经改了草稿（如图像编辑器的"预设"）：界面重新显示这一页。
    Refresh,
    /// 设置 › 书签的"删除"：界面先确认，再调用 `clear_archive_positions`。
    ConfirmClearArchivePositions,
    /// Linux 格式关联页的"恢复原来的默认程序"：换回"设为默认看图软件"之前各类型的默认程序。
    RestoreDefaultViewer,
    /// Linux 的 Flatpak 版：在用户目录登记用 flatpak run 调用 Anyview 的缩略图生成器。
    RegisterThumbnailer,
    /// Linux 的 Flatpak 版：删掉上面登记的缩略图生成器。
    UnregisterThumbnailer,
    /// Windows 格式关联页的“格式关联”：打开系统设置的“默认应用”。
    OpenDefaultApps,
    /// Windows 格式关联页的“经典方法”：打开系统里按程序逐个扩展名选默认程序的那一页。
    OpenAssociationsByApp,
}

/// 设置窗口显不显示这一项，看程序是不是在沙盒（Linux 的 Flatpak 版）里：
/// - 图像编辑器那两行只在沙盒外显示：沙盒里看不到系统装的程序，“用图像编辑器打开”改用系统的“打开方式”对话框；
/// - 文件管理器缩略图的说明和按钮只在沙盒里：安装包自己带缩略图生成器，不用登记。
pub fn visible_with_sandbox(key: &str, sandboxed: bool) -> bool {
    match key {
        "misc.editor_heading" | "misc.editor_note" | EDITOR => !sandboxed,
        FORMAT_NOTE_FLATPAK_THUMBNAILS | FORMAT_BUTTONS_FLATPAK_THUMBNAILS => sandboxed,
        _ => true,
    }
}

/// 图像编辑器的"预设"：macOS 是"预览"；Linux 是装了的 GIMP、Krita、Pinta 中的第一个；
/// 都没有时为空。
pub fn preset_editor(platform: Platform) -> String {
    match platform {
        Platform::Macos => "/System/Applications/Preview.app".to_string(),
        Platform::Windows => "mspaint.exe".to_string(),
        Platform::Linux => ["gimp", "krita", "pinta"]
            .iter()
            .find_map(|name| {
                std::env::var_os("PATH").and_then(|paths| {
                    std::env::split_paths(&paths)
                        .map(|dir| dir.join(name))
                        .find(|p| p.is_file())
                })
            })
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    }
}

/// 设置窗口里的按钮：哪一项的哪个按钮。项不可用时为空。
pub fn button_command(
    key: &str,
    button: &str,
    values: &Settings,
    platform: Platform,
) -> Option<ButtonCommand> {
    let def = find(key)?;
    if !def.visible_on(platform) || !item_enabled(def, values, platform) {
        return None;
    }
    match (def.kind, button) {
        (Kind::Buttons(_), "set_default") => Some(ButtonCommand::SetDefaultViewer),
        (Kind::Buttons(_), "restore_default") => Some(ButtonCommand::RestoreDefaultViewer),
        (Kind::Buttons(_), "register_thumbnailer") => Some(ButtonCommand::RegisterThumbnailer),
        (Kind::Buttons(_), "unregister_thumbnailer") => Some(ButtonCommand::UnregisterThumbnailer),
        (Kind::Buttons(_), "associate") => Some(ButtonCommand::OpenDefaultApps),
        (Kind::Buttons(_), "classic") => Some(ButtonCommand::OpenAssociationsByApp),
        (Kind::Buttons(_), "clear") => Some(ButtonCommand::ConfirmClearArchivePositions),
        (Kind::Path(_), "browse") if def.key == EDITOR => Some(ButtonCommand::ChooseApp {
            key: def.key.to_string(),
        }),
        (Kind::Buttons(_), "open_finder") => Some(ButtonCommand::OpenFolder {
            path: pictures_folder(),
        }),
        (Kind::Color(_), "choose") => Some(ButtonCommand::ChooseColor {
            key: def.key.to_string(),
        }),
        (Kind::Path(_), "browse") if def.key == BACKGROUND_IMAGE_PATH => {
            Some(ButtonCommand::ChooseFile {
                key: def.key.to_string(),
            })
        }
        (Kind::Path(_), "browse") => Some(ButtonCommand::ChooseFolder {
            key: def.key.to_string(),
        }),
        (Kind::Path(_), "open") => {
            let text = values.text(def.key);
            let path = PathBuf::from(&text);
            // 背景图片的"打开文件夹"打开图片所在的文件夹。
            let folder = if path.is_file() {
                path.parent().map(Path::to_path_buf).unwrap_or(path)
            } else {
                path
            };
            (!text.is_empty()).then(|| ButtonCommand::OpenFolder {
                path: folder.display().to_string(),
            })
        }
        _ => None,
    }
}

/// 要翻译的文字：设置窗口各页的名字和说明、各项的标签、说明、选项、按钮，映射的动作，
/// (中文, 英文, 用在哪里)。
#[cfg(test)]
pub(crate) fn texts() -> Vec<(&'static str, &'static str, String)> {
    let mut out = Vec::new();
    for page in PAGES {
        let place = format!("settings page ({})", page.id());
        out.push((page.title(Lang::ZhHans), page.title(Lang::En), place.clone()));
        out.push((page.lead(Lang::ZhHans), page.lead(Lang::En), place));
    }
    for def in DEFS.iter().filter(|d| d.page.is_some()) {
        let place = |what: &str| format!("settings {what} ({})", def.key);
        out.push((def.zh, def.en, place("label")));
        if let Some((_, zh, en)) = def.alt {
            out.push((zh, en, place("label")));
        }
        if let Some((zh, en)) = def.detail {
            out.push((zh, en, place("detail")));
        }
        if let Some((_, zh, en)) = def.detail_alt {
            out.push((zh, en, place("detail")));
        }
        match def.kind {
            Kind::Choice(options) => {
                for o in options {
                    out.push((o.zh, o.en, place("option")));
                }
            }
            Kind::Path(buttons) | Kind::Buttons(buttons) => {
                for b in buttons {
                    out.push((b.zh, b.en, place("button")));
                }
            }
            _ => {}
        }
    }
    for &(id, zh, en) in MAPPABLE {
        out.push((zh, en, format!("mappable action ({id})")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只在 macOS、Windows 有的几项（界面按设置表里的平台决定生效与否，api.rs 的 ui_settings）：Windows 上显示、做好了，
    /// Linux 上不显示（以前核心写死成只有 macOS，Windows 上这几项都不起作用）。
    #[test]
    fn window_items_are_ready_on_windows() {
        for key in [SHOW_TITLE_BAR, TITLE_FILENAME, ALWAYS_ON_TOP, SNAP_WINDOW] {
            let def = find(key).unwrap();
            for platform in [Platform::Macos, Platform::Windows] {
                assert!(def.visible_on(platform) && def.ready_on(platform), "{key} {platform:?}");
            }
            assert!(!def.visible_on(Platform::Linux), "{key}");
        }
    }


    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "anyview-settings-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn definitions_are_consistent() {
        let mut keys = std::collections::HashSet::new();
        for def in DEFS {
            assert!(keys.insert(def.key), "重复的键 {}", def.key);
            let (section, name) = def.file_key();
            assert!(!name.is_empty(), "{}", def.key);
            match def.page {
                Some(page) => assert_eq!(section, page.id(), "{} 不在所在页的节里", def.key),
                None => assert_eq!(section, "state", "{}", def.key),
            }
            assert_eq!(
                def.kind.has_value(),
                def.initial.value().is_some(),
                "{} 的默认值与种类不符",
                def.key
            );
            assert_eq!(
                def.ready & !def.platforms,
                0,
                "{} 在不显示的平台上标了已实现",
                def.key
            );
            if let Kind::Choice(options) = def.kind {
                assert!(!options.is_empty());
                let Initial::Choice(id) = def.initial else {
                    panic!("{}", def.key)
                };
                assert!(
                    options.iter().any(|o| o.id == id),
                    "{} 的默认选项不存在",
                    def.key
                );
            }
            if let Some((key, _)) = def.requires {
                let target = find(key).unwrap_or_else(|| panic!("{} 依赖的 {key} 不存在", def.key));
                assert!(matches!(target.kind, Kind::Toggle | Kind::Mapping));
            }
            if def.page.is_some()
                && !matches!(
                    def.kind,
                    Kind::Buttons(_) | Kind::Path(_) | Kind::KeyMappings
                )
                && def.key != LANGUAGE
            {
                assert!(
                    !def.zh.is_empty() && !def.en.is_empty(),
                    "{} 没有标签",
                    def.key
                );
            }
        }
        for key in [
            NO_RECENT,
            CONFIRM_DELETE,
            CORNER_NOTICES,
            ALWAYS_ON_TOP,
            ESC_QUITS,
            AT_BOUNDARY,
            SAVE_ZOOM_STATE,
            HIDE_CONTROL_BAR,
            HIDE_SIDE_ARROWS,
            HIDE_ZOOM_INFO,
            HIDE_TOOLTIPS,
            SMALL_INFO_FONT,
            BACKGROUND_COLOR,
            EXIF_ROTATE,
            STRONG_SHARPEN,
            PREFETCH,
            CACHE_MB,
            TOP_FIRST,
            FAST_PREVIEW,
            FILE_ICC,
            ALPHA_BACKGROUND,
            INTERPOLATION,
            FORMAT_BUTTONS_MAC,
            DOUBLE_CLICK_MAXIMIZE,
            KEY_MAPPINGS,
            MOUSE_LEFT,
            CLICK_SIDES,
            MOUSE_MIDDLE,
            MOUSE_RIGHT,
            MOUSE_BACK,
            MOUSE_FORWARD,
            MOUSE_WHEEL_UP,
            MOUSE_WHEEL_DOWN,
            FIT_WINDOW,
            LANGUAGE,
            STATE_SORT,
            STATE_SORT_DESC,
            STATE_INFO,
            STATE_BAR_LOCKED,
            STATE_EDGE,
            STATE_MODE,
            STATE_STRETCH,
            STATE_ZOOM_LOCK,
            STATE_ZOOM,
        ] {
            assert!(find(key).is_some(), "{key} 不在 DEFS 里");
        }
    }

    #[test]
    fn defaults_follow_the_design_images() {
        let s = Settings::defaults();
        assert!(s.bool(CONFIRM_DELETE));
        assert!(s.bool(CORNER_NOTICES));
        assert!(s.bool(ESC_QUITS));
        // 到达首尾时的默认行为：循环回到首张。
        assert_eq!(s.boundary(), Boundary::Loop);
        assert_eq!(s.archive_boundary(), Boundary::Loop);
        assert_eq!(s.color(BACKGROUND_COLOR), 0x202020);
        // 默认缓存上限 1 GB、预读 5 张。
        assert_eq!(s.int(CACHE_MB), 1024);
        assert_eq!(s.int(PREFETCH_COUNT), 5);
        assert!(s.bool(EXIF_ROTATE) && s.bool(PREFETCH) && s.bool(TOP_FIRST));
        assert!(s.bool(FAST_PREVIEW) && s.bool(FILE_ICC));
        assert!(!s.bool(STRONG_SHARPEN));
        assert_eq!(s.alpha_background(), AlphaBackground::Grid);
        assert_eq!(s.choice(INTERPOLATION), "bilinear");
        assert!(s.bool(DOUBLE_CLICK_MAXIMIZE) && s.bool(FIT_WINDOW));
        assert_eq!(s.lang(Lang::En), Lang::En);
        assert_eq!(s.lang(Lang::ZhHans), Lang::ZhHans);
        assert_eq!(s.choice(STATE_EDGE), "soften");
    }

    /// Windows：上下文菜单页装好后才有；格式关联的两个按钮没装时灰显，并多一行说明。
    /// 这些项在设置文件里照常读写，与装没装无关。
    #[test]
    fn windows_integration_items_depend_on_being_installed() {
        let s = Settings::defaults();
        let win = Platform::Windows;
        assert!(!visible_pages_with(win, false).contains(&Page::ContextMenu));
        assert!(visible_pages_with(win, true).contains(&Page::ContextMenu));
        for platform in [Platform::Macos, Platform::Linux] {
            assert!(!visible_pages_with(platform, true).contains(&Page::ContextMenu));
        }
        let keys = |installed| -> Vec<(&'static str, bool)> {
            page_view_with(Page::Formats, &s, win, Lang::ZhHans, installed)
                .iter()
                .map(|i| (i.key, i.enabled))
                .collect()
        };
        assert_eq!(
            keys(false),
            [
                ("formats.buttons_windows", false),
                ("formats.note_windows_not_installed", true),
            ]
        );
        assert_eq!(
            keys(true),
            [
                ("formats.buttons_windows", true),
                ("formats.note_windows_thumbnails", true),
            ]
        );
        // 上下文菜单页：主开关默认关，“显示软件图标”跟着灰显；缩略图的四个开关默认开。
        let page = page_view_with(Page::ContextMenu, &s, win, Lang::ZhHans, true);
        let item = |key: &str| page.iter().find(|i| i.key == key).unwrap();
        assert_eq!(item(CONTEXT_MENU).control, Control::Toggle(false));
        assert!(!item(CONTEXT_MENU_ICON).enabled);
        for key in [THUMB_WEBP, THUMB_PSD, THUMB_PSB, THUMB_OTHER] {
            assert_eq!(item(key).control, Control::Toggle(true), "{key}");
            assert!(item(key).enabled);
        }
        let mut on = s.clone();
        on.set(CONTEXT_MENU, Value::Bool(true));
        let page = page_view_with(Page::ContextMenu, &on, win, Lang::ZhHans, true);
        assert!(page.iter().find(|i| i.key == CONTEXT_MENU_ICON).unwrap().enabled);
        // 写进设置文件、读回来（没装时也一样）。
        let dir = std::env::temp_dir().join(format!("anyview-settings-context-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.toml");
        on.set(THUMB_PSD, Value::Bool(false));
        save(&file, &to_table(&on, &toml::Table::new(), win)).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("[context_menu]"), "{text}");
        let (back, _) = read_file(&file).unwrap();
        assert!(back.bool(CONTEXT_MENU) && !back.bool(THUMB_PSD) && back.bool(THUMB_WEBP));
        std::fs::remove_dir_all(&dir).unwrap();
        // 别的系统的设置文件里不写这一节。
        save(&dir.with_extension("mac.toml"), &to_table(&on, &toml::Table::new(), Platform::Macos)).unwrap();
        let mac = std::fs::read_to_string(dir.with_extension("mac.toml")).unwrap();
        assert!(!mac.contains("context_menu"), "{mac}");
        std::fs::remove_file(dir.with_extension("mac.toml")).unwrap();
    }

    #[test]
    fn pages_and_items_depend_on_the_platform() {
        let mac = visible_pages(Platform::Macos);
        assert!(!mac.contains(&Page::ContextMenu));
        assert_eq!(mac.first(), Some(&Page::General));
        assert_eq!(mac.last(), Some(&Page::Language));
        let s = Settings::defaults();
        let general = page_view(Page::General, &s, Platform::Macos, Lang::ZhHans);
        let top = general.iter().find(|i| i.key == ALWAYS_ON_TOP).unwrap();
        assert_eq!(top.label, "总在最前");
        assert_eq!(top.shortcut, "⇧⌘A");
        assert!(top.enabled);
        assert!(
            page_view(Page::General, &s, Platform::Linux, Lang::ZhHans)
                .iter()
                .all(|i| i.key != ALWAYS_ON_TOP)
        );
        let windows = page_view(Page::General, &s, Platform::Windows, Lang::ZhHans);
        assert_eq!(
            windows
                .iter()
                .find(|i| i.key == ALWAYS_ON_TOP)
                .unwrap()
                .shortcut,
            "Ctrl+A"
        );
        // 没实现的项灰显；已实现的可用。
        let updates = general
            .iter()
            .find(|i| i.key == "general.check_updates")
            .unwrap();
        assert!(!updates.enabled);
        // 窗口这一组：吸附边缘、显示标题栏只在 macOS（Linux 由系统负责）；
        // "用当前窗口"各端都有，macOS 写作"从访达打开"。
        let find = |items: &[ItemView], key: &str| items.iter().find(|i| i.key == key).cloned();
        assert!(find(&general, SNAP_WINDOW).unwrap().enabled);
        let reuse = find(&general, SINGLE_INSTANCE).unwrap();
        assert!(reuse.enabled);
        assert_eq!(reuse.label, "从访达打开图片时用当前窗口");
        let browse = page_view(Page::Browse, &s, Platform::Macos, Lang::ZhHans);
        assert!(find(&browse, SHOW_TITLE_BAR).unwrap().enabled);
        let linux = page_view(Page::General, &s, Platform::Linux, Lang::ZhHans);
        assert!(find(&linux, SNAP_WINDOW).is_none());
        assert_eq!(
            find(&linux, SINGLE_INSTANCE).unwrap().label,
            "从文件管理器打开图片时用当前窗口"
        );
        let linux_browse = page_view(Page::Browse, &s, Platform::Linux, Lang::ZhHans);
        assert!(find(&linux_browse, SHOW_TITLE_BAR).is_none());
        // Windows 上窗口这一组设置全部可用；同进程支持多窗口，不显示"多窗口共享设置"。
        assert!(find(&windows, SNAP_WINDOW).unwrap().enabled);
        assert!(find(&windows, SINGLE_INSTANCE).unwrap().enabled);
        assert!(find(&windows, ALWAYS_ON_TOP).unwrap().enabled);
        let windows_browse = page_view(Page::Browse, &s, Platform::Windows, Lang::ZhHans);
        assert!(find(&windows_browse, SHOW_TITLE_BAR).unwrap().enabled);
        assert!(find(&windows_browse, TITLE_FILENAME).unwrap().enabled);
        let windows_bookmarks = page_view(Page::Bookmarks, &s, Platform::Windows, Lang::ZhHans);
        assert!(find(&windows_bookmarks, SHARE_SETTINGS).is_none());
        let image = page_view(Page::Image, &s, Platform::Macos, Lang::En);
        assert!(image.iter().all(|i| i.key != MONITOR_ICC));
        let cache = image.iter().find(|i| i.key == CACHE_MB).unwrap();
        assert_eq!(
            cache.control,
            Control::Number {
                text: "1024".into(),
                unit: "MB".into()
            }
        );
        // 预读的张数：单位跟着语言。
        let count = image.iter().find(|i| i.key == PREFETCH_COUNT).unwrap();
        assert_eq!(
            count.control,
            Control::Number {
                text: "5".into(),
                unit: "images".into()
            }
        );
        let misc = page_view(Page::Misc, &s, Platform::Macos, Lang::ZhHans);
        let saver = misc
            .iter()
            .find(|i| i.key == "misc.no_screensaver")
            .unwrap();
        assert_eq!(saver.label, "看图时不让屏幕变暗或睡眠");
        // 键盘页只列用过的行（默认的 PageDown、PageUp 两行），"添加快捷键"用第一行空行。
        let keyboard = page_view(Page::Keyboard, &s, Platform::Macos, Lang::ZhHans);
        let Control::KeyMappings { rows, add_row, .. } = &keyboard[0].control else {
            panic!()
        };
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].row, rows[1].row), (0, 1));
        assert_eq!(rows[0].key_label, "⇟");
        assert_eq!(rows[0].action, "next_container");
        assert_eq!(*add_row, Some(2));
    }

    #[test]
    fn draft_changes_validation_and_reset() {
        let current = Settings::defaults();
        let mut draft = Draft::new(&current);
        assert!(!draft.is_dirty());
        assert!(draft.set(BACKGROUND_COLOR, Input::Color(0x336699), Platform::Linux));
        assert!(draft.set(CACHE_MB, Input::Text("50".into()), Platform::Linux));
        assert!(draft.is_dirty());
        let error = draft.validate(Platform::Linux).unwrap_err();
        assert_eq!(error.page, Page::Image);
        assert_eq!(error.key, CACHE_MB);
        assert_eq!(
            error.message(Platform::Linux, Lang::ZhHans),
            "缓存大小须是 100 到 4096 之间的整数。"
        );
        assert!(draft.set(CACHE_MB, Input::Text(" 512 ".into()), Platform::Linux));
        assert!(draft.validate(Platform::Linux).is_ok());
        let mut changes = draft.changes();
        changes.sort_by_key(|(k, _)| *k);
        assert_eq!(
            changes,
            vec![
                (BACKGROUND_COLOR, Value::Color(0x336699)),
                (CACHE_MB, Value::Int(512))
            ]
        );
        // 不能改的项、不存在的选项、类型不对的输入都不接受。
        assert!(!draft.set("general.snap_window", Input::Bool(true), Platform::Linux));
        assert!(!draft.set(AT_BOUNDARY, Input::Choice("bogus".into()), Platform::Linux));
        assert!(!draft.set(ESC_QUITS, Input::Text("x".into()), Platform::Linux));
        assert!(!draft.set(STATE_INFO, Input::Bool(true), Platform::Linux));
        // 重置只影响当前页。
        draft.reset_page(Page::Image, Platform::Linux);
        assert_eq!(draft.values().int(CACHE_MB), 1024);
        assert_eq!(draft.values().color(BACKGROUND_COLOR), 0x336699);
        draft.reset_page(Page::Browse, Platform::Linux);
        assert!(!draft.is_dirty());
    }

    #[test]
    fn requirements_disable_dependent_items() {
        let mut s = Settings::defaults();
        let def = find(DOUBLE_CLICK_MAXIMIZE).unwrap();
        assert!(item_enabled(def, &s, Platform::Macos));
        s.set(
            "mouse.left",
            Value::Mapping {
                enabled: true,
                action: "next_image".into(),
            },
        );
        assert!(!item_enabled(def, &s, Platform::Macos));
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("settings.toml");
        let loaded = load(&path);
        assert_eq!(loaded.status, LoadStatus::Missing);
        let mut s = loaded.settings;
        s.set(BACKGROUND_COLOR, Value::Color(0x112233));
        s.set(LANGUAGE, Value::Choice("en".into()));
        s.set(CACHE_MB, Value::Int(1024));
        s.set(STATE_SORT, Value::Choice("size".into()));
        s.set(INTERPOLATION, Value::Choice("lanczos".into()));
        // 认不出的键（更新的版本写的）保留下来。
        let mut base = toml::Table::new();
        let mut extra = toml::Table::new();
        extra.insert("future_key".into(), toml::Value::Boolean(true));
        base.insert("general".into(), toml::Value::Table(extra));
        let table = to_table(&s, &base, Platform::Linux);
        save(&path, &table).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Anyview"));
        assert!(text.contains("background_color = \"#112233\""), "{text}");
        assert!(text.contains("future_key = true"));
        // 没实现的项不写进文件。
        assert!(!text.contains("snap_window"));
        let again = load(&path);
        assert_eq!(again.status, LoadStatus::Loaded);
        assert_eq!(again.settings.color(BACKGROUND_COLOR), 0x112233);
        assert_eq!(again.settings.lang(Lang::ZhHans), Lang::En);
        assert_eq!(again.settings.int(CACHE_MB), 1024);
        assert_eq!(again.settings.choice(STATE_SORT), "size");
        assert_eq!(again.settings.choice(INTERPOLATION), "lanczos");
        assert!(
            again.table["general"]
                .as_table()
                .unwrap()
                .contains_key("future_key")
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn invalid_values_fall_back_one_by_one() {
        let dir = temp_dir("invalid");
        let path = dir.join("settings.toml");
        fs::write(
            &path,
            "version = 1\n[general]\nconfirm_delete = \"yes\"\nesc_quits = false\nat_boundary = \"jump\"\n\
             [image]\ncache_mb = 5\nprefetch = false\n[browse]\nbackground_color = \"blue\"\n",
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.status, LoadStatus::Loaded);
        let s = loaded.settings;
        assert!(s.bool(CONFIRM_DELETE), "类型不对用默认值");
        assert!(!s.bool(ESC_QUITS));
        assert_eq!(s.boundary(), Boundary::Loop);
        assert_eq!(s.int(CACHE_MB), 1024);
        assert!(!s.bool(PREFETCH));
        assert_eq!(s.color(BACKGROUND_COLOR), 0x202020);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_broken_file_is_backed_up_and_defaults_are_used() {
        let dir = temp_dir("broken");
        let path = dir.join("settings.toml");
        fs::write(&path, "version = 1\n[general\nconfirm_delete = false\n").unwrap();
        let loaded = load(&path);
        let LoadStatus::Recovered { backup, reason } = loaded.status else {
            panic!("应当恢复默认值")
        };
        assert!(!reason.is_empty());
        let backup = backup.expect("备份文件");
        assert!(backup.exists());
        assert!(!path.exists());
        assert!(
            backup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("settings-broken-")
        );
        assert!(loaded.settings.bool(CONFIRM_DELETE));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn colors_and_buttons() {
        assert_eq!(parse_color("#336699"), Some(0x336699));
        assert_eq!(parse_color("#abcdef"), Some(0xABCDEF));
        assert_eq!(parse_color("336699"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(color_text(0x0A0B0C), "#0A0B0C");
        let s = Settings::defaults();
        assert_eq!(
            button_command(FORMAT_BUTTONS_MAC, "set_default", &s, Platform::Macos),
            Some(ButtonCommand::SetDefaultViewer)
        );
        assert_eq!(
            button_command(FORMAT_BUTTONS_MAC, "set_default", &s, Platform::Linux),
            None
        );
        assert_eq!(
            button_command(BACKGROUND_COLOR, "choose", &s, Platform::Linux),
            Some(ButtonCommand::ChooseColor {
                key: BACKGROUND_COLOR.into()
            })
        );
        // 图片文件夹选文件夹，图像编辑器选程序；书签页的"删除"先确认。
        assert_eq!(
            button_command(FOLDER1, "browse", &s, Platform::Linux),
            Some(ButtonCommand::ChooseFolder {
                key: FOLDER1.into()
            })
        );
        assert_eq!(
            button_command(EDITOR, "browse", &s, Platform::Macos),
            Some(ButtonCommand::ChooseApp { key: EDITOR.into() })
        );
        assert_eq!(
            button_command("bookmarks.clear_archive", "clear", &s, Platform::Macos),
            Some(ButtonCommand::ConfirmClearArchivePositions)
        );
        // 没实现的项的按钮不执行。
        assert_eq!(
            button_command("browse.background_image_path", "browse", &s, Platform::Linux),
            None
        );
    }

    #[test]
    fn sandbox_decides_only_existing_linux_items() {
        // 沙盒里不显示编辑器那两行，沙盒外不显示缩略图的说明和按钮；都是 Linux 上有的项，别的项都显示。
        for (key, in_sandbox) in [
            ("misc.editor_heading", false),
            (EDITOR, false),
            (FORMAT_NOTE_FLATPAK_THUMBNAILS, true),
            (FORMAT_BUTTONS_FLATPAK_THUMBNAILS, true),
        ] {
            let def = find(key).expect(key);
            assert!(def.visible_on(Platform::Linux), "{key}");
            assert_eq!(visible_with_sandbox(key, true), in_sandbox, "{key}");
            assert_eq!(visible_with_sandbox(key, false), !in_sandbox, "{key}");
        }
        for key in ["misc.folder1", PAGE_SOUND, "formats.buttons_linux"] {
            assert!(visible_with_sandbox(key, true) && visible_with_sandbox(key, false), "{key}");
        }
        // 只在 Linux 上有，按钮各自对应登记、取消登记。
        let s = Settings::defaults();
        assert_eq!(
            button_command(FORMAT_BUTTONS_FLATPAK_THUMBNAILS, "register_thumbnailer", &s, Platform::Linux),
            Some(ButtonCommand::RegisterThumbnailer)
        );
        assert_eq!(
            button_command(FORMAT_BUTTONS_FLATPAK_THUMBNAILS, "unregister_thumbnailer", &s, Platform::Linux),
            Some(ButtonCommand::UnregisterThumbnailer)
        );
        assert_eq!(
            button_command(FORMAT_BUTTONS_FLATPAK_THUMBNAILS, "register_thumbnailer", &s, Platform::Macos),
            None
        );
    }

    #[test]
    fn default_paths_follow_platform_conventions() {
        if let Some(path) = default_path(Platform::Macos) {
            assert!(path.ends_with("Library/Application Support/Anyview/settings.toml"));
        }
        if let Some(path) = default_path(Platform::Linux) {
            assert!(path.ends_with("anyview/settings.toml"));
        }
    }

    fn key_rows(values: &Settings) -> Vec<KeyMapping> {
        match values.get(KEY_MAPPINGS) {
            Some(Value::KeyMappings(rows)) => rows.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// 设置窗口分组布局测试：每页按分组渲染为卡片，反向语义开关在界面上正向展示并在持久化时还原；
    /// 鼠标映射是一个下拉框，"默认"只是不勾选；键盘页能删一行，没填完的行不写进设置文件。
    #[test]
    fn redesigned_pages_have_cards_and_inverted_switches() {
        let s = Settings::defaults();
        let content = |page, platform| page_content_with(page, &s, platform, Lang::ZhHans, true);
        let general = content(Page::General, Platform::Macos);
        assert_eq!(general.sections, ["窗口", "翻到第一张或最后一张时", "其他"]);
        assert!(general.items.windows(2).all(|w| w[0].section <= w[1].section));
        assert_eq!(content(Page::General, Platform::Linux).sections.len(), 3);
        // 这个平台一项也没有的组名不出现。
        assert_eq!(
            content(Page::Formats, Platform::Macos).sections,
            ["默认看图软件", "访达里的缩略图和预览"]
        );
        assert_eq!(
            content(Page::Formats, Platform::Windows).sections,
            ["默认看图软件", "资源管理器的缩略图"]
        );
        assert_eq!(content(Page::Language, Platform::Linux).sections, [""]);
        // 每项都在有效分组卡片内。
        for page in PAGES {
            for platform in [Platform::Macos, Platform::Windows, Platform::Linux] {
                let c = content(*page, platform);
                for item in &c.items {
                    assert!(item.section < c.sections.len(), "{}", item.key);
                }
            }
        }
        // "控制栏"开关：设置文件里存的是"不显示控制栏"。
        let browse = page_view(Page::Browse, &s, Platform::Linux, Lang::ZhHans);
        let bar = browse.iter().find(|i| i.key == HIDE_CONTROL_BAR).unwrap();
        assert_eq!((bar.label.as_str(), bar.inverted), ("控制栏", true));
        assert_eq!(bar.control, Control::Toggle(true));
        let mut draft = Draft::new(&s);
        assert!(draft.set(HIDE_CONTROL_BAR, Input::Bool(false), Platform::Linux));
        assert!(draft.values().bool(HIDE_CONTROL_BAR));
        let browse = page_view(Page::Browse, draft.values(), Platform::Linux, Lang::ZhHans);
        let bar = browse.iter().find(|i| i.key == HIDE_CONTROL_BAR).unwrap();
        assert_eq!(bar.control, Control::Toggle(false));
        // "打开 RAW 照片"关掉时，下面两项灰显。
        assert!(draft.set(SKIP_RAW, Input::Bool(false), Platform::Linux));
        assert!(draft.values().bool(SKIP_RAW));
        let misc = page_view(Page::Misc, draft.values(), Platform::Linux, Lang::ZhHans);
        let enabled = |key: &str| misc.iter().find(|i| i.key == key).unwrap().enabled;
        assert!(!enabled(SKIP_RAW_WITH_JPEG) && !enabled(RAW_THUMBNAIL));
        // 鼠标：选动作就是勾选；选"默认"只是不勾选，动作留着；空是"无操作"。
        assert!(draft.set(MOUSE_BACK, Input::MappingChoice("zoom_in".into()), Platform::Linux));
        assert_eq!(mouse_binding(draft.values(), MouseInput::Back), MouseBinding::Action("zoom_in"));
        assert!(draft.set(MOUSE_BACK, Input::MappingChoice(MAPPING_DEFAULT.into()), Platform::Linux));
        assert_eq!(mouse_binding(draft.values(), MouseInput::Back), MouseBinding::Default);
        assert_eq!(
            draft.values().get(MOUSE_BACK),
            Some(&Value::Mapping { enabled: false, action: "zoom_in".into() })
        );
        assert!(draft.set(MOUSE_BACK, Input::MappingChoice(String::new()), Platform::Linux));
        assert_eq!(mouse_binding(draft.values(), MouseInput::Back), MouseBinding::Nothing);
        assert!(!draft.set(MOUSE_BACK, Input::MappingChoice("bogus".into()), Platform::Linux));
        let mouse = page_view(Page::Mouse, draft.values(), Platform::Linux, Lang::ZhHans);
        let back = mouse.iter().find(|i| i.key == MOUSE_BACK).unwrap();
        assert!(matches!(&back.control, Control::Mapping { selected, .. } if selected.is_empty()));
        // 键盘：删掉第一行；"添加"的行（勾选了、还没填）列出来，但不写进设置文件。
        let p = Platform::Linux;
        assert!(draft.set(KEY_MAPPINGS, Input::KeyRowRemove { row: 0 }, p));
        assert!(draft.set(KEY_MAPPINGS, Input::KeyRowEnabled { row: 2, on: true }, p));
        let keyboard = page_view(Page::Keyboard, draft.values(), p, Lang::ZhHans);
        let Control::KeyMappings { rows, add_row, .. } = &keyboard[0].control else {
            panic!()
        };
        assert_eq!(rows.iter().map(|r| r.row).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(rows[1].key_label, "");
        assert_eq!(*add_row, Some(0));
        let table = to_table(draft.values(), &toml::Table::new(), p);
        let written = table["keyboard"]["mappings"].as_array().unwrap();
        assert_eq!(written.len(), 1, "{written:?}");
    }

    #[test]
    fn mappable_actions_follow_the_design_dropdown() {
        assert_eq!(MAPPABLE.len(), 30);
        assert_eq!(MAPPABLE[0].0, "fullscreen");
        assert_eq!(MAPPABLE[29].0, "toggle_info");
        for (id, zh, en) in MAPPABLE {
            assert!(actions::find(id).is_some(), "{id}");
            assert!(!zh.is_empty() && !en.is_empty());
        }
        let s = Settings::defaults();
        let page = page_view(Page::Mouse, &s, Platform::Linux, Lang::ZhHans);
        let Control::Mapping { options, .. } = &page[0].control else {
            panic!()
        };
        assert_eq!(options[0], (MAPPING_DEFAULT.to_string(), "默认".to_string()));
        assert_eq!(options[1], (String::new(), "无操作".to_string()));
        assert_eq!(options[2].1, "全屏");
        let page = page_view(Page::Keyboard, &s, Platform::Linux, Lang::ZhHans);
        let Control::KeyMappings { options, .. } = &page[0].control else {
            panic!()
        };
        assert_eq!(options[0], (String::new(), String::new()));
    }

    #[test]
    fn key_rows_are_edited_padded_and_validated() {
        let mut draft = Draft::new(&Settings::defaults());
        let p = Platform::Linux;
        // 默认两行：PageDown、PageUp 换文件夹。
        let bindings = key_bindings(draft.values());
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].0, actions::Chord::parse("PageDown").unwrap());
        assert_eq!(bindings[0].1, "next_container");
        // 第 3 行：勾选、按 q、选放大。
        assert!(draft.set(KEY_MAPPINGS, Input::KeyRowEnabled { row: 2, on: true }, p));
        assert!(draft.set(
            KEY_MAPPINGS,
            Input::KeyRowKey {
                row: 2,
                key: "q".into(),
                mods: actions::CTRL
            },
            p
        ));
        // 还没选动作：确定时拦下。
        let error = draft.validate(p).unwrap_err();
        assert_eq!((error.page, error.row), (Page::Keyboard, Some(2)));
        assert_eq!(error.message(p, Lang::ZhHans), "第 3 行还没有选择操作。");
        assert!(draft.set(
            KEY_MAPPINGS,
            Input::KeyRowAction {
                row: 2,
                action: "zoom_in".into()
            },
            p
        ));
        assert!(draft.validate(p).is_ok());
        assert_eq!(key_rows(draft.values())[2].key, "Ctrl+Q");
        // 系统占用的按键、不在下拉框里的动作、不存在的行都不接受。
        assert!(!draft.set(
            KEY_MAPPINGS,
            Input::KeyRowKey {
                row: 3,
                key: "e".into(),
                mods: actions::CMD
            },
            p
        ));
        assert!(!draft.set(
            KEY_MAPPINGS,
            Input::KeyRowAction {
                row: 3,
                action: "about".into()
            },
            p
        ));
        assert!(!draft.set(KEY_MAPPINGS, Input::KeyRowEnabled { row: 99, on: true }, p));
        // 两行用同一个按键：拦下，并在后一行提示。
        assert!(draft.set(KEY_MAPPINGS, Input::KeyRowEnabled { row: 3, on: true }, p));
        assert!(draft.set(
            KEY_MAPPINGS,
            Input::KeyRowKey {
                row: 3,
                key: "PageDown".into(),
                mods: 0
            },
            p
        ));
        assert!(draft.set(
            KEY_MAPPINGS,
            Input::KeyRowAction {
                row: 3,
                action: "zoom_out".into()
            },
            p
        ));
        let error = draft.validate(p).unwrap_err();
        assert_eq!(error.row, Some(3));
        assert_eq!(
            error.message(p, Lang::ZhHans),
            "第 1 行和第 4 行用了同一个按键 PageDown。"
        );
        let page = page_view(Page::Keyboard, draft.values(), p, Lang::ZhHans);
        let Control::KeyMappings { rows, .. } = &page[0].control else {
            panic!()
        };
        assert_eq!(rows[3].note, "与第 1 行相同");
        // 盖掉默认键位时提示将替代的动作。
        assert!(draft.set(
            KEY_MAPPINGS,
            Input::KeyRowKey {
                row: 3,
                key: "Right".into(),
                mods: 0
            },
            p
        ));
        assert!(draft.validate(p).is_ok());
        let page = page_view(Page::Keyboard, draft.values(), p, Lang::ZhHans);
        let Control::KeyMappings { rows, .. } = &page[0].control else {
            panic!()
        };
        assert_eq!(rows[3].note, "将替代：下一张图片");
        assert_eq!(rows[3].key_label, "→");
        // 用到最后一行时自动多一行空行。
        for row in 4..KEY_ROWS {
            assert!(draft.set(KEY_MAPPINGS, Input::KeyRowEnabled { row, on: true }, p));
        }
        assert_eq!(key_rows(draft.values()).len(), KEY_ROWS + 1);
        // 清除按键。
        assert!(draft.set(
            KEY_MAPPINGS,
            Input::KeyRowKey {
                row: 2,
                key: String::new(),
                mods: 0
            },
            p
        ));
        assert_eq!(key_rows(draft.values())[2].key, "");
    }

    #[test]
    fn key_rows_round_trip_only_used_rows() {
        let dir = temp_dir("keys");
        let path = dir.join("settings.toml");
        let mut s = Settings::defaults();
        let mut rows = key_rows(&s);
        rows[0].enabled = false;
        rows[2] = KeyMapping {
            enabled: true,
            key: "Ctrl+Q".into(),
            action: "zoom_in".into(),
        };
        s.set(KEY_MAPPINGS, Value::KeyMappings(rows));
        save(&path, &to_table(&s, &toml::Table::new(), Platform::Linux)).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("[[keyboard.mappings]]").count(), 3, "{text}");
        let loaded = load(&path).settings;
        let rows = key_rows(&loaded);
        assert_eq!(rows.len(), KEY_ROWS);
        assert!(!rows[0].enabled && rows[0].key == "PageDown");
        assert_eq!(rows[2].key, "Ctrl+Q");
        let bindings = key_bindings(&loaded);
        assert_eq!(bindings.len(), 2, "第 1 行没勾选");
        // 全部删掉后保存：读回来是空表，不会补回默认的两行。
        let mut s = loaded;
        s.set(KEY_MAPPINGS, Value::KeyMappings(Vec::new()));
        save(&path, &to_table(&s, &toml::Table::new(), Platform::Linux)).unwrap();
        let loaded = load(&path).settings;
        assert!(key_bindings(&loaded).is_empty());
        assert_eq!(key_rows(&loaded).len(), KEY_ROWS);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn mouse_bindings_default_map_or_block() {
        let mut s = Settings::defaults();
        assert_eq!(
            mouse_binding(&s, MouseInput::WheelDown),
            MouseBinding::Default
        );
        s.set(
            MOUSE_WHEEL_DOWN,
            Value::Mapping {
                enabled: true,
                action: "zoom_out".into(),
            },
        );
        s.set(
            MOUSE_RIGHT,
            Value::Mapping {
                enabled: true,
                action: String::new(),
            },
        );
        s.set(
            MOUSE_MIDDLE,
            Value::Mapping {
                enabled: false,
                action: "quit".into(),
            },
        );
        assert_eq!(
            mouse_binding(&s, MouseInput::WheelDown),
            MouseBinding::Action("zoom_out")
        );
        assert_eq!(mouse_binding(&s, MouseInput::Right), MouseBinding::Nothing);
        assert_eq!(mouse_binding(&s, MouseInput::Middle), MouseBinding::Default);
        let options = special_key_options(Platform::Macos, Lang::ZhHans);
        assert_eq!(options[0], (String::new(), "清除".to_string()));
        assert!(options.contains(&("PageDown".to_string(), "⇟".to_string())));
    }

    /// 键盘映射里有一行认不出（新版本的动作、写坏的键）：只丢这一行，别的映射留着。
    #[test]
    fn one_bad_key_mapping_row_does_not_drop_the_others() {
        let dir = temp_dir("badrow");
        let path = dir.join("settings.toml");
        fs::write(
            &path,
            "version = 1\n[[keyboard.mappings]]\nenabled = true\nkey = \"SS\"\naction = \"next_image\"\n\
             [[keyboard.mappings]]\nenabled = true\nkey = \"J\"\naction = \"no_such_action\"\n\
             [[keyboard.mappings]]\nenabled = true\nkey = \"K\"\naction = \"prev_image\"\n",
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.status, LoadStatus::Loaded);
        let Some(Value::KeyMappings(rows)) = loaded.settings.get(KEY_MAPPINGS) else {
            panic!("应有键盘映射")
        };
        let kept: Vec<_> = rows
            .iter()
            .filter(|r| !r.action.is_empty())
            .map(|r| (r.key.as_str(), r.action.as_str(), r.enabled))
            .collect();
        // 按键认不出的那行动作还在、按键清掉了；动作认不出的那行没有了；好的那行原样。
        assert_eq!(kept, [("", "next_image", true), ("K", "prev_image", true)], "{rows:?}");
        assert_eq!(key_bindings(&loaded.settings).len(), 1);
        fs::remove_dir_all(dir).ok();
    }

    /// 设置文件暂时读不了（这里用一个同名的文件夹代替）：不改名，用默认值，提示读不了。
    #[test]
    fn an_unreadable_file_is_left_alone() {
        let dir = temp_dir("unreadable");
        let path = dir.join("settings.toml");
        fs::create_dir_all(&path).unwrap();
        let loaded = load(&path);
        assert!(loaded.unreadable);
        let LoadStatus::Recovered { backup, reason } = loaded.status else {
            panic!("应当用默认值")
        };
        assert_eq!(backup, None);
        assert!(!reason.is_empty());
        assert!(path.is_dir(), "没有被改名");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).ok();
    }

    /// 保存只盖上自己改过的项；临时文件不留下。
    #[test]
    fn overlay_keeps_what_others_changed_and_save_leaves_no_temp_file() {
        let baseline = Settings::defaults();
        let mut ours = baseline.clone();
        ours.set(CACHE_MB, Value::Int(100));
        let mut disk = baseline.clone();
        disk.set(BACKGROUND_COLOR, Value::Color(0x112233));
        disk.set(CACHE_MB, Value::Int(900));
        let merged = overlay(&disk, &ours, &baseline);
        assert_eq!(merged.int(CACHE_MB), 100, "自己改的");
        assert_eq!(merged.color(BACKGROUND_COLOR), 0x112233, "别人改的");
        let dir = temp_dir("temp");
        let path = dir.join("settings.toml");
        save(&path, &to_table(&merged, &toml::Table::new(), Platform::Linux)).unwrap();
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["settings.toml"]);
        fs::remove_dir_all(dir).ok();
    }

    /// 第 1 版的设置文件里写着的"询问"（当时的默认值）换成新的默认值；别的选择不动。
    #[test]
    fn version_1_files_get_the_new_boundary_default() {
        let dir = temp_dir("boundary-v1");
        let path = dir.join("settings.toml");
        fs::write(
            &path,
            "version = 1\n[general]\nat_boundary = \"ask\"\nat_archive_boundary = \"next_folder\"\nconfirm_delete = false\n",
        )
        .unwrap();
        let s = load(&path).settings;
        assert_eq!(s.boundary(), Boundary::Loop);
        assert_eq!(s.archive_boundary(), Boundary::NextFolder);
        assert!(!s.bool(CONFIRM_DELETE));
        // 第 2 版里的"询问"是用户选的：留着。
        fs::write(&path, "version = 2\n[general]\nat_boundary = \"ask\"\n").unwrap();
        assert_eq!(load(&path).settings.boundary(), Boundary::Ask);
        // 存下去是现在的版本。
        save(&path, &to_table(&s, &toml::Table::new(), Platform::Linux)).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains(&format!("version = {VERSION}")));
        fs::remove_dir_all(dir).ok();
    }

    /// 免安装版：程序旁边有 portable.txt 时，设置放在旁边的 data 文件夹里。
    #[test]
    fn portable_settings_live_next_to_the_program() {
        let dir = temp_dir("portable");
        assert_eq!(portable_path(&dir), None);
        fs::write(dir.join(PORTABLE_MARKER), "x").unwrap();
        assert_eq!(portable_path(&dir), Some(dir.join("data").join("settings.toml")));
        fs::remove_dir_all(dir).ok();
    }

    /// 第 2 版 → 第 3 版：写着旧默认值 300 MB 的缓存大小换成新的默认值 1024 MB，别的数照旧。
    #[test]
    fn version_2_files_get_the_new_cache_default() {
        let dir = temp_dir("cache-v2");
        let path = dir.join("settings.toml");
        fs::write(&path, "version = 2\n[image]\ncache_mb = 300\nprefetch = true\n").unwrap();
        let s = load(&path).settings;
        assert_eq!(s.int(CACHE_MB), 1024);
        assert_eq!(s.int(PREFETCH_COUNT), 5);
        fs::write(&path, "version = 2\n[image]\ncache_mb = 512\n").unwrap();
        assert_eq!(load(&path).settings.int(CACHE_MB), 512);
        // 第 3 版里的 300 是用户填的：留着。
        fs::write(&path, "version = 3\n[image]\ncache_mb = 300\n").unwrap();
        assert_eq!(load(&path).settings.int(CACHE_MB), 300);
        fs::remove_dir_all(dir).ok();
    }
}
