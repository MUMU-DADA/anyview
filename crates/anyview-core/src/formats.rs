//! 支持的格式（docs/FORMATS.md）。浏览集合按扩展名筛选；解码时再按文件头识别真实格式。

/// 由内置解码器直接支持的格式扩展名（小写，不带点）。
pub const RUST_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", // JPEG
    "png", "apng", // PNG
    "gif", "webp", "bmp", "dib", "tif", "tiff", "tga", // 其他常见格式
    "pbm", "pgm", "ppm", "pnm", "pam", // PNM 系列
    "qoi", "ico", "cur",
    "psd", "psb", "dds", "pcx", // 内置解码器（PSD/PSB、DDS、PCX）
    "exr", "jxl", "svg", "svgz", // exr、jxl-oxide、resvg
    "jp2", "j2k", "j2c", "jpc", "jpx", "jpf", // JPEG 2000（FM8）：hayro-jpeg2000
    "clip", // Clip Studio Paint 的画布预览图
    "jxr", "wdp", "hdp", // JPEG XR：jxrlib；Windows 用系统的 WIC
    "avif", "avifs", // AVIF：系统解码器优先，没有时 mp4parse + rav1d
    "hdr", // Radiance HDR：image crate 带的解码器
];

/// 文件名的扩展名是否属于当前能解码的格式（含系统解码器负责的，如 macOS 的 HEIC、AVIF）。
pub fn is_supported_name(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            let ext = ext.to_ascii_lowercase();
            RUST_EXTENSIONS.contains(&ext.as_str())
                || crate::raw::is_raw_extension(&ext)
                || crate::decode::platform_extensions().contains(&ext)
        }
        _ => false,
    }
}

/// 当前能打开的全部扩展名，供界面的打开面板筛选。
pub fn supported_extensions() -> Vec<&'static str> {
    let mut all = RUST_EXTENSIONS.to_vec();
    all.extend_from_slice(crate::raw::EXTENSIONS);
    for ext in crate::decode::platform_extensions() {
        if !all.contains(&ext.as_str()) {
            all.push(ext.as_str());
        }
    }
    all
}

/// Linux 的图片 MIME 类型（名字按 shared-mime-info；`.desktop` 的 MimeType 和"设为默认看图软件"用）。
/// 与 [`RUST_EXTENSIONS`]、RAW、libheif 负责的扩展名对应。PSB 系统算作 PSD 的类型；PAM、CLIP 在 shared-mime-info 里没有，
/// 由 Anyview 自带的类型定义（`apps/linux/data/mime/local.anyview.Anyview.xml`，安装包放进系统）补上。
pub const IMAGE_MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/apng",
    "image/gif",
    "image/webp",
    "image/bmp",
    "image/tiff",
    "image/x-tga",
    "image/x-portable-bitmap",
    "image/x-portable-graymap",
    "image/x-portable-pixmap",
    "image/x-portable-anymap",
    "image/x-portable-arbitrarymap",
    "image/qoi",
    "image/vnd.microsoft.icon",
    "image/x-win-bitmap",
    "image/vnd.adobe.photoshop",
    "image/vnd.ms-dds",
    "image/vnd.zbrush.pcx",
    "image/x-exr",
    "image/vnd.radiance",
    "image/jxl",
    "image/svg+xml",
    "image/svg+xml-compressed",
    "image/jp2",
    "image/x-jp2-codestream",
    "image/jpx",
    "image/jxr",
    "image/x-clip-studio-paint",
    "image/avif",
    "image/heif",
    "image/heic",
    "image/heic-sequence",
    "image/heif-sequence",
    "image/x-adobe-dng",
    "image/x-canon-cr2",
    "image/x-canon-cr3",
    "image/x-canon-crw",
    "image/x-nikon-nef",
    "image/x-nikon-nrw",
    "image/x-olympus-orf",
    "image/x-panasonic-rw2",
    "image/x-pentax-pef",
    "image/x-sony-sr2",
    "image/x-sony-arw",
    "image/x-sony-srf",
    "image/x-fuji-raf",
    "image/x-hasselblad-3fr",
    "image/x-hasselblad-fff",
    "image/x-phaseone-iiq",
    "image/x-samsung-srw",
    "image/x-sigma-x3f",
    "image/x-epson-erf",
    "image/x-minolta-mrw",
    "image/x-leaf-mos",
    "image/x-kodak-kdc",
    "image/x-kodak-dcr",
];

/// 压缩包的 MIME 类型：写进 `.desktop` 的 MimeType，让"打开方式"里有 Anyview（与 macOS 登记成"备选"相同），
/// "设为默认看图软件"不改它们（压缩包照常用压缩工具打开）。
pub const ARCHIVE_MIME_TYPES: &[&str] = &[
    "application/vnd.comicbook+zip",
    "application/vnd.comicbook-rar",
    "application/x-cb7",
    "application/zip",
    "application/x-7z-compressed",
    "application/vnd.rar",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// macOS 的 Info.plist 按扩展名登记了全部图片格式（"打开方式"、"设为默认看图软件"靠它）：
    /// 核心认的扩展名、RAW、macOS 交给 ImageIO 的（ImageIODecoder.extensions）都要在里面。
    #[test]
    fn macos_info_plist_lists_every_extension() {
        let plist = include_str!("../../../apps/macos/Resources/Info.plist");
        let start = plist.find("<string>Image File</string>").expect("Info.plist 里没有按扩展名的文档类型");
        let block = &plist[start..start + plist[start..].find("</array>").unwrap()];
        let mac_platform = ["heic", "heif", "hif", "heics", "avif", "avifs"];
        let missing: Vec<&str> = RUST_EXTENSIONS
            .iter()
            .chain(crate::raw::EXTENSIONS)
            .chain(mac_platform.iter())
            .copied()
            .filter(|ext| !block.contains(&format!("<string>{ext}</string>")))
            .collect();
        assert!(missing.is_empty(), "Info.plist 缺这些扩展名：{missing:?}");
    }

    #[test]
    fn matches_extensions_case_insensitively() {
        assert!(is_supported_name("a.JPG"));
        assert!(is_supported_name("照片.jpeg"));
        assert!(is_supported_name("x.tar.png"));
        assert!(!is_supported_name("notes.txt"));
        assert!(!is_supported_name("png"));
        assert!(!is_supported_name(".png"));
    }
}
