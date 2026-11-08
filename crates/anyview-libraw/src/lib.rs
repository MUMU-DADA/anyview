//! RAW 完整解码：LibRaw（third_party/libraw，按 CDDL 使用）从源代码编译，
//! 这里只有一个函数：把内存里的 RAW 文件显影成 8 位 sRGB（相机白平衡，按需摆正）。

use std::ffi::{CStr, c_char, c_int, c_void};

#[repr(C)]
struct RawImage {
    rgb: *mut u8,
    width: c_int,
    height: c_int,
}

unsafe extern "C" {
    fn anyview_libraw_decode(
        data: *const c_void,
        size: usize,
        auto_orient: c_int,
        out: *mut RawImage,
        error: *mut c_char,
        error_len: usize,
    ) -> c_int;
    fn anyview_libraw_free(rgb: *mut u8);
    fn anyview_libraw_version() -> *const c_char;
}

/// 显影结果：RGB8，按行连续存放。
pub struct Developed {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// 显影一个 RAW 文件；`auto_orient` 为真时按文件里的方向摆正。认不出、坏了时返回 LibRaw 的说明。
pub fn develop(data: &[u8], auto_orient: bool) -> Result<Developed, String> {
    let mut out = RawImage {
        rgb: std::ptr::null_mut(),
        width: 0,
        height: 0,
    };
    let mut error = [0 as c_char; 256];
    // SAFETY：data 在调用期间有效；外壳只读它，结果由 malloc 分配，复制后用 anyview_libraw_free 释放。
    let rc = unsafe {
        anyview_libraw_decode(
            data.as_ptr().cast(),
            data.len(),
            c_int::from(auto_orient),
            &mut out,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if rc != 0 || out.rgb.is_null() {
        // SAFETY：外壳总把 error 写成以 0 结尾的字符串（或保持全 0）。
        let message = unsafe { CStr::from_ptr(error.as_ptr()) };
        return Err(message.to_string_lossy().into_owned());
    }
    let (width, height) = (out.width.max(0) as u32, out.height.max(0) as u32);
    let len = width as usize * height as usize * 3;
    // SAFETY：外壳分配了 width × height × 3 字节。
    let rgb = unsafe { std::slice::from_raw_parts(out.rgb, len) }.to_vec();
    unsafe { anyview_libraw_free(out.rgb) };
    Ok(Developed { width, height, rgb })
}

/// LibRaw 的版本号（如 "0.22.0-Snapshot"）。
pub fn version() -> String {
    // SAFETY：LibRaw 返回静态字符串。
    unsafe { CStr::from_ptr(anyview_libraw_version()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_raw_data() {
        assert!(develop(b"not a raw file at all", true).is_err());
        assert!(!version().is_empty());
    }
}
