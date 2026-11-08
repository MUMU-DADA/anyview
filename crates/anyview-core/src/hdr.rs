//! 高动态范围、广色域的图片色调映射并转换为标准动态范围（SDR）sRGB 显示。
//! 按 ITU-T H.273 的色彩信息（AVIF 的 nclx：原色、传递特性）：
//!
//! - 传递特性是 PQ（SMPTE ST 2084）、HLG（ARIB STD-B67）的：先换成显示亮度，以 203 尼特为普通的白（ITU-R BT.2408），
//!   再用色调曲线把更亮的部分压进普通亮度：0.75 以下原样，往上平滑地压，图里最亮的那一点正好到白（静态图按它自己
//!   的最亮处，动图按 1000 尼特，免得一帧一帧忽明忽暗）。按亮度压缩（RGB 按同一个比例），颜色的色相不变；压完以后
//!   个别通道仍超过白的（很亮又很饱和的颜色），向同样亮度的灰靠拢一点，不单独截掉那个通道（截掉会偏色，如橙变黄）。
//!   HLG 按 1000 尼特的显示器换成显示亮度（系统 Gamma 1.2，BT.2100）。
//! - 原色是 BT.2020、P3 的：在线性光里用矩阵换到 sRGB 的原色，超出 sRGB 的颜色截到边上（与 macOS 的系统解码器一样）。
//! - 普通的传递特性一律按 sRGB 的曲线解（与原来的显示一致）；线性的按线性。
//!
//! 别的原色（BT.709、没写的，和与 sRGB 差不多的 BT.601 那几种）、普通的传递特性都不换算（[`Conversion::new`] 返回空），
//! 与原来一样。传递曲线、HLG 的增益、输出的 sRGB 编码都查表（线性插值），1200 万像素的 HDR 图片不会因为逐点算幂次而慢。

use std::sync::OnceLock;

/// 普通的白（尼特，ITU-R BT.2408）。
pub const REFERENCE_WHITE: f32 = 203.0;
/// 不扫最亮处时（动图）当作最亮的亮度（尼特）。
const DEFAULT_PEAK: f32 = 1000.0;
/// 色调曲线从哪里开始压（以普通的白为 1）。
const KNEE: f32 = 0.75;
/// HLG 换成显示亮度时假定的显示器（尼特）和系统 Gamma（BT.2100）。
const HLG_PEAK: f64 = 1000.0;
const HLG_GAMMA: f64 = 1.2;
/// sRGB 的亮度系数（换到 sRGB 原色以后用）。
const SRGB_LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Transfer {
    Srgb,
    Linear,
    Pq,
    Hlg,
}

/// 一种来源（原色 + 传递特性）到 sRGB 的换算。
#[derive(Clone, Debug)]
pub struct Conversion {
    transfer: Transfer,
    /// 来源原色的线性 RGB → sRGB 的线性 RGB；原色就是 sRGB 的时为空。
    matrix: Option<[[f32; 3]; 3]>,
    /// 来源原色下的亮度系数（XYZ 的 Y 那一行）。
    luma: [f32; 3],
    /// 色调曲线的上限：当作最亮的亮度（以普通的白为 1）。只有 PQ、HLG 用。
    peak: f32,
}

impl Conversion {
    /// H.273 的原色、传递特性 → 要做的换算；不用换算时为空。
    pub fn new(primaries: u8, transfer: u8) -> Option<Self> {
        let transfer = match transfer {
            16 => Transfer::Pq,
            18 => Transfer::Hlg,
            8 => Transfer::Linear,
            _ => Transfer::Srgb,
        };
        let source = chromaticities(primaries);
        if source.is_none() && transfer == Transfer::Srgb {
            return None;
        }
        let srgb = to_xyz(SRGB);
        let (matrix, luma) = match source {
            Some(source) => {
                let m = to_xyz(source);
                (Some(to_f32(mul(&invert(&srgb), &m))), to_f32_row(m[1]))
            }
            None => (None, to_f32_row(srgb[1])),
        };
        Some(Self {
            transfer,
            matrix,
            luma,
            peak: DEFAULT_PEAK / REFERENCE_WHITE,
        })
    }

    /// 是不是高动态范围（PQ、HLG）：要做色调映射。
    pub fn is_hdr(&self) -> bool {
        matches!(self.transfer, Transfer::Pq | Transfer::Hlg)
    }

    /// 简短的名字，写进像素格式（图片信息里能看出做过色调映射）。
    pub fn label(&self) -> Option<&'static str> {
        match self.transfer {
            Transfer::Pq => Some("PQ"),
            Transfer::Hlg => Some("HLG"),
            _ => None,
        }
    }

    /// 按图里最亮的亮度（以普通的白为 1，[`Self::luminance`]）定色调曲线的上限。
    pub fn set_peak(&mut self, peak: f32) {
        self.peak = if peak.is_finite() { peak.max(1.0) } else { 1.0 };
    }

    /// 非线性的 R'G'B'（0–1）→ 线性光（以普通的白为 1；PQ、HLG 的亮部还没压）。
    pub fn linear(&self, rgb: [f32; 3]) -> [f32; 3] {
        match self.transfer {
            Transfer::Srgb => rgb.map(|v| tables().srgb.eval(v)),
            Transfer::Linear => rgb.map(|v| if v > 0.0 { v } else { 0.0 }),
            Transfer::Pq => rgb.map(|v| tables().pq.eval(v)),
            Transfer::Hlg => {
                let scene = rgb.map(|v| tables().hlg.eval(v));
                let gain = tables().hlg_gain.eval(dot(self.luma, scene));
                scene.map(|v| v * gain)
            }
        }
    }

    /// 线性光的亮度（以普通的白为 1）。
    pub fn luminance(&self, linear: [f32; 3]) -> f32 {
        dot(self.luma, linear)
    }

    /// 非线性的 R'G'B'（0–1）→ sRGB 编码的 8 位颜色。
    pub fn apply(&self, rgb: [f32; 3]) -> [u8; 3] {
        let mut linear = self.linear(rgb);
        if self.is_hdr() {
            let y = self.luminance(linear);
            if y > KNEE {
                let scale = tone(y, self.peak) / y;
                linear = linear.map(|v| v * scale);
            }
        }
        if let Some(matrix) = &self.matrix {
            linear = mul_vec(matrix, linear);
        }
        if self.is_hdr() {
            linear = toward_gray(linear);
        }
        linear.map(crate::psd::linear_to_srgb8)
    }
}

/// 色调曲线：[`KNEE`] 以下原样；往上平滑地压，`peak` 正好到 1（在超出的那一段上用扩展的 Reinhard，起点的斜率是 1，
/// 和下面接得上）。
fn tone(y: f32, peak: f32) -> f32 {
    if y <= KNEE {
        return y;
    }
    if peak <= 1.0 {
        return y.min(1.0);
    }
    let room = 1.0 - KNEE;
    let t = (y - KNEE) / room;
    let top = (peak - KNEE) / room;
    let g = t * (1.0 + t / (top * top)) / (1.0 + t);
    (KNEE + room * g).min(1.0)
}

/// 亮度不超过白、但个别通道超过的颜色：保持亮度，按同一个比例向同样亮度的灰靠拢，直到最大的通道正好到白
/// （单独截掉那个通道会偏色）。
fn toward_gray(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    if max <= 1.0 {
        return c;
    }
    let y = dot(SRGB_LUMA, c);
    if y >= 1.0 {
        return [1.0; 3];
    }
    let k = (1.0 - y) / (max - y);
    c.map(|v| y + k * (v - y))
}

// ---- 查找表 ----

/// [0, 1] 等分成若干段的查找表，段内线性插值。
struct Table {
    values: Vec<f32>,
}

impl Table {
    fn build(segments: usize, f: impl Fn(f64) -> f64) -> Self {
        Self {
            values: (0..=segments)
                .map(|i| f(i as f64 / segments as f64) as f32)
                .collect(),
        }
    }

    fn eval(&self, x: f32) -> f32 {
        let last = self.values.len() - 1;
        // 不是 0–1 的（包括 NaN）按两头算。
        let t = if x > 0.0 { x.min(1.0) * last as f32 } else { 0.0 };
        let i = (t as usize).min(last - 1);
        let f = t - i as f32;
        self.values[i] + (self.values[i + 1] - self.values[i]) * f
    }
}

struct Tables {
    /// sRGB 的曲线：信号 → 线性。
    srgb: Table,
    /// PQ：信号 → 亮度（以普通的白为 1）。
    pq: Table,
    /// HLG：信号 → 场景的线性光。
    hlg: Table,
    /// HLG：场景的亮度 → 换成显示亮度时乘的系数（以普通的白为 1）。
    hlg_gain: Table,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables {
        srgb: Table::build(4096, |v| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }),
        pq: Table::build(16384, |v| pq_to_nits(v) / f64::from(REFERENCE_WHITE)),
        hlg: Table::build(4096, hlg_to_scene),
        hlg_gain: Table::build(4096, |ys| HLG_PEAK * ys.powf(HLG_GAMMA - 1.0) / f64::from(REFERENCE_WHITE)),
    })
}

/// PQ（SMPTE ST 2084）的信号（0–1）→ 亮度（尼特）。
fn pq_to_nits(signal: f64) -> f64 {
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 4096.0 * 128.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 4096.0 * 32.0;
    const C3: f64 = 2392.0 / 4096.0 * 32.0;
    let p = signal.clamp(0.0, 1.0).powf(1.0 / M2);
    10000.0 * ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1)
}

/// HLG 的信号（0–1）→ 场景的线性光（0–1，BT.2100 的反 OETF）。
fn hlg_to_scene(signal: f64) -> f64 {
    const A: f64 = 0.178_832_77;
    const B: f64 = 0.284_668_92;
    const C: f64 = 0.559_910_73;
    let e = signal.clamp(0.0, 1.0);
    if e <= 0.5 {
        e * e / 3.0
    } else {
        (((e - C) / A).exp() + B) / 12.0
    }
}

// ---- 原色 ----

/// 红、绿、蓝、白的色度坐标 (x, y)。
type Chromaticities = [[f64; 2]; 4];

const D65: [f64; 2] = [0.3127, 0.3290];
const SRGB: Chromaticities = [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060], D65];

/// H.273 的原色编号 → 色度坐标；只换算明显比 sRGB 宽的（BT.2020、P3）。BT.709、没写的，和与 sRGB 差不多的
/// BT.601 那几种（5、6、7）为空，按 sRGB（与别的看图软件一样）。
fn chromaticities(primaries: u8) -> Option<Chromaticities> {
    Some(match primaries {
        // BT.2020、BT.2100
        9 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046], D65],
        // P3：Display P3（12）；DCI-P3（11）的白点不同，按 D65 近似（不做色适应）
        11 | 12 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060], D65],
        _ => return None,
    })
}

/// 原色 → 线性 RGB 到 XYZ 的矩阵（白点的 Y 为 1）。
fn to_xyz(c: Chromaticities) -> [[f64; 3]; 3] {
    let xyz = |[x, y]: [f64; 2]| [x / y, 1.0, (1.0 - x - y) / y];
    let (r, g, b, w) = (xyz(c[0]), xyz(c[1]), xyz(c[2]), xyz(c[3]));
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let inverse = invert(&m);
    let s: [f64; 3] = std::array::from_fn(|i| (0..3).map(|j| inverse[i][j] * w[j]).sum());
    std::array::from_fn(|i| std::array::from_fn(|j| m[i][j] * s[j]))
}

fn invert(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ]
}

fn mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn to_f32(m: [[f64; 3]; 3]) -> [[f32; 3]; 3] {
    m.map(to_f32_row)
}

fn to_f32_row(row: [f64; 3]) -> [f32; 3] {
    row.map(|v| v as f32)
}

fn mul_vec(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PQ 的信号值（亮度 → 信号，测试里用来造输入）。
    fn pq_signal(nits: f32) -> f32 {
        const M1: f32 = 2610.0 / 16384.0;
        const M2: f32 = 2523.0 / 4096.0 * 128.0;
        const C1: f32 = 3424.0 / 4096.0;
        const C2: f32 = 2413.0 / 4096.0 * 32.0;
        const C3: f32 = 2392.0 / 4096.0 * 32.0;
        let y = (nits / 10000.0).powf(M1);
        ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
    }

    #[test]
    fn srgb_needs_nothing() {
        assert!(Conversion::new(1, 13).is_none());
        assert!(Conversion::new(2, 2).is_none(), "没写的按 sRGB");
        assert!(Conversion::new(1, 1).is_none(), "BT.709 的曲线按 sRGB 显示，与原来一样");
        assert!(Conversion::new(6, 6).is_none(), "BT.601 的原色与 sRGB 差不多，不换算");
        assert!(Conversion::new(1, 5).is_none(), "BT.470 BG 的 2.8 次方曲线按 sRGB 显示（以前按 2.8 算，中灰暗了一截）");
        assert!(Conversion::new(9, 16).is_some());
        assert!(Conversion::new(12, 13).is_some());
        assert!(Conversion::new(1, 8).is_some(), "线性的要换");
    }

    #[test]
    fn pq_and_hlg_curves_match_the_standards() {
        for nits in [0.1, 10.0, 100.0, 203.0, 1000.0, 4000.0, 10000.0] {
            let back = pq_to_nits(f64::from(pq_signal(nits))) as f32;
            assert!((back - nits).abs() / nits < 1e-3, "{nits} → {back}");
            // 查表（线性插值）的误差也很小。
            let looked_up = tables().pq.eval(pq_signal(nits)) * REFERENCE_WHITE;
            assert!((looked_up - nits).abs() / nits < 3e-3, "表：{nits} → {looked_up}");
        }
        // BT.2408：HLG 的 75% 信号在 1000 尼特的显示器上是普通的白（约 203 尼特）。
        let hlg = Conversion::new(9, 18).unwrap();
        let white = hlg.linear([0.75; 3]);
        assert!((white[0] - 1.0).abs() < 0.01, "{white:?}");
    }

    /// PQ、BT.2020：普通的白接近白（原来按 sRGB 显示是中灰 148），1000 尼特比它亮，暗处、中间调基本照原样，
    /// BT.2020 的纯红换到 sRGB 仍是红。
    #[test]
    fn pq_bt2020_is_tone_mapped_to_srgb() {
        let pq = Conversion::new(9, 16).unwrap();
        assert!(pq.is_hdr() && pq.label() == Some("PQ"));
        let white = pq.apply([pq_signal(203.0); 3]);
        let bright = pq.apply([pq_signal(1000.0); 3]);
        let dark = pq.apply([pq_signal(10.0); 3]);
        assert!(white[0] >= 230 && white == [white[0]; 3], "普通的白：{white:?}");
        assert_eq!(bright, [255; 3], "按默认 1000 尼特为最亮");
        assert!((60..=66).contains(&dark[0]), "10 尼特（普通的白的 5%）应是 sRGB 约 63：{dark:?}");
        let red = pq.apply([pq_signal(100.0), 0.0, 0.0]);
        assert!(red[0] > 150 && red[1] == 0 && red[2] == 0, "BT.2020 的红：{red:?}");
    }

    /// 很亮又很饱和的颜色：压完亮度后红通道仍超过白——向灰靠拢，不单独截掉红（截掉会偏黄）。
    #[test]
    fn bright_saturated_colors_keep_their_hue() {
        let clipped = [1.0f32, 0.6, 0.05];
        let kept = toward_gray([1.6, 0.6, 0.05]);
        assert!((kept[0] - 1.0).abs() < 1e-6 && kept.iter().all(|v| (0.0..=1.0).contains(v)), "{kept:?}");
        let ratio = |c: [f32; 3]| (c[1] - c[2]) / (c[0] - c[2]);
        // 保持亮度、向灰靠拢以后，绿相对红的比例比直接截掉时小（截掉会更偏黄）。
        assert!(ratio(kept) < ratio(clipped), "{kept:?}");
        let y = |c: [f32; 3]| dot(SRGB_LUMA, c);
        assert!((y(kept) - y([1.6, 0.6, 0.05])).abs() < 1e-4, "亮度不变");
        assert_eq!(toward_gray([0.5, 0.2, 0.1]), [0.5, 0.2, 0.1]);
    }

    /// 最亮处只有 400 尼特时按它压：400 尼特正好到白，普通的白比按 1000 尼特时亮一些。
    #[test]
    fn the_peak_follows_the_image() {
        let mut pq = Conversion::new(9, 16).unwrap();
        let at_1000 = pq.apply([pq_signal(203.0); 3])[0];
        let peak = pq.luminance(pq.linear([pq_signal(400.0); 3]));
        pq.set_peak(peak);
        assert_eq!(pq.apply([pq_signal(400.0); 3]), [255; 3]);
        assert!(pq.apply([pq_signal(203.0); 3])[0] > at_1000);
        // 色调曲线连续、单调。
        let mut last = 0.0;
        for i in 0..=400 {
            let y = i as f32 * 0.025;
            let out = tone(y, 6.0);
            assert!(out >= last - 1e-6 && out <= 1.0, "{y} → {out}");
            last = out;
        }
    }

    /// P3（普通亮度）的橙色换到 sRGB：绿通道明显变低（原来当 sRGB 显示偏淡）；灰色不变。
    #[test]
    fn p3_colors_are_converted_to_srgb() {
        let p3 = Conversion::new(12, 13).unwrap();
        assert!(!p3.is_hdr() && p3.label().is_none());
        let orange = p3.apply([1.0, 0.5, 0.0]);
        assert!(orange[0] == 255 && (110..=125).contains(&orange[1]) && orange[2] == 0, "{orange:?}");
        let gray = p3.apply([0.5; 3]);
        assert!(gray.iter().all(|&v| v.abs_diff(128) <= 1), "{gray:?}");
    }
}
