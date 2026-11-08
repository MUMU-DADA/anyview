//! 规范路径（打开文件、文件夹时）。核心里的路径都是规范路径：同一个文件只有一种写法，比较、查找序号才对得上
//! （macOS 的 /var 是 /private/var 的链接；Windows 的大小写、映射的网络盘）。
//!
//! Windows 上 `std::fs::canonicalize` 给出的是 `\\?\C:\…`、`\\?\UNC\服务器\共享\…` 这种写法，系统的很多接口不认
//! （Windows 10 上WIC 解码、复制到剪贴板都报"路径包含无效字符"，回收站、桌面背景、资源管理器
//! 同样不认）。能用普通写法表示同一个文件时就用普通写法：本地盘交给 dunce（去掉前缀，保留文件名里的保留字等特殊情况）；
//! 网络路径换成 `\\服务器\共享\…`，再规范一次核对确实是同一个文件才用。其他系统上就是 `std::fs::canonicalize`。

use std::io;
use std::path::{Path, PathBuf};

/// 规范路径；Windows 上尽量用普通写法（见模块说明）。
pub fn canonical(path: &Path) -> io::Result<PathBuf> {
    let canonical = dunce::canonicalize(path)?;
    #[cfg(windows)]
    {
        if let Some(plain) = plain_unc(&canonical) {
            return Ok(plain);
        }
    }
    Ok(canonical)
}

/// `\\?\UNC\服务器\共享\…` → `\\服务器\共享\…`：普通写法规范以后还是同一个路径（没有末尾的点、空格，不是保留字，
/// 不太长），才用它。
#[cfg(windows)]
fn plain_unc(canonical: &Path) -> Option<PathBuf> {
    let text = canonical.to_str()?;
    let rest = text.strip_prefix(r"\\?\UNC\")?;
    let plain = format!(r"\\{rest}");
    // 普通写法的长度限制（MAX_PATH，含结尾的 0）。
    if plain.encode_utf16().count() >= 260 {
        return None;
    }
    let plain = PathBuf::from(plain);
    (std::fs::canonicalize(&plain).ok()? == canonical).then_some(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_paths_are_absolute_and_resolve_links() {
        let dir = std::env::temp_dir().join(format!("anyview-paths-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        let relative_ish = dir.join("sub").join("..").join("a.png");
        let canonical = canonical(&relative_ish).unwrap();
        assert!(canonical.is_absolute());
        assert_eq!(canonical, super::canonical(&dir.join("a.png")).unwrap());
        assert!(canonical.ends_with("a.png"));
        assert!(!canonical.to_string_lossy().starts_with(r"\\?\"));
        assert!(super::canonical(&dir.join("missing.png")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
