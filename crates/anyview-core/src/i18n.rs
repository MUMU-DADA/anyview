//! 界面文本表。三端都从这里取文本。
//!
//! 简体中文和英文写在代码里（中文是设计时的原文，英文是翻译文件的键）；其他语言各有一个翻译文件
//! `locales/<代码>.toml`，按英文原文查译文，没有的条目显示英文。代码里散落的文字一律写成
//! [`tr!`]（马上要文字）或 [`msg!`]（先记下、显示时再按语言出文字，如出错原因），中英文两个字面量紧挨着，
//! `tools/i18n/messages.py` 和测试按这个写法把它们找出来。

use std::collections::HashMap;
use std::sync::OnceLock;

/// 界面语言。顺序就是设置 › 语言里的顺序（跟随系统之后）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    ZhHans,
    En,
    ZhHant,
    Ja,
    Ko,
    Es,
    Fr,
    De,
    PtBr,
    Ru,
    It,
    Tr,
    Pl,
    Nl,
    Vi,
    Id,
    Th,
    Uk,
    Hi,
}

pub const ALL_LANGS: &[Lang] = &[
    Lang::ZhHans,
    Lang::ZhHant,
    Lang::En,
    Lang::Ja,
    Lang::Ko,
    Lang::Es,
    Lang::Fr,
    Lang::De,
    Lang::PtBr,
    Lang::Ru,
    Lang::It,
    Lang::Tr,
    Lang::Pl,
    Lang::Nl,
    Lang::Vi,
    Lang::Id,
    Lang::Th,
    Lang::Uk,
    Lang::Hi,
];

impl Lang {
    /// 语言标签（BCP 47），也是设置文件里 `language.language` 的值。
    pub fn code(self) -> &'static str {
        match self {
            Self::ZhHans => "zh-Hans",
            Self::En => "en",
            Self::ZhHant => "zh-Hant",
            Self::Ja => "ja",
            Self::Ko => "ko",
            Self::Es => "es",
            Self::Fr => "fr",
            Self::De => "de",
            Self::PtBr => "pt-BR",
            Self::Ru => "ru",
            Self::It => "it",
            Self::Tr => "tr",
            Self::Pl => "pl",
            Self::Nl => "nl",
            Self::Vi => "vi",
            Self::Id => "id",
            Self::Th => "th",
            Self::Uk => "uk",
            Self::Hi => "hi",
        }
    }

    /// 用这种语言自己写的名字（设置 › 语言的选项）。
    pub fn native_name(self) -> &'static str {
        match self {
            Self::ZhHans => "中文(简体)",
            Self::En => "English",
            Self::ZhHant => "中文(繁體)",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
            Self::Es => "Español",
            Self::Fr => "Français",
            Self::De => "Deutsch",
            Self::PtBr => "Português (Brasil)",
            Self::Ru => "Русский",
            Self::It => "Italiano",
            Self::Tr => "Türkçe",
            Self::Pl => "Polski",
            Self::Nl => "Nederlands",
            Self::Vi => "Tiếng Việt",
            Self::Id => "Bahasa Indonesia",
            Self::Th => "ไทย",
            Self::Uk => "Українська",
            Self::Hi => "हिन्दी",
        }
    }

    /// Linux 的桌面文件（.desktop）、文件类型定义（MIME）里这种语言的写法，如 `zh_CN`、`pt_BR`。
    /// 有的语言写几个，和界面选语言的规矩一致（`supported`）：繁体还有香港、澳门，简体还有新加坡，
    /// 葡萄牙语不分巴西、葡萄牙。
    pub fn posix_locales(self) -> &'static [&'static str] {
        match self {
            Self::ZhHans => &["zh_CN", "zh_SG"],
            Self::En => &["en"],
            Self::ZhHant => &["zh_TW", "zh_HK", "zh_MO"],
            Self::Ja => &["ja"],
            Self::Ko => &["ko"],
            Self::Es => &["es"],
            Self::Fr => &["fr"],
            Self::De => &["de"],
            Self::PtBr => &["pt_BR", "pt"],
            Self::Ru => &["ru"],
            Self::It => &["it"],
            Self::Tr => &["tr"],
            Self::Pl => &["pl"],
            Self::Nl => &["nl"],
            Self::Vi => &["vi"],
            Self::Id => &["id"],
            Self::Th => &["th"],
            Self::Uk => &["uk"],
            Self::Hi => &["hi"],
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        ALL_LANGS.iter().copied().find(|l| l.code() == code)
    }

    /// 按系统语言标签选择界面语言，如 `zh-Hant-TW`、`zh_CN.UTF-8`、`pt-BR`、`ja_JP`；没有的语言用英文。
    pub fn from_locale(tag: &str) -> Self {
        Self::supported(tag).unwrap_or(Self::En)
    }

    /// 系统的偏好语言（按先后）里第一种有的；都没有用英文。如偏好"瑞典语、德语"时用德语。
    pub fn from_locales<'a>(tags: impl IntoIterator<Item = &'a str>) -> Self {
        tags.into_iter().find_map(Self::supported).unwrap_or(Self::En)
    }

    /// 这个语言标签对应的界面语言；没有这种语言（或是 `C`、`POSIX`）时为空。
    /// 中文按文字分简繁（台湾、香港、澳门默认繁体）；葡萄牙语一律用巴西葡萄牙语。
    pub fn supported(tag: &str) -> Option<Self> {
        let tag = tag.trim().to_ascii_lowercase().replace('_', "-");
        let tag = tag.split(['.', '@']).next().unwrap_or("");
        let mut parts = tag.split('-');
        let language = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();
        Some(match language {
            "zh" => {
                if rest.contains(&"hant") || (!rest.contains(&"hans") && rest.iter().any(|r| matches!(*r, "tw" | "hk" | "mo"))) {
                    Self::ZhHant
                } else {
                    Self::ZhHans
                }
            }
            "en" => Self::En,
            "ja" => Self::Ja,
            "ko" => Self::Ko,
            "es" => Self::Es,
            "fr" => Self::Fr,
            "de" => Self::De,
            "pt" => Self::PtBr,
            "ru" => Self::Ru,
            "it" => Self::It,
            "tr" => Self::Tr,
            "pl" => Self::Pl,
            "nl" => Self::Nl,
            "vi" => Self::Vi,
            "id" | "in" => Self::Id,
            "th" => Self::Th,
            "uk" => Self::Uk,
            "hi" => Self::Hi,
            _ => return None,
        })
    }

    /// 翻译文件的内容（简体中文、英文没有翻译文件）。
    fn catalog_source(self) -> Option<&'static str> {
        Some(match self {
            Self::ZhHans | Self::En => return None,
            Self::ZhHant => include_str!("../locales/zh-Hant.toml"),
            Self::Ja => include_str!("../locales/ja.toml"),
            Self::Ko => include_str!("../locales/ko.toml"),
            Self::Es => include_str!("../locales/es.toml"),
            Self::Fr => include_str!("../locales/fr.toml"),
            Self::De => include_str!("../locales/de.toml"),
            Self::PtBr => include_str!("../locales/pt-BR.toml"),
            Self::Ru => include_str!("../locales/ru.toml"),
            Self::It => include_str!("../locales/it.toml"),
            Self::Tr => include_str!("../locales/tr.toml"),
            Self::Pl => include_str!("../locales/pl.toml"),
            Self::Nl => include_str!("../locales/nl.toml"),
            Self::Vi => include_str!("../locales/vi.toml"),
            Self::Id => include_str!("../locales/id.toml"),
            Self::Th => include_str!("../locales/th.toml"),
            Self::Uk => include_str!("../locales/uk.toml"),
            Self::Hi => include_str!("../locales/hi.toml"),
        })
    }

    /// 这种语言的翻译表（英文原文 → 译文），第一次用到时读一次。
    fn catalog(self) -> Option<&'static HashMap<String, String>> {
        static CATALOGS: [OnceLock<HashMap<String, String>>; 19] = [const { OnceLock::new() }; 19];
        let source = self.catalog_source()?;
        let index = ALL_LANGS.iter().position(|l| *l == self)?;
        Some(CATALOGS[index].get_or_init(|| parse_catalog(source).unwrap_or_default()))
    }
}

/// 读翻译文件：TOML，键是英文原文，值是译文。
pub fn parse_catalog(source: &str) -> Result<HashMap<String, String>, String> {
    let table: toml::Table = source.parse().map_err(|e: toml::de::Error| e.to_string())?;
    table
        .into_iter()
        .map(|(key, value)| match value {
            toml::Value::String(text) => Ok((key, text)),
            other => Err(format!("{key}: 不是文字（{other}）")),
        })
        .collect()
}

/// 一句界面文字在这种语言里的样子：简体中文、英文直接用代码里的原文，其他语言按英文从翻译表里找，
/// 没有就用英文。
pub fn translate(lang: Lang, zh: &'static str, en: &'static str) -> &'static str {
    match lang {
        Lang::ZhHans => zh,
        Lang::En => en,
        other => other
            .catalog()
            .and_then(|map| map.get(en))
            .map(String::as_str)
            .unwrap_or(en),
    }
}

/// 马上要的一句文字：`tr!(lang, "中文", "English")`。两个字面量要紧挨着写，工具才找得到。
#[macro_export]
macro_rules! tr {
    ($lang:expr, $zh:literal, $en:literal $(,)?) => {
        $crate::i18n::translate($lang, $zh, $en)
    };
}

/// 先记下、显示时再按语言出文字的一句话（如解码器的出错原因）：
/// `msg!("文件里没有图像：{name}", "No image in the file: {name}", name = path)`。
/// 参数可以是文字，也可以是另一个 [`Message`]（按同一种语言出文字）。
#[macro_export]
macro_rules! msg {
    ($zh:literal, $en:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::Message::new($zh, $en)$(.arg(stringify!($name), $value))*
    };
}

/// 还没定语言的一句话：中英文模板加上填进 `{名字}` 的值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    zh: &'static str,
    en: &'static str,
    args: Vec<(&'static str, Arg)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    Text(String),
    Message(Message),
}

impl From<String> for Arg {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for Arg {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

impl From<Message> for Arg {
    fn from(message: Message) -> Self {
        Self::Message(message)
    }
}

impl Message {
    pub const fn new(zh: &'static str, en: &'static str) -> Self {
        Self {
            zh,
            en,
            args: Vec::new(),
        }
    }

    /// 不用翻译的文字（库给的出错说明、文件名等）。
    pub fn raw(text: impl Into<String>) -> Self {
        Self::new("{text}", "{text}").arg("text", text.into())
    }

    pub fn arg(mut self, name: &'static str, value: impl Into<Arg>) -> Self {
        self.args.push((name, value.into()));
        self
    }

    pub fn text(&self, lang: Lang) -> String {
        let mut out = translate(lang, self.zh, self.en).to_string();
        for (name, value) in &self.args {
            let value = match value {
                Arg::Text(text) => text.clone(),
                Arg::Message(message) => message.text(lang),
            };
            out = out.replace(&format!("{{{name}}}"), &value);
        }
        out
    }
}

impl std::fmt::Display for Message {
    /// 日志、测试里用英文。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text(Lang::En))
    }
}

impl From<String> for Message {
    fn from(text: String) -> Self {
        Self::raw(text)
    }
}

/// 文本键。新增键时两种语言都要给出文本，由 `match` 的穷尽检查保证。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    AppName,
    /// 空态提示，`{key}` 替换为"打开图片"的快捷键。
    EmptyStateHint,
    /// 状态行：核心通信正常，`{ms}` 替换为往返耗时。
    CoreLinkOk,
    NoticeAtFirst,
    NoticeAtLast,
    NoticeEmptyFolder,
    /// `{message}` 替换为原因。
    NoticeOpenFailed,
    /// `{message}` 替换为原因。
    NoticeImageFailed,
    OpenDialogTitle,
    ImageFilterName,
    /// 左上角图片信息的各行，`{value}` 替换为内容。
    InfoName,
    InfoSize,
    InfoModified,
    InfoImage,
    /// 拍摄信息的各行。
    InfoCamera,
    InfoLens,
    InfoExposure,
    InfoTaken,
    /// 等效焦距，跟在焦距后面，`{value}` 替换为如 `450mm`。
    InfoFocal35,
    /// 图片信息上方的按钮。
    InfoClose,
    InfoCopy,
    /// 控制栏（MENUS"底部控制栏"）。
    BarLock,
    BarPrevTip,
    BarNextTip,
    /// 进度条：目录还在扫描。
    BarScanning,
    /// 相邻文件夹、随机退回的提示。
    NoticeLastFolder,
    NoticeFirstFolder,
    NoticeNoRandomBack,
    /// `{message}` 替换为原因。
    NoticeTrashFailed,
    NoticeCopied,
    /// 设置（docs/SETTINGS.md）：启动时设置文件坏了，`{name}` 替换为备份的文件名。
    NoticeSettingsRecovered,
    /// 同上，但没能备份。
    NoticeSettingsUnreadable,
    /// `{message}` 替换为原因。
    NoticeSettingsSaveFailed,
    /// 设置窗口。
    SettingsTitle,
    SettingsReset,
    SettingsOk,
    /// 关闭窗口时有没确定的修改。
    SettingsDiscardMessage,
    SettingsDiscard,
    SettingsKeepEditing,
    /// "设为默认看图软件"的结果；`{message}` 替换为原因。
    SettingsDefaultViewerDone,
    SettingsDefaultViewerFailed,
    /// Linux 的"设为默认看图软件"：`{count}` 替换为设了多少种图片类型。
    SettingsDefaultViewerDoneCount,
    /// Linux 的"恢复原来的默认程序"：`{count}` 替换为换回了多少种。
    SettingsDefaultViewerRestored,
    /// Linux：还没有用 Anyview 设过默认程序，没有可恢复的。
    SettingsDefaultViewerNothing,
    /// Linux 的 Flatpak 版改不了系统的默认程序：放进 SettingsDefaultViewerFailed 的 `{message}`。
    SettingsDefaultViewerFlatpak,
    /// Flatpak 版“让文件管理器用 Anyview 做缩略图”的结果。
    SettingsThumbnailerRegistered,
    SettingsThumbnailerRemoved,
    SettingsThumbnailerNothing,
    SettingsThumbnailerNotOurs,
    SettingsThumbnailerNotFlatpak,
    SettingsThumbnailerFailed,
    /// 键盘设置：热键框等待按键时显示。
    SettingsPressKey,
    /// 键盘设置：按的键由系统使用，不能设置（ACTIONS 规则 6）。
    SettingsKeyReserved,
    /// 键盘设置：列表下面的按钮。
    SettingsAddShortcut,
    /// 键盘设置：这一行还没设按键时，按键框上的字。
    SettingsKeyUnset,
    /// 键盘设置：每行最后的删除按钮（悬停提示、读屏的名字）。
    SettingsRemoveShortcut,
    /// 压缩包、文件操作的提示；`{folder}`、`{name}`、`{message}` 替换为内容。
    NoticeArchivePassword,
    NoticeEmptyArchive,
    NoticeCopiedTo,
    NoticeMoved,
    NoticeFolderNotSet,
    NoticeFileFailed,
    NoticeWallpaperSet,
    /// Flatpak 里系统的壁纸门户没有设好（用户点了“不允许”、以前不允许过，或桌面出错，门户都只回“失败”）。
    NoticeWallpaperNotSet,
    NoticeEditorMissing,
    NoticeEditorFailed,
    PromptClearPositions,
    ButtonDelete,
    ButtonCancel,
    /// 选择图片窗口：标题、列表的三列、勾选框、按钮。
    PickerTitle,
    PickerColumnName,
    PickerColumnModified,
    PickerColumnSize,
    PickerHideThumbnails,
    PickerOk,
}

pub const ALL_KEYS: &[Key] = &[
    Key::AppName,
    Key::EmptyStateHint,
    Key::CoreLinkOk,
    Key::NoticeAtFirst,
    Key::NoticeAtLast,
    Key::NoticeEmptyFolder,
    Key::NoticeOpenFailed,
    Key::NoticeImageFailed,
    Key::OpenDialogTitle,
    Key::ImageFilterName,
    Key::InfoName,
    Key::InfoSize,
    Key::InfoModified,
    Key::InfoImage,
    Key::InfoCamera,
    Key::InfoLens,
    Key::InfoExposure,
    Key::InfoTaken,
    Key::InfoFocal35,
    Key::InfoClose,
    Key::InfoCopy,
    Key::BarLock,
    Key::BarPrevTip,
    Key::BarNextTip,
    Key::BarScanning,
    Key::NoticeLastFolder,
    Key::NoticeFirstFolder,
    Key::NoticeNoRandomBack,
    Key::NoticeTrashFailed,
    Key::NoticeCopied,
    Key::NoticeSettingsRecovered,
    Key::NoticeSettingsUnreadable,
    Key::NoticeSettingsSaveFailed,
    Key::SettingsTitle,
    Key::SettingsReset,
    Key::SettingsOk,
    Key::SettingsDiscardMessage,
    Key::SettingsDiscard,
    Key::SettingsKeepEditing,
    Key::SettingsDefaultViewerDone,
    Key::SettingsDefaultViewerFailed,
    Key::SettingsDefaultViewerDoneCount,
    Key::SettingsDefaultViewerRestored,
    Key::SettingsDefaultViewerNothing,
    Key::SettingsDefaultViewerFlatpak,
    Key::SettingsThumbnailerRegistered,
    Key::SettingsThumbnailerRemoved,
    Key::SettingsThumbnailerNothing,
    Key::SettingsThumbnailerNotOurs,
    Key::SettingsThumbnailerNotFlatpak,
    Key::SettingsThumbnailerFailed,
    Key::SettingsPressKey,
    Key::SettingsKeyReserved,
    Key::SettingsAddShortcut,
    Key::SettingsKeyUnset,
    Key::SettingsRemoveShortcut,
    Key::NoticeArchivePassword,
    Key::NoticeEmptyArchive,
    Key::NoticeCopiedTo,
    Key::NoticeMoved,
    Key::NoticeFolderNotSet,
    Key::NoticeFileFailed,
    Key::NoticeWallpaperSet,
    Key::NoticeWallpaperNotSet,
    Key::NoticeEditorMissing,
    Key::NoticeEditorFailed,
    Key::PromptClearPositions,
    Key::ButtonDelete,
    Key::ButtonCancel,
    Key::PickerTitle,
    Key::PickerColumnName,
    Key::PickerColumnModified,
    Key::PickerColumnSize,
    Key::PickerHideThumbnails,
    Key::PickerOk,
];

/// 一个键的中文、英文原文。
fn source(key: Key) -> (&'static str, &'static str) {
    use Key::*;
    match key {
        AppName => ("Anyview", "Anyview"),
        EmptyStateHint => ("将图片拖到窗口中，或按 {key} 打开图片", "Drop an image here, or press {key} to open one"),
        CoreLinkOk => ("核心通信正常（{ms} ms）", "Core link OK ({ms} ms)"),
        NoticeAtFirst => ("已是第一张", "This is the first image"),
        NoticeAtLast => ("已是最后一张", "This is the last image"),
        NoticeEmptyFolder => ("文件夹中没有可以打开的图片", "No images in this folder"),
        NoticeOpenFailed => ("无法打开：{message}", "Cannot open: {message}"),
        NoticeImageFailed => ("无法显示这张图片：{message}", "Cannot show this image: {message}"),
        OpenDialogTitle => ("打开图片", "Open Image"),
        ImageFilterName => ("图片", "Images"),
        InfoName => ("文件名: {value}", "File: {value}"),
        InfoSize => ("图片大小: {value}", "Size: {value}"),
        InfoModified => ("修改日期: {value}", "Modified: {value}"),
        InfoImage => ("图片信息: {value}", "Image: {value}"),
        InfoCamera => ("相机: {value}", "Camera: {value}"),
        InfoLens => ("镜头: {value}", "Lens: {value}"),
        InfoExposure => ("拍摄参数: {value}", "Exposure: {value}"),
        InfoTaken => ("拍摄时间: {value}", "Taken: {value}"),
        InfoFocal35 => ("（等效 {value}）", " ({value} equiv.)"),
        InfoClose => ("关闭图片信息", "Hide Image Info"),
        InfoCopy => ("复制图片信息", "Copy Image Info"),
        BarLock => ("锁定", "Lock"),
        BarPrevTip => ("左键：向上，右键：向上10张", "Click: previous, right-click: back 10"),
        BarNextTip => ("左键：向下，右键：向下10张", "Click: next, right-click: forward 10"),
        BarScanning => ("扫描中", "Scanning…"),
        NoticeLastFolder => ("已是最后一个文件夹", "This is the last folder"),
        NoticeFirstFolder => ("已是第一个文件夹", "This is the first folder"),
        NoticeNoRandomBack => ("没有可以退回的随机跳转", "No earlier random jump"),
        NoticeTrashFailed => ("无法删除：{message}", "Cannot delete: {message}"),
        NoticeCopied => ("已复制到剪贴板", "Copied to the clipboard"),
        NoticeSettingsRecovered => ("设置文件无法读取，已改用默认设置（原文件另存为 {name}）", "The settings file could not be read; using the defaults (the old file was saved as {name})"),
        NoticeSettingsUnreadable => ("设置文件无法读取，已改用默认设置", "The settings file could not be read; using the defaults"),
        NoticeSettingsSaveFailed => ("无法保存设置：{message}", "Cannot save the settings: {message}"),
        SettingsTitle => ("设置", "Settings"),
        SettingsReset => ("恢复本页默认", "Restore Defaults"),
        SettingsOk => ("确定", "OK"),
        SettingsDiscardMessage => ("放弃对设置的修改吗？", "Discard your changes to the settings?"),
        SettingsDiscard => ("放弃修改", "Discard Changes"),
        SettingsKeepEditing => ("继续修改", "Keep Editing"),
        SettingsDefaultViewerDone => ("已把 Anyview 设为图片的默认程序", "Anyview is now the default app for images"),
        SettingsDefaultViewerFailed => ("无法设为默认程序：{message}", "Cannot make Anyview the default app: {message}"),
        SettingsDefaultViewerDoneCount => ("已把 Anyview 设为 {count} 种图片的默认程序", "Anyview is now the default app for {count} image types"),
        SettingsDefaultViewerRestored => ("已换回原来的默认程序（{count} 种图片）", "Restored the previous default apps ({count} image types)"),
        SettingsDefaultViewerNothing => ("Anyview 没有改过默认程序，不用恢复", "Anyview hasn’t changed any default apps"),
        SettingsDefaultViewerFlatpak => ("Flatpak 版请在系统设置的“默认应用程序”里选择 Anyview", "in the Flatpak version, choose Anyview in your system’s Default Applications settings"),
        SettingsThumbnailerRegistered => ("已登记：KDE 的 Dolphin 等文件管理器会用 Anyview 给 {count} 种图片做缩略图（之后打开的文件夹里生效）", "Registered: file managers such as KDE’s Dolphin will use Anyview for thumbnails of {count} image types (in folders you open from now on)"),
        SettingsThumbnailerRemoved => ("已取消登记，文件管理器不再用 Anyview 做缩略图", "Unregistered: the file manager no longer uses Anyview for thumbnails"),
        SettingsThumbnailerNothing => ("没有登记过，不用取消", "Nothing to unregister"),
        SettingsThumbnailerNotOurs => ("{path} 不是 Anyview 登记的，没有动它", "{path} wasn’t registered by Anyview, so it was left alone"),
        SettingsThumbnailerNotFlatpak => ("只有 Flatpak 版要登记：安装包自己带了缩略图生成器", "Only the Flatpak version needs this: the installed package already brings its own thumbnailer"),
        SettingsThumbnailerFailed => ("没有做成：{message}", "That didn’t work: {message}"),
        SettingsPressKey => ("按下按键…", "Press a key…"),
        SettingsKeyReserved => ("这个按键由系统使用，不能设置", "This key is used by the system and can't be assigned"),
        SettingsAddShortcut => ("添加快捷键", "Add Shortcut"),
        SettingsKeyUnset => ("设置按键", "Set Key"),
        SettingsRemoveShortcut => ("删除这个快捷键", "Remove this shortcut"),
        NoticeArchivePassword => ("压缩包有密码，打不开", "This archive is password protected"),
        NoticeEmptyArchive => ("压缩包里没有可以打开的图片", "No images in this archive"),
        NoticeCopiedTo => ("已复制到“{folder}”：{name}", "Copied to “{folder}”: {name}"),
        NoticeMoved => ("已移动到“{folder}”：{name}", "Moved to “{folder}”: {name}"),
        NoticeFolderNotSet => ("还没有设置图片文件夹：在 设置 › 杂项 里设置", "No image folder set yet (Settings › Other)"),
        NoticeFileFailed => ("操作失败：{message}", "Failed: {message}"),
        NoticeWallpaperSet => ("已设为桌面图片", "Set as the desktop picture"),
        NoticeWallpaperNotSet => ("系统没有设为桌面图片：可能没允许 Anyview 设置背景（在系统设置的应用权限里可以改）", "The system didn’t set the desktop picture. Anyview may not be allowed to set backgrounds (check your system’s app permissions)"),
        NoticeEditorMissing => ("还没有设置图像编辑器：在 设置 › 杂项 里选择", "No image editor set yet (Settings › Other)"),
        NoticeEditorFailed => ("无法用图像编辑器打开：{message}", "Cannot open in the image editor: {message}"),
        PromptClearPositions => ("要忘掉所有压缩包里看到的位置吗？", "Forget the positions in all archives?"),
        ButtonDelete => ("删除", "Delete"),
        ButtonCancel => ("取消", "Cancel"),
        PickerTitle => ("选择图片", "Select Image"),
        PickerColumnName => ("列表", "Name"),
        PickerColumnModified => ("修改日期", "Date Modified"),
        PickerColumnSize => ("大小", "Size"),
        PickerHideThumbnails => ("隐藏缩略图导航", "Hide thumbnail navigation"),
        PickerOk => ("确定", "OK"),
    }
}

pub fn text(key: Key, lang: Lang) -> &'static str {
    let (zh, en) = source(key);
    translate(lang, zh, en)
}

/// Linux 桌面文件里程序的类别名和说明（GenericName、Comment），各种语言：(语言写法, 类别名, 说明)。
/// 英文写在不带语言的那两行，这里不列。
pub fn desktop_entry_texts() -> Vec<(&'static str, &'static str, &'static str)> {
    ALL_LANGS
        .iter()
        .filter(|&&lang| lang != Lang::En)
        .flat_map(|&lang| {
            let generic = tr!(lang, "看图软件", "Image Viewer");
            let comment = tr!(lang, "看图片", "View images");
            lang.posix_locales().iter().map(move |&locale| (locale, generic, comment))
        })
        .collect()
}

/// 对话框的文字：一条说明加若干按钮（按钮从左到右，最后一个是取消）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub message: String,
    pub buttons: Vec<String>,
}

/// 到达首尾时的询问：按钮依次为"转到首张"（向前时为"转到末张"）、"转到下一个文件夹"（"上一个"）、"取消"。
pub fn boundary_prompt(forward: bool, archive: bool, lang: Lang) -> Prompt {
    let mut texts: [&str; 4] = if forward {
        [
            tr!(lang, "已是最后一张图片", "This is the last image"),
            tr!(lang, "转到首张", "Go to First"),
            tr!(lang, "转到下一个文件夹", "Next Folder"),
            tr!(lang, "取消", "Cancel"),
        ]
    } else {
        [
            tr!(lang, "已是第一张图片", "This is the first image"),
            tr!(lang, "转到末张", "Go to Last"),
            tr!(lang, "转到上一个文件夹", "Previous Folder"),
            tr!(lang, "取消", "Cancel"),
        ]
    };
    // 压缩包里：转到下一个（上一个）压缩包。
    if archive {
        texts[2] = if forward {
            tr!(lang, "转到下一个压缩包", "Next Archive")
        } else {
            tr!(lang, "转到上一个压缩包", "Previous Archive")
        };
    }
    Prompt {
        message: texts[0].to_string(),
        buttons: texts[1..].iter().map(|t| t.to_string()).collect(),
    }
}

/// 图片被安全上限拦下时的询问：按钮为"仍然打开"和"取消"。默认按钮（回车）是"取消"——
/// 避免误触回车直接放行超限文件。
pub fn limit_prompt(lang: Lang) -> Prompt {
    Prompt {
        message: tr!(
            lang,
            "这个文件超出了安全上限，打开它可能占用大量内存或磁盘空间（损坏的、别有用心的文件会这样，特别大的正常文件也会）。\n确定它没有问题的话，可以仍然打开。",
            "This file exceeds the safety limits and may use a lot of memory or disk space (damaged or malicious files do this, and so do unusually large ones).\nOpen it anyway if you trust it."
        )
        .to_string(),
        buttons: vec![
            tr!(lang, "仍然打开", "Open Anyway").to_string(),
            tr!(lang, "取消", "Cancel").to_string(),
        ],
    }
}

/// 删除前的确认：按平台称呼废纸篓或回收站；按钮为"确认"和"取消"。
pub fn trash_prompt(platform: crate::Platform, lang: Lang, name: &str) -> Prompt {
    use crate::Platform::{Linux, Macos, Windows};
    let (message, confirm) = match platform {
        Macos => (
            tr!(lang, "要把“{name}”移到废纸篓吗？", "Move “{name}” to the Trash?"),
            tr!(lang, "移到废纸篓", "Move to Trash"),
        ),
        Linux => (
            tr!(lang, "要把“{name}”移到回收站吗？", "Move “{name}” to the Trash?"),
            tr!(lang, "删除", "Move to Trash"),
        ),
        Windows => (
            tr!(lang, "要把“{name}”移到回收站吗？", "Move “{name}” to the Recycle Bin?"),
            tr!(lang, "删除", "Delete"),
        ),
    };
    Prompt {
        message: message.replace("{name}", name),
        buttons: vec![confirm.to_string(), tr!(lang, "取消", "Cancel").to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_tags_map_to_languages() {
        assert_eq!(Lang::from_locale("zh-Hans-CN"), Lang::ZhHans);
        assert_eq!(Lang::from_locale("zh_CN.UTF-8"), Lang::ZhHans);
        assert_eq!(Lang::from_locale("zh-Hant-TW"), Lang::ZhHant);
        assert_eq!(Lang::from_locale("zh-TW"), Lang::ZhHant);
        assert_eq!(Lang::from_locale("zh_HK.UTF-8"), Lang::ZhHant);
        assert_eq!(Lang::from_locale("zh-Hans-HK"), Lang::ZhHans);
        assert_eq!(Lang::from_locale("ja-JP"), Lang::Ja);
        assert_eq!(Lang::from_locale("pt_PT.UTF-8"), Lang::PtBr);
        assert_eq!(Lang::from_locale("de_DE@euro"), Lang::De);
        assert_eq!(Lang::from_locale("in-ID"), Lang::Id);
        assert_eq!(Lang::from_locale("nb-NO"), Lang::En);
        assert_eq!(Lang::from_locales(["sv-SE", "de-DE", "en-US"]), Lang::De);
        assert_eq!(Lang::from_locales(["C"]), Lang::En);
        assert_eq!(Lang::from_locales(["en-GB", "ja-JP"]), Lang::En);
        assert_eq!(Lang::from_locale("en-US"), Lang::En);
        assert_eq!(Lang::from_locale("C"), Lang::En);
        assert_eq!(Lang::from_locale(""), Lang::En);
    }

    #[test]
    fn prompts_follow_n5_and_platform_wording() {
        let p = boundary_prompt(true, false, Lang::ZhHans);
        assert_eq!(p.buttons, ["转到首张", "转到下一个文件夹", "取消"]);
        assert_eq!(boundary_prompt(false, false, Lang::En).buttons[0], "Go to Last");
        assert_eq!(boundary_prompt(true, true, Lang::ZhHans).buttons[1], "转到下一个压缩包");
        let t = trash_prompt(crate::Platform::Macos, Lang::ZhHans, "a.png");
        assert_eq!(t.message, "要把“a.png”移到废纸篓吗？");
        assert_eq!(t.buttons, ["移到废纸篓", "取消"]);
        assert!(
            trash_prompt(crate::Platform::Windows, Lang::En, "a")
                .message
                .contains("Recycle Bin")
        );
    }

    #[test]
    fn every_key_has_text_in_every_language() {
        for &key in ALL_KEYS {
            for &lang in ALL_LANGS {
                assert!(!text(key, lang).is_empty(), "{key:?} {lang:?}");
            }
        }
    }

    /// Rust 字面量里的转义（`\n`、`\"`、`\\`、`\u{…}`）。
    fn unescape(literal: &str) -> String {
        let mut out = String::new();
        let mut chars = literal.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let code: String = chars.by_ref().skip(1).take_while(|&c| c != '}').collect();
                    out.extend(u32::from_str_radix(&code, 16).ok().and_then(char::from_u32));
                }
                Some(other) => out.push(other),
                None => {}
            }
        }
        out
    }

    /// 从 `rest` 开头（跳过空白）读一个字符串字面量，返回内容和剩下的部分。
    fn literal(rest: &str) -> Option<(String, &str)> {
        let rest = rest.trim_start().strip_prefix('"')?;
        let mut escaped = false;
        for (i, c) in rest.char_indices() {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => return Some((unescape(&rest[..i]), &rest[i + 1..])),
                _ => {}
            }
        }
        None
    }

    /// 源代码里散落的 `tr!(语言, "中文", "English")`、`msg!("中文", "English", …)`（不含注释和测试）。
    fn scattered() -> Vec<(String, String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .collect();
        files.sort();
        let mut out = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            let text = match text.find("\n#[cfg(test)]\nmod tests") {
                Some(end) => &text[..end],
                None => &text[..],
            };
            let code: String = text
                .lines()
                .map(|line| if line.trim_start().starts_with("//") { "" } else { line })
                .collect::<Vec<_>>()
                .join("\n");
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            for (marker, skip_argument) in [("tr!(", true), ("msg!(", false)] {
                let mut rest = code.as_str();
                while let Some(at) = rest.find(marker) {
                    rest = &rest[at + marker.len()..];
                    let mut args = rest;
                    if skip_argument {
                        // 第一个参数是语言（表达式，里面没有逗号以外的花样）。
                        let Some(comma) = args.find(',') else { continue };
                        args = &args[comma + 1..];
                    }
                    let Some((zh, after)) = literal(args) else { continue };
                    let Some(after) = after.trim_start().strip_prefix(',') else { continue };
                    let Some((en, _)) = literal(after) else { continue };
                    out.push((zh, en, format!("{name} ({})", marker.trim_end_matches('('))));
                }
            }
        }
        out
    }

    /// 所有要翻译的句子：英文原文 → (各处的中文, 用在哪里)。中英文一样的（名字、格式名）不用翻译。
    fn messages() -> std::collections::BTreeMap<String, (std::collections::BTreeSet<String>, std::collections::BTreeSet<String>)> {
        let mut all: Vec<(String, String, String)> = Vec::new();
        for &key in ALL_KEYS {
            let (zh, en) = source(key);
            all.push((zh.into(), en.into(), format!("text ({key:?})")));
        }
        for (zh, en, place) in crate::settings::texts().into_iter().chain(crate::actions::texts()) {
            all.push((zh.into(), en.into(), place));
        }
        all.extend(scattered());
        let mut map: std::collections::BTreeMap<String, (std::collections::BTreeSet<String>, std::collections::BTreeSet<String>)> =
            std::collections::BTreeMap::new();
        for (zh, en, place) in all {
            if en.is_empty() || zh == en {
                continue;
            }
            let entry = map.entry(en).or_default();
            entry.0.insert(zh);
            entry.1.insert(place);
        }
        map
    }

    fn toml_string(text: &str) -> String {
        let mut out = String::from("\"");
        for c in text.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    fn placeholders(text: &str) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        let mut rest = text;
        while let Some(start) = rest.find('{') {
            let Some(end) = rest[start..].find('}') else { break };
            let inner = &rest[start + 1..start + end];
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
                out.insert(format!("{{{inner}}}"));
            }
            rest = &rest[start + end + 1..];
        }
        out
    }

    /// 给翻译用的总表 `locales/messages.toml`：每条是英文原文，上面的注释写中文和用在哪里。
    /// 句子改了以后用 `ANYVIEW_UPDATE_MESSAGES=1 cargo test -p anyview-core messages_file` 重新生成。
    #[test]
    fn messages_file_is_current() {
        let mut text = String::from(
            "# Anyview 要翻译的全部句子（由 i18n.rs 的测试生成，不要手改）。键是英文原文；注释是中文原文和用在哪里。\n\
             # 翻译文件 locales/<语言>.toml 用同样的键，值是译文；检查：python3 tools/i18n/messages.py check <语言>。\n",
        );
        for (en, (zh, places)) in messages() {
            let zh: Vec<_> = zh.into_iter().collect();
            let places: Vec<_> = places.into_iter().take(4).collect();
            text.push_str(&format!(
                "\n# 中文：{}\n# 用在：{}\n{} = \"\"\n",
                zh.join(" ／ ").replace('\n', "⏎"),
                places.join("，"),
                toml_string(&en)
            ));
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales/messages.toml");
        if std::env::var_os("ANYVIEW_UPDATE_MESSAGES").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            current == text,
            "locales/messages.toml 过时了：用 ANYVIEW_UPDATE_MESSAGES=1 cargo test -p anyview-core messages_file 重新生成，再补各语言的翻译"
        );
    }

    /// 每种语言的翻译都要齐：不缺、不多、`{占位符}` 和原文一样。
    #[test]
    fn every_language_has_every_message() {
        let messages = messages();
        let mut problems = Vec::new();
        for &lang in ALL_LANGS {
            let Some(source) = lang.catalog_source() else { continue };
            let catalog = match parse_catalog(source) {
                Ok(catalog) => catalog,
                Err(e) => {
                    problems.push(format!("{}：读不了：{e}", lang.code()));
                    continue;
                }
            };
            let missing = messages.keys().filter(|en| !catalog.contains_key(*en)).count();
            if missing > 0 {
                problems.push(format!("{}：缺 {missing} 条", lang.code()));
            }
            for (en, text) in &catalog {
                if !messages.contains_key(en) {
                    problems.push(format!("{}：多出来的（原文已经没有了）：{en}", lang.code()));
                } else if text.trim().is_empty() {
                    problems.push(format!("{}：译文是空的：{en}", lang.code()));
                } else if placeholders(text) != placeholders(en) {
                    problems.push(format!("{}：占位符对不上：{en} → {text}", lang.code()));
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn translate_falls_back_to_english_and_fills_messages() {
        assert_eq!(translate(Lang::ZhHans, "取消", "Cancel"), "取消");
        assert_eq!(translate(Lang::En, "取消", "Cancel"), "Cancel");
        assert_eq!(translate(Lang::Ja, "这句没有翻译", "No translation for this"), "No translation for this");
        let inner = msg!("文件里没有图像", "no image in the file");
        let outer = msg!("AVIF：{e}", "AVIF: {e}", e = inner);
        assert_eq!(outer.text(Lang::ZhHans), "AVIF：文件里没有图像");
        assert_eq!(outer.to_string(), "AVIF: no image in the file");
        assert_eq!(Message::raw("disk full").text(Lang::ZhHans), "disk full");
        for &lang in ALL_LANGS {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
        }
    }

}
