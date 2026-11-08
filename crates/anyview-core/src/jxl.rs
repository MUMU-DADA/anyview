//! JPEG XL：用 jxl-oxide 解码（纯 Rust），含动图。
//!
//! 文件里写了颜色编码（没有 ICC）时请解码器直接输出 sRGB；带 ICC 时按原样输出，ICC 交给色彩管理，
//! 万一没有色彩管理模块转不了，就退回请它输出 sRGB。方向（码流头里的）总是摆正。

use crate::i18n::Message;
use std::io::Cursor;

use jxl_oxide::{AllocTracker, EnumColourEncoding, JxlImage, RenderingIntent};

use crate::decode::{Raster, alloc_limit};

/// JPEG XL 的文件头：裸码流（FF 0A）或容器（ISO BMFF，"JXL " 签名盒）。
pub fn is_jxl(head: &[u8]) -> bool {
    head.starts_with(&[0xFF, 0x0A])
        || head.starts_with(&[0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A])
}

/// 读好的一个 JPEG XL 文件。
pub struct Jxl {
    image: JxlImage,
    /// 输出像素所在的色彩空间（ICC）；为空表示 sRGB。
    icc: Option<Vec<u8>>,
}

/// 一帧：RGBA8（未预乘）和显示时长（毫秒，静态图为 0）。
pub struct JxlFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub has_alpha: bool,
    pub delay_ms: u32,
}

impl Jxl {
    pub fn open(data: &[u8]) -> Result<Self, Message> {
        // 几百字节的文件可以写一个几万像素见方的纯色画面：先看文件头写的尺寸，太大的不解（解码器要先在自己的
        // 缓冲区里把整张图画出来，比最后的 RGBA 大好几倍）；解码器自己的内存也给个上限，超了报错而不是把内存用光。
        let limit = alloc_limit();
        let mut image = JxlImage::builder()
            .alloc_tracker(AllocTracker::with_limit(
                usize::try_from(limit.saturating_mul(4)).unwrap_or(usize::MAX),
            ))
            .read(Cursor::new(data))
            .map_err(|e| e.to_string())?;
        let (width, height) = (image.width(), image.height());
        if u64::from(width) * u64::from(height) * 4 > limit {
            crate::decode::limit_hit();
            return Err(msg!("图片太大：{width}x{height}", "Image too large: {width}x{height}", width = format!("{width}"), height = format!("{height}")));
        }
        let icc = image.original_icc().map(<[u8]>::to_vec);
        if icc.is_none() {
            image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
        }
        Ok(Self { image, icc })
    }

    /// 已经读到的帧数（动图的每一帧；静态图是 1）。
    pub fn frame_count(&self) -> usize {
        self.image.num_loaded_keyframes()
    }

    /// 是动图：文件头写了动画，而且不止一帧。
    pub fn is_animated(&self) -> bool {
        self.image.image_header().metadata.animation.is_some() && self.frame_count() > 1
    }

    pub fn icc(&self) -> Option<&[u8]> {
        self.icc.as_deref()
    }

    /// 解出第 `index` 帧。
    pub fn frame(&mut self, index: usize) -> Result<JxlFrame, Message> {
        let render = match self.image.render_frame(index) {
            Ok(render) => render,
            // 带 ICC 的图没有色彩管理模块时转不了：改为输出 sRGB 再试一次。
            Err(_) if self.icc.is_some() => {
                self.icc = None;
                self.image
                    .request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
                self.image.render_frame(index).map_err(|e| e.to_string())?
            }
            Err(e) => return Err(e.to_string().into()),
        };
        let mut stream = render.stream();
        let (width, height, channels) = (stream.width(), stream.height(), stream.channels());
        let mut rgba = Raster::alloc(width, height)?;
        let count = width as usize * height as usize;
        let mut samples = vec![0u8; count * channels as usize];
        stream.write_to_buffer::<u8>(&mut samples);
        let has_alpha = matches!(channels, 2 | 4) && !self.image.pixel_format().has_black();
        match channels {
            1 | 2 => {
                for (out, s) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(samples.chunks_exact(channels as usize)) {
                    out[..3].fill(s[0]);
                    out[3] = if channels == 2 { s[1] } else { 255 };
                }
            }
            3 | 4 if !self.image.pixel_format().has_black() => {
                for (out, s) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(samples.chunks_exact(channels as usize)) {
                    out[..3].copy_from_slice(&s[..3]);
                    out[3] = if channels == 4 { s[3] } else { 255 };
                }
            }
            _ => return Err(msg!("不支持的 JPEG XL 像素格式（{channels} 个通道）", "Unsupported JPEG XL pixel format ({channels} channels)", channels = format!("{channels}"))),
        }
        Ok(JxlFrame {
            width,
            height,
            rgba,
            has_alpha,
            delay_ms: self.delay_ms(render.duration()),
        })
    }

    /// 帧时长：码流里是"刻"，每秒刻数写在动画头里。
    fn delay_ms(&self, ticks: u32) -> u32 {
        match &self.image.image_header().metadata.animation {
            Some(a) if a.tps_numerator > 0 => {
                (u64::from(ticks) * 1000 * u64::from(a.tps_denominator) / u64::from(a.tps_numerator))
                    .min(u64::from(u32::MAX)) as u32
            }
            _ => 0,
        }
    }
}

/// 解出第一帧（静态显示用）。
pub fn decode(data: &[u8]) -> Result<Raster, Message> {
    let mut jxl = Jxl::open(data)?;
    let frame = jxl.frame(0)?;
    Ok(Raster {
        width: frame.width,
        height: frame.height,
        rgba: frame.rgba,
        has_alpha: frame.has_alpha,
        format: "JXL".to_string(),
        pixel_format: if frame.has_alpha { "RGBA32" } else { "RGB24" }.to_string(),
        icc_profile: jxl.icc().map(<[u8]>::to_vec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_both_kinds_of_header() {
        assert!(is_jxl(&[0xFF, 0x0A, 1, 2]));
        assert!(is_jxl(&[0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A, 0]));
        assert!(!is_jxl(b"\x89PNG"));
        assert!(decode(b"\xFF\x0Agarbage").is_err());
        assert!(decode(&[]).is_err());
    }

    /// 尺寸超过内存上限的在打开时就拒绝，不等解码器把整张图画出来。
    #[test]
    fn oversized_images_are_refused_before_rendering() {
        let data = include_bytes!("../../anyview-fixtures/data/still.jxl");
        assert!(Jxl::open(data).is_ok());
        // 128x96 的 RGBA 是 49152 字节。
        let refused = crate::decode::with_alloc_limit(40 << 10, || Jxl::open(data).map(|_| ()));
        assert!(refused.is_err_and(|e| e.text(crate::i18n::Lang::ZhHans).contains("图片太大")));
        assert!(crate::decode::with_alloc_limit(64 << 10, || Jxl::open(data)).is_ok());
        assert!(crate::decode::with_alloc_limit(64 << 10, || decode(data)).is_ok());
    }
}
