//! DDS：DirectDraw Surface 纹理，显示第一张（立方体贴图、数组的第一面，最大的 mip）。
//!
//! 支持：
//! - 旧式像素格式：按位掩码的 RGB、RGBA（32、24、16 位，含 565、1555、4444）、亮度（L8、L16、A8L8）、只有 Alpha，
//!   8 位索引色。亮度、Alpha 的掩码放不下时按这种位数通常的布局读。
//! - FourCC：DXT1–DXT5（DXT2、DXT4 是预乘的，还原）、ATI1/BC4U/BC4S、ATI2/BC5U/BC5S，以及 D3DFMT 编号
//!   36（RGBA16）、111（R16F）、113（RGBA16F）、114（R32F）、116（RGBA32F）。
//! - DX10 扩展头里常见的 DXGI 格式：R8G8B8A8、B8G8R8A8、B8G8R8X8（含 sRGB、TYPELESS）、R10G10B10A2、R11G11B10_FLOAT、
//!   R16G16B16A16（UNORM、FLOAT）、R32G32B32A32_FLOAT、R8G8、R8、R16、R16_FLOAT、R32_FLOAT、A8、B5G6R5、B5G5R5A1、
//!   B4G4R4A4、BC1–BC7（含 sRGB、SNORM、TYPELESS）。
//!
//! 块解码用 bcdec_rs。显示约定：单通道（BC4、R8、R16、亮度）按灰度；只有 Alpha 的按灰度显示 Alpha 值（不透明）；
//! BC5 和 R8G8 是红、绿两个通道，蓝为 0（不假定是法线贴图）；有符号的值 −1–1 映射到 0–255，没有的蓝色也按 0 映射（128）。
//! 浮点格式（BC6H、16、32 位浮点、R11G11B10）是线性的：截到 0–1 后按 sRGB 编码（HDR 映射到普通亮度）。
//! 8 位的 UNORM 格式原样显示（与常见看图软件相同，不当线性值处理）。

use crate::i18n::Message;
use crate::decode::Raster;
use crate::psd::linear_to_srgb8;

const TRUNCATED: Message = msg!("DDS 文件不完整", "The DDS file is incomplete");

/// 文件头之后图像数据的位置（"DDS " + 124 字节文件头；DX10 再加 20 字节）。
const HEADER_END: usize = 128;
const DX10_END: usize = 148;
/// 8 位索引色的调色板：256 色，每色 4 字节。
const PALETTE_BYTES: usize = 1024;

// 像素格式的标志。
const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_ALPHA: u32 = 0x2;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_PALETTEINDEXED8: u32 = 0x20;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x20000;

/// 块压缩格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc6h,
    Bc7,
}

/// 浮点格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Float {
    Rgba16,
    Rgba32,
    R16,
    R32,
    R11G11B10,
}

/// 按位掩码存放的像素怎么解释。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaskKind {
    Rgb,
    Luminance,
    AlphaOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Masks {
    bits: u32,
    r: u32,
    g: u32,
    b: u32,
    /// 不用 Alpha 时为 0。
    a: u32,
    kind: MaskKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Block {
        kind: Block,
        signed: bool,
        premultiplied: bool,
    },
    Masked(Masks),
    /// R16G16B16A16_UNORM（D3DFMT 36）。
    Unorm16x4,
    Float(Float),
    /// 8 位索引色（D3DFMT_P8）：文件头后面是 256 色的调色板（每色 R、G、B 和一个字节的 Alpha 或标志）。
    Palette8,
}

impl Format {
    fn block(kind: Block) -> Self {
        Format::Block {
            kind,
            signed: false,
            premultiplied: false,
        }
    }

    fn signed(kind: Block) -> Self {
        Format::Block {
            kind,
            signed: true,
            premultiplied: false,
        }
    }

    fn premultiplied(kind: Block) -> Self {
        Format::Block {
            kind,
            signed: false,
            premultiplied: true,
        }
    }

    fn masked(bits: u32, r: u32, g: u32, b: u32, a: u32, kind: MaskKind) -> Self {
        Format::Masked(Masks {
            bits,
            r,
            g,
            b,
            a,
            kind,
        })
    }

    /// 像素格式。
    fn pixel_format(self) -> String {
        match self {
            Format::Block { kind, .. } => match kind {
                Block::Bc1 => "BC1",
                Block::Bc2 => "BC2",
                Block::Bc3 => "BC3",
                Block::Bc4 => "BC4",
                Block::Bc5 => "BC5",
                Block::Bc6h => "BC6H",
                Block::Bc7 => "BC7",
            }
            .to_string(),
            Format::Masked(m) => {
                let name = match m.kind {
                    MaskKind::AlphaOnly => "Alpha",
                    MaskKind::Luminance => "Gray",
                    MaskKind::Rgb if m.b == 0 => "RG",
                    MaskKind::Rgb => "RGB",
                };
                let alpha = if m.a != 0 && m.kind != MaskKind::AlphaOnly {
                    "A"
                } else {
                    ""
                };
                format!("{name}{alpha}{}", m.bits)
            }
            Format::Unorm16x4 => "RGBA64".to_string(),
            Format::Float(f) => match f {
                Float::Rgba16 => "RGBA64F",
                Float::Rgba32 => "RGBA128F",
                Float::R16 => "Gray16F",
                Float::R32 => "Gray32F",
                Float::R11G11B10 => "RGB32F",
            }
            .to_string(),
            Format::Palette8 => "P8".to_string(),
        }
    }
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, Message> {
    let bytes = data.get(at..at + 4).ok_or(TRUNCATED)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// 解码整个文件。格式不对、数据坏了时返回错误说明（不 panic）。
pub fn decode(data: &[u8]) -> Result<Raster, Message> {
    if data.get(..4) != Some(b"DDS ") {
        return Err(msg!("不是 DDS 文件", "Not a DDS file"));
    }
    if data.len() < HEADER_END {
        return Err(TRUNCATED);
    }
    let height = u32_at(data, 12)?;
    let width = u32_at(data, 16)?;
    let (format, start) = pixel_format(data)?;
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 {
        return Err(msg!("DDS 的尺寸不对：{width}x{height}", "Invalid DDS size: {width}x{height}", width = format!("{width}"), height = format!("{height}")));
    }
    let src = data.get(start..).ok_or(TRUNCATED)?;
    // 先看数据够不够，再分配（坏文件不会让我们分配一大块内存）。
    let need = match format {
        Format::Block { kind, .. } => {
            let block_size = match kind {
                Block::Bc1 | Block::Bc4 => 8,
                _ => 16,
            };
            w.div_ceil(4)
                .checked_mul(h.div_ceil(4))
                .and_then(|n| n.checked_mul(block_size))
        }
        Format::Masked(m) => w
            .checked_mul(h)
            .and_then(|n| n.checked_mul(m.bits as usize / 8)),
        Format::Unorm16x4 => w.checked_mul(h).and_then(|n| n.checked_mul(8)),
        Format::Float(f) => w.checked_mul(h).and_then(|n| n.checked_mul(float_bytes(f))),
        Format::Palette8 => w.checked_mul(h),
    }
    .ok_or(TRUNCATED)?;
    let src = src.get(..need).ok_or(TRUNCATED)?;
    let mut rgba = Raster::alloc(width, height)?;
    match format {
        Format::Block { kind, signed, .. } => decode_blocks(src, w, h, kind, signed, &mut rgba),
        Format::Masked(m) => decode_masked(src, m, &mut rgba),
        Format::Unorm16x4 => {
            for (px, s) in rgba
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<8>().0)
            {
                for (c, pair) in px.iter_mut().zip(s.as_chunks::<2>().0) {
                    *c = from16(u16::from_le_bytes(*pair));
                }
            }
        }
        Format::Float(f) => decode_float(src, f, &mut rgba),
        Format::Palette8 => {
            let palette = data
                .get(HEADER_END..HEADER_END + PALETTE_BYTES)
                .ok_or(TRUNCATED)?;
            // 第四个字节在 D3D9 里是标志：全是 0 时不当 Alpha（整张图都会变透明）。
            let use_alpha = palette.iter().skip(3).step_by(4).any(|&a| a != 0);
            for (px, &i) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                let entry = &palette[usize::from(i) * 4..usize::from(i) * 4 + 4];
                *px = [
                    entry[0],
                    entry[1],
                    entry[2],
                    if use_alpha { entry[3] } else { 255 },
                ];
            }
        }
    }
    if let Format::Block {
        premultiplied: true,
        ..
    } = format
    {
        unpremultiply(&mut rgba);
    }
    let has_alpha = rgba.iter().skip(3).step_by(4).any(|&a| a < 255);
    Ok(Raster {
        width,
        height,
        rgba,
        has_alpha,
        format: "DDS".to_string(),
        pixel_format: format.pixel_format(),
        icc_profile: None,
    })
}

/// 读像素格式；返回格式和图像数据的开头。
fn pixel_format(data: &[u8]) -> Result<(Format, usize), Message> {
    let flags = u32_at(data, 80)?;
    let fourcc: [u8; 4] = data[84..88].try_into().map_err(|_| TRUNCATED)?;
    if flags & DDPF_FOURCC != 0 {
        if &fourcc == b"DX10" {
            let dxgi = u32_at(data, HEADER_END)?;
            return Ok((from_dxgi(dxgi)?, DX10_END));
        }
        return Ok((from_fourcc(fourcc)?, HEADER_END));
    }
    if flags & DDPF_PALETTEINDEXED8 != 0 {
        // 位数等字段常常没填对（如 Pillow 的测试图写 24），只看标志。
        return Ok((Format::Palette8, HEADER_END + PALETTE_BYTES));
    }
    let bits = u32_at(data, 88)?;
    let r = u32_at(data, 92)?;
    let g = u32_at(data, 96)?;
    let b = u32_at(data, 100)?;
    let a = u32_at(data, 104)?;
    let kind = if flags & DDPF_LUMINANCE != 0 {
        MaskKind::Luminance
    } else if flags & DDPF_RGB != 0 {
        MaskKind::Rgb
    } else if flags & DDPF_ALPHA != 0 {
        MaskKind::AlphaOnly
    } else {
        return Err(msg!("不支持的 DDS 像素格式（标志 0x{flags}）", "Unsupported DDS pixel format (flags 0x{flags})", flags = format!("{flags:X}")));
    };
    if !matches!(bits, 8 | 16 | 24 | 32) {
        return Err(msg!("不支持的 DDS 像素位数：{bits}", "Unsupported DDS bit depth: {bits}", bits = format!("{bits}")));
    }
    let mut alpha = if flags & DDPF_ALPHAPIXELS != 0 || kind == MaskKind::AlphaOnly {
        a
    } else {
        0
    };
    let limit = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };
    let (mut r, mut g, mut b) = (r, g, b);
    // 有的程序在 8、16 位的亮度、Alpha 格式里填了放不下的掩码（如 Pillow 测试图 uncompressed_l.dds 的
    // 0xFF000000）：这时按这种位数通常的布局读（L8、A8、A8L8、L16），与 Pillow 相同。
    if (r | g | b | alpha) & !limit != 0 && kind != MaskKind::Rgb {
        (r, alpha) = match (bits, alpha != 0, kind) {
            (8, _, MaskKind::AlphaOnly) => (0, 0xFF),
            (8, false, _) => (0xFF, 0),
            (16, true, _) => (0xFF, 0xFF00),
            (16, false, _) => (0xFFFF, 0),
            _ => (r, alpha),
        };
        (g, b) = (r, r);
    }
    if (r | g | b | alpha) & !limit != 0 {
        return Err(msg!("DDS 的颜色掩码超出了像素位数", "The DDS color masks exceed the pixel size"));
    }
    Ok((Format::masked(bits, r, g, b, alpha, kind), HEADER_END))
}

fn from_fourcc(fourcc: [u8; 4]) -> Result<Format, Message> {
    Ok(match &fourcc {
        b"DXT1" => Format::block(Block::Bc1),
        b"DXT2" => Format::premultiplied(Block::Bc2),
        b"DXT3" => Format::block(Block::Bc2),
        b"DXT4" => Format::premultiplied(Block::Bc3),
        b"DXT5" => Format::block(Block::Bc3),
        b"ATI1" | b"BC4U" => Format::block(Block::Bc4),
        b"BC4S" => Format::signed(Block::Bc4),
        b"ATI2" | b"BC5U" => Format::block(Block::Bc5),
        b"BC5S" => Format::signed(Block::Bc5),
        _ => match u32::from_le_bytes(fourcc) {
            36 => Format::Unorm16x4,
            111 => Format::Float(Float::R16),
            113 => Format::Float(Float::Rgba16),
            114 => Format::Float(Float::R32),
            116 => Format::Float(Float::Rgba32),
            n => {
                let name = if fourcc.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
                    String::from_utf8_lossy(&fourcc).into_owned()
                } else {
                    n.to_string()
                };
                return Err(msg!("不支持的 DDS 格式：{name}", "Unsupported DDS format: {name}", name = name));
            }
        },
    })
}

fn from_dxgi(dxgi: u32) -> Result<Format, Message> {
    use MaskKind::{AlphaOnly, Luminance, Rgb};
    Ok(match dxgi {
        2 => Format::Float(Float::Rgba32),
        10 => Format::Float(Float::Rgba16),
        9 | 11 => Format::Unorm16x4,
        23 | 24 => Format::masked(32, 0x3FF, 0xF_FC00, 0x3FF0_0000, 0xC000_0000, Rgb),
        26 => Format::Float(Float::R11G11B10),
        27..=29 => Format::masked(32, 0xFF, 0xFF00, 0xFF_0000, 0xFF00_0000, Rgb),
        41 => Format::Float(Float::R32),
        48 | 49 => Format::masked(16, 0xFF, 0xFF00, 0, 0, Rgb),
        54 => Format::Float(Float::R16),
        53 | 56 => Format::masked(16, 0xFFFF, 0, 0, 0, Luminance),
        60 | 61 => Format::masked(8, 0xFF, 0, 0, 0, Luminance),
        65 => Format::masked(8, 0, 0, 0, 0xFF, AlphaOnly),
        70..=72 => Format::block(Block::Bc1),
        73..=75 => Format::block(Block::Bc2),
        76..=78 => Format::block(Block::Bc3),
        79 | 80 => Format::block(Block::Bc4),
        81 => Format::signed(Block::Bc4),
        82 | 83 => Format::block(Block::Bc5),
        84 => Format::signed(Block::Bc5),
        85 => Format::masked(16, 0xF800, 0x07E0, 0x001F, 0, Rgb),
        86 => Format::masked(16, 0x7C00, 0x03E0, 0x001F, 0x8000, Rgb),
        87 | 90 | 91 => Format::masked(32, 0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000, Rgb),
        88 | 92 | 93 => Format::masked(32, 0xFF_0000, 0xFF00, 0xFF, 0, Rgb),
        94 | 95 => Format::block(Block::Bc6h),
        96 => Format::signed(Block::Bc6h),
        97..=99 => Format::block(Block::Bc7),
        115 => Format::masked(16, 0x0F00, 0x00F0, 0x000F, 0xF000, Rgb),
        n => return Err(msg!("不支持的 DDS 格式（DXGI {n}）", "Unsupported DDS format (DXGI {n})", n = format!("{n}"))),
    })
}

fn float_bytes(f: Float) -> usize {
    match f {
        Float::Rgba16 => 8,
        Float::Rgba32 => 16,
        Float::R16 => 2,
        Float::R32 | Float::R11G11B10 => 4,
    }
}

/// 块压缩：每 4x4 一块，边上不满 4 的块只取图片里的部分。
fn decode_blocks(src: &[u8], w: usize, h: usize, kind: Block, signed: bool, out: &mut [u8]) {
    let block_size = match kind {
        Block::Bc1 | Block::Bc4 => 8,
        _ => 16,
    };
    let blocks_wide = w.div_ceil(4);
    let mut tile = [0u8; 64];
    for (i, block) in src.chunks_exact(block_size).enumerate() {
        let bx = i % blocks_wide;
        let by = i / blocks_wide;
        decode_block(block, kind, signed, &mut tile);
        for py in 0..4 {
            let y = by * 4 + py;
            if y >= h {
                break;
            }
            for px in 0..4 {
                let x = bx * 4 + px;
                if x >= w {
                    break;
                }
                let from = (py * 4 + px) * 4;
                let to = (y * w + x) * 4;
                out[to..to + 4].copy_from_slice(&tile[from..from + 4]);
            }
        }
    }
}

/// 解一块到 4x4 的 RGBA。
fn decode_block(block: &[u8], kind: Block, signed: bool, tile: &mut [u8; 64]) {
    match kind {
        Block::Bc1 => bcdec_rs::bc1(block, tile, 16),
        Block::Bc2 => bcdec_rs::bc2(block, tile, 16),
        Block::Bc3 => bcdec_rs::bc3(block, tile, 16),
        Block::Bc7 => bcdec_rs::bc7(block, tile, 16),
        Block::Bc4 => {
            let mut red = [0u8; 16];
            bcdec_rs::bc4(block, &mut red, 4, signed);
            for (px, &v) in tile.as_chunks_mut::<4>().0.iter_mut().zip(&red) {
                let v = unit(v, signed);
                px.copy_from_slice(&[v, v, v, 255]);
            }
        }
        Block::Bc5 => {
            let mut rg = [0u8; 32];
            bcdec_rs::bc5(block, &mut rg, 8, signed);
            for (px, pair) in tile
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(rg.as_chunks::<2>().0)
            {
                // 没有蓝色：取"0"，有符号时是 −1–1 的中间（128），与 Pillow 相同。
                px.copy_from_slice(&[
                    unit(pair[0], signed),
                    unit(pair[1], signed),
                    unit(0, signed),
                    255,
                ]);
            }
        }
        Block::Bc6h => {
            let mut rgb = [0f32; 48];
            bcdec_rs::bc6h_float(block, &mut rgb, 12, signed);
            for (px, c) in tile
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(rgb.as_chunks::<3>().0)
            {
                px.copy_from_slice(&[
                    linear_to_srgb8(c[0]),
                    linear_to_srgb8(c[1]),
                    linear_to_srgb8(c[2]),
                    255,
                ]);
            }
        }
    }
}

/// 有符号的 8 位值（−127–127，补码存放）映射到 0–255；无符号的原样。
fn unit(v: u8, signed: bool) -> u8 {
    if !signed {
        return v;
    }
    let s = i32::from(v as i8).max(-127);
    (((s + 127) * 255 + 127) / 254) as u8
}

/// 一个颜色通道的掩码：位置和最大值。
#[derive(Clone, Copy)]
struct Channel {
    mask: u32,
    shift: u32,
    max: u64,
}

impl Channel {
    fn new(mask: u32) -> Self {
        let shift = if mask == 0 { 0 } else { mask.trailing_zeros() };
        Self {
            mask,
            shift,
            max: u64::from(mask >> shift),
        }
    }

    /// 按掩码取出这个通道，换算到 0–255。
    fn get(self, px: u32) -> u8 {
        if self.max == 0 {
            return 0;
        }
        let v = u64::from((px & self.mask) >> self.shift);
        ((v * 255 + self.max / 2) / self.max).min(255) as u8
    }
}

fn decode_masked(src: &[u8], m: Masks, out: &mut [u8]) {
    let bytes = m.bits as usize / 8;
    let (r, g, b, a) = (
        Channel::new(m.r),
        Channel::new(m.g),
        Channel::new(m.b),
        Channel::new(m.a),
    );
    for (px, s) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src.chunks_exact(bytes))
    {
        let mut word = [0u8; 4];
        word[..bytes].copy_from_slice(s);
        let v = u32::from_le_bytes(word);
        let rgba = match m.kind {
            MaskKind::Rgb => [
                r.get(v),
                g.get(v),
                b.get(v),
                if m.a == 0 { 255 } else { a.get(v) },
            ],
            MaskKind::Luminance => {
                let l = r.get(v);
                [l, l, l, if m.a == 0 { 255 } else { a.get(v) }]
            }
            MaskKind::AlphaOnly => {
                let l = a.get(v);
                [l, l, l, 255]
            }
        };
        px.copy_from_slice(&rgba);
    }
}

fn decode_float(src: &[u8], f: Float, out: &mut [u8]) {
    let bytes = float_bytes(f);
    for (px, s) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src.chunks_exact(bytes))
    {
        let half = |i: usize| half_to_f32(u16::from_le_bytes([s[i], s[i + 1]]));
        let single = |i: usize| f32::from_le_bytes([s[i], s[i + 1], s[i + 2], s[i + 3]]);
        let [r, g, b, a] = match f {
            Float::Rgba16 => [half(0), half(2), half(4), half(6)],
            Float::Rgba32 => [single(0), single(4), single(8), single(12)],
            Float::R16 => {
                let v = half(0);
                [v, v, v, 1.0]
            }
            Float::R32 => {
                let v = single(0);
                [v, v, v, 1.0]
            }
            Float::R11G11B10 => {
                let v = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                [
                    small_float(v & 0x7FF, 6),
                    small_float((v >> 11) & 0x7FF, 6),
                    small_float(v >> 22, 5),
                    1.0,
                ]
            }
        };
        px.copy_from_slice(&[
            linear_to_srgb8(r),
            linear_to_srgb8(g),
            linear_to_srgb8(b),
            alpha8(a),
        ]);
    }
}

/// 透明度（0–1，不做 sRGB 编码）。
fn alpha8(a: f32) -> u8 {
    if a >= 1.0 {
        255
    } else if a > 0.0 {
        (a * 255.0).round() as u8
    } else {
        0
    }
}

/// 半精度浮点 → f32。
fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1F);
    let man = f32::from(h & 0x3FF);
    let v = match exp {
        0 => man * 2f32.powi(-24),
        31 if man == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    };
    sign * v
}

/// R11G11B10 里没有符号位的小浮点数（5 位指数，`mantissa_bits` 位尾数）→ f32。
fn small_float(v: u32, mantissa_bits: u32) -> f32 {
    let scale = (1u32 << mantissa_bits) as f32;
    let man = (v & ((1 << mantissa_bits) - 1)) as f32;
    let exp = ((v >> mantissa_bits) & 0x1F) as i32;
    match exp {
        0 => man / scale * 2f32.powi(-14),
        31 if man == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + man / scale) * 2f32.powi(e - 15),
    }
}

/// 16 位 → 8 位（四舍五入）。
fn from16(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32767) / 65535) as u8
}

/// DXT2、DXT4 存的是预乘过的颜色：还原成不预乘的。
fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0.iter_mut() {
        let a = u32::from(px[3]);
        if a == 0 || a == 255 {
            continue;
        }
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手写的 DDS：`pf` 是像素格式的标志、FourCC、位数和四个掩码；`dxgi` 不为空时加 DX10 扩展头。
    fn dds(
        w: u32,
        h: u32,
        pf: (u32, &[u8; 4], u32, [u32; 4]),
        dxgi: Option<u32>,
        data: &[u8],
    ) -> Vec<u8> {
        let mut out = b"DDS ".to_vec();
        let mut header = [0u32; 31];
        header[0] = 124;
        header[1] = 0x1 | 0x2 | 0x4 | 0x1000;
        header[2] = h;
        header[3] = w;
        header[18] = 32;
        header[19] = pf.0;
        header[20] = u32::from_le_bytes(*pf.1);
        header[21] = pf.2;
        header[22..26].copy_from_slice(&pf.3);
        header[26] = 0x1000;
        for v in header {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if let Some(format) = dxgi {
            for v in [format, 3, 0, 1, 0] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out.extend_from_slice(data);
        out
    }

    fn pixel(raster: &Raster, i: usize) -> [u8; 4] {
        raster.rgba[i * 4..i * 4 + 4].try_into().unwrap()
    }

    #[test]
    fn masked_formats_by_bit_masks() {
        // A8R8G8B8，2x1。
        let pf = (
            DDPF_RGB | DDPF_ALPHAPIXELS,
            &[0; 4],
            32,
            [0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000],
        );
        let raster = decode(&dds(2, 1, pf, None, &[3, 2, 1, 255, 30, 20, 10, 128])).unwrap();
        assert_eq!(pixel(&raster, 0), [1, 2, 3, 255]);
        assert_eq!(pixel(&raster, 1), [10, 20, 30, 128]);
        assert_eq!(
            (raster.pixel_format.as_str(), raster.has_alpha),
            ("RGBA32", true)
        );
        // R5G6B5：纯红、纯绿。
        let pf = (DDPF_RGB, &[0; 4], 16, [0xF800, 0x07E0, 0x001F, 0]);
        let data = [0x00, 0xF8, 0xE0, 0x07];
        let raster = decode(&dds(2, 1, pf, None, &data)).unwrap();
        assert_eq!(pixel(&raster, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&raster, 1), [0, 255, 0, 255]);
        assert_eq!(raster.pixel_format, "RGB16");
        // A8L8：亮度和 Alpha。
        let pf = (
            DDPF_LUMINANCE | DDPF_ALPHAPIXELS,
            &[0; 4],
            16,
            [0xFF, 0, 0, 0xFF00],
        );
        let raster = decode(&dds(1, 1, pf, None, &[90, 200])).unwrap();
        assert_eq!(pixel(&raster, 0), [90, 90, 90, 200]);
        assert_eq!(raster.pixel_format, "GrayA16");
        // 只有 Alpha：按灰度显示。
        let pf = (DDPF_ALPHA, &[0; 4], 8, [0, 0, 0, 0xFF]);
        let raster = decode(&dds(1, 1, pf, None, &[77])).unwrap();
        assert_eq!(pixel(&raster, 0), [77, 77, 77, 255]);
        // 24 位 RGB。
        let pf = (DDPF_RGB, &[0; 4], 24, [0xFF_0000, 0xFF00, 0xFF, 0]);
        let raster = decode(&dds(1, 1, pf, None, &[7, 8, 9])).unwrap();
        assert_eq!(pixel(&raster, 0), [9, 8, 7, 255]);
    }

    #[test]
    fn block_formats_cover_partial_blocks() {
        // 6x5 的 DXT1：2x2 块，边上只取图片里的部分。每块是纯色 565（c0 == c1，序号 0）。
        let red = 0xF800u16.to_le_bytes();
        let blue = 0x001Fu16.to_le_bytes();
        let mut data = Vec::new();
        for (i, c) in [red, blue, blue, red].iter().enumerate() {
            let _ = i;
            data.extend_from_slice(&[c[0], c[1], c[0], c[1], 0, 0, 0, 0]);
        }
        let pf = (DDPF_FOURCC, b"DXT1", 0, [0; 4]);
        let raster = decode(&dds(6, 5, pf, None, &data)).unwrap();
        assert_eq!((raster.width, raster.height), (6, 5));
        assert_eq!(pixel(&raster, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&raster, 5), [0, 0, 255, 255]); // (5, 0)：右上块
        assert_eq!(pixel(&raster, 4 * 6), [0, 0, 255, 255]); // (0, 4)：左下块
        assert_eq!(pixel(&raster, 4 * 6 + 5), [255, 0, 0, 255]);
        assert_eq!(raster.pixel_format, "BC1");
        assert!(!raster.has_alpha);
        // BC4：单通道按灰度；有符号的 BC4S：−127 → 0，127 → 255。
        let bc4 = [200, 200, 0, 0, 0, 0, 0, 0];
        let raster = decode(&dds(4, 4, (DDPF_FOURCC, b"ATI1", 0, [0; 4]), None, &bc4)).unwrap();
        assert_eq!(pixel(&raster, 15), [200, 200, 200, 255]);
        let bc4s = [127, 127, 0, 0, 0, 0, 0, 0];
        let raster = decode(&dds(4, 4, (DDPF_FOURCC, b"BC4S", 0, [0; 4]), None, &bc4s)).unwrap();
        assert_eq!(pixel(&raster, 0), [255, 255, 255, 255]);
        // BC5（DXGI 83）：红、绿两个通道，蓝为 0。
        let bc5 = [10, 10, 0, 0, 0, 0, 0, 0, 250, 250, 0, 0, 0, 0, 0, 0];
        let raster = decode(&dds(
            4,
            4,
            (DDPF_FOURCC, b"DX10", 0, [0; 4]),
            Some(83),
            &bc5,
        ))
        .unwrap();
        assert_eq!(pixel(&raster, 3), [10, 250, 0, 255]);
        assert_eq!(raster.pixel_format, "BC5");
        // DXT2：预乘的颜色还原。Alpha 128，存的颜色是 64 → 还原成约 128。
        let mut dxt2 = [0x88u8; 8].to_vec(); // 4 位 Alpha 全是 8（×17 = 136）
        let c = (8u16 << 11).to_le_bytes(); // R5 = 8 → 66
        dxt2.extend_from_slice(&[c[0], c[1], c[0], c[1], 0, 0, 0, 0]);
        let raster = decode(&dds(4, 4, (DDPF_FOURCC, b"DXT2", 0, [0; 4]), None, &dxt2)).unwrap();
        let [r, g, b, a] = pixel(&raster, 0);
        assert_eq!((g, b, a), (0, 0, 136));
        assert!(r.abs_diff(124) <= 1, "{r}");
        assert!(raster.has_alpha);
    }

    #[test]
    fn float_formats_are_srgb_encoded() {
        // RGBA16F：0.5 → 188，1.0 → 255，2.0 截到 255；Alpha 0.5 → 128。
        let half = |v: u16| v.to_le_bytes();
        let mut data = Vec::new();
        for v in [0x3800u16, 0x3C00, 0x4000, 0x3800] {
            data.extend_from_slice(&half(v));
        }
        let raster = decode(&dds(
            1,
            1,
            (DDPF_FOURCC, b"DX10", 0, [0; 4]),
            Some(10),
            &data,
        ))
        .unwrap();
        assert_eq!(pixel(&raster, 0), [188, 255, 255, 128]);
        assert_eq!(raster.pixel_format, "RGBA64F");
        // D3DFMT 116（RGBA32F）。
        let mut data = Vec::new();
        for v in [0.0f32, 0.5, 1.0, 1.0] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let raster = decode(&dds(
            1,
            1,
            (DDPF_FOURCC, &116u32.to_le_bytes(), 0, [0; 4]),
            None,
            &data,
        ))
        .unwrap();
        assert_eq!(pixel(&raster, 0), [0, 188, 255, 255]);
        // R11G11B10：1.0 的位样式（指数 15、尾数 0）。
        let one11 = 15u32 << 6;
        let one10 = 15u32 << 5;
        let v = one11 | (one11 << 11) | (one10 << 22);
        let raster = decode(&dds(
            1,
            1,
            (DDPF_FOURCC, b"DX10", 0, [0; 4]),
            Some(26),
            &v.to_le_bytes(),
        ))
        .unwrap();
        assert_eq!(pixel(&raster, 0), [255, 255, 255, 255]);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert!(half_to_f32(0x7E00).is_nan());
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
    }

    /// 兼容第三方软件写入的特殊文件（参考 Pillow 测试样本）：掩码填错的 L8、8 位索引色、有符号的 BC5。
    #[test]
    fn lenient_masks_palette_and_signed_bc5() {
        // L8，三个掩码都写成 0xFF000000（Pillow 的 uncompressed_l.dds）。
        let pf = (
            DDPF_LUMINANCE,
            &[0; 4],
            8,
            [0xFF00_0000, 0xFF00_0000, 0xFF00_0000, 0],
        );
        let raster = decode(&dds(2, 1, pf, None, &[10, 200])).unwrap();
        assert_eq!(pixel(&raster, 0), [10, 10, 10, 255]);
        assert_eq!(pixel(&raster, 1), [200, 200, 200, 255]);
        // P8：调色板第 1 色是 (1, 2, 3)，第四个字节全是 0 时不当 Alpha。
        let mut data = vec![0u8; PALETTE_BYTES];
        data[4..8].copy_from_slice(&[1, 2, 3, 0]);
        data.extend_from_slice(&[1, 0]);
        let raster = decode(&dds(
            2,
            1,
            (DDPF_PALETTEINDEXED8, &[0; 4], 24, [0; 4]),
            None,
            &data,
        ))
        .unwrap();
        assert_eq!(pixel(&raster, 0), [1, 2, 3, 255]);
        assert_eq!(pixel(&raster, 1), [0, 0, 0, 255]);
        assert_eq!(
            (raster.pixel_format.as_str(), raster.has_alpha),
            ("P8", false)
        );
        // 调色板带 Alpha 时照用。
        data[7] = 128;
        let raster = decode(&dds(
            2,
            1,
            (DDPF_PALETTEINDEXED8, &[0; 4], 8, [0; 4]),
            None,
            &data,
        ))
        .unwrap();
        assert_eq!(pixel(&raster, 0)[3], 128);
        // 有符号的 BC5：红、绿的端点都是 0（−1–1 的中间），蓝色同样是 128。
        let raster = decode(&dds(
            4,
            4,
            (DDPF_FOURCC, b"BC5S", 0, [0; 4]),
            None,
            &[0; 16],
        ))
        .unwrap();
        assert_eq!(pixel(&raster, 0), [128, 128, 128, 255]);
        let raster = decode(&dds(
            4,
            4,
            (DDPF_FOURCC, b"BC5U", 0, [0; 4]),
            None,
            &[0; 16],
        ))
        .unwrap();
        assert_eq!(pixel(&raster, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn bad_files_are_errors_not_panics() {
        let pf = (DDPF_FOURCC, b"DXT1", 0, [0; 4]);
        let good = dds(8, 8, pf, None, &[0x55; 32]);
        assert!(decode(&good).is_ok());
        for len in 0..good.len() {
            let _ = decode(&good[..len]);
        }
        assert!(
            decode(&good[..good.len() - 1])
                .unwrap_err()
                .text(crate::i18n::Lang::ZhHans)
                .contains("不完整")
        );
        assert!(decode(b"DDS ").is_err());
        assert!(decode(b"PNG not dds").is_err());
        let err = decode(&dds(
            4,
            4,
            (DDPF_FOURCC, b"ZZZZ", 0, [0; 4]),
            None,
            &[0; 64],
        ))
        .unwrap_err();
        assert!(err.to_string().contains("ZZZZ"), "{err}");
        let err = decode(&dds(
            4,
            4,
            (DDPF_FOURCC, b"DX10", 0, [0; 4]),
            Some(1234),
            &[0; 64],
        ))
        .unwrap_err();
        assert!(err.to_string().contains("1234"), "{err}");
        // 尺寸大得离谱、数据很少：报不完整，不去分配内存。
        let huge = dds(1 << 30, 1 << 30, pf, None, &[0; 64]);
        assert!(decode(&huge).is_err());
        // 掩码超出位数。
        let pf = (DDPF_RGB, &[0; 4], 16, [0xFF_0000, 0, 0, 0]);
        assert!(decode(&dds(1, 1, pf, None, &[0, 0])).is_err());
        // 随便乱写的块：BC7、BC6H 的保留模式不出错。
        for dxgi in [95, 96, 98] {
            let noise: Vec<u8> = (0..16u32).map(|i| (i * 37 + 11) as u8).collect();
            let raster = decode(&dds(
                4,
                4,
                (DDPF_FOURCC, b"DX10", 0, [0; 4]),
                Some(dxgi),
                &noise,
            ));
            assert!(raster.is_ok());
            let reserved = [0u8; 16];
            assert!(
                decode(&dds(
                    4,
                    4,
                    (DDPF_FOURCC, b"DX10", 0, [0; 4]),
                    Some(dxgi),
                    &reserved
                ))
                .is_ok()
            );
        }
    }
}
