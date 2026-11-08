//! 选择图片窗口：列表每一行的文字、排序按钮的文字，以及缩略图。
//!
//! 窗口由界面画；这里给出与平台无关的部分。缩略图在后台按界面要的那几张生成（只要看得见的），
//! 放在各窗口共用的小缓存里（[`ThumbCache`]）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::collection::{Entry, SortKey, SortOrder};
use crate::decode::{DecodeOptions, DecodedImage};
use crate::i18n::Lang;

/// 缩略图的长边（像素）：缩略图带里最大的格子约 96 点，Retina 屏上是 192 像素。
pub const THUMB_SIZE: u32 = 192;
/// 缩略图缓存最多留多少张（每张最多 192x192x4 ≈ 144 KB，总共不到 30 MB）。
pub const THUMB_LIMIT: usize = 200;

/// 列表的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// 浏览集合里的路径（压缩包里的图片是虚拟路径），请求缩略图时用。
    pub path: String,
    /// 显示的名字：所在文件夹（或压缩包）名加文件名，如"Pictures/a.jpg"。
    pub name: String,
    /// 修改日期，与图片信息同一写法。
    pub modified: String,
    /// 大小，三位有效数字，如"922 KB""2.20 MB"。
    pub size: String,
}

/// 浏览集合的一项 → 列表的一行。`container` 是浏览的文件夹或压缩包。
pub fn row(entry: &Entry, container: Option<&Path>) -> Row {
    Row {
        path: entry.path.to_string_lossy().into_owned(),
        name: display_name(entry, container),
        modified: entry
            .modified
            .map(crate::overlay::date_text)
            .unwrap_or_default(),
        size: size_text(entry.size),
    }
}

/// 列表里的名字：文件夹名加文件名；压缩包里的图片是压缩包名加包内路径。
pub fn display_name(entry: &Entry, container: Option<&Path>) -> String {
    if let Some(container) = container
        && let Ok(inner) = entry.path.strip_prefix(container)
        && let Some(parent) = container.file_name()
    {
        let inner = inner
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if !inner.is_empty() {
            return format!("{}/{inner}", parent.to_string_lossy());
        }
    }
    entry.name.clone()
}

/// 文件大小：三位有效数字加单位（如 "922 KB""2.20 MB""10.4 MB"）。
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if value < 10.0 {
        2
    } else if value < 100.0 {
        1
    } else {
        0
    };
    format!("{value:.digits$} {}", UNITS[unit])
}

/// 排序按钮上的文字（如"按名称排序"），降序时加注。
pub fn sort_label(order: SortOrder, lang: Lang) -> String {
    let base = match order.key {
        SortKey::Name => tr!(lang, "按名称排序", "Sort by Name"),
        SortKey::Modified => tr!(lang, "按修改日期排序", "Sort by Date Modified"),
        SortKey::Size => tr!(lang, "按大小排序", "Sort by Size"),
        SortKey::Type => tr!(lang, "按类型排序", "Sort by Type"),
    };
    if order.descending {
        tr!(lang, "{base}（降序）", "{base} (Descending)").replace("{base}", base)
    } else {
        base.to_string()
    }
}

/// 生成一张缩略图（长边不超过 [`THUMB_SIZE`]，BGRA 预乘）。`original` 是缓存里已经解码好的原图（有就不再解码）。
/// JPEG 带足够大的 EXIF 缩略图时直接用它（快）；否则完整解码再缩小。
pub fn make_thumbnail(
    path: &Path,
    options: DecodeOptions,
    original: Option<Arc<DecodedImage>>,
) -> Result<DecodedImage, String> {
    if let Some(image) = original {
        return Ok(downscale(&image, THUMB_SIZE));
    }
    let source = crate::archive::materialize(path).map_err(|e| e.to_string())?;
    if let Some(preview) = crate::jpeg::preview_at_least(&source.file, 0, options) {
        let small = &preview.image;
        let long = small.width.max(small.height);
        // 有的相机把 3:2 的照片存成带黑边的 4:3 缩略图：比例对不上就不用。
        let ratio = |w: u32, h: u32| f64::from(w) / f64::from(h.max(1));
        let same_shape =
            (ratio(small.width, small.height) / ratio(preview.width, preview.height) - 1.0).abs() < 0.05;
        if long >= 120 && same_shape {
            return Ok(downscale(small, THUMB_SIZE));
        }
    }
    let image = crate::decode::decode_file_with(path, options).map_err(|e| e.to_string())?;
    Ok(downscale(&image, THUMB_SIZE))
}

/// 按比例缩到长边不超过 `max`（本来就小时原样复制）。超大图用它的代理纹理缩，快得多。
pub fn downscale(image: &DecodedImage, max: u32) -> DecodedImage {
    let source = image.proxy.as_deref().unwrap_or(image);
    let long = image.width.max(image.height).max(1);
    let scale = (f64::from(max) / f64::from(long)).min(1.0);
    let width = ((f64::from(image.width) * scale).round() as u32).max(1);
    let height = ((f64::from(image.height) * scale).round() as u32).max(1);
    let packed = packed(source);
    if width == source.width && height == source.height {
        return DecodedImage::new(width, height, packed, image.info.clone());
    }
    let resized = ImageRef::new(source.width, source.height, &packed, PixelType::U8x4)
        .ok()
        .and_then(|src| {
            let mut dst = Image::new(width, height, PixelType::U8x4);
            // 缓冲区是预乘 Alpha，直接插值。
            let options = ResizeOptions::new()
                .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
                .use_alpha(false);
            Resizer::new().resize(&src, &mut dst, &options).ok()?;
            Some(dst.into_vec())
        });
    match resized {
        Some(pixels) => DecodedImage::new(width, height, pixels, image.info.clone()),
        None => DecodedImage::new(source.width, source.height, packed, image.info.clone()),
    }
}

fn packed(image: &DecodedImage) -> Vec<u8> {
    let row = image.width as usize * 4;
    if image.stride == row {
        return image.pixels.clone();
    }
    (0..image.height as usize)
        .flat_map(|y| image.pixels[y * image.stride..y * image.stride + row].iter().copied())
        .collect()
}

/// 各窗口共用的缩略图缓存：按路径和文件的修改时间存，最久没用的先丢；记下生成失败的，不反复重试。
#[derive(Default)]
pub struct ThumbCache {
    items: HashMap<PathBuf, (Option<SystemTime>, Arc<DecodedImage>, u64)>,
    failed: HashSet<(PathBuf, Option<SystemTime>)>,
    tick: u64,
}

impl ThumbCache {
    pub fn get(&mut self, path: &Path, modified: Option<SystemTime>) -> Option<Arc<DecodedImage>> {
        self.tick += 1;
        let tick = self.tick;
        let (stamp, image, used) = self.items.get_mut(path)?;
        if *stamp != modified {
            return None;
        }
        *used = tick;
        Some(Arc::clone(image))
    }

    pub fn has_failed(&self, path: &Path, modified: Option<SystemTime>) -> bool {
        self.failed.contains(&(path.to_path_buf(), modified))
    }

    pub fn insert(&mut self, path: PathBuf, modified: Option<SystemTime>, image: Arc<DecodedImage>) {
        self.tick += 1;
        self.items.insert(path, (modified, image, self.tick));
        while self.items.len() > THUMB_LIMIT {
            let Some(oldest) = self
                .items
                .iter()
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(p, _)| p.clone())
            else {
                break;
            };
            self.items.remove(&oldest);
        }
    }

    pub fn mark_failed(&mut self, path: PathBuf, modified: Option<SystemTime>) {
        if self.failed.len() > 10_000 {
            self.failed.clear();
        }
        self.failed.insert((path, modified));
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ImageInfo;

    fn entry(path: &str, size: u64) -> Entry {
        Entry {
            path: PathBuf::from(path),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            modified: None,
            size,
        }
    }

    fn image(width: u32, height: u32) -> DecodedImage {
        DecodedImage::new(
            width,
            height,
            [10u8, 20, 30, 255].repeat((width * height) as usize),
            ImageInfo {
                format: "PNG".into(),
                pixel_format: "RGB24".into(),
                file_size: 0,
                icc_profile: None,
                modified: None,
                has_alpha: false,
                decode_ms: 0.0,
                shooting: None,
                gif_background: None,
            },
        )
    }

    #[test]
    fn names_carry_the_folder_or_archive() {
        let folder = Path::new("/home/u/Pictures");
        assert_eq!(
            display_name(&entry("/home/u/Pictures/a.jpg", 1), Some(folder)),
            "Pictures/a.jpg"
        );
        let archive = Path::new("/x/book.cbz");
        assert_eq!(
            display_name(&entry("/x/book.cbz/ch1/002.jpg", 1), Some(archive)),
            "book.cbz/ch1/002.jpg"
        );
        assert_eq!(display_name(&entry("/elsewhere/b.png", 1), Some(folder)), "b.png");
        assert_eq!(display_name(&entry("/elsewhere/b.png", 1), None), "b.png");
    }

    #[test]
    fn sizes_use_three_significant_digits() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(922 * 1024), "922 KB");
        assert_eq!(size_text(2_306_867), "2.20 MB");
        assert_eq!(size_text(10_905_190), "10.4 MB");
        assert_eq!(size_text(5 * 1024 * 1024 * 1024), "5.00 GB");
        assert_eq!(size_text(1023 * 1024), "1.00 MB");
    }

    #[test]
    fn sort_label_names_the_key_and_direction() {
        let mut order = SortOrder::default();
        assert_eq!(sort_label(order, Lang::ZhHans), "按名称排序");
        order.key = SortKey::Size;
        order.descending = true;
        assert_eq!(sort_label(order, Lang::ZhHans), "按大小排序（降序）");
        assert_eq!(sort_label(order, Lang::En), "Sort by Size (Descending)");
    }

    #[test]
    fn thumbnails_keep_the_aspect_and_never_grow() {
        let big = downscale(&image(400, 100), THUMB_SIZE);
        assert_eq!((big.width, big.height), (192, 48));
        assert_eq!(&big.pixels[..4], &[10, 20, 30, 255]);
        let small = downscale(&image(50, 80), THUMB_SIZE);
        assert_eq!((small.width, small.height), (50, 80));
    }

    #[test]
    fn cache_drops_the_least_recently_used_and_checks_the_date() {
        let mut cache = ThumbCache::default();
        let t = Some(SystemTime::UNIX_EPOCH);
        for i in 0..THUMB_LIMIT + 2 {
            if i == THUMB_LIMIT {
                // 第 0 张刚用过，不会先被丢掉；先丢的是第 1、2 张。
                assert!(cache.get(Path::new("/0"), t).is_some());
            }
            cache.insert(PathBuf::from(format!("/{i}")), t, Arc::new(image(1, 1)));
        }
        assert_eq!(cache.len(), THUMB_LIMIT);
        assert!(cache.get(Path::new("/0"), t).is_some());
        assert!(cache.get(Path::new("/1"), t).is_none());
        assert!(cache.get(Path::new("/2"), t).is_none());
        assert!(cache.get(Path::new("/3"), t).is_some());
        assert!(cache.get(Path::new("/0"), None).is_none(), "文件改过就不算");
        cache.mark_failed(PathBuf::from("/bad"), t);
        assert!(cache.has_failed(Path::new("/bad"), t));
        assert!(!cache.has_failed(Path::new("/bad"), None));
    }
}
