//! PSD、PSB：读文件里保存的合成图（Image Data 段），不解析图层。
//!
//! 支持：PSD（版本 1）和 PSB（版本 2）；不压缩或 PackBits（RLE）压缩（ZIP 压缩的合成图很少见，报错）；
//! 1、8、16、32 位；位图、灰度、索引色、RGB、CMYK、双色调（按灰度）、多通道（第一个通道按灰度）、Lab。
//!
//! - 透明：图层数为负时（Adobe 文件格式说明），合成图的第一个额外通道是透明度；16、32 位文件的图层数
//!   可能写在附加信息块 Lr16、Lr32 里，也看。图层数不为负时额外通道是用户存的选区，不当透明度。
//!   带透明的合成图里，颜色是和白色混合后存的（ImageMagick、psd-tools 同样处理），读出时按 Alpha 还原。
//! - 色彩：内嵌 ICC（资源 1039）是 RGB 的（RGB、索引色）交给色彩管理；是 CMYK、灰度的在这里用 qcms 转成 sRGB
//!   （界面只认 RGB 像素）。没有配置文件、读不了，或关掉了"使用图片文件的 ICC"时，CMYK 按简单公式转 RGB，
//!   灰度原样。Lab 按 D50 白点转 sRGB。32 位是线性的浮点数：截到 0–1 后按 sRGB 编码（HDR 映射到普通亮度），
//!   不再用它的 ICC。
//! - 索引色的透明色号（资源 1047）显示为透明。

use crate::i18n::Message;
use std::sync::OnceLock;

use crate::color::{self, SampleKind};
use crate::decode::Raster;

const TRUNCATED: Message = msg!("PSD 文件不完整", "The PSD file is incomplete");

/// 颜色模式（文件头里的编号）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Bitmap,
    Gray,
    Indexed,
    Rgb,
    Cmyk,
    Multichannel,
    Duotone,
    Lab,
}

impl Mode {
    fn from_id(id: u16) -> Option<Self> {
        Some(match id {
            0 => Mode::Bitmap,
            1 => Mode::Gray,
            2 => Mode::Indexed,
            3 => Mode::Rgb,
            4 => Mode::Cmyk,
            7 => Mode::Multichannel,
            8 => Mode::Duotone,
            9 => Mode::Lab,
            _ => return None,
        })
    }

    /// 合成图里表示颜色的通道数（多通道模式只取第一个通道）。
    fn color_channels(self) -> usize {
        match self {
            Mode::Bitmap | Mode::Gray | Mode::Indexed | Mode::Multichannel | Mode::Duotone => 1,
            Mode::Rgb | Mode::Lab => 3,
            Mode::Cmyk => 4,
        }
    }

    /// 像素格式里的名字（如 "RGB24"、"GrayA16"）。
    fn name(self) -> &'static str {
        match self {
            Mode::Bitmap => "Bitmap",
            Mode::Gray => "Gray",
            Mode::Indexed => "Indexed",
            Mode::Rgb => "RGB",
            Mode::Cmyk => "CMYK",
            Mode::Multichannel => "Multichannel",
            Mode::Duotone => "Duotone",
            Mode::Lab => "Lab",
        }
    }

    /// 和白色混合时，这个通道的"白色"：Lab 的 a、b 是 128（无色），其他都是 255（CMYK 存的是反过来的值，255 = 没有油墨）。
    fn white(self, channel: usize) -> u8 {
        match (self, channel) {
            (Mode::Lab, 1 | 2) => 128,
            _ => 255,
        }
    }
}

/// 按大端读字节的游标；越界时报"文件不完整"。
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Message> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or(TRUNCATED)?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Message> {
        let bytes = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, Message> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, Message> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, Message> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, Message> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    /// 段长度：PSD 是 4 字节，PSB 的部分段是 8 字节。
    fn length(&mut self, long: bool) -> Result<usize, Message> {
        let n = if long {
            self.u64()?
        } else {
            u64::from(self.u32()?)
        };
        usize::try_from(n).map_err(|_| TRUNCATED)
    }
}

/// 图像资源段里用到的几项。
#[derive(Debug, Default)]
struct Resources {
    /// 内嵌 ICC（资源 1039）。
    icc: Option<Vec<u8>>,
    /// 索引色的透明色号（资源 1047）。
    transparent_index: Option<u8>,
    /// 版本信息（资源 1057）里的"有真正的合成图"：关掉"最大兼容"存的为假。
    real_merged: Option<bool>,
}

/// 解码整个文件（使用内嵌的 ICC）。格式不对、数据坏了时返回错误说明（不 panic）。
pub fn decode(data: &[u8]) -> Result<Raster, Message> {
    decode_with(data, true)
}

/// 解码整个文件；`use_icc` 为假时（关闭“使用图片文件的 ICC”）CMYK、灰度不用内嵌的配置文件。
/// 文件里没有真正的合成图（关掉"最大兼容"存的）时合成图层；合成图读不出来而有图层时也合成。
pub fn decode_with(data: &[u8], use_icc: bool) -> Result<Raster, Message> {
    match decode_inner(data, use_icc, false) {
        Ok(raster) => Ok(raster),
        Err(error) => decode_inner(data, use_icc, true).map_err(|_| error),
    }
}

/// 不管有没有合成图，都把图层合成出来（核对合成用：与文件里的合成图比）。
pub fn decode_layers(data: &[u8], use_icc: bool) -> Result<Raster, Message> {
    decode_inner(data, use_icc, true)
}

fn decode_inner(data: &[u8], use_icc: bool, force_layers: bool) -> Result<Raster, Message> {
    let mut r = Reader::new(data);
    if r.take(4)? != b"8BPS" {
        return Err(msg!("不是 PSD 文件", "Not a PSD file"));
    }
    let version = r.u16()?;
    let big = match version {
        1 => false,
        2 => true,
        _ => return Err(msg!("不认识的 PSD 版本：{version}", "Unknown PSD version: {version}", version = format!("{version}"))),
    };
    r.take(6)?;
    let channels = usize::from(r.u16()?);
    let height = r.u32()?;
    let width = r.u32()?;
    let depth = r.u16()?;
    let mode_id = r.u16()?;
    let mode = Mode::from_id(mode_id).ok_or_else(|| msg!("不认识的 PSD 颜色模式：{mode_id}", "Unknown PSD color mode: {mode_id}", mode_id = format!("{mode_id}")))?;
    let max_side = if big { 300_000 } else { 30_000 };
    if !(1..=56).contains(&channels) {
        return Err(msg!("PSD 的通道数不对：{channels}", "Invalid PSD channel count: {channels}", channels = format!("{channels}")));
    }
    if width == 0 || height == 0 || width > max_side || height > max_side {
        return Err(msg!("PSD 的尺寸不对：{width}x{height}", "Invalid PSD size: {width}x{height}", width = format!("{width}"), height = format!("{height}")));
    }
    match (mode, depth) {
        (Mode::Bitmap, 1) | (Mode::Indexed, 8) => {}
        (Mode::Bitmap | Mode::Indexed, _) | (_, 1) => {
            return Err(msg!(
                "PSD 的位深不对：{mode} 模式 {depth} 位",
                "Invalid PSD bit depth: {depth} bits in {mode} mode",
                mode = mode.name(),
                depth = depth.to_string()
            ));
        }
        (_, 8 | 16 | 32) => {}
        _ => return Err(msg!("不支持 {depth} 位的 PSD", "{depth}-bit PSD files aren't supported", depth = format!("{depth}"))),
    }
    let color = mode.color_channels();
    if channels < color {
        return Err(msg!(
            "PSD 的通道不够：{mode} 模式要 {color} 个，文件里只有 {channels} 个",
            "Not enough PSD channels: {mode} mode needs {color}, the file has {channels}",
            mode = mode.name(),
            color = color.to_string(),
            channels = channels.to_string()
        ));
    }

    // 颜色模式数据（索引色的调色板）、图像资源、图层和蒙版信息（只看图层数的正负）。
    let color_data_len = r.length(false)?;
    let color_data = r.take(color_data_len)?;
    let resources_len = r.length(false)?;
    let resources = parse_resources(r.take(resources_len)?);
    let layers_len = r.length(big)?;
    let layer_section = r.take(layers_len)?;
    let merged_alpha = merged_alpha(layer_section, big);

    // 没有真正的合成图：自己合成图层（位图、索引色没有图层）。
    if (force_layers || resources.real_merged == Some(false))
        && !matches!(mode, Mode::Bitmap | Mode::Indexed)
    {
        let layers = crate::psd_layers::composite(
            layer_section,
            big,
            depth,
            color,
            mode == Mode::Rgb,
            width,
            height,
        );
        match layers {
            Some(c) => {
                return finish(Finish {
                    mode,
                    depth,
                    color,
                    unblend: c.has_alpha,
                    rgba: c.rgba,
                    black: c.black,
                    resources,
                    color_data,
                    use_icc,
                    width,
                    height,
                    big,
                });
            }
            None if force_layers => return Err(msg!("PSD 没有能合成的图层", "The PSD has no layers that can be composited")),
            // 图层因为内存上限没合成：报出来（界面会问要不要仍然打开），不悄悄显示文件里那张空白的合成图。
            None if crate::decode::limit_was_hit() => {
                return Err(msg!("PSD 的图层太多、太大，超过了内存上限", "The PSD's layers are too many or too large for the memory limit"));
            }
            None => {}
        }
    }

    // 合成图。
    let compression = r.u16()?;
    let image_data = &data[r.pos..];
    let w = width as usize;
    let h = height as usize;
    let row_bytes = match depth {
        1 => w.div_ceil(8),
        d => w * usize::from(d / 8),
    };
    let alpha_channel =
        (merged_alpha && channels > color && !matches!(mode, Mode::Bitmap | Mode::Indexed))
            .then_some(color);
    let needed = color + usize::from(alpha_channel.is_some());
    let planes = Planes::locate(image_data, compression, channels, needed, h, row_bytes, big)?;

    let mut rgba = Raster::alloc(width, height)?;
    let mut scratch = vec![0u8; row_bytes];
    let pixel_count = w * h;

    if mode == Mode::Bitmap {
        for y in 0..h {
            let row = planes.row(0, y, &mut scratch);
            for x in 0..w {
                // 位图模式：1 是黑，0 是白。
                let bit = (row[x / 8] >> (7 - x % 8)) & 1;
                let v = if bit == 1 { 0 } else { 255 };
                rgba[(y * w + x) * 4..][..4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        return Ok(Raster {
            width,
            height,
            rgba,
            has_alpha: false,
            format: format_name(big),
            pixel_format: "Bitmap1".to_string(),
            icc_profile: None,
        });
    }

    // 透明度先读：还原和白色混合过的颜色要用到。
    match alpha_channel {
        Some(channel) => {
            for y in 0..h {
                let row = planes.row(channel, y, &mut scratch);
                for x in 0..w {
                    rgba[(y * w + x) * 4 + 3] = alpha_sample(row, x, depth);
                }
            }
        }
        None => rgba.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255),
    }
    let unblend = alpha_channel.is_some();

    // 颜色通道：RGB、Lab 依次放进 R、G、B；CMYK 的 C、M、Y 放进 R、G、B，K 另放；单通道的放进 R。
    let mut black = if mode == Mode::Cmyk {
        vec![0u8; pixel_count]
    } else {
        Vec::new()
    };
    for channel in 0..color {
        let white = mode.white(channel);
        // 放进哪个通道（CMYK 的 K 另放，为空）。
        let target = match (mode, channel) {
            (Mode::Cmyk, 3) => None,
            (Mode::Rgb | Mode::Lab | Mode::Cmyk, c) => Some(c),
            _ => Some(0),
        };
        for y in 0..h {
            let row = planes.row(channel, y, &mut scratch);
            let line = &mut rgba[y * w * 4..(y + 1) * w * 4];
            for x in 0..w {
                let v = if depth == 8 && !unblend {
                    row[x]
                } else {
                    color_sample(row, x, depth, unblend.then_some(line[x * 4 + 3]), white)
                };
                match target {
                    Some(c) => line[x * 4 + c] = v,
                    None => black[y * w + x] = v,
                }
            }
        }
    }

    finish(Finish {
        mode,
        depth,
        color,
        unblend,
        rgba,
        black,
        resources,
        color_data,
        use_icc,
        width,
        height,
        big,
    })
}

/// 读出或合成好的通道（放法见 [`crate::psd_layers::Composite`]）按颜色模式转成 RGB。
struct Finish<'a> {
    mode: Mode,
    depth: u16,
    color: usize,
    /// 有透明度（Alpha 已是不预乘的）。
    unblend: bool,
    rgba: Vec<u8>,
    black: Vec<u8>,
    resources: Resources,
    color_data: &'a [u8],
    use_icc: bool,
    width: u32,
    height: u32,
    big: bool,
}

fn finish(f: Finish<'_>) -> Result<Raster, Message> {
    let Finish {
        mode,
        depth,
        color,
        unblend,
        mut rgba,
        black,
        resources,
        color_data,
        use_icc,
        width,
        height,
        big,
    } = f;
    // 按颜色模式转成 RGB。CMYK、灰度的配置文件在这里用（32 位的不用，见文件开头）。
    let icc_for_samples = resources.icc.as_deref().filter(|_| use_icc && depth != 32);
    let mut transparent_used = false;
    match mode {
        Mode::Rgb | Mode::Bitmap => {}
        Mode::Gray | Mode::Duotone | Mode::Multichannel => {
            // 灰度带灰度配置文件时按它转（Photoshop 默认的"灰度系数 20%"显示时比原值暗）。
            let profile = icc_for_samples.filter(|_| mode == Mode::Gray);
            let converted = profile.is_some_and(|icc| {
                let samples: Vec<u8> = rgba.iter().step_by(4).copied().collect();
                color::samples_to_srgb(&samples, SampleKind::Gray, icc, &mut rgba)
            });
            if !converted {
                for px in rgba.as_chunks_mut::<4>().0.iter_mut() {
                    px[1] = px[0];
                    px[2] = px[0];
                }
            }
        }
        Mode::Indexed => {
            if color_data.len() < 768 {
                return Err(msg!("PSD 索引色的调色板不完整", "The PSD's indexed-color palette is incomplete"));
            }
            for px in rgba.as_chunks_mut::<4>().0.iter_mut() {
                let i = usize::from(px[0]);
                px[0] = color_data[i];
                px[1] = color_data[256 + i];
                px[2] = color_data[512 + i];
                if resources.transparent_index == Some(i as u8) {
                    px[3] = 0;
                    transparent_used = true;
                }
            }
        }
        Mode::Cmyk => {
            // 带 CMYK 配置文件时按它转（ICC 的约定是 0 = 没有油墨，PSD 存的是反过来的值）。
            let converted = icc_for_samples.is_some_and(|icc| {
                let samples: Vec<u8> = rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(&black)
                    .flat_map(|(px, &k)| [255 - px[0], 255 - px[1], 255 - px[2], 255 - k])
                    .collect();
                color::samples_to_srgb(&samples, SampleKind::Cmyk, icc, &mut rgba)
            });
            if !converted {
                // 简单公式：R = C' × K' / 255，G、B 同样。
                for (px, &k) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(&black) {
                    for c in &mut px[..3] {
                        *c = ((u32::from(*c) * u32::from(k) + 127) / 255) as u8;
                    }
                }
            }
        }
        Mode::Lab => {
            for px in rgba.as_chunks_mut::<4>().0.iter_mut() {
                let [r, g, b] = lab_to_srgb(px[0], px[1], px[2]);
                px[0] = r;
                px[1] = g;
                px[2] = b;
            }
        }
    }

    let has_alpha =
        (unblend || transparent_used) && rgba.iter().skip(3).step_by(4).any(|&a| a < 255);
    let bits = (color + usize::from(unblend)) * usize::from(depth);
    let pixel_format = format!(
        "{}{}{}{}",
        mode.name(),
        if unblend { "A" } else { "" },
        bits,
        if depth == 32 { "F" } else { "" }
    );
    // ICC：只传 RGB 配置文件，而且只在像素就是按它存的时候（RGB、索引色的 8、16 位）。
    let icc_profile = resources
        .icc
        .filter(|icc| icc.get(16..20) == Some(b"RGB "))
        .filter(|_| matches!(mode, Mode::Rgb | Mode::Indexed) && depth != 32);
    Ok(Raster {
        width,
        height,
        rgba,
        has_alpha,
        format: format_name(big),
        pixel_format,
        icc_profile,
    })
}

fn format_name(big: bool) -> String {
    if big { "PSB" } else { "PSD" }.to_string()
}

/// 读图像资源段；某一项坏了就停在那里（不影响显示合成图）。
fn parse_resources(block: &[u8]) -> Resources {
    let mut out = Resources::default();
    let mut r = Reader::new(block);
    while r.remaining() >= 12 {
        let Some((id, body)) = next_resource(&mut r) else {
            break;
        };
        match id {
            1039 if !body.is_empty() => out.icc = Some(body.to_vec()),
            1057 if body.len() >= 5 => out.real_merged = Some(body[4] != 0),
            1047 if body.len() >= 2 => {
                out.transparent_index = u8::try_from(u16::from_be_bytes([body[0], body[1]])).ok();
            }
            _ => {}
        }
    }
    out
}

/// 一项图像资源：签名（一般是 8BIM）、编号、Pascal 字符串的名字（连长度字节补齐到偶数）、长度、内容（补齐到偶数）。
fn next_resource<'a>(r: &mut Reader<'a>) -> Option<(u16, &'a [u8])> {
    r.take(4).ok()?;
    let id = r.u16().ok()?;
    let name = usize::from(r.u8().ok()?) + 1;
    r.take(name + name % 2 - 1).ok()?;
    let size = r.length(false).ok()?;
    let body = r.take(size).ok()?;
    if size % 2 == 1 {
        let _ = r.take(1);
    }
    Some((id, body))
}

/// 附加信息块里长度是 8 字节的键（只在 PSB 里）。
const LONG_KEYS: [&[u8; 4]; 13] = [
    b"LMsk", b"Lr16", b"Lr32", b"Layr", b"Mt16", b"Mt32", b"Mtrn", b"Alph", b"FMsk", b"lnk2",
    b"FEid", b"FXid", b"PxSD",
];

/// PSB 里这个附加信息块的长度是 8 字节。
pub(crate) fn is_long_key(key: &[u8; 4]) -> bool {
    LONG_KEYS.contains(&key)
}

/// 图层和蒙版信息段：图层数为负表示合成图的第一个额外通道是透明度。
fn merged_alpha(section: &[u8], big: bool) -> bool {
    let mut r = Reader::new(section);
    let Ok(info_len) = r.length(big) else {
        return false;
    };
    let info = &section[r.pos..];
    if info_len >= 2 && info.len() >= 2 && i16::from_be_bytes([info[0], info[1]]) < 0 {
        return true;
    }
    if r.take(info_len).is_err() {
        return false;
    }
    // 全局图层蒙版，然后是附加信息块（16、32 位文件的图层在 Lr16、Lr32 里）。
    let Ok(mask_len) = r.length(false) else {
        return false;
    };
    if r.take(mask_len).is_err() {
        return false;
    }
    tagged_layer_count_negative(&mut r, big)
}

fn tagged_layer_count_negative(r: &mut Reader<'_>, big: bool) -> bool {
    loop {
        // 各块之间可能补齐到 4 字节：在接下来的 4 个位置里找签名。
        let Some(skip) = (0..4).find(|&k| {
            let at = r.pos + k;
            matches!(r.data.get(at..at + 4), Some(b"8BIM" | b"8B64"))
        }) else {
            return false;
        };
        r.pos += skip + 4;
        let Ok(key) = r.array::<4>() else {
            return false;
        };
        let long = big && LONG_KEYS.contains(&&key);
        let Ok(len) = r.length(long) else {
            return false;
        };
        let Ok(body) = r.take(len) else {
            return false;
        };
        if matches!(&key, b"Lr16" | b"Lr32" | b"Layr")
            && body.len() >= 2
            && i16::from_be_bytes([body[0], body[1]]) < 0
        {
            return true;
        }
    }
}

/// 合成图各通道每一行在文件里的位置。
enum Planes<'a> {
    /// 不压缩：通道一个接一个，每个通道 `height` 行，每行 `row_bytes` 字节。
    Raw {
        data: &'a [u8],
        row_bytes: usize,
        height: usize,
    },
    /// PackBits：先是全部通道全部行的压缩后字节数，然后是数据；这里只记要读的通道。
    Rle {
        data: &'a [u8],
        rows: Vec<(usize, usize)>,
        height: usize,
    },
}

impl<'a> Planes<'a> {
    fn locate(
        data: &'a [u8],
        compression: u16,
        channels: usize,
        needed: usize,
        height: usize,
        row_bytes: usize,
        big: bool,
    ) -> Result<Self, Message> {
        match compression {
            0 => {
                let need = needed
                    .checked_mul(height)
                    .and_then(|n| n.checked_mul(row_bytes))
                    .ok_or(TRUNCATED)?;
                if data.len() < need {
                    return Err(TRUNCATED);
                }
                Ok(Planes::Raw {
                    data,
                    row_bytes,
                    height,
                })
            }
            1 => {
                let count_size = if big { 4 } else { 2 };
                let entries = channels.checked_mul(height).ok_or(TRUNCATED)?;
                let table_len = entries.checked_mul(count_size).ok_or(TRUNCATED)?;
                let table = data.get(..table_len).ok_or(TRUNCATED)?;
                let mut rows = Vec::with_capacity(needed * height);
                let mut offset = table_len;
                for (i, count) in table.chunks_exact(count_size).enumerate() {
                    let len = match count {
                        [a, b] => usize::from(u16::from_be_bytes([*a, *b])),
                        [a, b, c, d] => u32::from_be_bytes([*a, *b, *c, *d]) as usize,
                        _ => unreachable!("每项 2 或 4 字节"),
                    };
                    if i / height >= needed {
                        break;
                    }
                    let end = offset
                        .checked_add(len)
                        .filter(|&e| e <= data.len())
                        .ok_or(TRUNCATED)?;
                    rows.push((offset, len));
                    offset = end;
                }
                Ok(Planes::Rle { data, rows, height })
            }
            2 | 3 => Err(msg!("PSD 的合成图用了 ZIP 压缩，暂不支持", "The PSD's composite image uses ZIP compression, which isn't supported yet")),
            other => Err(msg!("不认识的 PSD 压缩方式：{other}", "Unknown PSD compression: {other}", other = format!("{other}"))),
        }
    }

    /// 第 `channel` 个通道的第 `y` 行（解压到 `scratch` 里，或直接借用文件里的字节）。
    fn row<'s>(&'s self, channel: usize, y: usize, scratch: &'s mut [u8]) -> &'s [u8] {
        match self {
            Planes::Raw {
                data,
                row_bytes,
                height,
            } => {
                let at = (channel * height + y) * row_bytes;
                &data[at..at + row_bytes]
            }
            Planes::Rle { data, rows, height } => {
                let (start, len) = rows[channel * height + y];
                unpack_bits(&data[start..start + len], scratch);
                scratch
            }
        }
    }
}

/// PackBits 解一行：数据不够时其余补 0，多出来的忽略。
pub(crate) fn unpack_bits(src: &[u8], out: &mut [u8]) {
    let mut i = 0;
    let mut o = 0;
    while o < out.len() && i < src.len() {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let count = n as usize + 1;
            let take = count.min(out.len() - o).min(src.len() - i);
            out[o..o + take].copy_from_slice(&src[i..i + take]);
            o += take;
            i += count;
        } else if n != -128 {
            let count = (1 - isize::from(n)) as usize;
            let Some(&v) = src.get(i) else {
                break;
            };
            i += 1;
            let take = count.min(out.len() - o);
            out[o..o + take].fill(v);
            o += take;
        }
    }
    out[o..].fill(0);
}

/// 透明度通道的一个值 → 8 位（32 位的透明度是 0–1 的线性值，不做 sRGB 编码）。
fn alpha_sample(row: &[u8], x: usize, depth: u16) -> u8 {
    match depth {
        8 => row[x],
        16 => from16(u16::from_be_bytes([row[2 * x], row[2 * x + 1]])),
        _ => {
            let v =
                f32::from_be_bytes([row[4 * x], row[4 * x + 1], row[4 * x + 2], row[4 * x + 3]]);
            unit_to_u8(v)
        }
    }
}

/// 颜色通道的一个值 → 8 位。`alpha` 不为空时先去掉和白色（`white`）的混合；32 位是线性的，最后做 sRGB 编码。
fn color_sample(row: &[u8], x: usize, depth: u16, alpha: Option<u8>, white: u8) -> u8 {
    match depth {
        8 | 16 => {
            let v = if depth == 8 {
                row[x]
            } else {
                from16(u16::from_be_bytes([row[2 * x], row[2 * x + 1]]))
            };
            match alpha {
                Some(a) if a > 0 && a < 255 => unblend(v, a, white),
                _ => v,
            }
        }
        _ => {
            let mut v =
                f32::from_be_bytes([row[4 * x], row[4 * x + 1], row[4 * x + 2], row[4 * x + 3]]);
            if let Some(a) = alpha.filter(|&a| a > 0 && a < 255) {
                let a = f32::from(a) / 255.0;
                v = (v - (1.0 - a)) / a;
            }
            linear_to_srgb8(v)
        }
    }
}

/// 16 位 → 8 位（四舍五入）。
fn from16(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32767) / 65535) as u8
}

/// 0–1 的值 → 0–255（超出的截掉，NaN 当 0）。
fn unit_to_u8(v: f32) -> u8 {
    if v >= 1.0 {
        255
    } else if v > 0.0 {
        (v * 255.0).round() as u8
    } else {
        0
    }
}

/// 去掉和白色的混合：存的值 = 颜色 × a + 白 × (1 − a)。
fn unblend(stored: u8, alpha: u8, white: u8) -> u8 {
    let a = i32::from(alpha);
    let numerator = i32::from(stored) * 255 - i32::from(white) * (255 - a);
    if numerator <= 0 {
        return 0;
    }
    ((numerator + a / 2) / a).min(255) as u8
}

/// sRGB 编码查表的大小（线性值等分）。
const LUT_SIZE: usize = 1 << 14;

/// 线性亮度（0–1，超出的截掉，NaN 当 0）→ sRGB 8 位。HDR 的亮部直接截到最亮（映射到普通亮度）。
/// DDS 的浮点格式也用它。
pub(crate) fn linear_to_srgb8(x: f32) -> u8 {
    static LUT: OnceLock<Vec<u8>> = OnceLock::new();
    if x >= 1.0 {
        return 255;
    }
    if x.is_nan() || x <= 0.0 {
        return 0;
    }
    let lut = LUT.get_or_init(|| {
        (0..LUT_SIZE)
            .map(|i| {
                let v = i as f64 / (LUT_SIZE - 1) as f64;
                let s = if v <= 0.003_130_8 {
                    12.92 * v
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                };
                (s * 255.0).round().clamp(0.0, 255.0) as u8
            })
            .collect()
    });
    lut[(x * (LUT_SIZE - 1) as f32).round() as usize]
}

/// Lab（8 位：L 0–255 对应 0–100，a、b 减 128）→ sRGB。Lab 的白点是 D50，用 Bradford 转到 sRGB 的 D65。
fn lab_to_srgb(l: u8, a: u8, b: u8) -> [u8; 3] {
    let l = f32::from(l) * (100.0 / 255.0);
    let a = f32::from(a) - 128.0;
    let b = f32::from(b) - 128.0;
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let inverse = |t: f32| {
        const D: f32 = 6.0 / 29.0;
        if t > D {
            t * t * t
        } else {
            3.0 * D * D * (t - 4.0 / 29.0)
        }
    };
    let x = 0.9642 * inverse(fx);
    let y = inverse(fy);
    let z = 0.8251 * inverse(fz);
    let r = 3.133_856 * x - 1.616_867 * y - 0.490_614_6 * z;
    let g = -0.978_768_4 * x + 1.916_141_5 * y + 0.033_454 * z;
    let bl = 0.071_945_3 * x - 0.228_991_4 * y + 1.405_242_7 * z;
    [linear_to_srgb8(r), linear_to_srgb8(g), linear_to_srgb8(bl)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手写一个小 PSD：不压缩，没有资源和图层（`layers` 为图层和蒙版段的内容）。
    fn psd(mode: u16, depth: u16, w: u32, h: u32, planes: &[Vec<u8>], layers: &[u8]) -> Vec<u8> {
        let mut out = b"8BPS".to_vec();
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        out.extend_from_slice(&(planes.len() as u16).to_be_bytes());
        out.extend_from_slice(&h.to_be_bytes());
        out.extend_from_slice(&w.to_be_bytes());
        out.extend_from_slice(&depth.to_be_bytes());
        out.extend_from_slice(&mode.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes()); // 颜色模式数据
        out.extend_from_slice(&0u32.to_be_bytes()); // 图像资源
        out.extend_from_slice(&(layers.len() as u32).to_be_bytes());
        out.extend_from_slice(layers);
        out.extend_from_slice(&0u16.to_be_bytes()); // 不压缩
        for plane in planes {
            out.extend_from_slice(plane);
        }
        out
    }

    fn pixel(raster: &Raster, i: usize) -> [u8; 4] {
        raster.rgba[i * 4..i * 4 + 4].try_into().unwrap()
    }

    /// 给 [`psd`] 做的文件加上一项图像资源（原来的图像资源段是空的）。
    fn with_resource(mut file: Vec<u8>, id: u16, body: &[u8]) -> Vec<u8> {
        let mut block = b"8BIM".to_vec();
        block.extend_from_slice(&id.to_be_bytes());
        block.extend_from_slice(&[0, 0]); // 空名字（长度 0，补齐到偶数）
        block.extend_from_slice(&(body.len() as u32).to_be_bytes());
        block.extend_from_slice(body);
        if body.len() % 2 == 1 {
            block.push(0);
        }
        file[30..34].copy_from_slice(&(block.len() as u32).to_be_bytes());
        file.splice(34..34, block);
        file
    }

    /// 关掉"最大兼容"存的 PSD（资源 1057 写明没有真正的合成图）：合成图层，不用文件里那张空白的合成图。
    #[test]
    fn files_without_real_merged_data_use_their_layers() {
        use crate::psd_layers::tests::{TestLayer, section};
        let red = TestLayer {
            rect: (0, 0, 1, 1),
            channels: vec![(0, vec![255]), (1, vec![0]), (2, vec![0])],
            ..Default::default()
        };
        // 文件里的"合成图"是白色的。
        let file = psd(3, 8, 1, 1, &[vec![255], vec![255], vec![255]], &section(&[red]));
        assert_eq!(pixel(&decode(&file).unwrap(), 0), [255, 255, 255, 255]);
        let mut version = 1u32.to_be_bytes().to_vec();
        version.push(0); // 没有真正的合成图
        let raster = decode(&with_resource(file, 1057, &version)).unwrap();
        assert_eq!(pixel(&raster, 0), [255, 0, 0, 255]);
    }

    /// 灰度、CMYK 带配置文件时按它转成 sRGB；关闭 ICC 或配置文件不对时使用默认转换公式。
    #[test]
    fn gray_and_cmyk_use_their_embedded_profiles() {
        let gray = psd(1, 8, 2, 1, &[vec![128, 255]], &[]);
        let profile = crate::color::linear_gray_test_profile();
        let tagged = with_resource(gray.clone(), 1039, &profile);
        let raster = decode(&tagged).unwrap();
        let mid = pixel(&raster, 0);
        assert!(
            (186..=190).contains(&mid[0]) && mid[0] == mid[1] && mid[1] == mid[2],
            "{mid:?}"
        );
        assert_eq!(pixel(&raster, 1), [255, 255, 255, 255]);
        assert_eq!(raster.icc_profile, None);
        // 设置里关掉"使用图片文件的 ICC"：原样。
        assert_eq!(
            pixel(&decode_with(&tagged, false).unwrap(), 0),
            [128, 128, 128, 255]
        );
        // CMYK 文件带的不是 CMYK 配置文件：仍用简单公式。
        let cmyk = psd(4, 8, 1, 1, &[vec![255], vec![0], vec![200], vec![128]], &[]);
        let raster = decode(&with_resource(cmyk, 1039, &profile)).unwrap();
        assert_eq!(pixel(&raster, 0), [128, 0, 100, 255]);
    }

    #[test]
    fn packbits_handles_runs_literals_and_short_data() {
        let mut out = [9u8; 8];
        // 字面 3 个、重复 4 个、空操作（-128）、再字面 1 个。
        unpack_bits(&[2, 1, 2, 3, 0xFD, 7, 0x80, 0, 5], &mut out);
        assert_eq!(out, [1, 2, 3, 7, 7, 7, 7, 5]);
        // 数据不够：其余补 0。
        unpack_bits(&[0xFE, 4], &mut out);
        assert_eq!(out, [4, 4, 4, 0, 0, 0, 0, 0]);
        // 字面数据说要 5 个、只剩 2 个：不越界。
        unpack_bits(&[4, 1, 2], &mut out);
        assert_eq!(out, [1, 2, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn lab_and_cmyk_become_srgb() {
        // Lab：L 100 是白、L 0 是黑；Lab(53, 80, 67) 约等于 sRGB 红。
        assert_eq!(lab_to_srgb(255, 128, 128), [255, 255, 255]);
        assert_eq!(lab_to_srgb(0, 128, 128), [0, 0, 0]);
        let [r, g, b] = lab_to_srgb(136, 208, 195);
        assert!(r >= 245 && g <= 30 && b <= 30, "{r} {g} {b}");
        // 2x1 的 Lab 文件。
        let lab = psd(
            9,
            8,
            2,
            1,
            &[vec![255, 0], vec![128, 128], vec![128, 128]],
            &[],
        );
        let raster = decode(&lab).unwrap();
        assert_eq!(pixel(&raster, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&raster, 1), [0, 0, 0, 255]);
        assert_eq!(raster.pixel_format, "Lab24");
        // CMYK：存的是反过来的值；K' 128 让颜色减半。
        let cmyk = psd(4, 8, 1, 1, &[vec![255], vec![0], vec![200], vec![128]], &[]);
        let raster = decode(&cmyk).unwrap();
        assert_eq!(pixel(&raster, 0), [128, 0, 100, 255]);
        assert_eq!(
            (raster.pixel_format.as_str(), raster.has_alpha),
            ("CMYK32", false)
        );
    }

    #[test]
    fn gray_16_and_32_bit_samples() {
        let gray16 = psd(1, 16, 2, 1, &[vec![0xFF, 0xFF, 0x80, 0x80]], &[]);
        let raster = decode(&gray16).unwrap();
        assert_eq!(pixel(&raster, 0), [255, 255, 255, 255]);
        assert_eq!(pixel(&raster, 1), [128, 128, 128, 255]);
        assert_eq!(raster.pixel_format, "Gray16");
        // 32 位是线性值：0.5 → sRGB 188；超过 1 的截到 255。
        let mut plane = 0.5f32.to_be_bytes().to_vec();
        plane.extend_from_slice(&4.0f32.to_be_bytes());
        let raster = decode(&psd(1, 32, 2, 1, &[plane], &[])).unwrap();
        assert_eq!(pixel(&raster, 0)[0], 188);
        assert_eq!(pixel(&raster, 1)[0], 255);
        assert_eq!(raster.pixel_format, "Gray32F");
    }

    #[test]
    fn negative_layer_count_turns_the_extra_channel_into_alpha() {
        // 灰度 + 一个额外通道。图层数为 0：额外通道是选区，不透明。
        let planes = [vec![100], vec![0]];
        let mut layers = 2u32.to_be_bytes().to_vec();
        layers.extend_from_slice(&0i16.to_be_bytes());
        layers.extend_from_slice(&0u32.to_be_bytes());
        let raster = decode(&psd(1, 8, 1, 1, &planes, &layers)).unwrap();
        assert_eq!(pixel(&raster, 0), [100, 100, 100, 255]);
        assert!(!raster.has_alpha);
        // 图层数为负：额外通道是透明度；颜色和白色混合过，按 Alpha 还原。
        let stored = (100u32 * 128 + 255 * 127) / 255; // 和白色混合后存的值
        let planes = [vec![stored as u8], vec![128]];
        let mut layers = 2u32.to_be_bytes().to_vec();
        layers.extend_from_slice(&(-1i16).to_be_bytes());
        layers.extend_from_slice(&0u32.to_be_bytes());
        let raster = decode(&psd(1, 8, 1, 1, &planes, &layers)).unwrap();
        let [g, _, _, a] = pixel(&raster, 0);
        assert_eq!(a, 128);
        assert!(g.abs_diff(100) <= 1, "{g}");
        assert!(raster.has_alpha);
        assert_eq!(raster.pixel_format, "GrayA16");
        // 16 位文件：主图层信息为空，图层数写在附加信息块 Lr16 里（块之间补齐到 4 字节）。
        let mut layers = 0u32.to_be_bytes().to_vec(); // 图层信息为空
        layers.extend_from_slice(&0u32.to_be_bytes()); // 全局图层蒙版
        layers.extend_from_slice(b"8BIMlnsr");
        layers.extend_from_slice(&3u32.to_be_bytes());
        layers.extend_from_slice(b"abc\0"); // 3 字节内容，补齐 1 字节
        layers.extend_from_slice(b"8BIMLr16");
        layers.extend_from_slice(&2u32.to_be_bytes());
        layers.extend_from_slice(&(-1i16).to_be_bytes());
        let raster = decode(&psd(1, 8, 1, 1, &[vec![255], vec![0]], &layers)).unwrap();
        assert_eq!(pixel(&raster, 0)[3], 0);
        assert!(raster.has_alpha);
    }

    #[test]
    fn bad_headers_are_errors_not_panics() {
        let good = psd(3, 8, 2, 2, &[vec![1; 4], vec![2; 4], vec![3; 4]], &[]);
        assert!(decode(&good).is_ok());
        for len in 0..good.len() {
            let _ = decode(&good[..len]);
        }
        let mut bad = good.clone();
        bad[4..6].copy_from_slice(&3u16.to_be_bytes()); // 版本 3
        assert!(decode(&bad).is_err());
        let mut bad = good.clone();
        bad[22..24].copy_from_slice(&1u16.to_be_bytes()); // RGB 1 位
        assert!(decode(&bad).is_err());
        let mut bad = good.clone();
        bad[12..14].copy_from_slice(&2u16.to_be_bytes()); // RGB 只有 2 个通道
        assert!(decode(&bad).is_err());
        assert!(decode(b"8BPS").is_err());
        assert!(decode(b"not a psd at all, just text").is_err());
        // ZIP 压缩：报错。
        let mut zip = good.clone();
        let at = zip.len() - 12 - 2;
        zip[at..at + 2].copy_from_slice(&2u16.to_be_bytes());
        assert!(decode(&zip).unwrap_err().to_string().contains("ZIP"));
    }

    #[test]
    fn linear_values_are_srgb_encoded() {
        assert_eq!(linear_to_srgb8(0.0), 0);
        assert_eq!(linear_to_srgb8(f32::NAN), 0);
        assert_eq!(linear_to_srgb8(-3.0), 0);
        assert_eq!(linear_to_srgb8(1.0), 255);
        assert_eq!(linear_to_srgb8(f32::INFINITY), 255);
        assert_eq!(linear_to_srgb8(0.215_861), 128);
        assert_eq!(linear_to_srgb8(0.5), 188);
    }
}
