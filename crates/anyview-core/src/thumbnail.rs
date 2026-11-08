//! 文件管理器的缩略图：系统自己不认识的格式（QOI、CLIP、JPEG XR 等）由本软件生成。
//! macOS 的访达缩略图扩展、Linux 的缩略图生成器（`anyview --thumbnail`）、以后 Windows 的缩略图提供程序都用这里。
//!
//! 不读设置、不用缓存，每次单独解码一个文件：按默认选项（按 EXIF 方向摆正、内嵌 ICC 按平台处理、RAW 先用相机的预览图），
//! 再缩到长边不超过要的尺寸（不放大）。动图取第一帧。解码器出错只当这个文件没有缩略图。
//! 系统在后台给整个文件夹做缩略图，所以解码的内存上限比看图时小（[`MAX_ALLOC`]），太大的图不做。

use std::path::Path;

use crate::decode::{DecodeOptions, DecodedImage};

/// 最大的边长（像素）：访达的缩略图最大要 1024（512 点的 2 倍）；按空格的预览要到 4096，够满屏看。
pub const MAX_SIDE: u32 = 4096;

/// 生成缩略图时解码一张图最多用多少内存（字节，按 RGBA 算约 1.3 亿像素）：访达、文件管理器在后台给很多文件
/// 做缩略图（访达的扩展还可能有内存限制），超大的图（看图时的上限是 2 GB）不做，系统显示普通的文件图标。
pub const MAX_ALLOC: u64 = 512 << 20;

/// 一个文件的缩略图：长边不超过 `max_side` 像素（BGRA8 预乘，按行连续存放）。
pub fn file_thumbnail(path: &Path, max_side: u32) -> Result<DecodedImage, String> {
    let max_side = max_side.clamp(1, MAX_SIDE);
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if crate::svg::is_svg_extension(&extension) {
        return svg_thumbnail(path, max_side);
    }
    let options = DecodeOptions::default();
    // 不附代理纹理（超大图看图时用的缩小版），缩略图用不到。
    let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::decode::with_alloc_limit(MAX_ALLOC, || crate::decode::decode_original(path, options))
    }))
    .map_err(|_| msg!("解码器出错", "The decoder failed").to_string())?
    .map_err(|e| e.to_string())?;
    Ok(crate::picker::downscale(&image, max_side))
}

/// SVG、SVGZ：长边按要的尺寸渲染（小图标也放大，矢量图不糊），见 [`crate::svg::render_fit`]。
fn svg_thumbnail(path: &Path, max_side: u32) -> Result<DecodedImage, String> {
    let start = std::time::Instant::now();
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let raster = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::decode::with_alloc_limit(MAX_ALLOC, || crate::svg::render_fit(&data, max_side))
    }))
    .map_err(|_| msg!("解码器出错", "The decoder failed").to_string())?
    .map_err(|e| e.to_string())?;
    Ok(crate::decode::from_raster(raster, start, &metadata, DecodeOptions::default()))
}

/// 缩略图编成 PNG（RGBA，不预乘；Linux 的缩略图生成器写文件用）。
pub fn encode_png(image: &DecodedImage) -> Result<Vec<u8>, String> {
    let mut rgba = Vec::with_capacity(image.width as usize * image.height as usize * 4);
    for y in 0..image.height as usize {
        let row = &image.pixels[y * image.stride..y * image.stride + image.width as usize * 4];
        for px in row.as_chunks::<4>().0 {
            let [b, g, r, a] = *px;
            let straight = |c: u8| {
                if a == 0 {
                    0
                } else {
                    ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
                }
            };
            rgba.extend_from_slice(&[straight(r), straight(g), straight(b), a]);
        }
    }
    let mut png = Vec::new();
    {
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&rgba, image.width, image.height, image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string())?;
    }
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../anyview-fixtures/data").join(name)
    }

    #[test]
    fn makes_small_thumbnails_without_growing() {
        // 128x96 的 JPEG 2000 四色块：要 64 时缩成 64x48；要 1000 时不放大。
        let small = file_thumbnail(&fixture("still.jp2"), 64).unwrap();
        assert_eq!((small.width, small.height), (64, 48));
        let same = file_thumbnail(&fixture("still.jp2"), 1000).unwrap();
        assert_eq!((same.width, same.height), (128, 96));
        // 按空格的预览要 4096：3000 像素宽的图照原样给（以前上限是 2048，预览只拿到 2048）。
        let dir = std::env::temp_dir().join(format!("anyview-thumb-wide-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wide = dir.join("wide.qoi");
        image::RgbImage::from_pixel(3000, 20, image::Rgb([40, 90, 220]))
            .save_with_format(&wide, image::ImageFormat::Qoi)
            .unwrap();
        let preview = file_thumbnail(&wide, 4096).unwrap();
        assert_eq!((preview.width, preview.height), (3000, 20));
        let thumb = file_thumbnail(&wide, 1024).unwrap();
        assert_eq!(thumb.width, 1024);
        std::fs::remove_dir_all(&dir).ok();
        // 左上那块是 #DC2828（BGRA 预乘，不透明）。
        let px = &small.pixels[(10 * small.stride + 10 * 4)..(10 * small.stride + 10 * 4 + 4)];
        assert!(px[2].abs_diff(0xDC) <= 3 && px[1].abs_diff(0x28) <= 3 && px[3] == 255, "{px:?}");
    }

    #[test]
    fn decoders_respect_the_memory_limit_and_it_is_restored() {
        // 128x96 的图按 RGBA 要 48 KB：上限 40 KB 时各个解码器都不解（JPEG 2000、JPEG XR、AVIF、JPEG XL），
        // 出了这一段恢复原来的上限，照常能解。
        for name in ["still.jp2", "still.j2k", "still.jxr", "still.avif", "still.jxl"] {
            let path = fixture(name);
            let refused = crate::decode::with_alloc_limit(40 << 10, || {
                crate::decode::decode_original(&path, DecodeOptions::default())
            });
            assert!(refused.is_err(), "{name} 超过上限也解了");
            assert_eq!(crate::decode::alloc_limit(), crate::decode::MAX_ALLOC);
            assert!(file_thumbnail(&path, 64).is_ok(), "{name}");
        }
        // panic 时也恢复。
        let caught = std::panic::catch_unwind(|| crate::decode::with_alloc_limit(1, || panic!("测试")));
        assert!(caught.is_err());
        assert_eq!(crate::decode::alloc_limit(), crate::decode::MAX_ALLOC);
    }

    #[test]
    fn svg_thumbnails_are_rendered_at_the_requested_size() {
        // 16x12 的四色块 SVG：要 256 时按 256x192 渲染（矢量图放大也清晰），要 8 时缩成 8x6；SVGZ（gzip）一样。
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="12">
<rect width="8" height="6" fill="#DC2828"/><rect x="8" width="8" height="6" fill="#28AA46"/>
<rect y="6" width="8" height="6" fill="#285ADC"/><rect x="8" y="6" width="8" height="6" fill="#F0C828"/></svg>"##;
        let dir = std::env::temp_dir().join(format!("anyview-thumb-svg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("q.svg");
        std::fs::write(&plain, svg).unwrap();
        let packed = dir.join("q.svgz");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, svg.as_bytes()).unwrap();
        std::fs::write(&packed, gz.finish().unwrap()).unwrap();
        for path in [&plain, &packed] {
            let big = file_thumbnail(path, 256).unwrap();
            assert_eq!((big.width, big.height), (256, 192), "{path:?}");
            // 左上 #DC2828、右下 #F0C828（BGRA 预乘，不透明）。
            let at = |x: usize, y: usize| &big.pixels[y * big.stride + x * 4..y * big.stride + x * 4 + 4];
            let tl = at(64, 48);
            assert!(tl[2].abs_diff(0xDC) <= 2 && tl[1].abs_diff(0x28) <= 2 && tl[3] == 255, "{tl:?}");
            let br = at(192, 144);
            assert!(br[2].abs_diff(0xF0) <= 2 && br[1].abs_diff(0xC8) <= 2, "{br:?}");
            let small = file_thumbnail(path, 8).unwrap();
            assert_eq!((small.width, small.height), (8, 6));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn png_round_trip_keeps_size_and_colors() {
        let thumb = file_thumbnail(&fixture("still.jxr"), 96).unwrap();
        let png = encode_png(&thumb).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (thumb.width, thumb.height));
        let p = decoded.get_pixel(5, 5);
        assert!(p[0].abs_diff(0xDC) <= 3 && p[1].abs_diff(0x28) <= 3 && p[3] == 255, "{p:?}");
    }

    #[test]
    fn broken_files_have_no_thumbnail() {
        let dir = std::env::temp_dir().join(format!("anyview-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.qoi");
        std::fs::write(&bad, b"qoif\0\0\0\x10broken").unwrap();
        assert!(file_thumbnail(&bad, 128).is_err());
        assert!(file_thumbnail(&dir.join("missing.png"), 128).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
