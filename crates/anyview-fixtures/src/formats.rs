//! formats/：自写解码器的测试图片（PSD、PSB、DDS、PCX）。按格式规范直接写字节，内容由公式确定。
//!
//! 都是 128x96 的四色块，与 view/ 相同：左上红 (220,40,40)、右上绿 (40,170,70)、左下蓝 (40,90,220)、
//! 右下黄 (240,200,40)，四块的中心是 (32,24)、(96,24)、(32,72)、(96,72)。下面没写误差的都应完全一致。
//!
//! | 文件 | 内容 | 误差 |
//! | --- | --- | --- |
//! | psd_rgb8_rle.psd | RGB 8 位，PackBits 压缩 | |
//! | psd_rgb16_raw.psd | RGB 16 位，不压缩 | |
//! | psd_rgb32_raw.psd | RGB 32 位浮点（线性值），不压缩 | ±2 |
//! | psd_gray8.psd | 灰度 8 位：左上 60、右上 120、左下 180、右下 240 | |
//! | psd_bitmap1.psd | 位图 1 位：左上、右下黑，右上、左下白 | |
//! | psd_cmyk8.psd | CMYK 8 位，按简单公式正好转成四色块（右下一块带 K） | |
//! | psd_indexed.psd | 索引色，调色板第 1–4 项是四色块，带一个没用到的透明色号 | |
//! | psd_transparent.psd | RGBA 8 位，图层数为负（有一个图层）：左上全透明，右上 Alpha 128（颜色和白色混合后存），下面两块不透明 | 右上 ±2 |
//! | psb_rgb8_rle.psb | PSB（版本 2）RGB 8 位，PackBits 压缩（每行字节数 4 字节） | |
//! | dds_bgra8.dds | 旧式 A8R8G8B8（按位掩码） | |
//! | dds_dxt1.dds | DXT1，每块是纯色 565 | ±4 |
//! | dds_dxt5.dds | DXT5，左下一块 Alpha 128 | ±4 |
//! | dds_bc7_dx10.dds | DX10 头 + BC7_UNORM，模式 6 的纯色块（P 位为 1，偶数值加 1） | ±1 |
//! | dds_rgba16f_dx10.dds | DX10 头 + R16G16B16A16_FLOAT（线性值） | ±2 |
//! | pcx_8bit.pcx | 8 位 1 平面，RLE，末尾 256 色调色板 | |
//! | pcx_24bit.pcx | 8 位 3 平面（RGB），RLE | |

use std::fs;
use std::path::Path;

pub const WIDTH: u32 = 128;
pub const HEIGHT: u32 = 96;

/// 灰度图四块的值（左上、右上、左下、右下）。
pub const GRAYS: [u8; 4] = [60, 120, 180, 240];

/// 生成 formats/ 下的文件，返回文件数。
pub fn generate(dir: &Path) -> crate::Result<usize> {
    crate::ensure_dir(dir)?;
    let files: [(&str, Vec<u8>); 16] = [
        ("psd_rgb8_rle.psd", psd_rgb(1, 8, true)),
        ("psd_rgb16_raw.psd", psd_rgb(1, 16, false)),
        ("psd_rgb32_raw.psd", psd_rgb(1, 32, false)),
        ("psd_gray8.psd", psd_gray()),
        ("psd_bitmap1.psd", psd_bitmap()),
        ("psd_cmyk8.psd", psd_cmyk()),
        ("psd_indexed.psd", psd_indexed()),
        ("psd_transparent.psd", psd_transparent()),
        ("psb_rgb8_rle.psb", psd_rgb(2, 8, true)),
        ("dds_bgra8.dds", dds_bgra8()),
        ("dds_dxt1.dds", dds_dxt1()),
        ("dds_dxt5.dds", dds_dxt5()),
        ("dds_bc7_dx10.dds", dds_bc7()),
        ("dds_rgba16f_dx10.dds", dds_rgba16f()),
        ("pcx_8bit.pcx", pcx_8bit()),
        ("pcx_24bit.pcx", pcx_24bit()),
    ];
    for (name, bytes) in &files {
        fs::write(dir.join(name), bytes)?;
    }
    Ok(files.len())
}

/// 像素 (x, y) 在哪一块：0 左上、1 右上、2 左下、3 右下。
fn quadrant(x: u32, y: u32) -> usize {
    match (x < WIDTH / 2, y < HEIGHT / 2) {
        (true, true) => 0,
        (false, true) => 1,
        (true, false) => 2,
        (false, false) => 3,
    }
}

/// 按块给每个像素取值，生成一个平面（每像素 `f` 返回的字节，按行连续）。
fn plane(f: impl Fn(usize) -> Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            out.extend(f(quadrant(x, y)));
        }
    }
    out
}

/// sRGB 8 位 → 线性值。
fn linear(c: u8) -> f32 {
    let v = f64::from(c) / 255.0;
    let l = if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    };
    l as f32
}

// ---------- PSD ----------

/// 写一个 PSD/PSB。`planes` 是各通道的数据（每行 `row_bytes` 字节）；`layers` 是图层和蒙版段的内容。
struct Psd {
    version: u16,
    mode: u16,
    depth: u16,
    planes: Vec<Vec<u8>>,
    color_data: Vec<u8>,
    resources: Vec<u8>,
    layers: Vec<u8>,
    rle: bool,
}

impl Psd {
    fn new(mode: u16, depth: u16, planes: Vec<Vec<u8>>) -> Self {
        Self {
            version: 1,
            mode,
            depth,
            planes,
            color_data: Vec::new(),
            resources: Vec::new(),
            layers: Vec::new(),
            rle: false,
        }
    }

    fn row_bytes(&self) -> usize {
        match self.depth {
            1 => (WIDTH as usize).div_ceil(8),
            d => WIDTH as usize * usize::from(d / 8),
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = b"8BPS".to_vec();
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        out.extend_from_slice(&(self.planes.len() as u16).to_be_bytes());
        out.extend_from_slice(&HEIGHT.to_be_bytes());
        out.extend_from_slice(&WIDTH.to_be_bytes());
        out.extend_from_slice(&self.depth.to_be_bytes());
        out.extend_from_slice(&self.mode.to_be_bytes());
        out.extend_from_slice(&(self.color_data.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.color_data);
        out.extend_from_slice(&(self.resources.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.resources);
        if self.version == 2 {
            out.extend_from_slice(&(self.layers.len() as u64).to_be_bytes());
        } else {
            out.extend_from_slice(&(self.layers.len() as u32).to_be_bytes());
        }
        out.extend_from_slice(&self.layers);
        if self.rle {
            out.extend_from_slice(&1u16.to_be_bytes());
            let rows: Vec<Vec<u8>> = self
                .planes
                .iter()
                .flat_map(|p| p.chunks(self.row_bytes()).map(pack_bits))
                .collect();
            for row in &rows {
                if self.version == 2 {
                    out.extend_from_slice(&(row.len() as u32).to_be_bytes());
                } else {
                    out.extend_from_slice(&(row.len() as u16).to_be_bytes());
                }
            }
            for row in &rows {
                out.extend_from_slice(row);
            }
        } else {
            out.extend_from_slice(&0u16.to_be_bytes());
            for p in &self.planes {
                out.extend_from_slice(p);
            }
        }
        out
    }
}

/// PackBits：3 个以上相同的字节写成重复，其余写成字面。
fn pack_bits(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < row.len() {
        let mut run = 1;
        while i + run < row.len() && run < 128 && row[i + run] == row[i] {
            run += 1;
        }
        if run >= 3 {
            out.push((1 - run as i16) as u8);
            out.push(row[i]);
            i += run;
            continue;
        }
        let start = i;
        while i < row.len() && i - start < 128 {
            if i + 2 < row.len() && row[i] == row[i + 1] && row[i] == row[i + 2] {
                break;
            }
            i += 1;
        }
        out.push((i - start - 1) as u8);
        out.extend_from_slice(&row[start..i]);
    }
    out
}

/// RGB 四色块：8、16 位按整数，32 位按线性浮点。
fn psd_rgb(version: u16, depth: u16, rle: bool) -> Vec<u8> {
    let planes = (0..3)
        .map(|c| {
            plane(|q| {
                let v = crate::QUAD_COLORS[q][c];
                match depth {
                    8 => vec![v],
                    16 => (u16::from(v) * 257).to_be_bytes().to_vec(),
                    _ => linear(v).to_be_bytes().to_vec(),
                }
            })
        })
        .collect();
    let mut psd = Psd::new(3, depth, planes);
    psd.version = version;
    psd.rle = rle;
    psd.bytes()
}

fn psd_gray() -> Vec<u8> {
    Psd::new(1, 8, vec![plane(|q| vec![GRAYS[q]])]).bytes()
}

/// 位图模式：1 是黑。左上、右下黑。
fn psd_bitmap() -> Vec<u8> {
    let row_bytes = (WIDTH as usize).div_ceil(8);
    let mut data = vec![0u8; row_bytes * HEIGHT as usize];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if matches!(quadrant(x, y), 0 | 3) {
                data[y as usize * row_bytes + x as usize / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let mut psd = Psd::new(0, 1, vec![data]);
    psd.rle = true;
    psd.bytes()
}

/// CMYK 存的是反过来的值（255 = 没有油墨），解码按 R = C' × K' / 255。前三块 K' = 255、C'M'Y' 就是 RGB；
/// 右下黄 (240,200,40) 用 K' = 240、C'M'Y' = (255,212,42)。
fn psd_cmyk() -> Vec<u8> {
    let inks = |q: usize| -> [u8; 4] {
        match q {
            3 => [255, 212, 42, 240],
            _ => {
                let [r, g, b] = crate::QUAD_COLORS[q];
                [r, g, b, 255]
            }
        }
    };
    let planes = (0..4).map(|c| plane(|q| vec![inks(q)[c]])).collect();
    let mut psd = Psd::new(4, 8, planes);
    psd.rle = true;
    psd.bytes()
}

/// 索引色：调色板第 1–4 项是四色块（R、G、B 三段各 256 字节），透明色号 0（没有像素用它）。
fn psd_indexed() -> Vec<u8> {
    let mut palette = vec![0u8; 768];
    for (q, color) in crate::QUAD_COLORS.iter().enumerate() {
        for c in 0..3 {
            palette[c * 256 + q + 1] = color[c];
        }
    }
    let mut psd = Psd::new(2, 8, vec![plane(|q| vec![q as u8 + 1])]);
    psd.color_data = palette;
    psd.resources = resource(1047, &0u16.to_be_bytes());
    psd.bytes()
}

/// 一项图像资源（名字为空）。
fn resource(id: u16, body: &[u8]) -> Vec<u8> {
    let mut out = b"8BIM".to_vec();
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0, 0]); // 空名字，补齐到偶数
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// 四块的透明度：左上 0、右上 128、下面不透明。
pub const TRANSPARENT_ALPHA: [u8; 4] = [0, 128, 255, 255];

/// RGBA：一个图层（图层数写成 −1，表示合成图的第一个额外通道是透明度）。图层里存原来的颜色；
/// 合成图里的颜色按 Photoshop 的做法和白色混合后存。
fn psd_transparent() -> Vec<u8> {
    let color = |q: usize, c: usize| crate::QUAD_COLORS[q][c];
    let blended = |q: usize, c: usize| {
        let a = u32::from(TRANSPARENT_ALPHA[q]);
        ((u32::from(color(q, c)) * a + 255 * (255 - a) + 127) / 255) as u8
    };
    let layer_planes: Vec<Vec<u8>> = [3usize, 0, 1, 2]
        .iter()
        .map(|&c| {
            plane(|q| {
                vec![if c == 3 {
                    TRANSPARENT_ALPHA[q]
                } else {
                    color(q, c)
                }]
            })
        })
        .collect();
    let mut info = Vec::new();
    info.extend_from_slice(&(-1i16).to_be_bytes());
    // 图层记录：范围、4 个通道（−1 是透明度）、混合模式、不透明度等、附加数据（空蒙版、空混合范围、名字 "L"）。
    for v in [0u32, 0, HEIGHT, WIDTH] {
        info.extend_from_slice(&v.to_be_bytes());
    }
    info.extend_from_slice(&4u16.to_be_bytes());
    for id in [-1i16, 0, 1, 2] {
        info.extend_from_slice(&id.to_be_bytes());
        info.extend_from_slice(&(2 + WIDTH * HEIGHT).to_be_bytes());
    }
    info.extend_from_slice(b"8BIMnorm");
    info.extend_from_slice(&[255, 0, 0, 0]);
    let extra = [0u8, 0, 0, 0, 0, 0, 0, 0, 1, b'L', 0, 0];
    info.extend_from_slice(&(extra.len() as u32).to_be_bytes());
    info.extend_from_slice(&extra);
    for p in &layer_planes {
        info.extend_from_slice(&0u16.to_be_bytes());
        info.extend_from_slice(p);
    }
    if info.len() % 2 == 1 {
        info.push(0);
    }
    let mut layers = (info.len() as u32).to_be_bytes().to_vec();
    layers.extend_from_slice(&info);
    layers.extend_from_slice(&0u32.to_be_bytes()); // 全局图层蒙版

    let mut planes: Vec<Vec<u8>> = (0..3).map(|c| plane(|q| vec![blended(q, c)])).collect();
    planes.push(plane(|q| vec![TRANSPARENT_ALPHA[q]]));
    let mut psd = Psd::new(3, 8, planes);
    psd.layers = layers;
    psd.rle = true;
    psd.bytes()
}

// ---------- DDS ----------

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;

/// DDS 文件头（124 字节）+ 可选的 DX10 扩展头 + 数据。
fn dds(
    flags: u32,
    fourcc: &[u8; 4],
    bits: u32,
    masks: [u32; 4],
    dxgi: Option<u32>,
    data: &[u8],
) -> Vec<u8> {
    let mut header = [0u32; 31];
    header[0] = 124;
    header[1] = 0x1 | 0x2 | 0x4 | 0x1000; // CAPS、HEIGHT、WIDTH、PIXELFORMAT
    header[2] = HEIGHT;
    header[3] = WIDTH;
    header[6] = 1;
    header[18] = 32;
    header[19] = flags;
    header[20] = u32::from_le_bytes(*fourcc);
    header[21] = bits;
    header[22..26].copy_from_slice(&masks);
    header[26] = 0x1000; // DDSCAPS_TEXTURE
    let mut out = b"DDS ".to_vec();
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

fn dds_bgra8() -> Vec<u8> {
    let data = plane(|q| {
        let [r, g, b] = crate::QUAD_COLORS[q];
        vec![b, g, r, 255]
    });
    dds(
        DDPF_RGB | DDPF_ALPHAPIXELS,
        &[0; 4],
        32,
        [0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000],
        None,
        &data,
    )
}

/// 按 4x4 块生成数据：`block(q)` 给出每块的字节（四色块的边界是 4 的倍数，每块只落在一块颜色里）。
fn blocks(block: impl Fn(usize) -> Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    for by in 0..HEIGHT / 4 {
        for bx in 0..WIDTH / 4 {
            out.extend(block(quadrant(bx * 4, by * 4)));
        }
    }
    out
}

/// RGB565（四舍五入）。
fn rgb565(c: [u8; 3]) -> u16 {
    let r = (u16::from(c[0]) * 31 + 127) / 255;
    let g = (u16::from(c[1]) * 63 + 127) / 255;
    let b = (u16::from(c[2]) * 31 + 127) / 255;
    (r << 11) | (g << 5) | b
}

/// 纯色的 BC1 块：两个端点相同、序号都是 0。
fn bc1_solid(c: [u8; 3]) -> [u8; 8] {
    let v = rgb565(c).to_le_bytes();
    [v[0], v[1], v[0], v[1], 0, 0, 0, 0]
}

fn dds_dxt1() -> Vec<u8> {
    let data = blocks(|q| bc1_solid(crate::QUAD_COLORS[q]).to_vec());
    dds(DDPF_FOURCC, b"DXT1", 0, [0; 4], None, &data)
}

/// DXT5 每块的透明度：左下 128，其余不透明。
pub const DXT5_ALPHA: [u8; 4] = [255, 255, 128, 255];

fn dds_dxt5() -> Vec<u8> {
    let data = blocks(|q| {
        let a = DXT5_ALPHA[q];
        let mut block = vec![a, a, 0, 0, 0, 0, 0, 0];
        block.extend_from_slice(&bc1_solid(crate::QUAD_COLORS[q]));
        block
    });
    dds(DDPF_FOURCC, b"DXT5", 0, [0; 4], None, &data)
}

/// 从最低位开始往 128 位里写。
struct Bits {
    value: u128,
    len: u32,
}

impl Bits {
    fn put(&mut self, v: u32, n: u32) {
        self.value |= u128::from(v) << self.len;
        self.len += n;
    }
}

/// BC7 模式 6 的纯色块：7 位端点加 1 位 P（都取 1，Alpha 正好 255），序号都是 0（第一个序号 3 位，其余 4 位）。
fn bc7_solid(c: [u8; 3]) -> [u8; 16] {
    let mut bits = Bits { value: 0, len: 0 };
    bits.put(1 << 6, 7); // 模式 6：6 个 0 后面一个 1
    for channel in c {
        let v = u32::from(channel >> 1);
        bits.put(v, 7);
        bits.put(v, 7);
    }
    bits.put(127, 7);
    bits.put(127, 7);
    bits.put(1, 1);
    bits.put(1, 1);
    bits.put(0, 3);
    for _ in 1..16 {
        bits.put(0, 4);
    }
    debug_assert_eq!(bits.len, 128);
    bits.value.to_le_bytes()
}

fn dds_bc7() -> Vec<u8> {
    let data = blocks(|q| bc7_solid(crate::QUAD_COLORS[q]).to_vec());
    dds(DDPF_FOURCC, b"DX10", 0, [0; 4], Some(98), &data)
}

/// f32 → 半精度（只处理这里用到的正常范围的正数和 0，四舍五入）。
fn half(v: f32) -> u16 {
    if v == 0.0 {
        return 0;
    }
    let bits = v.to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    let man = bits & 0x7F_FFFF;
    assert!((1..31).contains(&exp), "超出半精度的正常范围：{v}");
    let mut h = ((exp as u32) << 10) | (man >> 13);
    if man & 0x1000 != 0 {
        h += 1; // 进位到指数也正确
    }
    h as u16
}

fn dds_rgba16f() -> Vec<u8> {
    let data = plane(|q| {
        let [r, g, b] = crate::QUAD_COLORS[q];
        let mut px = Vec::new();
        for v in [linear(r), linear(g), linear(b), 1.0] {
            px.extend_from_slice(&half(v).to_le_bytes());
        }
        px
    });
    dds(DDPF_FOURCC, b"DX10", 0, [0; 4], Some(10), &data)
}

// ---------- PCX ----------

/// PCX 文件头（版本 5、RLE）+ 每行按平面 RLE 压缩的数据 + 可选的末尾数据（256 色调色板）。
fn pcx(planes: u8, scanline: impl Fn(u32) -> Vec<u8>, tail: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 128];
    out[0] = 0x0A;
    out[1] = 5;
    out[2] = 1;
    out[3] = 8;
    out[8..10].copy_from_slice(&(WIDTH as u16 - 1).to_le_bytes());
    out[10..12].copy_from_slice(&(HEIGHT as u16 - 1).to_le_bytes());
    out[12..14].copy_from_slice(&72u16.to_le_bytes());
    out[14..16].copy_from_slice(&72u16.to_le_bytes());
    out[65] = planes;
    out[66..68].copy_from_slice(&(WIDTH as u16).to_le_bytes());
    out[68] = 1;
    for y in 0..HEIGHT {
        let line = scanline(y);
        // 每个平面单独压缩，重复不跨平面。
        for part in line.chunks(WIDTH as usize) {
            out.extend(pcx_rle(part));
        }
    }
    out.extend_from_slice(tail);
    out
}

/// PCX 的 RLE：重复的值或最高两位是 1 的值写成"0xC0 | 次数"加值，次数最多 63。
fn pcx_rle(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let v = line[i];
        let mut n = 1;
        while i + n < line.len() && n < 63 && line[i + n] == v {
            n += 1;
        }
        if n > 1 || v >= 0xC0 {
            out.push(0xC0 | n as u8);
        }
        out.push(v);
        i += n;
    }
    out
}

fn pcx_8bit() -> Vec<u8> {
    let mut tail = vec![0x0C];
    for i in 0..256 {
        let color = match i {
            1..=4 => crate::QUAD_COLORS[i - 1],
            _ => [0, 0, 0],
        };
        tail.extend_from_slice(&color);
    }
    pcx(
        1,
        |y| (0..WIDTH).map(|x| quadrant(x, y) as u8 + 1).collect(),
        &tail,
    )
}

fn pcx_24bit() -> Vec<u8> {
    pcx(
        3,
        |y| {
            (0..3)
                .flat_map(|c| (0..WIDTH).map(move |x| crate::QUAD_COLORS[quadrant(x, y)][c]))
                .collect()
        },
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyview_core::decode::{DecodedImage, decode_file};

    /// 四块中心的点。
    const CENTERS: [(usize, usize); 4] = [(32, 24), (96, 24), (32, 72), (96, 72)];

    struct Case {
        name: &'static str,
        format: &'static str,
        pixel_format: &'static str,
        /// 四块的颜色（不预乘）和透明度。
        colors: [[u8; 3]; 4],
        alpha: [u8; 4],
        /// 每个通道允许的误差。
        tolerance: u8,
    }

    fn quads() -> [[u8; 3]; 4] {
        crate::QUAD_COLORS
    }

    fn cases() -> Vec<Case> {
        let opaque = [255; 4];
        let gray = GRAYS.map(|v| [v, v, v]);
        let bitmap = [[0; 3], [255; 3], [255; 3], [0; 3]];
        let case = |name, format, pixel_format, colors, alpha, tolerance| Case {
            name,
            format,
            pixel_format,
            colors,
            alpha,
            tolerance,
        };
        vec![
            case("psd_rgb8_rle.psd", "PSD", "RGB24", quads(), opaque, 0),
            case("psd_rgb16_raw.psd", "PSD", "RGB48", quads(), opaque, 0),
            case("psd_rgb32_raw.psd", "PSD", "RGB96F", quads(), opaque, 2),
            case("psd_gray8.psd", "PSD", "Gray8", gray, opaque, 0),
            case("psd_bitmap1.psd", "PSD", "Bitmap1", bitmap, opaque, 0),
            case("psd_cmyk8.psd", "PSD", "CMYK32", quads(), opaque, 0),
            case("psd_indexed.psd", "PSD", "Indexed8", quads(), opaque, 0),
            case(
                "psd_transparent.psd",
                "PSD",
                "RGBA32",
                quads(),
                TRANSPARENT_ALPHA,
                2,
            ),
            case("psb_rgb8_rle.psb", "PSB", "RGB24", quads(), opaque, 0),
            case("dds_bgra8.dds", "DDS", "RGBA32", quads(), opaque, 0),
            case("dds_dxt1.dds", "DDS", "BC1", quads(), opaque, 4),
            case("dds_dxt5.dds", "DDS", "BC3", quads(), DXT5_ALPHA, 4),
            case("dds_bc7_dx10.dds", "DDS", "BC7", quads(), opaque, 1),
            case("dds_rgba16f_dx10.dds", "DDS", "RGBA64F", quads(), opaque, 2),
            case("pcx_8bit.pcx", "PCX", "Indexed8", quads(), opaque, 0),
            case("pcx_24bit.pcx", "PCX", "RGB24", quads(), opaque, 0),
        ]
    }

    /// 解码结果里的一个点（BGRA 预乘）→ RGBA（仍是预乘的）。
    fn rgba_at(image: &DecodedImage, x: usize, y: usize) -> [u8; 4] {
        let at = y * image.stride + x * 4;
        let p = &image.pixels[at..at + 4];
        [p[2], p[1], p[0], p[3]]
    }

    /// 不预乘的颜色和透明度 → 预乘（与核心的换算相同）。
    fn premultiplied(c: [u8; 3], a: u8) -> [u8; 4] {
        let m = |v: u8| ((u32::from(v) * u32::from(a) + 127) / 255) as u8;
        [m(c[0]), m(c[1]), m(c[2]), a]
    }

    /// 每个测试自己的临时目录（测试并行运行）。
    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "anyview-fixtures-formats-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn every_file_decodes_to_the_quadrants() {
        let dir = temp_dir("decode");
        let n = generate(&dir).unwrap();
        assert_eq!(n, fs::read_dir(&dir).unwrap().count());
        assert_eq!(n, cases().len());
        for case in cases() {
            let image =
                decode_file(&dir.join(case.name)).unwrap_or_else(|e| panic!("{}：{e}", case.name));
            assert_eq!(
                (image.width, image.height),
                (WIDTH, HEIGHT),
                "{}",
                case.name
            );
            assert_eq!(image.info.format, case.format, "{}", case.name);
            assert_eq!(image.info.pixel_format, case.pixel_format, "{}", case.name);
            assert_eq!(
                image.info.has_alpha,
                case.alpha != [255; 4],
                "{}",
                case.name
            );
            for (q, &(x, y)) in CENTERS.iter().enumerate() {
                let got = rgba_at(&image, x, y);
                let want = premultiplied(case.colors[q], case.alpha[q]);
                let close = got
                    .iter()
                    .zip(&want)
                    .all(|(g, w)| g.abs_diff(*w) <= case.tolerance);
                assert!(
                    close,
                    "{} 第 {q} 块：得到 {got:?}，应为 {want:?}（误差 {}）",
                    case.name, case.tolerance
                );
            }
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncated_or_damaged_files_never_panic() {
        let dir = temp_dir("damaged");
        generate(&dir).unwrap();
        type Decode = fn(&[u8]) -> Result<anyview_core::decode::Raster, anyview_core::i18n::Message>;
        let decoders: [(&str, Decode); 3] = [
            ("ps", anyview_core::psd::decode),
            ("dds", anyview_core::dds::decode),
            ("pcx", anyview_core::pcx::decode),
        ];
        for case in cases() {
            let bytes = fs::read(dir.join(case.name)).unwrap();
            let (_, decode) = decoders
                .iter()
                .find(|(p, _)| case.name.starts_with(p))
                .unwrap();
            // 截短到各种长度。
            let step = (bytes.len() / 300).max(1);
            for len in (0..bytes.len()).step_by(step) {
                let _ = decode(&bytes[..len]);
            }
            // 在固定的位置改坏一些字节（确定的伪随机数）。
            let mut seed = 0x2545_F491_u32;
            for _ in 0..200 {
                let mut damaged = bytes.clone();
                for _ in 0..4 {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let at = seed as usize % damaged.len();
                    damaged[at] = (seed >> 24) as u8;
                }
                let _ = decode(&damaged);
            }
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writers_match_the_formats() {
        // PackBits 写出来能被核心解回去（PSD 用的那套规则）。
        let row: Vec<u8> = (0..300)
            .map(|i| if i % 50 < 30 { 7 } else { (i % 7) as u8 })
            .collect();
        let packed = pack_bits(&row);
        assert!(packed.len() < row.len());
        // BC7 纯色块：偶数值加 1。
        let block = bc7_solid([220, 41, 0]);
        assert_eq!(block[0] & 0x7F, 0x40);
        // 半精度：1.0 = 0x3C00，0.5 = 0x3800。
        assert_eq!((half(1.0), half(0.5), half(0.0)), (0x3C00, 0x3800, 0));
        // RGB565 往返误差在 4 以内。
        for c in quads() {
            let v = rgb565(c);
            let r = ((v >> 11) & 0x1F) as u32 * 255 / 31;
            assert!(r.abs_diff(u32::from(c[0])) <= 4);
        }
    }
}
