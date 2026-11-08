//! 解码（docs/ARCHITECTURE.md"解码"、docs/FORMATS.md）。
//!
//! 输出统一为 8 位 BGRA、预乘 Alpha：CoreGraphics、Direct2D、GDK 的原生格式，界面拿到后直接上传 GPU。
//! 常见格式由 image crate 等纯 Rust 解码器处理；按文件头识别格式，扩展名不对也能打开。
//! JPEG 走快速路径（[`crate::jpeg`]：直接解码成 BGRA）。
//! EXIF 方向暂时在解码时直接应用到像素，以后改为显示变换的一部分。
//! 超大图另附缩小的代理纹理（[`crate::proxy`]）。
//! 内嵌 ICC：Linux、Windows 在这里转成 sRGB；macOS 保留，交给 ColorSync。
//! 系统解码器（如 macOS 的 ImageIO）由界面登记，负责 HEIC、AVIF 等 Rust 还不能解的格式。

use std::fmt;

use crate::i18n::Message;
use std::fs;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use image::metadata::Orientation;
use image::{ColorType, DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

/// 单张图片解码后允许占用的最大内存（字节）。
pub(crate) const MAX_ALLOC: u64 = 2 << 30;

thread_local! {
    /// 这个线程上解码时的内存上限：平时是 [`MAX_ALLOC`]，生成缩略图时更小（[`with_alloc_limit`]）。
    static ALLOC_LIMIT: std::cell::Cell<u64> = const { std::cell::Cell::new(MAX_ALLOC) };
}

/// 当前线程解码时的内存上限（字节）。解码器按它拒绝太大的图片（不直接用 [`MAX_ALLOC`]）。
/// 用户选了"仍然打开"的文件（[`with_limits_lifted`]）没有上限。
pub(crate) fn alloc_limit() -> u64 {
    if limits_lifted() {
        return u64::MAX;
    }
    ALLOC_LIMIT.with(std::cell::Cell::get)
}

// ---- 被上限拦下的文件可以问用户要不要仍然打开 ----
//
// 各种上限（解码内存、SVGZ 解压后的大小、PSD 图层总量、压缩包解出的大小）是防坏文件把内存、磁盘用光的，
// 数值是估的，真有特别大的正常文件也会被拦下。所以解码器拦下文件时记一笔（[`limit_hit`]），核心据此
// 不报"解码失败"而是让界面问用户；用户说仍然打开，这个文件（压缩包里的图片是整个压缩包）之后就不设上限。

thread_local! {
    static LIMITS_LIFTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LIMIT_HIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 这个线程上正在解的文件不设上限（用户选了"仍然打开"）。
pub(crate) fn limits_lifted() -> bool {
    LIMITS_LIFTED.with(std::cell::Cell::get)
}

/// `lifted` 为真时不设上限地运行 `f`；结束时（包括 panic）恢复。
pub(crate) fn with_limits_lifted<T>(lifted: bool, f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            LIMITS_LIFTED.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(LIMITS_LIFTED.with(|c| c.replace(lifted)));
    f()
}

/// 解码器因为上限拒绝了文件时调用。
pub(crate) fn limit_hit() {
    LIMIT_HIT.with(|c| c.set(true));
}

/// 有没有被上限拦下过（不清掉）。
pub(crate) fn limit_was_hit() -> bool {
    LIMIT_HIT.with(std::cell::Cell::get)
}

/// 上一次 [`take_limit_hit`] 之后这个线程上有没有文件被上限拦下（取了就清掉）。
pub(crate) fn take_limit_hit() -> bool {
    LIMIT_HIT.with(|c| c.replace(false))
}

/// 用户说过"仍然打开"的文件（压缩包里的图片记的是压缩包）；只在这一次运行里有效。
static TRUSTED: OnceLock<std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>> = OnceLock::new();

fn trusted() -> std::sync::MutexGuard<'static, std::collections::HashSet<std::path::PathBuf>> {
    TRUSTED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 记下用户对这个文件选了"仍然打开"。压缩包里的图片（虚拟路径）对整个压缩包生效。
pub(crate) fn trust(path: &Path) {
    let file = crate::archive::split(path).map_or_else(|| path.to_path_buf(), |(archive, _)| archive);
    trusted().insert(file.clone());
    crate::archive::lift_limits(&file);
}

/// 用户对这个文件（或它所在的压缩包）选过"仍然打开"。
pub(crate) fn is_trusted(path: &Path) -> bool {
    let trusted = trusted();
    trusted.contains(path)
        || crate::archive::split(path).is_some_and(|(archive, _)| trusted.contains(&archive))
}

impl DecodeError {
    /// 是被上限拦下的（不是文件坏了）：解码器记过一笔，或者 image crate、压缩包报的就是超限。
    /// 调用前后各取一次 [`take_limit_hit`]，这里只看错误本身。
    pub(crate) fn is_limit(&self) -> bool {
        match self {
            DecodeError::Image(image::ImageError::Limits(_)) => true,
            DecodeError::Archive(e) => e.is_too_large(),
            _ => false,
        }
    }
}

/// 在更小的内存上限下运行 `f`（不超过 [`MAX_ALLOC`]）；结束时（包括 panic）恢复原来的上限。
/// 只管这个线程：解码器在调用的线程上检查尺寸、分配整张图。
pub(crate) fn with_alloc_limit<T>(limit: u64, f: impl FnOnce() -> T) -> T {
    struct Restore(u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            ALLOC_LIMIT.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(ALLOC_LIMIT.with(|c| c.replace(limit.min(MAX_ALLOC))));
    f()
}

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// 每行字节数。
    pub stride: usize,
    /// BGRA8，预乘 Alpha，按行连续存放。
    pub pixels: Vec<u8>,
    pub info: ImageInfo,
    /// 超大图交给 GPU 的缩小版（ARCHITECTURE"超大图"，[`crate::proxy`]）：界面用它做纹理，
    /// 拉伸到原图尺寸显示；查看计算和精确重绘仍按原图。
    pub proxy: Option<Arc<DecodedImage>>,
    /// 矢量图（SVG）的文件内容：精确重绘时按屏幕像素重新渲染（[`crate::svg`]）。
    pub vector: Option<Arc<Vec<u8>>>,
    /// AI 生成的图片里记的生成信息：看图时读（[`DecodeOptions::generation`]），生成缩略图时不读。
    pub generation: Option<Arc<crate::generation::GenerationInfo>>,
}

impl fmt::Debug for DecodedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl DecodedImage {
    /// 按行连续存放的图像（每行 `width * 4` 字节），没有代理纹理。
    pub fn new(width: u32, height: u32, pixels: Vec<u8>, info: ImageInfo) -> Self {
        Self {
            width,
            height,
            stride: width as usize * 4,
            pixels,
            info,
            proxy: None,
            vector: None,
            generation: None,
        }
    }

    /// 占用的内存（含代理纹理）。
    pub fn byte_len(&self) -> usize {
        self.pixels.len() + self.proxy.as_ref().map_or(0, |p| p.pixels.len())
    }
}

/// 内置解码器（PSD、DDS、PCX 等）的输出：RGBA8、未预乘。
/// 这里统一做色彩管理、预乘和换成 BGRA，各解码器只管把像素读出来。
#[derive(Debug)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// RGBA8，未预乘 Alpha，每行 `width * 4` 字节、连续存放。
    pub rgba: Vec<u8>,
    /// 原图带透明通道（显示时在透明处画棋盘格）。
    pub has_alpha: bool,
    /// 格式名，如 "PSD"、"PSB"、"DDS"、"PCX"。
    pub format: String,
    /// 像素格式，如 "RGB24"、"CMYK32"、"BC7"。
    pub pixel_format: String,
    /// 像素所在色彩空间的 ICC（RGB 或灰度的配置文件）；为空表示 sRGB。
    pub icc_profile: Option<Vec<u8>>,
}

/// 用 image crate 解内存里的一张图（RAW 里的预览、CLIP 的画布预览），内存上限按当前线程的 [`alloc_limit`]
/// （`image::load_from_memory` 用的是它自己固定的 512 MB，生成缩略图时的小上限管不到）。
pub(crate) fn load_limited(data: &[u8], format: ImageFormat) -> image::ImageResult<DynamicImage> {
    let mut reader = ImageReader::with_format(std::io::Cursor::new(data), format);
    let mut limits = Limits::default();
    limits.max_alloc = Some(alloc_limit());
    reader.limits(limits);
    reader.decode()
}

impl Raster {
    /// 按尺寸分配 RGBA 缓冲区（全 0）。尺寸为 0 或超过内存上限（[`alloc_limit`]）时报错，解码器不应自己 `vec![]` 大块内存。
    pub fn alloc(width: u32, height: u32) -> Result<Vec<u8>, Message> {
        let bytes = u64::from(width) * u64::from(height) * 4;
        if width == 0 || height == 0 {
            return Err(msg!("图像尺寸不对：{width}x{height}", "Invalid image size: {width}x{height}", width = format!("{width}"), height = format!("{height}")));
        }
        if bytes > alloc_limit() {
            limit_hit();
            return Err(msg!("图片太大：{width}x{height}", "Image too large: {width}x{height}", width = format!("{width}"), height = format!("{height}")));
        }
        Ok(vec![0u8; usize::try_from(bytes).map_err(|_| msg!("图片太大：{width}x{height}", "Image too large: {width}x{height}", width = format!("{width}"), height = format!("{height}")))?])
    }
}

/// 图片信息叠加和报告用的基本信息。
#[derive(Clone, Debug, PartialEq)]
pub struct ImageInfo {
    /// 格式名，如 "JPEG"、"PNG"。
    pub format: String,
    /// 原始像素格式，如 "RGB24"、"RGBA32"。
    pub pixel_format: String,
    pub file_size: u64,
    /// 显示时使用的 ICC 配置文件（只在 macOS 保留；其他平台已转成 sRGB，为空）。
    pub icc_profile: Option<Arc<Vec<u8>>>,
    /// 文件的修改时间（用于图片信息展示）。
    pub modified: Option<std::time::SystemTime>,
    /// 原图带透明通道：显示时在透明处画棋盘格。
    pub has_alpha: bool,
    /// 读取加解码耗时（毫秒）。
    pub decode_ms: f64,
    /// EXIF 里的拍摄信息；没有时为空。
    pub shooting: Option<crate::exif::ShootingInfo>,
    /// GIF 文件头里的背景色 0xRRGGBB（设置"为 GIF 图片使用透明背景"）；不是 GIF 或没有时为空。
    pub gif_background: Option<u32>,
}

#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    Unsupported,
    Image(image::ImageError),
    /// 解码器报告的错误（还没定语言的一句话）。
    Corrupt(Message),
    /// 压缩包里的图片解不出来。
    Archive(crate::archive::ArchiveError),
}

impl DecodeError {
    /// 显示给用户的原因（按界面语言出文字）。系统、库给的说明原样放进去。
    pub fn message(&self) -> Message {
        match self {
            DecodeError::Io(e) => msg!("无法读取文件：{e}", "Can't read the file: {e}", e = e.to_string()),
            DecodeError::Unsupported => msg!("不支持的图片格式", "Unsupported image format"),
            DecodeError::Image(e) => msg!("解码失败：{e}", "Decoding failed: {e}", e = e.to_string()),
            DecodeError::Corrupt(e) => msg!("解码失败：{e}", "Decoding failed: {e}", e = e.clone()),
            DecodeError::Archive(crate::archive::ArchiveError::Password) => {
                msg!("压缩包有密码，打不开", "The archive is password protected")
            }
            DecodeError::Archive(e) => {
                msg!("无法从压缩包里读出：{e}", "Can't read from the archive: {e}", e = e.message())
            }
        }
    }
}

impl fmt::Display for DecodeError {
    /// 日志、测试里用英文（界面用 [`DecodeError::message`]）。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message().text(crate::i18n::Lang::En))
    }
}

impl std::error::Error for DecodeError {}

impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self {
        DecodeError::Io(e)
    }
}

impl From<image::ImageError> for DecodeError {
    fn from(e: image::ImageError) -> Self {
        match e {
            image::ImageError::IoError(io) => DecodeError::Io(io),
            image::ImageError::Unsupported(_) => DecodeError::Unsupported,
            other => DecodeError::Image(other),
        }
    }
}

/// 解码选项（来自设置）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeOptions {
    /// 按 EXIF 方向摆正（"根据 EXIF 信息自动旋转图片"）。
    pub auto_orient: bool,
    /// 使用图片内嵌的 ICC（"使用图片文件的 ICC 设置"）；关掉时按 sRGB 显示。
    pub use_icc: bool,
    /// RAW 显示相机内嵌的预览图（设置"打开 RAW 图片文件时，显示已有缩略图"）；
    /// 关掉时交给系统解码器完整解码（macOS 的 ImageIO），没有系统解码器时仍显示预览图。
    pub raw_preview: bool,
    /// 读 AI 生成的图片里记的生成信息：看图时读，生成缩略图时不读（默认不读）。
    pub generation: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            auto_orient: true,
            use_icc: true,
            raw_preview: true,
            generation: false,
        }
    }
}

/// 系统解码器（如 macOS 的 ImageIO）。在核心的后台线程调用。
pub trait PlatformDecode: Send + Sync {
    /// 解码成 BGRA8 预乘的图像，`auto_orient` 为真时按 EXIF 方向摆正；`use_icc` 是设置"使用文件内的 ICC"
    /// （返回 ICC 的系统不用管，由这里按它取舍；解码时就转成 sRGB 的系统按它决定做不做）。不能解码时返回 None。
    fn decode(&self, path: &Path, auto_orient: bool, use_icc: bool) -> Option<PlatformImage>;

    /// 帧数（静态图为 1）。系统解码器能解动图时实现（如 macOS 的动态 AVIF、HEICS）。
    fn frame_count(&self, _path: &Path) -> u32 {
        1
    }

    /// 第 `index` 帧（BGRA8 预乘）和它的显示时长（毫秒，文件里写的原样）。
    fn decode_frame(&self, _path: &Path, _index: u32) -> Option<(PlatformImage, u32)> {
        None
    }
}

/// 系统解码器负责的文件的帧数（不是它负责的扩展名时为空）。
pub(crate) fn platform_frame_count(path: &Path) -> Option<u32> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if !platform_extensions().contains(&extension) {
        return None;
    }
    Some(PLATFORM.get()?.decoder.frame_count(path))
}

/// 系统解码器解出的第 `index` 帧和时长（毫秒）。
pub(crate) fn platform_decode_frame(path: &Path, index: u32) -> Option<(PlatformImage, u32)> {
    PLATFORM.get()?.decoder.decode_frame(path, index)
}

/// 系统解码器的结果。
pub struct PlatformImage {
    pub width: u32,
    pub height: u32,
    /// BGRA8 预乘，按行连续存放。
    pub pixels: Vec<u8>,
    pub format: String,
    pub pixel_format: String,
    pub has_alpha: bool,
    /// 像素所在的色彩空间（ICC）；为空表示 sRGB。
    pub icc_profile: Option<Vec<u8>>,
    /// 拍摄信息。
    pub shooting: Option<crate::exif::ShootingInfo>,
}

struct Registered {
    decoder: Arc<dyn PlatformDecode>,
    extensions: Vec<String>,
}

static PLATFORM: OnceLock<Registered> = OnceLock::new();

/// 登记系统解码器和它负责的扩展名（小写，不带点）。只能登记一次，返回是否成功。
pub fn register_platform_decoder(
    decoder: Arc<dyn PlatformDecode>,
    extensions: Vec<String>,
) -> bool {
    let extensions = extensions
        .into_iter()
        .map(|e| e.to_ascii_lowercase())
        .collect();
    PLATFORM
        .set(Registered {
            decoder,
            extensions,
        })
        .is_ok()
}

/// 系统解码器负责的扩展名。
pub fn platform_extensions() -> &'static [String] {
    PLATFORM
        .get()
        .map(|r| r.extensions.as_slice())
        .unwrap_or(&[])
}

fn platform_decode(
    path: &Path,
    start: Instant,
    metadata: &fs::Metadata,
    options: DecodeOptions,
) -> Option<DecodedImage> {
    let image = PLATFORM
        .get()?
        .decoder
        .decode(path, options.auto_orient, options.use_icc)?;
    from_platform(image, start, metadata, options)
}

/// 系统解码器（或 libheif）交出的图像 → 核心的图像；尺寸、像素数对不上的不要。
fn from_platform(
    image: PlatformImage,
    start: Instant,
    metadata: &fs::Metadata,
    options: DecodeOptions,
) -> Option<DecodedImage> {
    let expected = image.width as usize * image.height as usize * 4;
    if image.width == 0 || image.height == 0 || image.pixels.len() < expected {
        return None;
    }
    Some(DecodedImage::new(
        image.width,
        image.height,
        image.pixels,
        ImageInfo {
            format: image.format,
            pixel_format: image.pixel_format,
            file_size: metadata.len(),
            icc_profile: image
                .icc_profile
                .filter(|_| options.use_icc)
                .map(Arc::new),
            modified: metadata.modified().ok(),
            has_alpha: image.has_alpha,
            decode_ms: start.elapsed().as_secs_f64() * 1000.0,
            shooting: image.shooting.and_then(crate::exif::ShootingInfo::non_empty),
            gif_background: None,
        },
    ))
}

/// 按默认选项解码一个文件；超大图附上代理纹理。
pub fn decode_file(path: &Path) -> Result<DecodedImage, DecodeError> {
    decode_file_with(path, DecodeOptions::default())
}

/// 按设置的选项解码一个文件；超大图附上代理纹理。
pub fn decode_file_with(path: &Path, options: DecodeOptions) -> Result<DecodedImage, DecodeError> {
    decode_original(path, options).map(crate::proxy::attach)
}

/// 解码一个文件，不附代理纹理（缩略图用不到）。
pub(crate) fn decode_original(path: &Path, options: DecodeOptions) -> Result<DecodedImage, DecodeError> {
    let start = Instant::now();
    // 压缩包里的图片：先解到临时文件，解码那个文件；大小、时间按包里记的。
    let source = crate::archive::materialize(path).map_err(DecodeError::Archive)?;
    // 解码器读的是任意文件：万一出错（panic）也只当这张图解不出来。不然等结果的一方（当前图片、缩略图）
    // 永远等不到，这个文件之后也不会再试。
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decode_path(&source.file, start, options)
    }));
    let mut image = decoded.unwrap_or_else(|_| Err(DecodeError::Corrupt(msg!("解码器出错", "The decoder failed"))))?;
    if options.generation {
        image.generation = crate::generation::read(&source.file).map(Arc::new);
    }
    if let Some(entry) = source.entry {
        image.info.file_size = entry.size;
        image.info.modified = entry.modified;
    }
    Ok(image)
}

fn decode_path(path: &Path, start: Instant, options: DecodeOptions) -> Result<DecodedImage, DecodeError> {
    // 文件的大小、修改时间在读内容之前取这一次，各解码器的结果都记这一份：之后核对文件有没有改过
    // 是拿它和文件现在的比。读的过程中文件又变了的话，记下的是旧的，下次核对会发现、重新读。
    let metadata = fs::metadata(path)?;
    // 系统解码器负责的扩展名先交给它（如 HEIC、AVIF）。
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let platform = platform_extensions().contains(&extension);
    // 高动态范围、广色域的 AVIF（PQ、HLG、BT.2020、P3，没有 ICC）：Windows、Linux 的系统解码器（WIC、libheif）
    // 不做色调映射、不换原色，画面发灰，由核心自己解。macOS 的 ImageIO 自己会换算，照旧交给它。系统没有 AVIF 解码器时
    // 本来就由核心解（下面按文件头认），不用先看。
    // 核心解不了的（如拼接的 grid）照旧交给系统的解码器，至少能显示；系统的也解不了时报核心的错（不再解第二遍）。
    let own_avif = cfg!(not(target_os = "macos"))
        && platform
        && crate::avif::EXTENSIONS.contains(&extension.as_str())
        && fs::read(path).is_ok_and(|data| crate::avif::needs_own_color(&data));
    let own_error = match own_avif {
        true => match decode_own(Own::Avif, path, start, &metadata, options) {
            Ok(image) => return Ok(image),
            Err(error) => Some(error),
        },
        false => None,
    };
    if platform && let Some(image) = platform_decode(path, start, &metadata, options) {
        return Ok(image);
    }
    if let Some(error) = own_error {
        return Err(error);
    }
    let head = file_head(path, 64).unwrap_or_default();
    // HEIC：Windows 上系统的解码器（WIC，要装微软的 HEVC 扩展）没有或解不了时，程序带着 heif.dll 就用它。
    // Linux 的 libheif 是登记成系统解码器的，上面已经试过。
    #[cfg(windows)]
    if crate::heif::is_heic(&head)
        && let Some(image) = crate::heif::decode_file(path)
        && let Some(image) = from_platform(image, start, &metadata, options)
    {
        return Ok(image);
    }
    // 自写的解码器（PSD、DDS、PCX、RAW 内嵌预览）：按文件头（PCX、RAW 另看扩展名）认格式。
    if let Some(own) = sniff_own(&head, &extension) {
        return decode_own(own, path, start, &metadata, options);
    }
    match decode_with_rust(path, start, &metadata, options) {
        // Rust 认不出的格式（如扩展名写成 .jpg 的 HEIC）再试系统解码器（按文件头识别）。
        Err(DecodeError::Unsupported) => {
            platform_decode(path, start, &metadata, options).ok_or(DecodeError::Unsupported)
        }
        other => other,
    }
}

/// 自写或单独接入的解码器负责的格式（image crate 以外的）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Own {
    Psd,
    Dds,
    Pcx,
    Raw,
    Jxl,
    Svg,
    Jp2,
    Clip,
    Jxr,
    Avif,
    Radiance,
}

fn sniff_own(head: &[u8], extension: &str) -> Option<Own> {
    if head.starts_with(b"8BPS") {
        Some(Own::Psd)
    } else if head.starts_with(b"DDS ") {
        Some(Own::Dds)
    } else if crate::jxl::is_jxl(head) {
        Some(Own::Jxl)
    } else if crate::jp2::is_jpeg2000(head) {
        Some(Own::Jp2)
    } else if crate::clip::is_clip(head) {
        Some(Own::Clip)
    } else if crate::avif::is_avif(head) {
        // 系统的解码器（扩展名登记过的先试过了）解不了、或者没有时。
        Some(Own::Avif)
    } else if crate::radiance::is_radiance(head) {
        Some(Own::Radiance)
    } else if crate::jxr::is_jxr(head) {
        // Windows 先由系统的 WIC 解码（扩展名登记过的先试过了），WIC 解不了的才到这里。
        Some(Own::Jxr)
    } else if extension == "pcx" && head.first() == Some(&0x0A) {
        Some(Own::Pcx)
    } else if crate::raw::is_raw_extension(extension) {
        Some(Own::Raw)
    } else if crate::svg::is_svg_extension(extension) {
        Some(Own::Svg)
    } else {
        None
    }
}

fn decode_own(
    own: Own,
    path: &Path,
    start: Instant,
    metadata: &fs::Metadata,
    options: DecodeOptions,
) -> Result<DecodedImage, DecodeError> {
    let data = fs::read(path)?;
    // 自写的解码器读的是任意文件：万一出错（panic）也只当这张图解不出来。
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let raster = match own {
            Own::Psd => crate::psd::decode_with(&data, options.use_icc),
            Own::Dds => crate::dds::decode(&data),
            Own::Pcx => crate::pcx::decode(&data),
            Own::Jxl => crate::jxl::decode(&data),
            Own::Svg => crate::svg::decode(&data),
            Own::Jp2 => crate::jp2::decode(&data, options.use_icc),
            Own::Clip => crate::clip::decode(&data),
            Own::Jxr => crate::jxr::decode(&data, options.use_icc),
            Own::Avif => crate::avif::decode(&data, options.use_icc),
            Own::Radiance => crate::radiance::decode(&data),
            Own::Raw => return decode_raw(path, &data, start, metadata, options),
        }
        .map_err(DecodeError::Corrupt)?;
        let mut image = from_raster(raster, start, metadata, options);
        if own == Own::Svg {
            image.vector = Some(Arc::new(data.clone()));
        }
        Ok(image)
    }));
    decoded.unwrap_or_else(|_| Err(DecodeError::Corrupt(msg!("解码器出错", "The decoder failed"))))
}

/// 自写解码器的结果 → 显示用的 BGRA 预乘图像（色彩管理按平台）。
pub(crate) fn from_raster(
    raster: Raster,
    start: Instant,
    metadata: &fs::Metadata,
    options: DecodeOptions,
) -> DecodedImage {
    let Raster {
        width,
        height,
        mut rgba,
        has_alpha,
        format,
        pixel_format,
        icc_profile,
    } = raster;
    let icc = icc_profile.filter(|_| options.use_icc);
    let icc_profile = crate::color::manage(&mut rgba, icc, crate::color::ChannelOrder::Rgba);
    rgba_to_bgra_premultiplied(&mut rgba);
    DecodedImage::new(
        width,
        height,
        rgba,
        ImageInfo {
            format,
            pixel_format,
            file_size: metadata.len(),
            icc_profile,
            modified: metadata.modified().ok(),
            has_alpha,
            decode_ms: start.elapsed().as_secs_f64() * 1000.0,
            shooting: None,
            gif_background: None,
        },
    )
}

/// RAW：先显示相机内嵌的最大 JPEG 预览，按 RAW 自己的方向摆正；没有预览、预览坏了，
/// 或者设置关掉了预览时完整解码：先试系统解码器（macOS 的 ImageIO），再用 LibRaw 显影。
fn decode_raw(
    path: &Path,
    data: &[u8],
    start: Instant,
    metadata: &fs::Metadata,
    options: DecodeOptions,
) -> Result<DecodedImage, DecodeError> {
    let full = || -> Result<DecodedImage, DecodeError> {
        if let Some(image) = platform_decode(path, start, metadata, options) {
            return Ok(image);
        }
        develop_raw(path, data, start, metadata, options)
    };
    if !options.raw_preview {
        // 新机型 LibRaw 可能还没跟上（读错数据、偏色）：与相机的预览图差得多时改用预览图。
        return match full() {
            Ok(image) => match decode_raw_preview(path, data, start, metadata, options) {
                Ok(preview) if !looks_alike(&image, &preview) => Ok(preview),
                _ => Ok(image),
            },
            Err(_) => decode_raw_preview(path, data, start, metadata, options),
        };
    }
    match decode_raw_preview(path, data, start, metadata, options) {
        Ok(image) => Ok(image),
        Err(preview_error) => full().map_err(|_| preview_error),
    }
}

/// 完整解码与相机的预览图是不是同一幅画面：都缩成 16×12 格，比亮度分布的相关系数和整体的偏色。
/// 读错了数据会是花屏（相关性低），色彩数据不对会整体偏色。
fn looks_alike(full: &DecodedImage, preview: &DecodedImage) -> bool {
    const W: usize = 16;
    const H: usize = 12;
    let grid = |image: &DecodedImage| -> Vec<[f64; 3]> {
        let (w, h) = (image.width as usize, image.height as usize);
        let mut cells = vec![[0f64; 4]; W * H];
        let step_y = (h / (H * 8)).max(1);
        let step_x = (w / (W * 8)).max(1);
        for y in (0..h).step_by(step_y) {
            let row = &image.pixels[y * image.stride..];
            for x in (0..w).step_by(step_x) {
                let p = &row[x * 4..x * 4 + 3];
                let cell = &mut cells[(y * H / h) * W + x * W / w];
                cell[0] += f64::from(p[2]);
                cell[1] += f64::from(p[1]);
                cell[2] += f64::from(p[0]);
                cell[3] += 1.0;
            }
        }
        cells
            .iter()
            .map(|c| {
                let n = c[3].max(1.0);
                [c[0] / n, c[1] / n, c[2] / n]
            })
            .collect()
    };
    let (a, b) = (grid(full), grid(preview));
    let luma = |c: &[f64; 3]| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
    let (la, lb): (Vec<f64>, Vec<f64>) = a.iter().zip(&b).map(|(x, y)| (luma(x), luma(y))).unzip();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (ma, mb) = (mean(&la), mean(&lb));
    let (mut cov, mut va, mut vb) = (0.0, 0.0, 0.0);
    for (x, y) in la.iter().zip(&lb) {
        cov += (x - ma) * (y - mb);
        va += (x - ma) * (x - ma);
        vb += (y - mb) * (y - mb);
    }
    // 几乎纯色的画面比不了相关性，只看偏色。
    let correlated = va < 1.0 || vb < 1.0 || cov / (va.sqrt() * vb.sqrt()) > 0.6;
    // 偏色：红/绿、蓝/绿 的比例差超过 30%。
    let sums = |g: &[[f64; 3]]| g.iter().fold([1.0f64; 3], |s, c| [s[0] + c[0], s[1] + c[1], s[2] + c[2]]);
    let (sa, sb) = (sums(&a), sums(&b));
    let cast = |i: usize| ((sa[i] / sa[1]) / (sb[i] / sb[1])).ln().abs();
    correlated && cast(0) < 0.26 && cast(2) < 0.26
}

/// 拍摄信息：RAW 自己的 EXIF。
fn raw_shooting(data: &[u8]) -> Option<crate::exif::ShootingInfo> {
    crate::raw::exif_tiff(data)
        .as_deref()
        .and_then(crate::exif::Exif::parse)
        .map(|e| e.shooting())
        .and_then(crate::exif::ShootingInfo::non_empty)
}

fn raw_extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_default()
}

/// 用 LibRaw 完整显影：相机白平衡，输出 sRGB；格式写扩展名本身（预览图写"NEF/JPEG"）。
fn develop_raw(
    path: &Path,
    data: &[u8],
    start: Instant,
    metadata: &fs::Metadata,
    options: DecodeOptions,
) -> Result<DecodedImage, DecodeError> {
    let developed =
        anyview_libraw::develop(data, options.auto_orient)
            .map_err(|e| DecodeError::Corrupt(Message::raw(e)))?;
    let (width, height) = (developed.width, developed.height);
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) * 4 > alloc_limit() {
        return Err(DecodeError::Corrupt(msg!("RAW 的尺寸不对：{width}x{height}", "Invalid RAW size: {width}x{height}", width = format!("{width}"), height = format!("{height}"))));
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for rgb in developed.rgb.as_chunks::<3>().0 {
        pixels.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
    }
    Ok(DecodedImage::new(
        width,
        height,
        pixels,
        ImageInfo {
            format: raw_extension(path),
            pixel_format: "RGB24".to_string(),
            file_size: metadata.len(),
            icc_profile: None,
            modified: metadata.modified().ok(),
            has_alpha: false,
            decode_ms: start.elapsed().as_secs_f64() * 1000.0,
            shooting: raw_shooting(data),
            gif_background: None,
        },
    ))
}

/// 相机内嵌的最大 JPEG 预览。
fn decode_raw_preview(
    path: &Path,
    data: &[u8],
    start: Instant,
    metadata: &fs::Metadata,
    options: DecodeOptions,
) -> Result<DecodedImage, DecodeError> {
    let preview = crate::raw::find_preview(data)
        .ok_or_else(|| DecodeError::Corrupt(msg!("RAW 文件里没有能显示的预览图", "The RAW file has no preview that can be shown")))?;
    let jpeg = data
        .get(preview.offset..preview.offset.saturating_add(preview.len))
        .ok_or_else(|| DecodeError::Corrupt(msg!("RAW 的预览图超出文件", "The RAW preview extends past the end of the file")))?;
    // 预览图自己的 EXIF 方向不算数（一般没有），按 RAW 文件的方向摆正。
    let inner = DecodeOptions {
        auto_orient: false,
        ..options
    };
    let (width, height, pixels, icc_profile, jpeg_shooting) =
        match crate::jpeg::decode(jpeg, alloc_limit(), inner).map_err(DecodeError::Corrupt)? {
            Some(d) => (d.width, d.height, d.pixels, d.icc_profile, d.shooting),
            None => {
                // 灰度等少见的预览：交给 image crate。
                let image = load_limited(jpeg, ImageFormat::Jpeg)?;
                let mut rgba = image.into_rgba8();
                let (w, h) = rgba.dimensions();
                rgba_to_bgra_premultiplied(&mut rgba);
                (w, h, rgba.into_raw(), None, None)
            }
        };
    let orientation = match options.auto_orient {
        true => Orientation::from_exif(preview.orientation.clamp(1, 8) as u8)
            .unwrap_or(Orientation::NoTransforms),
        false => Orientation::NoTransforms,
    };
    let (pixels, width, height) = crate::jpeg::orient(pixels, width, height, orientation);
    let shooting = raw_shooting(data).or(jpeg_shooting);
    let extension = raw_extension(path);
    Ok(DecodedImage::new(
        width,
        height,
        pixels,
        ImageInfo {
            format: format!("{extension}/JPEG"),
            pixel_format: "RGB24".to_string(),
            file_size: metadata.len(),
            icc_profile,
            modified: metadata.modified().ok(),
            has_alpha: false,
            decode_ms: start.elapsed().as_secs_f64() * 1000.0,
            shooting,
            gif_background: None,
        },
    ))
}

fn decode_with_rust(
    path: &Path,
    start: Instant,
    metadata: &fs::Metadata,
    options: DecodeOptions,
) -> Result<DecodedImage, DecodeError> {
    let file_size = metadata.len();
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let format = reader.format().ok_or(DecodeError::Unsupported)?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(alloc_limit());
    reader.limits(limits);

    if format == ImageFormat::Jpeg {
        let data = fs::read(path)?;
        match crate::jpeg::decode(&data, alloc_limit(), options) {
            Ok(Some(jpeg)) => {
                return Ok(DecodedImage::new(
                    jpeg.width,
                    jpeg.height,
                    jpeg.pixels,
                    ImageInfo {
                        format: "JPEG".to_string(),
                        pixel_format: "RGB24".to_string(),
                        file_size,
                        icc_profile: jpeg.icc_profile,
                        modified: metadata.modified().ok(),
                        has_alpha: false,
                        decode_ms: start.elapsed().as_secs_f64() * 1000.0,
                        shooting: jpeg.shooting,
                        gif_background: None,
                    },
                ));
            }
            Err(message) => return Err(DecodeError::Corrupt(message)),
            // 灰度、CMYK 等：走下面的通用路径。
            Ok(None) => {}
        }
    }

    let mut decoder = reader.into_decoder()?;
    let orientation = match options.auto_orient {
        true => decoder.orientation().unwrap_or(Orientation::NoTransforms),
        false => Orientation::NoTransforms,
    };
    let color = decoder.color_type();
    let icc = decoder
        .icc_profile()
        .ok()
        .flatten()
        .filter(|_| options.use_icc);
    // 拍摄信息：JPEG、PNG、WebP 由解码器给出 EXIF；TIFF 文件本身就是 TIFF 结构，读文件开头。
    let shooting = match decoder.exif_metadata().ok().flatten() {
        Some(exif) => crate::exif::Exif::parse(&exif).map(|e| e.shooting()),
        None if format == ImageFormat::Tiff => tiff_head(path)
            .as_deref()
            .and_then(crate::exif::Exif::parse)
            .map(|e| e.shooting()),
        None => None,
    }
    .and_then(crate::exif::ShootingInfo::non_empty);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);

    // OpenEXR 是线性的浮点值：截到普通亮度再按 sRGB 编码。
    let rgba = if format == ImageFormat::OpenExr {
        hdr_to_srgb8(image)
    } else {
        image.into_rgba8()
    };
    let (width, height) = rgba.dimensions();
    let mut pixels = rgba.into_raw();
    // 内嵌 ICC：macOS 保留给 ColorSync；其他平台转成 sRGB（认不出的配置文件按 sRGB 显示）。
    let icc_profile = crate::color::manage(&mut pixels, icc, crate::color::ChannelOrder::Rgba);
    rgba_to_bgra_premultiplied(&mut pixels);

    Ok(DecodedImage::new(
        width,
        height,
        pixels,
        ImageInfo {
            format: format_name(format).to_string(),
            pixel_format: pixel_format_name(color).to_string(),
            file_size,
            icc_profile,
            modified: metadata.modified().ok(),
            has_alpha: color.has_alpha(),
            decode_ms: start.elapsed().as_secs_f64() * 1000.0,
            shooting,
            gif_background: match format {
                ImageFormat::Gif => file_head(path, 13 + 768)
                    .as_deref()
                    .and_then(crate::anim::gif_background),
                _ => None,
            },
        },
    ))
}

/// 线性浮点图像 → sRGB 8 位（超过 1 的亮部截到最亮，Alpha 照原样）。OpenEXR、Radiance HDR 共用。
pub(crate) fn hdr_to_srgb8(image: DynamicImage) -> image::RgbaImage {
    let float = image.into_rgba32f();
    let (width, height) = float.dimensions();
    let mut out = image::RgbaImage::new(width, height);
    for (o, p) in out.pixels_mut().zip(float.pixels()) {
        let [r, g, b, a] = p.0;
        o.0 = [
            crate::psd::linear_to_srgb8(r),
            crate::psd::linear_to_srgb8(g),
            crate::psd::linear_to_srgb8(b),
            (a.clamp(0.0, 1.0) * 255.0).round() as u8,
        ];
    }
    out
}

/// 文件开头的 `len` 个字节（文件更短时是整个文件）。
fn file_head(path: &Path, len: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut head = Vec::new();
    fs::File::open(path).ok()?.take(len).read_to_end(&mut head).ok()?;
    Some(head)
}

/// TIFF 文件开头的 1 MB：目录和拍摄信息一般都在这里（读不到的项就不显示）。
fn tiff_head(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut head = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(1 << 20)
        .read_to_end(&mut head)
        .ok()?;
    Some(head)
}

/// RGBA8 → BGRA8 预乘 Alpha（原地转换）。
pub fn rgba_to_bgra_premultiplied(pixels: &mut [u8]) {
    let (chunks, _) = pixels.as_chunks_mut::<4>();
    for p in chunks {
        let [r, g, b, a] = *p;
        if a == 255 {
            p[0] = b;
            p[2] = r;
        } else {
            let a = u32::from(a);
            p[0] = premultiply(b, a);
            p[1] = premultiply(g, a);
            p[2] = premultiply(r, a);
        }
    }
}

fn premultiply(channel: u8, alpha: u32) -> u8 {
    ((u32::from(channel) * alpha + 127) / 255) as u8
}

fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Png => "PNG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Tga => "TGA",
        ImageFormat::Pnm => "PNM",
        ImageFormat::Qoi => "QOI",
        ImageFormat::Ico => "ICO",
        ImageFormat::OpenExr => "EXR",
        _ => "其他",
    }
}

/// 格式化像素格式，如 "RGBA32"（每像素总位数）。
fn pixel_format_name(color: ColorType) -> &'static str {
    match color {
        ColorType::L8 => "Gray8",
        ColorType::La8 => "GrayA16",
        ColorType::Rgb8 => "RGB24",
        ColorType::Rgba8 => "RGBA32",
        ColorType::L16 => "Gray16",
        ColorType::La16 => "GrayA32",
        ColorType::Rgb16 => "RGB48",
        ColorType::Rgba16 => "RGBA64",
        ColorType::Rgb32F => "RGB96F",
        ColorType::Rgba32F => "RGBA128F",
        _ => "其他",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("anyview-decode-{}-{name}", std::process::id()))
    }

    #[test]
    fn embedded_icc_is_converted_or_kept_per_platform() {
        use image::ImageEncoder;
        let path = temp("linear.png");
        let gray = RgbImage::from_pixel(4, 4, Rgb([128, 128, 128]));
        let mut encoder =
            image::codecs::png::PngEncoder::new(std::fs::File::create(&path).unwrap());
        encoder
            .set_icc_profile(crate::color::linear_srgb_test_profile())
            .unwrap();
        encoder
            .write_image(gray.as_raw(), 4, 4, image::ExtendedColorType::Rgb8)
            .unwrap();
        let decoded = decode_file(&path).unwrap();
        let blue = decoded.pixels[0];
        if crate::color::converts_in_core() {
            // 线性 128 → sRGB 约 188；转换后不再带 ICC。
            assert!((186..=190).contains(&blue), "{blue}");
            assert!(decoded.info.icc_profile.is_none());
        } else {
            // macOS：像素不变，ICC 交给 ColorSync。
            assert_eq!(blue, 128);
            assert!(decoded.info.icc_profile.is_some());
        }
        std::fs::remove_file(&path).unwrap();
    }

    struct FakeDecoder;

    impl PlatformDecode for FakeDecoder {
        fn decode(&self, _path: &Path, _auto_orient: bool, _use_icc: bool) -> Option<PlatformImage> {
            Some(PlatformImage {
                width: 2,
                height: 1,
                pixels: vec![1, 2, 3, 255, 4, 5, 6, 255],
                format: "FAKE".into(),
                pixel_format: "RGB24".into(),
                has_alpha: false,
                icc_profile: None,
                shooting: None,
            })
        }
    }

    #[test]
    fn platform_decoder_handles_its_extensions() {
        assert!(register_platform_decoder(
            Arc::new(FakeDecoder),
            vec!["FAKE".into()]
        ));
        assert!(!register_platform_decoder(
            Arc::new(FakeDecoder),
            vec!["other".into()]
        ));
        assert!(crate::formats::is_supported_name("photo.fake"));
        assert!(crate::formats::supported_extensions().contains(&"fake"));
        let path = temp("x.fake");
        std::fs::write(&path, b"whatever").unwrap();
        let decoded = decode_file(&path).unwrap();
        assert_eq!(
            (decoded.width, decoded.height, decoded.info.format.as_str()),
            (2, 1, "FAKE")
        );
        assert_eq!(decoded.info.file_size, 8);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn premultiplies_and_swaps_channels() {
        let mut px = vec![200, 100, 50, 255, 200, 100, 50, 128, 10, 20, 30, 0];
        rgba_to_bgra_premultiplied(&mut px);
        assert_eq!(px, vec![50, 100, 200, 255, 25, 50, 100, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn decodes_png_with_alpha() {
        let path = temp("alpha.png");
        let mut img = RgbaImage::from_pixel(3, 2, Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 0, Rgba([0, 0, 255, 128]));
        img.save(&path).unwrap();
        let decoded = decode_file(&path).unwrap();
        assert_eq!((decoded.width, decoded.height, decoded.stride), (3, 2, 12));
        assert_eq!(decoded.info.format, "PNG");
        assert_eq!(decoded.info.pixel_format, "RGBA32");
        assert_eq!(&decoded.pixels[0..4], &[128, 0, 0, 128]); // 蓝色半透明 → BGRA 预乘
        assert_eq!(&decoded.pixels[4..8], &[0, 0, 255, 255]); // 红色不透明
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn detects_format_from_content_not_extension() {
        let path = temp("really-png.jpg");
        RgbImage::from_pixel(4, 4, Rgb([1, 2, 3]))
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();
        let decoded = decode_file(&path).unwrap();
        assert_eq!(decoded.info.format, "PNG");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn reports_bad_and_missing_files() {
        let path = temp("text.png");
        std::fs::write(&path, "这不是图片").unwrap();
        assert!(decode_file(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            decode_file(&temp("missing.png")),
            Err(DecodeError::Io(_))
        ));
    }

    /// 真正的灰度图带灰度配置文件（Photoshop 存的灰度图常见）：按灰度转，画面不乱，三个平台都不再带配置文件。
    #[test]
    fn gray_png_with_a_gray_profile_is_converted_not_scrambled() {
        use image::ImageEncoder;
        let path = std::env::temp_dir().join(format!("anyview-gray-icc-{}.png", std::process::id()));
        // 左半 128、右半 0：读乱了的话左右对不上。
        let gray: Vec<u8> = (0..8 * 4).map(|i| if i % 8 < 4 { 128 } else { 0 }).collect();
        let mut encoder =
            image::codecs::png::PngEncoder::new(std::fs::File::create(&path).unwrap());
        encoder
            .set_icc_profile(crate::color::linear_gray_test_profile())
            .unwrap();
        encoder
            .write_image(&gray, 8, 4, image::ExtendedColorType::L8)
            .unwrap();
        let decoded = decode_file(&path).unwrap();
        assert_eq!((decoded.width, decoded.height), (8, 4));
        assert!(decoded.info.icc_profile.is_none());
        for y in 0..4 {
            for x in 0..8 {
                let px = &decoded.pixels[y * decoded.stride + x * 4..][..4];
                if x < 4 {
                    // 线性 128 → sRGB 约 188。
                    assert!(px[..3].iter().all(|c| (186..=190).contains(c)), "({x},{y}) {px:?}");
                } else {
                    assert_eq!(&px[..3], &[0, 0, 0], "({x},{y})");
                }
                assert_eq!(px[3], 255);
            }
        }
        std::fs::remove_file(&path).unwrap();
    }
}
