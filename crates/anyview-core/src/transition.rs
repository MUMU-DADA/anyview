//! 翻页效果与翻页音效。
//!
//! 切到别的图片时（翻页、跳转、随机、换文件夹，不含"打开"），界面把旧画面和新画面按这里算出的参数叠在一起画几百毫秒：
//! - 淡出：旧画面在上面逐渐变透明，露出下面的新画面。
//! - 滑动：旧画面滑出、新画面从另一边滑进来（往后翻向左滑，往前翻向右滑）。
//! - 翻页：像翻书——往后翻时旧的一页以左边为轴向左翻起、立起来后消失，露出下面的新一页；
//!   往前翻时新的一页从左边翻回来盖住旧的一页。翻起的一页逐渐变暗。
//!
//! 参数都按画布的大小给出（平移是画布宽度的倍数，转轴在画布左边），两端只负责画。
//! 音效是程序里合成的一小段"刷"的翻纸声（WAV），不用外部的声音文件。

use std::sync::OnceLock;

/// 翻页效果（设置"翻页效果"，默认"无"）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageEffect {
    #[default]
    None,
    Page,
    Fade,
    Slide,
}

impl PageEffect {
    pub fn from_id(id: &str) -> Self {
        match id {
            "page" => Self::Page,
            "fade" => Self::Fade,
            "slide" => Self::Slide,
            _ => Self::None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Page => "page",
            Self::Fade => "fade",
            Self::Slide => "slide",
        }
    }

    /// 效果持续的时间（毫秒）。
    pub fn duration_ms(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Fade => 250,
            Self::Slide => 300,
            Self::Page => 450,
        }
    }
}

/// 这一次切图要做的效果和音效（[`crate::AppCore::transition_for`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub effect: PageEffect,
    /// 往后翻（下一张、跳到后面、随机、下一个文件夹）为真。
    pub forward: bool,
    pub duration_ms: u32,
    /// 播放翻页音效（设置"翻页时播放音效"）。
    pub sound: bool,
}

/// 一层画面在某一时刻的样子。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerParams {
    /// 不透明度 0–1。
    pub opacity: f64,
    /// 水平平移：画布宽度的倍数，正数向右。
    pub shift: f64,
    /// 绕竖直轴转的角度（度）：转轴在画布左边，负数表示右边缘朝观看者翻起来、向左转。
    pub angle: f64,
    /// 盖在这一层上的黑色的不透明度 0–1（翻起的一页变暗）。
    pub shade: f64,
}

impl LayerParams {
    pub const STILL: Self = Self {
        opacity: 1.0,
        shift: 0.0,
        angle: 0.0,
        shade: 0.0,
    };
}

/// 某一时刻两层画面的样子。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionFrame {
    pub old: LayerParams,
    pub new: LayerParams,
    /// 旧画面画在新画面上面。
    pub old_on_top: bool,
}

/// 透视的视点距离：画布宽度的几倍（翻页时用）。
pub const PERSPECTIVE: f64 = 2.5;

/// 缓动：开头和结尾慢、中间快（smoothstep）。
pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 进度 `progress`（0–1，按时间算，这里再做缓动）时两层画面的样子。
pub fn frame(effect: PageEffect, forward: bool, progress: f64) -> TransitionFrame {
    let p = ease(progress);
    let still = LayerParams::STILL;
    match effect {
        PageEffect::None => TransitionFrame {
            old: LayerParams {
                opacity: 0.0,
                ..still
            },
            new: still,
            old_on_top: false,
        },
        // 淡出：旧画面在上面变透明（两张重叠处是两者按比例混合）。
        PageEffect::Fade => TransitionFrame {
            old: LayerParams {
                opacity: 1.0 - p,
                ..still
            },
            new: still,
            old_on_top: true,
        },
        // 滑动：往后翻时两张一起向左移一个画布宽，往前翻时向右。
        PageEffect::Slide => {
            let direction = if forward { -1.0 } else { 1.0 };
            TransitionFrame {
                old: LayerParams {
                    shift: direction * p,
                    ..still
                },
                new: LayerParams {
                    shift: direction * (p - 1.0),
                    ..still
                },
                old_on_top: false,
            }
        }
        // 翻页：往后翻时旧的一页翻起（0° → -90°）；往前翻时新的一页翻回来（-90° → 0°）。
        PageEffect::Page => {
            if forward {
                TransitionFrame {
                    old: LayerParams {
                        angle: -90.0 * p,
                        shade: 0.35 * p,
                        ..still
                    },
                    new: still,
                    old_on_top: true,
                }
            } else {
                TransitionFrame {
                    old: still,
                    new: LayerParams {
                        angle: -90.0 * (1.0 - p),
                        shade: 0.35 * (1.0 - p),
                        ..still
                    },
                    old_on_top: false,
                }
            }
        }
    }
}

/// 翻页音效：合成的一小段翻纸声，WAV（16 位、单声道、44.1 kHz，约 0.22 秒）。第一次用时生成，之后复用。
pub fn page_sound_wav() -> &'static [u8] {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    WAV.get_or_init(|| wav(&synthesize_page_sound(), SAMPLE_RATE))
}

const SAMPLE_RATE: u32 = 44_100;

/// 翻纸声：带通滤波的噪声（纸面摩擦的"刷"），加上随机起伏的纹理（纸的细碎声），
/// 结尾一个很轻的低频"啪"（纸落下）。内容由公式和固定的随机种子决定，每次相同。
fn synthesize_page_sound() -> Vec<f32> {
    let n = (SAMPLE_RATE as f64 * 0.22) as usize;
    let mut rng = 0x9E37_79B9_7F4A_7C15_u64;
    let mut noise = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let mut swish = Biquad::band_pass(3_000.0, 0.8);
    let mut flap = Biquad::band_pass(500.0, 1.2);
    let mut texture = 0.0_f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / f64::from(SAMPLE_RATE);
        // 摩擦声：频率从 2.2 kHz 滑到 4.5 kHz（纸翻得越来越快），30 ms 起、到 0.2 秒落完。
        swish.set_band_pass(2_200.0 + 2_300.0 * (t / 0.22), 0.8);
        let attack = (t / 0.03).min(1.0);
        let release = ((0.2 - t) / 0.17).clamp(0.0, 1.0);
        let envelope = attack * release * release;
        // 纸的细碎声：慢变化的随机起伏（约 40 Hz 的低通噪声）。
        texture += (noise() - texture) * 0.006;
        let crinkle = 0.65 + 0.35 * (texture * 6.0).clamp(-1.0, 1.0);
        let mut sample = swish.process(noise()) * envelope * crinkle;
        // 纸落下的"啪"：0.15 秒开始，20 ms 内衰减。
        if t >= 0.15 {
            let k = (-(t - 0.15) / 0.02).exp();
            sample += flap.process(noise()) * 0.6 * k;
        }
        out.push(sample as f32);
    }
    // 最后 5 ms 淡出到 0，不会"咔"一声。
    let tail = (f64::from(SAMPLE_RATE) * 0.005) as usize;
    for (k, s) in out.iter_mut().rev().take(tail).enumerate() {
        *s *= k as f32 / tail as f32;
    }
    // 峰值归一到 0.35（约 -9 dBFS），声音轻一些。
    let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs())).max(1e-6);
    for s in &mut out {
        *s *= 0.35 / peak;
    }
    out
}

/// 二阶 IIR 滤波器（RBJ 的带通公式，常数峰值增益为 0 dB）。
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    fn band_pass(frequency: f64, q: f64) -> Self {
        let mut filter = Self {
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        };
        filter.set_band_pass(frequency, q);
        filter
    }

    fn set_band_pass(&mut self, frequency: f64, q: f64) {
        let w = 2.0 * std::f64::consts::PI * frequency / f64::from(SAMPLE_RATE);
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.b1 = 0.0;
        self.b2 = -alpha / a0;
        self.a1 = -2.0 * w.cos() / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// 单声道 16 位 PCM 的 WAV 文件。
fn wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_start_on_the_old_image_and_end_on_the_new_one() {
        for effect in [PageEffect::Fade, PageEffect::Slide, PageEffect::Page] {
            for forward in [true, false] {
                let start = frame(effect, forward, 0.0);
                let end = frame(effect, forward, 1.0);
                // 开始时只看得见旧画面（新画面在下面被盖住，或在画布外，或侧立着）。
                assert_eq!(start.old, LayerParams::STILL, "{effect:?} {forward}");
                let new_hidden = start.new.shift.abs() >= 1.0
                    || start.new.angle.abs() >= 90.0
                    || (start.old_on_top && start.old.opacity == 1.0);
                assert!(new_hidden, "{effect:?} {forward}");
                // 结束时新画面就是平常的样子，旧画面看不见了。
                assert_eq!(end.new, LayerParams::STILL, "{effect:?} {forward}");
                let old_gone = end.old.opacity == 0.0
                    || end.old.shift.abs() >= 1.0
                    || end.old.angle.abs() >= 90.0
                    || !end.old_on_top;
                assert!(old_gone, "{effect:?} {forward}");
            }
        }
    }

    #[test]
    fn slide_and_page_follow_the_direction() {
        let next = frame(PageEffect::Slide, true, 0.5);
        assert!(next.old.shift < 0.0 && next.new.shift > 0.0, "往后翻向左滑");
        let back = frame(PageEffect::Slide, false, 0.5);
        assert!(back.old.shift > 0.0 && back.new.shift < 0.0, "往前翻向右滑");
        assert!((next.old.shift - next.new.shift + 1.0).abs() < 1e-9, "两张首尾相接");
        let turn = frame(PageEffect::Page, true, 0.5);
        assert!(turn.old_on_top && (turn.old.angle + 45.0).abs() < 1e-9 && turn.old.shade > 0.0);
        let back = frame(PageEffect::Page, false, 0.5);
        assert!(!back.old_on_top && (back.new.angle + 45.0).abs() < 1e-9);
        let fade = frame(PageEffect::Fade, true, 0.5);
        assert!((fade.old.opacity - 0.5).abs() < 1e-9);
        assert_eq!(PageEffect::from_id("slide"), PageEffect::Slide);
        assert_eq!(PageEffect::from_id("x"), PageEffect::None);
        assert!(PageEffect::Page.duration_ms() > PageEffect::Fade.duration_ms());
    }

    #[test]
    fn page_sound_is_a_short_quiet_wav() {
        let wav = page_sound_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(wav.len(), 44 + data_len);
        let samples: Vec<i16> = wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();
        let seconds = samples.len() as f64 / f64::from(SAMPLE_RATE);
        assert!((0.15..0.3).contains(&seconds), "{seconds}");
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!((10_000..=12_000).contains(&peak), "峰值约 0.35：{peak}");
        // 开头和结尾都是静的（不会"咔"一声）。
        assert!(samples[0].unsigned_abs() < 300);
        assert_eq!(samples[samples.len() - 1], 0);
        // 第二次取的是同一份。
        assert!(std::ptr::eq(wav, page_sound_wav()));
    }
}
