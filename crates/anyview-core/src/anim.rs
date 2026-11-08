//! 动图：GIF、APNG、动态 WebP 的帧和时长；"更多格式"一步加上 JPEG XL 动图。
//!
//! 播放由 [`crate::api`] 里的播放线程负责：每张动图一个线程，按帧的时长送出 `AnimationFrame` 事件，
//! 暂停时不占 CPU。帧在这里解码并合成成完整的画面（与浏览器一致）；整段动画不大时全部留在内存里
//! （[`FRAME_CACHE_BYTES`]），太大时边播边解，每播完一遍从头再读。

use crate::i18n::Message;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, Frames, ImageFormat, ImageReader};

/// 一段动画最多在内存里留多少字节的帧；超过时边播边解。
pub const FRAME_CACHE_BYTES: usize = 128 * 1024 * 1024;

/// 帧时长：10 ms 及以下的按 100 ms 算，与主流浏览器相同（很多 GIF 把时长写成 0）。
pub fn frame_delay_ms(raw: u32) -> u32 {
    if raw <= 10 { 100 } else { raw }
}

/// 可能是动图的文件（按文件头判断格式）：GIF、PNG（APNG）、WebP、JPEG XL、动态 AVIF，以及系统解码器说有多帧的
/// （macOS 的动态 AVIF、HEICS）。
pub fn may_animate(path: &Path) -> bool {
    matches!(
        sniff(path),
        Some(ImageFormat::Gif | ImageFormat::Png | ImageFormat::WebP)
    ) || is_jxl_file(path)
        || is_avis_file(path)
        || crate::decode::platform_frame_count(path).is_some_and(|n| n > 1)
}

/// 文件头的品牌里有 avis（动态 AVIF）。
fn is_avis_file(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 64];
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let n = file.read(&mut head).unwrap_or(0);
    crate::avif::is_avis(&head[..n])
}

fn is_jxl_file(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 12];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|_| crate::jxl::is_jxl(&head))
}

fn sniff(path: &Path) -> Option<ImageFormat> {
    ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .format()
}

/// 一帧：合成好的完整画面和显示时长。
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// RGBA8 未预乘；`bgra` 为真时已是 BGRA8 预乘（系统解码器给的）。
    pub rgba: Vec<u8>,
    pub bgra: bool,
    pub delay_ms: u32,
}

/// 按顺序读出动图的各帧；不是动图（APNG、WebP、JPEG XL 没有动画标记）或读不了时为空。
/// GIF 没有动画标记，由调用的一方看有没有第二帧。
pub struct FrameReader {
    source: Source,
}

enum Source {
    Image(Frames<'static>),
    /// JPEG XL（"更多格式"）：jxl-oxide 读好整个文件，按序号逐帧渲染。
    Jxl(Box<crate::jxl::Jxl>, usize),
    /// 系统解码器（macOS 的动态 AVIF、HEICS）：文件、下一帧的序号、帧数。
    Platform(std::path::PathBuf, u32, u32),
    /// 动态 AVIF 的核心解码器（系统解码器放不了时）。
    Avif(Box<crate::avif::Sequence>),
}

impl FrameReader {
    pub fn open(path: &Path) -> Option<Self> {
        // 系统解码器说有多帧就交给它；说只有一帧（或者它解不了，如 Windows 没装 AV1 扩展）时再看核心自己能不能放。
        if let Some(count) = crate::decode::platform_frame_count(path)
            && count > 1
        {
            return Some(Self {
                source: Source::Platform(path.to_path_buf(), 0, count),
            });
        }
        if is_avis_file(path) {
            let sequence = crate::avif::Sequence::open(std::fs::read(path).ok()?)?;
            return Some(Self {
                source: Source::Avif(Box::new(sequence)),
            });
        }
        if is_jxl_file(path) {
            let data = std::fs::read(path).ok()?;
            let jxl = crate::jxl::Jxl::open(&data).ok()?;
            return jxl.is_animated().then(|| Self {
                source: Source::Jxl(Box::new(jxl), 0),
            });
        }
        let reader = || File::open(path).ok().map(BufReader::new);
        let frames = match sniff(path)? {
            ImageFormat::Gif => GifDecoder::new(reader()?).ok()?.into_frames(),
            ImageFormat::Png => {
                let decoder = PngDecoder::new(reader()?).ok()?;
                if !decoder.is_apng().ok()? {
                    return None;
                }
                decoder.apng().ok()?.into_frames()
            }
            ImageFormat::WebP => {
                let decoder = WebPDecoder::new(reader()?).ok()?;
                if !decoder.has_animation() {
                    return None;
                }
                decoder.into_frames()
            }
            _ => return None,
        };
        Some(Self {
            source: Source::Image(frames),
        })
    }

    /// 下一帧；读完一遍时为 `Ok(None)`。
    pub fn next_frame(&mut self) -> Result<Option<Frame>, Message> {
        let frames = match &mut self.source {
            Source::Image(frames) => frames,
            Source::Jxl(jxl, next) => {
                if *next >= jxl.frame_count() {
                    return Ok(None);
                }
                let frame = jxl.frame(*next)?;
                *next += 1;
                return Ok(Some(Frame {
                    width: frame.width,
                    height: frame.height,
                    rgba: frame.rgba,
                    bgra: false,
                    delay_ms: frame_delay_ms(frame.delay_ms),
                }));
            }
            Source::Avif(sequence) => {
                let Some((width, height, rgba, delay)) = sequence.next_frame()? else {
                    return Ok(None);
                };
                return Ok(Some(Frame {
                    width,
                    height,
                    rgba,
                    bgra: false,
                    delay_ms: frame_delay_ms(delay),
                }));
            }
            Source::Platform(path, next, count) => {
                if *next >= *count {
                    return Ok(None);
                }
                let (image, delay) = crate::decode::platform_decode_frame(path, *next)
                    .ok_or_else(|| msg!("第 {n} 帧解不出来", "Frame {n} could not be decoded", n = (*next + 1).to_string()))?;
                *next += 1;
                return Ok(Some(Frame {
                    width: image.width,
                    height: image.height,
                    rgba: image.pixels,
                    bgra: true,
                    delay_ms: frame_delay_ms(delay),
                }));
            }
        };
        match frames.next() {
            None => Ok(None),
            Some(Err(e)) => Err(e.to_string().into()),
            Some(Ok(frame)) => {
                let (numer, denom) = frame.delay().numer_denom_ms();
                let raw = numer.checked_div(denom).unwrap_or(0);
                let buffer = frame.into_buffer();
                let (width, height) = buffer.dimensions();
                Ok(Some(Frame {
                    width,
                    height,
                    rgba: buffer.into_raw(),
                    bgra: false,
                    delay_ms: frame_delay_ms(raw),
                }))
            }
        }
    }
}

/// GIF 文件头里的背景色（逻辑屏幕描述符的背景色序号 → 全局调色板），0xRRGGBB；
/// 没有全局调色板时为空。设置"为 GIF 图片使用透明背景"选"默认 GIF 调色板"时用它。
pub fn gif_background(head: &[u8]) -> Option<u32> {
    if head.len() < 13 || !(head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a")) {
        return None;
    }
    let flags = head[10];
    if flags & 0x80 == 0 {
        return None;
    }
    let entries = 2usize << (flags & 0x07);
    let index = usize::from(head[11]);
    if index >= entries {
        return None;
    }
    let at = 13 + index * 3;
    let rgb = head.get(at..at + 3)?;
    Some(u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_delays_play_at_ten_frames_a_second() {
        assert_eq!(frame_delay_ms(0), 100);
        assert_eq!(frame_delay_ms(10), 100);
        assert_eq!(frame_delay_ms(20), 20);
        assert_eq!(frame_delay_ms(500), 500);
    }

    #[test]
    fn gif_background_comes_from_the_global_palette() {
        // 4 色全局调色板（flags 0x81），背景色序号 2。
        let mut head = b"GIF89a".to_vec();
        head.extend_from_slice(&[1, 0, 1, 0, 0x81, 2, 0]);
        head.extend_from_slice(&[0, 0, 0, 255, 255, 255, 0x12, 0x34, 0x56, 9, 9, 9]);
        assert_eq!(gif_background(&head), Some(0x123456));
        head[10] = 0x01; // 没有全局调色板
        assert_eq!(gif_background(&head), None);
        assert_eq!(gif_background(b"\x89PNG"), None);
    }
}
