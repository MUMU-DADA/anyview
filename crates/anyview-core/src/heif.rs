//! HEIC、HEIF（和 Linux 上的 AVIF）：运行时打开 libheif（LGPL-3.0），按它的 C 接口解码。只动态加载，不连进核心里。
//!
//! - Linux：系统里的 `libheif.so.1`，由界面登记成系统解码器（HEIC、HEIF、AVIF）。没装时这几种格式就不认。
//!   AVIF 用它带的 dav1d 或 aom 插件解，动态 AVIF 只显示第一帧。
//! - Windows：程序自己带的 `heif.dll`（libheif 加 libde265，从 third_party 里的源代码编，`tools/windows/build-heif.sh`）。
//!   系统的解码器（WIC，要装微软的 HEVC 扩展）没有或解不了 HEIC 时，核心旁边有这个文件就用它；没有就和原来
//!   一样报不支持。它只带 HEVC 解码器，AVIF 仍由核心自己的解码器解。
//! - macOS 用系统的 ImageIO，不编这个模块。
//!
//! libheif 自己按文件里的旋转、镜像摆正。文件里带 ICC 时转成 sRGB 再交出去（Linux、Windows 在核心里转）。
//! 文件里的 Exif 块读出拍摄信息（相机、镜头、拍摄参数、拍摄时间）。

use crate::i18n::Message;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::decode::{PlatformDecode, PlatformImage, register_platform_decoder};
use libloading::Library;

/// 登记成系统解码器时（Linux）libheif 负责的扩展名。
const EXTENSIONS: &[&str] = &["heic", "heif", "hif", "heics", "avif", "avifs"];

#[repr(C)]
#[derive(Clone, Copy)]
struct HeifError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

impl HeifError {
    fn check(self) -> Result<(), Message> {
        if self.code == 0 {
            return Ok(());
        }
        let message = if self.message.is_null() {
            String::new()
        } else {
            // SAFETY: libheif 保证 message 是以 0 结尾的静态字符串（"is always defined"）。
            unsafe { CStr::from_ptr(self.message) }.to_string_lossy().into_owned()
        };
        Err(msg!("libheif 错误 {code}：{message}", "libheif error {code}: {message}", code = self.code.to_string(), message = message))
    }
}

const COLORSPACE_RGB: c_int = 1;
const CHROMA_INTERLEAVED_RGBA: c_int = 11;
const CHANNEL_INTERLEAVED: c_int = 10;
const PROFILE_PROF: u32 = u32::from_be_bytes(*b"prof");
const PROFILE_RICC: u32 = u32::from_be_bytes(*b"rICC");

/// 解出的主图。
struct Decoded {
    width: u32,
    height: u32,
    /// RGBA8，未预乘。
    rgba: Vec<u8>,
    has_alpha: bool,
    icc: Option<Vec<u8>>,
    /// Exif 块里的 TIFF 数据（从 "II*\0" 或 "MM\0*" 开始）。
    exif: Option<Vec<u8>>,
}

/// Exif 块最大多少字节（再大的不是正常的 Exif）。
const MAX_EXIF: usize = 4 << 20;

/// 读 Exif 块用的几个函数（libheif 1.x 早就有；万一缺了只是没有拍摄信息）。
#[derive(Clone, Copy)]
struct MetadataApi {
    count: unsafe extern "C" fn(*const c_void, *const c_char) -> c_int,
    ids: unsafe extern "C" fn(*const c_void, *const c_char, *mut u32, c_int) -> c_int,
    size: unsafe extern "C" fn(*const c_void, u32) -> usize,
    data: unsafe extern "C" fn(*const c_void, u32, *mut c_void) -> HeifError,
}

/// 用到的 libheif 函数（C 接口，1.x 以来一直不变）。
struct Api {
    _library: Library,
    init: Option<unsafe extern "C" fn(*const c_void) -> HeifError>,
    initialized: std::sync::Once,
    context_alloc: unsafe extern "C" fn() -> *mut c_void,
    context_free: unsafe extern "C" fn(*mut c_void),
    read_from_memory:
        unsafe extern "C" fn(*mut c_void, *const c_void, usize, *const c_void) -> HeifError,
    primary_handle: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HeifError,
    handle_release: unsafe extern "C" fn(*const c_void),
    handle_has_alpha: unsafe extern "C" fn(*const c_void) -> c_int,
    profile_type: unsafe extern "C" fn(*const c_void) -> u32,
    profile_size: unsafe extern "C" fn(*const c_void) -> usize,
    profile_data: unsafe extern "C" fn(*const c_void, *mut c_void) -> HeifError,
    decode_image:
        unsafe extern "C" fn(*const c_void, *mut *mut c_void, c_int, c_int, *const c_void) -> HeifError,
    image_width: unsafe extern "C" fn(*const c_void, c_int) -> c_int,
    image_height: unsafe extern "C" fn(*const c_void, c_int) -> c_int,
    image_plane: unsafe extern "C" fn(*const c_void, c_int, *mut c_int) -> *const u8,
    image_release: unsafe extern "C" fn(*const c_void),
    metadata: Option<MetadataApi>,
}

impl Api {
    fn load() -> Option<Self> {
        let library = open_library()?;
        // SAFETY: 下面按 libheif 头文件里的签名取函数。
        unsafe {
            macro_rules! symbol {
                ($name:literal) => {
                    *library.get($name).ok()?
                };
            }
            Some(Self {
                // libheif 1.13 起要先初始化（加载 dav1d 等插件）；更早的版本没有这个函数。第一次解码时再做，
                // 不拖慢启动。
                init: library
                    .get::<unsafe extern "C" fn(*const c_void) -> HeifError>(b"heif_init\0")
                    .ok()
                    .map(|f| *f),
                initialized: std::sync::Once::new(),
                context_alloc: symbol!(b"heif_context_alloc\0"),
                context_free: symbol!(b"heif_context_free\0"),
                read_from_memory: symbol!(b"heif_context_read_from_memory_without_copy\0"),
                primary_handle: symbol!(b"heif_context_get_primary_image_handle\0"),
                handle_release: symbol!(b"heif_image_handle_release\0"),
                handle_has_alpha: symbol!(b"heif_image_handle_has_alpha_channel\0"),
                profile_type: symbol!(b"heif_image_handle_get_color_profile_type\0"),
                profile_size: symbol!(b"heif_image_handle_get_raw_color_profile_size\0"),
                profile_data: symbol!(b"heif_image_handle_get_raw_color_profile\0"),
                decode_image: symbol!(b"heif_decode_image\0"),
                image_width: symbol!(b"heif_image_get_width\0"),
                image_height: symbol!(b"heif_image_get_height\0"),
                image_plane: symbol!(b"heif_image_get_plane_readonly\0"),
                image_release: symbol!(b"heif_image_release\0"),
                metadata: (|| {
                    Some(MetadataApi {
                        count: *library
                            .get(b"heif_image_handle_get_number_of_metadata_blocks\0")
                            .ok()?,
                        ids: *library
                            .get(b"heif_image_handle_get_list_of_metadata_block_IDs\0")
                            .ok()?,
                        size: *library.get(b"heif_image_handle_get_metadata_size\0").ok()?,
                        data: *library.get(b"heif_image_handle_get_metadata\0").ok()?,
                    })
                })(),
                _library: library,
            })
        }
    }

    /// 主图的 Exif 块里的 TIFF 数据（HEIF 规范附录 A：块的前 4 个字节是到 TIFF 头的偏移，
    /// 偏移写错的文件在前 64 个字节里找 TIFF 头）。没有时为空。
    ///
    /// # Safety
    /// `handle` 是 libheif 的有效图像句柄。
    unsafe fn exif(&self, handle: *const c_void) -> Option<Vec<u8>> {
        let api = self.metadata?;
        let filter = c"Exif".as_ptr();
        // SAFETY: 按 libheif 的接口约定使用；缓冲区按它报的大小分配。
        unsafe {
            let count = (api.count)(handle, filter);
            if count <= 0 {
                return None;
            }
            let mut ids = vec![0u32; count as usize];
            let got = (api.ids)(handle, filter, ids.as_mut_ptr(), count);
            let id = *ids.get(..got.clamp(0, count) as usize)?.first()?;
            let size = (api.size)(handle, id);
            if !(8..=MAX_EXIF).contains(&size) {
                return None;
            }
            let mut block = vec![0u8; size];
            (api.data)(handle, id, block.as_mut_ptr().cast()).check().ok()?;
            tiff_in_exif_block(&block).map(<[u8]>::to_vec)
        }
    }

    /// 解出主图：RGBA8（未预乘）、是否带透明、ICC、Exif。
    fn decode(&self, data: &[u8]) -> Result<Decoded, Message> {
        // SAFETY: 按 libheif 的接口约定使用；每个分配的对象都在离开前释放（Guard）。
        unsafe {
            if let Some(init) = self.init {
                self.initialized.call_once(|| {
                    let _ = init(std::ptr::null());
                });
            }
            let context = (self.context_alloc)();
            if context.is_null() {
                return Err(msg!("libheif 无法分配", "libheif could not allocate memory"));
            }
            let _context = Guard(|| (self.context_free)(context));
            (self.read_from_memory)(context, data.as_ptr().cast(), data.len(), std::ptr::null())
                .check()?;
            let mut handle = std::ptr::null_mut();
            (self.primary_handle)(context, &mut handle).check()?;
            let _handle = Guard(|| (self.handle_release)(handle));
            let has_alpha = (self.handle_has_alpha)(handle) != 0;
            let exif = self.exif(handle);
            let icc = match (self.profile_type)(handle) {
                PROFILE_PROF | PROFILE_RICC => {
                    let size = (self.profile_size)(handle);
                    let mut profile = vec![0u8; size];
                    (size > 0 && (self.profile_data)(handle, profile.as_mut_ptr().cast()).check().is_ok())
                        .then_some(profile)
                }
                _ => None,
            };
            let mut image = std::ptr::null_mut();
            (self.decode_image)(
                handle,
                &mut image,
                COLORSPACE_RGB,
                CHROMA_INTERLEAVED_RGBA,
                std::ptr::null(),
            )
            .check()?;
            let _image = Guard(|| (self.image_release)(image));
            let width = (self.image_width)(image, CHANNEL_INTERLEAVED);
            let height = (self.image_height)(image, CHANNEL_INTERLEAVED);
            let mut stride: c_int = 0;
            let plane = (self.image_plane)(image, CHANNEL_INTERLEAVED, &mut stride);
            if plane.is_null() || width <= 0 || height <= 0 || stride < width * 4 {
                return Err(msg!("libheif 没有给出图像", "libheif returned no image"));
            }
            let (w, h, stride) = (width as usize, height as usize, stride as usize);
            if (w as u64) * (h as u64) * 4 > 2 << 30 {
                return Err(msg!("图片太大：{w}x{h}", "Image too large: {w}x{h}", w = format!("{w}"), h = format!("{h}")));
            }
            let mut rgba = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                rgba.extend_from_slice(std::slice::from_raw_parts(plane.add(y * stride), w * 4));
            }
            Ok(Decoded {
                width: width as u32,
                height: height as u32,
                rgba,
                has_alpha,
                icc,
                exif,
            })
        }
    }
}

/// Exif 块里 TIFF 头的位置：先按块开头写的偏移，不对时在前 64 个字节里找。
fn tiff_in_exif_block(block: &[u8]) -> Option<&[u8]> {
    let is_tiff = |at: usize| {
        matches!(
            block.get(at..at.checked_add(4)?),
            Some([0x49, 0x49, 42, 0] | [0x4D, 0x4D, 0, 42])
        )
        .then_some(at)
    };
    let offset = u32::from_be_bytes(block.get(..4)?.try_into().ok()?) as usize;
    let start = 4usize
        .checked_add(offset)
        .and_then(is_tiff)
        .or_else(|| (0..block.len().min(64)).find_map(is_tiff))?;
    block.get(start..)
}

/// 离开作用域时调用一次（释放 libheif 的对象）。
struct Guard<F: FnMut()>(F);

impl<F: FnMut()> Drop for Guard<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// 打开 libheif。Linux 是系统里的 libheif.so.1。Windows 是程序自己带的 heif.dll：只从核心这个模块所在的文件夹里
/// 按完整路径加载，不按名字让系统去搜（免得加载到别处的同名文件）；没有这个文件就是没带。
fn open_library() -> Option<Library> {
    #[cfg(windows)]
    {
        let path = module_dir()?.join("heif.dll");
        if !path.is_file() {
            return None;
        }
        // SAFETY: 加载程序自带的 heif.dll；它只依赖 Windows 自带的系统库，加载时没有别的初始化。
        unsafe { Library::new(path).ok() }
    }
    #[cfg(not(windows))]
    {
        // SAFETY: 加载系统的 libheif。
        unsafe { Library::new("libheif.so.1").ok() }
    }
}

/// 核心这个模块（anyview_ffi.dll，或直接连了核心的程序）所在的文件夹。不用"程序所在的文件夹"：核心被别的进程加载时
/// （资源管理器的缩略图），程序是别人的。
#[cfg(windows)]
pub fn module_dir() -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
    }
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;
    let mut module = std::ptr::null_mut();
    let mut buffer = vec![0u16; 32768];
    // SAFETY: 按文档调用；缓冲区的长度如实给出，返回的长度不含结尾的 0。
    let length = unsafe {
        if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, module_dir as *const c_void, &mut module) == 0 {
            return None;
        }
        GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) as usize
    };
    if length == 0 || length >= buffer.len() {
        return None;
    }
    std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length]))
        .parent()
        .map(Path::to_path_buf)
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(Api::load).as_ref()
}

/// 有没有可用的 libheif（Linux：系统里装了；Windows：程序旁边带着 heif.dll）。
pub fn available() -> bool {
    api().is_some()
}

/// 文件头是不是 HEIF 容器里的 HEVC 图片（ftyp 的主品牌或兼容品牌里有 heic、heix 等）。AVIF 不算。
pub fn is_heic(head: &[u8]) -> bool {
    if head.len() < 12 || &head[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let end = size.clamp(12, head.len());
    // 主品牌在第 8 个字节，之后 4 个字节是版本，再往后每 4 个字节一个兼容品牌。
    let (major, _) = head[8..12].as_chunks::<4>();
    let (compatible, _) = head[16.min(end)..end].as_chunks::<4>();
    let mut hevc = false;
    for brand in major.iter().chain(compatible) {
        match brand {
            b"avif" | b"avis" => return false,
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"hevm" | b"hevs" => hevc = true,
            _ => {}
        }
    }
    hevc
}

/// 用 libheif 解一个文件的主图：BGRA 预乘、已经按文件摆正、已经转成 sRGB，带拍摄信息。没有 libheif、解不了时为空。
pub fn decode_file(path: &Path) -> Option<PlatformImage> {
    let api = api()?;
    let data = std::fs::read(path).ok()?;
    let Decoded {
        width,
        height,
        rgba: mut pixels,
        has_alpha,
        icc,
        exif,
    } = api.decode(&data).ok()?;
    // 在核心里转成 sRGB；要在预乘之前转。
    if let Some(icc) = &icc {
        crate::color::to_srgb(&mut pixels, icc, crate::color::ChannelOrder::Rgba);
    }
    crate::decode::rgba_to_bgra_premultiplied(&mut pixels);
    let shooting = exif
        .as_deref()
        .and_then(crate::exif::Exif::parse)
        .and_then(|e| e.shooting().non_empty());
    let format = match path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()) {
        Some(e) if e.starts_with("avif") => "AVIF",
        _ => "HEIF",
    };
    Some(PlatformImage {
        width,
        height,
        pixels,
        format: format.to_string(),
        pixel_format: if has_alpha { "RGBA32" } else { "RGB24" }.to_string(),
        has_alpha,
        icc_profile: None,
        shooting,
    })
}

struct HeifDecoder;

impl PlatformDecode for HeifDecoder {
    fn decode(&self, path: &Path, _auto_orient: bool, _use_icc: bool) -> Option<PlatformImage> {
        decode_file(path)
    }
}

/// 系统里有 libheif 时登记成系统解码器（HEIC、HEIF、AVIF），返回是否登记了。Linux 的界面启动时调用；
/// Windows 的系统解码器是 WIC，不登记，改由核心在 WIC 解不了时接手（decode.rs）。
pub fn register() -> bool {
    api().is_some()
        && register_platform_decoder(
            Arc::new(HeifDecoder),
            EXTENSIONS.iter().map(|e| e.to_string()).collect(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_fixtures_when_libheif_is_installed() {
        let Some(api) = api() else {
            eprintln!("没有 libheif，跳过");
            return;
        };
        let data = include_bytes!("../../../crates/anyview-fixtures/data/still.heic");
        let Decoded {
            width: w,
            height: h,
            rgba,
            has_alpha: alpha,
            exif,
            ..
        } = api.decode(data).unwrap();
        assert_eq!((w, h, alpha), (128, 96, false));
        assert!(exif.is_none());
        let at = |x: usize, y: usize| &rgba[(y * 128 + x) * 4..(y * 128 + x) * 4 + 3];
        let close = |got: &[u8], want: [u8; 3]| got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 2);
        assert!(close(at(32, 24), [220, 40, 40]), "{:?}", at(32, 24));
        assert!(close(at(96, 72), [240, 200, 40]), "{:?}", at(96, 72));
        let avif = include_bytes!("../../../crates/anyview-fixtures/data/still.avif");
        let Decoded {
            width: w,
            height: h,
            rgba,
            ..
        } = api.decode(avif).unwrap();
        assert_eq!((w, h), (128, 96));
        let at = |x: usize, y: usize| &rgba[(y * 128 + x) * 4..(y * 128 + x) * 4 + 3];
        assert!(close(at(96, 24), [40, 170, 70]), "{:?}", at(96, 24));
        assert!(api.decode(b"not a heif file").is_err());
        assert!(api.decode(&data[..200]).is_err());
    }

    /// 拍摄信息：exif.heic 是 heif-enc 从带 EXIF 的 JPEG 转来的（佳能 EOS R5，50mm f/2.8 1/250s ISO 400）。
    #[test]
    fn reads_shooting_info_from_the_exif_block() {
        if api().is_none() {
            eprintln!("没有 libheif，跳过");
            return;
        }
        let dir = std::env::temp_dir().join(format!("anyview-heif-exif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("exif.heic");
        std::fs::write(&path, include_bytes!("../../../crates/anyview-fixtures/data/exif.heic")).unwrap();
        let image = HeifDecoder.decode(&path, true, true).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let info = image.shooting.expect("没有读出拍摄信息");
        assert_eq!(info.model.as_deref(), Some("Canon EOS R5"));
        assert_eq!(info.lens.as_deref(), Some("RF50mm F1.8 STM"));
        assert_eq!(info.iso, Some(400));
        assert_eq!(info.f_number, Some(2.8));
        assert_eq!(info.exposure_time, Some(1.0 / 250.0));
        assert_eq!(info.taken.as_deref(), Some("2025:05:17 14:03:21"));
    }

    /// 按 ftyp 里的品牌认 HEVC 的 HEIF：主品牌或兼容品牌里有 heic 等算；AVIF、别的文件不算。
    #[test]
    fn recognises_hevc_heif_by_its_brands() {
        let ftyp = |brands: &[&[u8; 4]]| {
            let mut v = Vec::new();
            v.extend_from_slice(&((8 + 4 + 4 * brands.len()) as u32).to_be_bytes());
            v.extend_from_slice(b"ftyp");
            v.extend_from_slice(brands[0]);
            v.extend_from_slice(&[0, 0, 0, 0]);
            for b in &brands[1..] {
                v.extend_from_slice(*b);
            }
            v.extend_from_slice(b"....mdat");
            v
        };
        assert!(is_heic(&ftyp(&[b"heic", b"mif1", b"heic"])));
        assert!(is_heic(&ftyp(&[b"mif1", b"mif1", b"heic", b"miaf"])));
        assert!(is_heic(&ftyp(&[b"heix"])));
        assert!(!is_heic(&ftyp(&[b"avif", b"mif1", b"miaf"])));
        assert!(!is_heic(&ftyp(&[b"mif1", b"avif", b"heic"])), "带 avif 品牌的按 AVIF 算");
        assert!(!is_heic(&ftyp(&[b"mif1", b"miaf"])));
        assert!(!is_heic(&ftyp(&[b"isom", b"mp42"])));
        assert!(!is_heic(b"\x89PNG\r\n\x1a\n...."));
        assert!(!is_heic(b""));
        assert!(is_heic(include_bytes!("../../../crates/anyview-fixtures/data/still.heic")));
        assert!(!is_heic(include_bytes!("../../../crates/anyview-fixtures/data/still.avif")));
    }

    #[test]
    fn finds_the_tiff_header_in_exif_blocks() {
        let mut block = vec![0, 0, 0, 6];
        block.extend_from_slice(b"Exif\0\0II*\0\x08\0\0\0");
        assert_eq!(tiff_in_exif_block(&block).map(|t| &t[..4]), Some(&b"II*\0"[..]));
        // 偏移写错（指到外面）时在前 64 个字节里找。
        block[3] = 200;
        assert_eq!(tiff_in_exif_block(&block).map(|t| &t[..4]), Some(&b"II*\0"[..]));
        assert!(tiff_in_exif_block(b"\0\0\0\0Exif").is_none());
        assert!(tiff_in_exif_block(&[]).is_none());
    }
}
