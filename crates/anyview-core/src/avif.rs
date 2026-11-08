//! AVIF（.avif .avifs）：核心自己的解码器，系统没有 AVIF 解码器时用：Windows 没装"AV1 视频扩展"、
//! Linux 没装 libheif。有系统解码器时照旧先用它（macOS 的 ImageIO、Windows 的 WIC、Linux 的 libheif）。
//!
//! 容器用 mp4parse（Mozilla，Firefox 用的就是它）解析，AV1 用 rav1d（dav1d 的 Rust 移植，不带汇编）解码，都是 Rust。
//! 做到的：主图（没有主图的动态 AVIF 取第一帧）、8/10/12 位、4:0:0/4:2:0/4:2:2/4:4:4（色度按中心位置双线性插值）、
//! 各种矩阵系数（BT.601/709/2020、FCC、SMPTE 240、恒等即 GBR、YCgCo）、有限和完整范围、透明通道（含预乘的）、
//! 旋转和镜像（irot、imir）、内嵌 ICC（RGB 的交给色彩管理，灰度的在这里转）；没有 ICC 时按 nclx 的原色、传递特性
//! 换成 sRGB：高动态范围（PQ、HLG）做色调映射，BT.2020、P3 的颜色换到 sRGB（[`crate::hdr`]）。写着高动态范围的
//! 带 ICC 也按 nclx（ICC 表达不了色调映射，按它管只会截掉亮部）。容器里没有 nclx 时用 AV1 序列头里的。预乘的透明
//! 先在换算之前除掉（预乘是按非线性的值乘的）。
//! Windows、Linux 上这种图片不交给系统的解码器（它们不做这些换算，画面发灰），由这里解（[`needs_own_color`]）。
//! 没做的：拼接（grid）、裁切（clap）。

use crate::i18n::Message;
use std::io::Cursor;
use std::ptr::NonNull;

use mp4parse::{Feature, ImageMirror, ImageRotation, ParseStrictness};
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_MC_BT470BG, DAV1D_MC_BT601, DAV1D_MC_BT709, DAV1D_MC_BT2020_CL, DAV1D_MC_BT2020_NCL,
    DAV1D_MC_FCC, DAV1D_MC_IDENTITY, DAV1D_MC_SMPTE_YCGCO, DAV1D_MC_SMPTE240,
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422, Dav1dSequenceHeader,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_parse_sequence_header, dav1d_picture_unref, dav1d_send_data,
};

use crate::color::{self, SampleKind};
use crate::decode::{Raster, alloc_limit};
use crate::hdr::Conversion;

/// 扩展名（小写，不带点）。
pub const EXTENSIONS: &[&str] = &["avif", "avifs"];

/// 文件头是不是 AVIF：第一个盒子是 ftyp，主品牌或兼容品牌里有 avif、avis。
pub fn is_avif(head: &[u8]) -> bool {
    if head.get(4..8) != Some(b"ftyp") {
        return false;
    }
    let size = head
        .get(..4)
        .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let end = size.clamp(8, head.len());
    let brands = |range: std::ops::Range<usize>| {
        head.get(range)
            .is_some_and(|b| b == b"avif" || b == b"avis")
    };
    brands(8..12)
        || (16..end)
            .step_by(4)
            .any(|at| at + 4 <= end && brands(at..at + 4))
}

/// 解码。`use_icc` 为假时（关闭“使用图片文件的 ICC”）不用内嵌的配置文件。
pub fn decode(data: &[u8], use_icc: bool) -> Result<Raster, Message> {
    let context = mp4parse::read_avif(&mut Cursor::new(data), ParseStrictness::Normal)
        .map_err(|e| msg!("AVIF：读不懂文件结构（{e}）", "AVIF: can't read the file structure ({e})", e = format!("{e:?}")))?;
    if context.unsupported_features.contains(Feature::Grid) {
        return Err(msg!("AVIF：拼接（grid）的图片暂不支持", "AVIF: grid images aren't supported yet"));
    }
    // 没有主图的动态 AVIF：取颜色轨道和透明轨道的第一帧（按轨道的类型认，不按先后）。
    let (coded, alpha_coded) = match context.primary_item_coded_data() {
        Some(coded) => (
            coded.to_vec(),
            context.alpha_item_coded_data().map(<[u8]>::to_vec),
        ),
        None => {
            let (color, alpha) = sequence_tracks(&context).ok_or(msg!("AVIF：文件里没有图像", "AVIF: no image in the file"))?;
            let first = |track: &mp4parse::Track| {
                let &(offset, size) = track_samples(track)?.first()?;
                data.get(offset..offset.checked_add(size)?)
                    .map(<[u8]>::to_vec)
            };
            (
                first(color).ok_or(msg!("AVIF：文件里没有图像", "AVIF: no image in the file"))?,
                alpha.and_then(first),
            )
        }
    };
    // 容器里写了不支持的矩阵时不用先解 AV1（大图要好一会儿）。
    let container = nclx(&context);
    if let Some(n) = container {
        supported_matrix(u32::from(n.matrix))?;
    }
    let color = decode_av1(&coded)?;
    let colour = container.unwrap_or(color.nclx);
    let (matrix, full_range) = (u32::from(colour.matrix), colour.full_range);
    supported_matrix(matrix)?;
    let alpha = match &alpha_coded {
        Some(coded) => Some(decode_av1(coded)?),
        None => None,
    };
    let (width, height) = (color.width, color.height);
    if alpha.as_ref().is_some_and(|alpha| (alpha.width, alpha.height) != (width, height)) {
        return Err(msg!("AVIF：透明通道的尺寸与图像不同", "AVIF: the alpha channel's size differs from the image"));
    }
    let mut rgba = Raster::alloc(width, height)?;
    let monochrome = color.layout == DAV1D_PIXEL_LAYOUT_I400;
    let icc = context
        .icc_colour_information()
        .and_then(Result::ok)
        .map(<[u8]>::to_vec)
        .filter(|_| use_icc);
    // 按 nclx 换成 sRGB（高动态范围的按这张图自己最亮的地方做色调映射）；带 ICC 的按 ICC，写着高动态范围的
    // 除外，那时就不再按 ICC 管（不然换算两次）。动图（这里解的是先显示的那一张）按播放时的 1000 尼特压，开始播放时
    // 亮度不跳。
    let conversion = conversion_for(colour, icc.is_some());
    let icc = icc.filter(|_| conversion.is_none());
    let label = conversion.as_ref().and_then(Conversion::label);
    let animated = sequence_tracks(&context)
        .and_then(|(track, _)| track_samples(track))
        .is_some_and(|samples| samples.len() >= 2);
    let alpha = alpha.as_ref().map(|alpha| Alpha::new(alpha, context.premultiplied_alpha));
    to_rgb(&color, matrix, full_range, conversion, !animated, alpha.as_ref(), &mut rgba);
    let icc_profile = match (monochrome, icc) {
        // 灰度的配置文件在这里用（界面只认 RGB 像素）。
        (true, Some(profile)) => {
            let grays: Vec<u8> = rgba.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
            color::samples_to_srgb(&grays, SampleKind::Gray, &profile, &mut rgba);
            None
        }
        (_, icc) => icc,
    };
    // 旋转、镜像（HEIF 规定先旋转后镜像）。
    let rotation = context.image_rotation().unwrap_or(ImageRotation::D0);
    let mirror = match context.image_mirror_ptr() {
        // SAFETY: mp4parse 返回的指针指向 context 里的数据，context 还活着；为空时表示没有镜像。
        Ok(pointer) if !pointer.is_null() => {
            Some(matches!(unsafe { &*pointer }, ImageMirror::LeftRight))
        }
        _ => None,
    };
    let (rgba, width, height) = orient(rgba, width, height, rotation, mirror);
    let layout = match color.layout {
        DAV1D_PIXEL_LAYOUT_I400 => "YUV400",
        DAV1D_PIXEL_LAYOUT_I420 => "YUV420",
        DAV1D_PIXEL_LAYOUT_I422 => "YUV422",
        _ => "YUV444",
    };
    Ok(Raster {
        width,
        height,
        rgba,
        has_alpha: alpha.is_some(),
        format: "AVIF".to_string(),
        pixel_format: format!(
            "{layout}P{}{}{}",
            color.bits,
            if alpha.is_some() { "A" } else { "" },
            label.map(|l| format!(" {l}")).unwrap_or_default()
        ),
        icc_profile,
    })
}

/// 动态 AVIF 的颜色轨道和透明轨道（按轨道类型：图像或视频轨道是颜色，辅助视频轨道是透明）。
fn sequence_tracks(
    context: &mp4parse::AvifContext,
) -> Option<(&mp4parse::Track, Option<&mp4parse::Track>)> {
    let tracks = &context.sequence.as_ref()?.tracks;
    let color = tracks.iter().find(|t| {
        matches!(
            t.track_type,
            mp4parse::TrackType::Picture | mp4parse::TrackType::Video
        )
    })?;
    let alpha = tracks
        .iter()
        .find(|t| matches!(t.track_type, mp4parse::TrackType::AuxiliaryVideo));
    Some((color, alpha))
}

/// 色彩信息（ITU-T H.273）：原色、传递特性、矩阵系数、是否完整范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Nclx {
    primaries: u8,
    transfer: u8,
    matrix: u8,
    full_range: bool,
}

/// 与 mp4parse 的 `NclxColourInformation` 相同的内存布局（它是 `#[repr(C)]`，字段不公开，给 C 接口用）。
#[repr(C)]
struct RawNclx {
    colour_primaries: u8,
    transfer_characteristics: u8,
    matrix_coefficients: u8,
    full_range_flag: bool,
}

// mp4parse 升级后布局变了的话编译不过，不会读错（字段的先后在 mp4parse 的源代码里核对过：0.17.0）。
const _: () = assert!(
    size_of::<mp4parse::NclxColourInformation>() == size_of::<RawNclx>()
        && align_of::<mp4parse::NclxColourInformation>() == align_of::<RawNclx>()
);

/// 容器里的 nclx。
fn nclx(context: &mp4parse::AvifContext) -> Option<Nclx> {
    let pointer = context.nclx_colour_information_ptr()?.ok()?;
    if pointer.is_null() {
        return None;
    }
    // SAFETY: mp4parse 的 NclxColourInformation 是 #[repr(C)]，依次是三个 u8 和一个 bool，与 RawNclx 相同（大小、对齐
    // 在编译时核对）；
    // 指针指向 context 里的数据，context 还活着。
    let raw = unsafe { &*pointer.cast::<RawNclx>() };
    Some(Nclx {
        primaries: raw.colour_primaries,
        transfer: raw.transfer_characteristics,
        matrix: raw.matrix_coefficients,
        full_range: raw.full_range_flag,
    })
}

/// 这个 AVIF 要不要由核心自己解：色彩信息写的是高动态范围（PQ、HLG），或者没有 ICC、原色是 BT.2020、P3
/// （[`conversion_for`]）。容器里没有 nclx 时看 AV1 序列头里的（不用解码）。Windows、Linux 的系统解码器（WIC、
/// libheif）不做这些换算，这种图片不交给它们。
pub fn needs_own_color(data: &[u8]) -> bool {
    let Ok(context) = mp4parse::read_avif(&mut Cursor::new(data), ParseStrictness::Normal) else {
        return false;
    };
    let Some(colour) = nclx(&context).or_else(|| color_coded(&context, data).and_then(sequence_nclx)) else {
        return false;
    };
    let has_icc = context.icc_colour_information().is_some_and(|icc| icc.is_ok());
    conversion_for(colour, has_icc).is_some()
}

/// 按色彩信息要做的换算：带 ICC 时按 ICC（这里不换算），但写着高动态范围（PQ、HLG）的照样按 nclx——ICC 表达不了
/// 色调映射，按它做色彩管理只会把亮部截掉。
fn conversion_for(colour: Nclx, has_icc: bool) -> Option<Conversion> {
    Conversion::new(colour.primaries, colour.transfer).filter(|c| !has_icc || c.is_hdr())
}

/// 颜色那一幅的 AV1 数据：主图；没有主图的动态 AVIF 是颜色轨道的第一帧。
fn color_coded<'a>(context: &'a mp4parse::AvifContext, data: &'a [u8]) -> Option<&'a [u8]> {
    if let Some(coded) = context.primary_item_coded_data() {
        return Some(coded);
    }
    let (color, _) = sequence_tracks(context)?;
    let &(offset, size) = track_samples(color)?.first()?;
    data.get(offset..offset.checked_add(size)?)
}

/// AV1 序列头里的色彩信息：只读码流里的序列头，不解码。读不出时为空。
fn sequence_nclx(coded: &[u8]) -> Option<Nclx> {
    let data = NonNull::new(coded.as_ptr().cast_mut()).filter(|_| !coded.is_empty())?;
    let mut header = std::mem::MaybeUninit::<Dav1dSequenceHeader>::uninit();
    // SAFETY: 按 dav1d 的 C 接口用：数据指针、长度来自 coded（不为空），解析时只读；返回 0 时整个序列头写进了 header。
    let parsed = unsafe { dav1d_parse_sequence_header(NonNull::new(header.as_mut_ptr()), Some(data), coded.len()) };
    if parsed.0 != 0 {
        return None;
    }
    // SAFETY: 返回 0 时 header 已经写好。
    let header = unsafe { header.assume_init() };
    let byte = |v: u32| u8::try_from(v).unwrap_or(2);
    Some(Nclx {
        primaries: byte(header.pri),
        transfer: byte(header.trc),
        matrix: byte(header.mtrx),
        full_range: header.color_range != 0,
    })
}

/// 解出的一幅 AV1 图像：每个平面的样本都放成 u16（8 位的也是），一行接一行。
struct Planes {
    width: u32,
    height: u32,
    bits: u32,
    layout: u32,
    /// AV1 序列头里的色彩信息（容器里没有 nclx 时用）。
    nclx: Nclx,
    /// Y、U、V（单色时只有 Y）。
    planes: Vec<Vec<u16>>,
    /// 各平面的宽、高。
    sizes: Vec<(usize, usize)>,
}

/// 用 rav1d 解一幅 AV1 图像（AVIF 的一项：一串 OBU）。
fn decode_av1(coded: &[u8]) -> Result<Planes, Message> {
    Av1Decoder::new()?.decode(coded)
}

/// rav1d 的解码器。一个 AV1 码流用一个：动图的各帧要按顺序送进同一个解码器（后面的帧参照前面的）。
struct Av1Decoder {
    context: Option<Dav1dContext>,
}

impl Av1Decoder {
    fn new() -> Result<Self, Message> {
        // SAFETY: 按 dav1d 的 C 接口用：设置先由 dav1d_default_settings 填好全部字段，再改几项；
        // 上下文写进 self.context，Drop 时 dav1d_close。
        unsafe {
            let mut settings = std::mem::MaybeUninit::<Dav1dSettings>::uninit();
            dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr()));
            let mut settings = settings.assume_init();
            settings.n_threads =
                std::thread::available_parallelism().map_or(1, |n| n.get().min(8)) as i32;
            settings.max_frame_delay = 1;
            // 只要最高质量的那一层（多层的 AVIF 很少见）。
            settings.all_layers = 0;
            // 图像大小的上限：与其他解码器相同（按 RGBA 算的内存上限）。
            settings.frame_size_limit = (alloc_limit() / 4).min(u64::from(u32::MAX)) as u32;
            let mut decoder = Self { context: None };
            if dav1d_open(
                Some(NonNull::from(&mut decoder.context)),
                Some(NonNull::from(&mut settings)),
            )
            .0 < 0
            {
                return Err(msg!("AVIF：AV1 解码器打不开", "AVIF: the AV1 decoder could not be started"));
            }
            Ok(decoder)
        }
    }

    /// 送进一帧的数据（静态图的一项，或动图的一个样本），取出这一帧的图像。
    fn decode(&mut self, coded: &[u8]) -> Result<Planes, Message> {
        // SAFETY: 数据由 dav1d_data_create 分配、复制进去，用完 dav1d_data_unref；拿到的图像复制出来后
        // dav1d_picture_unref。所有指针都指向这里的局部变量或 self.context。
        unsafe {
            let mut data = Dav1dData::default();
            let buffer = dav1d_data_create(Some(NonNull::from(&mut data)), coded.len());
            if buffer.is_null() {
                return Err(msg!("AVIF：内存不够", "AVIF: not enough memory"));
            }
            std::ptr::copy_nonoverlapping(coded.as_ptr(), buffer, coded.len());
            let mut picture = Dav1dPicture::default();
            let mut idle = 0;
            let outcome = loop {
                if data.sz > 0 {
                    let sent = dav1d_send_data(self.context, Some(NonNull::from(&mut data))).0;
                    if sent < 0 && !is_again(sent) {
                        break Err(msg!("AVIF：AV1 数据有错（{sent}）", "AVIF: error in the AV1 data ({sent})", sent = format!("{sent}")));
                    }
                }
                let result = dav1d_get_picture(self.context, Some(NonNull::from(&mut picture))).0;
                if result == 0 {
                    break Ok(());
                }
                if !is_again(result) {
                    break Err(msg!("AVIF：AV1 解不出来（{result}）", "AVIF: the AV1 data could not be decoded ({result})", result = format!("{result}")));
                }
                if data.sz == 0 {
                    // 数据都送完了还没有图像：再等一会儿（多线程解码），还没有就算这段数据里没有图像。
                    idle += 1;
                    if idle > 1000 {
                        break Err(msg!("AVIF：AV1 没有解出图像", "AVIF: the AV1 decoder produced no image"));
                    }
                    std::thread::yield_now();
                }
            };
            dav1d_data_unref(Some(NonNull::from(&mut data)));
            outcome?;
            let planes = copy_planes(&picture);
            dav1d_picture_unref(Some(NonNull::from(&mut picture)));
            planes
        }
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        // SAFETY: context 是 dav1d_open 给的（或为空，dav1d_close 什么也不做），只关一次。
        unsafe { dav1d_close(Some(NonNull::from(&mut self.context))) };
    }
}

/// 动态 AVIF 的一帧：宽、高、RGBA8（未预乘）、时长（毫秒）。
pub type Frame = (u32, u32, Vec<u8>, u32);

/// 动态 AVIF（图像序列，avis）：按顺序解出各帧（动图播放用，anim.rs）。系统解码器能解动图时（macOS 的 ImageIO）不用它。
pub struct Sequence {
    data: Vec<u8>,
    /// 颜色轨道的各帧：(位置, 长度, 时长毫秒)。
    color: Vec<(usize, usize, u32)>,
    /// 透明轨道的各帧（位置, 长度）；没有时为空。
    alpha: Option<Vec<(usize, usize)>>,
    premultiplied: bool,
    nclx: Option<Nclx>,
    /// 有 ICC 时不按 nclx 换算（与静态图相同）。
    has_icc: bool,
    color_decoder: Av1Decoder,
    alpha_decoder: Option<Av1Decoder>,
    next: usize,
}

impl Sequence {
    /// 不是动态 AVIF、只有一帧或读不了时为空。
    pub fn open(data: Vec<u8>) -> Option<Self> {
        let context = mp4parse::read_avif(&mut Cursor::new(&data), ParseStrictness::Normal).ok()?;
        let (color_track, alpha_track) = sequence_tracks(&context)?;
        let samples = track_samples(color_track)?;
        if samples.len() < 2 {
            return None;
        }
        let delays = track_delays(color_track, samples.len());
        let color = samples
            .into_iter()
            .zip(delays)
            .map(|((offset, size), delay)| (offset, size, delay))
            .collect();
        let alpha = alpha_track.and_then(track_samples);
        let alpha_decoder = match &alpha {
            Some(_) => Some(Av1Decoder::new().ok()?),
            None => None,
        };
        Some(Self {
            premultiplied: context.premultiplied_alpha,
            nclx: nclx(&context),
            has_icc: context.icc_colour_information().is_some_and(|icc| icc.is_ok()),
            data,
            color,
            alpha,
            color_decoder: Av1Decoder::new().ok()?,
            alpha_decoder,
            next: 0,
        })
    }

    /// 下一帧（见 [`Frame`]）；读完一遍时为空。
    pub fn next_frame(&mut self) -> Result<Option<Frame>, Message> {
        let Some(&(offset, size, delay)) = self.color.get(self.next) else {
            return Ok(None);
        };
        let coded = self
            .data
            .get(offset..offset.saturating_add(size))
            .ok_or(msg!("AVIF：帧的数据不全", "AVIF: frame data is incomplete"))?;
        let color = self.color_decoder.decode(coded)?;
        let mut rgba = Raster::alloc(color.width, color.height)?;
        let colour = self.nclx.unwrap_or(color.nclx);
        let (matrix, full_range) = (u32::from(colour.matrix), colour.full_range);
        supported_matrix(matrix)?;
        // 透明轨道的这一帧（每帧都要送进它的解码器：后面的帧参照前面的）；尺寸对不上的不用。
        let mut alpha = None;
        if let (Some(samples), Some(decoder)) = (&self.alpha, &mut self.alpha_decoder)
            && let Some(&(offset, size)) = samples.get(self.next)
            && let Some(coded) = self.data.get(offset..offset.saturating_add(size))
        {
            alpha = Some(decoder.decode(coded)?).filter(|a| (a.width, a.height) == (color.width, color.height));
        }
        // 动图的各帧按同一个上限（1000 尼特）压，不按每帧自己的最亮处：不然一帧一帧忽明忽暗。
        let conversion = conversion_for(colour, self.has_icc);
        let alpha = alpha.as_ref().map(|alpha| Alpha::new(alpha, self.premultiplied));
        to_rgb(&color, matrix, full_range, conversion, false, alpha.as_ref(), &mut rgba);
        self.next += 1;
        Ok(Some((color.width, color.height, rgba, delay)))
    }
}

/// 一条轨道各帧的 (位置, 长度)：按 stsc 把样本分进各个块（stco），块里的样本一个接一个（stsz 给长度）。
fn track_samples(track: &mp4parse::Track) -> Option<Vec<(usize, usize)>> {
    let offsets = &track.stco.as_ref()?.offsets;
    let runs = &track.stsc.as_ref()?.samples;
    let sizes = track.stsz.as_ref()?;
    let total = if sizes.sample_size > 0 {
        track
            .stts
            .as_ref()?
            .samples
            .iter()
            .map(|s| s.sample_count as usize)
            .sum()
    } else {
        sizes.sample_sizes.len()
    };
    // 上限：一段动画最多这么多帧（防止坏文件让这里分配巨大的表）。
    if total == 0 || total > 100_000 {
        return None;
    }
    let runs: Vec<(u32, u32)> = runs
        .iter()
        .take(offsets.len().saturating_add(1))
        .map(|r| (r.first_chunk, r.samples_per_chunk))
        .collect();
    let offsets: Vec<u64> = offsets.iter().take(total.saturating_add(1)).copied().collect();
    sample_layout(&offsets, &runs, total, |i| {
        Some(if sizes.sample_size > 0 {
            sizes.sample_size as usize
        } else {
            *sizes.sample_sizes.get(i)? as usize
        })
    })
}

/// 每个采样（一帧）在文件里的位置和大小：`offsets` 是各块的起点，`runs` 是（从第几块起, 每块几个采样），
/// 按起始块从小到大；`size(i)` 是第 i 个采样的大小。表是文件写的，对不上（块比采样还多、一块里没有采样、
/// 没有覆盖第一块）的当坏文件，不照着空转——每一块只往后找它的那一条，不从头数。
fn sample_layout(
    offsets: &[u64],
    runs: &[(u32, u32)],
    total: usize,
    size: impl Fn(usize) -> Option<usize>,
) -> Option<Vec<(usize, usize)>> {
    if offsets.len() > total || runs.len() > offsets.len() {
        return None;
    }
    let mut samples = Vec::with_capacity(total);
    let mut run = 0;
    for (chunk, &offset) in offsets.iter().enumerate() {
        let number = u32::try_from(chunk).ok()?.checked_add(1)?;
        while runs.get(run + 1).is_some_and(|next| next.0 <= number) {
            run += 1;
        }
        let &(first_chunk, per_chunk) = runs.get(run)?;
        if first_chunk > number || per_chunk == 0 {
            return None;
        }
        let mut at = usize::try_from(offset).ok()?;
        for _ in 0..per_chunk {
            if samples.len() == total {
                return Some(samples);
            }
            let size = size(samples.len())?;
            samples.push((at, size));
            at = at.checked_add(size)?;
        }
    }
    (samples.len() == total).then_some(samples)
}

/// 一条轨道各帧的时长（毫秒，按 stts 和轨道的时间单位）；读不出时每帧 100 毫秒。
fn track_delays(track: &mp4parse::Track, count: usize) -> Vec<u32> {
    let scale = track.timescale.as_ref().map_or(0, |t| t.0);
    let mut delays: Vec<u32> = track
        .stts
        .as_ref()
        .map(|stts| {
            stts.samples
                .iter()
                .flat_map(|s| {
                    let ms = (u64::from(s.sample_delta) * 1000)
                        .checked_div(scale)
                        .unwrap_or(100);
                    std::iter::repeat_n(ms.min(u64::from(u32::MAX)) as u32, s.sample_count as usize)
                })
                .take(count)
                .collect()
        })
        .unwrap_or_default();
    delays.resize(count, 100);
    delays
}

/// 文件是不是动态 AVIF（看文件头里的品牌有没有 avis；真正有几帧打开时再看）。
pub fn is_avis(head: &[u8]) -> bool {
    if head.get(4..8) != Some(b"ftyp") {
        return false;
    }
    let size = head
        .get(..4)
        .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let end = size.clamp(8, head.len());
    head.get(8..12) == Some(b"avis")
        || (16..end)
            .step_by(4)
            .any(|at| at + 4 <= end && head.get(at..at + 4) == Some(b"avis"))
}

/// dav1d 的"再试一次"（EAGAIN，按平台的 errno）。
fn is_again(result: i32) -> bool {
    result == -libc::EAGAIN
}

/// 把 rav1d 的图像平面复制成 Planes。
///
/// # Safety
/// `picture` 是 dav1d_get_picture 刚给出的、还没释放的图像。
unsafe fn copy_planes(picture: &Dav1dPicture) -> Result<Planes, Message> {
    let width = usize::try_from(picture.p.w).map_err(|_| msg!("AVIF：尺寸不对", "AVIF: invalid size"))?;
    let height = usize::try_from(picture.p.h).map_err(|_| msg!("AVIF：尺寸不对", "AVIF: invalid size"))?;
    if width == 0 || height == 0 {
        return Err(msg!("AVIF：尺寸不对", "AVIF: invalid size"));
    }
    let bits = picture.p.bpc as u32;
    let layout = picture.p.layout;
    let nclx = match picture.seq_hdr {
        // SAFETY: 图像还没释放，序列头跟着它活着。
        Some(header) => {
            let header = unsafe { header.as_ref() };
            let byte = |v: u32| u8::try_from(v).unwrap_or(2);
            Nclx {
                primaries: byte(header.pri),
                transfer: byte(header.trc),
                matrix: byte(header.mtrx),
                full_range: header.color_range != 0,
            }
        }
        None => Nclx {
            primaries: 2,
            transfer: 2,
            matrix: 2,
            full_range: false,
        },
    };
    let (sx, sy) = match layout {
        DAV1D_PIXEL_LAYOUT_I420 => (2, 2),
        DAV1D_PIXEL_LAYOUT_I422 => (2, 1),
        _ => (1, 1),
    };
    let count = if layout == DAV1D_PIXEL_LAYOUT_I400 {
        1
    } else {
        3
    };
    let mut planes = Vec::with_capacity(count);
    let mut sizes = Vec::with_capacity(count);
    for index in 0..count {
        let (w, h) = if index == 0 {
            (width, height)
        } else {
            (width.div_ceil(sx), height.div_ceil(sy))
        };
        let stride = picture.stride[usize::from(index > 0)];
        let Some(base) = picture.data[index] else {
            return Err(msg!("AVIF：图像数据不全", "AVIF: image data is incomplete"));
        };
        let base = base.as_ptr().cast::<u8>();
        let mut plane = Vec::with_capacity(w * h);
        for y in 0..h {
            // SAFETY: dav1d 保证每个平面有 h 行、每行 stride 字节（可能为负，按行偏移算），8 位每样本 1 字节，更高位深 2 字节。
            let row = unsafe { base.offset(stride * y as isize) };
            if bits > 8 {
                let row = row.cast::<u16>();
                plane.extend((0..w).map(|x| unsafe { row.add(x).read_unaligned() }));
            } else {
                plane.extend((0..w).map(|x| u16::from(unsafe { row.add(x).read() })));
            }
        }
        planes.push(plane);
        sizes.push((w, h));
    }
    Ok(Planes {
        width: width as u32,
        height: height as u32,
        bits,
        layout,
        nclx,
        planes,
        sizes,
    })
}

/// 恒定亮度的矩阵（BT.2020 CL、色度推导的 CL、ICtCp）要在线性域里算，暂不支持：明确报错，不按普通矩阵算错颜色。
fn supported_matrix(matrix: u32) -> Result<(), Message> {
    use rav1d::include::dav1d::headers::{DAV1D_MC_CHROMAT_CL, DAV1D_MC_ICTCP};
    if matches!(
        matrix,
        DAV1D_MC_BT2020_CL | DAV1D_MC_CHROMAT_CL | DAV1D_MC_ICTCP
    ) {
        return Err(msg!("AVIF：暂不支持恒定亮度的颜色矩阵（{matrix}）", "AVIF: constant-luminance color matrices aren't supported yet ({matrix})", matrix = format!("{matrix}")));
    }
    Ok(())
}

/// 矩阵系数 → (Kr, Kb)。未指定的按 BT.601（与 libavif 相同）。
fn coefficients(matrix: u32) -> (f32, f32) {
    match matrix {
        DAV1D_MC_BT709 => (0.2126, 0.0722),
        DAV1D_MC_FCC => (0.30, 0.11),
        DAV1D_MC_BT470BG | DAV1D_MC_BT601 => (0.299, 0.114),
        DAV1D_MC_SMPTE240 => (0.212, 0.087),
        DAV1D_MC_BT2020_NCL => (0.2627, 0.0593),
        _ => (0.299, 0.114),
    }
}

/// YUV（或 GBR、YCgCo）→ RGBA8（不预乘）。有 `conversion` 时按它换成 sRGB；`adapt_peak` 为真时高动态范围的
/// 先扫一遍这张图最亮的地方（看不见的全透明像素不算），按它做色调映射。有透明通道时一起写进去；预乘过的颜色在换算
/// 之前先除掉（预乘是按非线性的值乘的，换算以后再除，HDR 图片半透明的边会发暗）。大图按行分给几个线程同时算
/// （避免逐像素查表开销导致高像素 HDR 图片解码变慢）。
fn to_rgb(
    planes: &Planes,
    matrix: u32,
    full_range: bool,
    conversion: Option<Conversion>,
    adapt_peak: bool,
    alpha: Option<&Alpha>,
    rgba: &mut [u8],
) {
    let width = planes.width as usize;
    // 没有透明通道时单独编一份（每个像素少一次判断）。
    match alpha {
        Some(alpha) => convert(planes, matrix, full_range, conversion, adapt_peak, rgba, |y, x, rgb| {
            let a = alpha.at(y * width + x);
            (alpha.unpremultiply(rgb, a), a)
        }),
        None => convert(planes, matrix, full_range, conversion, adapt_peak, rgba, |_, _, rgb| (rgb, 1.0)),
    }
}

/// [`to_rgb`] 的做法：`pixel` 给出一个像素（第几行、第几列、颜色）还原好的颜色和不透明度。
fn convert(
    planes: &Planes,
    matrix: u32,
    full_range: bool,
    conversion: Option<Conversion>,
    adapt_peak: bool,
    rgba: &mut [u8],
    pixel: impl Fn(usize, usize, [f32; 3]) -> ([f32; 3], f32) + Sync,
) {
    let yuv = Yuv::new(planes, matrix, full_range);
    let width = planes.width as usize;
    let threads = threads_for(width * planes.height as usize);
    let to_byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    match conversion {
        None => {
            in_rows(rgba, width, threads, |y, row| {
                yuv.row(y, |x, rgb| {
                    let ([r, g, b], a) = pixel(y, x, rgb);
                    row[x * 4..x * 4 + 4].copy_from_slice(&[to_byte(r), to_byte(g), to_byte(b), to_byte(a)]);
                });
            });
        }
        Some(mut conversion) => {
            if adapt_peak && conversion.is_hdr() {
                let peak = {
                    let conversion = &conversion;
                    max_over_rows(planes.height as usize, threads, |y| {
                        let mut peak = 0.0f32;
                        yuv.row(y, |x, rgb| {
                            let (rgb, a) = pixel(y, x, rgb);
                            if a > 0.0 {
                                peak = peak.max(conversion.luminance(conversion.linear(rgb)));
                            }
                        });
                        peak
                    })
                };
                conversion.set_peak(peak);
            }
            let conversion = &conversion;
            in_rows(rgba, width, threads, |y, row| {
                yuv.row(y, |x, rgb| {
                    let (rgb, a) = pixel(y, x, rgb);
                    let [r, g, b] = conversion.apply(rgb);
                    row[x * 4..x * 4 + 4].copy_from_slice(&[r, g, b, to_byte(a)]);
                });
            });
        }
    }
}

/// 透明通道（单色的 AV1 图像，尺寸和颜色的一样）：每个像素的不透明度。
struct Alpha<'a> {
    samples: &'a [u16],
    offset: f32,
    range: f32,
    /// 颜色是预乘过的（按非线性的值乘的）。
    premultiplied: bool,
}

impl<'a> Alpha<'a> {
    fn new(planes: &'a Planes, premultiplied: bool) -> Self {
        let max = ((1u32 << planes.bits) - 1) as f32;
        let scale = (1u32 << (planes.bits - 8)) as f32;
        let (offset, range) = if planes.nclx.full_range {
            (0.0, max)
        } else {
            (16.0 * scale, 219.0 * scale)
        };
        Self {
            samples: &planes.planes[0],
            offset,
            range,
            premultiplied,
        }
    }

    /// 第 `i` 个像素的不透明度（0–1）。
    fn at(&self, i: usize) -> f32 {
        self.samples
            .get(i)
            .map_or(1.0, |&a| ((f32::from(a) - self.offset) / self.range).clamp(0.0, 1.0))
    }

    /// 预乘过的颜色还原成不预乘的（全透明的不动）。
    fn unpremultiply(&self, rgb: [f32; 3], a: f32) -> [f32; 3] {
        if self.premultiplied && a > 0.0 && a < 1.0 {
            rgb.map(|v| v / a)
        } else {
            rgb
        }
    }
}

/// 用几个线程：小图（不到 25 万像素）一个，大的按处理器数，最多 8 个。
fn threads_for(pixels: usize) -> usize {
    if pixels < 1 << 18 {
        return 1;
    }
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8)
}

/// 一行一行地处理 RGBA 像素（`f`：第几行、这一行的像素），按行分成 `threads` 段同时算。
fn in_rows(rgba: &mut [u8], width: usize, threads: usize, f: impl Fn(usize, &mut [u8]) + Sync) {
    let row_bytes = width * 4;
    if row_bytes == 0 {
        return;
    }
    let height = rgba.len() / row_bytes;
    let rows = height.div_ceil(threads.max(1)).max(1);
    let run = |first: usize, chunk: &mut [u8]| {
        for (i, row) in chunk.chunks_exact_mut(row_bytes).enumerate() {
            f(first + i, row);
        }
    };
    if threads <= 1 {
        run(0, rgba);
        return;
    }
    std::thread::scope(|scope| {
        for (i, chunk) in rgba.chunks_mut(rows * row_bytes).enumerate() {
            let run = &run;
            scope.spawn(move || run(i * rows, chunk));
        }
    });
}

/// 各行的最大值（`f`：一行的最大值），按行分成 `threads` 段同时算。
fn max_over_rows(height: usize, threads: usize, f: impl Fn(usize) -> f32 + Sync) -> f32 {
    let rows = height.div_ceil(threads.max(1)).max(1);
    let part = |first: usize| (first..(first + rows).min(height)).map(&f).fold(0.0f32, f32::max);
    if threads <= 1 {
        return part(0);
    }
    std::thread::scope(|scope| {
        let parts: Vec<_> = (0..height)
            .step_by(rows)
            .map(|first| {
                let part = &part;
                scope.spawn(move || part(first))
            })
            .collect();
        parts
            .into_iter()
            .map(|p| p.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .fold(0.0f32, f32::max)
    })
}

/// 把 YUV（或 GBR、YCgCo）的平面换成非线性的 R'G'B'（0–1，还没截），一行一行地取。色度平面按中心位置双线性插值到
/// 亮度的分辨率。
struct Yuv<'a> {
    planes: &'a Planes,
    matrix: u32,
    width: usize,
    /// 亮度换成 0–1、色度换成 -0.5–0.5 用的偏移和范围。
    y_offset: f32,
    y_range: f32,
    c_offset: f32,
    c_range: f32,
    kr: f32,
    kb: f32,
    /// 每一列在色度平面里的两个相邻样本和权重（中心位置对齐）。单色时为空。
    columns: Vec<(usize, usize, f32)>,
    /// 色度的纵向采样比例。
    sy: f32,
}

impl<'a> Yuv<'a> {
    fn new(planes: &'a Planes, matrix: u32, full_range: bool) -> Self {
        let width = planes.width as usize;
        let max = ((1u32 << planes.bits) - 1) as f32;
        let scale = (1u32 << (planes.bits - 8)) as f32;
        let (y_offset, y_range, c_range) = if full_range {
            (0.0, max, max)
        } else {
            (16.0 * scale, 219.0 * scale, 224.0 * scale)
        };
        let (kr, kb) = coefficients(matrix);
        // 色度的采样比例按布局固定（4:2:0 横竖都是一半，4:2:2 只有横向一半）；平面的实际宽高（奇数尺寸时向上取整）
        // 只用来夹边界。
        let (columns, sy) = match planes.sizes.get(1) {
            Some(&(cw, ch)) if planes.planes.len() == 3 => {
                let sx = if cw < width { 0.5 } else { 1.0 };
                let columns = (0..width)
                    .map(|x| {
                        let c = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                        let left = (c as usize).min(cw - 1);
                        (left, (left + 1).min(cw - 1), c - left as f32)
                    })
                    .collect();
                (columns, if ch < planes.height as usize { 0.5 } else { 1.0 })
            }
            _ => (Vec::new(), 1.0),
        };
        Self {
            planes,
            matrix,
            width,
            y_offset,
            y_range,
            c_offset: (1u32 << (planes.bits - 1)) as f32,
            c_range,
            kr,
            kb,
            columns,
            sy,
        }
    }

    /// 第 `y` 行的每个像素交给 `f`（第几列、颜色）。
    fn row(&self, y: usize, mut f: impl FnMut(usize, [f32; 3])) {
        let width = self.width;
        let Some(luma) = self.planes.planes[0].get(y * width..(y + 1) * width) else {
            return;
        };
        let (y_offset, y_range) = (self.y_offset, self.y_range);
        if self.columns.is_empty() {
            for (x, &v) in luma.iter().enumerate() {
                let g = (f32::from(v) - y_offset) / y_range;
                f(x, [g, g, g]);
            }
            return;
        }
        let (cw, ch) = self.planes.sizes[1];
        let c = ((y as f32 + 0.5) * self.sy - 0.5).max(0.0);
        let top = (c as usize).min(ch - 1);
        let bottom = (top + 1).min(ch - 1);
        let fy = c - top as f32;
        let (u_plane, v_plane) = (&self.planes.planes[1], &self.planes.planes[2]);
        let (kr, kb) = (self.kr, self.kb);
        let kg = 1.0 - kr - kb;
        let (c_offset, c_range) = (self.c_offset, self.c_range);
        for (x, (&yv, &(left, right, fx))) in luma.iter().zip(&self.columns).enumerate() {
            let sample = |plane: &[u16]| {
                let a = f32::from(plane[top * cw + left]);
                let b = f32::from(plane[top * cw + right]);
                let c = f32::from(plane[bottom * cw + left]);
                let d = f32::from(plane[bottom * cw + right]);
                let upper = a + (b - a) * fx;
                let lower = c + (d - c) * fx;
                upper + (lower - upper) * fy
            };
            let yv = f32::from(yv);
            let (u, v) = (sample(u_plane), sample(v_plane));
            let rgb = match self.matrix {
                // 恒等矩阵：平面依次是 G、B、R。
                DAV1D_MC_IDENTITY => {
                    let n = |s: f32| (s - y_offset) / y_range;
                    [n(v), n(yv), n(u)]
                }
                // YCgCo：Cg、Co 的范围与 Y 相同（有限范围时是 219，不是 YCbCr 的 224，ITU-T H.273）。
                DAV1D_MC_SMPTE_YCGCO => {
                    let l = (yv - y_offset) / y_range;
                    let cg = (u - c_offset) / y_range;
                    let co = (v - c_offset) / y_range;
                    let t = l - cg;
                    [t + co, l + cg, t - co]
                }
                _ => {
                    let l = (yv - y_offset) / y_range;
                    let cb = (u - c_offset) / c_range;
                    let cr = (v - c_offset) / c_range;
                    let r = l + 2.0 * (1.0 - kr) * cr;
                    let b = l + 2.0 * (1.0 - kb) * cb;
                    let g = (l - kr * r - kb * b) / kg;
                    [r, g, b]
                }
            };
            f(x, rgb);
        }
    }
}

/// 按 irot（逆时针转 0/90/180/270 度）、imir（true 为左右镜像，false 为上下）摆正。
fn orient(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    rotation: ImageRotation,
    mirror: Option<bool>,
) -> (Vec<u8>, u32, u32) {
    let (w, h) = (width as usize, height as usize);
    let quarter_turns = match rotation {
        ImageRotation::D0 => 0,
        ImageRotation::D90 => 1,
        ImageRotation::D180 => 2,
        ImageRotation::D270 => 3,
    };
    if quarter_turns == 0 && mirror.is_none() {
        return (rgba, width, height);
    }
    let (out_w, out_h) = if quarter_turns % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u8; rgba.len()];
    for y in 0..h {
        for x in 0..w {
            // 逆时针转：原图 (x, y) 到新图的位置。
            let (mut nx, mut ny) = match quarter_turns {
                1 => (y, w - 1 - x),
                2 => (w - 1 - x, h - 1 - y),
                3 => (h - 1 - y, x),
                _ => (x, y),
            };
            match mirror {
                Some(true) => nx = out_w - 1 - nx,
                Some(false) => ny = out_h - 1 - ny,
                None => {}
            }
            let from = (y * w + x) * 4;
            let to = (ny * out_w + nx) * 4;
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    (out, out_w as u32, out_h as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_avif_brands() {
        let mut head = vec![0, 0, 0, 0x1C];
        head.extend_from_slice(b"ftypavif\0\0\0\0mif1miafMA1B");
        assert!(is_avif(&head));
        let mut compatible = vec![0, 0, 0, 0x18];
        compatible.extend_from_slice(b"ftypmif1\0\0\0\0mif1avif");
        assert!(is_avif(&compatible));
        let mut heic = vec![0, 0, 0, 0x18];
        heic.extend_from_slice(b"ftypheic\0\0\0\0mif1heic");
        assert!(!is_avif(&heic));
        assert!(!is_avif(b"\x89PNG"));
    }

    #[test]
    fn orientation_rotates_anticlockwise_then_mirrors() {
        // 2x1：左 A、右 B。逆时针转 90 度后是 1x2：上 B、下 A。
        let pixels = vec![1, 1, 1, 1, 2, 2, 2, 2];
        let (out, w, h) = orient(pixels.clone(), 2, 1, ImageRotation::D90, None);
        assert_eq!((w, h), (1, 2));
        assert_eq!((out[0], out[4]), (2, 1));
        let (out, ..) = orient(pixels, 2, 1, ImageRotation::D0, Some(true));
        assert_eq!((out[0], out[4]), (2, 1));
    }

    #[test]
    fn broken_files_report_errors() {
        let mut data = vec![0, 0, 0, 0x18];
        data.extend_from_slice(b"ftypavif\0\0\0\0mif1avif");
        data.extend((0..500u32).map(|i| (i * 13 % 251) as u8));
        assert!(decode(&data, true).is_err());
    }

    #[test]
    fn sample_tables_are_walked_once_and_bad_ones_refused() {
        let size = |_: usize| Some(10usize);
        // 3 块：第 1 块 2 个采样，第 2 块起每块 1 个。
        assert_eq!(
            sample_layout(&[100, 200, 300], &[(1, 2), (2, 1)], 4, size),
            Some(vec![(100, 10), (110, 10), (200, 10), (300, 10)])
        );
        // 采样数比表里能给的少：到数就停。
        assert_eq!(sample_layout(&[100, 200], &[(1, 2)], 3, size), Some(vec![(100, 10), (110, 10), (200, 10)]));
        // 不够数、没有覆盖第一块、一块里没有采样、块比采样多、条目比块多：都是坏文件。
        assert_eq!(sample_layout(&[100], &[(1, 2)], 3, size), None);
        assert_eq!(sample_layout(&[100, 200], &[(2, 1)], 2, size), None);
        assert_eq!(sample_layout(&[100, 200], &[(1, 0)], 2, size), None);
        assert_eq!(sample_layout(&[100, 200, 300], &[(1, 1)], 2, size), None);
        assert_eq!(sample_layout(&[100], &[(1, 1), (1, 1)], 1, size), None);
        // 很大的表也只走一遍（以前每一块都把条目从头数一遍：10 万 × 10 万）。
        let offsets: Vec<u64> = (0..100_000).map(|i| i * 10).collect();
        let runs: Vec<(u32, u32)> = (1..=100_000).map(|i| (i, 1)).collect();
        let start = std::time::Instant::now();
        assert_eq!(sample_layout(&offsets, &runs, 100_000, size).map(|s| s.len()), Some(100_000));
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
    /// PQ 的信号值（亮度 → 信号，造输入用）。
    fn pq_signal(nits: f64) -> f64 {
        const M1: f64 = 2610.0 / 16384.0;
        const M2: f64 = 2523.0 / 4096.0 * 128.0;
        const C1: f64 = 3424.0 / 4096.0;
        const C2: f64 = 2413.0 / 4096.0 * 32.0;
        const C3: f64 = 2392.0 / 4096.0 * 32.0;
        let y = (nits / 10000.0).powf(M1);
        ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
    }

    /// 完整范围、4:4:4 的 10 位图像，一行；每个像素给 Y、U、V。
    fn planes10(pixels: &[[u16; 3]]) -> Planes {
        let plane = |c: usize| pixels.iter().map(|p| p[c]).collect::<Vec<u16>>();
        let size = (pixels.len(), 1);
        Planes {
            width: pixels.len() as u32,
            height: 1,
            bits: 10,
            layout: rav1d::include::dav1d::headers::DAV1D_PIXEL_LAYOUT_I444,
            nclx: Nclx { primaries: 9, transfer: 16, matrix: 9, full_range: true },
            planes: vec![plane(0), plane(1), plane(2)],
            sizes: vec![size; 3],
        }
    }

    /// 完整范围的 10 位透明通道，一行。
    fn alpha10(values: &[u16]) -> Planes {
        Planes {
            width: values.len() as u32,
            height: 1,
            bits: 10,
            layout: DAV1D_PIXEL_LAYOUT_I400,
            nclx: Nclx { primaries: 2, transfer: 2, matrix: 2, full_range: true },
            planes: vec![values.to_vec()],
            sizes: vec![(values.len(), 1)],
        }
    }

    fn pq_gray(nits: f64) -> [u16; 3] {
        [(pq_signal(nits) * 1023.0).round() as u16, 512, 512]
    }

    /// 预乘的透明（按非线性的值乘的）在换算之前除掉：半透明的 203 尼特的白和不透明的一样亮（约 240）。以前换算完
    /// 才除，显示成约 118，边上一圈发暗。
    #[test]
    fn premultiplied_alpha_is_undone_before_tone_mapping() {
        let white = pq_gray(203.0);
        let mut opaque = vec![0u8; 4];
        to_rgb(&planes10(&[white]), DAV1D_MC_BT2020_NCL, true, Conversion::new(9, 16), false, None, &mut opaque);
        assert!((237..=243).contains(&opaque[0]) && opaque[3] == 255, "{opaque:?}");
        let premultiplied = [(f64::from(white[0]) * 0.5).round() as u16, 512, 512];
        let half = alpha10(&[512]);
        let mut out = vec![0u8; 4];
        to_rgb(
            &planes10(&[premultiplied]),
            DAV1D_MC_BT2020_NCL,
            true,
            Conversion::new(9, 16),
            false,
            Some(&Alpha::new(&half, true)),
            &mut out,
        );
        assert!(out[..3].iter().zip(&opaque[..3]).all(|(a, b)| a.abs_diff(*b) <= 2), "{out:?} / {opaque:?}");
        assert_eq!(out[3], 128);
    }

    /// 看不见的全透明像素不算最亮处（不预乘的文件里它们可以是任何颜色）：看得见的最亮的 400 尼特正好到白。
    #[test]
    fn transparent_pixels_do_not_set_the_peak() {
        let planes = planes10(&[pq_gray(400.0), pq_gray(10000.0)]);
        let alpha = alpha10(&[1023, 0]);
        let mut out = vec![0u8; 8];
        to_rgb(&planes, DAV1D_MC_BT2020_NCL, true, Conversion::new(9, 16), true, Some(&Alpha::new(&alpha, false)), &mut out);
        assert_eq!(out[0], 255, "{out:?}");
        assert_eq!(out[7], 0);
        let mut without = vec![0u8; 8];
        to_rgb(&planes, DAV1D_MC_BT2020_NCL, true, Conversion::new(9, 16), true, None, &mut without);
        assert!(without[0] < 255, "没有透明通道时 10000 尼特那个算最亮，400 尼特的到不了白：{without:?}");
    }

    /// 容器里没有 nclx 时看 AV1 序列头里的色彩信息（不解码）：照样认出要自己解，解出来和有 nclx 时一样。
    #[test]
    fn sequence_header_colour_is_used_without_a_colr_box() {
        let data: &[u8] = include_bytes!("../../../crates/anyview-fixtures/data/avif_pq.avif");
        let context = mp4parse::read_avif(&mut Cursor::new(data), ParseStrictness::Normal).unwrap();
        let header = color_coded(&context, data).and_then(sequence_nclx).expect("序列头里有色彩信息");
        assert_eq!((header.primaries, header.transfer), (9, 16));
        let at = data.windows(8).position(|w| w == b"colrnclx").expect("有 colr 盒子");
        let mut stripped = data.to_vec();
        stripped[at..at + 4].copy_from_slice(b"free");
        let context = mp4parse::read_avif(&mut Cursor::new(&stripped), ParseStrictness::Normal).unwrap();
        assert!(nclx(&context).is_none());
        assert!(needs_own_color(&stripped));
        let (a, b) = (decode(data, true).unwrap(), decode(&stripped, true).unwrap());
        assert_eq!((a.pixel_format.as_str(), b.pixel_format.as_str()), ("YUV444P10 PQ", "YUV444P10 PQ"));
        assert!(a.rgba == b.rgba);
        assert!(sequence_nclx(&[]).is_none() && sequence_nclx(b"not av1").is_none());
    }

    /// 带 ICC 的按 ICC，但写着高动态范围（PQ、HLG）的照样按 nclx（ICC 表达不了色调映射）。
    #[test]
    fn hdr_colour_info_wins_over_icc() {
        let pq = Nclx { primaries: 9, transfer: 16, matrix: 9, full_range: false };
        let p3 = Nclx { primaries: 12, transfer: 13, matrix: 1, full_range: true };
        assert!(conversion_for(pq, true).is_some_and(|c| c.is_hdr()));
        assert!(conversion_for(p3, true).is_none());
        assert!(conversion_for(p3, false).is_some());
    }

    /// 测速度用：`ANYVIEW_TIME_AVIF=文件 cargo test --release -p anyview-core time_avif -- --ignored --nocapture`。
    #[test]
    #[ignore]
    fn time_avif() {
        let Some(path) = std::env::var_os("ANYVIEW_TIME_AVIF") else { return };
        let data = std::fs::read(&path).unwrap();
        for _ in 0..3 {
            let start = std::time::Instant::now();
            let raster = decode(&data, true).unwrap();
            eprintln!("{}x{} {} {:.0} ms", raster.width, raster.height, raster.pixel_format, start.elapsed().as_secs_f64() * 1000.0);
        }
    }

}
