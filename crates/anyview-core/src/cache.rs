//! 解码结果缓存：按像素字节数计算，超过上限时淘汰最久没用到的；
//! 当前图片和受保护的预加载目标不淘汰。多个窗口共用一个缓存，各窗口分别登记要保护的图片。
//!
//! 每张图片可以有多份：原图（无效果）和按当前效果处理后的显示缓冲区（反相、Gamma），
//! 所以开着效果翻页时，预加载的也是处理好的图。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::decode::DecodedImage;
use crate::effects::Effects;

/// 默认缓存上限 1 GB（对应设置“缓存大小”的默认值）。
pub const DEFAULT_BUDGET: usize = 1024 * 1024 * 1024;

type Key = (PathBuf, Effects);

pub struct ImageCache {
    budget: usize,
    used: usize,
    clock: u64,
    items: HashMap<Key, (Arc<DecodedImage>, u64)>,
    /// 各窗口受保护的图片（当前图片和预加载目标），按窗口编号登记。
    protected: HashMap<u64, Protected>,
}

/// 一个窗口要保护的图片：先近后远，前 `keep` 张（当前图片、双页时一起显示的另一页）一定要留。
struct Protected {
    paths: Vec<PathBuf>,
    keep: usize,
}

impl ImageCache {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            clock: 0,
            items: HashMap::new(),
            protected: HashMap::new(),
        }
    }

    /// 登记一个窗口现在要保护的图片（先近后远，所有效果版本都不淘汰；前 `keep` 张一定要留），替换这个窗口
    /// 上次登记的。淘汰时每个窗口只保护它分到的那一份放得下的（[`Self::kept`]）。
    pub fn protect(&mut self, owner: u64, paths: Vec<PathBuf>, keep: usize) {
        self.protected.insert(owner, Protected { paths, keep });
    }

    /// 窗口关掉了：不再保护它的图片。
    pub fn forget(&mut self, owner: u64) {
        self.protected.remove(&owner);
    }

    pub fn get(&mut self, path: &Path, effects: Effects) -> Option<Arc<DecodedImage>> {
        self.clock += 1;
        let clock = self.clock;
        self.items
            .get_mut(&(path.to_path_buf(), effects))
            .map(|(image, used_at)| {
                *used_at = clock;
                Arc::clone(image)
            })
    }

    pub fn contains(&self, path: &Path, effects: Effects) -> bool {
        self.items.contains_key(&(path.to_path_buf(), effects))
    }

    /// 有没有这张图片的任何一个版本。
    pub fn has(&self, path: &Path) -> bool {
        self.items.keys().any(|(p, _)| p == path)
    }

    /// 放入缓存，然后淘汰到上限以内；各窗口登记保护的图片不淘汰。
    pub fn insert(&mut self, path: PathBuf, effects: Effects, image: Arc<DecodedImage>) {
        self.clock += 1;
        let key = (path, effects);
        if let Some((old, _)) = self.items.remove(&key) {
            self.used -= old.byte_len();
        }
        self.used += image.byte_len();
        self.items.insert(key, (image, self.clock));
        self.evict();
    }

    /// 文件现在的修改时间、大小与缓存里这张图片解码时的对不上（解码之后文件被改过）：去掉它的所有版本。
    /// 返回是不是去掉了。
    pub fn remove_stale(&mut self, path: &Path, modified: Option<SystemTime>, size: u64) -> bool {
        let stale = self.items.iter().any(|((p, _), (image, _))| {
            p == path && (image.info.modified != modified || image.info.file_size != size)
        });
        if stale {
            self.remove(path);
        }
        stale
    }

    /// 去掉一张图片的所有版本（文件已被删除）。
    pub fn remove(&mut self, path: &Path) {
        let keys: Vec<Key> = self
            .items
            .keys()
            .filter(|(p, _)| p == path)
            .cloned()
            .collect();
        for key in keys {
            if let Some((image, _)) = self.items.remove(&key) {
                self.used -= image.byte_len();
            }
        }
    }

    fn evict(&mut self) {
        if self.used <= self.budget {
            return;
        }
        let keep = self.kept();
        while self.used > self.budget {
            let victim = self
                .items
                .iter()
                .filter(|((path, _), _)| !keep.contains(path))
                .min_by_key(|(_, (_, used_at))| *used_at)
                .map(|(key, _)| key.clone());
            let Some(victim) = victim else { break };
            if let Some((image, _)) = self.items.remove(&victim) {
                self.used -= image.byte_len();
            }
        }
    }

    /// 现在不能挤掉的图片：每个窗口在它分到的那一份里（“缓存大小”按窗口平分）从近到远放得下的那些，按缓存里
    /// 实际的大小算；一定要留的总在。窗口自己算预读范围时用的是估计的大小，而且只在它自己翻页等时候算：别的窗口
    /// 后来才开始分缓存、缓存改小了，闲着的窗口没重新算，这里也不会让它占着超过它那一份。
    fn kept(&self) -> HashSet<PathBuf> {
        let sizes = self.sizes();
        let share = self.budget / self.owners().max(1);
        let mut kept = HashSet::new();
        for protected in self.protected.values() {
            let mut used = 0usize;
            for (k, path) in protected.paths.iter().enumerate() {
                let bytes = sizes.get(path.as_path()).map_or(0, |&(total, _)| total);
                if k >= protected.keep && used.saturating_add(bytes) > share {
                    break;
                }
                used = used.saturating_add(bytes);
                kept.insert(path.clone());
            }
        }
        kept
    }

    /// 缓存里每张图片占多少字节：所有效果版本加起来、其中原图（无效果）的。一遍算出来（预读的候选一张张查的话，
    /// 每张都要把缓存从头看一遍）。
    pub fn sizes(&self) -> HashMap<&Path, (usize, usize)> {
        let mut sizes: HashMap<&Path, (usize, usize)> = HashMap::new();
        for ((path, effects), (image, _)) in &self.items {
            let entry = sizes.entry(path.as_path()).or_default();
            entry.0 += image.byte_len();
            if effects.is_none() {
                entry.1 += image.byte_len();
            }
        }
        sizes
    }

    /// 登记了要保护的图片的窗口有几个。
    fn owners(&self) -> usize {
        self.protected.values().filter(|p| !p.paths.is_empty()).count()
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }

    /// 改变上限（设置"缓存大小"），超出时立即淘汰；受保护的不淘汰。
    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
        self.evict();
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    /// 几个窗口在分这个缓存：登记了要保护的图片的窗口，加上 `owner` 自己（它还没登记时也算一个）。
    /// 预读按平分的那一份算，几个窗口加起来也不超过上限。
    pub fn sharers(&self, owner: u64) -> usize {
        let others = self
            .protected
            .iter()
            .filter(|(id, p)| **id != owner && !p.paths.is_empty())
            .count();
        others + 1
    }

    /// 清空（解码方式变了：EXIF 方向、ICC 的设置，旧的解码结果都不能再用）。
    pub fn clear(&mut self) {
        self.items.clear();
        self.used = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ImageInfo;

    fn image(bytes: usize) -> Arc<DecodedImage> {
        Arc::new(DecodedImage::new(
            1,
            1,
            vec![0; bytes],
            ImageInfo {
                format: "PNG".into(),
                pixel_format: "RGBA32".into(),
                file_size: 0,
                icc_profile: None,
                modified: None,
                has_alpha: true,
                decode_ms: 0.0,
                shooting: None,
                gif_background: None,
            },
        ))
    }

    const NONE: Effects = Effects::NONE;

    #[test]
    fn sizes_and_sharers() {
        let mut cache = ImageCache::new(1000);
        let a = PathBuf::from("a");
        cache.insert(a.clone(), NONE, image(100));
        cache.insert(a.clone(), Effects { invert: true, ..NONE }, image(60));
        let sizes = cache.sizes();
        assert_eq!(sizes.get(a.as_path()), Some(&(160, 100)), "所有版本加起来、其中原图");
        assert_eq!(sizes.get(Path::new("b")), None);
        assert_eq!(cache.sharers(1), 1);
        cache.protect(1, vec![a.clone()], 1);
        assert_eq!(cache.sharers(1), 1);
        assert_eq!(cache.sharers(2), 2);
        cache.protect(2, Vec::new(), 0);
        assert_eq!(cache.sharers(1), 1, "没有图片要保护的窗口不算");
        cache.forget(1);
        assert_eq!(cache.sharers(2), 1);
    }

    /// 每个窗口只保护它那一份放得下的（按缓存里实际的大小）：别的窗口后来才开始分缓存、缓存改小了，闲着的窗口
    /// 按原来那一份登记的多出来的部分可以挤掉；一定要留的总在。
    #[test]
    fn windows_only_keep_what_fits_in_their_share() {
        let mut cache = ImageCache::new(1000);
        let paths: Vec<PathBuf> = (0..6).map(|i| PathBuf::from(format!("a{i}"))).collect();
        // 窗口 1 一个人时保护 6 张 150 的（900）。
        cache.protect(1, paths.clone(), 1);
        for p in &paths {
            cache.insert(p.clone(), NONE, image(150));
        }
        assert_eq!(cache.used_bytes(), 900);
        // 窗口 2 开始分缓存（各 500）：窗口 1 没重新登记，它只保护前 3 张（450）。
        let b = PathBuf::from("b");
        cache.protect(2, vec![b.clone()], 1);
        cache.insert(b.clone(), NONE, image(400));
        assert!(cache.used_bytes() <= 1000, "{}", cache.used_bytes());
        assert!(paths[..3].iter().all(|p| cache.contains(p, NONE)) && cache.contains(&b, NONE));
        // 缓存改小：马上按新的一份淘汰（不等下次放进新图片）。
        cache.set_budget(400);
        assert!(cache.contains(&paths[0], NONE) && cache.contains(&b, NONE), "一定要留的总在");
        assert!(!cache.contains(&paths[1], NONE) && !cache.contains(&paths[2], NONE));
    }

    #[test]
    fn evicts_least_recently_used_but_keeps_protected() {
        let mut cache = ImageCache::new(250);
        let (a, b, c) = (PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c"));
        cache.insert(a.clone(), NONE, image(100));
        cache.insert(b.clone(), NONE, image(100));
        assert!(cache.get(&a, NONE).is_some()); // a 刚用过，b 最久没用
        cache.protect(1, vec![c.clone()], 1);
        cache.insert(c.clone(), NONE, image(100));
        assert!(cache.contains(&a, NONE) && cache.contains(&c, NONE) && !cache.contains(&b, NONE));
        assert_eq!(cache.used_bytes(), 200);
        // 一定要留的都受保护时宁可超出上限，也不淘汰。
        cache.protect(1, vec![a.clone(), b.clone(), c.clone()], 3);
        cache.insert(b.clone(), NONE, image(100));
        assert_eq!(cache.used_bytes(), 300);
    }

    #[test]
    fn every_window_keeps_its_own_images() {
        // 两个窗口共用缓存：一个窗口翻页不会挤掉另一个窗口正在看的图片；窗口关掉后才能淘汰。
        let mut cache = ImageCache::new(250);
        let (a, b, c) = (PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c"));
        cache.protect(1, vec![a.clone()], 1);
        cache.insert(a.clone(), NONE, image(100));
        cache.protect(2, vec![b.clone()], 1);
        cache.insert(b.clone(), NONE, image(100));
        cache.protect(2, vec![c.clone()], 1);
        cache.insert(c.clone(), NONE, image(100));
        assert!(cache.contains(&a, NONE) && cache.contains(&c, NONE) && !cache.contains(&b, NONE));
        cache.forget(1);
        cache.set_budget(150);
        assert!(!cache.contains(&a, NONE) && cache.contains(&c, NONE));
    }

    #[test]
    fn versions_are_kept_apart_and_removed_together() {
        let mut cache = ImageCache::new(1000);
        let a = PathBuf::from("a");
        let inverted = Effects {
            invert: true,
            ..NONE
        };
        cache.insert(a.clone(), NONE, image(100));
        cache.insert(a.clone(), inverted, image(100));
        assert!(cache.contains(&a, NONE) && cache.contains(&a, inverted));
        assert!(!cache.contains(
            &a,
            Effects {
                gamma: true,
                ..NONE
            }
        ));
        cache.remove(&a);
        assert!(!cache.contains(&a, NONE) && !cache.contains(&a, inverted));
        assert_eq!(cache.used_bytes(), 0);
    }
}
