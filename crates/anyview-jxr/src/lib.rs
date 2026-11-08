//! JPEG XR 解码：jxrlib 的薄外壳。输出 8 位的灰度、RGB 或 RGBA（未预乘，按容器写的方向摆正），
//! 高动态范围的格式已经按 scRGB → sRGB 换算好。交进来的数据要先由核心去掉元数据（jxrlib 解析元数据的代码不安全，见 shim.c）。
//! Windows 上系统的 WIC 先解，解不了的才到这里。jxrlib 没能编译进来时（cfg `jxr_unavailable`），`decode` 直接报不支持。

/// 解码结果。
/// 解不出来的原因（文字由 anyview-core 按界面语言写）。C 端写成"代号|参数"，这里拆开。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JxrError {
    /// 图像尺寸不对。
    BadSize(i64, i64),
    /// 图片太大（超过内存上限）。
    TooLarge(i64, i64),
    /// 不支持的像素格式。
    PixelFormat(String),
    /// 内存不够。
    NoMemory(i64, i64),
    /// jxrlib 自己报的错误码。
    Library(i64),
    /// 这台机器上编译时没能编进 jxrlib。
    Unavailable,
}

impl JxrError {
    #[cfg_attr(jxr_unavailable, allow(dead_code))]
    fn parse(text: &str, code: i32) -> Self {
        let mut parts = text.split('|');
        let kind = parts.next().unwrap_or("");
        let numbers: Vec<i64> = text.split('|').skip(1).filter_map(|p| p.parse().ok()).collect();
        let pair = |f: fn(i64, i64) -> Self| f(numbers.first().copied().unwrap_or(0), numbers.get(1).copied().unwrap_or(0));
        match kind {
            "size" => pair(Self::BadSize),
            "large" => pair(Self::TooLarge),
            "memory" => pair(Self::NoMemory),
            "format" => Self::PixelFormat(parts.next().unwrap_or("").to_string()),
            "library" => Self::Library(numbers.first().copied().unwrap_or(i64::from(code))),
            _ => Self::Library(i64::from(code)),
        }
    }
}

pub struct JxrImage {
    pub width: u32,
    pub height: u32,
    /// 1 灰度，3 RGB，4 RGBA（未预乘）。
    pub channels: u32,
    /// 文件里的像素格式，如 "RGB24"、"RGBA64Half"。
    pub pixel_format: String,
    /// `width * height * channels` 字节。
    pub pixels: Vec<u8>,
}

#[cfg(not(jxr_unavailable))]
mod ffi {
    use std::os::raw::{c_char, c_int, c_uchar, c_uint};

    #[repr(C)]
    pub struct AnyviewJxrImage {
        pub width: c_uint,
        pub height: c_uint,
        pub channels: c_uint,
        pub pixel_format: [c_char; 32],
        pub pixels: *mut c_uchar,
    }

    unsafe extern "C" {
        pub fn anyview_jxr_decode(
            data: *const c_uchar,
            size: usize,
            max_bytes: usize,
            image: *mut AnyviewJxrImage,
            error: *mut c_char,
            error_size: usize,
        ) -> c_int;
        pub fn anyview_jxr_free(image: *mut AnyviewJxrImage);
    }
}

/// 解码内存里的一个 JPEG XR 文件。`max_bytes` 是输出像素的上限（超过时报错，不分配）。
#[cfg(not(jxr_unavailable))]
pub fn decode(data: &[u8], max_bytes: usize) -> Result<JxrImage, JxrError> {
    use std::ffi::CStr;
    let mut image = ffi::AnyviewJxrImage {
        width: 0,
        height: 0,
        channels: 0,
        pixel_format: [0; 32],
        pixels: std::ptr::null_mut(),
    };
    let mut error = [0 as std::os::raw::c_char; 256];
    // SAFETY: data 在调用期间有效、只读；image、error 是这里的局部变量，大小与 C 端一致。
    let code = unsafe {
        ffi::anyview_jxr_decode(
            data.as_ptr(),
            data.len(),
            max_bytes,
            &mut image,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if code != 0 {
        // SAFETY: C 端保证 error 以 0 结尾（snprintf）。
        let message = unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        return Err(JxrError::parse(&message, code));
    }
    let len = image.width as usize * image.height as usize * image.channels as usize;
    // SAFETY: 成功时 pixels 指向 len 字节，由 C 端分配，复制后释放。
    let result = unsafe {
        let pixels = std::slice::from_raw_parts(image.pixels, len).to_vec();
        let pixel_format = CStr::from_ptr(image.pixel_format.as_ptr())
            .to_string_lossy()
            .into_owned();
        ffi::anyview_jxr_free(&mut image);
        JxrImage {
            width: image.width,
            height: image.height,
            channels: image.channels,
            pixel_format,
            pixels,
        }
    };
    Ok(result)
}

/// jxrlib 在这台机器上编译不过（build.rs 打过警告）：报不支持。
#[cfg(jxr_unavailable)]
pub fn decode(_data: &[u8], _max_bytes: usize) -> Result<JxrImage, JxrError> {
    Err(JxrError::Unavailable)
}
