//! PCX：ZSoft PCX，RLE 压缩或不压缩；1、2、4、8 位，1–4 个平面；
//! 黑白、文件头里的 16 色调色板、文件末尾的 256 色调色板和 24/32 位真彩色。
//!
//! - 每行按平面依次存放（每个平面 `bytes_per_line` 字节，含补齐）；图像范围是文件头里的 Xmin..Xmax、Ymin..Ymax。
//! - 位数 × 平面数不超过 4 位的是索引色：1 位 1 平面是黑白；其余用文件头的 16 色调色板
//!   （版本 3 或调色板全是 0 时用标准 EGA 调色板）。
//! - 8 位 1 平面：文件末尾 0x0C 之后的 256 色调色板；没有时按灰度。8 位 3、4 平面是 RGB、RGBA。
//! - 数据不够（文件被截短）时，缺的部分按 0 显示，不报错。

use crate::i18n::Message;
use crate::decode::Raster;

const TRUNCATED: Message = msg!("PCX 文件不完整", "The PCX file is incomplete");

/// 标准 EGA 16 色。
const EGA: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00],
    [0x00, 0x00, 0xAA],
    [0x00, 0xAA, 0x00],
    [0x00, 0xAA, 0xAA],
    [0xAA, 0x00, 0x00],
    [0xAA, 0x00, 0xAA],
    [0xAA, 0x55, 0x00],
    [0xAA, 0xAA, 0xAA],
    [0x55, 0x55, 0x55],
    [0x55, 0x55, 0xFF],
    [0x55, 0xFF, 0x55],
    [0x55, 0xFF, 0xFF],
    [0xFF, 0x55, 0x55],
    [0xFF, 0x55, 0xFF],
    [0xFF, 0xFF, 0x55],
    [0xFF, 0xFF, 0xFF],
];

fn u16_at(header: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([header[at], header[at + 1]])
}

/// 解码整个文件。格式不对、数据坏了时返回错误说明（不 panic）。
pub fn decode(data: &[u8]) -> Result<Raster, Message> {
    let header = data.get(..128).ok_or(TRUNCATED)?;
    if header[0] != 0x0A {
        return Err(msg!("不是 PCX 文件", "Not a PCX file"));
    }
    let version = header[1];
    let encoding = header[2];
    let bits = usize::from(header[3]);
    let (xmin, ymin, xmax, ymax) = (
        u16_at(header, 4),
        u16_at(header, 6),
        u16_at(header, 8),
        u16_at(header, 10),
    );
    let planes = usize::from(header[65]);
    let bytes_per_line = usize::from(u16_at(header, 66));
    if xmax < xmin || ymax < ymin {
        return Err(msg!("PCX 的图像范围不对：{xmin},{ymin} – {xmax},{ymax}", "Invalid PCX image bounds: {xmin},{ymin} – {xmax},{ymax}", xmin = format!("{xmin}"), ymin = format!("{ymin}"), xmax = format!("{xmax}"), ymax = format!("{ymax}")));
    }
    let width = u32::from(xmax - xmin) + 1;
    let height = u32::from(ymax - ymin) + 1;
    let w = width as usize;
    let h = height as usize;
    let supported = matches!(
        (bits, planes),
        (1, 1..=4) | (2, 1) | (4, 1) | (8, 1) | (8, 3) | (8, 4)
    );
    if !supported {
        return Err(msg!("不支持的 PCX：每平面 {bits} 位、{planes} 个平面", "Unsupported PCX: {bits} bits per plane, {planes} planes", bits = format!("{bits}"), planes = format!("{planes}")));
    }
    if !matches!(encoding, 0 | 1) {
        return Err(msg!("不认识的 PCX 压缩方式：{encoding}", "Unknown PCX encoding: {encoding}", encoding = format!("{encoding}")));
    }
    // 每个平面一行至少要放得下整行像素；补齐太多的当成坏文件（免得按它分配大块内存）。
    let needed = (w * bits).div_ceil(8);
    if bytes_per_line < needed || bytes_per_line > needed + 256 {
        return Err(msg!("PCX 每行的字节数不对：{bytes_per_line}", "Invalid PCX bytes per line: {bytes_per_line}", bytes_per_line = format!("{bytes_per_line}")));
    }
    let line = bytes_per_line * planes;
    let total = line.checked_mul(h).ok_or(TRUNCATED)?;
    let body = &data[128..];
    // RLE 最多把 2 个字节展开成 63 个：数据少得装不下这么大的图时是坏文件（截短一点的照样显示）。
    if body.is_empty() || total > body.len().saturating_mul(64) {
        return Err(TRUNCATED);
    }
    let mut rgba = Raster::alloc(width, height)?;
    let pixels = if encoding == 1 {
        unpack_rle(body, total)
    } else {
        let mut raw = body[..body.len().min(total)].to_vec();
        raw.resize(total, 0);
        raw
    };

    let (pixel_format, has_alpha) = match (bits, planes) {
        (8, 3 | 4) => {
            for y in 0..h {
                let scan = &pixels[y * line..(y + 1) * line];
                for x in 0..w {
                    let at = (y * w + x) * 4;
                    for c in 0..planes {
                        rgba[at + c] = scan[c * bytes_per_line + x];
                    }
                    if planes == 3 {
                        rgba[at + 3] = 255;
                    }
                }
            }
            if planes == 3 {
                ("RGB24", false)
            } else {
                ("RGBA32", rgba.iter().skip(3).step_by(4).any(|&a| a < 255))
            }
        }
        (8, 1) => {
            let palette = vga_palette(data, version);
            for y in 0..h {
                let scan = &pixels[y * line..(y + 1) * line];
                for x in 0..w {
                    let [r, g, b] = palette[usize::from(scan[x])];
                    rgba[(y * w + x) * 4..][..4].copy_from_slice(&[r, g, b, 255]);
                }
            }
            ("Indexed8", false)
        }
        _ => {
            // 索引色：每个像素的序号由各平面的位拼起来（平面 0 是最低位）。
            let palette = small_palette(header, version, bits * planes);
            let mask = (1usize << bits) - 1;
            for y in 0..h {
                let scan = &pixels[y * line..(y + 1) * line];
                for x in 0..w {
                    let bit = x * bits;
                    let mut index = 0;
                    for p in 0..planes {
                        let byte = scan[p * bytes_per_line + bit / 8];
                        let v = (usize::from(byte) >> (8 - bits - bit % 8)) & mask;
                        index |= v << (p * bits);
                    }
                    let [r, g, b] = palette[index];
                    rgba[(y * w + x) * 4..][..4].copy_from_slice(&[r, g, b, 255]);
                }
            }
            let name = match bits * planes {
                1 => "Indexed1",
                2 => "Indexed2",
                3 => "Indexed3",
                _ => "Indexed4",
            };
            (name, false)
        }
    };
    Ok(Raster {
        width,
        height,
        rgba,
        has_alpha,
        format: "PCX".to_string(),
        pixel_format: pixel_format.to_string(),
        icc_profile: None,
    })
}

/// 解 RLE：最高两位都是 1 的字节表示重复（低 6 位是次数，下一个字节是值），其余字节就是值本身。
/// 解够 `total` 字节就停（文件末尾的调色板不会被当成图像）；数据不够时其余补 0。
fn unpack_rle(src: &[u8], total: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(total);
    let mut i = 0;
    while out.len() < total && i < src.len() {
        let b = src[i];
        i += 1;
        if b & 0xC0 == 0xC0 {
            let count = usize::from(b & 0x3F);
            let Some(&v) = src.get(i) else {
                break;
            };
            i += 1;
            let n = count.min(total - out.len());
            out.resize(out.len() + n, v);
        } else {
            out.push(b);
        }
    }
    out.resize(total, 0);
    out
}

/// 8 位 1 平面的调色板：文件末尾 769 字节（0x0C 加 256 × RGB）；没有时按灰度。
fn vga_palette(data: &[u8], version: u8) -> Vec<[u8; 3]> {
    if version >= 5
        && data.len() >= 128 + 769
        && let Some(tail) = data.get(data.len() - 769..)
        && tail[0] == 0x0C
    {
        return tail[1..].as_chunks::<3>().0.to_vec();
    }
    (0..=255u8).map(|v| [v, v, v]).collect()
}

/// 1–4 位索引色的调色板（16 项）：1 位黑白；其余用文件头里的，版本 3 或全是 0 时用标准 EGA。
fn small_palette(header: &[u8], version: u8, bits: usize) -> [[u8; 3]; 16] {
    let mut palette = [[0u8; 3]; 16];
    if bits == 1 {
        palette[1] = [255, 255, 255];
        return palette;
    }
    let stored = &header[16..64];
    if version == 3 || stored.iter().all(|&b| b == 0) {
        return EGA;
    }
    for (entry, c) in palette.iter_mut().zip(stored.as_chunks::<3>().0) {
        *entry = *c;
    }
    palette
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手写的 PCX：`spec` 是每平面位数、平面数、宽、高、每平面一行的字节数；`scanlines` 已经按平面拼好
    /// （每行 `planes × bpl` 字节），按 RLE 或不压缩写出。
    fn pcx(spec: (u8, u8, u16, u16, u16), scanlines: &[u8], rle: bool, tail: &[u8]) -> Vec<u8> {
        let (bits, planes, w, h, bpl) = spec;
        let mut out = vec![0u8; 128];
        out[0] = 0x0A;
        out[1] = 5;
        out[2] = u8::from(rle);
        out[3] = bits;
        out[8..10].copy_from_slice(&(w - 1).to_le_bytes());
        out[10..12].copy_from_slice(&(h - 1).to_le_bytes());
        out[65] = planes;
        out[66..68].copy_from_slice(&bpl.to_le_bytes());
        out[68] = 1;
        for &b in scanlines {
            if rle && b >= 0xC0 {
                out.extend_from_slice(&[0xC1, b]);
            } else {
                out.push(b);
            }
        }
        out.extend_from_slice(tail);
        out
    }

    fn pixel(raster: &Raster, i: usize) -> [u8; 4] {
        raster.rgba[i * 4..i * 4 + 4].try_into().unwrap()
    }

    #[test]
    fn rle_runs_and_literals() {
        assert_eq!(unpack_rle(&[0xC3, 7, 5, 0xC1, 0xC5], 5), [7, 7, 7, 5, 0xC5]);
        // 解够就停；数据不够补 0。
        assert_eq!(unpack_rle(&[0xC5, 1, 9, 9], 3), [1, 1, 1]);
        assert_eq!(unpack_rle(&[0xC2, 4], 4), [4, 4, 0, 0]);
        assert_eq!(unpack_rle(&[0xC2], 2), [0, 0]);
    }

    #[test]
    fn mono_ega_and_vga() {
        // 黑白：10 像素宽，每行 2 字节；第一个像素白，其余黑。
        let raster = decode(&pcx((1, 1, 10, 1, 2), &[0x80, 0x00], true, &[])).unwrap();
        assert_eq!(pixel(&raster, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&raster, 9), [0, 0, 0, 255]);
        assert_eq!(raster.pixel_format, "Indexed1");
        // EGA：1 位 4 平面，像素 0 的序号是 0b1001 = 9（平面 0 和 3），像素 1 是 0b0110 = 6。调色板全 0 → 标准 EGA。
        let scan = [0x80, 0x00, 0x40, 0x00, 0x40, 0x00, 0x80, 0x00];
        let raster = decode(&pcx((1, 4, 2, 1, 2), &scan, false, &[])).unwrap();
        assert_eq!(pixel(&raster, 0), [0x55, 0x55, 0xFF, 255]);
        assert_eq!(pixel(&raster, 1), [0xAA, 0x55, 0x00, 255]);
        assert_eq!(raster.pixel_format, "Indexed4");
        // 4 位 1 平面：高 4 位是第一个像素。文件头调色板第 1、2 项。
        let mut file = pcx((4, 1, 2, 1, 2), &[0x12, 0x00], true, &[]);
        file[16 + 3..16 + 9].copy_from_slice(&[10, 20, 30, 40, 50, 60]);
        let raster = decode(&file).unwrap();
        assert_eq!(pixel(&raster, 0), [10, 20, 30, 255]);
        assert_eq!(pixel(&raster, 1), [40, 50, 60, 255]);
        // 8 位：末尾的 256 色调色板。
        let mut tail = vec![0x0C];
        for i in 0..=255u8 {
            tail.extend_from_slice(&[i, 255 - i, 7]);
        }
        let raster = decode(&pcx((8, 1, 2, 1, 2), &[3, 200], true, &tail)).unwrap();
        assert_eq!(pixel(&raster, 0), [3, 252, 7, 255]);
        assert_eq!(pixel(&raster, 1), [200, 55, 7, 255]);
        // 没有调色板：按灰度。
        let raster = decode(&pcx((8, 1, 2, 1, 2), &[3, 200], true, &[])).unwrap();
        assert_eq!(pixel(&raster, 1), [200, 200, 200, 255]);
    }

    #[test]
    fn true_colour_with_padding() {
        // 3x1 的 RGB，每平面一行补齐到 4 字节。
        let scan = [1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0];
        let raster = decode(&pcx((8, 3, 3, 1, 4), &scan, true, &[])).unwrap();
        assert_eq!(pixel(&raster, 0), [1, 4, 7, 255]);
        assert_eq!(pixel(&raster, 2), [3, 6, 9, 255]);
        assert_eq!(raster.pixel_format, "RGB24");
        // RGBA，第二个像素半透明。
        let scan = [1, 2, 3, 4, 5, 6, 255, 128];
        let raster = decode(&pcx((8, 4, 2, 1, 2), &scan, false, &[])).unwrap();
        assert_eq!(pixel(&raster, 1), [2, 4, 6, 128]);
        assert!(raster.has_alpha);
    }

    #[test]
    fn bad_files_are_errors_not_panics() {
        let good = pcx((8, 3, 3, 2, 4), &[9; 24], true, &[]);
        assert!(decode(&good).is_ok());
        for len in 0..good.len() {
            let _ = decode(&good[..len]);
        }
        assert!(decode(b"not a pcx").is_err());
        let mut bad = good.clone();
        bad[3] = 3; // 每平面 3 位
        assert!(decode(&bad).is_err());
        let mut bad = good.clone();
        bad[66..68].copy_from_slice(&1u16.to_le_bytes()); // 每行太短
        assert!(decode(&bad).is_err());
        let mut bad = good.clone();
        bad[66..68].copy_from_slice(&60000u16.to_le_bytes()); // 补齐太多
        assert!(decode(&bad).is_err());
        let mut bad = good.clone();
        bad[8..10].copy_from_slice(&0u16.to_le_bytes());
        bad[4..6].copy_from_slice(&5u16.to_le_bytes()); // Xmax < Xmin
        assert!(decode(&bad).is_err());
    }
}
