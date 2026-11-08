//! Radiance HDR（.hdr）：image crate 带的解码器（RGBE，行程编码）。线性值按通道截到普通亮度、
//! 再按 sRGB 编码显示，与 OpenEXR 相同。
//! 文件头写成 `#?RGBE` 的（有的程序这样写）换成 `#?RADIANCE` 再交给解码器；XYZE 的不支持（解码器报错）。

use std::io::Cursor;

use image::codecs::hdr::HdrDecoder;
use image::{DynamicImage, ImageDecoder};

use crate::decode::{Raster, alloc_limit};
use crate::i18n::Message;

/// 扩展名（小写，不带点）。
pub const EXTENSIONS: &[&str] = &["hdr"];

const SIGNATURE: &[u8] = b"#?RADIANCE";
const SIGNATURE_RGBE: &[u8] = b"#?RGBE";

/// 文件开头是不是 Radiance HDR。
pub fn is_radiance(head: &[u8]) -> bool {
    head.starts_with(SIGNATURE) || head.starts_with(SIGNATURE_RGBE)
}

pub fn decode(data: &[u8]) -> Result<Raster, Message> {
    let fixed;
    let bytes: &[u8] = match data.strip_prefix(SIGNATURE_RGBE) {
        Some(rest) => {
            fixed = [SIGNATURE, rest].concat();
            &fixed
        }
        None => data,
    };
    let decoder = HdrDecoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(msg!("图像尺寸不对：{width}x{height}", "Invalid image size: {width}x{height}", width = format!("{width}"), height = format!("{height}")));
    }
    // 解码器先解成每像素 12 字节的浮点数，再换成 4 字节的 RGBA：两份一起算，按文件头写的尺寸先看放不放得下
    // （几百字节的文件可以写一个几万像素见方的画面）。
    if (u64::from(width) * u64::from(height)).saturating_mul(16) > alloc_limit() {
        crate::decode::limit_hit();
        return Err(msg!("图片太大：{width}x{height}", "Image too large: {width}x{height}", width = format!("{width}"), height = format!("{height}")));
    }
    let image = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    // 直接从每像素 3 个浮点数换成 RGBA8（不先换成 4 个浮点数的 RGBA，少占一份内存）。
    let rgba = match image {
        DynamicImage::ImageRgb32F(float) => {
            let mut rgba = Vec::with_capacity(float.as_raw().len() / 3 * 4);
            for p in float.as_raw().as_chunks::<3>().0 {
                rgba.extend_from_slice(&[
                    crate::psd::linear_to_srgb8(p[0]),
                    crate::psd::linear_to_srgb8(p[1]),
                    crate::psd::linear_to_srgb8(p[2]),
                    255,
                ]);
            }
            rgba
        }
        other => crate::decode::hdr_to_srgb8(other).into_raw(),
    };
    Ok(Raster {
        width,
        height,
        rgba,
        has_alpha: false,
        format: "HDR".to_string(),
        pixel_format: "RGBE32".to_string(),
        icc_profile: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    /// 2x1：左边线性 0.5（sRGB 约 188），右边 (2, 0.25, 0.25)（红截到 255，绿、蓝约 137）。
    fn sample() -> Vec<u8> {
        let image: ImageBuffer<Rgb<f32>, Vec<f32>> =
            ImageBuffer::from_fn(2, 1, |x, _| if x == 0 { Rgb([0.5, 0.5, 0.5]) } else { Rgb([2.0, 0.25, 0.25]) });
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb32F(image)
            .write_to(&mut bytes, image::ImageFormat::Hdr)
            .unwrap();
        bytes.into_inner()
    }

    fn near(got: &[u8], want: [u8; 3]) -> bool {
        got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 2)
    }

    #[test]
    fn decodes_and_clips_like_exr() {
        let data = sample();
        assert!(is_radiance(&data));
        let raster = decode(&data).unwrap();
        assert_eq!((raster.width, raster.height, raster.has_alpha), (2, 1, false));
        assert_eq!((raster.format.as_str(), raster.pixel_format.as_str()), ("HDR", "RGBE32"));
        assert!(near(&raster.rgba[0..3], [188, 188, 188]), "{:?}", &raster.rgba[0..4]);
        assert!(near(&raster.rgba[4..7], [255, 137, 137]), "超过普通亮度的截到最亮：{:?}", &raster.rgba[4..8]);
        assert_eq!((raster.rgba[3], raster.rgba[7]), (255, 255));
    }

    /// 文件头写成 `#?RGBE` 的也能解。
    #[test]
    fn accepts_the_rgbe_signature() {
        let data = sample();
        let rgbe = [SIGNATURE_RGBE, &data[SIGNATURE.len()..]].concat();
        assert!(is_radiance(&rgbe));
        assert_eq!(decode(&rgbe).unwrap().rgba, decode(&data).unwrap().rgba);
    }

    /// 文件头写的尺寸放不下时不解，记一笔“被上限拦下”。
    #[test]
    fn refuses_huge_images_before_decoding() {
        let data = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 60000 +X 60000\n".to_vec();
        let error = crate::decode::with_alloc_limit(1 << 30, || decode(&data)).unwrap_err();
        assert!(error.text(crate::i18n::Lang::En).contains("too large"), "{}", error.text(crate::i18n::Lang::En));
        assert!(decode(b"#?RADIANCE\nnot a header").is_err());
    }
}
