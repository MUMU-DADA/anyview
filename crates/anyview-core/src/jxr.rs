//! JPEG XR（.jxr .wdp .hdp）：用微软开源的 jxrlib（anyview-jxr，从源代码编译）。
//! Windows 先交给系统自带的 WIC，WIC 解不了的再走这里。
//!
//! jxrlib 把各种像素格式统一换成 8 位的灰度、RGB 或 RGBA；高动态范围的（半精度、单精度浮点、定点、RGBE，如游戏的 HDR
//! 截图）按 scRGB 的线性值换成 sRGB，超出范围的截断（与 OpenEXR 相同）。8、16 位整数的格式带着内嵌的 ICC 时交给色彩管理。
//!
//! 交给 jxrlib 之前先在这里把容器重新拼一遍（`rebuild`）：只留解码要用的几项（像素格式、方向、宽高、分辨率、图像和透明数据），
//! 核对每一项的位置都在文件里面，方向的值合法；元数据（EXIF、GPS、XMP、IPTC、描述信息）全去掉，ICC 在这里取出。
//! jxrlib 解析元数据的代码有越界写（长度相加溢出）、无限递归（EXIF 里的 IFD 互相引用）的问题，
//! 这样它们根本不会被用到。

use crate::i18n::Message;
use crate::color::{self, SampleKind};
use crate::decode::{Raster, alloc_limit};

/// 扩展名（小写，不带点）。
pub const EXTENSIONS: &[&str] = &["jxr", "wdp", "hdp"];

/// 文件头是不是 JPEG XR（"II"、0xBC、版本 0 或 1）。
pub fn is_jxr(head: &[u8]) -> bool {
    head.len() >= 4 && head[..3] == *b"II\xBC" && head[3] <= 1
}

/// 解码。`use_icc` 为假时（关闭“使用图片文件的 ICC”）不用内嵌的配置文件。
/// jxrlib 外壳报的原因按界面语言写出来。
fn jxr_error(e: anyview_jxr::JxrError) -> Message {
    use anyview_jxr::JxrError as E;
    let size = |w: i64, h: i64| format!("{w}x{h}");
    let reason = match e {
        E::BadSize(w, h) => msg!("图像尺寸不对：{size}", "Invalid image size: {size}", size = size(w, h)),
        E::TooLarge(w, h) => msg!("图片太大：{size}", "Image too large: {size}", size = size(w, h)),
        E::PixelFormat(name) => msg!("不支持的像素格式 {name}", "Unsupported pixel format {name}", name = name),
        E::NoMemory(w, h) => msg!("内存不够：{size}", "Not enough memory: {size}", size = size(w, h)),
        E::Library(code) => msg!("jxrlib 错误 {code}", "jxrlib error {code}", code = code.to_string()),
        E::Unavailable => msg!(
            "这台机器上编译时没能编进 jxrlib，JPEG XR 不可用",
            "JPEG XR isn't available: jxrlib could not be built on this machine"
        ),
    };
    msg!("JPEG XR：{e}", "JPEG XR: {e}", e = reason)
}

pub fn decode(data: &[u8], use_icc: bool) -> Result<Raster, Message> {
    let (clean, icc) = rebuild(data)?;
    let image =
        anyview_jxr::decode(&clean, alloc_limit() as usize).map_err(jxr_error)?;
    let mut rgba = Raster::alloc(image.width, image.height)?;
    let pixels = image.width as usize * image.height as usize;
    let channels = image.channels as usize;
    if !(channels == 1 || channels == 3 || channels == 4) || image.pixels.len() < pixels * channels
    {
        return Err(msg!("JPEG XR：解出的数据不对（{channels} 个通道）", "JPEG XR: unexpected decoded data ({channels} channels)", channels = format!("{channels}")));
    }
    for (out, px) in rgba
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(image.pixels.chunks_exact(channels))
    {
        match channels {
            1 => *out = [px[0], px[0], px[0], 255],
            3 => *out = [px[0], px[1], px[2], 255],
            _ => out.copy_from_slice(px),
        }
    }
    // 只有 8、16 位整数的格式用 ICC（高动态范围的已经按 scRGB 换成 sRGB）。
    let integer = !["Half", "Float", "Fixed"]
        .iter()
        .any(|kind| image.pixel_format.ends_with(kind))
        && !image.pixel_format.starts_with("RGBE");
    let icc = icc.filter(|_| use_icc && integer);
    let icc_profile = match (channels, icc) {
        // 灰度的配置文件在这里用（界面只认 RGB 像素，与 PSD、JPEG 2000 相同）。
        (1, Some(profile)) => {
            color::samples_to_srgb(&image.pixels, SampleKind::Gray, &profile, &mut rgba);
            None
        }
        (_, icc) => icc,
    };
    Ok(Raster {
        width: image.width,
        height: image.height,
        rgba,
        has_alpha: channels == 4,
        format: "JXR".to_string(),
        pixel_format: image.pixel_format,
        icc_profile,
    })
}

const TAG_PIXEL_FORMAT: u16 = 0xBC01;
const TAG_TRANSFORMATION: u16 = 0xBC02;
const TAG_IMAGE_OFFSET: u16 = 0xBCC0;
const TAG_IMAGE_BYTE_COUNT: u16 = 0xBCC1;
const TAG_ALPHA_OFFSET: u16 = 0xBCC2;
const TAG_ALPHA_BYTE_COUNT: u16 = 0xBCC3;
const TAG_ICC_PROFILE: u16 = 0x8773;
/// 原样留下的几项（单个值，不指向别处）：压缩、图像类型、宽、高、横竖分辨率、图像和透明数据的丢弃标记。
const KEPT_VALUES: [u16; 8] = [
    0xBC03, 0xBC04, 0xBC80, 0xBC81, 0xBC82, 0xBC83, 0xBCC4, 0xBCC5,
];

/// 重新拼一个只含解码所需各项的 JPEG XR 容器，返回它和内嵌的 ICC（核对过位置、不超过 16 MB）。
fn rebuild(data: &[u8]) -> Result<(Vec<u8>, Option<Vec<u8>>), Message> {
    let u16_at = |at: usize| {
        data.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let u32_at = |at: usize| {
        data.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let bad = |what: Message| {
        msg!("JPEG XR：文件结构不对（{what}）", "JPEG XR: invalid file structure ({what})", what = what)
    };
    if !is_jxr(data) {
        return Err(bad(msg!("文件头", "header")));
    }
    let ifd = u32_at(4).ok_or_else(|| bad(msg!("目录位置", "directory offset")))? as usize;
    let count = usize::from(u16_at(ifd).ok_or_else(|| bad(msg!("目录", "directory")))?);
    // 一个条目：(标签, 类型, 个数, 值或位置)。
    let mut entries: Vec<(u16, u16, u32, u32)> = Vec::new();
    for i in 0..count {
        let at = ifd + 2 + i * 12;
        let (Some(tag), Some(kind), Some(n), Some(value)) =
            (u16_at(at), u16_at(at + 2), u32_at(at + 4), u32_at(at + 8))
        else {
            return Err(bad(msg!("目录不全", "incomplete directory")));
        };
        entries.push((tag, kind, n, value));
    }
    let find = |tag: u16| entries.iter().find(|e| e.0 == tag).copied();
    let range = |offset: u32, len: u32| -> Option<std::ops::Range<usize>> {
        let start = offset as usize;
        let end = start.checked_add(len as usize)?;
        (end <= data.len()).then_some(start..end)
    };
    // 图像（和透明）数据：长度写 0 时到下一段数据或文件结尾。
    let segment = |offset_tag: u16,
                   count_tag: u16|
     -> Result<Option<std::ops::Range<usize>>, Message> {
        let Some((_, _, 1, offset)) = find(offset_tag) else {
            return Ok(None);
        };
        let len = match find(count_tag) {
            Some((_, _, 1, len)) if len > 0 => len,
            _ => {
                let next = entries
                    .iter()
                    .filter(|e| matches!(e.0, TAG_IMAGE_OFFSET | TAG_ALPHA_OFFSET) && e.3 > offset)
                    .map(|e| e.3 as usize)
                    .min()
                    .unwrap_or(data.len());
                u32::try_from(next.saturating_sub(offset as usize)).map_err(|_| bad(msg!("数据长度", "data length")))?
            }
        };
        range(offset, len)
            .map(Some)
            .ok_or_else(|| bad(msg!("数据越过文件结尾", "data past the end of the file")))
    };
    let image =
        segment(TAG_IMAGE_OFFSET, TAG_IMAGE_BYTE_COUNT)?.ok_or_else(|| bad(msg!("没有图像数据", "no image data")))?;
    let alpha = segment(TAG_ALPHA_OFFSET, TAG_ALPHA_BYTE_COUNT)?;
    let guid = match find(TAG_PIXEL_FORMAT) {
        Some((_, _, 16, offset)) => range(offset, 16).ok_or_else(|| bad(msg!("像素格式", "pixel format")))?,
        _ => return Err(bad(msg!("没有像素格式", "no pixel format"))),
    };
    let icc = match find(TAG_ICC_PROFILE) {
        Some((_, _, n, offset)) if n > 0 && n <= 16 << 20 => {
            range(offset, n).map(|r| data[r].to_vec())
        }
        _ => None,
    };
    // 新的目录：标签从小到大。
    let mut kept: Vec<(u16, u16, u32, u32)> = entries
        .iter()
        .filter(|e| KEPT_VALUES.contains(&e.0) && e.2 == 1)
        .copied()
        .collect();
    if let Some((tag, kind, 1, value)) = find(TAG_TRANSFORMATION) {
        // 方向只有 8 种（jxrlib 里遇到别的值会直接中止程序）。
        if value >= 8 {
            return Err(bad(msg!("方向", "orientation")));
        }
        kept.push((tag, kind, 1, value));
    }
    let directory = 8 + 2 + (kept.len() + 5) * 12 + 4;
    let guid_at = directory as u32;
    let image_at = guid_at + 16;
    let image_len = image.len() as u32;
    kept.push((TAG_PIXEL_FORMAT, 1, 16, guid_at));
    kept.push((TAG_IMAGE_OFFSET, 4, 1, image_at));
    kept.push((TAG_IMAGE_BYTE_COUNT, 4, 1, image_len));
    if let Some(alpha) = &alpha {
        kept.push((TAG_ALPHA_OFFSET, 4, 1, image_at + image_len));
        kept.push((TAG_ALPHA_BYTE_COUNT, 4, 1, alpha.len() as u32));
    }
    kept.sort_by_key(|e| e.0);
    let mut out =
        Vec::with_capacity(directory + 16 + image.len() + alpha.as_ref().map_or(0, |a| a.len()));
    out.extend_from_slice(&data[..4]);
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(kept.len() as u16).to_le_bytes());
    for (tag, kind, n, value) in &kept {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.resize(directory, 0);
    out.extend_from_slice(&data[guid]);
    out.extend_from_slice(&data[image]);
    if let Some(alpha) = alpha {
        out.extend_from_slice(&data[alpha]);
    }
    Ok((out, icc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_signature() {
        assert!(is_jxr(b"II\xBC\x01\x08\x00\x00\x00"));
        assert!(is_jxr(b"II\xBC\x00"));
        assert!(!is_jxr(b"II*\x00"));
        assert!(!is_jxr(b"II\xBC\x02"));
    }

    /// 元数据在交给 jxrlib 之前就去掉了：EXIF 的目录指向自己（jxrlib 会无限递归）、ICC 的长度接近 4 GB
    /// （jxrlib 算缓冲区大小时溢出）的文件照样只解出图像。
    #[test]
    fn hostile_metadata_never_reaches_jxrlib() {
        let original: &[u8] = include_bytes!("../../anyview-fixtures/data/still.jxr");
        let le16 = |d: &[u8], at: usize| u16::from_le_bytes([d[at], d[at + 1]]);
        let le32 =
            |d: &[u8], at: usize| u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]);
        let ifd = le32(original, 4) as usize;
        let count = usize::from(le16(original, ifd));
        let mut entries: Vec<[u8; 12]> = (0..count)
            .map(|i| {
                original[ifd + 2 + i * 12..ifd + 14 + i * 12]
                    .try_into()
                    .unwrap()
            })
            .collect();
        let mut data = original.to_vec();
        data.resize(data.len() + data.len() % 2, 0);
        let new_ifd = data.len();
        let exif_ifd = new_ifd + 2 + (count + 2) * 12 + 4;
        let entry = |tag: u16, kind: u16, n: u32, value: u32| {
            let mut e = [0u8; 12];
            e[..2].copy_from_slice(&tag.to_le_bytes());
            e[2..4].copy_from_slice(&kind.to_le_bytes());
            e[4..8].copy_from_slice(&n.to_le_bytes());
            e[8..].copy_from_slice(&value.to_le_bytes());
            e
        };
        entries.push(entry(0x8769, 4, 1, exif_ifd as u32));
        entries.push(entry(0x8773, 7, 0xFFFF_FFFE, 8));
        entries.sort_by_key(|e| u16::from_le_bytes([e[0], e[1]]));
        data.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for e in &entries {
            data.extend_from_slice(e);
        }
        data.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(data.len(), exif_ifd);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&entry(0x8769, 4, 1, exif_ifd as u32));
        data.extend_from_slice(&0u32.to_le_bytes());
        data[4..8].copy_from_slice(&(new_ifd as u32).to_le_bytes());
        let (clean, icc) = rebuild(&data).unwrap();
        assert!(icc.is_none(), "长度不对的 ICC 不要");
        assert!(
            !clean.windows(2).any(|w| w == 0x8769u16.to_le_bytes()),
            "EXIF 的条目去掉了"
        );
        let raster = decode(&data, true).unwrap();
        assert_eq!((raster.width, raster.height), (128, 96));
        // 方向的值不合法：拒绝（jxrlib 遇到会中止程序）。
        let mut bad = original.to_vec();
        let mut entries: Vec<[u8; 12]> = (0..count)
            .map(|i| {
                original[ifd + 2 + i * 12..ifd + 14 + i * 12]
                    .try_into()
                    .unwrap()
            })
            .collect();
        entries.retain(|e| u16::from_le_bytes([e[0], e[1]]) != 0xBC02);
        entries.push(entry(0xBC02, 1, 1, 9));
        entries.sort_by_key(|e| u16::from_le_bytes([e[0], e[1]]));
        let at = bad.len();
        bad.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for e in &entries {
            bad.extend_from_slice(e);
        }
        bad.extend_from_slice(&0u32.to_le_bytes());
        bad[4..8].copy_from_slice(&(at as u32).to_le_bytes());
        assert!(decode(&bad, true).is_err());
    }

    #[test]
    fn broken_files_report_errors() {
        let mut data = b"II\xBC\x01".to_vec();
        data.extend((0..3000u32).map(|i| (i * 7919 % 251) as u8));
        assert!(decode(&data, true).is_err());
        assert!(decode(b"II\xBC\x01", true).is_err());
    }
}
