//! 供跨语言界面层零复制访问像素缓冲区的 C 接口：
//! 建核心、读设置、打开图片、取出解码好的图像。
//!
//! 除 UniFFI 生成的绑定外，保留这组轻量 C 接口供需要极简 ABI 的调用方使用。字符串一律是 UTF-8、以 0 结尾。
//! 图像直接借出核心的像素（BGRA8 预乘，不复制），用完要调 [`anyview_image_free`]。

use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyview_core as core;

/// 核心有新事件时在后台线程调用（界面转到自己的线程后再取事件）。
pub type WakeFn = extern "C" fn(context: *mut c_void);

struct CallbackListener {
    wake: WakeFn,
    /// 界面给的上下文指针，原样传回（存成整数，好在线程间传递）。
    context: usize,
}

impl core::CoreListener for CallbackListener {
    fn wake(&self) {
        (self.wake)(self.context as *mut c_void);
    }
}

/// 一个窗口的核心，加上还没取走的事件。
pub struct AnyviewCore {
    core: core::AppCore,
    pending: Mutex<VecDeque<core::Event>>,
}

/// 事件里的图像（[`anyview_core_next_image`]）。
#[repr(C)]
pub struct AnyviewImage {
    /// 1 新图片（复位查看状态），2 快速预览（拉伸到 `display_width` x `display_height` 显示），
    /// 3 同一张图片换了缓冲区（清晰图替换预览等），4 解码失败。
    pub kind: u32,
    pub generation: u64,
    /// 交给 GPU 的缓冲区（超大图是缩小的代理）：宽、高、每行字节数、像素。
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: *const u8,
    /// 原图尺寸（预览、代理时与缓冲区不同）。
    pub display_width: u32,
    pub display_height: u32,
    /// 解码耗时（毫秒）。
    pub decode_ms: f64,
    /// 借出的图像；用完调 [`anyview_image_free`]（失败时为空）。
    pub handle: *mut c_void,
}

/// 核心的版本号（静态字符串，不用释放）。
#[unsafe(no_mangle)]
pub extern "C" fn anyview_version() -> *const c_char {
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr().cast()
}

/// 建核心。`wake` 会在后台线程被调用。
#[unsafe(no_mangle)]
pub extern "C" fn anyview_core_new(wake: WakeFn, context: *mut c_void) -> *mut AnyviewCore {
    let listener = Arc::new(CallbackListener {
        wake,
        context: context as usize,
    });
    Box::into_raw(Box::new(AnyviewCore {
        core: core::AppCore::new(listener),
        pending: Mutex::new(VecDeque::new()),
    }))
}

/// 放掉核心。
///
/// # Safety
/// `core` 必须是 [`anyview_core_new`] 返回的、还没放掉的指针（或空）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anyview_core_free(core: *mut AnyviewCore) {
    if !core.is_null() {
        // SAFETY: 调用方保证指针来自 anyview_core_new 且只放一次。
        drop(unsafe { Box::from_raw(core) });
    }
}

/// UTF-8 字符串（空指针为空）。
///
/// # Safety
/// `text` 为空或指向以 0 结尾的字符串。
unsafe fn text(text: *const c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: 调用方保证以 0 结尾。
    Some(unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned())
}

/// 读设置文件；`path` 为空时用默认位置。返回是否读到了文件。
///
/// # Safety
/// `core` 来自 [`anyview_core_new`]；`path` 为空或以 0 结尾。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anyview_core_load_settings(core: *mut AnyviewCore, path: *const c_char) -> bool {
    // SAFETY: 见函数说明。
    let (Some(core), path) = (unsafe { core.as_ref() }, unsafe { text(path) }) else {
        return false;
    };
    matches!(
        core.core.load_settings(path.map(PathBuf::from)),
        core::settings::LoadStatus::Loaded
    )
}

/// 打开图片或文件夹（在后台读，结果以事件送达）。
///
/// # Safety
/// `core` 来自 [`anyview_core_new`]；`path` 以 0 结尾。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anyview_core_open(core: *mut AnyviewCore, path: *const c_char) {
    // SAFETY: 见函数说明。
    let (Some(core), Some(path)) = (unsafe { core.as_ref() }, unsafe { text(path) }) else {
        return;
    };
    core.core.dispatch(core::Command::Open { path });
}

/// 取下一个带图像的事件（其余事件丢掉）；没有时返回 false。
///
/// # Safety
/// `core` 来自 [`anyview_core_new`]；`out` 指向可写的 [`AnyviewImage`]。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anyview_core_next_image(core: *mut AnyviewCore, out: *mut AnyviewImage) -> bool {
    // SAFETY: 见函数说明。
    let (Some(core), Some(out)) = (unsafe { core.as_ref() }, unsafe { out.as_mut() }) else {
        return false;
    };
    let mut pending = core.pending.lock().unwrap_or_else(|p| p.into_inner());
    pending.extend(core.core.drain_events());
    while let Some(event) = pending.pop_front() {
        let (kind, generation, image, display) = match event {
            core::Event::ImageReady {
                generation, image, ..
            } => (1, generation, Some(image), None),
            core::Event::ImagePreview {
                generation,
                image,
                width,
                height,
                ..
            } => (2, generation, Some(image), Some((width, height))),
            core::Event::ImageUpdated {
                generation, image, ..
            } => (3, generation, Some(image), None),
            core::Event::ImageFailed { generation, .. } => (4, generation, None, None),
            _ => continue,
        };
        *out = match image {
            Some(image) => {
                let texture = image.proxy.clone().unwrap_or_else(|| Arc::clone(&image));
                let (display_width, display_height) = display.unwrap_or((image.width, image.height));
                AnyviewImage {
                    kind,
                    generation,
                    width: texture.width,
                    height: texture.height,
                    stride: texture.stride as u32,
                    pixels: texture.pixels.as_ptr(),
                    display_width,
                    display_height,
                    decode_ms: image.info.decode_ms,
                    handle: Box::into_raw(Box::new(texture)).cast(),
                }
            }
            None => AnyviewImage {
                kind,
                generation,
                width: 0,
                height: 0,
                stride: 0,
                pixels: std::ptr::null(),
                display_width: 0,
                display_height: 0,
                decode_ms: 0.0,
                handle: std::ptr::null_mut(),
            },
        };
        return true;
    }
    false
}

/// 放掉借出的图像。
///
/// # Safety
/// `handle` 是 [`AnyviewImage::handle`]（或空），只放一次。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anyview_image_free(handle: *mut c_void) {
    if !handle.is_null() {
        // SAFETY: handle 来自 anyview_core_next_image 的 Box<Arc<DecodedImage>>。
        drop(unsafe { Box::from_raw(handle.cast::<Arc<core::DecodedImage>>()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    static WAKES: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn wake(_context: *mut c_void) {
        WAKES.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn opens_an_image_through_the_c_interface() {
        let dir = std::env::temp_dir().join(format!("anyview-capi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.png");
        image::RgbImage::from_pixel(5, 3, image::Rgb([10, 20, 30]))
            .save(&file)
            .unwrap();
        // SAFETY: 测试按接口约定调用。
        unsafe {
            assert!(!CStr::from_ptr(anyview_version()).to_bytes().is_empty());
            let core = anyview_core_new(wake, std::ptr::null_mut());
            let path = CString::new(file.display().to_string()).unwrap();
            anyview_core_open(core, path.as_ptr());
            let mut image = std::mem::zeroed::<AnyviewImage>();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !anyview_core_next_image(core, &mut image) {
                assert!(Instant::now() < deadline, "没等到图像");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(WAKES.load(Ordering::SeqCst) > 0);
            assert_eq!((image.kind, image.width, image.height, image.stride), (1, 5, 3, 20));
            // BGRA：蓝 30、绿 20、红 10。
            assert_eq!(std::slice::from_raw_parts(image.pixels, 4), &[30, 20, 10, 255]);
            anyview_image_free(image.handle);
            anyview_core_free(core);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
