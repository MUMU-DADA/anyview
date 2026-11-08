//! 最近打开的文件、文件夹、压缩包（菜单栏"文件 › 打开最近使用"）。只在 Windows 上用：macOS 用系统自己的
//! "最近使用"，Linux 没有菜单栏。存在设置文件旁边的 `recent.toml`，最多记 [`LIMIT`] 个，最近打开的在最前；
//! 设置"不保存浏览过的文件夹路径"打开时不记，已经记下的清掉。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 最多记多少个（与 macOS 的"最近使用"默认个数相同）。
pub const LIMIT: usize = 10;

/// 文件名（与设置文件放在同一个文件夹）。
pub const FILE_NAME: &str = "recent.toml";

#[derive(Debug, Default)]
pub struct Recent {
    file: Option<PathBuf>,
    /// 最近打开的在最前。
    items: Vec<String>,
}

impl Recent {
    /// 读记录文件；没有或读不了时从空开始。`file` 为空时不保存。
    pub fn load(file: Option<PathBuf>) -> Self {
        let mut items = Vec::new();
        if let Some(text) = file.as_ref().and_then(|f| fs::read_to_string(f).ok())
            && let Ok(table) = text.parse::<toml::Table>()
            && let Some(list) = table.get("recent").and_then(|v| v.as_array())
        {
            for entry in list {
                if let Some(path) = entry.as_str()
                    && !path.is_empty()
                    && !items.iter().any(|p| p == path)
                {
                    items.push(path.to_string());
                }
            }
        }
        items.truncate(LIMIT);
        Self { file, items }
    }

    /// 最近打开的路径，最近的在最前。
    pub fn list(&self) -> &[String] {
        &self.items
    }

    /// 记下刚打开的这一个：挪到最前，超过 [`LIMIT`] 个时忘掉最久的。已经在最前时返回 false（不用再写文件）。
    pub fn note(&mut self, path: &Path) -> bool {
        let key = path.to_string_lossy().into_owned();
        if self.items.first() == Some(&key) {
            return false;
        }
        self.items.retain(|p| *p != key);
        self.items.insert(0, key);
        self.items.truncate(LIMIT);
        true
    }

    /// 清空。原来就是空的时返回 false。
    pub fn clear(&mut self) -> bool {
        let had = !self.items.is_empty();
        self.items.clear();
        had
    }

    /// 写进文件（先写临时文件再改名）。空的时候删掉文件。
    pub fn save(&self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if self.items.is_empty() {
            return match fs::remove_file(file) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let list: Vec<toml::Value> = self
            .items
            .iter()
            .map(|p| toml::Value::String(p.clone()))
            .collect();
        let mut table = toml::Table::new();
        table.insert("recent".to_string(), toml::Value::Array(list));
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let temp = file.with_extension(format!("toml.{}.tmp", std::process::id()));
        fs::write(&temp, table.to_string())?;
        fs::rename(&temp, file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这一项测试自己的空文件夹（系统临时目录里，带进程号）。
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anyview-recent-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn newest_first_without_duplicates_and_survives_a_reload() {
        let dir = scratch("reload");
        let file = dir.join(FILE_NAME);
        let mut r = Recent::load(Some(file.clone()));
        assert!(r.list().is_empty());
        assert!(r.note(Path::new("/pics/a.png")));
        assert!(r.note(Path::new("/pics/b")));
        assert!(r.note(Path::new("/pics/a.png")));
        assert!(!r.note(Path::new("/pics/a.png")), "已经在最前，不用再写");
        assert_eq!(r.list(), ["/pics/a.png", "/pics/b"]);
        r.save().unwrap();
        let again = Recent::load(Some(file.clone()));
        assert_eq!(again.list(), ["/pics/a.png", "/pics/b"]);
        // 清空以后文件也删掉。
        let mut again = again;
        assert!(again.clear());
        assert!(!again.clear());
        again.save().unwrap();
        assert!(!file.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn forgets_the_oldest_beyond_the_limit() {
        let mut r = Recent::load(None);
        for i in 0..LIMIT + 3 {
            r.note(Path::new(&format!("/pics/{i}.png")));
        }
        assert_eq!(r.list().len(), LIMIT);
        assert_eq!(r.list()[0], format!("/pics/{}.png", LIMIT + 2));
        assert!(!r.list().iter().any(|p| p == "/pics/0.png"));
    }

    #[test]
    fn a_broken_file_starts_empty() {
        let dir = scratch("broken");
        let file = dir.join(FILE_NAME);
        fs::write(&file, "recent = [1, \"\", \"/x.png\", \"/x.png\"\n").unwrap();
        assert!(Recent::load(Some(file.clone())).list().is_empty());
        fs::write(&file, "recent = [1, \"\", \"/x.png\", \"/x.png\"]\n").unwrap();
        assert_eq!(Recent::load(Some(file)).list(), ["/x.png"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
