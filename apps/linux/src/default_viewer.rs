//! Linux 的"设为默认看图软件"和"恢复原来的默认程序"。
//!
//! - 默认程序按 freedesktop 的规范写在 `$XDG_CONFIG_HOME/mimeapps.list` 的 [Default Applications]
//!   （GIO、KDE 都按它）：每种图片类型（`formats::IMAGE_MIME_TYPES`）把 Anyview 排在最前面。
//!   桌面自己的文件（如 KDE 的 `kde-mimeapps.list`，按 XDG_CURRENT_DESKTOP）比它优先：那里已经给某种类型
//!   指定了程序时，那一项也改。压缩包不改（照常用压缩工具打开），只在桌面文件里声明，"打开方式"里有 Anyview。
//! - 改之前的值记在 `$XDG_STATE_HOME/anyview/default-viewer.json`（先写好记录再改，写记录失败就什么都不改）；
//!   "恢复"时这一类型的默认程序还是 Anyview 才换回原来的值（用户后来自己改成别的程序的不动），然后删掉这份记录。
//!   记录读不出来时不恢复、也不删任何东西，报告出错。
//! - 程序的桌面文件：系统里没有安装时（开发版、直接运行的程序），在 `$XDG_DATA_HOME/applications/` 写一个
//!   `local.anyview.Anyview.desktop`，Exec 指向正在运行的这个程序；带 `X-Anyview-Created=true`，
//!   "恢复"时只删带这个记号的。已经有别的同名桌面文件（安装包装的）时不动它。
//! - Flatpak 里改不了系统的默认程序：提示到系统设置里改。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyview_core::formats::{ARCHIVE_MIME_TYPES, IMAGE_MIME_TYPES};
use gtk::glib;

/// set_default 在 Flatpak 里返回的原因。
pub const FLATPAK: &str = "flatpak";

/// 应用桌面文件名。
pub const DESKTOP_ID: &str = "local.anyview.Anyview.desktop";
/// 自己写的桌面文件带这一行。
const CREATED: &str = "X-Anyview-Created=true";
/// 图标的名字（与桌面文件、应用 ID 相同）。
const ICON_NAME: &str = "local.anyview.Anyview";
/// 随桌面文件一起放进 `$XDG_DATA_HOME/icons/hicolor` 的图标（由 tools/icon/make-icons.py 生成）：
/// (主题里的子目录, 扩展名, 内容)。
const ICONS: &[(&str, &str, &[u8])] = &[
    ("48x48", "png", include_bytes!("../data/icons/hicolor/48x48/apps/local.anyview.Anyview.png")),
    ("128x128", "png", include_bytes!("../data/icons/hicolor/128x128/apps/local.anyview.Anyview.png")),
    ("256x256", "png", include_bytes!("../data/icons/hicolor/256x256/apps/local.anyview.Anyview.png")),
    ("scalable", "svg", include_bytes!("../data/icons/hicolor/scalable/apps/local.anyview.Anyview.svg")),
];
const DEFAULTS: &str = "Default Applications";

/// 要读写的几个文件。自测时由 XDG 环境变量指到报告目录里。
pub struct Locations {
    /// `$XDG_DATA_HOME/applications`
    pub applications: PathBuf,
    /// `$XDG_CONFIG_HOME/mimeapps.list`
    pub mimeapps: PathBuf,
    /// `$XDG_STATE_HOME/anyview/default-viewer.json`
    pub record: PathBuf,
    /// 桌面自己的设置文件名（按 XDG_CURRENT_DESKTOP，如 `kde-mimeapps.list`），放在 mimeapps.list 旁边，比它优先。
    pub desktop_lists: Vec<String>,
}

impl Locations {
    pub fn current() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let dir = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| home.as_ref().map(|h| h.join(fallback)))
        };
        let desktop_lists = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .map(str::trim)
            .filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .map(|d| format!("{}-mimeapps.list", d.to_ascii_lowercase()))
            .collect();
        Some(Self {
            applications: dir("XDG_DATA_HOME", ".local/share")?.join("applications"),
            mimeapps: dir("XDG_CONFIG_HOME", ".config")?.join("mimeapps.list"),
            record: dir("XDG_STATE_HOME", ".local/state")?
                .join("anyview")
                .join("default-viewer.json"),
            desktop_lists,
        })
    }

    /// 设置文件名 → 路径（都在 mimeapps.list 所在的目录里）。
    fn list_file(&self, name: &str) -> PathBuf {
        self.mimeapps.with_file_name(name)
    }

    fn desktop_file(&self) -> PathBuf {
        self.applications.join(DESKTOP_ID)
    }

    /// `$XDG_DATA_HOME/icons/hicolor/<子目录>/apps/local.anyview.Anyview.<扩展名>`。
    fn icon_file(&self, size: &str, extension: &str) -> PathBuf {
        let data_home = self.applications.parent().unwrap_or(&self.applications);
        data_home
            .join("icons/hicolor")
            .join(size)
            .join("apps")
            .join(format!("{ICON_NAME}.{extension}"))
    }
}

/// "设为默认"改了什么（恢复时按它换回、删掉）。
#[derive(Default, Debug, PartialEq)]
struct Record {
    /// 改之前的值：设置文件名 → (类型 → 原来的整行值，没有这一项时为空)。
    files: BTreeMap<String, BTreeMap<String, Option<String>>>,
    /// 自己新放进用户图标主题的图标（"子目录/扩展名"，如 "48x48/png"）；本来就有的不算，恢复时不删。
    icons: Vec<String>,
}

impl Record {
    fn is_empty(&self) -> bool {
        self.files.is_empty() && self.icons.is_empty()
    }
}

const GENERIC_LIST: &str = "mimeapps.list";

/// 设为默认看图软件，返回设了多少种图片类型。Flatpak 里改不了（返回的原因由界面换成提示）。
pub fn set_default(at: &Locations, exe: &Path) -> Result<usize, String> {
    if crate::sandbox::in_flatpak() {
        return Err(FLATPAK.to_string());
    }
    let mut record = load_record(&at.record)?;
    // 要改的文件：桌面自己的（已经存在的）在前，通用的 mimeapps.list 最后（总要改）。
    let mut lists: Vec<(String, glib::KeyFile)> = Vec::new();
    for name in at.desktop_lists.iter().map(String::as_str).chain([GENERIC_LIST]) {
        let path = at.list_file(name);
        if name != GENERIC_LIST && !path.exists() {
            continue;
        }
        lists.push((name.to_string(), load_mimeapps(&path)?));
    }
    let mut changed: Vec<usize> = Vec::new();
    for (i, (name, file)) in lists.iter().enumerate() {
        let generic = name == GENERIC_LIST;
        for mime in IMAGE_MIME_TYPES {
            let current = file.string(DEFAULTS, mime).ok().map(|v| v.to_string());
            // 桌面自己的文件只改它已经指定了程序的类型（没指定的由通用文件决定）。
            if (!generic && current.is_none()) || current.as_deref().is_some_and(starts_with_us) {
                continue;
            }
            // 记下现在的值（已经是 Anyview 的上面跳过了，不会把自己记成"原来的"）。上次设为默认之后
            // 用户又在系统里换成了别的程序时，恢复要回到后换的那个，所以盖掉以前记的。那时有的桌面把
            // Anyview 留在后面：记的时候去掉（恢复之后 Anyview 的桌面文件就没有了）。
            let previous = current
                .as_deref()
                .map(|v| if contains_us(v) { without_us(v) } else { v.to_string() });
            record
                .files
                .entry(name.clone())
                .or_default()
                .insert((*mime).to_string(), previous);
            let rest = current.as_deref().map(without_us).unwrap_or_default();
            file.set_string(DEFAULTS, mime, &format!("{DESKTOP_ID};{rest}"));
            if !changed.contains(&i) {
                changed.push(i);
            }
        }
    }
    // 要新放的图标也先记下（本来就有的不记，恢复时不删）。
    let desktop_ours = !at.desktop_file().exists() || is_ours(&at.desktop_file());
    if desktop_ours {
        for (size, extension, _) in ICONS {
            let id = format!("{size}/{extension}");
            if !at.icon_file(size, extension).exists() && !record.icons.contains(&id) {
                record.icons.push(id);
            }
        }
    }
    // 先把原来的值记好再改：记录写不了时什么都不改，以后才恢复得了。
    save_record(&at.record, &record)?;
    write_desktop_file(at, exe)?;
    for i in changed {
        let (name, file) = &lists[i];
        save_mimeapps(file, &at.list_file(name))?;
    }
    Ok(IMAGE_MIME_TYPES.len())
}

/// 换回原来的默认程序，返回换回了多少种；没有设过时为 `Ok(None)`。记录读不出来时报错，什么都不动。
pub fn restore(at: &Locations) -> Result<Option<usize>, String> {
    let record = load_record(&at.record)?;
    let desktop = at.desktop_file();
    let ours = is_ours(&desktop);
    if record.is_empty() && !ours {
        return Ok(None);
    }
    let mut restored = 0;
    for (name, previous_values) in &record.files {
        let path = at.list_file(name);
        if !path.exists() {
            continue;
        }
        let file = load_mimeapps(&path)?;
        let mut any = false;
        for (mime, previous) in previous_values {
            let current = file.string(DEFAULTS, mime).ok();
            if !current.as_deref().is_some_and(starts_with_us) {
                continue;
            }
            match previous {
                Some(value) => file.set_string(DEFAULTS, mime, value),
                None => {
                    let _ = file.remove_key(DEFAULTS, mime);
                }
            }
            any = true;
            if name == GENERIC_LIST {
                restored += 1;
            }
        }
        if any {
            save_mimeapps(&file, &path)?;
        }
    }
    if at.record.exists() {
        std::fs::remove_file(&at.record).map_err(|e| e.to_string())?;
    }
    if ours {
        std::fs::remove_file(&desktop).map_err(|e| e.to_string())?;
    }
    remove_icons(at, &record.icons);
    Ok(Some(restored))
}

/// 自测用：几种类型现在的默认程序，桌面文件和记录在不在、Exec 写的什么。
pub fn snapshot(at: &Locations) -> serde_json::Value {
    let file = load_mimeapps(&at.mimeapps).ok();
    let get = |mime: &str| {
        file.as_ref()
            .and_then(|f| f.string(DEFAULTS, mime).ok())
            .map(|v| v.to_string())
    };
    let exec = std::fs::read_to_string(at.desktop_file()).ok().and_then(|text| {
        text.lines()
            .find_map(|l| l.strip_prefix("Exec=").map(str::to_string))
    });
    serde_json::json!({
        "png": get("image/png"),
        "jpeg": get("image/jpeg"),
        "heic": get("image/heic"),
        "zip": get("application/zip"),
        "desktopFile": at.desktop_file().exists(),
        "icon": at.icon_file("256x256", "png").exists(),
        "exec": exec,
        "record": at.record.exists(),
    })
}

/// 桌面文件：没有时写一个；是自己以前写的就更新 Exec（程序换了位置），别人装的不动。
/// 自己写桌面文件时连图标一起放进用户的 hicolor 主题（已经有同名的别的图标时不动）。
fn write_desktop_file(at: &Locations, exe: &Path) -> Result<(), String> {
    let path = at.desktop_file();
    if path.exists() && !is_ours(&path) {
        return Ok(());
    }
    std::fs::create_dir_all(&at.applications).map_err(|e| e.to_string())?;
    write_atomically(&path, desktop_entry(exe).as_bytes())?;
    for (size, extension, data) in ICONS {
        let icon = at.icon_file(size, extension);
        if icon.exists() && std::fs::read(&icon).ok().as_deref() != Some(*data) {
            continue;
        }
        if let Some(dir) = icon.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        write_atomically(&icon, data)?;
    }
    Ok(())
}

/// 删掉记录里自己新放的图标（内容还与程序里带的相同才删，被别人换过的不动）。
fn remove_icons(at: &Locations, created: &[String]) {
    for (size, extension, data) in ICONS {
        if !created.iter().any(|id| *id == format!("{size}/{extension}")) {
            continue;
        }
        let icon = at.icon_file(size, extension);
        if std::fs::read(&icon).ok().as_deref() == Some(*data) {
            let _ = std::fs::remove_file(&icon);
        }
    }
}

fn is_ours(desktop: &Path) -> bool {
    std::fs::read_to_string(desktop).is_ok_and(|text| text.lines().any(|l| l.trim() == CREATED))
}

/// 桌面文件的内容（freedesktop 的 Desktop Entry 规范）。图标按名字在 hicolor 主题里找（随桌面文件一起放进去）。
pub fn desktop_entry(exe: &Path) -> String {
    entry(&exec_argument(&exe.to_string_lossy()), Some(CREATED))
}

/// 安装包（Flatpak 等）里的桌面文件：Exec 是 PATH 里的 `anyview`，不带“自己写的”记号
/// （“恢复”不会删它）。`anyview --print-desktop-entry` 打出来，打包时写进 share/applications。
pub fn packaged_desktop_entry() -> String {
    entry("anyview", None)
}

fn entry(exec: &str, marker: Option<&str>) -> String {
    let mime: String = IMAGE_MIME_TYPES
        .iter()
        .chain(ARCHIVE_MIME_TYPES)
        .map(|m| format!("{m};"))
        .collect();
    let marker = marker.map(|m| format!("{m}\n")).unwrap_or_default();
    // 各种语言的类别名和说明，文字在核心。
    let names: String = anyview_core::i18n::desktop_entry_texts()
        .into_iter()
        .map(|(locale, generic, comment)| format!("GenericName[{locale}]={generic}\nComment[{locale}]={comment}\n"))
        .collect();
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Anyview\n\
         GenericName=Image Viewer\n\
         Comment=View images\n\
         {names}\
         Exec={exec} %f\n\
         Icon=local.anyview.Anyview\n\
         Terminal=false\n\
         StartupNotify=true\n\
         Categories=Graphics;Viewer;2DGraphics;RasterGraphics;\n\
         MimeType={mime}\n\
         {marker}"
    )
}

/// Exec 里的程序路径：有空格等保留字符时加双引号，引号里的 `"`、`` ` ``、`$`、`\` 前面加反斜杠；
/// `%` 写成 `%%`；最后按桌面文件的字符串规则再把反斜杠写成两个。
fn exec_argument(path: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`',
    ];
    let path = path.replace('%', "%%");
    let quoted = if path.contains(RESERVED) {
        let mut q = String::from("\"");
        for c in path.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
    } else {
        path
    };
    quoted.replace('\\', "\\\\")
}

/// 值的第一个程序是 Anyview。
fn starts_with_us(value: &str) -> bool {
    value.split(';').next().map(str::trim) == Some(DESKTOP_ID)
}

fn contains_us(value: &str) -> bool {
    value.split(';').map(str::trim).any(|id| id == DESKTOP_ID)
}

/// 去掉值里的 Anyview（保持其余程序的顺序，以分号结尾）。
fn without_us(value: &str) -> String {
    value
        .split(';')
        .map(str::trim)
        .filter(|id| !id.is_empty() && *id != DESKTOP_ID)
        .map(|id| format!("{id};"))
        .collect()
}

fn load_mimeapps(path: &Path) -> Result<glib::KeyFile, String> {
    let file = glib::KeyFile::new();
    if path.exists() {
        file.load_from_file(
            path,
            glib::KeyFileFlags::KEEP_COMMENTS | glib::KeyFileFlags::KEEP_TRANSLATIONS,
        )
        .map_err(|e| format!("{}：{e}", path.display()))?;
    }
    Ok(file)
}

fn save_mimeapps(file: &glib::KeyFile, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    write_atomically(path, file.to_data().as_bytes())
}

/// 读改之前的值。没有记录文件时为空；文件读不了、内容不对时报错（不当成"没设过"，免得恢复时误删）：
/// 每个值只能是字符串或 null。也认第一版的记录（只有 mimeapps.list：`{"previous": {类型: 值}}`）。
fn load_record(path: &Path) -> Result<Record, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Record::default()),
        Err(e) => return Err(format!("{}：{e}", path.display())),
    };
    let bad = || format!("{}：内容不对", path.display());
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|_| bad())?;
    let values = |map: &serde_json::Map<String, serde_json::Value>| -> Result<BTreeMap<String, Option<String>>, String> {
        map.iter()
            .map(|(mime, value)| match value {
                serde_json::Value::String(v) => Ok((mime.clone(), Some(v.clone()))),
                serde_json::Value::Null => Ok((mime.clone(), None)),
                _ => Err(bad()),
            })
            .collect()
    };
    let mut record = Record::default();
    if let Some(files) = json.get("files") {
        for (name, map) in files.as_object().ok_or_else(bad)? {
            if !name.ends_with("mimeapps.list") || name.contains('/') {
                return Err(bad());
            }
            record.files.insert(name.clone(), values(map.as_object().ok_or_else(bad)?)?);
        }
        if let Some(icons) = json.get("icons") {
            for icon in icons.as_array().ok_or_else(bad)? {
                record.icons.push(icon.as_str().ok_or_else(bad)?.to_string());
            }
        }
        return Ok(record);
    }
    let previous = json.get("previous").and_then(|p| p.as_object()).ok_or_else(bad)?;
    record.files.insert(GENERIC_LIST.to_string(), values(previous)?);
    Ok(record)
}

fn save_record(path: &Path, record: &Record) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::json!({ "desktop": DESKTOP_ID, "files": record.files, "icons": record.icons });
    write_atomically(path, serde_json::to_string_pretty(&json).unwrap_or_default().as_bytes())
}

/// 先写临时文件再改名：写到一半断电也不会把原来的文件弄坏。
fn write_atomically(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("anyview-{}.tmp", std::process::id()));
    std::fs::write(&tmp, data).map_err(|e| format!("{}：{e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}：{e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_locations(name: &str) -> (PathBuf, Locations) {
        let root = std::env::temp_dir().join(format!("anyview-default-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let at = Locations {
            applications: root.join("data/applications"),
            mimeapps: root.join("config/mimeapps.list"),
            record: root.join("state/anyview/default-viewer.json"),
            desktop_lists: vec!["kde-mimeapps.list".to_string()],
        };
        (root, at)
    }

    /// KDE 自己的 kde-mimeapps.list 比 mimeapps.list 优先：那里指定了的类型也改，恢复时换回去；没指定的不添。
    #[test]
    fn desktop_specific_lists_are_changed_and_restored_too() {
        let (root, at) = temp_locations("kde");
        std::fs::create_dir_all(at.mimeapps.parent().unwrap()).unwrap();
        std::fs::write(at.list_file("kde-mimeapps.list"), "[Default Applications]\nimage/png=org.kde.gwenview.desktop;\n").unwrap();
        set_default(&at, Path::new("/usr/bin/anyview")).unwrap();
        let kde = load_mimeapps(&at.list_file("kde-mimeapps.list")).unwrap();
        assert_eq!(
            kde.string(DEFAULTS, "image/png").unwrap().as_str(),
            "local.anyview.Anyview.desktop;org.kde.gwenview.desktop;"
        );
        assert!(kde.string(DEFAULTS, "image/jpeg").is_err(), "没指定的类型不添进桌面自己的文件");
        restore(&at).unwrap();
        let kde = load_mimeapps(&at.list_file("kde-mimeapps.list")).unwrap();
        assert_eq!(kde.string(DEFAULTS, "image/png").unwrap().as_str(), "org.kde.gwenview.desktop;");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 记录读不出来（坏了）时不恢复、不删桌面文件；记录写不了时什么都不改。
    #[test]
    fn unreadable_records_change_nothing() {
        let (root, at) = temp_locations("broken");
        set_default(&at, Path::new("/usr/bin/anyview")).unwrap();
        std::fs::write(&at.record, "not json").unwrap();
        assert!(restore(&at).is_err());
        assert!(at.desktop_file().exists() && at.record.exists());
        let generic = load_mimeapps(&at.mimeapps).unwrap();
        assert!(starts_with_us(generic.string(DEFAULTS, "image/png").unwrap().as_str()));

        let (root2, at2) = temp_locations("readonly");
        // 记录的位置是个文件，目录建不起来：写不了记录。
        std::fs::create_dir_all(root2.join("state")).unwrap();
        std::fs::write(root2.join("state/anyview"), "a file, not a folder").unwrap();
        assert!(set_default(&at2, Path::new("/usr/bin/anyview")).is_err());
        assert!(!at2.mimeapps.exists() && !at2.desktop_file().exists());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&root2);
    }

    /// 本来就有的同名图标（内容也一样）不是自己放的：恢复时不删。
    #[test]
    fn keeps_icons_that_were_already_there() {
        let (root, at) = temp_locations("icons");
        let icon = at.icon_file("48x48", "png");
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        std::fs::write(&icon, ICONS[0].2).unwrap();
        set_default(&at, Path::new("/usr/bin/anyview")).unwrap();
        assert!(at.icon_file("256x256", "png").exists());
        restore(&at).unwrap();
        assert!(icon.exists(), "本来就有的图标不能删");
        assert!(!at.icon_file("256x256", "png").exists(), "自己放的要删掉");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 第一版的记录（只有 mimeapps.list）也认。
    #[test]
    fn reads_first_version_records() {
        let (root, at) = temp_locations("v1");
        std::fs::create_dir_all(at.record.parent().unwrap()).unwrap();
        std::fs::write(&at.record, r#"{"desktop":"local.anyview.Anyview.desktop","previous":{"image/png":"org.gnome.eog.desktop;","image/jpeg":null}}"#).unwrap();
        let record = load_record(&at.record).unwrap();
        assert_eq!(record.files[GENERIC_LIST]["image/png"].as_deref(), Some("org.gnome.eog.desktop;"));
        assert_eq!(record.files[GENERIC_LIST]["image/jpeg"], None);
        // 值只能是字符串或 null：别的（数字等）整份记录算坏的，恢复时什么都不动。
        std::fs::write(&at.record, r#"{"files":{"mimeapps.list":{"image/png":123}}}"#).unwrap();
        assert!(load_record(&at.record).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sets_and_restores_previous_defaults_exactly() {
        let (root, at) = temp_locations("roundtrip");
        std::fs::create_dir_all(at.mimeapps.parent().unwrap()).unwrap();
        let original = "# 用户自己的注释\n[Default Applications]\nimage/png=org.gnome.eog.desktop;\ntext/plain=org.kde.kate.desktop;\n\n[Added Associations]\nimage/jpeg=org.kde.gwenview.desktop;\n";
        std::fs::write(&at.mimeapps, original).unwrap();
        let exe = Path::new("/opt/any view/anyview");

        assert_eq!(set_default(&at, exe).unwrap(), IMAGE_MIME_TYPES.len());
        let file = load_mimeapps(&at.mimeapps).unwrap();
        assert_eq!(
            file.string(DEFAULTS, "image/png").unwrap().as_str(),
            "local.anyview.Anyview.desktop;org.gnome.eog.desktop;"
        );
        assert_eq!(file.string(DEFAULTS, "image/jpeg").unwrap().as_str(), "local.anyview.Anyview.desktop;");
        assert_eq!(file.string(DEFAULTS, "text/plain").unwrap().as_str(), "org.kde.kate.desktop;");
        let desktop = std::fs::read_to_string(at.desktop_file()).unwrap();
        assert!(desktop.contains("Exec=\"/opt/any view/anyview\" %f\n"), "{desktop}");
        assert!(desktop.contains("Icon=local.anyview.Anyview\n"));
        assert!(at.icon_file("256x256", "png").is_file() && at.icon_file("scalable", "svg").is_file());
        assert!(desktop.contains("image/x-canon-cr3;") && desktop.contains("application/vnd.comicbook+zip;"));

        // 再按一次不会把 Anyview 自己记成"原来的"。
        set_default(&at, exe).unwrap();
        // 用户后来自己把 WebP 改成了别的程序：恢复时不动它。
        let file = load_mimeapps(&at.mimeapps).unwrap();
        file.set_string(DEFAULTS, "image/webp", "org.gnome.Loupe.desktop;");
        save_mimeapps(&file, &at.mimeapps).unwrap();

        assert_eq!(restore(&at).unwrap(), Some(IMAGE_MIME_TYPES.len() - 1));
        let file = load_mimeapps(&at.mimeapps).unwrap();
        assert_eq!(file.string(DEFAULTS, "image/png").unwrap().as_str(), "org.gnome.eog.desktop;");
        assert!(file.string(DEFAULTS, "image/jpeg").is_err());
        assert_eq!(file.string(DEFAULTS, "image/webp").unwrap().as_str(), "org.gnome.Loupe.desktop;");
        assert_eq!(file.string("Added Associations", "image/jpeg").unwrap().as_str(), "org.kde.gwenview.desktop;");
        assert!(std::fs::read_to_string(&at.mimeapps).unwrap().contains("# 用户自己的注释"));
        assert!(!at.desktop_file().exists() && !at.record.exists());
        assert!(!at.icon_file("256x256", "png").exists() && !at.icon_file("scalable", "svg").exists());
        assert_eq!(restore(&at).unwrap(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn leaves_someone_elses_desktop_file_alone() {
        let (root, at) = temp_locations("foreign");
        std::fs::create_dir_all(&at.applications).unwrap();
        std::fs::write(at.desktop_file(), "[Desktop Entry]\nType=Application\nName=Anyview\nExec=anyview %f\n").unwrap();
        set_default(&at, Path::new("/usr/bin/anyview")).unwrap();
        assert!(!std::fs::read_to_string(at.desktop_file()).unwrap().contains(CREATED));
        restore(&at).unwrap();
        assert!(at.desktop_file().exists(), "安装包装的桌面文件不能删");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn packaged_desktop_file_has_no_marker_and_lists_all_types() {
        let text = packaged_desktop_entry();
        let file = glib::KeyFile::new();
        file.load_from_data(&text, glib::KeyFileFlags::NONE).unwrap();
        assert_eq!(file.string("Desktop Entry", "Exec").unwrap().as_str(), "anyview %f");
        assert_eq!(file.string("Desktop Entry", "Icon").unwrap().as_str(), ICON_NAME);
        assert!(!text.contains(CREATED), "安装包的桌面文件不能带“自己写的”记号");
        let mime = file.string("Desktop Entry", "MimeType").unwrap();
        for m in IMAGE_MIME_TYPES.iter().chain(ARCHIVE_MIME_TYPES) {
            assert!(mime.split(';').any(|x| x == *m), "{m}");
        }
        // 开发版写的桌面文件仍带记号。
        assert!(desktop_entry(Path::new("/usr/bin/anyview")).contains(CREATED));
    }

    #[test]
    fn quotes_exec_paths() {
        assert_eq!(exec_argument("/usr/bin/anyview"), "/usr/bin/anyview");
        assert_eq!(exec_argument("/a b/any"), "\"/a b/any\"");
        assert_eq!(exec_argument("/a$b/any"), "\"/a\\\\$b/any\"");
        assert_eq!(exec_argument("/100%/any"), "/100%%/any");
    }

    /// 设为默认之后用户在系统里换成了别的程序，又设了一次：恢复时回到后换的那个，不是最早记的。
    #[test]
    fn restore_goes_back_to_the_latest_previous_default() {
        let (root, at) = temp_locations("latest");
        std::fs::create_dir_all(at.mimeapps.parent().unwrap()).unwrap();
        std::fs::write(&at.mimeapps, "[Default Applications]\nimage/png=org.gnome.eog.desktop;\n").unwrap();
        let exe = Path::new("/usr/bin/anyview");
        set_default(&at, exe).unwrap();
        let file = load_mimeapps(&at.mimeapps).unwrap();
        // 有的桌面换默认程序时把原来的留在后面。
        file.set_string(
            DEFAULTS,
            "image/png",
            "org.kde.gwenview.desktop;local.anyview.Anyview.desktop;org.gnome.eog.desktop;",
        );
        save_mimeapps(&file, &at.mimeapps).unwrap();
        set_default(&at, exe).unwrap();
        let file = load_mimeapps(&at.mimeapps).unwrap();
        assert_eq!(
            file.string(DEFAULTS, "image/png").unwrap().as_str(),
            "local.anyview.Anyview.desktop;org.kde.gwenview.desktop;org.gnome.eog.desktop;"
        );
        // 再按一次（已经是 Anyview）：记录不变。
        set_default(&at, exe).unwrap();
        restore(&at).unwrap();
        let file = load_mimeapps(&at.mimeapps).unwrap();
        assert_eq!(
            file.string(DEFAULTS, "image/png").unwrap().as_str(),
            "org.kde.gwenview.desktop;org.gnome.eog.desktop;",
            "恢复的值里没有 Anyview"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
