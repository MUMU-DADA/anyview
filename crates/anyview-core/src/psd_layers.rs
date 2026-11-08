//! PSD 的图层合成：文件里没有真正的合成图（Photoshop 关掉"最大兼容"存的，资源 1057 写明）
//! 或者合成图的数据缺了时，自己把图层叠出来，只为看图，不提供图层查看。
//!
//! 支持：显示和隐藏、不透明度、填充不透明度、图层蒙版（含默认色、停用）、剪贴蒙版、图层组（含"穿透"）、
//! Photoshop 的各种混合模式（色相、饱和度、颜色、明度只在 RGB 里，其他颜色模式按"正常"）；
//! 图层数据不压缩、PackBits、ZIP、带预测的 ZIP；8、16、32 位。纯色填充图层（SoCo）按它的颜色填满。
//! 矢量蒙版（形状图层）用 tiny-skia 光栅化，子路径的合并、减去、相交、排除照 psd-tools（MIT）的做法。
//! 不支持（文件里没有像素）：调整图层、图层样式（投影、描边等）。
//!
//! 在文档的通道空间里合成（RGB、灰度、Lab，CMYK 按 PSD 存的"反过来的值"），0–1 的浮点数、预乘。
//! 32 位的线性值先按 sRGB 编码再合成（与读合成图时一样，只为看图）。

use std::io::Read;

/// 合成结果：颜色通道放法与读合成图时相同（RGB、Lab 依次放进 R、G、B；CMYK 的 C、M、Y 放进 R、G、B，
/// K 在 `black`；单通道的放进 R），Alpha 不预乘。
pub(crate) struct Composite {
    pub rgba: Vec<u8>,
    pub black: Vec<u8>,
    pub has_alpha: bool,
}

/// 合成的画布最多占这么多内存（每像素 5 个 f32）；更大的文件不合成。
const MAX_CANVAS_BYTES: usize = 1 << 30;

struct Rect {
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
}

impl Rect {
    fn width(&self) -> usize {
        (self.right - self.left).max(0) as usize
    }
    fn height(&self) -> usize {
        (self.bottom - self.top).max(0) as usize
    }
}

struct Mask {
    rect: Rect,
    default: u8,
    data: Vec<u8>,
}

/// 像素蒙版（−2 通道）的参数。文件里另有的"真实"蒙版（−3 通道）是像素蒙版和矢量蒙版合在一起的结果，
/// 我们自己画矢量蒙版，所以不用它（psd-tools 同样分开处理）。
struct MaskInfo {
    rect: Rect,
    default: u8,
    enabled: bool,
    /// 密度（0–1）：低于 1 时蒙版变淡。
    density: f32,
    /// 矢量蒙版的密度。
    vector_density: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Blend {
    Normal,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
    PassThrough,
}

impl Blend {
    fn from_key(key: &[u8; 4]) -> Self {
        match key {
            b"dark" => Blend::Darken,
            b"mul " => Blend::Multiply,
            b"idiv" => Blend::ColorBurn,
            b"lbrn" => Blend::LinearBurn,
            b"dkCl" => Blend::DarkerColor,
            b"lite" => Blend::Lighten,
            b"scrn" => Blend::Screen,
            b"div " => Blend::ColorDodge,
            b"lddg" => Blend::LinearDodge,
            b"lgCl" => Blend::LighterColor,
            b"over" => Blend::Overlay,
            b"sLit" => Blend::SoftLight,
            b"hLit" => Blend::HardLight,
            b"vLit" => Blend::VividLight,
            b"lLit" => Blend::LinearLight,
            b"pLit" => Blend::PinLight,
            b"hMix" => Blend::HardMix,
            b"diff" => Blend::Difference,
            b"smud" => Blend::Exclusion,
            b"fsub" => Blend::Subtract,
            b"fdiv" => Blend::Divide,
            b"hue " => Blend::Hue,
            b"sat " => Blend::Saturation,
            b"colr" => Blend::Color,
            b"lum " => Blend::Luminosity,
            b"pass" => Blend::PassThrough,
            // norm、diss（溶解按正常）和认不出的
            _ => Blend::Normal,
        }
    }

    fn non_separable(self) -> bool {
        matches!(
            self,
            Blend::Hue
                | Blend::Saturation
                | Blend::Color
                | Blend::Luminosity
                | Blend::DarkerColor
                | Blend::LighterColor
        )
    }
}

/// 一个图层：像素（文档坐标的矩形里，颜色 0–1，Alpha 0–1）和合成参数。
struct Layer {
    rect: Rect,
    /// 每个颜色通道一份，长度 = 矩形面积；没有的通道为空。
    colors: Vec<Vec<f32>>,
    alpha: Option<Vec<f32>>,
    /// 纯色填充图层的颜色（文档通道空间，0–1）。
    fill: Option<Vec<f32>>,
    mask: Option<Mask>,
    /// 矢量蒙版，已光栅化成文档大小（0–255）。
    vector: Option<Vec<u8>>,
    blend: Blend,
    opacity: f32,
    fill_opacity: f32,
    clipped: bool,
    visible: bool,
    /// 0：普通图层；1：图层组（文件里在组的上面）；2：组的结束标记（在组的下面）。
    kind: u8,
}

enum Node {
    Layer(Box<Layer>),
    Group {
        group: Box<Layer>,
        children: Vec<Node>,
    },
}

/// 合成图层。`section` 是整个"图层和蒙版信息"段；文件里没有图层、数据坏了或太大时返回 None。
pub(crate) fn composite(
    section: &[u8],
    big: bool,
    depth: u16,
    color: usize,
    rgb: bool,
    width: u32,
    height: u32,
) -> Option<Composite> {
    let (w, h) = (width as usize, height as usize);
    // 生成缩略图时上限更小（decode::alloc_limit）。
    // 用户选了"仍然打开"的不设上限。因为上限不合成的记一笔（decode::limit_hit），调用方据此报错、
    // 让界面问用户，而不是悄悄显示文件里那张空白的合成图。
    let max_canvas = if crate::decode::limits_lifted() {
        usize::MAX
    } else {
        MAX_CANVAS_BYTES.min(usize::try_from(crate::decode::alloc_limit()).unwrap_or(usize::MAX))
    };
    if w.checked_mul(h)?.checked_mul((color + 1) * 4)? > max_canvas {
        crate::decode::limit_hit();
        return None;
    }
    let info = layer_info(section, big)?;
    // 所有图层的像素、蒙版加起来的上限（解码内存上限的 2 倍，默认 4 GB）。几百 KB 的坏文件可以写
    // 一万个图层、每个通道几个字节就"解"出几百万个像素：不设总量的话要几百 GB，程序被系统直接杀掉。
    let budget = usize::try_from(crate::decode::alloc_limit().saturating_mul(2)).unwrap_or(usize::MAX);
    let layers = parse_layers(info, big, depth, color, w, h, budget)?;
    if layers.is_empty() {
        return None;
    }
    let (tree, nesting) = build_tree(layers)?;
    // 每套一层图层组，合成时多一张整幅的画面。
    if (w * h * (color + 1) * 4).checked_mul(nesting + 1)? > budget {
        crate::decode::limit_hit();
        return None;
    }
    let ctx = Ctx { w, h, color, rgb };
    let mut canvas = vec![0f32; w * h * (color + 1)];
    ctx.render(&tree, &mut canvas);

    let mut rgba = vec![0u8; w * h * 4];
    let mut black = if color == 4 {
        vec![0u8; w * h]
    } else {
        Vec::new()
    };
    let mut has_alpha = false;
    let n = color + 1;
    for (i, px) in canvas.chunks_exact(n).enumerate() {
        let a = px[color].clamp(0.0, 1.0);
        let out = &mut rgba[i * 4..i * 4 + 4];
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        // 透明处的颜色：CMYK 按"没有油墨"（255），其他按白色。
        for c in 0..color {
            let v = if a > 0.0 { px[c] / a } else { 1.0 };
            if c < 3 {
                out[c] = to8(v);
            } else {
                black[i] = to8(v);
            }
        }
        out[3] = to8(a);
        has_alpha |= out[3] < 255;
    }
    Some(Composite {
        rgba,
        black,
        has_alpha,
    })
}

/// "图层和蒙版信息"段里的图层信息（16、32 位文件在附加信息块 Lr16、Lr32 里）。
fn layer_info(section: &[u8], big: bool) -> Option<&[u8]> {
    let mut r = Cursor::new(section);
    let info_len = r.length(big)?;
    if info_len >= 2 {
        return r.take(info_len);
    }
    r.take(info_len)?;
    let mask_len = r.u32()? as usize;
    r.take(mask_len)?;
    loop {
        let skip = (0..4).find(|&k| {
            matches!(
                section.get(r.pos + k..r.pos + k + 4),
                Some(b"8BIM" | b"8B64")
            )
        })?;
        r.pos += skip + 4;
        let key = r.array::<4>()?;
        let long = big && super::psd::is_long_key(&key);
        let len = r.length(long)?;
        let body = r.take(len)?;
        if matches!(&key, b"Lr16" | b"Lr32" | b"Layr") {
            return Some(body);
        }
    }
}

struct Record {
    rect: Rect,
    channels: Vec<(i16, usize)>,
    blend: Blend,
    opacity: u8,
    clipped: bool,
    visible: bool,
    mask: Option<MaskInfo>,
    fill_opacity: u8,
    /// 图层组：1、2 是组的开始（文件里在组的上面），3 是组的结束标记（在组的下面）。
    divider: u32,
    divider_blend: Option<Blend>,
    fill: Option<Vec<f64>>,
    /// 矢量蒙版的原始数据（vmsk、vsms）。
    vector: Option<Vec<u8>>,
}

/// 从总量里扣掉 `bytes`；不够了记一笔（被上限拦下），返回空。
fn spend(budget: usize, bytes: usize) -> Option<usize> {
    let left = budget.checked_sub(bytes);
    if left.is_none() {
        crate::decode::limit_hit();
    }
    left
}

fn parse_layers(
    info: &[u8],
    big: bool,
    depth: u16,
    color: usize,
    w: usize,
    h: usize,
    mut budget: usize,
) -> Option<Vec<Layer>> {
    let mut r = Cursor::new(info);
    let count = r.i16()?.unsigned_abs() as usize;
    if count > 10_000 {
        return None;
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(parse_record(&mut r, big)?);
    }
    let mut layers = Vec::with_capacity(count);
    for record in records {
        let mut colors = vec![Vec::new(); color];
        let mut alpha = None;
        let mut mask_data = None;
        for &(id, len) in &record.channels {
            let data = r.take(len)?;
            let (cw, ch) = match id {
                -2 => match &record.mask {
                    Some(m) => (m.rect.width(), m.rect.height()),
                    None => continue,
                },
                -3 => continue,
                _ => (record.rect.width(), record.rect.height()),
            };
            // 坏文件可能写一个极大的矩形：图层可以超出画布，但不会大过画布很多。
            let limit = (w * h).saturating_mul(4).clamp(1 << 20, 1 << 28);
            if cw == 0 || ch == 0 || cw.saturating_mul(ch) > limit {
                continue;
            }
            // 总量超了：不合成（调用方改用别的办法或报错），不是跳过这一个通道接着吃内存。
            budget = spend(budget, cw * ch * 4)?;
            let Some(samples) = channel_samples(data, cw, ch, depth, big, id >= 0) else {
                continue;
            };
            match id {
                -1 => alpha = Some(samples),
                -2 => mask_data = Some(samples),
                c if c >= 0 && (c as usize) < color => colors[c as usize] = samples,
                _ => {}
            }
        }
        let vector_density = record.mask.as_ref().map_or(1.0, |m| m.vector_density);
        let mask = match (record.mask, mask_data) {
            (Some(info), Some(data)) if info.enabled => {
                let d = info.density;
                let dense = |v: f32| ((1.0 - d * (1.0 - v)) * 255.0 + 0.5) as u8;
                Some(Mask {
                    rect: info.rect,
                    default: dense(f32::from(info.default) / 255.0),
                    data: data.into_iter().map(dense).collect(),
                })
            }
            _ => None,
        };
        // 形状图层也是纯色填充，但文件里存了画好的像素（透明度就是形状）：有像素时用像素。
        let has_pixels = colors.iter().any(|c| !c.is_empty());
        let fill = record
            .fill
            .filter(|_| !has_pixels)
            .map(|c| fill_color(&c, color));
        if record.vector.is_some() {
            budget = spend(budget, w * h)?;
        }
        let vector = record
            .vector
            .as_deref()
            .and_then(|v| vector_mask(v, w, h))
            .map(|v| {
                v.into_iter()
                    .map(|m| {
                        ((1.0 - vector_density * (1.0 - f32::from(m) / 255.0)) * 255.0 + 0.5) as u8
                    })
                    .collect()
            });
        layers.push((
            record.divider,
            record.divider_blend,
            Layer {
                rect: record.rect,
                colors,
                alpha,
                fill,
                mask,
                vector,
                blend: record.blend,
                opacity: f32::from(record.opacity) / 255.0,
                fill_opacity: f32::from(record.fill_opacity) / 255.0,
                clipped: record.clipped,
                visible: record.visible,
                kind: match record.divider {
                    1 | 2 => 1,
                    3 => 2,
                    _ => 0,
                },
            },
        ));
    }
    // 组的混合模式写在分隔块里（"穿透"只在那里）。
    Some(
        layers
            .into_iter()
            .map(|(divider, divider_blend, mut layer)| {
                if divider == 1 || divider == 2 {
                    layer.blend = divider_blend.unwrap_or(layer.blend);
                    layer.colors.clear();
                    layer.alpha = None;
                    layer.fill = None;
                }
                layer
            })
            .collect(),
    )
}

fn parse_record(r: &mut Cursor<'_>, big: bool) -> Option<Record> {
    let rect = Rect {
        top: r.i32()?,
        left: r.i32()?,
        bottom: r.i32()?,
        right: r.i32()?,
    };
    let n = r.u16()? as usize;
    if n > 64 {
        return None;
    }
    let mut channels = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.i16()?;
        let len = r.length(big)?;
        channels.push((id, len));
    }
    if r.take(4)? != b"8BIM" {
        return None;
    }
    let blend = Blend::from_key(&r.array::<4>()?);
    let opacity = r.u8()?;
    let clipped = r.u8()? == 1;
    let flags = r.u8()?;
    r.u8()?;
    let extra_len = r.u32()? as usize;
    let extra = r.take(extra_len)?;
    let mut x = Cursor::new(extra);
    // 图层蒙版。
    let mask_len = x.u32()? as usize;
    let mask_block = x.take(mask_len)?;
    let mask = parse_mask(mask_block, channels.iter().any(|&(id, _)| id == -3));
    // 混合范围、名字。
    let ranges = x.u32()? as usize;
    x.take(ranges)?;
    let name_len = x.u8()? as usize;
    let padded = (name_len + 1).div_ceil(4) * 4 - 1;
    x.take(padded)?;
    let mut record = Record {
        rect,
        channels,
        blend,
        opacity,
        clipped,
        visible: flags & 0x02 == 0,
        mask,
        fill_opacity: 255,
        divider: 0,
        divider_blend: None,
        fill: None,
        vector: None,
    };
    // 附加信息块。
    while x.pos + 12 <= extra.len() {
        let Some(skip) = (0..4)
            .find(|&k| matches!(extra.get(x.pos + k..x.pos + k + 4), Some(b"8BIM" | b"8B64")))
        else {
            break;
        };
        x.pos += skip + 4;
        let key = x.array::<4>()?;
        let long = big && super::psd::is_long_key(&key);
        let len = x.length(long)?;
        let Some(body) = x.take(len) else { break };
        match &key {
            b"lsct" | b"lsdk" if body.len() >= 4 => {
                record.divider = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                if body.len() >= 12 && &body[4..8] == b"8BIM" {
                    record.divider_blend =
                        Some(Blend::from_key(&[body[8], body[9], body[10], body[11]]));
                }
            }
            b"iOpa" if !body.is_empty() => record.fill_opacity = body[0],
            b"SoCo" => record.fill = solid_color(body),
            b"vmsk" | b"vsms" => record.vector = Some(body.to_vec()),
            _ => {}
        }
    }
    Some(record)
}

/// 像素蒙版：矩形、默认色、标志，有 −3 通道时接着是"真实"蒙版的字段（跳过），最后是参数（密度、羽化）。
/// 顺序照 psd-tools（Adobe 的说明在这里写得不对）。
fn parse_mask(block: &[u8], has_real: bool) -> Option<MaskInfo> {
    if block.len() < 18 {
        return None;
    }
    let mut m = Cursor::new(block);
    let rect = Rect {
        top: m.i32()?,
        left: m.i32()?,
        bottom: m.i32()?,
        right: m.i32()?,
    };
    let default = m.u8()?;
    let flags = m.u8()?;
    if has_real && block.len() >= 36 {
        m.take(18)?;
    }
    let (mut density, mut vector_density) = (1.0, 1.0);
    if flags & 0x10 != 0 {
        let params = m.u8()?;
        if params & 0x01 != 0 {
            density = f32::from(m.u8()?) / 255.0;
        }
        if params & 0x02 != 0 {
            m.take(8)?;
        }
        if params & 0x04 != 0 {
            vector_density = f32::from(m.u8()?) / 255.0;
        }
    }
    Some(MaskInfo {
        rect,
        default,
        enabled: flags & 0x02 == 0,
        density,
        vector_density,
    })
}

/// 矢量蒙版（vmsk、vsms）光栅化成文档大小的蒙版（0–255）；停用、读不出时为空。
/// 26 字节一条记录：子路径长度（带合并方式）、贝塞尔节点（前控制点、锚点、后控制点，8.24 定点的 y、x，
/// 相对文档大小）、初始填充规则。合并方式照 psd-tools：1 合并、2 减去、3 相交、0 排除，-1 并入上一个组件。
/// 矢量蒙版的工作量上限：组件数 × 文档像素数（2^33：约对应 1600 万像素文档 512 个组件）。
const MAX_MASK_WORK: usize = 1 << 33;

fn vector_mask(body: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    use tiny_skia::{FillRule, Mask as SkMask, PathBuilder, Transform};
    let flags = u32::from_be_bytes(body.get(4..8)?.try_into().ok()?);
    if flags & 0x04 != 0 {
        return None;
    }
    struct Sub {
        operation: i16,
        closed: bool,
        knots: Vec<[f32; 6]>,
    }
    let mut subs: Vec<Sub> = Vec::new();
    let mut initial_fill = false;
    let fixed = |b: &[u8]| i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 / 16_777_216.0;
    for record in body.get(8..)?.as_chunks::<26>().0 {
        let selector = u16::from_be_bytes([record[0], record[1]]);
        match selector {
            0 | 3 => subs.push(Sub {
                operation: i16::from_be_bytes([record[4], record[5]]),
                closed: selector == 0,
                knots: Vec::new(),
            }),
            1 | 2 | 4 | 5 => {
                let sub = subs.last_mut()?;
                let v: Vec<f32> = record[2..26]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| fixed(b))
                    .collect();
                // y、x 依次：前控制点、锚点、后控制点；换成像素坐标 (x, y)。
                sub.knots.push([
                    v[1] * w as f32,
                    v[0] * h as f32,
                    v[3] * w as f32,
                    v[2] * h as f32,
                    v[5] * w as f32,
                    v[4] * h as f32,
                ]);
            }
            8 => initial_fill = u16::from_be_bytes([record[2], record[3]]) == 1,
            _ => {}
        }
    }
    let n = w * h;
    let mut mask = vec![
        if initial_fill && subs.is_empty() {
            1f32
        } else {
            0f32
        };
        n
    ];
    // 把子路径按组件分组（合并方式 −1 并入上一个）。
    let mut components: Vec<Vec<&Sub>> = Vec::new();
    for sub in &subs {
        match components.last_mut() {
            Some(last) if sub.operation == -1 => last.push(sub),
            _ => components.push(vec![sub]),
        }
    }
    // 每个组件要画一张整幅的蒙版、再整幅合并一遍：组件数 × 像素数太大的（几 MB 的坏文件能写十万个空组件，
    // 让解码线程空转几十分钟）不做，这个图层按没有矢量蒙版显示。
    if components.len().saturating_mul(n) > MAX_MASK_WORK {
        return None;
    }
    let mut first = true;
    for component in &components {
        let mut pb = PathBuilder::new();
        for sub in component {
            let Some(k0) = sub.knots.first() else {
                continue;
            };
            pb.move_to(k0[2], k0[3]);
            let count = sub.knots.len();
            let segments = if sub.closed { count } else { count - 1 };
            for i in 0..segments {
                let (a, b) = (&sub.knots[i], &sub.knots[(i + 1) % count]);
                pb.cubic_to(a[4], a[5], b[0], b[1], b[2], b[3]);
            }
            if sub.closed {
                pb.close();
            }
        }
        let mut plane = SkMask::new(w as u32, h as u32)?;
        if let Some(path) = pb.finish() {
            plane.fill_path(&path, FillRule::Winding, true, Transform::identity());
        }
        let operation = component[0].operation;
        for (m, &p) in mask.iter_mut().zip(plane.data()) {
            let p = f32::from(p) / 255.0;
            *m = match operation {
                0 => *m + p - 2.0 * *m * p,
                2 => {
                    let base = if first { 1.0 - *m } else { *m };
                    (base - p).max(0.0)
                }
                3 => {
                    let base = if first { 1.0 - *m } else { *m };
                    base * p
                }
                _ => *m + p - *m * p,
            };
        }
        first = false;
    }
    let invert = flags & 0x01 != 0;
    Some(
        mask.into_iter()
            .map(|m| {
                let m = if invert { 1.0 - m } else { m };
                (m.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            })
            .collect(),
    )
}

/// 纯色填充（SoCo）：描述符里的 RGB（"Rd  "、"Grn "、"Bl  "，0–255 的双精度数）；认不出时为空。
fn solid_color(body: &[u8]) -> Option<Vec<f64>> {
    let mut out = Vec::new();
    for key in [b"Rd  ", b"Grn ", b"Bl  "] {
        let at = body
            .windows(8)
            .position(|w| &w[..4] == key && &w[4..8] == b"doub")?;
        let v = body.get(at + 8..at + 16)?;
        out.push(f64::from_be_bytes(v.try_into().ok()?));
    }
    Some(out)
}

/// 纯色填充的颜色换到文档的通道空间（RGB 原样；灰度取亮度；其他模式认不出，按灰度近似）。
fn fill_color(rgb: &[f64], color: usize) -> Vec<f32> {
    let [r, g, b] = [rgb[0] / 255.0, rgb[1] / 255.0, rgb[2] / 255.0];
    match color {
        3 => vec![r as f32, g as f32, b as f32],
        // CMYK 存的是反过来的值：近似为 C' = R、M' = G、Y' = B，不用黑色。
        4 => vec![r as f32, g as f32, b as f32, 1.0],
        _ => vec![(0.299 * r + 0.587 * g + 0.114 * b) as f32; color],
    }
}

/// 一个通道的数据（压缩方式 + 数据）解成 0–1 的数。32 位的颜色按 sRGB 编码。
fn channel_samples(
    data: &[u8],
    w: usize,
    h: usize,
    depth: u16,
    big: bool,
    is_color: bool,
) -> Option<Vec<f32>> {
    if data.len() < 2 {
        return None;
    }
    let compression = u16::from_be_bytes([data[0], data[1]]);
    let body = &data[2..];
    let bytes = usize::from(depth / 8).max(1);
    let row = w * bytes;
    let total = row.checked_mul(h)?;
    // 压缩的数据再怎么压也有个限度（PackBits 2 个字节最多变 128 个，zlib 约 1032 倍）：几个字节声称能解出
    // 几百万个像素的是坏文件，不为它分配内存。
    let most = match compression {
        1 => body.len().saturating_mul(64),
        2 | 3 => body.len().saturating_mul(1100),
        _ => usize::MAX,
    };
    if total > most {
        return None;
    }
    let raw: Vec<u8> = match compression {
        0 => body.get(..total)?.to_vec(),
        1 => {
            let count_size = if big { 4 } else { 2 };
            let counts = body.get(..h * count_size)?;
            let mut pos = h * count_size;
            let mut out = vec![0u8; total];
            for y in 0..h {
                let c = &counts[y * count_size..y * count_size + count_size];
                let n = if big {
                    u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as usize
                } else {
                    usize::from(u16::from_be_bytes([c[0], c[1]]))
                };
                let src = body.get(pos..pos + n)?;
                super::psd::unpack_bits(src, &mut out[y * row..(y + 1) * row]);
                pos += n;
            }
            out
        }
        2 | 3 => {
            let mut out = Vec::with_capacity(total);
            flate2::read::ZlibDecoder::new(body)
                .take(total as u64)
                .read_to_end(&mut out)
                .ok()?;
            out.resize(total, 0);
            if compression == 3 {
                unpredict(&mut out, w, h, depth);
            }
            out
        }
        _ => return None,
    };
    let mut samples = Vec::with_capacity(w * h);
    match depth {
        8 => samples.extend(raw.iter().map(|&v| f32::from(v) / 255.0)),
        16 => samples.extend(
            raw.as_chunks::<2>()
                .0
                .iter()
                .map(|p| f32::from(u16::from_be_bytes([p[0], p[1]])) / 65535.0),
        ),
        32 => samples.extend(raw.as_chunks::<4>().0.iter().map(|p| {
            let v = f32::from_be_bytes([p[0], p[1], p[2], p[3]]);
            if is_color {
                f32::from(super::psd::linear_to_srgb8(v)) / 255.0
            } else {
                v.clamp(0.0, 1.0)
            }
        })),
        _ => return None,
    }
    Some(samples)
}

/// 带预测的 ZIP：每行存的是与前一个值的差。32 位的每行先按字节平面重排过（高位字节在前）。
fn unpredict(data: &mut [u8], w: usize, h: usize, depth: u16) {
    match depth {
        8 => {
            for row in data.chunks_exact_mut(w) {
                for x in 1..w {
                    row[x] = row[x].wrapping_add(row[x - 1]);
                }
            }
        }
        16 => {
            for row in data.chunks_exact_mut(w * 2) {
                for x in 1..w {
                    let prev = u16::from_be_bytes([row[x * 2 - 2], row[x * 2 - 1]]);
                    let cur = u16::from_be_bytes([row[x * 2], row[x * 2 + 1]]).wrapping_add(prev);
                    row[x * 2..x * 2 + 2].copy_from_slice(&cur.to_be_bytes());
                }
            }
        }
        32 => {
            let n = w * 4;
            let mut planes = vec![0u8; n];
            for y in 0..h {
                let row = &mut data[y * n..(y + 1) * n];
                for i in 1..n {
                    row[i] = row[i].wrapping_add(row[i - 1]);
                }
                planes.copy_from_slice(row);
                for x in 0..w {
                    for b in 0..4 {
                        row[x * 4 + b] = planes[b * w + x];
                    }
                }
            }
        }
        _ => {}
    }
}

/// 图层是自下而上排的：组的结束标记在组的最下面，组本身的记录在最上面。
/// 图层组最多套这么多层（Photoshop 自己最多 10 层）：每套一层，合成时多一张整幅的画面、多一层递归。
const MAX_NESTING: usize = 10;

/// 按组的开始、结束标记把图层排成树；返回树和组套了几层。套得太深的（坏文件）为空。
fn build_tree(layers: Vec<Layer>) -> Option<(Vec<Node>, usize)> {
    let mut stack: Vec<Vec<Node>> = vec![Vec::new()];
    let mut nesting = 0;
    for layer in layers {
        if layer.kind == 2 {
            if stack.len() > MAX_NESTING {
                return None;
            }
            nesting = nesting.max(stack.len());
            stack.push(Vec::new());
        } else if layer.kind == 1 {
            let children = if stack.len() > 1 {
                stack.pop().unwrap_or_default()
            } else {
                Vec::new()
            };
            stack.last_mut().expect("至少有顶层").push(Node::Group {
                group: Box::new(layer),
                children,
            });
        } else {
            stack
                .last_mut()
                .expect("至少有顶层")
                .push(Node::Layer(Box::new(layer)));
        }
    }
    // 结束标记多于组时（坏文件）把剩下的都并到顶层。
    while stack.len() > 1 {
        let rest = stack.pop().unwrap_or_default();
        stack.last_mut().expect("至少有顶层").extend(rest);
    }
    Some((stack.pop().unwrap_or_default(), nesting))
}

struct Ctx {
    w: usize,
    h: usize,
    color: usize,
    rgb: bool,
}

impl Ctx {
    /// 把一串（自下而上的）节点合成到 `canvas`（预乘）上。
    fn render(&self, nodes: &[Node], canvas: &mut [f32]) {
        // 剪贴蒙版：被剪贴的图层只在最近一个不剪贴的图层（基底）有像素的地方显示。
        let mut base: Option<&Layer> = None;
        for node in nodes {
            match node {
                Node::Layer(layer) => {
                    if layer.clipped {
                        if let Some(base) = base
                            && layer.visible
                            && base.visible
                        {
                            self.draw_layer(layer, canvas, Some(base));
                        }
                        continue;
                    }
                    base = Some(layer);
                    if layer.visible {
                        self.draw_layer(layer, canvas, None);
                    }
                }
                Node::Group { group, children } => {
                    base = None;
                    if !group.visible {
                        continue;
                    }
                    let n = self.color + 1;
                    if group.blend == Blend::PassThrough {
                        // 穿透：组里的图层直接与下面的画面混合，组的不透明度和蒙版在前后两个画面之间插值。
                        let mut inside = canvas.to_vec();
                        self.render(children, &mut inside);
                        for i in 0..self.w * self.h {
                            let k = group.opacity
                                * group.fill_opacity
                                * self.mask_at(group, i % self.w, i / self.w);
                            for c in 0..n {
                                let j = i * n + c;
                                canvas[j] += (inside[j] - canvas[j]) * k;
                            }
                        }
                    } else {
                        let mut inside = vec![0f32; canvas.len()];
                        self.render(children, &mut inside);
                        for i in 0..self.w * self.h {
                            let k = group.opacity
                                * group.fill_opacity
                                * self.mask_at(group, i % self.w, i / self.w);
                            let src = &inside[i * n..(i + 1) * n];
                            let a = src[self.color] * k;
                            if a <= 0.0 {
                                continue;
                            }
                            let mut colors = [0f32; 4];
                            for (c, out) in colors[..self.color].iter_mut().enumerate() {
                                *out = src[c] / src[self.color];
                            }
                            self.blend_pixel(
                                &mut canvas[i * n..(i + 1) * n],
                                &colors[..self.color],
                                a,
                                group.blend,
                            );
                        }
                    }
                }
            }
        }
    }

    fn layer_alpha(&self, layer: &Layer, x: usize, y: usize) -> f32 {
        if layer.fill.is_some() {
            return 1.0;
        }
        let (lx, ly) = (x as i32 - layer.rect.left, y as i32 - layer.rect.top);
        if lx < 0
            || ly < 0
            || lx as usize >= layer.rect.width()
            || ly as usize >= layer.rect.height()
        {
            return 0.0;
        }
        match &layer.alpha {
            Some(a) => a[ly as usize * layer.rect.width() + lx as usize],
            None => 1.0,
        }
    }

    fn mask_at(&self, layer: &Layer, x: usize, y: usize) -> f32 {
        let vector = layer
            .vector
            .as_ref()
            .map_or(1.0, |v| f32::from(v[y * self.w + x]) / 255.0);
        let Some(mask) = &layer.mask else {
            return vector;
        };
        let (mx, my) = (x as i32 - mask.rect.left, y as i32 - mask.rect.top);
        let v = if mx < 0
            || my < 0
            || mx as usize >= mask.rect.width()
            || my as usize >= mask.rect.height()
        {
            mask.default
        } else {
            mask.data[my as usize * mask.rect.width() + mx as usize]
        };
        f32::from(v) / 255.0 * vector
    }

    /// `clip`：剪贴蒙版的基底图层，被剪贴的图层只在它有像素的地方显示。
    fn draw_layer(&self, layer: &Layer, canvas: &mut [f32], clip: Option<&Layer>) {
        let n = self.color + 1;
        let k = layer.opacity * layer.fill_opacity;
        if k <= 0.0 {
            return;
        }
        // 只画图层（或填充图层的蒙版）覆盖的范围。
        let (x0, y0, x1, y1) = if layer.fill.is_some() {
            (0, 0, self.w, self.h)
        } else {
            (
                layer.rect.left.clamp(0, self.w as i32) as usize,
                layer.rect.top.clamp(0, self.h as i32) as usize,
                layer.rect.right.clamp(0, self.w as i32) as usize,
                layer.rect.bottom.clamp(0, self.h as i32) as usize,
            )
        };
        let lw = layer.rect.width();
        let mut colors = [0f32; 4];
        for y in y0..y1 {
            for x in x0..x1 {
                let mut a = self.layer_alpha(layer, x, y) * self.mask_at(layer, x, y) * k;
                if let Some(base) = clip {
                    a *= self.layer_alpha(base, x, y) * self.mask_at(base, x, y);
                }
                if a <= 0.0 {
                    continue;
                }
                match &layer.fill {
                    Some(fill) => colors[..self.color].copy_from_slice(&fill[..self.color]),
                    None => {
                        let i = (y as i32 - layer.rect.top) as usize * lw
                            + (x as i32 - layer.rect.left) as usize;
                        for (c, out) in colors[..self.color].iter_mut().enumerate() {
                            // 没有的颜色通道：CMYK 按"没有油墨"，其他按 0。
                            *out = layer.colors[c]
                                .get(i)
                                .copied()
                                .unwrap_or(if self.color == 4 { 1.0 } else { 0.0 });
                        }
                    }
                }
                let i = y * self.w + x;
                self.blend_pixel(
                    &mut canvas[i * n..(i + 1) * n],
                    &colors[..self.color],
                    a,
                    layer.blend,
                );
            }
        }
    }

    /// 源（不预乘的颜色、Alpha）按混合模式叠到目标（预乘）上（W3C 合成公式）。
    fn blend_pixel(&self, dst: &mut [f32], src: &[f32], a_s: f32, blend: Blend) {
        let c = self.color;
        let a_b = dst[c];
        let mut backdrop = [0f32; 4];
        if a_b > 0.0 {
            for i in 0..c {
                backdrop[i] = dst[i] / a_b;
            }
        }
        // CMYK 存的是反过来的值：混合按油墨量算（与 Photoshop 相同），所以先翻过来。
        let cmyk = c == 4;
        let flip = |v: f32| if cmyk { 1.0 - v } else { v };
        let mut mixed = [0f32; 4];
        if blend.non_separable() && self.rgb && c == 3 {
            let m = non_separable(
                blend,
                [backdrop[0], backdrop[1], backdrop[2]],
                [src[0], src[1], src[2]],
            );
            mixed[..3].copy_from_slice(&m);
        } else if blend == Blend::Normal || blend == Blend::PassThrough {
            mixed[..c].copy_from_slice(src);
        } else {
            for i in 0..c {
                mixed[i] = flip(separable(blend, flip(backdrop[i]), flip(src[i])));
            }
        }
        let a_o = a_s + a_b * (1.0 - a_s);
        for i in 0..c {
            // Cs' = (1 − αb)·Cs + αb·B(Cb, Cs)
            let cs = (1.0 - a_b) * src[i] + a_b * mixed[i];
            dst[i] = a_s * cs + (1.0 - a_s) * dst[i];
        }
        dst[c] = a_o;
    }
}

fn separable(blend: Blend, b: f32, s: f32) -> f32 {
    let v = match blend {
        Blend::Normal | Blend::PassThrough => s,
        Blend::Darken => b.min(s),
        Blend::Multiply => b * s,
        Blend::ColorBurn => {
            if b >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }
        Blend::LinearBurn => b + s - 1.0,
        Blend::Lighten => b.max(s),
        Blend::Screen => b + s - b * s,
        Blend::ColorDodge => {
            if b <= 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }
        Blend::LinearDodge => b + s,
        Blend::Overlay => hard_light(s, b),
        Blend::SoftLight => {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 {
                    ((16.0 * b - 12.0) * b + 4.0) * b
                } else {
                    b.sqrt()
                };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }
        Blend::HardLight => hard_light(b, s),
        Blend::VividLight => {
            if s <= 0.5 {
                separable(Blend::ColorBurn, b, 2.0 * s)
            } else {
                separable(Blend::ColorDodge, b, 2.0 * s - 1.0)
            }
        }
        Blend::LinearLight => b + 2.0 * s - 1.0,
        Blend::PinLight => {
            if s <= 0.5 {
                b.min(2.0 * s)
            } else {
                b.max(2.0 * s - 1.0)
            }
        }
        Blend::HardMix => {
            if b + s >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        Blend::Difference => (b - s).abs(),
        Blend::Exclusion => b + s - 2.0 * b * s,
        Blend::Subtract => b - s,
        Blend::Divide => {
            if s <= 0.0 {
                if b > 0.0 { 1.0 } else { 0.0 }
            } else {
                b / s
            }
        }
        // 不可分的模式在不是 RGB 的文档里按正常。
        _ => s,
    };
    v.clamp(0.0, 1.0)
}

/// 强光：`b` 是下面的颜色，`s` 是上面的（叠加 = 交换两者的强光）。
fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b * 2.0 * s
    } else {
        let t = 2.0 * s - 1.0;
        b + t - b * t
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    for v in &mut out {
        if n < 0.0 && l - n > 0.0 {
            *v = l + (*v - l) * l / (l - n);
        }
        if x > 1.0 && x - l > 0.0 {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    out
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let (max, min) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    if max - min <= 0.0 {
        return [0.0; 3];
    }
    c.map(|v| (v - min) * s / (max - min))
}

fn non_separable(blend: Blend, b: [f32; 3], s: [f32; 3]) -> [f32; 3] {
    match blend {
        Blend::Hue => set_lum(set_sat(s, sat(b)), lum(b)),
        Blend::Saturation => set_lum(set_sat(b, sat(s)), lum(b)),
        Blend::Color => set_lum(s, lum(b)),
        Blend::Luminosity => set_lum(b, lum(s)),
        Blend::DarkerColor => {
            if lum(s) < lum(b) {
                s
            } else {
                b
            }
        }
        Blend::LighterColor => {
            if lum(s) > lum(b) {
                s
            } else {
                b
            }
        }
        _ => s,
    }
}

/// 大端读字节；越界返回 None。
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let out = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }
    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.array()?))
    }
    fn i16(&mut self) -> Option<i16> {
        Some(i16::from_be_bytes(self.array()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.array()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_be_bytes(self.array()?))
    }
    /// PSB 里有的长度是 8 字节。
    fn length(&mut self, long: bool) -> Option<usize> {
        if long {
            usize::try_from(u64::from_be_bytes(self.array()?)).ok()
        } else {
            Some(self.u32()? as usize)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 测试用的一个图层：矩形、通道（编号、不压缩的 8 位数据）、混合模式、不透明度、标志、附加信息块。
    pub(crate) struct TestLayer<'a> {
        pub rect: (i32, i32, i32, i32),
        pub channels: Vec<(i16, Vec<u8>)>,
        pub blend: &'a [u8; 4],
        pub opacity: u8,
        pub flags: u8,
        pub mask: Option<((i32, i32, i32, i32), u8)>,
        pub blocks: Vec<(&'a [u8; 4], Vec<u8>)>,
    }

    impl Default for TestLayer<'_> {
        fn default() -> Self {
            Self {
                rect: (0, 0, 0, 0),
                channels: Vec::new(),
                blend: b"norm",
                opacity: 255,
                flags: 0,
                mask: None,
                blocks: Vec::new(),
            }
        }
    }

    /// 整个"图层和蒙版信息"段（8 位 PSD）。
    pub(crate) fn section(layers: &[TestLayer<'_>]) -> Vec<u8> {
        let mut info = (layers.len() as i16).to_be_bytes().to_vec();
        let mut data = Vec::new();
        for l in layers {
            for v in [l.rect.0, l.rect.1, l.rect.2, l.rect.3] {
                info.extend_from_slice(&v.to_be_bytes());
            }
            info.extend_from_slice(&(l.channels.len() as u16).to_be_bytes());
            for (id, bytes) in &l.channels {
                info.extend_from_slice(&id.to_be_bytes());
                info.extend_from_slice(&((bytes.len() + 2) as u32).to_be_bytes());
                data.extend_from_slice(&0u16.to_be_bytes());
                data.extend_from_slice(bytes);
            }
            info.extend_from_slice(b"8BIM");
            info.extend_from_slice(l.blend);
            info.extend_from_slice(&[l.opacity, 0, l.flags, 0]);
            let mut extra = Vec::new();
            match l.mask {
                Some((r, default)) => {
                    extra.extend_from_slice(&20u32.to_be_bytes());
                    for v in [r.0, r.1, r.2, r.3] {
                        extra.extend_from_slice(&v.to_be_bytes());
                    }
                    extra.extend_from_slice(&[default, 0, 0, 0]);
                }
                None => extra.extend_from_slice(&0u32.to_be_bytes()),
            }
            extra.extend_from_slice(&0u32.to_be_bytes()); // 混合范围
            extra.extend_from_slice(&[0, 0, 0, 0]); // 空名字，补齐到 4 字节
            for (key, body) in &l.blocks {
                extra.extend_from_slice(b"8BIM");
                extra.extend_from_slice(*key);
                extra.extend_from_slice(&(body.len() as u32).to_be_bytes());
                extra.extend_from_slice(body);
            }
            info.extend_from_slice(&(extra.len() as u32).to_be_bytes());
            info.extend_from_slice(&extra);
        }
        info.extend_from_slice(&data);
        let mut out = (info.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&info);
        out.extend_from_slice(&0u32.to_be_bytes()); // 全局图层蒙版
        out
    }

    fn rgb(c: &Composite, i: usize) -> [u8; 4] {
        c.rgba[i * 4..i * 4 + 4].try_into().unwrap()
    }

    #[test]
    fn opacity_alpha_and_multiply() {
        // 下面：不透明的红色 2x1；上面：蓝色，第 1 个像素不透明、第 2 个透明，不透明度 50%。
        let red = TestLayer {
            rect: (0, 0, 1, 2),
            channels: vec![(0, vec![255, 255]), (1, vec![0, 0]), (2, vec![0, 0])],
            ..Default::default()
        };
        let blue = TestLayer {
            rect: (0, 0, 1, 2),
            channels: vec![
                (-1, vec![255, 0]),
                (0, vec![0, 0]),
                (1, vec![0, 0]),
                (2, vec![255, 255]),
            ],
            opacity: 128,
            ..Default::default()
        };
        let c = composite(&section(&[red, blue]), false, 8, 3, true, 2, 1).unwrap();
        let p = rgb(&c, 0);
        assert!(
            p[0].abs_diff(127) <= 1 && p[2].abs_diff(128) <= 1 && p[3] == 255,
            "{p:?}"
        );
        assert_eq!(rgb(&c, 1), [255, 0, 0, 255]);
        assert!(!c.has_alpha);
        // 正片叠底：中灰叠在白色上还是中灰；隐藏的图层不算；蒙版默认色 0 时整层看不见。
        let white = TestLayer {
            rect: (0, 0, 1, 2),
            channels: vec![(0, vec![255; 2]), (1, vec![255; 2]), (2, vec![255; 2])],
            ..Default::default()
        };
        let gray = TestLayer {
            rect: (0, 0, 1, 2),
            channels: vec![(0, vec![128; 2]), (1, vec![128; 2]), (2, vec![128; 2])],
            blend: b"mul ",
            ..Default::default()
        };
        let hidden = TestLayer {
            rect: (0, 0, 1, 2),
            channels: vec![(0, vec![0; 2]), (1, vec![0; 2]), (2, vec![0; 2])],
            flags: 0x02,
            ..Default::default()
        };
        let masked = TestLayer {
            rect: (0, 0, 1, 2),
            channels: vec![
                (0, vec![0; 2]),
                (1, vec![0; 2]),
                (2, vec![0; 2]),
                (-2, vec![0]),
            ],
            mask: Some(((0, 0, 1, 1), 0)),
            ..Default::default()
        };
        let c = composite(
            &section(&[white, gray, hidden, masked]),
            false,
            8,
            3,
            true,
            2,
            1,
        )
        .unwrap();
        assert_eq!(rgb(&c, 0), [128, 128, 128, 255]);
        assert_eq!(rgb(&c, 1), [128, 128, 128, 255]);
    }

    #[test]
    fn groups_and_clipping() {
        // 组里的一个白色图层，组的不透明度 50%（"穿透"），下面是黑色：结果中灰。
        let black = TestLayer {
            rect: (0, 0, 1, 1),
            channels: vec![(0, vec![0]), (1, vec![0]), (2, vec![0])],
            ..Default::default()
        };
        let end = TestLayer {
            blocks: vec![(b"lsct", 3u32.to_be_bytes().to_vec())],
            ..Default::default()
        };
        let white = TestLayer {
            rect: (0, 0, 1, 1),
            channels: vec![(0, vec![255]), (1, vec![255]), (2, vec![255])],
            ..Default::default()
        };
        let mut divider = 1u32.to_be_bytes().to_vec();
        divider.extend_from_slice(b"8BIMpass");
        let group = TestLayer {
            opacity: 128,
            blocks: vec![(b"lsct", divider)],
            ..Default::default()
        };
        let c = composite(
            &section(&[black, end, white, group]),
            false,
            8,
            3,
            true,
            1,
            1,
        )
        .unwrap();
        assert!(rgb(&c, 0)[0].abs_diff(128) <= 1, "{:?}", rgb(&c, 0));
        // 剪贴：红色图层剪贴到只占左边一个像素的基底上，右边不显示。
        let base = TestLayer {
            rect: (0, 0, 1, 1),
            channels: vec![(-1, vec![255]), (0, vec![0]), (1, vec![0]), (2, vec![0])],
            ..Default::default()
        };
        let clipped = TestLayer {
            rect: (0, 0, 1, 2),
            channels: vec![(0, vec![255; 2]), (1, vec![0; 2]), (2, vec![0; 2])],
            ..Default::default()
        };
        let mut s = section(&[base, clipped]);
        // 第二个图层的"剪贴"字节：记录里混合模式之后的第 2 个字节。
        let at = s.windows(8).rposition(|w| w == b"8BIMnorm").unwrap() + 9;
        s[at] = 1;
        let c = composite(&s, false, 8, 3, true, 2, 1).unwrap();
        assert_eq!(rgb(&c, 0), [255, 0, 0, 255]);
        assert_eq!(rgb(&c, 1)[3], 0);
    }

    /// 矢量蒙版的一条记录（26 字节）。
    fn path_record(selector: u16, values: &[i32]) -> Vec<u8> {
        let mut r = selector.to_be_bytes().to_vec();
        for v in values {
            r.extend_from_slice(&v.to_be_bytes());
        }
        r.resize(26, 0);
        r
    }

    #[test]
    fn vector_mask_fills_its_path() {
        // 闭合子路径：左半边的矩形（4 个节点，控制点与锚点重合）。坐标是相对文档的 8.24 定点数 (y, x)。
        let one = 1 << 24;
        let half = 1 << 23;
        let mut body = 3u32.to_be_bytes().to_vec();
        body.extend_from_slice(&0u32.to_be_bytes());
        let mut length = 0u16.to_be_bytes().to_vec();
        length.extend_from_slice(&4u16.to_be_bytes());
        length.extend_from_slice(&1i16.to_be_bytes());
        length.resize(26, 0);
        body.extend_from_slice(&length);
        for (y, x) in [(0, 0), (0, half), (one, half), (one, 0)] {
            body.extend_from_slice(&path_record(1, &[y, x, y, x, y, x]));
        }
        let mask = vector_mask(&body, 4, 2).unwrap();
        assert_eq!(&mask[..4], &[255, 255, 0, 0]);
        assert_eq!(&mask[4..], &[255, 255, 0, 0]);
        // 停用的矢量蒙版不算。
        body[7] = 0x04;
        assert!(vector_mask(&body, 4, 2).is_none());
    }

    /// 所有图层加起来有内存上限：超了不合成（以前每个通道各算各的，一万个图层能要几百 GB）。
    #[test]
    fn layers_share_one_memory_budget() {
        let layer = || TestLayer {
            rect: (0, 0, 1, 2),
            channels: vec![(0, vec![255, 255]), (1, vec![0, 0]), (2, vec![0, 0])],
            ..Default::default()
        };
        // 2x1 的 RGB：每个图层 3 个通道 × 2 个像素 × 4 字节 = 24 字节；上限 64 字节时总量是 128 字节。
        let with = |count: usize| {
            let layers: Vec<_> = (0..count).map(|_| layer()).collect();
            crate::decode::with_alloc_limit(64, || composite(&section(&layers), false, 8, 3, true, 2, 1).is_some())
        };
        assert!(with(5));
        assert!(!with(6));
        // 平常的上限下照常合成。
        let layers: Vec<_> = (0..6).map(|_| layer()).collect();
        assert!(composite(&section(&layers), false, 8, 3, true, 2, 1).is_some());
    }

    /// 压缩的通道声称能解出的像素比数据撑得起的多得多：不为它分配内存。
    #[test]
    fn implausibly_compressed_channels_are_ignored() {
        // PackBits：1 行，行长度 2 字节，数据 2 字节（重复 128 次的 0）。
        let rle = |w: usize| channel_samples(&[0, 1, 0, 2, 0x81, 0], w, 1, 8, false, true);
        assert_eq!(rle(128).map(|s| s.len()), Some(128));
        assert_eq!(rle(4_000_000), None);
        // zlib：几个字节解不出一百万个像素。
        let mut zipped = vec![0, 2];
        {
            use std::io::Write;
            let mut encoder = flate2::write::ZlibEncoder::new(&mut zipped, flate2::Compression::best());
            encoder.write_all(&[9u8; 4096]).unwrap();
            encoder.finish().unwrap();
        }
        assert_eq!(channel_samples(&zipped, 4096, 1, 8, false, true).map(|s| s.len()), Some(4096));
        assert_eq!(channel_samples(&zipped, 4096, 1000, 8, false, true), None);
    }

    /// 图层组套得太深的（Photoshop 最多 10 层）不合成；矢量蒙版的组件太多的不做蒙版。
    #[test]
    fn deep_nesting_and_huge_vector_masks_are_refused() {
        let nested = |depth: usize| {
            let mut layers: Vec<TestLayer<'_>> = Vec::new();
            for _ in 0..depth {
                layers.push(TestLayer {
                    blocks: vec![(b"lsct", 3u32.to_be_bytes().to_vec())],
                    ..Default::default()
                });
            }
            layers.push(TestLayer {
                rect: (0, 0, 1, 2),
                channels: vec![(0, vec![255, 255]), (1, vec![0, 0]), (2, vec![0, 0])],
                ..Default::default()
            });
            for _ in 0..depth {
                layers.push(TestLayer {
                    blocks: vec![(b"lsct", 1u32.to_be_bytes().to_vec())],
                    ..Default::default()
                });
            }
            composite(&section(&layers), false, 8, 3, true, 2, 1)
        };
        let shallow = nested(MAX_NESTING).expect("10 层照常合成");
        assert_eq!(rgb(&shallow, 0), [255, 0, 0, 255]);
        assert!(nested(MAX_NESTING + 1).is_none());

        // 矢量蒙版：版本、标志，然后每个组件一条"闭合子路径"记录（没有节点）。
        let vmsk = |components: usize| {
            let mut body = vec![0, 0, 0, 3, 0, 0, 0, 0];
            for _ in 0..components {
                let mut record = [0u8; 26];
                record[5] = 1; // 合并方式：合并
                body.extend_from_slice(&record);
            }
            body
        };
        assert!(vector_mask(&vmsk(8), 64, 64).is_some());
        // 1024x1024 的文档：2^33 / 2^20 = 8192 个组件以内才做。
        assert!(vector_mask(&vmsk(8193), 1024, 1024).is_none(), "组件数 × 像素数超了");
    }
}
