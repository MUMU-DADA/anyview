//! 浏览集合：所打开图片所在文件夹中的受支持图片，不含子文件夹。
//! 排序方式：文件名（自然排序）、修改日期、大小、类型，可降序；同级文件夹的顺序。

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::formats;
use crate::natural::natural_cmp;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

/// 排序依据（右键菜单"排序方式"）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Size,
    Type,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SortOrder {
    pub key: SortKey,
    /// 降序：把整个顺序反过来（含并列时的次序）。
    pub descending: bool,
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// 两项的先后（升序）。并列时依次比较文件名、修改时间、完整路径，保证顺序稳定。
fn compare(a: &Entry, b: &Entry, key: SortKey) -> Ordering {
    let by_name = || natural_cmp(&a.name, &b.name);
    let by_time = || a.modified.cmp(&b.modified);
    let primary = match key {
        SortKey::Name => by_name().then_with(by_time),
        SortKey::Modified => by_time().then_with(by_name),
        SortKey::Size => a.size.cmp(&b.size).then_with(by_name),
        SortKey::Type => extension(&a.name)
            .cmp(&extension(&b.name))
            .then_with(by_name),
    };
    primary.then_with(|| a.path.cmp(&b.path))
}

pub fn sort_entries(entries: &mut [Entry], order: SortOrder) {
    entries.sort_by(|a, b| {
        let ordering = compare(a, b, order.key);
        if order.descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

/// RAW 文件的筛选（设置"不打开 RAW 图片文件""打开 JPEG 文件时，不打开同名 RAW 文件"，RW1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawFilter {
    /// 浏览集合不含 RAW。
    pub skip_raw: bool,
    /// 同一文件夹里有同名 JPEG 时不含这个 RAW。
    pub skip_with_jpeg: bool,
}

/// 按设置去掉 RAW 文件；`keep` 是用户点名打开的文件，总是留着。
pub fn filter_raw(entries: &mut Vec<Entry>, filter: RawFilter, keep: Option<&Path>) {
    if !filter.skip_raw && !filter.skip_with_jpeg {
        return;
    }
    let is_raw = |e: &Entry| crate::raw::is_raw_extension(&extension(&e.name));
    let key = |e: &Entry| {
        let stem = Path::new(&e.name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        (e.path.parent().map(Path::to_path_buf), stem)
    };
    let jpegs: std::collections::HashSet<_> = if filter.skip_with_jpeg && !filter.skip_raw {
        entries
            .iter()
            .filter(|e| matches!(extension(&e.name).as_str(), "jpg" | "jpeg" | "jpe" | "jfif"))
            .map(key)
            .collect()
    } else {
        Default::default()
    };
    entries.retain(|e| {
        if !is_raw(e) || keep == Some(e.path.as_path()) {
            return true;
        }
        !filter.skip_raw && !jpegs.contains(&key(e))
    });
}

/// 扫描文件夹，返回排好序的图片列表。跳过隐藏文件（含 macOS 在网络盘上留下的 `._` 文件）。
pub fn scan_folder(dir: &Path, order: SortOrder) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !formats::is_supported_name(&name) {
            continue;
        }
        let path = item.path();
        // 符号链接按它指向的文件取信息。
        let metadata = match item.file_type() {
            Ok(t) if t.is_symlink() => fs::metadata(&path),
            _ => item.metadata(),
        };
        let Ok(metadata) = metadata else { continue };
        if metadata.is_file() {
            entries.push(Entry {
                path,
                name,
                modified: metadata.modified().ok(),
                size: metadata.len(),
            });
        }
    }
    sort_entries(&mut entries, order);
    Ok(entries)
}

/// 上级文件夹中的子文件夹和压缩包（不含隐藏的），按当前排序方式排列：
/// 按修改日期排序时用修改时间，其余按名字的自然排序。
pub fn sibling_folders(parent: &Path, order: SortOrder) -> io::Result<Vec<PathBuf>> {
    let mut folders = Vec::new();
    for item in fs::read_dir(parent)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = item.path();
        if path.is_dir() || (crate::archive::is_archive_name(&name) && path.is_file()) {
            let modified = fs::metadata(&path).and_then(|m| m.modified()).ok();
            folders.push(Entry {
                path,
                name,
                modified,
                size: 0,
            });
        }
    }
    let key = match order.key {
        SortKey::Modified => SortKey::Modified,
        _ => SortKey::Name,
    };
    sort_entries(&mut folders, SortOrder { key, ..order });
    Ok(folders.into_iter().map(|f| f.path).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_files_follow_the_two_settings() {
        let entry = |name: &str| Entry {
            path: PathBuf::from(format!("/p/{name}")),
            name: name.to_string(),
            modified: None,
            size: 1,
        };
        let all = || vec![entry("a.JPG"), entry("A.cr2"), entry("b.nef"), entry("c.png")];
        let names = |v: &[Entry]| v.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        let mut v = all();
        filter_raw(&mut v, RawFilter::default(), None);
        assert_eq!(names(&v), ["a.JPG", "A.cr2", "b.nef", "c.png"]);
        let mut v = all();
        filter_raw(&mut v, RawFilter { skip_raw: false, skip_with_jpeg: true }, None);
        assert_eq!(names(&v), ["a.JPG", "b.nef", "c.png"], "同名 JPEG 的 RAW 不要");
        let mut v = all();
        filter_raw(&mut v, RawFilter { skip_raw: true, skip_with_jpeg: true }, None);
        assert_eq!(names(&v), ["a.JPG", "c.png"]);
        // 点名打开的 RAW 留着。
        let mut v = all();
        filter_raw(&mut v, RawFilter { skip_raw: true, skip_with_jpeg: false }, Some(Path::new("/p/b.nef")));
        assert_eq!(names(&v), ["a.JPG", "b.nef", "c.png"]);
    }
    use std::time::Duration;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anyview-scan-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn scans_sorts_and_filters() {
        let dir = temp_dir("filter");
        fs::create_dir_all(dir.join("sub")).unwrap();
        for name in [
            "10.png",
            "2.JPG",
            "1.png",
            "notes.txt",
            ".hidden.png",
            "._1.png",
        ] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        fs::write(dir.join("sub/3.png"), b"x").unwrap();
        let entries = scan_folder(&dir, SortOrder::default()).unwrap();
        assert_eq!(names(&entries), ["1.png", "2.JPG", "10.png"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn entry(name: &str, size: u64, secs: u64) -> Entry {
        Entry {
            path: PathBuf::from(format!("/x/{name}")),
            name: name.to_string(),
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
            size,
        }
    }

    #[test]
    fn four_keys_and_descending() {
        let mut list = vec![
            entry("b.jpg", 300, 20),
            entry("a10.png", 100, 30),
            entry("a2.png", 200, 10),
            entry("c.gif", 100, 40),
        ];
        let sorted = |list: &mut Vec<Entry>, key, descending| {
            sort_entries(list, SortOrder { key, descending });
            names(list)
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sorted(&mut list, SortKey::Name, false),
            ["a2.png", "a10.png", "b.jpg", "c.gif"]
        );
        assert_eq!(
            sorted(&mut list, SortKey::Modified, false),
            ["a2.png", "b.jpg", "a10.png", "c.gif"]
        );
        // 大小相同时按文件名。
        assert_eq!(
            sorted(&mut list, SortKey::Size, false),
            ["a10.png", "c.gif", "a2.png", "b.jpg"]
        );
        // 类型按扩展名，相同时按文件名。
        assert_eq!(
            sorted(&mut list, SortKey::Type, false),
            ["c.gif", "b.jpg", "a2.png", "a10.png"]
        );
        assert_eq!(
            sorted(&mut list, SortKey::Name, true),
            ["c.gif", "b.jpg", "a10.png", "a2.png"]
        );
    }

    /// 性能基准：1 万张图片的文件夹扫描加排序不超过 1 秒。手动运行：
    /// `cargo test -p anyview-core --release -- --ignored scan_ten_thousand --nocapture`
    #[test]
    #[ignore]
    fn scan_ten_thousand_files_within_t6() {
        let dir = temp_dir("t6");
        for i in 0..10_000 {
            fs::write(dir.join(format!("img{i}.png")), b"").unwrap();
        }
        for key in [
            SortKey::Name,
            SortKey::Modified,
            SortKey::Size,
            SortKey::Type,
        ] {
            let start = std::time::Instant::now();
            let entries = scan_folder(
                &dir,
                SortOrder {
                    key,
                    descending: false,
                },
            )
            .unwrap();
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            println!(
                "扫描并按 {key:?} 排序 {} 个文件：{ms:.1} ms",
                entries.len()
            );
            assert_eq!(entries.len(), 10_000);
            assert!(ms < 1000.0);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sibling_folders_skip_hidden_and_files() {
        let dir = temp_dir("siblings");
        for name in ["b10", "b2", "a", ".git"] {
            fs::create_dir_all(dir.join(name)).unwrap();
        }
        fs::write(dir.join("file.png"), b"x").unwrap();
        let folders: Vec<String> = sibling_folders(&dir, SortOrder::default())
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(folders, ["a", "b2", "b10"]);
        let descending = sibling_folders(
            &dir,
            SortOrder {
                key: SortKey::Size,
                descending: true,
            },
        )
        .unwrap();
        assert_eq!(descending.first().unwrap().file_name().unwrap(), "b10");
        fs::remove_dir_all(&dir).unwrap();
    }
}
