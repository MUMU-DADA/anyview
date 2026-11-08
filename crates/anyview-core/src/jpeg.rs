//! JPEG 的快速路径。
//!
//! - 解码：用 zune-jpeg 直接输出 BGRA，省掉 RGB → RGBA → BGRA 两次转换。只处理 YCbCr 的 JPEG（照片几乎都是）；
//!   灰度、CMYK 等仍走 image crate 的通用路径。
//! - 快速预览：大图先显示 EXIF 里内嵌的缩略图（界面把它拉伸到原图尺寸），清晰图解码完成后替换。

use crate::i18n::Message;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use image::metadata::Orientation;
use image::{DynamicImage, RgbaImage};
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::color::{self, ChannelOrder};
use crate::decode::{DecodeOptions, DecodedImage, ImageInfo};
use crate::exif::{Exif, ShootingInfo};

/// 原图至少有这么多像素时才先显示快速预览：小图解码很快，不值得先闪一下模糊的缩略图。
pub const PREVIEW_MIN_PIXELS: u64 = 4_000_000;
/// 找缩略图时只读文件开头这么多字节：EXIF 段最长 64 KB，前后还可能有 JFIF、ICC 等段。
const HEAD_BYTES: u64 = 256 * 1024;

fn options() -> DecoderOptions {
    DecoderOptions::default()
        .set_strict_mode(false)
        .set_max_width(usize::MAX)
        .set_max_height(usize::MAX)
        .jpeg_set_out_colorspace(ColorSpace::BGRA)
}

/// 快速路径的解码结果：BGRA8（不透明，预乘与否相同），已按 EXIF 方向摆正，已按平台做色彩管理。
pub(crate) struct Decoded {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub icc_profile: Option<Arc<Vec<u8>>>,
    /// 拍摄信息。
    pub shooting: Option<ShootingInfo>,
}

/// 用快速路径解码。不是 YCbCr 的 JPEG 返回 `Ok(None)`，由调用方改用通用路径。
pub(crate) fn decode(
    data: &[u8],
    max_bytes: u64,
    opts: DecodeOptions,
) -> Result<Option<Decoded>, Message> {
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options());
    decoder.decode_headers().map_err(|e| e.to_string())?;
    if decoder.input_colorspace() != Some(ColorSpace::YCbCr) {
        return Ok(None);
    }
    let (width, height) = decoder.dimensions().ok_or(msg!("没有图像尺寸", "No image size"))?;
    let bytes = width as u64 * height as u64 * 4;
    if bytes > max_bytes {
        crate::decode::limit_hit();
        return Err(msg!("图片太大：{width}x{height}", "Image too large: {width}x{height}", width = format!("{width}"), height = format!("{height}")));
    }
    let icc = decoder.icc_profile().filter(|_| opts.use_icc);
    let orientation = decoder
        .exif()
        .filter(|_| opts.auto_orient)
        .and_then(|exif| Orientation::from_exif_chunk(exif))
        .unwrap_or(Orientation::NoTransforms);
    let shooting = decoder
        .exif()
        .and_then(|exif| Exif::parse(exif))
        .and_then(|exif| exif.shooting().non_empty());
    let mut pixels = vec![0u8; bytes as usize];
    decoder.decode_into(&mut pixels).map_err(|e| e.to_string())?;
    let icc_profile = color::manage(&mut pixels, icc, ChannelOrder::Bgra);
    let (pixels, width, height) = orient(pixels, width as u32, height as u32, orientation);
    Ok(Some(Decoded {
        width,
        height,
        pixels,
        icc_profile,
        shooting,
    }))
}

/// 快速预览：EXIF 内嵌的缩略图（已摆正、已做色彩管理），以及原图摆正后的尺寸。
pub struct Preview {
    pub image: DecodedImage,
    pub width: u32,
    pub height: u32,
}

/// 读取快速预览。不是 JPEG、原图不够大、没有内嵌缩略图或读不出来时返回 None。
pub fn preview(path: &Path) -> Option<Preview> {
    preview_with(path, DecodeOptions::default())
}

/// 按设置的解码选项（EXIF 方向、ICC）读取快速预览。
pub fn preview_with(path: &Path, opts: DecodeOptions) -> Option<Preview> {
    preview_at_least(path, PREVIEW_MIN_PIXELS, opts)
}

pub(crate) fn preview_at_least(
    path: &Path,
    min_pixels: u64,
    opts: DecodeOptions,
) -> Option<Preview> {
    let start = Instant::now();
    let metadata = fs::metadata(path).ok()?;
    let mut head = Vec::with_capacity(HEAD_BYTES as usize);
    fs::File::open(path)
        .ok()?
        .take(HEAD_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    if !head.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    // 只解析到图像数据之前的各段（尺寸、EXIF、ICC），不解码原图。
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(&head[..]), options());
    decoder.decode_headers().ok()?;
    let (width, height) = decoder.dimensions()?;
    if (width as u64) * (height as u64) < min_pixels {
        return None;
    }
    let exif = decoder.exif()?;
    let orientation = match opts.auto_orient {
        true => Orientation::from_exif_chunk(exif).unwrap_or(Orientation::NoTransforms),
        false => Orientation::NoTransforms,
    };
    let thumbnail = Exif::parse(exif)?.thumbnail()?;
    let mut small = JpegDecoder::new_with_options(ZCursor::new(thumbnail), options());
    small.decode_headers().ok()?;
    if small.input_colorspace() != Some(ColorSpace::YCbCr) {
        return None;
    }
    let (tw, th) = small.dimensions()?;
    if !(8..=4096).contains(&tw) || !(8..=4096).contains(&th) {
        return None;
    }
    let mut pixels = vec![0u8; tw * th * 4];
    small.decode_into(&mut pixels).ok()?;
    let (mut pixels, tw, th) =
        crop_letterbox(pixels, tw as u32, th as u32, width as u32, height as u32);
    // 缩略图与原图在同一色彩空间，用原图的 ICC。
    let icc = decoder.icc_profile().filter(|_| opts.use_icc);
    let icc_profile = color::manage(&mut pixels, icc, ChannelOrder::Bgra);
    let (pixels, tw, th) = orient(pixels, tw, th, orientation);
    let (full_w, full_h) = if swaps_axes(orientation) {
        (height as u32, width as u32)
    } else {
        (width as u32, height as u32)
    };
    Some(Preview {
        image: DecodedImage::new(
            tw,
            th,
            pixels,
            ImageInfo {
                format: "JPEG".into(),
                pixel_format: "RGB24".into(),
                file_size: metadata.len(),
                icc_profile,
                modified: metadata.modified().ok(),
                has_alpha: false,
                decode_ms: start.elapsed().as_secs_f64() * 1000.0,
                shooting: None,
                gif_background: None,
            },
        ),
        width: full_w,
        height: full_h,
    })
}

/// 缩略图与原图的宽高比不同、多出来的部分是黑边时（相机常把 3:2 的照片放进 4:3 的缩略图），裁掉黑边。
fn crop_letterbox(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    full_w: u32,
    full_h: u32,
) -> (Vec<u8>, u32, u32) {
    let target = f64::from(full_w) / f64::from(full_h);
    let actual = f64::from(width) / f64::from(height);
    if (actual / target - 1.0).abs() < 0.02 {
        return (pixels, width, height);
    }
    let (cw, ch) = if actual > target {
        (((f64::from(height) * target).round() as u32).clamp(1, width), height)
    } else {
        (width, ((f64::from(width) / target).round() as u32).clamp(1, height))
    };
    let (x0, y0) = ((width - cw) / 2, (height - ch) / 2);
    let inside = |x: u32, y: u32| x >= x0 && x < x0 + cw && y >= y0 && y < y0 + ch;
    // 多出来的部分平均亮度很低才算黑边；不是黑边（缩略图本来就是裁过的）时原样拉伸。
    let (mut sum, mut count) = (0u64, 0u64);
    for y in 0..height {
        for x in 0..width {
            if !inside(x, y) {
                let i = (y * width + x) as usize * 4;
                sum += u64::from(*pixels[i..i + 3].iter().max().unwrap_or(&0));
                count += 1;
            }
        }
    }
    if count == 0 || sum / count > 24 {
        return (pixels, width, height);
    }
    let mut out = Vec::with_capacity(cw as usize * ch as usize * 4);
    for y in y0..y0 + ch {
        let row = (y * width + x0) as usize * 4;
        out.extend_from_slice(&pixels[row..row + cw as usize * 4]);
    }
    (out, cw, ch)
}

/// EXIF 方向会交换宽高（含 90°、270° 旋转）。
pub(crate) fn swaps_axes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// 按 EXIF 方向摆正四通道像素（通道顺序不影响旋转、翻转）。
pub(crate) fn orient(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    orientation: Orientation,
) -> (Vec<u8>, u32, u32) {
    if orientation == Orientation::NoTransforms
        || pixels.len() < width as usize * height as usize * 4
    {
        return (pixels, width, height);
    }
    let Some(buffer) = RgbaImage::from_raw(width, height, pixels) else {
        unreachable!("长度已检查");
    };
    let mut image = DynamicImage::ImageRgba8(buffer);
    image.apply_orientation(orientation);
    let rgba = image.into_rgba8();
    let (w, h) = rgba.dimensions();
    (rgba.into_raw(), w, h)
}

/// 测试用：在 JPEG 的 SOI 后面插入 EXIF 段，IFD0 只有方向，IFD1 指向缩略图。`big_endian` 选字节序。
#[doc(hidden)]
pub fn insert_exif_thumbnail(
    main: &[u8],
    thumbnail: &[u8],
    orientation: u16,
    big_endian: bool,
) -> Vec<u8> {
    let mut tiff = Vec::new();
    let u16b = |v: u16| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let u32b = |v: u32| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    tiff.extend_from_slice(if big_endian { b"MM\0*" } else { b"II*\0" });
    tiff.extend_from_slice(&u32b(8));
    // IFD0：1 项（方向），然后是 IFD1 的位置。
    tiff.extend_from_slice(&u16b(1));
    tiff.extend_from_slice(&u16b(0x0112));
    tiff.extend_from_slice(&u16b(3));
    tiff.extend_from_slice(&u32b(1));
    tiff.extend_from_slice(&u16b(orientation));
    tiff.extend_from_slice(&[0, 0]);
    let ifd1 = 8 + 2 + 12 + 4;
    tiff.extend_from_slice(&u32b(ifd1));
    // IFD1：缩略图的位置和长度。
    let data_at = ifd1 + 2 + 12 * 2 + 4;
    tiff.extend_from_slice(&u16b(2));
    for (tag, value) in [(0x0201u16, data_at), (0x0202, thumbnail.len() as u32)] {
        tiff.extend_from_slice(&u16b(tag));
        tiff.extend_from_slice(&u16b(4));
        tiff.extend_from_slice(&u32b(1));
        tiff.extend_from_slice(&u32b(value));
    }
    tiff.extend_from_slice(&u32b(0));
    tiff.extend_from_slice(thumbnail);
    let mut segment = b"Exif\0\0".to_vec();
    segment.extend_from_slice(&tiff);
    let mut out = main[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((segment.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&segment);
    out.extend_from_slice(&main[2..]);
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    /// 编码一张 JPEG（RGB 渐变，可选的纯色块用来检查方向）。
    pub fn encode_jpeg(width: u32, height: u32, quality: u8) -> Vec<u8> {
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                // 左上角是红色块，其余是渐变：摆正后能看出方向。
                if x < width / 4 && y < height / 4 {
                    rgb.extend_from_slice(&[230, 20, 20]);
                } else {
                    rgb.extend_from_slice(&[
                        (x * 255 / width.max(1)) as u8,
                        (y * 255 / height.max(1)) as u8,
                        128,
                    ]);
                }
            }
        }
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, quality)
            .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    #[test]
    fn fast_path_matches_the_generic_decoder() {
        let data = encode_jpeg(301, 203, 90);
        let fast = decode(&data, u64::MAX, DecodeOptions::default()).unwrap().unwrap();
        let generic = image::load_from_memory(&data).unwrap().into_rgba8();
        assert_eq!((fast.width, fast.height), generic.dimensions());
        let worst = fast
            .pixels
            .chunks(4)
            .zip(generic.as_raw().chunks(4))
            .map(|(bgra, rgba)| {
                let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
                d(bgra[0], rgba[2]).max(d(bgra[1], rgba[1])).max(d(bgra[2], rgba[0]))
            })
            .max()
            .unwrap();
        assert!(worst <= 1, "快速路径与通用路径相差 {worst}");
        assert!(fast.pixels.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn grayscale_jpeg_is_left_to_the_generic_path() {
        let gray = vec![100u8; 64 * 64];
        let mut data = Vec::new();
        JpegEncoder::new(&mut data)
            .write_image(&gray, 64, 64, ExtendedColorType::L8)
            .unwrap();
        assert!(decode(&data, u64::MAX, DecodeOptions::default()).unwrap().is_none());
    }

    #[test]
    fn too_large_is_an_error() {
        let data = encode_jpeg(64, 64, 80);
        assert!(decode(&data, 64 * 64 * 4 - 1, DecodeOptions::default()).is_err());
    }

    #[test]
    fn preview_reads_the_exif_thumbnail_and_orients_it() {
        let dir = std::env::temp_dir().join(format!("anyview-preview-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let main = encode_jpeg(400, 200, 80);
        let thumb = encode_jpeg(160, 80, 80);
        for (orientation, big_endian) in [(1u16, false), (6, true)] {
            let path = dir.join(format!("o{orientation}.jpg"));
            fs::write(
                &path,
                insert_exif_thumbnail(&main, &thumb, orientation, big_endian),
            )
            .unwrap();
            // 原图不够大时不出预览。
            assert!(preview(&path).is_none());
            let p = preview_at_least(&path, 0, DecodeOptions::default()).expect("应有预览");
            if orientation == 6 {
                // 顺时针转 90°：原图 400x200 摆正后是 200x400；缩略图的红色块从左上角转到右上角。
                assert_eq!((p.width, p.height), (200, 400));
                assert_eq!((p.image.width, p.image.height), (80, 160));
                let px = |x: u32, y: u32| {
                    let i = (y * p.image.width + x) as usize * 4;
                    [p.image.pixels[i + 2], p.image.pixels[i + 1], p.image.pixels[i]]
                };
                assert!(px(75, 5)[0] > 180 && px(5, 5)[0] < 180, "{:?} {:?}", px(75, 5), px(5, 5));
            } else {
                assert_eq!((p.width, p.height), (400, 200));
                assert_eq!((p.image.width, p.image.height), (160, 80));
            }
            // 完整解码也按同样的方向摆正，尺寸与预览给出的一致。
            let full = decode(&fs::read(&path).unwrap(), u64::MAX, DecodeOptions::default()).unwrap().unwrap();
            assert_eq!((full.width, full.height), (p.width, p.height));
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn letterboxed_thumbnail_is_cropped_but_real_content_is_kept() {
        // 160x120 的缩略图，上下各 13 行黑边，中间是 3:2 的内容。
        let (w, h) = (160u32, 120u32);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for y in 13..107 {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&[90, 140, 200, 255]);
            }
        }
        let (out, cw, ch) = crop_letterbox(pixels.clone(), w, h, 3000, 2000);
        assert_eq!((cw, ch), (160, 107));
        assert_eq!(out.len(), (cw * ch * 4) as usize);
        // 不是黑边：不裁。
        let bright = vec![200u8; (w * h * 4) as usize];
        let (_, cw, ch) = crop_letterbox(bright, w, h, 3000, 2000);
        assert_eq!((cw, ch), (160, 120));
    }
}
