//! Flatpak 版和文件管理器的缩略图：Flatpak 不导出缩略图生成器，装了 Flatpak 版，文件管理器也不会用
//! Anyview 做缩略图。用户在 设置 › 格式关联 里按“让文件管理器用 Anyview 做缩略图”，才在自己的用户目录（沙盒外的
//! `~/.local/share/thumbnailers/`）登记一个用 `flatpak run` 调用 Anyview 的缩略图生成器；KDE 的 Dolphin 等直接运行
//! 缩略图生成器的文件管理器就会用它（GNOME 的“文件”把生成器关在自己的 bubblewrap 沙盒里，那里启动不了 Flatpak）。
//! “取消登记”只删带 Anyview 记号的那个文件。
//!
//! - 要放行 `/tmp`：文件管理器让把缩略图写到 /tmp 里的临时文件，沙盒里的 /tmp 是自己的（`--filesystem=host` 也不含 /tmp）。
//! - TryExec 指向 Flatpak 导出的启动脚本：卸载 Anyview 后它不在了，文件管理器就不再用这个生成器。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 安装包里的缩略图生成器：图片类型的列表只有这一份。
const PACKAGED: &str = include_str!("../data/thumbnailers/local.anyview.Anyview.thumbnailer");
/// 我们写的文件里的记号：“取消登记”时只删带它的。
const MARKER: &str = "X-Anyview-Flatpak=true";

/// 登记的位置和名字。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// 缩略图生成器：`<沙盒外的 XDG_DATA_HOME>/thumbnailers/<应用 ID>.thumbnailer`。
    pub file: PathBuf,
    /// Flatpak 导出的启动脚本：`<安装位置>/exports/bin/<应用 ID>`。
    pub launcher: PathBuf,
    pub app_id: String,
}

/// 登记、取消登记没做成的原因（界面按核心的文本说明）。
#[derive(Debug, PartialEq, Eq)]
pub enum Problem {
    /// 不在 Flatpak 里：安装包自己带缩略图生成器，不用登记。
    NotFlatpak,
    /// 同名的文件不是 Anyview 登记的（没有记号），不动它。
    NotOurs(PathBuf),
    Io(String),
}

/// 现在这个 Flatpak 的登记位置。
pub fn current() -> Result<Target, Problem> {
    if !crate::sandbox::in_flatpak() {
        return Err(Problem::NotFlatpak);
    }
    let app_id = crate::sandbox::flatpak_id()
        .and_then(|id| id.into_string().ok())
        .ok_or_else(|| Problem::Io("FLATPAK_ID".to_string()))?;
    let info = std::fs::read_to_string("/.flatpak-info").map_err(|e| Problem::Io(format!("/.flatpak-info: {e}")))?;
    let launcher = launcher_path(&info, &app_id).ok_or_else(|| Problem::Io("/.flatpak-info: app-path".to_string()))?;
    let data_home = host_data_home(std::env::var_os("HOST_XDG_DATA_HOME"), std::env::var_os("HOME"))
        .ok_or_else(|| Problem::Io("HOME".to_string()))?;
    Ok(target_in(&data_home, launcher, &app_id))
}

fn target_in(data_home: &Path, launcher: PathBuf, app_id: &str) -> Target {
    Target {
        file: data_home.join("thumbnailers").join(format!("{app_id}.thumbnailer")),
        launcher,
        app_id: app_id.to_string(),
    }
}

/// Flatpak 导出的启动脚本。`/.flatpak-info` 的 [Instance] app-path 是
/// `<安装位置>/app/<应用 ID>/<架构>/<分支>/<提交>/files`（用户装的在 ~/.local/share/flatpak，系统装的在 /var/lib/flatpak），
/// 启动脚本在 `<安装位置>/exports/bin/<应用 ID>`。
fn launcher_path(flatpak_info: &str, app_id: &str) -> Option<PathBuf> {
    let app_path = ini_value(flatpak_info, "Instance", "app-path")?;
    let base = &app_path[..app_path.find(&format!("/app/{app_id}/"))?];
    (!base.is_empty()).then(|| Path::new(base).join("exports").join("bin").join(app_id))
}

/// 简单的 ini 读取：`[组]` 里 `键=值` 的值。
fn ini_value<'a>(text: &'a str, group: &str, key: &str) -> Option<&'a str> {
    let header = format!("[{group}]");
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == header;
        } else if inside
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim());
        }
    }
    None
}

/// 沙盒外的 XDG_DATA_HOME：Flatpak 把沙盒里的 XDG_DATA_HOME 换成了 `~/.var/app/<应用 ID>/data`，主机上设过的值放在
/// HOST_XDG_DATA_HOME 里；主机上没设时按规范是 `~/.local/share`（沙盒里的 HOME 就是你的主目录）。
fn host_data_home(host: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    host.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|h| h.join(".local").join("share"))
        })
}

/// 安装包里的生成器支持的图片类型（`MimeType=` 那一行）。
fn mime_line() -> &'static str {
    PACKAGED
        .lines()
        .find(|line| line.starts_with("MimeType="))
        .unwrap_or("MimeType=")
}

/// 图片类型有几种（说明里用）。
fn type_count() -> usize {
    mime_line()
        .trim_start_matches("MimeType=")
        .split(';')
        .filter(|t| !t.is_empty())
        .count()
}

/// 登记的缩略图生成器：图片类型与安装包里的相同。
pub fn entry(target: &Target) -> String {
    format!(
        "# Anyview（Flatpak 版）在“设置 › 格式关联”里登记的缩略图生成器：文件管理器用 flatpak run 调用沙盒里的 Anyview。\n\
         # 在 Anyview 里点“取消登记”就删掉；卸载 Anyview 后 TryExec 指的启动脚本不在了，文件管理器就不再用它。\n\
         [Thumbnailer Entry]\n\
         TryExec={launcher}\n\
         Exec=flatpak run --filesystem=/tmp {app_id} --thumbnail %i %o %s\n\
         {mime}\n\
         {MARKER}\n",
        launcher = target.launcher.display(),
        app_id = target.app_id,
        mime = mime_line(),
    )
}

/// 文件是不是 Anyview 登记的（带记号）；不存在时为空。
fn ours(file: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(file).ok()?;
    Some(text.lines().any(|line| line.trim() == MARKER))
}

/// 登记（已登记过就换成现在的内容）。返回支持的图片类型有几种。别人的同名文件（没有记号）不动。
pub fn register(target: &Target) -> Result<usize, Problem> {
    if target.file.exists() && ours(&target.file) != Some(true) {
        return Err(Problem::NotOurs(target.file.clone()));
    }
    let dir = target.file.parent().ok_or_else(|| Problem::Io(target.file.display().to_string()))?;
    std::fs::create_dir_all(dir).map_err(|e| Problem::Io(format!("{}: {e}", dir.display())))?;
    // 先写临时文件再换名：文件管理器不会读到写了一半的。
    let temp = target.file.with_extension("thumbnailer.tmp");
    std::fs::write(&temp, entry(target))
        .and_then(|()| std::fs::rename(&temp, &target.file))
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            Problem::Io(format!("{}: {e}", target.file.display()))
        })?;
    Ok(type_count())
}

/// 取消登记：删掉 Anyview 登记的文件。返回删没删（没登记过时为 false）。
pub fn unregister(target: &Target) -> Result<bool, Problem> {
    match ours(&target.file) {
        None if !target.file.exists() => Ok(false),
        Some(true) => std::fs::remove_file(&target.file)
            .map(|()| true)
            .map_err(|e| Problem::Io(format!("{}: {e}", target.file.display()))),
        _ => Err(Problem::NotOurs(target.file.clone())),
    }
}

/// 自测：登记的位置和现在的样子。
pub fn snapshot() -> serde_json::Value {
    match current() {
        Ok(target) => serde_json::json!({
            "file": target.file,
            "exists": target.file.exists(),
            "ours": ours(&target.file),
            "launcher": target.launcher,
        }),
        Err(problem) => serde_json::json!({ "problem": format!("{problem:?}") }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO_USER: &str = "[Application]\nname=local.anyview.Anyview\nruntime=runtime/org.gnome.Platform/x86_64/51\n\n\
        [Instance]\ninstance-id=123\n\
        app-path=/home/me/.local/share/flatpak/app/local.anyview.Anyview/x86_64/master/abc/files\n\
        branch=master\n";

    #[test]
    fn launcher_follows_the_installation() {
        assert_eq!(
            launcher_path(INFO_USER, "local.anyview.Anyview"),
            Some(PathBuf::from("/home/me/.local/share/flatpak/exports/bin/local.anyview.Anyview"))
        );
        let system = INFO_USER.replace("/home/me/.local/share/flatpak", "/var/lib/flatpak");
        assert_eq!(
            launcher_path(&system, "local.anyview.Anyview"),
            Some(PathBuf::from("/var/lib/flatpak/exports/bin/local.anyview.Anyview"))
        );
        // 别的组里的同名键不算；没有 app-path 或路径不对时为空。
        assert_eq!(launcher_path("[Application]\napp-path=/x/app/local.anyview.Anyview/f\n", "local.anyview.Anyview"), None);
        assert_eq!(launcher_path(INFO_USER, "org.other.App"), None);
    }

    #[test]
    fn data_home_is_the_one_outside_the_sandbox() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(host_data_home(os("/data/me"), os("/home/me")), Some(PathBuf::from("/data/me")));
        assert_eq!(host_data_home(None, os("/home/me")), Some(PathBuf::from("/home/me/.local/share")));
        // 相对路径不可信，退回到主目录下的默认位置。
        assert_eq!(host_data_home(os("relative"), os("/home/me")), Some(PathBuf::from("/home/me/.local/share")));
        assert_eq!(host_data_home(None, None), None);
    }

    #[test]
    fn entry_runs_anyview_through_flatpak_with_the_packaged_types() {
        let target = target_in(
            Path::new("/home/me/.local/share"),
            PathBuf::from("/var/lib/flatpak/exports/bin/local.anyview.Anyview"),
            "local.anyview.Anyview",
        );
        assert_eq!(
            target.file,
            PathBuf::from("/home/me/.local/share/thumbnailers/local.anyview.Anyview.thumbnailer")
        );
        let text = entry(&target);
        assert!(text.contains("\n[Thumbnailer Entry]\n"));
        assert!(text.contains("\nTryExec=/var/lib/flatpak/exports/bin/local.anyview.Anyview\n"));
        assert!(text.contains("\nExec=flatpak run --filesystem=/tmp local.anyview.Anyview --thumbnail %i %o %s\n"));
        assert!(text.contains(&format!("\n{MARKER}\n")));
        // 图片类型与安装包里的一样（含自带的 PAM、CLIP）。
        assert!(text.contains(mime_line()) && mime_line().contains("image/x-clip-studio-paint;"));
        assert!(type_count() > 20);
    }

    #[test]
    fn register_and_unregister_only_touch_our_file() {
        let dir = std::env::temp_dir().join(format!("anyview-flatpak-thumbnailer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = target_in(&dir, PathBuf::from("/var/lib/flatpak/exports/bin/x.y.Z"), "x.y.Z");
        // 没登记过：取消登记什么都不做。
        assert_eq!(unregister(&target), Ok(false));
        assert_eq!(register(&target), Ok(type_count()));
        assert_eq!(std::fs::read_to_string(&target.file).unwrap(), entry(&target));
        // 再按一次照样成功（换成现在的内容）。
        assert_eq!(register(&target), Ok(type_count()));
        assert_eq!(unregister(&target), Ok(true));
        assert!(!target.file.exists());
        // 别人的同名文件：不覆盖、不删。
        std::fs::write(&target.file, "[Thumbnailer Entry]\nExec=other %i %o %s\n").unwrap();
        assert_eq!(register(&target), Err(Problem::NotOurs(target.file.clone())));
        assert_eq!(unregister(&target), Err(Problem::NotOurs(target.file.clone())));
        assert!(target.file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
