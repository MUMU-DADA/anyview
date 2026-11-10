//! modern/：较新的格式：OpenEXR、JPEG XL（含动图）、SVG、SVGZ、AVIF（含动图）、HEIC；
//! 扩展格式样例：JPEG 2000、Clip Studio Paint、JPEG XR、AVIF、Radiance HDR。
//!
//! - EXR、Radiance HDR、SVG、SVGZ 在这里按公式生成；画面都是 128x96 的四色块（与 view/、formats/ 相同的颜色）。
//! - JPEG XL、AVIF、HEIC 无纯 Rust 编码器：使用 cjxl、avifenc、heif-enc 从项目自生成的源图编码后随仓库保存
//!   四色块 PNG（和 anim/a_frames.gif）编码好，放在 `data/` 里随代码一起保存（几百字节到几 KB），这里原样写出。
//!   都是无损编码；HEIC 用 RGB 矩阵（-L），颜色应完全一致或只差 1–2。JPEG 2000 用 OpenJPEG 的 opj_compress
//!   （默认无损）从同一张四色块 PNG（和左上那块半透明的 RGBA 版）编码；JPEG XR 用 jxrlib 自带的 JxrEncApp（从 4creators/jxrlib
//!   的源代码编译，-q 1 无损）从四色块 BMP 和一张小的 Radiance HDR 编码。Clip Studio Paint 的文件在这里按它的结构拼出来：
//!   Head、SQLi（内嵌的 SQLite 数据库，CanvasPreview 表里放四色块 PNG）、Foot 三块。
//! - `data/exif.heic` 不写进 modern/，只给 Linux 的 libheif 单元测试读拍摄信息：用容器里的 Pillow 画一张 128x96
//!   的图、写上 EXIF（Canon EOS R5，RF50mm F1.8 STM，50mm f/2.8 1/250s ISO 400，拍摄时间 2025:05:17 14:03:21；
//!   分数一律用 Pillow 的 IFDRational，否则会写成两个整数），存成 JPEG，再用 heif-enc -q 80 转成 HEIC（它把 JPEG 的
//!   EXIF 原样放进 Exif 块）。
//!
//! | 文件 | 内容 |
//! | --- | --- |
//! | exr_rgba32f.exr | 线性浮点 RGBA，四色块换算成线性值，显示时应回到原来的颜色（±1） |
//! | exr_bright.exr | 同上，但左上那块的亮度是 4 倍（红通道超过普通亮度，显示时截到 255，其余两个通道约 82） |
//! | hdr_quadrants.hdr | Radiance HDR（RGBE），四色块换算成线性值，显示时应回到原来的颜色（RGBE 有舍入，±2） |
//! | jxl_still.jxl | 无损，128x96 四色块 |
//! | jxl_anim.jxl | 无损动图，240x160、3 帧，每帧 100 ms（由 anim/a_frames.gif 编码；GIF 里时长为 0 的那帧被编码器并进了下一帧） |
//! | svg_quadrants.svg | 128x96 四色块 |
//! | svgz_quadrants.svgz | 同上，gzip 压缩 |
//! | svg_text.svg | 白底上的一行黑字（有文字时才加载系统字体） |
//! | avif_still.avif | 无损，128x96 四色块（macOS 用 ImageIO，Linux 用系统的 libheif，Windows 用 WIC；系统解不了时用核心的解码器） |
//! | avif_anim.avif | 无损动图，4 帧、每帧 0.25 秒（四色块轮转） |
//! | avif_pq.avif | 高动态范围（BT.2020、PQ，10 位无损）：左上 203 尼特的灰（普通的白）、右上 1000 尼特的灰、左下 BT.2020 的纯红（100 尼特）、右下 10 尼特的灰；由 16 位 PNG 编码，像素值就是 PQ 信号 |
//! | avif_p3.avif | 广色域（Display P3、sRGB 曲线，8 位无损）：左上橙 (255,128,0)、右上灰 128、左下青 (0,200,200)、右下 P3 的纯绿 |
//! | heic_still.heic | 无损（RGB），128x96 四色块 |
//! | jp2_still.jp2 | JPEG 2000 容器，无损，128x96 四色块 |
//! | jp2_alpha.jp2 | 同上，带透明通道，左上那块透明度 128 |
//! | j2k_still.j2k | JPEG 2000 裸码流，无损，128x96 四色块 |
//! | jxr_still.jxr | JPEG XR，无损，128x96 四色块（Windows 先用 WIC，解不了时用核心的 jxrlib） |
//! | jxr_hdr.jxr | JPEG XR 的 RGBE（高动态范围），32x16：左半边线性 0.5（sRGB 约 188），右半边 (2, 0.25, 0.25)（红截到 255，其余约 137） |
//! | clip_canvas.clip | Clip Studio Paint，画布预览图是 128x96 四色块 |

use std::fs;
use std::io::Write;
use std::path::Path;

use image::{ImageBuffer, Rgba};

const JXL_STILL: &[u8] = include_bytes!("../data/still.jxl");
const JXL_ANIM: &[u8] = include_bytes!("../data/anim.jxl");
const AVIF_STILL: &[u8] = include_bytes!("../data/still.avif");
const AVIF_ANIM: &[u8] = include_bytes!("../data/anim.avif");
const AVIF_PQ: &[u8] = include_bytes!("../data/avif_pq.avif");
const AVIF_P3: &[u8] = include_bytes!("../data/avif_p3.avif");
const HEIC_STILL: &[u8] = include_bytes!("../data/still.heic");
const JP2_STILL: &[u8] = include_bytes!("../data/still.jp2");
const JP2_ALPHA: &[u8] = include_bytes!("../data/alpha.jp2");
const J2K_STILL: &[u8] = include_bytes!("../data/still.j2k");
const JXR_STILL: &[u8] = include_bytes!("../data/still.jxr");
const JXR_HDR: &[u8] = include_bytes!("../data/hdr.jxr");

pub const WIDTH: u32 = 128;
pub const HEIGHT: u32 = 96;

/// 四色块的 SVG。
pub const SVG_QUADRANTS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="96" viewBox="0 0 128 96">
<rect x="0" y="0" width="64" height="48" fill="#DC2828"/>
<rect x="64" y="0" width="64" height="48" fill="#28AA46"/>
<rect x="0" y="48" width="64" height="48" fill="#285ADC"/>
<rect x="64" y="48" width="64" height="48" fill="#F0C828"/>
</svg>
"##;

// 文字放在画面核对的两行采样点（高的 3/8、5/8 处，即 45、75）之间：Retina 上按屏幕像素重新渲染的文字边缘
// 与原图差得多，采样点不能落在字上。
const SVG_TEXT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="120">
<rect width="240" height="120" fill="#FFFFFF"/>
<text x="12" y="66" font-family="sans-serif" font-size="20" fill="#000000">Anyview SVG</text>
</svg>
"##;

/// sRGB 8 位 → 线性值。
fn linear(c: u8) -> f32 {
    let v = f32::from(c) / 255.0;
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn exr(bright: bool) -> crate::Result<Vec<u8>> {
    let quadrants = crate::quadrants(WIDTH, HEIGHT);
    let image: ImageBuffer<Rgba<f32>, Vec<f32>> = ImageBuffer::from_fn(WIDTH, HEIGHT, |x, y| {
        let [r, g, b] = quadrants.get_pixel(x, y).0;
        let k = if bright && x < WIDTH / 2 && y < HEIGHT / 2 { 4.0 } else { 1.0 };
        Rgba([linear(r) * k, linear(g) * k, linear(b) * k, 1.0])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba32F(image).write_to(&mut bytes, image::ImageFormat::OpenExr)?;
    Ok(bytes.into_inner())
}

/// Radiance HDR：四色块换算成线性值，RGBE 编码。
fn radiance() -> crate::Result<Vec<u8>> {
    let quadrants = crate::quadrants(WIDTH, HEIGHT);
    let image: ImageBuffer<image::Rgb<f32>, Vec<f32>> = ImageBuffer::from_fn(WIDTH, HEIGHT, |x, y| {
        let [r, g, b] = quadrants.get_pixel(x, y).0;
        image::Rgb([linear(r), linear(g), linear(b)])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb32F(image).write_to(&mut bytes, image::ImageFormat::Hdr)?;
    Ok(bytes.into_inner())
}

/// Clip Studio Paint 的文件：按它的结构拼出 Head、SQLi、Foot 三块，SQLi 是内嵌的 SQLite 数据库，
/// CanvasPreview 表里放四色块 PNG（与软件存的表结构相同）。
fn clip() -> crate::Result<Vec<u8>> {
    let mut png = std::io::Cursor::new(Vec::new());
    crate::quadrants(WIDTH, HEIGHT).write_to(&mut png, image::ImageFormat::Png)?;
    let database = {
        let mut connection = rusqlite::Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE Canvas (_PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, CanvasWidth REAL, \
             CanvasHeight REAL); \
             INSERT INTO Canvas (MainId, CanvasWidth, CanvasHeight) VALUES (1, 128, 96); \
             CREATE TABLE CanvasPreview (_PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, \
             CanvasId INTEGER, ImageType INTEGER, ImageWidth INTEGER, ImageHeight INTEGER, ImageData BLOB);",
        )?;
        connection.execute(
            "INSERT INTO CanvasPreview (MainId, CanvasId, ImageType, ImageWidth, ImageHeight, ImageData) \
             VALUES (1, 1, 1, 128, 96, ?1)",
            [png.into_inner()],
        )?;
        connection.serialize(rusqlite::MAIN_DB)?.to_vec()
    };
    let mut chunks = Vec::new();
    for (kind, body) in [(b"Head", vec![0u8; 40]), (b"SQLi", database), (b"Foot", Vec::new())] {
        chunks.extend_from_slice(b"CHNK");
        chunks.extend_from_slice(kind);
        chunks.extend_from_slice(&(body.len() as u64).to_be_bytes());
        chunks.extend_from_slice(&body);
    }
    let mut file = b"CSFCHUNK".to_vec();
    file.extend_from_slice(&((24 + chunks.len()) as u64).to_be_bytes());
    file.extend_from_slice(&24u64.to_be_bytes());
    file.extend_from_slice(&chunks);
    Ok(file)
}

fn svgz() -> crate::Result<Vec<u8>> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(SVG_QUADRANTS.as_bytes())?;
    Ok(gz.finish()?)
}

/// 生成 modern/ 下的文件，返回文件数。
pub fn generate(dir: &Path) -> crate::Result<usize> {
    crate::ensure_dir(dir)?;
    let files: [(&str, Vec<u8>); 19] = [
        ("exr_rgba32f.exr", exr(false)?),
        ("exr_bright.exr", exr(true)?),
        ("hdr_quadrants.hdr", radiance()?),
        ("jxl_still.jxl", JXL_STILL.to_vec()),
        ("jxl_anim.jxl", JXL_ANIM.to_vec()),
        ("svg_quadrants.svg", SVG_QUADRANTS.as_bytes().to_vec()),
        ("svgz_quadrants.svgz", svgz()?),
        ("svg_text.svg", SVG_TEXT.as_bytes().to_vec()),
        ("avif_still.avif", AVIF_STILL.to_vec()),
        ("avif_anim.avif", AVIF_ANIM.to_vec()),
        ("avif_pq.avif", AVIF_PQ.to_vec()),
        ("avif_p3.avif", AVIF_P3.to_vec()),
        ("heic_still.heic", HEIC_STILL.to_vec()),
        ("jp2_still.jp2", JP2_STILL.to_vec()),
        ("jp2_alpha.jp2", JP2_ALPHA.to_vec()),
        ("j2k_still.j2k", J2K_STILL.to_vec()),
        ("jxr_still.jxr", JXR_STILL.to_vec()),
        ("jxr_hdr.jxr", JXR_HDR.to_vec()),
        ("clip_canvas.clip", clip()?),
    ];
    for (name, bytes) in &files {
        fs::write(dir.join(name), bytes)?;
    }
    Ok(files.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyview_core::decode::decode_file;

    /// 四块中心的颜色（BGRA 预乘，不透明时就是 BGRA）。
    fn bgra(image: &anyview_core::DecodedImage, x: u32, y: u32) -> [u8; 4] {
        let at = y as usize * image.stride + x as usize * 4;
        image.pixels[at..at + 4].try_into().unwrap()
    }

    const CENTERS: [(u32, u32); 4] = [(32, 24), (96, 24), (32, 72), (96, 72)];
    const COLORS: [[u8; 3]; 4] = [[220, 40, 40], [40, 170, 70], [40, 90, 220], [240, 200, 40]];

    fn check(path: &Path, tolerance: u8, skip_first: bool) {
        let image = decode_file(path).unwrap_or_else(|e| panic!("{}：{e}", path.display()));
        assert_eq!((image.width, image.height), (WIDTH, HEIGHT), "{}", path.display());
        for (q, &(x, y)) in CENTERS.iter().enumerate() {
            if skip_first && q == 0 {
                continue;
            }
            let [b, g, r, a] = bgra(&image, x, y);
            let want = COLORS[q];
            let close = [r, g, b].iter().zip(&want).all(|(got, w)| got.abs_diff(*w) <= tolerance);
            assert!(close && a == 255, "{} 第 {q} 块：{:?}，应为 {want:?}", path.display(), [r, g, b, a]);
        }
    }

    #[test]
    fn exr_jxl_and_svg_decode_to_the_quadrants() {
        let dir = std::env::temp_dir().join(format!("anyview-fixtures-modern-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(generate(&dir).unwrap(), fs::read_dir(&dir).unwrap().count());
        check(&dir.join("exr_rgba32f.exr"), 1, false);
        check(&dir.join("exr_bright.exr"), 1, true);
        let bright = decode_file(&dir.join("exr_bright.exr")).unwrap();
        // 左上红块亮度 4 倍：红通道超过普通亮度，截到 255；绿、蓝的线性值乘 4 后约为 82。
        assert_eq!(bgra(&bright, 32, 24), [82, 82, 255, 255], "超过普通亮度的按通道截到最亮");
        assert_eq!(bright.info.format, "EXR");
        check(&dir.join("hdr_quadrants.hdr"), 2, false);
        assert_eq!(decode_file(&dir.join("hdr_quadrants.hdr")).unwrap().info.format, "HDR");
        check(&dir.join("jxl_still.jxl"), 0, false);
        assert_eq!(decode_file(&dir.join("jxl_still.jxl")).unwrap().info.format, "JXL");
        let anim = decode_file(&dir.join("jxl_anim.jxl")).unwrap();
        assert_eq!((anim.width, anim.height), (240, 160));
        // JPEG XL 动图：一帧一帧读出 3 帧，每帧 100 ms。
        let mut reader = anyview_core::anim::FrameReader::open(&dir.join("jxl_anim.jxl")).expect("应是动图");
        let mut frames = Vec::new();
        while let Some(frame) = reader.next_frame().unwrap() {
            frames.push((frame.width, frame.height, frame.delay_ms, frame.rgba[..4].to_vec()));
        }
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|f| (f.0, f.1, f.2) == (240, 160, 100)));
        assert_ne!(frames[0].3, frames[1].3, "各帧不同");
        assert!(anyview_core::anim::FrameReader::open(&dir.join("jxl_still.jxl")).is_none());
        check(&dir.join("svg_quadrants.svg"), 0, false);
        check(&dir.join("svgz_quadrants.svgz"), 0, false);
        let svg = decode_file(&dir.join("svg_quadrants.svg")).unwrap();
        assert!(svg.vector.is_some(), "SVG 带着文件内容，精确重绘时按屏幕像素重新渲染");
        let text = decode_file(&dir.join("svg_text.svg")).unwrap();
        assert_eq!((text.width, text.height), (240, 120));
        // 文字真的画出来了（以前文字整个不显示，只核对了尺寸）。BGRA 预乘，黑字在白底上。
        let dark = text.pixels.as_chunks::<4>().0.iter().filter(|p| p[3] > 128 && p[2] < 100).count();
        assert!(dark > 100, "svg_text.svg 的文字没画出来（{dark} 个深色像素）");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Clip Studio Paint：显示画布预览图。
    #[test]
    fn clip_studio_paint_shows_the_canvas_preview() {
        let dir = std::env::temp_dir().join(format!("anyview-fixtures-clip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        generate(&dir).unwrap();
        check(&dir.join("clip_canvas.clip"), 0, false);
        assert_eq!(decode_file(&dir.join("clip_canvas.clip")).unwrap().info.format, "CLIP");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// JPEG XR：无损的还原四色块；高动态范围的按 scRGB → sRGB 换算、超出的截断。
    #[test]
    fn jpeg_xr_decodes_including_hdr() {
        let dir = std::env::temp_dir().join(format!("anyview-fixtures-jxr-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        generate(&dir).unwrap();
        check(&dir.join("jxr_still.jxr"), 0, false);
        let still = decode_file(&dir.join("jxr_still.jxr")).unwrap();
        assert_eq!(still.info.format, "JXR");
        let hdr = decode_file(&dir.join("jxr_hdr.jxr")).unwrap();
        assert_eq!((hdr.width, hdr.height), (32, 16));
        let [b, g, r, a] = bgra(&hdr, 4, 8);
        assert!([r, g, b].iter().all(|c| c.abs_diff(188) <= 1) && a == 255, "{:?}", [r, g, b, a]);
        let [b, g, r, _] = bgra(&hdr, 28, 8);
        assert!(r == 255 && g.abs_diff(137) <= 1 && b.abs_diff(137) <= 1, "{:?}", [r, g, b]);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// AVIF 的核心解码器（系统没有解码器时用；测试里没有登记系统解码器，走的就是它）：无损的完全还原；
    /// 有损的 4:2:0（8、10 位）、带透明通道、旋转 90 度、灰度的都与 libavif 的结果差不过 2（这几张只在测试里用，不放进 modern/）。
    #[test]
    fn avif_core_decoder_matches_libavif() {
        let dir = std::env::temp_dir().join(format!("anyview-fixtures-avif-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        generate(&dir).unwrap();
        check(&dir.join("avif_still.avif"), 0, false);
        check(&dir.join("avif_anim.avif"), 0, false);
        // 动态 AVIF：核心自己逐帧放（系统解码器放不了时），4 帧、每帧 250 ms，四色块轮转。
        let mut reader = anyview_core::anim::FrameReader::open(&dir.join("avif_anim.avif")).expect("应是动图");
        let mut frames = Vec::new();
        while let Some(frame) = reader.next_frame().unwrap() {
            frames.push((frame.width, frame.height, frame.delay_ms, frame.rgba[..4].to_vec()));
        }
        assert_eq!(frames.len(), 4);
        assert!(frames.iter().all(|f| (f.0, f.1, f.2) == (128, 96, 250)), "{:?}", frames.iter().map(|f| (f.0, f.1, f.2)).collect::<Vec<_>>());
        assert_ne!(frames[0].3, frames[1].3, "各帧不同");
        assert!(anyview_core::anim::FrameReader::open(&dir.join("avif_still.avif")).is_none());
        assert_eq!(decode_file(&dir.join("avif_still.avif")).unwrap().info.format, "AVIF");
        let extra: [(&str, &[u8]); 5] = [
            ("lossy420.avif", include_bytes!("../data/avif_lossy420.avif")),
            ("lossy420_10bit.avif", include_bytes!("../data/avif_lossy420_10bit.avif")),
            ("alpha444.avif", include_bytes!("../data/avif_alpha444.avif")),
            ("rot90.avif", include_bytes!("../data/avif_rot90.avif")),
            ("gray.avif", include_bytes!("../data/avif_gray.avif")),
        ];
        for (name, bytes) in extra {
            fs::write(dir.join(name), bytes).unwrap();
        }
        // 高动态范围、广色域：按 nclx 换成 sRGB。PQ 的普通的白（203 尼特）接近白（原来按 sRGB 显示是中灰 148），
        // 这张图最亮的是 1000 尼特，正好到白；BT.2020 的纯红换到 sRGB 还是红；10 尼特约 63。
        let near = |image: &anyview_core::DecodedImage, x: u32, y: u32, want: [u8; 3], tolerance: u8| {
            let [b, g, r, a] = bgra(image, x, y);
            let ok = [r, g, b].iter().zip(want).all(|(got, w)| got.abs_diff(w) <= tolerance) && a == 255;
            assert!(ok, "({x}, {y})：{:?}，应为 {want:?}", [r, g, b]);
        };
        let pq = decode_file(&dir.join("avif_pq.avif")).unwrap();
        assert_eq!(pq.info.pixel_format, "YUV444P10 PQ");
        near(&pq, 32, 24, [240, 240, 240], 3);
        near(&pq, 96, 24, [255, 255, 255], 0);
        near(&pq, 32, 72, [233, 0, 0], 3);
        near(&pq, 96, 72, [63, 63, 63], 2);
        // P3 的颜色换到 sRGB：橙色的绿通道从 128 降到约 119，青色两个通道都变亮；超出 sRGB 的纯绿截到边上。
        let p3 = decode_file(&dir.join("avif_p3.avif")).unwrap();
        assert_eq!(p3.info.pixel_format, "YUV444P8");
        near(&p3, 32, 24, [255, 119, 0], 2);
        near(&p3, 96, 24, [128, 128, 128], 1);
        near(&p3, 32, 72, [0, 204, 202], 2);
        near(&p3, 96, 72, [0, 255, 0], 1);
        assert!(anyview_core::avif::needs_own_color(AVIF_PQ) && anyview_core::avif::needs_own_color(AVIF_P3));
        assert!(!anyview_core::avif::needs_own_color(AVIF_STILL), "普通的 AVIF 照旧先交给系统的解码器");
        check(&dir.join("lossy420.avif"), 3, false);
        check(&dir.join("lossy420_10bit.avif"), 3, false);
        assert_eq!(decode_file(&dir.join("lossy420_10bit.avif")).unwrap().info.pixel_format, "YUV420P10");
        // 透明：左上那块透明度 128（预乘后约 110, 20, 20），其余不透明。
        check(&dir.join("alpha444.avif"), 3, true);
        let alpha = decode_file(&dir.join("alpha444.avif")).unwrap();
        assert!(alpha.info.has_alpha);
        let [b, g, r, a] = bgra(&alpha, 32, 24);
        assert!(a.abs_diff(128) <= 1 && r.abs_diff(110) <= 3 && g.abs_diff(20) <= 3 && b.abs_diff(20) <= 3, "{:?}", [r, g, b, a]);
        // irot 1（逆时针 90 度）：96x128，原来的右上（绿）到了左上。
        let rotated = decode_file(&dir.join("rot90.avif")).unwrap();
        assert_eq!((rotated.width, rotated.height), (96, 128));
        let [b, g, r, _] = bgra(&rotated, 24, 32);
        assert!(r.abs_diff(40) <= 3 && g.abs_diff(170) <= 3 && b.abs_diff(70) <= 3, "{:?}", [r, g, b]);
        // 灰度：三个通道相同。
        let gray = decode_file(&dir.join("gray.avif")).unwrap();
        let [b, g, r, _] = bgra(&gray, 32, 24);
        assert!(r == g && g == b, "{:?}", [r, g, b]);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// JPEG 2000：容器、裸码流都无损还原；透明通道按预乘保存。
    #[test]
    fn jpeg_2000_decodes_losslessly() {
        let dir = std::env::temp_dir().join(format!("anyview-fixtures-jp2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        generate(&dir).unwrap();
        check(&dir.join("jp2_still.jp2"), 0, false);
        check(&dir.join("j2k_still.j2k"), 0, false);
        let jp2 = decode_file(&dir.join("jp2_still.jp2")).unwrap();
        assert_eq!((jp2.info.format.as_str(), jp2.info.pixel_format.as_str()), ("JP2", "RGB24"));
        assert!(!jp2.info.has_alpha);
        assert_eq!(decode_file(&dir.join("j2k_still.j2k")).unwrap().info.format, "J2K");
        // 透明的：其余三块不透明、颜色不变；左上那块透明度 128，颜色按预乘（220 × 128 / 255 ≈ 110）。
        check(&dir.join("jp2_alpha.jp2"), 0, true);
        let alpha = decode_file(&dir.join("jp2_alpha.jp2")).unwrap();
        assert!(alpha.info.has_alpha);
        assert_eq!(alpha.info.pixel_format, "RGBA32");
        let [b, g, r, a] = bgra(&alpha, 32, 24);
        assert_eq!(a, 128);
        assert!(r.abs_diff(110) <= 1 && g.abs_diff(20) <= 1 && b.abs_diff(20) <= 1, "{:?}", [r, g, b, a]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
