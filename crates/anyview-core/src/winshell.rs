//! Windows 的系统集成里与系统无关的规则：安装后往注册表里登记什么——能打开的文件类型
//! （“打开方式”、默认应用）、资源管理器的缩略图、右键菜单、“应用和功能”里的卸载项。
//!
//! 这里只算出“要写哪些项”（[`registry_plan`]），不碰注册表：真正读写在 `anyview-ffi` 的 `shell` 模块（只在
//! Windows 上编译）。安装程序、卸载程序、设置窗口的“确定”都走同一条路：Anyview.exe 按这里的清单登记，并记下
//! 写过什么，撤销时照记录原样恢复。全部写在当前用户自己的那一半注册表里（HKEY_CURRENT_USER），不需要管理员权限。

use crate::i18n::Lang;
use crate::settings::{self, Settings};

/// Windows 自己会做缩略图的常见格式（系统自带的解码器）：只登记“能用 Anyview 打开”，缩略图仍由系统做。
/// `heics`（HEIF 图像序列）核心自带的解码器不解，也放在这一组。
pub const COMMON_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "dib", "tif", "tiff", "ico", "cur", "dds", "jxr", "wdp", "hdp",
    "heics",
];

/// 系统不认识、或者要另装扩展才认识的格式里，核心自己能解的：这些另外登记 Anyview 的缩略图提供程序。
/// RAW（[`crate::raw::EXTENSIONS`]）也在这一组，见 [`more_extensions`]。
const MORE_EXTENSIONS: &[&str] = &[
    "apng", "webp", "tga", "pbm", "pgm", "ppm", "pnm", "pam", "qoi", "psd", "psb", "pcx", "exr", "hdr", "jxl", "svg", "svgz",
    "jp2", "j2k", "j2c", "jpc", "jpx", "jpf", "clip", "avif", "avifs", "heic", "heif", "hif",
];

/// 由 Anyview 做缩略图的那一组扩展名（小写，不带点）。
pub fn more_extensions() -> Vec<&'static str> {
    let mut all = MORE_EXTENSIONS.to_vec();
    all.extend_from_slice(crate::raw::EXTENSIONS);
    all
}

/// 压缩包那一组：只登记“能用 Anyview 打开”和右键菜单。
pub fn archive_extensions() -> &'static [&'static str] {
    crate::archive::EXTENSIONS
}

/// 设置里“资源管理器的缩略图预览”的哪个开关管这个扩展名；不归 Anyview 做缩略图的为空。
fn thumbnail_switch(extension: &str) -> Option<&'static str> {
    match extension {
        "webp" => Some(settings::THUMB_WEBP),
        "psd" => Some(settings::THUMB_PSD),
        "psb" => Some(settings::THUMB_PSB),
        _ if MORE_EXTENSIONS.contains(&extension) || crate::raw::is_raw_extension(extension) => {
            Some(settings::THUMB_OTHER)
        }
        _ => None,
    }
}

/// 这个扩展名（小写，不带点）要不要登记 Anyview 的缩略图：归 Anyview 做的格式，并且设置里对应的开关开着。
pub fn thumbnail_registered(extension: &str, settings: &Settings) -> bool {
    thumbnail_switch(extension).is_some_and(|key| settings.bool(key))
}

/// 资源管理器右键菜单里的那一项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub title: &'static str,
    /// 带不带软件图标。
    pub icon: bool,
}

/// 右键菜单里显示什么；设置里没开“在资源管理器中使用上下文菜单”时为空（不登记）。
pub fn menu_entry(settings: &Settings, system: Lang) -> Option<MenuEntry> {
    if !settings.bool(settings::CONTEXT_MENU) {
        return None;
    }
    let title = tr!(settings.lang(system), "用 Anyview 浏览图片", "Browse with Anyview");
    Some(MenuEntry {
        title,
        icon: settings.bool(settings::CONTEXT_MENU_ICON),
    })
}

// ---------------------------------------------------------------------------------------------
// 注册表里的名字（都相对于 HKEY_CURRENT_USER）

/// 缩略图提供程序的类号（核心的 DLL 里的 COM 类，`anyview-ffi` 的 `shell` 模块）。
pub const THUMBNAIL_CLSID: &str = "{1DC1F4D7-6312-479A-9043-BCA81CE0234B}";
/// 系统规定的“缩略图提供程序”这个扩展点的编号（IThumbnailProvider）。
const THUMBNAIL_SLOT: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
/// 图片、压缩包两种文件类型登记的名字（ProgID）。
pub const IMAGE_PROG_ID: &str = "Anyview.Image";
pub const ARCHIVE_PROG_ID: &str = "Anyview.Archive";
/// Anyview 自己的项：安装位置、给“默认应用”看的功能表、登记时写过什么的记录。
pub const APP_KEY: &str = r"Software\Anyview";
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Anyview";
const CLASSES: &str = r"Software\Classes";
/// 右键菜单那一项在各扩展名下的名字。
const MENU_VERB: &str = "Anyview.Browse";

/// 安装的情况：登记时由安装程序告诉 Anyview.exe，也记在 [`APP_KEY`] 里（设置变了重新登记时再用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    /// 程序所在的文件夹（Anyview.exe、anyview_ffi.dll、uninstall.exe 都在里面）。
    pub dir: String,
    /// 版本号，显示在“应用和功能”里。
    pub version: String,
    /// 装好后占多少（KB），显示在“应用和功能”里。
    pub size_kb: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegData {
    Text(String),
    Number(u32),
}

/// 要写的一个值。`name` 为空串表示这个项的默认值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegValue {
    pub key: String,
    pub name: String,
    pub data: RegData,
    /// 只在系统里这个值还空着（哪个程序都没写过）时才写：用来认领没人认领的扩展名，不抢别人的。
    pub only_if_unset: bool,
}

/// 登记的清单。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegPlan {
    /// 整个归 Anyview 的项：登记时建，撤销时连里面的东西一起删。
    pub owned_keys: Vec<String>,
    /// 要写的值。在 `owned_keys` 里的随项一起删；别的是写在和别的程序共用的项里的（扩展名下的“打开方式”等），
    /// 登记时记下原来的值，撤销时恢复，并把因此空出来的项删掉。
    pub values: Vec<RegValue>,
}

impl RegPlan {
    fn own(&mut self, key: String) -> String {
        self.owned_keys.push(key.clone());
        key
    }

    fn text(&mut self, key: &str, name: &str, data: impl Into<String>) {
        self.values.push(RegValue {
            key: key.to_string(),
            name: name.to_string(),
            data: RegData::Text(data.into()),
            only_if_unset: false,
        });
    }

    fn number(&mut self, key: &str, name: &str, data: u32) {
        self.values.push(RegValue {
            key: key.to_string(),
            name: name.to_string(),
            data: RegData::Number(data),
            only_if_unset: false,
        });
    }

    /// 这个值是不是写在整个归 Anyview 的项里。
    pub fn is_owned(&self, value: &RegValue) -> bool {
        self.owned_keys.iter().any(|owned| {
            value.key.len() >= owned.len()
                && value.key[..owned.len()].eq_ignore_ascii_case(owned)
                && (value.key.len() == owned.len() || value.key.as_bytes()[owned.len()] == b'\\')
        })
    }
}

/// 按安装的情况和现在的设置，算出要往注册表里写什么。
pub fn registry_plan(install: &Install, settings: &Settings, system: Lang) -> RegPlan {
    let dir = install.dir.trim_end_matches('\\');
    let exe = format!(r"{dir}\Anyview.exe");
    let open_command = format!(r#""{exe}" "%1""#);
    let icon = format!(r#""{exe}",0"#);
    let lang = settings.lang(system);
    let mut plan = RegPlan::default();

    // Anyview 自己的项：安装位置；给系统的“默认应用”设置看的功能表（它按 RegisteredApplications 里的登记找到这里）。
    let app = plan.own(APP_KEY.to_string());
    plan.text(&app, "InstallDir", dir);
    plan.text(&app, "Version", install.version.as_str());
    plan.number(&app, "SizeKb", install.size_kb);
    let capabilities = format!(r"{APP_KEY}\Capabilities");
    plan.text(&capabilities, "ApplicationName", "Anyview");
    plan.text(
        &capabilities,
        "ApplicationDescription",
        tr!(lang, "看图软件", "Image viewer"),
    );
    plan.text(r"Software\RegisteredApplications", "Anyview", capabilities.as_str());

    // “应用和功能”里的卸载项。
    let uninstall = plan.own(UNINSTALL_KEY.to_string());
    plan.text(&uninstall, "DisplayName", "Anyview");
    plan.text(&uninstall, "DisplayVersion", install.version.as_str());
    plan.text(&uninstall, "Publisher", "Anyview");
    plan.text(&uninstall, "DisplayIcon", format!("{exe},0"));
    plan.text(&uninstall, "InstallLocation", dir);
    plan.text(&uninstall, "UninstallString", format!(r#""{dir}\uninstall.exe""#));
    plan.text(&uninstall, "QuietUninstallString", format!(r#""{dir}\uninstall.exe" /S"#));
    plan.number(&uninstall, "EstimatedSize", install.size_kb);
    plan.number(&uninstall, "NoModify", 1);
    plan.number(&uninstall, "NoRepair", 1);

    // 程序本身（“打开方式”里显示的名字），两种文件类型。
    let application = plan.own(format!(r"{CLASSES}\Applications\Anyview.exe"));
    plan.text(&application, "FriendlyAppName", "Anyview");
    plan.text(&format!(r"{application}\shell\open\command"), "", open_command.as_str());
    for prog_id in [IMAGE_PROG_ID, ARCHIVE_PROG_ID] {
        let key = plan.own(format!(r"{CLASSES}\{prog_id}"));
        plan.text(&format!(r"{key}\DefaultIcon"), "", icon.as_str());
        plan.text(&format!(r"{key}\shell\open\command"), "", open_command.as_str());
    }

    // 缩略图提供程序这个类：系统在单独的进程里加载核心的 DLL（没写 DisableProcessIsolation）。
    let class = plan.own(format!(r"{CLASSES}\CLSID\{THUMBNAIL_CLSID}"));
    plan.text(&class, "", "Anyview thumbnail provider");
    plan.text(&format!(r"{class}\InprocServer32"), "", format!(r"{dir}\anyview_ffi.dll"));
    plan.text(&format!(r"{class}\InprocServer32"), "ThreadingModel", "Apartment");

    let menu = menu_entry(settings, system);
    let images = COMMON_EXTENSIONS.iter().copied().chain(more_extensions());
    let archives = archive_extensions().iter().copied();
    for (extension, prog_id) in images
        .map(|e| (e, IMAGE_PROG_ID))
        .chain(archives.map(|e| (e, ARCHIVE_PROG_ID)))
    {
        let ext_key = format!(r"{CLASSES}\.{extension}");
        // “打开方式”里有 Anyview；“默认应用”里能把这种文件交给 Anyview。
        plan.text(&format!(r"{ext_key}\OpenWithProgids"), prog_id, "");
        plan.text(&format!(r"{capabilities}\FileAssociations"), &format!(".{extension}"), prog_id);
        plan.text(&format!(r"{application}\SupportedTypes"), &format!(".{extension}"), "");
        // 没人认领的扩展名（系统里这种文件还没有对应的类型）直接归 Anyview：装完双击就能打开。已经有主的不动，
        // 那些要用户自己在系统的“默认应用”里改（Windows 不让程序替用户改）。
        plan.values.push(RegValue {
            key: ext_key.clone(),
            name: String::new(),
            data: RegData::Text(prog_id.to_string()),
            only_if_unset: true,
        });
        if thumbnail_registered(extension, settings) {
            plan.text(&format!(r"{ext_key}\ShellEx\{THUMBNAIL_SLOT}"), "", THUMBNAIL_CLSID);
        }
        if let Some(entry) = menu {
            // 右键菜单：不管这种文件默认由谁打开都有这一项；只在选中一个文件时显示。
            let verb = plan.own(format!(r"{CLASSES}\SystemFileAssociations\.{extension}\shell\{MENU_VERB}"));
            plan.text(&verb, "", entry.title);
            plan.text(&verb, "MultiSelectModel", "Single");
            if entry.icon {
                plan.text(&verb, "Icon", icon.as_str());
            }
            plan.text(&format!(r"{verb}\command"), "", open_command.as_str());
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Value;

    fn install() -> Install {
        Install {
            dir: r"C:\Users\me\AppData\Local\Programs\Anyview\".to_string(),
            version: "0.0.1".to_string(),
            size_kb: 250_000,
        }
    }

    fn text<'a>(plan: &'a RegPlan, key: &str, name: &str) -> Option<&'a str> {
        plan.values
            .iter()
            .find(|v| v.key.eq_ignore_ascii_case(key) && v.name == name)
            .and_then(|v| match &v.data {
                RegData::Text(text) => Some(text.as_str()),
                RegData::Number(_) => None,
            })
    }

    #[test]
    fn the_groups_cover_what_the_core_opens_and_do_not_overlap() {
        let mut registered = COMMON_EXTENSIONS.to_vec();
        registered.extend(more_extensions());
        let mut sorted = registered.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), registered.len(), "有重复的扩展名");
        for extension in crate::formats::RUST_EXTENSIONS.iter().chain(crate::raw::EXTENSIONS) {
            assert!(registered.contains(extension), "{extension} 没有登记");
        }
        for extension in archive_extensions() {
            assert!(!registered.contains(extension));
        }
        // 系统自己会做缩略图的那一组没有开关管；Anyview 做的那一组每个都有。
        for extension in COMMON_EXTENSIONS {
            assert_eq!(thumbnail_switch(extension), None, "{extension}");
        }
        for extension in more_extensions() {
            assert!(thumbnail_switch(extension).is_some(), "{extension}");
        }
    }

    #[test]
    fn default_plan_registers_the_program_its_types_and_thumbnails_but_no_menu() {
        let plan = registry_plan(&install(), &Settings::defaults(), Lang::ZhHans);
        let dir = r"C:\Users\me\AppData\Local\Programs\Anyview";
        assert_eq!(text(&plan, APP_KEY, "InstallDir"), Some(dir));
        assert_eq!(
            text(&plan, r"Software\RegisteredApplications", "Anyview"),
            Some(r"Software\Anyview\Capabilities")
        );
        assert_eq!(
            text(&plan, r"Software\Classes\Anyview.Image\shell\open\command", ""),
            Some(r#""C:\Users\me\AppData\Local\Programs\Anyview\Anyview.exe" "%1""#)
        );
        assert_eq!(
            text(&plan, r"Software\Classes\CLSID\{1DC1F4D7-6312-479A-9043-BCA81CE0234B}\InprocServer32", ""),
            Some(r"C:\Users\me\AppData\Local\Programs\Anyview\anyview_ffi.dll")
        );
        assert_eq!(
            text(&plan, UNINSTALL_KEY, "UninstallString"),
            Some(r#""C:\Users\me\AppData\Local\Programs\Anyview\uninstall.exe""#)
        );
        assert!(plan.values.iter().any(|v| v.key == UNINSTALL_KEY
            && v.name == "EstimatedSize"
            && v.data == RegData::Number(250_000)));
        // 每种能打开的文件：“打开方式”、功能表里各一项；图片和压缩包用各自的类型。
        for extension in ["jpg", "psd", "cr2"] {
            assert_eq!(
                text(&plan, &format!(r"Software\Classes\.{extension}\OpenWithProgids"), "Anyview.Image"),
                Some("")
            );
            assert_eq!(
                text(&plan, r"Software\Anyview\Capabilities\FileAssociations", &format!(".{extension}")),
                Some("Anyview.Image")
            );
        }
        assert_eq!(text(&plan, r"Software\Classes\.zip\OpenWithProgids", "Anyview.Archive"), Some(""));
        // 认领只对没人认领的扩展名：这一条带着“只在空着时写”的标记，别的都不带。
        let claims: Vec<&RegValue> = plan.values.iter().filter(|v| v.only_if_unset).collect();
        assert_eq!(
            claims.len(),
            COMMON_EXTENSIONS.len() + more_extensions().len() + archive_extensions().len()
        );
        assert!(claims.iter().all(|v| v.name.is_empty() && v.key.starts_with(r"Software\Classes\.")));
        // 缩略图：系统不认识的格式登记，常见格式、压缩包不登记。
        let slot = r"ShellEx\{e357fccd-a995-4576-b01f-234630154e96}";
        for extension in ["webp", "psd", "psb", "qoi", "svg", "dng"] {
            assert_eq!(
                text(&plan, &format!(r"Software\Classes\.{extension}\{slot}"), ""),
                Some(THUMBNAIL_CLSID),
                "{extension}"
            );
        }
        for extension in ["jpg", "png", "zip"] {
            assert_eq!(text(&plan, &format!(r"Software\Classes\.{extension}\{slot}"), ""), None, "{extension}");
        }
        // 右键菜单默认不登记（主开关默认关闭）。
        assert!(!plan.values.iter().any(|v| v.key.contains(MENU_VERB)));
        assert!(!plan.owned_keys.iter().any(|k| k.contains(MENU_VERB)));
    }

    #[test]
    fn switches_change_what_is_registered() {
        let mut s = Settings::defaults();
        s.set(settings::THUMB_PSD, Value::Bool(false));
        s.set(settings::THUMB_OTHER, Value::Bool(false));
        s.set(settings::CONTEXT_MENU, Value::Bool(true));
        let plan = registry_plan(&install(), &s, Lang::ZhHans);
        let slot = r"ShellEx\{e357fccd-a995-4576-b01f-234630154e96}";
        let has_thumbnail = |extension: &str| text(&plan, &format!(r"Software\Classes\.{extension}\{slot}"), "").is_some();
        assert!(has_thumbnail("webp") && has_thumbnail("psb"));
        assert!(!has_thumbnail("psd") && !has_thumbnail("qoi") && !has_thumbnail("cr2"));
        // 右键菜单：图片和压缩包上都有，标题按界面语言，带图标，只在选中一个文件时显示。
        for extension in ["jpg", "psd", "zip"] {
            let verb = format!(r"Software\Classes\SystemFileAssociations\.{extension}\shell\Anyview.Browse");
            assert_eq!(text(&plan, &verb, ""), Some("用 Anyview 浏览图片"), "{extension}");
            assert_eq!(text(&plan, &verb, "MultiSelectModel"), Some("Single"));
            assert_eq!(
                text(&plan, &verb, "Icon"),
                Some(r#""C:\Users\me\AppData\Local\Programs\Anyview\Anyview.exe",0"#)
            );
            assert!(text(&plan, &format!(r"{verb}\command"), "").unwrap().ends_with(r#"Anyview.exe" "%1""#));
            assert!(plan.owned_keys.contains(&verb));
        }
        s.set(settings::CONTEXT_MENU_ICON, Value::Bool(false));
        s.set(settings::LANGUAGE, Value::Choice("en".into()));
        let plan = registry_plan(&install(), &s, Lang::ZhHans);
        let verb = r"Software\Classes\SystemFileAssociations\.jpg\shell\Anyview.Browse";
        assert_eq!(text(&plan, verb, ""), Some("Browse with Anyview"));
        assert_eq!(text(&plan, verb, "Icon"), None);
    }

    #[test]
    fn owned_keys_are_told_apart_from_shared_ones() {
        let mut s = Settings::defaults();
        s.set(settings::CONTEXT_MENU, Value::Bool(true));
        let plan = registry_plan(&install(), &s, Lang::En);
        let owned = |key: &str, name: &str| {
            let value = plan.values.iter().find(|v| v.key == key && v.name == name).unwrap();
            plan.is_owned(value)
        };
        assert!(owned(r"Software\Anyview\Capabilities\FileAssociations", ".jpg"));
        assert!(owned(r"Software\Classes\Anyview.Image\shell\open\command", ""));
        assert!(owned(r"Software\Classes\SystemFileAssociations\.jpg\shell\Anyview.Browse\command", ""));
        // 和别的程序共用的项：登记时要记下原来的值，撤销时恢复。
        assert!(!owned(r"Software\RegisteredApplications", "Anyview"));
        assert!(!owned(r"Software\Classes\.jpg\OpenWithProgids", "Anyview.Image"));
        assert!(!owned(r"Software\Classes\.jpg", ""));
        assert!(!owned(r"Software\Classes\.psd\ShellEx\{e357fccd-a995-4576-b01f-234630154e96}", ""));
        // 名字只是开头相同的项不算（Software\Anyview 不管 Software\AnyviewOther）。
        let other = RegValue {
            key: r"Software\AnyviewOther".into(),
            name: String::new(),
            data: RegData::Number(1),
            only_if_unset: false,
        };
        assert!(!plan.is_owned(&other));
    }
}
