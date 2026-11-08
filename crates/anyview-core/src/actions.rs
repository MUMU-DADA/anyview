//! 动作目录、默认键位与菜单结构（docs/ACTIONS.md、docs/MENUS.md、docs/PLATFORMS.md"菜单栏"）。
//!
//! 每个动作有固定的 ID。右键菜单、macOS 菜单栏和键盘都用这里的数据：三端按数据生成原生菜单，
//! 按键由界面转换成平台无关的描述（[`Chord`]），再由 [`resolve`] 解析成动作。

use crate::i18n::Lang;
use crate::platform::Platform;

/// 修饰键位。`CMD` 是 macOS 的 ⌘；Windows、Linux 的 Win/Super 键留给系统，不用于默认键位。
pub const SHIFT: u32 = 1;
pub const CTRL: u32 = 2;
pub const ALT: u32 = 4;
pub const CMD: u32 = 8;

/// 有名字的按键。其余按键用它输入的字符表示（字母一律大写）。
const NAMED_KEYS: &[&str] = &[
    "Left",
    "Right",
    "Up",
    "Down",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Enter",
    "Tab",
    "Escape",
    "Delete",
    "Backspace",
    "Insert",
    "Space",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
];

/// 字母键的写法：大写。大写不是单个字符的字母（ß 的大写是 SS）用它的小写——写成 "SS" 的话存进设置文件后
/// 读不回来；开着大写锁定打出来的 ẞ 也归到 ß，同一个键对得上。
fn letter_key(c: char) -> char {
    fn single(mut chars: impl Iterator<Item = char>) -> Option<char> {
        let first = chars.next()?;
        chars.next().is_none().then_some(first)
    }
    let lower = single(c.to_lowercase()).unwrap_or(c);
    single(lower.to_uppercase()).unwrap_or(lower)
}

/// 一个按键组合。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: String,
    pub mods: u32,
}

impl Chord {
    /// 把界面传来的按键规范化：
    /// - 字母转成大写，⇧ 保留（⌘O 与 ⇧⌘O 不同）；
    /// - 数字和符号已经是按下 ⇧ 之后的字符（如 `+`、`*`），⇧ 不再单独计算，换了键盘布局也能对上；
    /// - 有名字的按键不区分大小写。
    pub fn new(key: &str, mods: u32) -> Option<Self> {
        let mods = mods & (SHIFT | CTRL | ALT | CMD);
        let mut chars = key.chars();
        let first = chars.next()?;
        if chars.next().is_none() {
            if first == ' ' {
                return Some(Self {
                    key: "Space".into(),
                    mods,
                });
            }
            if first.is_control() {
                return None;
            }
            if first.is_alphabetic() {
                return Some(Self {
                    key: letter_key(first).to_string(),
                    mods,
                });
            }
            return Some(Self {
                key: first.to_string(),
                mods: mods & !SHIFT,
            });
        }
        let named = NAMED_KEYS
            .iter()
            .find(|name| name.eq_ignore_ascii_case(key))?;
        Some(Self {
            key: (*named).to_string(),
            mods,
        })
    }

    /// 解析目录里的写法，如 `Cmd+Shift+O`、`Alt+Up`、`Ctrl++`、`+`。
    pub fn parse(spec: &str) -> Option<Self> {
        let mut mods = 0;
        let mut rest = spec;
        loop {
            let (flag, prefix) = match () {
                _ if rest.starts_with("Shift+") && rest.len() > 6 => (SHIFT, 6),
                _ if rest.starts_with("Ctrl+") && rest.len() > 5 => (CTRL, 5),
                _ if rest.starts_with("Alt+") && rest.len() > 4 => (ALT, 4),
                _ if rest.starts_with("Cmd+") && rest.len() > 4 => (CMD, 4),
                _ => break,
            };
            mods |= flag;
            rest = &rest[prefix..];
        }
        Self::new(rest, mods)
    }

    /// 写成目录里的形式，如 `Ctrl+Shift+O`（与 [`Chord::parse`] 互逆）；设置文件里的键盘映射用这种写法。
    pub fn spec(&self) -> String {
        let mut text = String::new();
        for (flag, name) in [
            (CTRL, "Ctrl+"),
            (ALT, "Alt+"),
            (SHIFT, "Shift+"),
            (CMD, "Cmd+"),
        ] {
            if self.mods & flag != 0 {
                text.push_str(name);
            }
        }
        text.push_str(&self.key);
        text
    }

    /// 菜单和提示里显示的文字：macOS 用符号（⌃⌥⇧⌘ 的顺序），其他平台用 `Ctrl+Alt+Shift+键`。
    pub fn label(&self, platform: Platform) -> String {
        let mac = platform == Platform::Macos;
        let key = match (self.key.as_str(), mac) {
            ("Left", _) => "←",
            ("Right", _) => "→",
            ("Up", _) => "↑",
            ("Down", _) => "↓",
            ("Home", true) => "↖",
            ("End", true) => "↘",
            ("PageUp", true) => "⇞",
            ("PageDown", true) => "⇟",
            ("Enter", true) => "↩",
            ("Tab", true) => "⇥",
            ("Escape", true) => "⎋",
            ("Escape", false) => "Esc",
            ("Delete", true) => "⌦",
            ("Delete", false) => "Del",
            ("Backspace", true) => "⌫",
            ("Insert", false) => "Ins",
            (other, _) => other,
        };
        let mut text = String::new();
        if mac {
            for (flag, symbol) in [(CTRL, "⌃"), (ALT, "⌥"), (SHIFT, "⇧"), (CMD, "⌘")] {
                if self.mods & flag != 0 {
                    text.push_str(symbol);
                }
            }
        } else {
            for (flag, name) in [
                (CTRL, "Ctrl+"),
                (ALT, "Alt+"),
                (SHIFT, "Shift+"),
                (CMD, "Win+"),
            ] {
                if self.mods & flag != 0 {
                    text.push_str(name);
                }
            }
        }
        text.push_str(key);
        text
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    /// 执行一次。
    Command,
    /// 勾选与取消。
    Toggle,
    /// 同组单选；旋转命令也按这种方式显示当前角度。
    Choice,
}

/// 目录中的一条动作。
#[derive(Clone, Copy, Debug)]
pub struct ActionDef {
    pub id: &'static str,
    pub kind: ActionKind,
    pub zh: &'static str,
    pub en: &'static str,
    /// macOS 上改用的平台措辞（ACTIONS"macOS 上改用平台措辞的项目"）：(中文, 英文)。
    pub mac_title: Option<(&'static str, &'static str)>,
    /// Windows 键位，Linux 与之相同。第一个显示在菜单上。
    pub win_keys: &'static [&'static str],
    pub mac_keys: &'static [&'static str],
    /// 需要当前图片；没有打开图片时灰显。
    pub needs_image: bool,
    /// 已经实现。尚未实现的动作灰显而不隐藏（见 docs/MENUS.md）。
    pub ready: bool,
}

const fn act(id: &'static str, kind: ActionKind, zh: &'static str, en: &'static str) -> ActionDef {
    ActionDef {
        id,
        kind,
        zh,
        en,
        mac_title: None,
        win_keys: &[],
        mac_keys: &[],
        needs_image: false,
        ready: false,
    }
}

impl ActionDef {
    const fn keys(mut self, win: &'static [&'static str], mac: &'static [&'static str]) -> Self {
        self.win_keys = win;
        self.mac_keys = mac;
        self
    }

    const fn mac(mut self, zh: &'static str, en: &'static str) -> Self {
        self.mac_title = Some((zh, en));
        self
    }

    const fn image(mut self) -> Self {
        self.needs_image = true;
        self
    }

    const fn ready(mut self) -> Self {
        self.ready = true;
        self
    }

    pub fn keys_for(&self, platform: Platform) -> &'static [&'static str] {
        match platform {
            Platform::Macos => self.mac_keys,
            Platform::Windows | Platform::Linux => self.win_keys,
        }
    }

    pub fn title(&self, platform: Platform, lang: Lang) -> &'static str {
        let (zh, en) = match (platform, self.mac_title) {
            (Platform::Macos, Some(pair)) => pair,
            _ => (self.zh, self.en),
        };
        crate::i18n::translate(lang, zh, en)
    }

    /// 菜单上显示的快捷键：该平台的第一个键位。
    pub fn shortcut(&self, platform: Platform) -> Option<Chord> {
        self.keys_for(platform)
            .first()
            .and_then(|spec| Chord::parse(spec))
    }
}

use ActionKind::{Choice, Command, Toggle};

/// 动作目录，顺序与 docs/ACTIONS.md 一致。最后几项只用于 macOS 菜单栏。
pub static ACTIONS: &[ActionDef] = &[
    // 文件与程序
    act("open_file", Command, "打开图片", "Open Image")
        .keys(&["F2"], &["Cmd+O"])
        .ready(),
    act("open_folder", Command, "打开文件夹", "Open Folder")
        .keys(&["F"], &["Cmd+Shift+O", "F"])
        .ready(),
    act("close_image", Command, "关闭", "Close")
        .keys(&["F4"], &[])
        .image()
        .ready(),
    // 回车改为全屏，选择图片改用 G（Gallery）。
    act("select_image", Command, "选择图片", "Select Image")
        .keys(&["G"], &["G"])
        .image()
        .ready(),
    act("reveal_file", Command, "定位文件", "Show in Folder")
        .keys(&["Ctrl+Enter"], &["Cmd+Enter"])
        .mac("在访达中显示", "Show in Finder")
        .image()
        .ready(),
    act(
        "set_wallpaper",
        Command,
        "设为桌面壁纸…",
        "Set as Desktop Background…",
    )
    .mac("设为桌面图片", "Set Desktop Picture")
    .image()
    .ready(),
    act("delete_file", Command, "删除", "Delete")
        .keys(&["Delete"], &["Cmd+Backspace"])
        .mac("移到废纸篓", "Move to Trash")
        .image()
        .ready(),
    act(
        "copy_to_folder1",
        Command,
        "复制到 图片文件夹",
        "Copy to Image Folder",
    )
    .keys(&["Insert"], &[])
    .image()
    .ready(),
    act(
        "copy_to_folder2",
        Command,
        "复制到 图片文件夹 2",
        "Copy to Image Folder 2",
    )
    .image()
    .ready(),
    act(
        "move_to_folder1",
        Command,
        "移动到 图片文件夹",
        "Move to Image Folder",
    )
    .image()
    .ready(),
    act(
        "open_in_editor",
        Command,
        "用图像编辑器打开",
        "Open in Image Editor",
    )
    .keys(&["Ctrl+E"], &["Cmd+E"])
    .image()
    .ready(),
    act("print", Command, "打印", "Print")
        .keys(&["Ctrl+P"], &["Cmd+P"])
        .image()
        .ready(),
    act(
        "copy_to_clipboard",
        Command,
        "复制到剪贴板",
        "Copy to Clipboard",
    )
    .keys(&["Ctrl+C"], &["Cmd+C"])
    .mac("拷贝", "Copy")
    .image()
    .ready(),
    act("settings", Command, "设置", "Settings")
        .keys(&["F5"], &["Cmd+,"])
        .mac("设置…", "Settings…")
        .ready(),
    act("about", Command, "关于", "About")
        .keys(&["F1"], &[])
        .mac("关于 Anyview", "About Anyview")
        .ready(),
    act("quit", Command, "退出", "Exit")
        .keys(&["X"], &["Cmd+Q"])
        .mac("退出 Anyview", "Quit Anyview")
        .ready(),
    // Esc（"按 ESC 退出"默认开启，全屏时也是）：只有一个窗口时退出程序；macOS 开着多个窗口时
    // 只关当前窗口。菜单、对话框打开时由它们先处理 Esc。
    act(
        "close_or_quit",
        Command,
        "关闭窗口（最后一个窗口时退出）",
        "Close Window (Quit at the Last One)",
    )
    .keys(&["Escape"], &["Escape"])
    .ready(),
    // 窗口与界面
    // 回车进入、退出全屏；另有各平台通用的键：Windows、Linux 的 F11，macOS 的 ⌃⌘F（菜单上显示）。
    act("fullscreen", Command, "全屏", "Full Screen")
        .keys(&["Enter", "F11"], &["Ctrl+Cmd+F", "Enter"])
        .ready(),
    act("maximize", Command, "最大化/还原", "Maximize/Restore").ready(),
    act("minimize", Command, "最小化", "Minimize")
        .keys(&[], &["Cmd+M"])
        .ready(),
    // 只有 macOS 能置顶，可用与否由核心按平台判断（AppCore::action_state）。
    act("always_on_top", Toggle, "总在最前", "Always on Top")
        .keys(&["Ctrl+A"], &["Cmd+Shift+A"])
        .ready(),
    act("context_menu", Command, "上下文菜单", "Context Menu").ready(),
    act("toggle_bar_lock", Toggle, "锁定控制栏", "Lock Control Bar")
        .keys(&["~"], &["`"])
        .ready(),
    // 动图：只在显示动图时可用。
    act("anim_toggle", Toggle, "暂停动画", "Pause Animation")
        .keys(&["Space"], &["Space"])
        .image()
        .ready(),
    act("anim_prev_frame", Command, "上一帧", "Previous Frame")
        .keys(&[","], &[","])
        .image()
        .ready(),
    act("anim_next_frame", Command, "下一帧", "Next Frame")
        .keys(&["."], &["."])
        .image()
        .ready(),
    act("toggle_info", Toggle, "EXIF 信息", "EXIF Info")
        .keys(&["Tab"], &["Cmd+I", "Tab"])
        .ready(),
    // AI 生成的图片里记的提示词、参数（P 是 Prompt）：当前图片带生成信息时才可用。
    act("show_generation_info", Command, "生成信息", "Generation Info")
        .keys(&["P"], &["P"])
        .image()
        .ready(),
    act(
        "toggle_title_filename",
        Toggle,
        "顶端菜单隐藏时显示文件名",
        "Show File Name When Title Bar Is Hidden",
    )
    .keys(&["Shift+Tab"], &["Shift+Tab"])
    .mac("标题栏隐藏时显示文件名", "Show File Name When Title Bar Is Hidden")
    .ready(),
    // 像素边缘处理
    act("edge_none", Choice, "无", "None")
        .keys(&["U"], &["U"])
        .ready(),
    act("edge_soften", Choice, "减淡", "Soften")
        .keys(&["I"], &["I"])
        .ready(),
    act("edge_soften_sharpen", Choice, "减淡+锐化", "Soften+Sharpen")
        .keys(&["S"], &["S"])
        .ready(),
    // 浏览导航
    act("next_image", Command, "下一张图片", "Next Image")
        .keys(&["Right"], &["Right"])
        .image()
        .ready(),
    act("prev_image", Command, "上一张图片", "Previous Image")
        .keys(&["Left"], &["Left"])
        .image()
        .ready(),
    act("first_image", Command, "首张图片", "First Image")
        .keys(&["Home"], &["Home"])
        .image()
        .ready(),
    act("last_image", Command, "末尾图片", "Last Image")
        .keys(&["End"], &["End"])
        .image()
        .ready(),
    act("forward_10", Command, "向下10张图片", "Forward 10 Images")
        .keys(&["Ctrl+PageDown"], &["Cmd+PageDown"])
        .image()
        .ready(),
    act("back_10", Command, "向上10张图片", "Back 10 Images")
        .keys(&["Ctrl+PageUp"], &["Cmd+PageUp"])
        .image()
        .ready(),
    act(
        "random_forward",
        Command,
        "向后随机转到某张图片",
        "Random Image",
    )
    .keys(&["Ctrl+Alt+PageDown"], &["Alt+Cmd+PageDown"])
    .image()
    .ready(),
    act(
        "random_back",
        Command,
        "向前随机转到某张图片",
        "Back from Random Image",
    )
    .keys(&["Ctrl+Alt+PageUp"], &["Alt+Cmd+PageUp"])
    .image()
    .ready(),
    act(
        "next_container",
        Command,
        "下一个 文件夹/压缩包",
        "Next Folder/Archive",
    )
    // PageDown、PageUp 由键盘设置里默认勾选的两行映射提供，取消勾选就不起作用。
    .keys(&["]"], &["]"])
    .image()
    .ready(),
    act(
        "prev_container",
        Command,
        "上一个 文件夹/压缩包",
        "Previous Folder/Archive",
    )
    .keys(&["["], &["["])
    .image()
    .ready(),
    act(
        "next_page_double",
        Command,
        "下一页（双页阅览）",
        "Next Page (Two-Page View)",
    )
    .image()
    .ready(),
    act(
        "prev_page_double",
        Command,
        "上一页（双页阅览）",
        "Previous Page (Two-Page View)",
    )
    .image()
    .ready(),
    // 插值算法
    act(
        "interp_bilinear",
        Choice,
        "Bilinear 插值（快）",
        "Bilinear (Fast)",
    )
    .ready(),
    act(
        "interp_bicubic",
        Choice,
        "Bicubic 插值（慢，高品质）",
        "Bicubic (Slow, High Quality)",
    )
    .ready(),
    act(
        "interp_lanczos",
        Choice,
        "Lanczos 插值（慢，高品质）",
        "Lanczos (Slow, High Quality)",
    )
    .ready(),
    // 图片效果
    act("invert", Toggle, "反相", "Invert")
        .keys(&["Ctrl+I"], &["Cmd+Shift+I"])
        .ready(),
    act("gamma", Toggle, "Gamma补偿", "Gamma Correction")
        .keys(&["Ctrl+G"], &["Cmd+Shift+G"])
        .ready(),
    act("flip_vertical", Toggle, "垂直翻转", "Flip Vertical")
        .keys(&["Ctrl+F"], &["Cmd+Shift+F"])
        .image()
        .ready(),
    act("flip_horizontal", Toggle, "水平翻转", "Flip Horizontal")
        .keys(&["Ctrl+M"], &["Cmd+Shift+M"])
        .image()
        .ready(),
    // 排序方式
    act("sort_name", Choice, "文件名", "Name").ready(),
    act("sort_mtime", Choice, "修改日期", "Date Modified").ready(),
    act("sort_size", Choice, "大小", "Size").ready(),
    act("sort_type", Choice, "类型", "Type").ready(),
    act("sort_desc", Toggle, "降序", "Descending").ready(),
    // 浏览方式
    act("stretch_small", Toggle, "拉伸小图", "Stretch Small Images")
        .keys(&["Z"], &["Z"])
        .ready(),
    act(
        "zoom_original",
        Choice,
        "原始大小 (100%)",
        "Original Size (100%)",
    )
    .keys(&["0"], &["Cmd+0", "0"])
    .ready(),
    act("fit_window", Choice, "适合窗口", "Fit to Window")
        .keys(&["9"], &["Cmd+9", "9"])
        .ready(),
    act("fit_width", Choice, "适合宽度", "Fit to Width")
        .keys(&["8"], &["8"])
        .ready(),
    act(
        "double_page_ltr",
        Toggle,
        "智能双页阅览 (左->右)",
        "Two-Page View (Left to Right)",
    )
    .keys(&["7"], &["7"])
    .ready(),
    act(
        "double_page_rtl",
        Toggle,
        "智能双页阅览 (左<-右)",
        "Two-Page View (Right to Left)",
    )
    .keys(&["6"], &["6"])
    .ready(),
    // 缩放工具。"=" 与 "+" 在同一个键上，不按 ⇧ 也算放大。
    act("zoom_in", Command, "放大", "Zoom In")
        .keys(&["+", "="], &["Cmd++", "+", "Cmd+=", "="])
        .image()
        .ready(),
    act("zoom_out", Command, "缩小", "Zoom Out")
        .keys(&["-"], &["Cmd+-", "-"])
        .image()
        .ready(),
    act("zoom_lock", Toggle, "锁定", "Lock Zoom")
        .keys(&["Ctrl+K"], &["Cmd+K"])
        .ready(),
    act("zoom_100", Command, "100%", "100%")
        .keys(&["*"], &["*"])
        .image()
        .ready(),
    act("zoom_in_1", Command, "放大(1%)", "Zoom In 1%")
        .keys(&["Ctrl++", "Ctrl+="], &["Alt+Cmd++", "Alt+Cmd+="])
        .image()
        .ready(),
    act("zoom_out_1", Command, "缩小(1%)", "Zoom Out 1%")
        .keys(&["Ctrl+-"], &["Alt+Cmd+-"])
        .image()
        .ready(),
    // 图像旋转
    act("rotate_none", Choice, "不旋转", "No Rotation")
        .keys(&["Alt+Up"], &["Alt+Up"])
        .image()
        .ready(),
    act(
        "rotate_ccw",
        Choice,
        "逆时针旋转 90°",
        "Rotate 90° Counterclockwise",
    )
    .keys(&["Alt+Left"], &["Cmd+L", "Alt+Left"])
    .image()
    .ready(),
    act(
        "rotate_cw",
        Choice,
        "顺时针旋转 90°",
        "Rotate 90° Clockwise",
    )
    .keys(&["Alt+Right"], &["Cmd+R", "Alt+Right"])
    .image()
    .ready(),
    act("rotate_180", Choice, "旋转 180°", "Rotate 180°")
        .keys(&["Alt+Down"], &["Alt+Down"])
        .image()
        .ready(),
    // 画面平移
    act("pan_up", Command, "上移", "Pan Up")
        .keys(&["Up"], &["Up"])
        .image()
        .ready(),
    act("pan_down", Command, "下移", "Pan Down")
        .keys(&["Down"], &["Down"])
        .image()
        .ready(),
    act("pan_left", Command, "左移", "Pan Left").image().ready(),
    act("pan_right", Command, "右移", "Pan Right")
        .image()
        .ready(),
    // 只用于 macOS 菜单栏
    act("hide_app", Command, "隐藏 Anyview", "Hide Anyview")
        .keys(&[], &["Cmd+H"])
        .ready(),
    act("hide_others", Command, "隐藏其他", "Hide Others")
        .keys(&[], &["Alt+Cmd+H"])
        .ready(),
    act("show_all", Command, "全部显示", "Show All").ready(),
    act("close_window", Command, "关闭窗口", "Close Window")
        .keys(&[], &["Cmd+W"])
        .ready(),
    act("clear_recent", Command, "清除菜单", "Clear Menu").ready(),
    // macOS 在"帮助"菜单里；Linux 没有菜单栏，放在"关于"窗口的按钮上（F1 已是"关于"）。
    act(
        "help_shortcuts",
        Command,
        "键盘快捷键",
        "Keyboard Shortcuts",
    )
    .ready(),
    act("help_formats", Command, "支持的格式", "Supported Formats").ready(),
    // 开源许可：macOS、Windows 在"帮助"菜单里；Linux、Windows 的"关于"里另有按钮。
    act("help_licenses", Command, "开源许可", "Open Source Licenses").ready(),
];

/// 按钮的悬停提示：名称加上第一个快捷键，如"打开图片 (⌘O)"（MENUS"底部控制栏"）。
pub fn tooltip(id: &str, platform: Platform, lang: Lang) -> String {
    tooltip_with(id, platform, lang, &|def| def.shortcut(platform))
}

/// 同上，快捷键由 `shortcut` 给出（按用户的键盘映射）。
pub fn tooltip_with(
    id: &str,
    platform: Platform,
    lang: Lang,
    shortcut: &dyn Fn(&ActionDef) -> Option<Chord>,
) -> String {
    let Some(def) = find(id) else {
        return String::new();
    };
    match shortcut(def) {
        Some(chord) => format!("{} ({})", def.title(platform, lang), chord.label(platform)),
        None => def.title(platform, lang).to_string(),
    }
}

pub fn find(id: &str) -> Option<&'static ActionDef> {
    ACTIONS.iter().find(|a| a.id == id)
}

/// 系统占用的按键组合，不能设成键盘映射（ACTIONS 规则 6）。macOS：切换空间、调度中心（⌃ 加方向键）、
/// 输入法（⌃空格）、聚焦搜索（⌘空格）、切换程序（⌘Tab）、截屏（⇧⌘3/4/5，按键是 # $ %）、锁屏（⌃⌘Q）。
/// Windows、Linux：带 Win/Super 键的组合、Alt+Tab、Alt+F4。
pub fn reserved(platform: Platform, chord: &Chord) -> bool {
    let key = chord.key.as_str();
    match platform {
        Platform::Macos => match chord.mods {
            CTRL => matches!(key, "Left" | "Right" | "Up" | "Down" | "Space"),
            CMD => matches!(key, "Space" | "Tab" | "#" | "$" | "%"),
            m if m == CTRL | CMD => key == "Q",
            _ => false,
        },
        Platform::Windows | Platform::Linux => {
            chord.mods & CMD != 0 || (chord.mods == ALT && matches!(key, "Tab" | "F4"))
        }
    }
}

/// 不方便直接按出的按键（笔记本键盘上要按 fn 的、功能键等），键盘设置里热键旁的小按钮列出它们。
pub const SPECIAL_KEYS: &[&str] = &[
    "PageUp",
    "PageDown",
    "Home",
    "End",
    "Insert",
    "Delete",
    "Backspace",
    "Enter",
    "Tab",
    "Space",
    "Escape",
    "Up",
    "Down",
    "Left",
    "Right",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
];

/// 按默认键位把按键解析成动作。用户的键盘映射由核心（AppCore::action_for_key）先查。
pub fn resolve(platform: Platform, chord: &Chord) -> Option<&'static ActionDef> {
    ACTIONS.iter().find(|a| {
        a.keys_for(platform)
            .iter()
            .any(|spec| Chord::parse(spec).as_ref() == Some(chord))
    })
}

/// 菜单在系统中的角色（macOS 把它们交给系统管理：服务、窗口列表、帮助搜索）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRole {
    Normal,
    App,
    Services,
    Edit,
    Window,
    Help,
    /// macOS 的"打开最近使用"：系统在其中列出最近打开的文件。
    Recent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// 右键菜单。
    Context,
    /// macOS 菜单栏（PLATFORMS"菜单栏"）；其他平台没有菜单栏。
    MenuBar,
}

/// 菜单定义中的节点。
enum Node {
    Item(&'static str),
    /// 菜单栏中改用的文字：(动作, 中文, 英文)。
    Titled(&'static str, &'static str, &'static str),
    /// 只在 macOS 上改用的文字（苹果的说法，如"进入全屏幕""缩放"）；别的平台用动作自己的名字。
    MacTitled(&'static str, &'static str, &'static str),
    Separator,
    Submenu(&'static str, &'static str, MenuRole, &'static [Node]),
    /// 只在这些平台有的几项（`settings::MAC` 等的组合），原地展开。
    On(u8, &'static [Node]),
}

use crate::settings::{MAC, WIN};
use Node::{Item, MacTitled, On, Separator, Submenu, Titled};

const NAVIGATION: &[Node] = &[
    Item("next_image"),
    Item("prev_image"),
    Item("first_image"),
    Item("last_image"),
    Item("forward_10"),
    Item("back_10"),
    Item("random_forward"),
    Item("random_back"),
    Item("next_container"),
    Item("prev_container"),
];
const INTERPOLATION: &[Node] = &[
    Item("interp_bilinear"),
    Item("interp_bicubic"),
    Item("interp_lanczos"),
];
const SORT: &[Node] = &[
    Item("sort_name"),
    Item("sort_mtime"),
    Item("sort_size"),
    Item("sort_type"),
    Separator,
    Item("sort_desc"),
];
const EDGE: &[Node] = &[
    Item("edge_none"),
    Item("edge_soften"),
    Item("edge_soften_sharpen"),
];
const ROTATION: &[Node] = &[
    Item("rotate_none"),
    Item("rotate_ccw"),
    Item("rotate_cw"),
    Item("rotate_180"),
];

const CONTEXT: &[Node] = &[
    Item("open_file"),
    Item("open_folder"),
    Item("close_image"),
    Item("select_image"),
    Item("reveal_file"),
    Separator,
    Item("edge_none"),
    Item("edge_soften"),
    Item("edge_soften_sharpen"),
    Separator,
    Submenu("浏览导航", "Navigation", MenuRole::Normal, NAVIGATION),
    Submenu("插值算法", "Interpolation", MenuRole::Normal, INTERPOLATION),
    Submenu(
        "图片效果",
        "Image Effects",
        MenuRole::Normal,
        &[
            Item("invert"),
            Item("gamma"),
            Item("flip_vertical"),
            Item("flip_horizontal"),
        ],
    ),
    Submenu("排序方式", "Sort By", MenuRole::Normal, SORT),
    Separator,
    Submenu(
        "浏览方式",
        "View Mode",
        MenuRole::Normal,
        &[
            Item("stretch_small"),
            Separator,
            Item("zoom_original"),
            Item("fit_window"),
            Item("fit_width"),
            Item("double_page_ltr"),
            Item("double_page_rtl"),
        ],
    ),
    Submenu(
        "缩放工具",
        "Zoom",
        MenuRole::Normal,
        &[
            Item("zoom_in"),
            Item("zoom_out"),
            Item("zoom_lock"),
            Item("zoom_100"),
            Item("zoom_in_1"),
            Item("zoom_out_1"),
        ],
    ),
    Submenu("图像旋转", "Rotation", MenuRole::Normal, ROTATION),
    Separator,
    Item("toggle_info"),
    Item("show_generation_info"),
    Item("set_wallpaper"),
    Item("delete_file"),
    Item("copy_to_folder1"),
    Item("open_in_editor"),
    Item("print"),
    Item("copy_to_clipboard"),
    Separator,
    Item("settings"),
    Item("about"),
    Item("quit"),
];

/// 菜单栏：macOS 的系统菜单栏，和 Windows 上"不显示菜单栏"关掉时窗口顶端的菜单栏，是同一棵树。
/// 只在一个平台有的几项用 `On` 标出：macOS 的"Anyview"菜单里的设置、关于、退出，Windows 上按那边的习惯
/// 放在"文件"和"帮助"里。
const MENU_BAR: &[Node] = &[
    On(
        MAC,
        &[Submenu(
            "Anyview",
            "Anyview",
            MenuRole::App,
            &[
                Item("about"),
                Separator,
                Item("settings"),
                Separator,
                Submenu("服务", "Services", MenuRole::Services, &[]),
                Separator,
                Item("hide_app"),
                Item("hide_others"),
                Item("show_all"),
                Separator,
                Item("quit"),
            ],
        )],
    ),
    Submenu(
        "文件",
        "File",
        MenuRole::Normal,
        &[
            Titled("open_file", "打开图片…", "Open Image…"),
            Titled("open_folder", "打开文件夹…", "Open Folder…"),
            Submenu(
                "打开最近使用",
                "Open Recent",
                MenuRole::Recent,
                &[Item("clear_recent")],
            ),
            Separator,
            Titled("select_image", "选择图片…", "Select Image…"),
            Item("reveal_file"),
            Separator,
            Titled("close_image", "关闭图片", "Close Image"),
            Item("close_window"),
            Separator,
            Item("delete_file"),
            Titled(
                "copy_to_folder1",
                "复制到图片文件夹",
                "Copy to Image Folder",
            ),
            Titled(
                "copy_to_folder2",
                "复制到图片文件夹 2",
                "Copy to Image Folder 2",
            ),
            Titled(
                "move_to_folder1",
                "移动到图片文件夹",
                "Move to Image Folder",
            ),
            Item("open_in_editor"),
            Item("set_wallpaper"),
            Separator,
            Titled("print", "打印…", "Print…"),
            On(
                WIN,
                &[Separator, Item("settings"), Separator, Item("quit")],
            ),
        ],
    ),
    Submenu("编辑", "Edit", MenuRole::Edit, &[Item("copy_to_clipboard")]),
    Submenu(
        "显示",
        "View",
        MenuRole::Normal,
        &[
            Titled("zoom_original", "原始大小", "Original Size"),
            Item("fit_window"),
            Item("fit_width"),
            Item("stretch_small"),
            Separator,
            Titled(
                "double_page_ltr",
                "智能双页阅览（左→右）",
                "Two-Page View (Left to Right)",
            ),
            Titled(
                "double_page_rtl",
                "智能双页阅览（左←右）",
                "Two-Page View (Right to Left)",
            ),
            Separator,
            Item("zoom_in"),
            Item("zoom_out"),
            Titled("zoom_in_1", "放大 1%", "Zoom In 1%"),
            Titled("zoom_out_1", "缩小 1%", "Zoom Out 1%"),
            Item("zoom_100"),
            Titled("zoom_lock", "锁定缩放", "Lock Zoom"),
            Separator,
            Titled("toggle_info", "显示图片信息", "Show Image Info"),
            Item("show_generation_info"),
            Item("toggle_bar_lock"),
            Separator,
            Item("anim_toggle"),
            Item("anim_prev_frame"),
            Item("anim_next_frame"),
            Separator,
            MacTitled("fullscreen", "进入全屏幕", "Enter Full Screen"),
        ],
    ),
    Submenu(
        "图像",
        "Image",
        MenuRole::Normal,
        &[
            Item("rotate_none"),
            Item("rotate_ccw"),
            Item("rotate_cw"),
            Item("rotate_180"),
            Separator,
            Item("flip_horizontal"),
            Item("flip_vertical"),
            Separator,
            Item("invert"),
            Titled("gamma", "Gamma 补偿", "Gamma Correction"),
            Separator,
            Submenu("像素边缘", "Pixel Edges", MenuRole::Normal, EDGE),
            Submenu("插值算法", "Interpolation", MenuRole::Normal, INTERPOLATION),
        ],
    ),
    Submenu(
        "前往",
        "Go",
        MenuRole::Normal,
        &[
            Titled("next_image", "下一张", "Next"),
            Titled("prev_image", "上一张", "Previous"),
            Titled("first_image", "首张", "First"),
            Titled("last_image", "末张", "Last"),
            Separator,
            Titled("forward_10", "向后 10 张", "Forward 10"),
            Titled("back_10", "向前 10 张", "Back 10"),
            Titled("random_forward", "随机向后", "Random"),
            Titled("random_back", "随机向前", "Back from Random"),
            Separator,
            Titled(
                "next_container",
                "下一个文件夹或压缩包",
                "Next Folder or Archive",
            ),
            Titled(
                "prev_container",
                "上一个文件夹或压缩包",
                "Previous Folder or Archive",
            ),
            Separator,
            Submenu("排序方式", "Sort By", MenuRole::Normal, SORT),
        ],
    ),
    Submenu(
        "窗口",
        "Window",
        MenuRole::Window,
        &[
            Item("minimize"),
            MacTitled("maximize", "缩放", "Zoom"),
            Item("always_on_top"),
        ],
    ),
    Submenu(
        "帮助",
        "Help",
        MenuRole::Help,
        &[
            Item("help_shortcuts"),
            Item("help_formats"),
            Item("help_licenses"),
            On(WIN, &[Separator, Item("about")]),
        ],
    ),
];

/// 菜单中的一项，按顺序排成扁平列表；子菜单用开始、结束两项括起来。
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Action {
        id: &'static str,
        title: String,
        shortcut: Option<Chord>,
        /// 显示勾选标记（开关、单选）。
        checkable: bool,
    },
    Separator,
    SubmenuStart {
        title: String,
        role: MenuRole,
    },
    SubmenuEnd,
}

/// 生成菜单数据。快捷键按所在平台的默认键位，只取第一个（PLATFORMS"菜单栏"）。
pub fn menu(kind: MenuKind, platform: Platform, lang: Lang) -> Vec<MenuEntry> {
    menu_with(kind, platform, lang, &|def| def.shortcut(platform))
}

/// 同上，快捷键由 `shortcut` 给出（核心按用户的键盘映射算出实际生效的按键，ACTIONS 规则 7）。
pub fn menu_with(
    kind: MenuKind,
    platform: Platform,
    lang: Lang,
    shortcut: &dyn Fn(&ActionDef) -> Option<Chord>,
) -> Vec<MenuEntry> {
    let nodes = match (kind, platform) {
        (MenuKind::Context, _) => CONTEXT,
        (MenuKind::MenuBar, Platform::Macos | Platform::Windows) => MENU_BAR,
        // Linux 不做菜单栏（PLATFORMS）。
        (MenuKind::MenuBar, Platform::Linux) => &[],
    };
    let mut out = Vec::new();
    push_nodes(&mut out, nodes, platform, lang, shortcut);
    out
}

fn push_nodes(
    out: &mut Vec<MenuEntry>,
    nodes: &[Node],
    platform: Platform,
    lang: Lang,
    shortcut: &dyn Fn(&ActionDef) -> Option<Chord>,
) {
    let pick = |zh: &'static str, en: &'static str| crate::i18n::translate(lang, zh, en);
    for node in nodes {
        match node {
            On(mask, children) => {
                if mask & crate::settings::platform_mask(platform) != 0 {
                    push_nodes(out, children, platform, lang, shortcut);
                }
            }
            Item(id) | Titled(id, _, _) | MacTitled(id, _, _) => {
                let def = find(id).unwrap_or_else(|| panic!("菜单引用了不存在的动作 {id}"));
                let title = match node {
                    Titled(_, zh, en) => pick(zh, en),
                    MacTitled(_, zh, en) if platform == Platform::Macos => pick(zh, en),
                    _ => def.title(platform, lang),
                };
                out.push(MenuEntry::Action {
                    id: def.id,
                    title: title.to_string(),
                    shortcut: shortcut(def),
                    checkable: def.kind != ActionKind::Command,
                });
            }
            Separator => out.push(MenuEntry::Separator),
            Submenu(zh, en, role, children) => {
                out.push(MenuEntry::SubmenuStart {
                    title: pick(zh, en).to_string(),
                    role: *role,
                });
                push_nodes(out, children, platform, lang, shortcut);
                out.push(MenuEntry::SubmenuEnd);
            }
        }
    }
}

/// 要翻译的文字：各动作的名字（含 macOS 的说法）和菜单里改用的文字，(中文, 英文, 用在哪里)。
#[cfg(test)]
pub(crate) fn texts() -> Vec<(&'static str, &'static str, String)> {
    fn walk(nodes: &[Node], out: &mut Vec<(&'static str, &'static str, String)>) {
        for node in nodes {
            match node {
                Titled(id, zh, en) => out.push((zh, en, format!("menu item ({id})"))),
                MacTitled(id, zh, en) => out.push((zh, en, format!("macOS menu item ({id})"))),
                Submenu(zh, en, _, children) => {
                    out.push((zh, en, "menu title".to_string()));
                    walk(children, out);
                }
                On(_, children) => walk(children, out),
                Item(_) | Separator => {}
            }
        }
    }
    let mut out = Vec::new();
    for def in ACTIONS {
        out.push((def.zh, def.en, format!("action ({})", def.id)));
        if let Some((zh, en)) = def.mac_title {
            out.push((zh, en, format!("macOS action ({})", def.id)));
        }
    }
    walk(CONTEXT, &mut out);
    walk(MENU_BAR, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn ids_are_unique_and_keys_parse() {
        let mut ids = HashSet::new();
        for a in ACTIONS {
            assert!(ids.insert(a.id), "重复的动作 {}", a.id);
            for platform in [Platform::Macos, Platform::Windows] {
                for spec in a.keys_for(platform) {
                    assert!(
                        Chord::parse(spec).is_some(),
                        "{} 的键位 {spec} 无法解析",
                        a.id
                    );
                }
            }
        }
    }

    #[test]
    fn no_chord_is_bound_twice_on_a_platform() {
        for platform in [Platform::Macos, Platform::Windows] {
            let mut seen: HashMap<Chord, &str> = HashMap::new();
            for a in ACTIONS {
                for spec in a.keys_for(platform) {
                    let chord = Chord::parse(spec).unwrap();
                    if let Some(other) = seen.insert(chord.clone(), a.id) {
                        panic!("{platform:?} 上 {chord:?} 同时绑定了 {other} 和 {}", a.id);
                    }
                }
            }
        }
    }

    #[test]
    fn chords_normalize_shift_for_symbols_but_not_letters() {
        assert_eq!(Chord::new("+", SHIFT), Chord::parse("+"));
        assert_eq!(Chord::new("o", CMD | SHIFT), Chord::parse("Cmd+Shift+O"));
        assert_ne!(Chord::new("o", CMD | SHIFT), Chord::parse("Cmd+O"));
        assert_eq!(Chord::new("pagedown", CTRL), Chord::parse("Ctrl+PageDown"));
        assert_eq!(Chord::new(" ", 0).unwrap().key, "Space");
        assert_eq!(Chord::new("\u{1b}", 0), None);
        assert_eq!(
            Chord::parse("Ctrl++"),
            Some(Chord {
                key: "+".into(),
                mods: CTRL
            })
        );
    }

    #[test]
    fn resolves_platform_default_keys() {
        let id = |platform, key: &str, mods| {
            resolve(platform, &Chord::new(key, mods).unwrap()).map(|a| a.id)
        };
        assert_eq!(id(Platform::Macos, "o", CMD), Some("open_file"));
        assert_eq!(id(Platform::Macos, "O", CMD | SHIFT), Some("open_folder"));
        assert_eq!(id(Platform::Linux, "F2", 0), Some("open_file"));
        assert_eq!(id(Platform::Linux, "Right", 0), Some("next_image"));
        assert_eq!(id(Platform::Macos, "=", CMD), Some("zoom_in"));
        assert_eq!(id(Platform::Macos, "+", CMD | SHIFT), Some("zoom_in"));
        assert_eq!(id(Platform::Windows, "+", CTRL | SHIFT), Some("zoom_in_1"));
        assert_eq!(id(Platform::Windows, "*", SHIFT), Some("zoom_100"));
        assert_eq!(id(Platform::Macos, "Left", ALT), Some("rotate_ccw"));
        assert_eq!(id(Platform::Macos, "l", CMD), Some("rotate_ccw"));
        assert_eq!(
            id(Platform::Windows, "Tab", SHIFT),
            Some("toggle_title_filename")
        );
        assert_eq!(id(Platform::Windows, "q", CTRL), None);
        // 回车进入、退出全屏（三端），另有 F11（Windows、Linux）和 ⌃⌘F（macOS）；Esc 关窗口或退出；
        // 选择图片改用 G。
        for platform in [Platform::Macos, Platform::Windows, Platform::Linux] {
            assert_eq!(id(platform, "Enter", 0), Some("fullscreen"));
            assert_eq!(id(platform, "Escape", 0), Some("close_or_quit"));
            assert_eq!(id(platform, "g", 0), Some("select_image"));
        }
        assert_eq!(id(Platform::Macos, "q", CMD), Some("quit"));
        assert_eq!(id(Platform::Linux, "x", 0), Some("quit"));
        assert_eq!(id(Platform::Linux, "F11", 0), Some("fullscreen"));
        assert_eq!(id(Platform::Macos, "F", CTRL | CMD), Some("fullscreen"));
        assert_eq!(id(Platform::Windows, "Enter", CTRL), Some("reveal_file"));
    }

    #[test]
    fn specs_round_trip_and_system_keys_are_reserved() {
        for spec in [
            "Ctrl+Shift+O",
            "Cmd+O",
            "Ctrl++",
            "Alt+Cmd+PageDown",
            "Space",
            "F5",
            "Q",
        ] {
            let chord = Chord::parse(spec).unwrap();
            assert_eq!(Chord::parse(&chord.spec()), Some(chord.clone()), "{spec}");
        }
        let chord = |spec| Chord::parse(spec).unwrap();
        assert!(reserved(Platform::Macos, &chord("Cmd+Tab")));
        assert!(reserved(Platform::Macos, &chord("Ctrl+Left")));
        assert!(reserved(Platform::Macos, &chord("Ctrl+Cmd+Q")));
        assert!(!reserved(Platform::Macos, &chord("Cmd+Q")));
        assert!(!reserved(Platform::Macos, &chord("PageDown")));
        assert!(reserved(Platform::Linux, &chord("Cmd+E")));
        assert!(reserved(Platform::Windows, &chord("Alt+F4")));
        assert!(!reserved(Platform::Linux, &chord("Ctrl+E")));
        for key in SPECIAL_KEYS {
            assert!(Chord::new(key, 0).is_some(), "{key}");
        }
    }

    #[test]
    fn labels_follow_platform_conventions() {
        let label = |spec, platform| Chord::parse(spec).unwrap().label(platform);
        assert_eq!(label("Cmd+Shift+O", Platform::Macos), "⇧⌘O");
        assert_eq!(label("Alt+Cmd++", Platform::Macos), "⌥⌘+");
        assert_eq!(label("Ctrl+Cmd+F", Platform::Macos), "⌃⌘F");
        assert_eq!(label("Cmd+PageDown", Platform::Macos), "⌘⇟");
        assert_eq!(
            label("Ctrl+Alt+PageDown", Platform::Windows),
            "Ctrl+Alt+PageDown"
        );
        assert_eq!(label("Alt+Left", Platform::Linux), "Alt+←");
    }

    #[test]
    fn menus_reference_known_actions_and_balance() {
        for (kind, platform) in [
            (MenuKind::Context, Platform::Macos),
            (MenuKind::Context, Platform::Linux),
            (MenuKind::MenuBar, Platform::Macos),
            (MenuKind::MenuBar, Platform::Windows),
        ] {
            let entries = menu(kind, platform, Lang::ZhHans);
            assert!(!entries.is_empty());
            let mut depth = 0i32;
            for e in &entries {
                match e {
                    MenuEntry::SubmenuStart { .. } => depth += 1,
                    MenuEntry::SubmenuEnd => depth -= 1,
                    _ => {}
                }
                assert!(depth >= 0);
            }
            assert_eq!(depth, 0);
        }
        assert!(menu(MenuKind::MenuBar, Platform::Linux, Lang::En).is_empty());
    }

    /// Windows 的菜单栏与 macOS 是同一棵树：没有"Anyview"菜单和只在 macOS 上有的几项，
    /// 设置、退出在"文件"里，关于在"帮助"里；苹果的说法换回动作自己的名字。
    #[test]
    fn windows_menu_bar_is_the_mac_one_without_the_app_menu() {
        let top = |platform| {
            let mut depth = 0;
            let mut names = Vec::new();
            for e in menu(MenuKind::MenuBar, platform, Lang::ZhHans) {
                match e {
                    MenuEntry::SubmenuStart { title, .. } => {
                        if depth == 0 {
                            names.push(title);
                        }
                        depth += 1;
                    }
                    MenuEntry::SubmenuEnd => depth -= 1,
                    _ => {}
                }
            }
            names
        };
        let mac = top(Platform::Macos);
        let win = top(Platform::Windows);
        assert_eq!(mac[0], "Anyview");
        assert_eq!(win, mac[1..], "除了 Anyview 菜单，顶层的菜单一样");
        let ids = |platform| -> Vec<&'static str> {
            menu(MenuKind::MenuBar, platform, Lang::ZhHans)
                .into_iter()
                .filter_map(|e| match e {
                    MenuEntry::Action { id, .. } => Some(id),
                    _ => None,
                })
                .collect()
        };
        let win_ids = ids(Platform::Windows);
        for id in ["settings", "quit", "about", "clear_recent", "help_shortcuts"] {
            assert_eq!(win_ids.iter().filter(|i| **i == id).count(), 1, "{id} 应当正好出现一次");
        }
        for id in ["hide_app", "hide_others", "show_all"] {
            assert!(!win_ids.contains(&id), "{id} 只在 macOS 上有");
        }
        let mac_ids = ids(Platform::Macos);
        for id in &mac_ids {
            if !["hide_app", "hide_others", "show_all"].contains(id) {
                assert!(win_ids.contains(id), "{id} 在 macOS 的菜单栏里有，Windows 的也该有");
            }
        }
        let title = |platform, id: &str| {
            menu(MenuKind::MenuBar, platform, Lang::ZhHans)
                .into_iter()
                .find_map(|e| match e {
                    MenuEntry::Action { id: i, title, .. } if i == id => Some(title),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(title(Platform::Macos, "fullscreen"), "进入全屏幕");
        assert_eq!(title(Platform::Windows, "fullscreen"), "全屏");
        assert_eq!(title(Platform::Windows, "maximize"), "最大化/还原");
    }

    #[test]
    fn context_menu_shows_platform_wording_and_keys() {
        let find_entry = |platform, id: &str| {
            menu(MenuKind::Context, platform, Lang::ZhHans)
                .into_iter()
                .find_map(|e| match e {
                    MenuEntry::Action {
                        id: i,
                        title,
                        shortcut,
                        ..
                    } if i == id => Some((title, shortcut)),
                    _ => None,
                })
                .unwrap()
        };
        let (title, key) = find_entry(Platform::Macos, "delete_file");
        assert_eq!(title, "移到废纸篓");
        assert_eq!(key.unwrap().label(Platform::Macos), "⌘⌫");
        let (title, key) = find_entry(Platform::Linux, "delete_file");
        assert_eq!(title, "删除");
        assert_eq!(key.unwrap().label(Platform::Linux), "Del");
    }

    #[test]
    fn tooltips_show_the_first_shortcut() {
        assert_eq!(
            tooltip("open_file", Platform::Macos, Lang::ZhHans),
            "打开图片 (⌘O)"
        );
        assert_eq!(
            tooltip("open_file", Platform::Linux, Lang::ZhHans),
            "打开图片 (F2)"
        );
        assert_eq!(
            tooltip("toggle_bar_lock", Platform::Windows, Lang::ZhHans),
            "锁定控制栏 (~)"
        );
        assert_eq!(
            tooltip("maximize", Platform::Macos, Lang::En),
            "Maximize/Restore"
        );
        assert_eq!(tooltip("nope", Platform::Macos, Lang::En), "");
    }

    #[test]
    fn every_action_has_english_text() {
        for a in ACTIONS {
            assert!(!a.zh.is_empty() && !a.en.is_empty(), "{}", a.id);
            assert!(
                a.en.is_ascii() || a.en.contains('°') || a.en.contains('…'),
                "{}",
                a.id
            );
        }
    }

    /// 大写不是单个字符的字母（ß → SS）：键名保持原样，写进设置文件后读得回来。
    #[test]
    fn letters_without_a_single_uppercase_round_trip() {
        let chord = Chord::new("\u{df}", CTRL).unwrap();
        assert_eq!(chord.key, "\u{df}");
        assert_eq!(Chord::parse(&chord.spec()), Some(chord.clone()));
        // 开着大写锁定打出来的 ẞ 是同一个键。
        assert_eq!(Chord::new("\u{1e9e}", CTRL), Some(chord));
        // 普通字母照旧是大写：é → É，a、A → A。
        assert_eq!(Chord::new("\u{e9}", 0).unwrap().key, "\u{c9}");
        assert_eq!(Chord::new("a", 0), Chord::new("A", 0));
        assert_eq!(Chord::new("a", 0).unwrap().key, "A");
    }
}
