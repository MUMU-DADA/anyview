//! 压缩包内位置的记忆（设置"自动记忆当前图片在压缩包中的路径"）：每个压缩包上次看到哪一张，
//! 再打开同一个压缩包时从那一张开始。存在设置文件旁边的 `archive-positions.toml`，
//! 最多记 [`LIMIT`] 个，最久没打开的先忘。设置页的"删除"按钮清空。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 最多记多少个压缩包。
pub const LIMIT: usize = 500;

/// 文件名（与设置文件放在同一个文件夹）。
pub const FILE_NAME: &str = "archive-positions.toml";

#[derive(Debug, Default)]
pub struct Positions {
    file: Option<PathBuf>,
    /// (压缩包路径, 包内路径)，最近打开的在最后。
    items: Vec<(String, String)>,
    /// 有没写进文件的改动。
    dirty: bool,
}

impl Positions {
    /// 读记录文件；没有或读不了时从空开始。`file` 为空时不保存。
    pub fn load(file: Option<PathBuf>) -> Self {
        let mut items = Vec::new();
        if let Some(text) = file.as_ref().and_then(|f| fs::read_to_string(f).ok())
            && let Ok(table) = text.parse::<toml::Table>()
            && let Some(list) = table.get("archive").and_then(|v| v.as_array())
        {
            for entry in list {
                let path = entry.get("path").and_then(|v| v.as_str());
                let name = entry.get("entry").and_then(|v| v.as_str());
                if let (Some(path), Some(name)) = (path, name) {
                    items.push((path.to_string(), name.to_string()));
                }
            }
        }
        items.truncate(LIMIT);
        Self {
            file,
            items,
            dirty: false,
        }
    }

    /// 这个压缩包上次看到的那一张（包内路径）。
    pub fn get(&self, archive: &Path) -> Option<&str> {
        let key = archive.to_string_lossy();
        self.items
            .iter()
            .find(|(path, _)| *path == key)
            .map(|(_, entry)| entry.as_str())
    }

    /// 记下现在看到的那一张。
    pub fn set(&mut self, archive: &Path, entry: &str) {
        let key = archive.to_string_lossy().into_owned();
        if self
            .items
            .last()
            .is_some_and(|(path, name)| *path == key && name == entry)
        {
            return;
        }
        self.items.retain(|(path, _)| *path != key);
        self.items.push((key, entry.to_string()));
        if self.items.len() > LIMIT {
            let extra = self.items.len() - LIMIT;
            self.items.drain(..extra);
        }
        self.dirty = true;
    }

    /// 全部忘掉（设置页的"删除"）。
    pub fn clear(&mut self) {
        if !self.items.is_empty() {
            self.items.clear();
            self.dirty = true;
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 有改动时写进文件（先写临时文件再改名）。
    pub fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let Some(file) = self.file.clone() else {
            self.dirty = false;
            return Ok(());
        };
        let list: Vec<toml::Value> = self
            .items
            .iter()
            .map(|(path, entry)| {
                let mut row = toml::Table::new();
                row.insert("path".into(), toml::Value::String(path.clone()));
                row.insert("entry".into(), toml::Value::String(entry.clone()));
                toml::Value::Table(row)
            })
            .collect();
        let mut table = toml::Table::new();
        table.insert("archive".into(), toml::Value::Array(list));
        let text = format!(
            "# Anyview：每个压缩包上次看到的图片（设置 › 书签\"自动记忆当前图片在压缩包中的路径\"）\n{}",
            toml::to_string(&table).map_err(io::Error::other)?
        );
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        let partial = file.with_extension("toml.partial");
        fs::write(&partial, text)?;
        fs::rename(&partial, &file)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_the_last_entry_and_survives_a_reload() {
        let dir = std::env::temp_dir().join(format!("anyview-positions-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join(FILE_NAME);
        let mut p = Positions::load(Some(file.clone()));
        assert!(p.is_empty());
        p.set(Path::new("/x/a.zip"), "ch1/002.jpg");
        p.set(Path::new("/x/b.cbr"), "001.png");
        p.set(Path::new("/x/a.zip"), "ch2/010.jpg");
        assert_eq!(p.get(Path::new("/x/a.zip")), Some("ch2/010.jpg"));
        p.save().unwrap();
        let again = Positions::load(Some(file));
        assert_eq!(again.len(), 2);
        assert_eq!(again.get(Path::new("/x/b.cbr")), Some("001.png"));
        assert_eq!(again.get(Path::new("/x/a.zip")), Some("ch2/010.jpg"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn forgets_the_oldest_beyond_the_limit() {
        let mut p = Positions::load(None);
        for i in 0..LIMIT + 3 {
            p.set(Path::new(&format!("/a/{i}.zip")), "1.jpg");
        }
        assert_eq!(p.len(), LIMIT);
        assert_eq!(p.get(Path::new("/a/0.zip")), None);
        assert!(p.get(Path::new(&format!("/a/{}.zip", LIMIT + 2))).is_some());
        p.clear();
        assert!(p.is_empty());
        assert!(p.save().is_ok());
    }
}
