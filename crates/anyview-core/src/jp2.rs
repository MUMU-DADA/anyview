//! JPEG 2000（.jp2 .j2k .j2c .jpc .jpx .jpf）：用 hayro-jpeg2000（纯 Rust，MIT/Apache）解码，
//! 三端相同。JP2 容器和裸码流（以 SOC、SIZ 标记开头）都认。解码器已经处理了调色板、通道定义、sYCC、CIELab、
//! YCCK，并把各种位深统一缩放到 8 位；这里只把各通道排成 RGBA8：
//!
//! - 灰度、RGB（可带透明通道）直接排；
//! - 内嵌 ICC 是 RGB 的交给色彩管理；是灰度、CMYK 的在这里用 qcms 转成 sRGB（界面只认 RGB 像素，与 PSD 相同）；
//!   关掉"使用图片文件的 ICC"、配置文件读不了时按没有配置文件处理；
//! - CMYK 没有配置文件时按简单公式转 RGB；
//! - CIELab 的（解码器给的是 Lab 值和一个 Lab 配置文件）在这里换成 sRGB（Linux、Windows 的 qcms 不认 Lab 配置文件）；
//! - 透明通道是预乘的（cdef 里的类型 2，解码器不认）时换回不预乘；
//! - 其他通道数（多光谱等）前三个按 RGB、一个按灰度。
//!
//! 交给解码器之前先读码流的 SIZ 段，按图像大小、分块数、分量数估算解码器要用的内存，太大的、分块数超出规范的不解。

use crate::i18n::Message;
use hayro_jpeg2000::{ColorSpace, DecodeSettings, DecoderContext, Image};

use crate::color::{self, SampleKind};
use crate::decode::Raster;

/// 扩展名（小写，不带点）。
pub const EXTENSIONS: &[&str] = &["jp2", "j2k", "j2c", "jpc", "jpx", "jpf"];

/// JP2 容器的签名框。
const JP2_MAGIC: &[u8] = b"\x00\x00\x00\x0C\x6A\x50\x20\x20";
/// 裸码流：SOC + SIZ 标记。
const CODESTREAM_MAGIC: &[u8] = b"\xFF\x4F\xFF\x51";

/// 文件头是不是 JPEG 2000（容器或裸码流）。
pub fn is_jpeg2000(head: &[u8]) -> bool {
    head.starts_with(JP2_MAGIC) || head.starts_with(CODESTREAM_MAGIC)
}

/// 解码整个文件。`use_icc` 为假时（关闭“使用图片文件的 ICC”）灰度、CMYK 不用内嵌的配置文件，RGB 的配置文件也不交出去。
pub fn decode(data: &[u8], use_icc: bool) -> Result<Raster, Message> {
    check_size(data)?;
    let premultiplied = premultiplied_alpha(data);
    let image =
        Image::new(data, &DecodeSettings::default()).map_err(|e| msg!("JPEG 2000：{e}", "JPEG 2000: {e}", e = format!("{e}")))?;
    let (width, height) = (image.width(), image.height());
    // 先按尺寸检查（太大的图不解，免得解码器先吃掉大量内存）。
    let mut rgba = Raster::alloc(width, height)?;
    let has_alpha = image.has_alpha();
    let space = image.color_space().clone();
    let depth = image.original_bit_depth();
    let container = data.starts_with(JP2_MAGIC);
    let mut context = DecoderContext::default();
    let decoded = image
        .decode(&mut context)
        .map_err(|e| msg!("JPEG 2000：{e}", "JPEG 2000: {e}", e = format!("{e}")))?;
    let samples = decoded.data_u8();
    let colors = usize::from(space.num_channels());
    let channels = colors + usize::from(has_alpha);
    let pixels = width as usize * height as usize;
    if colors == 0 || samples.len() < pixels * channels {
        return Err(msg!(
            "JPEG 2000：解出的数据不全（{got} 字节，应为 {want}）",
            "JPEG 2000: decoded data is incomplete ({got} bytes, expected {want})",
            got = samples.len().to_string(),
            want = (pixels * channels).to_string()
        ));
    }
    let alpha = |px: &[u8]| if has_alpha { px[colors] } else { 255 };
    let mut icc_out = None;
    let kind = match &space {
        ColorSpace::Gray => Kind::Gray,
        ColorSpace::RGB => Kind::Rgb,
        ColorSpace::CMYK => Kind::Cmyk,
        ColorSpace::Unknown { num_channels } if *num_channels >= 3 => Kind::Rgb,
        ColorSpace::Unknown { .. } => Kind::Gray,
        // CIELab：解码器给的是 Lab 值（L 按 0–100 映射到 0–255，a、b 加了 128）和一个 Lab 配置文件。
        ColorSpace::Icc {
            profile,
            num_channels: 3,
        } if profile.get(16..20) == Some(b"Lab ") => Kind::Lab,
        ColorSpace::Icc {
            profile,
            num_channels,
        } => match num_channels {
            1 => Kind::GrayIcc(profile.clone()),
            4 => Kind::CmykIcc(profile.clone()),
            n if *n >= 3 => {
                if use_icc {
                    icc_out = Some(profile.clone());
                }
                Kind::Rgb
            }
            _ => Kind::Gray,
        },
    };
    let pixel_format = match (&kind, has_alpha, depth > 8) {
        (Kind::Gray | Kind::GrayIcc(_), false, false) => "Gray8",
        (Kind::Gray | Kind::GrayIcc(_), false, true) => "Gray16",
        (Kind::Gray | Kind::GrayIcc(_), true, false) => "GrayA16",
        (Kind::Gray | Kind::GrayIcc(_), true, true) => "GrayA32",
        (Kind::Rgb, false, false) => "RGB24",
        (Kind::Rgb, false, true) => "RGB48",
        (Kind::Rgb, true, false) => "RGBA32",
        (Kind::Rgb, true, true) => "RGBA64",
        (Kind::Lab, _, false) => "Lab24",
        (Kind::Lab, _, true) => "Lab48",
        (Kind::Cmyk | Kind::CmykIcc(_), _, false) => "CMYK32",
        (Kind::Cmyk | Kind::CmykIcc(_), _, true) => "CMYK64",
    };
    let source = samples.chunks_exact(channels).take(pixels);
    match &kind {
        Kind::Rgb => {
            for (out, px) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(source) {
                out.copy_from_slice(&[px[0], px[1], px[2], alpha(px)]);
            }
        }
        Kind::Lab => {
            for (out, px) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(source) {
                let [r, g, b] = lab_to_srgb(
                    f32::from(px[0]) * 100.0 / 255.0,
                    f32::from(px[1]) - 128.0,
                    f32::from(px[2]) - 128.0,
                );
                out.copy_from_slice(&[r, g, b, alpha(px)]);
            }
        }
        Kind::Gray | Kind::GrayIcc(_) => {
            let mut grays = Vec::with_capacity(if matches!(kind, Kind::GrayIcc(_)) {
                pixels
            } else {
                0
            });
            for (out, px) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(source) {
                *out = [px[0], px[0], px[0], alpha(px)];
                if matches!(kind, Kind::GrayIcc(_)) {
                    grays.push(px[0]);
                }
            }
            if let (Kind::GrayIcc(icc), true) = (&kind, use_icc) {
                color::samples_to_srgb(&grays, SampleKind::Gray, icc, &mut rgba);
            }
        }
        Kind::Cmyk | Kind::CmykIcc(_) => {
            // 先按简单公式填好；带配置文件时再按它转（转不了时保留简单公式的结果）。JPEG 2000 的 CMYK 是油墨量（0 = 没有油墨），
            // 与 ICC 的约定相同。
            let mut inks = Vec::with_capacity(if matches!(kind, Kind::CmykIcc(_)) {
                pixels * 4
            } else {
                0
            });
            for (out, px) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(source) {
                let k = 255 - u32::from(px[3]);
                let ink = |c: u8| ((255 - u32::from(c)) * k / 255) as u8;
                *out = [ink(px[0]), ink(px[1]), ink(px[2]), alpha(px)];
                if matches!(kind, Kind::CmykIcc(_)) {
                    inks.extend_from_slice(&px[..4]);
                }
            }
            if let (Kind::CmykIcc(icc), true) = (&kind, use_icc) {
                color::samples_to_srgb(&inks, SampleKind::Cmyk, icc, &mut rgba);
            }
        }
    }
    // 预乘的透明通道（cdef 类型 2）：颜色换回不预乘（之后统一预乘，不然半透明处会变暗）。
    if premultiplied && has_alpha {
        for px in rgba.as_chunks_mut::<4>().0 {
            let a = u32::from(px[3]);
            if a > 0 && a < 255 {
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
    Ok(Raster {
        width,
        height,
        rgba,
        has_alpha,
        format: if container { "JP2" } else { "J2K" }.to_string(),
        pixel_format: pixel_format.to_string(),
        icc_profile: icc_out,
    })
}

/// 怎么把通道排成 RGBA。
enum Kind {
    Gray,
    Rgb,
    Cmyk,
    Lab,
    GrayIcc(Vec<u8>),
    CmykIcc(Vec<u8>),
}

/// CIELab（D50 白点，JPEG 2000 的默认）→ sRGB 8 位：Lab → XYZ→ 线性 sRGB（含 Bradford 换到 D65）→ sRGB 编码。
fn lab_to_srgb(l: f32, a: f32, b: f32) -> [u8; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let inverse = |t: f32| {
        const DELTA: f32 = 6.0 / 29.0;
        if t > DELTA {
            t * t * t
        } else {
            3.0 * DELTA * DELTA * (t - 4.0 / 29.0)
        }
    };
    let (x, y, z) = (0.964_22 * inverse(fx), inverse(fy), 0.825_21 * inverse(fz));
    let linear = [
        3.133_856 * x - 1.616_867 * y - 0.490_615 * z,
        -0.978_768 * x + 1.916_142 * y + 0.033_454 * z,
        0.071_945 * x - 0.228_991 * y + 1.405_243 * z,
    ];
    linear.map(|v| {
        let v = v.clamp(0.0, 1.0);
        let encoded = if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (encoded * 255.0 + 0.5) as u8
    })
}

/// 码流（裸码流，或 JP2 容器里的 jp2c 盒子）。
fn codestream(data: &[u8]) -> Option<&[u8]> {
    if data.starts_with(CODESTREAM_MAGIC) {
        return Some(data);
    }
    boxes(data)
        .find(|(kind, _)| kind == b"jp2c")
        .map(|(_, body)| body)
}

/// 一层盒子：(类型, 内容)。长度不对时停下。
fn boxes(data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let header = data.get(at..at.checked_add(8)?)?;
        let kind: [u8; 4] = header[4..8].try_into().ok()?;
        let (start, len) = match u32::from_be_bytes(header[..4].try_into().ok()?) {
            0 => (at + 8, data.len() - at),
            1 => {
                let large = data.get(at + 8..at + 16)?;
                (
                    at + 16,
                    usize::try_from(u64::from_be_bytes(large.try_into().ok()?)).ok()?,
                )
            }
            n => (at + 8, n as usize),
        };
        let end = at.checked_add(len)?;
        if end < start || end > data.len() {
            return None;
        }
        at = end;
        Some((kind, &data[start..end]))
    })
}

/// JP2 的通道定义（jp2h 里的 cdef）有没有"预乘的透明度"（类型 2）。
fn premultiplied_alpha(data: &[u8]) -> bool {
    let Some((_, header)) = boxes(data).find(|(kind, _)| kind == b"jp2h") else {
        return false;
    };
    let Some((_, cdef)) = boxes(header).find(|(kind, _)| kind == b"cdef") else {
        return false;
    };
    let count = cdef
        .get(..2)
        .map_or(0, |n| usize::from(u16::from_be_bytes([n[0], n[1]])));
    (0..count).any(|i| cdef.get(2 + i * 6 + 2..2 + i * 6 + 4) == Some(&[0, 2][..]))
}

/// 交给解码器之前，按码流的 SIZ 段检查图像大小、分块数和分量数（解码器按分量用 32 位浮点存整幅图）。
fn check_size(data: &[u8]) -> Result<(), Message> {
    let stream = codestream(data).ok_or(msg!("JPEG 2000：文件里没有码流", "JPEG 2000: no codestream in the file"))?;
    let field = |at: usize| -> Result<u32, Message> {
        let bytes = stream.get(at..at + 4).ok_or(msg!("JPEG 2000：SIZ 段不全", "JPEG 2000: the SIZ segment is incomplete"))?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    };
    if !stream.starts_with(CODESTREAM_MAGIC) {
        return Err(msg!("JPEG 2000：码流不是以 SIZ 段开头", "JPEG 2000: the codestream doesn't start with a SIZ segment"));
    }
    let (xsiz, ysiz, xo, yo) = (field(8)?, field(12)?, field(16)?, field(20)?);
    let (xt, yt, xto, yto) = (field(24)?, field(28)?, field(32)?, field(36)?);
    let components = stream
        .get(40..42)
        .map(|c| u64::from(u16::from_be_bytes([c[0], c[1]])))
        .ok_or(msg!("JPEG 2000：SIZ 段不全", "JPEG 2000: the SIZ segment is incomplete"))?;
    if xt == 0 || yt == 0 || xsiz <= xo || ysiz <= yo || xto > xo || yto > yo || components == 0 {
        return Err(msg!("JPEG 2000：SIZ 段的数值不对", "JPEG 2000: invalid values in the SIZ segment"));
    }
    let tiles = u64::from(xsiz - xto)
        .div_ceil(u64::from(xt))
        .saturating_mul(u64::from(ysiz - yto).div_ceil(u64::from(yt)));
    // 分块的序号只有 16 位（规范最多 65535 块）。
    if tiles > 65_535 {
        return Err(msg!("JPEG 2000：分块太多（{tiles}）", "JPEG 2000: too many tiles ({tiles})", tiles = format!("{tiles}")));
    }
    let samples = u64::from(xsiz - xo)
        .saturating_mul(u64::from(ysiz - yo))
        .saturating_mul(components);
    if samples.saturating_mul(4) > crate::decode::alloc_limit() {
        crate::decode::limit_hit();
        return Err(msg!(
            "JPEG 2000：图片太大（{width}x{height}，{components} 个分量）",
            "JPEG 2000: image too large ({width}x{height}, {components} components)",
            width = (xsiz - xo).to_string(),
            height = (ysiz - yo).to_string(),
            components = components.to_string()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_containers_and_codestreams() {
        assert!(is_jpeg2000(
            b"\x00\x00\x00\x0C\x6A\x50\x20\x20\x0D\x0A\x87\x0A"
        ));
        assert!(is_jpeg2000(b"\xFF\x4F\xFF\x51\x00\x2F"));
        assert!(!is_jpeg2000(b"\xFF\xD8\xFF\xE0"));
        assert!(!is_jpeg2000(b""));
    }

    #[test]
    fn oversized_tile_grids_are_refused_before_decoding() {
        // SOC + SIZ：4096x4096，1x1 的分块（1600 多万块），3 个分量。
        let mut stream = CODESTREAM_MAGIC.to_vec();
        stream.extend_from_slice(&[0, 47, 0, 0]);
        for value in [4096u32, 4096, 0, 0, 1, 1, 0, 0] {
            stream.extend_from_slice(&value.to_be_bytes());
        }
        stream.extend_from_slice(&3u16.to_be_bytes());
        stream.extend_from_slice(&[7, 1, 1, 7, 1, 1, 7, 1, 1]);
        let error = decode(&stream, true).unwrap_err();
        assert!(error.text(crate::i18n::Lang::ZhHans).contains("分块太多"), "{error}");
    }

    #[test]
    fn lab_converts_to_srgb() {
        // L=100、a=b=0 是白，L=0 是黑；Lab (53.24, 80.09, 67.20) 是 sRGB 的纯红。
        assert_eq!(lab_to_srgb(100.0, 0.0, 0.0), [255, 255, 255]);
        assert_eq!(lab_to_srgb(0.0, 0.0, 0.0), [0, 0, 0]);
        let [r, g, b] = lab_to_srgb(53.24, 80.09, 67.20);
        assert!(r > 245 && g < 15 && b < 15, "{:?}", [r, g, b]);
    }

    #[test]
    fn premultiplied_channel_definitions_are_found() {
        // jp2h 里的 cdef：4 个通道，第 4 个类型 2（预乘的透明度）。
        let mut cdef = 4u16.to_be_bytes().to_vec();
        for (channel, kind, association) in [(0u16, 0u16, 1u16), (1, 0, 2), (2, 0, 3), (3, 2, 0)] {
            for v in [channel, kind, association] {
                cdef.extend_from_slice(&v.to_be_bytes());
            }
        }
        let mut jp2h = ((8 + cdef.len()) as u32).to_be_bytes().to_vec();
        jp2h.extend_from_slice(b"cdef");
        jp2h.extend_from_slice(&cdef);
        let mut file = JP2_MAGIC.to_vec();
        file.extend_from_slice(&[0x0D, 0x0A, 0x87, 0x0A]);
        file.extend_from_slice(&((8 + jp2h.len()) as u32).to_be_bytes());
        file.extend_from_slice(b"jp2h");
        file.extend_from_slice(&jp2h);
        assert!(premultiplied_alpha(&file));
    }

    #[test]
    fn broken_files_report_errors_instead_of_panicking() {
        // 只有签名、后面是乱码：报错，不崩溃、不卡住。
        let mut data = JP2_MAGIC.to_vec();
        data.extend_from_slice(&[0x0D, 0x0A, 0x87, 0x0A]);
        data.extend((0..2000u32).map(|i| (i * 7919 % 251) as u8));
        assert!(decode(&data, true).is_err());
        let mut stream = CODESTREAM_MAGIC.to_vec();
        stream.extend((0..500u32).map(|i| (i * 31 % 253) as u8));
        assert!(decode(&stream, true).is_err());
    }
}
