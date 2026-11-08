//! 帮助："键盘快捷键"和"支持的格式"两个窗口的内容。
//! 窗口由界面画（标题加一张两列的表，分组标题加粗）；内容在这里按平台和用户的键盘映射算好。
//! "开源许可"也是帮助窗口，但没有表，只有一段可以选中复制的文字（`AppCore::help_text`）。

use crate::i18n::Lang;
use crate::platform::Platform;

/// 哪一个帮助窗口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpKind {
    Shortcuts,
    Formats,
    /// 开源许可：Anyview 和它包含的第三方软件的许可证原文。
    Licenses,
}

impl HelpKind {
    pub fn title(self, lang: Lang) -> &'static str {
        match self {
            Self::Shortcuts => tr!(lang, "键盘快捷键", "Keyboard Shortcuts"),
            Self::Formats => tr!(lang, "支持的格式", "Supported Formats"),
            Self::Licenses => tr!(lang, "开源许可", "Open Source Licenses"),
        }
    }
}

/// 表的一行：分组标题（`heading` 为真，只有左边一列），或左右两列（动作和按键、格式和扩展名）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpRow {
    pub heading: bool,
    pub left: String,
    pub right: String,
}

impl HelpRow {
    pub fn heading(text: &str) -> Self {
        Self {
            heading: true,
            left: text.to_string(),
            right: String::new(),
        }
    }

    pub fn row(left: &str, right: &str) -> Self {
        Self {
            heading: false,
            left: left.to_string(),
            right: right.to_string(),
        }
    }
}

/// 一组：标题和各行（没有行的组不显示）。
pub fn push_group(rows: &mut Vec<HelpRow>, title: &str, items: Vec<HelpRow>) {
    if items.is_empty() {
        return;
    }
    rows.push(HelpRow::heading(title));
    rows.extend(items);
}

fn exts(list: &[&str]) -> String {
    list.iter()
        .map(|e| format!(".{e}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// "支持的格式"（FORMATS.md）：按类分组，每行是格式名和扩展名。HEIC、AVIF 看这台机器有没有系统解码器
/// （macOS 的 ImageIO，Linux 装了的 libheif）。
pub fn formats(platform: Platform, lang: Lang) -> Vec<HelpRow> {
    let platform_has = |ext: &str| {
        crate::decode::platform_extensions()
            .iter()
            .any(|e| e == ext)
    };
    let mut rows = Vec::new();
    let common = vec![
        HelpRow::row("JPEG", &exts(&["jpg", "jpeg", "jpe", "jfif"])),
        HelpRow::row(tr!(lang, "PNG、APNG", "PNG, APNG"), &exts(&["png", "apng"])),
        HelpRow::row("GIF", &exts(&["gif"])),
        HelpRow::row("WebP", &exts(&["webp"])),
        HelpRow::row("BMP", &exts(&["bmp", "dib"])),
        HelpRow::row("TIFF", &exts(&["tif", "tiff"])),
        HelpRow::row("TGA", &exts(&["tga"])),
        HelpRow::row("PNM", &exts(&["pbm", "pgm", "ppm", "pnm", "pam"])),
        HelpRow::row("QOI", &exts(&["qoi"])),
        HelpRow::row(tr!(lang, "ICO、CUR", "ICO, CUR"), &exts(&["ico", "cur"])),
        HelpRow::row(tr!(lang, "PSD、PSB（合成图）", "PSD, PSB (composite)"), &exts(&["psd", "psb"])),
        HelpRow::row("DDS", &exts(&["dds"])),
        HelpRow::row("PCX", &exts(&["pcx"])),
        HelpRow::row("JPEG 2000", &exts(crate::jp2::EXTENSIONS)),
        HelpRow::row("JPEG XR", &exts(crate::jxr::EXTENSIONS)),
    ];
    push_group(&mut rows, tr!(lang, "常见格式", "Common formats"), common);
    let mut newer = vec![
        HelpRow::row("OpenEXR", &exts(&["exr"])),
        HelpRow::row("Radiance HDR", &exts(crate::radiance::EXTENSIONS)),
        HelpRow::row("JPEG XL", &exts(&["jxl"])),
        HelpRow::row("SVG", &exts(&["svg", "svgz"])),
        HelpRow::row(tr!(lang, "Clip Studio Paint（画布预览图）", "Clip Studio Paint (canvas preview)"), &exts(crate::clip::EXTENSIONS)),
    ];
    // AVIF 核心自己也能解，总是列出。
    newer.push(HelpRow::row("AVIF", &exts(crate::avif::EXTENSIONS)));
    if platform_has("heic") {
        newer.push(HelpRow::row(tr!(lang, "HEIC、HEIF", "HEIC, HEIF"), &exts(&["heic", "heif", "hif", "heics"])));
    }
    push_group(&mut rows, tr!(lang, "新格式", "Newer formats"), newer);
    let raw: Vec<String> = crate::raw::EXTENSIONS.iter().map(|e| format!(".{e}")).collect();
    push_group(
        &mut rows,
        tr!(lang, "RAW（显示相机内嵌的预览图）", "RAW (embedded camera preview)"),
        raw.chunks(8)
            .enumerate()
            .map(|(i, chunk)| {
                HelpRow::row(if i == 0 { tr!(lang, "各家相机", "Cameras") } else { "" }, &chunk.join(" "))
            })
            .collect(),
    );
    // 动态 AVIF：核心自己能放，三端都有；HEIF 序列只有 macOS 的 ImageIO 能放。
    let mut animated = String::from(tr!(lang, "GIF、APNG、WebP、JPEG XL、AVIF", "GIF, APNG, WebP, JPEG XL, AVIF"));
    if platform == Platform::Macos && platform_has("heic") {
        animated.push_str(tr!(lang, "、HEIF 序列", ", HEIF sequences"));
    }
    push_group(
        &mut rows,
        tr!(lang, "动图", "Animations"),
        vec![HelpRow::row(tr!(lang, "自动播放", "Played automatically"), &animated)],
    );
    push_group(
        &mut rows,
        tr!(lang, "压缩包（当作文件夹浏览）", "Archives (browsed like folders)"),
        vec![
            HelpRow::row("ZIP", &exts(&["zip", "cbz"])),
            HelpRow::row("7Z", &exts(&["7z", "cb7"])),
            HelpRow::row("RAR", &exts(&["rar", "cbr"])),
        ],
    );
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_grouped_and_list_extensions() {
        let rows = formats(Platform::Linux, Lang::ZhHans);
        assert_eq!(rows[0], HelpRow::heading("常见格式"));
        assert!(rows.iter().any(|r| r.left == "JPEG XL" && r.right == ".jxl"));
        assert!(rows.iter().any(|r| r.right.contains(".cr3")));
        assert!(rows.iter().any(|r| r.left == "RAR" && r.right == ".rar .cbr"));
        let headings = rows.iter().filter(|r| r.heading).count();
        assert_eq!(headings, 5);
        assert_eq!(HelpKind::Formats.title(Lang::En), "Supported Formats");
        let mut empty = Vec::new();
        push_group(&mut empty, "x", Vec::new());
        assert!(empty.is_empty(), "没有行的组不显示");
    }
}
