//! 悬浮界面的规则与文字（docs/MENUS.md"底部控制栏""边缘切图箭头""图片信息"）：
//! 指针区域、显示与隐藏的时间、进度条的序号换算、左上角图片信息。
//! 坐标用点（逻辑像素），原点在窗口内容区左上角。

use std::time::SystemTime;

use crate::exif::ShootingInfo;
use crate::i18n::{self, Key, Lang};

/// 淡入、淡出时长。
pub const FADE_IN_MS: u32 = 150;
pub const FADE_OUT_MS: u32 = 300;
/// 指针离开控制栏区域后多久淡出。
pub const BAR_HIDE_DELAY_MS: u32 = 1000;
/// 边缘箭头：指针静止多久后淡出。
pub const ARROW_IDLE_MS: u32 = 2000;
/// 控制栏触发区：窗口底部 1/5 高度，至少 100 点。
const BAR_ZONE_MIN: f64 = 100.0;

/// 指针所在的悬停区域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoverZone {
    None,
    /// 左侧 1/6 宽度：显示左箭头。
    Left,
    /// 右侧 1/6 宽度：显示右箭头。
    Right,
    /// 底部 1/5 高度（至少 100 点）：显示控制栏，优先于箭头区。
    Bar,
}

pub fn hover_zone(x: f64, y: f64, width: f64, height: f64) -> HoverZone {
    if width <= 0.0 || height <= 0.0 || !(0.0..=width).contains(&x) || !(0.0..=height).contains(&y)
    {
        return HoverZone::None;
    }
    let bar = (height / 5.0).max(BAR_ZONE_MIN).min(height);
    if y >= height - bar {
        HoverZone::Bar
    } else if x < width / 6.0 {
        HoverZone::Left
    } else if x > width * 5.0 / 6.0 {
        HoverZone::Right
    } else {
        HoverZone::None
    }
}

/// 进度条滑块位置（0–1）：第一张在最左，最后一张在最右。
pub fn progress_fraction(index: usize, total: usize) -> f64 {
    if total <= 1 {
        return 0.0;
    }
    (index.min(total - 1) as f64) / ((total - 1) as f64)
}

/// 进度条上指针位置（0–1）对应的图片序号（从 0 开始），与滑块位置互为反算。
pub fn progress_index(fraction: f64, total: usize) -> Option<usize> {
    if total == 0 || !fraction.is_finite() {
        return None;
    }
    let f = fraction.clamp(0.0, 1.0);
    Some(((f * (total - 1) as f64).round() as usize).min(total - 1))
}

/// 进度提示，如 `841/1862`（序号从 1 开始）。
pub fn position_text(index: usize, total: usize) -> String {
    format!("{}/{total}", index + 1)
}

/// 文件大小，如 `1.8MB`、`523.4KB`、`900B`（不加空格）。
pub fn file_size_text(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1}GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1}MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.1}KB", b / KB)
    } else {
        format!("{bytes}B")
    }
}

/// 修改日期，本地时间，`yyyy/MM/dd HH:mm:ss`。
pub fn date_text(time: SystemTime) -> String {
    let local: chrono::DateTime<chrono::Local> = time.into();
    local.format("%Y/%m/%d %H:%M:%S").to_string()
}

/// 左上角图片信息需要的数据。图片还没解码完时只有文件名一项。
#[derive(Clone, Debug, Default)]
pub struct InfoSource {
    pub name: String,
    pub index: Option<usize>,
    pub total: Option<usize>,
    pub image: Option<ImageFacts>,
}

#[derive(Clone, Debug)]
pub struct ImageFacts {
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub modified: Option<SystemTime>,
    pub format: String,
    pub pixel_format: String,
    /// EXIF 里的拍摄信息。
    pub shooting: Option<ShootingInfo>,
    /// AI 生成的图片：生成工具。
    pub generation: Option<String>,
    /// 打开“生成信息”面板的快捷键（按键盘映射）；没有时为空。
    pub generation_key: Option<String>,
}

/// 图片信息的各行文字：文件名加 [序号/总数]、图片大小、修改日期、图片信息；
/// 有拍摄信息时追加相机、镜头、拍摄参数、拍摄时间。没有的项不出现，不留空行。
pub fn info_lines(source: &InfoSource, lang: Lang) -> Vec<String> {
    if source.name.is_empty() {
        return Vec::new();
    }
    let name = match (source.index, source.total) {
        (Some(i), Some(total)) => format!("{} [{}]", source.name, position_text(i, total)),
        _ => source.name.clone(),
    };
    let mut lines = vec![i18n::text(Key::InfoName, lang).replace("{value}", &name)];
    if let Some(image) = &source.image {
        lines.push(
            i18n::text(Key::InfoSize, lang).replace("{value}", &file_size_text(image.file_size)),
        );
        if let Some(modified) = image.modified {
            lines
                .push(i18n::text(Key::InfoModified, lang).replace("{value}", &date_text(modified)));
        }
        let facts = format!(
            "{}x{} ({},{})",
            image.width, image.height, image.format, image.pixel_format
        );
        lines.push(i18n::text(Key::InfoImage, lang).replace("{value}", &facts));
        if let Some(shooting) = &image.shooting {
            lines.extend(shooting_lines(shooting, lang));
        }
        // AI 生成的图片：生成工具，和看提示词的按键。
        if let Some(tool) = &image.generation {
            lines.push(match &image.generation_key {
                Some(key) => tr!(lang, "AI 生成: {tool} (按 {key} 看提示词)", "AI-generated: {tool} (press {key} for the prompt)")
                    .replace("{tool}", tool)
                    .replace("{key}", key),
                None => tr!(lang, "AI 生成: {tool}", "AI-generated: {tool}").replace("{tool}", tool),
            });
        }
    }
    lines
}

/// 拍摄信息的各行：相机、镜头、拍摄参数（焦距、光圈、快门、ISO 写在一行）、拍摄时间。
pub fn shooting_lines(info: &ShootingInfo, lang: Lang) -> Vec<String> {
    let line = |key: Key, value: &str| i18n::text(key, lang).replace("{value}", value);
    let mut lines = Vec::new();
    if let Some(camera) = camera_text(info.make.as_deref(), info.model.as_deref()) {
        lines.push(line(Key::InfoCamera, &camera));
    }
    if let Some(lens) = &info.lens {
        lines.push(line(Key::InfoLens, lens));
    }
    let mut parts = Vec::new();
    if let Some(focal) = info.focal_length {
        let mut text = format!("{}mm", number_text(focal));
        if let Some(equivalent) = info
            .focal_length_35mm
            .filter(|&e| (f64::from(e) - focal).abs() >= 1.0)
        {
            text.push_str(
                &i18n::text(Key::InfoFocal35, lang).replace("{value}", &format!("{equivalent}mm")),
            );
        }
        parts.push(text);
    }
    if let Some(f) = info.f_number {
        parts.push(format!("f/{}", number_text(f)));
    }
    if let Some(t) = info.exposure_time {
        parts.push(exposure_text(t));
    }
    if let Some(iso) = info.iso {
        parts.push(format!("ISO {iso}"));
    }
    if !parts.is_empty() {
        lines.push(line(Key::InfoExposure, &parts.join("  ")));
    }
    if let Some(taken) = &info.taken {
        lines.push(line(Key::InfoTaken, &exif_date_text(taken)));
    }
    lines
}

/// 相机名：厂商的第一个词加型号；型号已经以厂商开头时只用型号（如 "NIKON CORPORATION" + "NIKON D300" → "NIKON D300"）。
fn camera_text(make: Option<&str>, model: Option<&str>) -> Option<String> {
    let model = model.map(str::trim).filter(|m| !m.is_empty());
    let make = make.map(str::trim).filter(|m| !m.is_empty());
    let brand = make.and_then(|m| m.split_whitespace().next());
    match (brand, model) {
        (Some(brand), Some(model))
            if model.to_lowercase().starts_with(&brand.to_lowercase()) =>
        {
            Some(model.to_string())
        }
        (Some(brand), Some(model)) => Some(format!("{brand} {model}")),
        (None, Some(model)) => Some(model.to_string()),
        (Some(_), None) => make.map(str::to_string),
        (None, None) => None,
    }
}

/// 数字最多保留一位小数，去掉多余的 0（如 5.6、9、4.2）。
fn number_text(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < 1e-9 {
        format!("{}", rounded.round() as i64)
    } else {
        format!("{rounded:.1}")
    }
}

/// 快门：0.3 秒以上写秒数（如 0.5s、2s），更快的写分数（如 1/250s）。
fn exposure_text(seconds: f64) -> String {
    if seconds >= 0.3 {
        format!("{}s", number_text(seconds))
    } else {
        format!("1/{}s", (1.0 / seconds).round() as u64)
    }
}

/// EXIF 的日期 `2024:06:22 08:40:39` 写成与修改日期相同的格式 `2024/06/22 08:40:39`；认不出时原样。
fn exif_date_text(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.len() >= 19
        && bytes[..19].is_ascii()
        && bytes[4] == b':'
        && bytes[7] == b':'
        && bytes[10] == b' '
    {
        format!("{}/{}/{}{}", &text[..4], &text[5..7], &text[8..10], &text[10..19])
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn zones_follow_h1_and_h2() {
        let (w, h) = (900.0, 600.0);
        assert_eq!(hover_zone(450.0, 300.0, w, h), HoverZone::None);
        assert_eq!(hover_zone(100.0, 300.0, w, h), HoverZone::Left);
        assert_eq!(hover_zone(800.0, 300.0, w, h), HoverZone::Right);
        // 底部 1/5 = 120 点；箭头区不含控制栏区。
        assert_eq!(hover_zone(100.0, 500.0, w, h), HoverZone::Bar);
        assert_eq!(hover_zone(450.0, 479.0, w, h), HoverZone::None);
        // 矮窗口时控制栏区至少 100 点。
        assert_eq!(hover_zone(450.0, 210.0, 900.0, 300.0), HoverZone::Bar);
        assert_eq!(hover_zone(-1.0, 300.0, w, h), HoverZone::None);
        assert_eq!(hover_zone(10.0, 10.0, 0.0, 0.0), HoverZone::None);
    }

    #[test]
    fn progress_maps_both_ways() {
        assert_eq!(progress_fraction(0, 9), 0.0);
        assert_eq!(progress_fraction(8, 9), 1.0);
        assert_eq!(progress_fraction(0, 1), 0.0);
        assert_eq!(progress_index(0.5, 9), Some(4));
        assert_eq!(progress_index(1.7, 9), Some(8));
        assert_eq!(progress_index(-3.0, 9), Some(0));
        assert_eq!(progress_index(0.5, 0), None);
        for total in 1..20 {
            for i in 0..total {
                assert_eq!(progress_index(progress_fraction(i, total), total), Some(i));
            }
        }
        assert_eq!(position_text(840, 1862), "841/1862");
    }

    #[test]
    fn sizes_and_dates_use_design_format() {
        assert_eq!(file_size_text(900), "900B");
        assert_eq!(file_size_text(1536), "1.5KB");
        assert_eq!(file_size_text(1_887_437), "1.8MB");
        let text = date_text(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000));
        assert_eq!(text.len(), "2027/01/15 08:00:00".len(), "{text}");
        assert_eq!(&text[4..5], "/");
    }

    #[test]
    fn shooting_lines_follow_e7() {
        let info = ShootingInfo {
            make: Some("NIKON CORPORATION".into()),
            model: Some("NIKON D300".into()),
            lens: Some("70.0-300.0 mm f/4.0-5.6".into()),
            focal_length: Some(300.0),
            focal_length_35mm: Some(450),
            f_number: Some(8.0),
            exposure_time: Some(0.004),
            iso: Some(200),
            taken: Some("2012:11:10 13:22:46".into()),
        };
        assert_eq!(
            shooting_lines(&info, Lang::ZhHans),
            vec![
                "相机: NIKON D300",
                "镜头: 70.0-300.0 mm f/4.0-5.6",
                "拍摄参数: 300mm（等效 450mm）  f/8  1/250s  ISO 200",
                "拍摄时间: 2012/11/10 13:22:46",
            ]
        );
        assert_eq!(
            shooting_lines(&info, Lang::En)[2],
            "Exposure: 300mm (450mm equiv.)  f/8  1/250s  ISO 200"
        );
        // 厂商不在型号里时加在前面；等效焦距与实际相同时不写；慢门写秒数；缺的项不出现。
        let phone = ShootingInfo {
            make: Some("SONY".into()),
            model: Some("DSLR-A550".into()),
            focal_length: Some(28.0),
            focal_length_35mm: Some(28),
            f_number: Some(5.6),
            exposure_time: Some(1.3),
            taken: Some("坏日期".into()),
            ..ShootingInfo::default()
        };
        assert_eq!(
            shooting_lines(&phone, Lang::En),
            vec!["Camera: SONY DSLR-A550", "Exposure: 28mm  f/5.6  1.3s", "Taken: 坏日期"]
        );
        assert!(shooting_lines(&ShootingInfo::default(), Lang::En).is_empty());
        assert_eq!(camera_text(Some("Canon"), Some("Canon EOS R5")).unwrap(), "Canon EOS R5");
        assert_eq!(camera_text(Some("Apple"), None).unwrap(), "Apple");
        assert_eq!(exposure_text(1.0 / 3.0), "0.3s");
        assert_eq!(exposure_text(0.0125), "1/80s");
    }

    #[test]
    fn info_lines_skip_missing_parts() {
        let mut source = InfoSource {
            name: "a.png".into(),
            index: Some(71),
            total: Some(1862),
            image: None,
        };
        assert_eq!(
            info_lines(&source, Lang::ZhHans),
            vec!["文件名: a.png [72/1862]"]
        );
        source.image = Some(ImageFacts {
            width: 1414,
            height: 2048,
            file_size: 1_887_437,
            modified: None,
            format: "PNG".into(),
            pixel_format: "RGBA32".into(),
            shooting: None,
            generation: None,
            generation_key: None,
        });
        assert_eq!(
            info_lines(&source, Lang::ZhHans),
            vec![
                "文件名: a.png [72/1862]",
                "图片大小: 1.8MB",
                "图片信息: 1414x2048 (PNG,RGBA32)"
            ]
        );
        source.total = None;
        assert_eq!(info_lines(&source, Lang::En)[0], "File: a.png");
        assert!(info_lines(&InfoSource::default(), Lang::En).is_empty());
    }
}
