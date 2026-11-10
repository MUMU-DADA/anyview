//! Windows 的系统集成里和系统打交道的部分：资源管理器的缩略图提供程序，
//! 以及往注册表里登记、撤销（清单由核心的 `winshell` 算出）。
//!
//! 缩略图：安装时把这个类登记到 `anyview_ffi.dll`，资源管理器在单独的进程（dllhost）里加载这个 DLL，
//! 调 [`DllGetClassObject`] 要类工厂、建对象，把文件内容当流交过来（`IInitializeWithStream`），再要一张位图
//! （`IThumbnailProvider::GetThumbnail`）。解码用核心的同一个函数（`thumbnail::file_thumbnail`）。
//! 哪些格式归我们做，由登记决定（设置里关掉的格式不登记），这里来什么做什么。
//!
//! 核心按路径解码（看文件头、扩展名，RAW、HEIC 等的库也按路径读），系统给的却是流、拿不到路径：把流抄到
//! 临时目录的一个文件里（扩展名取流的名字里的），解完就删。上次没来得及删的（进程被系统中止）下次启动时清掉。
//!
//! COM 接口直接按内存布局写（虚函数表），不引入别的库：一个对象里放几个接口指针，各指向自己的函数表。
//! 所有入口都接住内部的错误（panic），不让它穿过系统的调用。

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use anyview_core as core;

type Hresult = i32;

const S_OK: Hresult = 0;
const S_FALSE: Hresult = 1;
const E_NOINTERFACE: Hresult = 0x8000_4002_u32 as i32;
const E_POINTER: Hresult = 0x8000_4003_u32 as i32;
const E_FAIL: Hresult = 0x8000_4005_u32 as i32;
const E_UNEXPECTED: Hresult = 0x8000_FFFF_u32 as i32;
const E_OUTOFMEMORY: Hresult = 0x8007_000E_u32 as i32;
const CLASS_E_NOAGGREGATION: Hresult = 0x8004_0110_u32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: Hresult = 0x8004_0111_u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_ALREADY_INITIALIZED)：系统的约定，同一个对象只初始化一次。
const E_ALREADY_INITIALIZED: Hresult = 0x8007_04DF_u32 as i32;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const fn guid(value: u128) -> Guid {
    Guid {
        data1: (value >> 96) as u32,
        data2: (value >> 80) as u16,
        data3: (value >> 64) as u16,
        data4: (value as u64).to_be_bytes(),
    }
}

/// 缩略图提供程序的类号（登记到注册表的是同一个，核心 `winshell::THUMBNAIL_CLSID`）。
pub const CLSID_THUMBNAIL: Guid = guid(0x1dc1f4d7_6312_479a_9043_bca81ce0234b);

const IID_UNKNOWN: Guid = guid(0x00000000_0000_0000_c000_000000000046);
const IID_CLASS_FACTORY: Guid = guid(0x00000001_0000_0000_c000_000000000046);
const IID_THUMBNAIL_PROVIDER: Guid = guid(0xe357fccd_a995_4576_b01f_234630154e96);
const IID_INITIALIZE_WITH_STREAM: Guid = guid(0xb824b49d_22ac_4161_ac8a_9916e8fa3f7f);
const IID_INITIALIZE_WITH_FILE: Guid = guid(0xb7d14566_0509_4cce_a71f_0a554233bd9b);
const IID_INITIALIZE_WITH_ITEM: Guid = guid(0x7f73be3f_fb79_493c_a6c7_7ee14e245841);

/// 流里的文件最大多少字节：再大的不抄、不做缩略图（系统显示普通的文件图标）。和解码的内存上限同一个数。
const MAX_FILE: u64 = core::thumbnail::MAX_ALLOC;

/// 临时文件放在系统临时目录的这个子目录里。
const TEMP_DIR: &str = "Anyview-thumbnails";

/// 还活着的对象个数（测试用它看有没有漏掉的引用）。
static LIVE_OBJECTS: AtomicU32 = AtomicU32::new(0);
static SERVER_LOCKS: AtomicU32 = AtomicU32::new(0);
static TEMP_SERIAL: AtomicU32 = AtomicU32::new(0);

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(memory: *mut c_void);
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateDIBSection(
        dc: isize,
        info: *const BitmapInfoHeader,
        usage: u32,
        bits: *mut *mut u8,
        section: isize,
        offset: u32,
    ) -> isize;
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    colors_used: u32,
    colors_important: u32,
}

/// WTS_ALPHATYPE：位图有没有透明度（有的话是预乘的 ARGB）。
const WTSAT_RGB: u32 = 1;
const WTSAT_ARGB: u32 = 2;

fn guard(f: impl FnOnce() -> Hresult) -> Hresult {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(E_UNEXPECTED)
}

// ---------------------------------------------------------------------------------------------
// 系统给的接口：只声明用到的函数，其余的占位。

#[repr(C)]
struct UnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

/// IStream：IUnknown 3 个、ISequentialStream 的 Read、Write，然后是 Seek……Stat、Clone。
#[repr(C)]
struct StreamVtbl {
    unknown: UnknownVtbl,
    read: unsafe extern "system" fn(*mut c_void, *mut u8, u32, *mut u32) -> Hresult,
    write: usize,
    seek: unsafe extern "system" fn(*mut c_void, i64, u32, *mut u64) -> Hresult,
    set_size: usize,
    copy_to: usize,
    commit: usize,
    revert: usize,
    lock_region: usize,
    unlock_region: usize,
    stat: unsafe extern "system" fn(*mut c_void, *mut StatStg, u32) -> Hresult,
    clone: usize,
}

/// STATSTG（x64 上 80 字节）。
#[repr(C)]
struct StatStg {
    name: *mut u16,
    kind: u32,
    size: u64,
    modified: u64,
    created: u64,
    accessed: u64,
    mode: u32,
    locks_supported: u32,
    clsid: Guid,
    state_bits: u32,
    reserved: u32,
}

const _: () = assert!(std::mem::size_of::<StatStg>() == 80);

/// IShellItem：只用 GetDisplayName。
#[repr(C)]
struct ShellItemVtbl {
    unknown: UnknownVtbl,
    bind_to_handler: usize,
    get_parent: usize,
    get_display_name: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> Hresult,
    get_attributes: usize,
    compare: usize,
}

const SIGDN_FILESYSPATH: u32 = 0x8005_8000;

/// 别人的 COM 对象的一个引用：拿着时加一次引用，丢掉时放开。
struct ComRef(*mut c_void);

// SAFETY: 只在持有它的对象的方法里用；对象登记成单线程套间（STA），系统保证调用在同一个线程上排队进行。
unsafe impl Send for ComRef {}

impl ComRef {
    /// # Safety
    /// `pointer` 是有效的 COM 接口指针。
    unsafe fn retain(pointer: *mut c_void) -> Self {
        // SAFETY: 调用方保证；COM 对象的开头是虚函数表的指针，前三项是 IUnknown。
        unsafe {
            let vtbl = *pointer.cast::<*const UnknownVtbl>();
            ((*vtbl).add_ref)(pointer);
        }
        Self(pointer)
    }

    /// # Safety
    /// 这个引用指向的对象的虚函数表确实是 `T` 的布局。
    unsafe fn vtbl<T>(&self) -> &T {
        // SAFETY: 同上。
        unsafe { &**self.0.cast::<*const T>() }
    }
}

impl Drop for ComRef {
    fn drop(&mut self) {
        // SAFETY: retain 时加过引用，对象还活着。
        unsafe {
            let vtbl = *self.0.cast::<*const UnknownVtbl>();
            ((*vtbl).release)(self.0);
        }
    }
}

/// 以 0 结尾的 UTF-16 字符串。
///
/// # Safety
/// `text` 为空，或指向以 0 结尾的 UTF-16 字符串。
unsafe fn wide_to_string(text: *const u16) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut len = 0;
    // SAFETY: 调用方保证以 0 结尾。
    unsafe {
        while *text.add(len) != 0 {
            len += 1;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(text, len)))
    }
}

// ---------------------------------------------------------------------------------------------
// 临时文件

/// 一个临时文件：丢掉时删除。
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        // 目录空了就一起删掉（不空时删不掉，正好）：用完不在电脑上留东西。
        let _ = std::fs::remove_dir(temp_dir());
    }
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(TEMP_DIR)
}

/// 清掉上次留下的临时文件（进程被系统中止时来不及删）：只删 10 分钟以前的，正在用的不会这么旧。
fn clean_stale_temp_files() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Ok(entries) = std::fs::read_dir(temp_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > 600);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    });
}

/// 名字里的扩展名（小写）：只收字母和数字、最多 16 个，别的一律不要（这个名字要拼进临时文件的路径）。
fn safe_extension(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    let ok = !extension.is_empty() && extension.len() <= 16 && extension.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| extension.to_ascii_lowercase())
}

/// 流的名字里的扩展名、流的大小（字节）。问不到时扩展名为空（核心按文件头认大多数格式）、大小为 0（抄的时候边抄边数）。
///
/// # Safety
/// `stream` 是有效的 IStream。
unsafe fn stream_info(stream: &ComRef) -> (Option<String>, u64) {
    // SAFETY: 调用方保证是 IStream。
    let vtbl = unsafe { stream.vtbl::<StreamVtbl>() };
    // SAFETY: StatStg 全是整数和指针，全 0 是有效的值。
    let mut stat: StatStg = unsafe { std::mem::zeroed() };
    // SAFETY: stat 可写；名字由我们用 CoTaskMemFree 释放。
    if unsafe { (vtbl.stat)(stream.0, &mut stat, 0) } < 0 {
        return (None, 0);
    }
    // SAFETY: 成功时 name 为空或是以 0 结尾的字符串。
    let extension = unsafe { wide_to_string(stat.name) }.as_deref().and_then(safe_extension);
    // SAFETY: 系统用 CoTaskMemAlloc 分配的；空指针也可以传。
    unsafe { CoTaskMemFree(stat.name.cast()) };
    (extension, stat.size)
}

/// 把流从头抄到一个新的临时文件里（文件名带上 `extension`）。
///
/// # Safety
/// `stream` 是有效的 IStream。
unsafe fn copy_stream_to_temp(stream: &ComRef, extension: Option<&str>) -> Result<TempFile, Hresult> {
    // SAFETY: 调用方保证是 IStream。
    let vtbl = unsafe { stream.vtbl::<StreamVtbl>() };
    // SAFETY: 回到开头（STREAM_SEEK_SET = 0）；不支持定位的流就从当前位置读。
    unsafe { (vtbl.seek)(stream.0, 0, 0, std::ptr::null_mut()) };

    clean_stale_temp_files();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).map_err(|_| E_FAIL)?;
    let name = format!(
        "{}-{}.{}",
        std::process::id(),
        TEMP_SERIAL.fetch_add(1, Ordering::Relaxed),
        extension.unwrap_or("bin"),
    );
    let temp = TempFile(dir.join(name));
    let mut file = std::fs::File::create(&temp.0).map_err(|_| E_FAIL)?;
    let mut buffer = vec![0_u8; 1 << 20];
    let mut total = 0_u64;
    loop {
        let mut read = 0_u32;
        // SAFETY: buffer 有 buffer.len 字节可写。
        let result = unsafe { (vtbl.read)(stream.0, buffer.as_mut_ptr(), buffer.len() as u32, &mut read) };
        if result < 0 {
            return Err(E_FAIL);
        }
        if read == 0 {
            break;
        }
        total += u64::from(read);
        if total > MAX_FILE {
            return Err(E_FAIL);
        }
        file.write_all(&buffer[..read as usize]).map_err(|_| E_FAIL)?;
    }
    file.flush().map_err(|_| E_FAIL)?;
    Ok(temp)
}

// ---------------------------------------------------------------------------------------------
// 缩略图提供程序

/// 系统交来的文件：流（资源管理器平时给的），或路径（直接给文件、给项目时）。
enum Source {
    Stream(ComRef),
    Path(PathBuf),
}

/// 对象的内存布局：开头是四个接口指针，各指向自己的函数表；函数拿到的 `this` 指向其中一个，
/// 减去它是第几个就是对象的开头。
#[repr(C)]
struct Provider {
    thumbnail: *const ThumbnailVtbl,
    with_stream: *const InitializeVtbl,
    with_file: *const InitializeVtbl,
    with_item: *const InitializeVtbl,
    references: AtomicU32,
    source: Mutex<Option<Source>>,
}

const SLOT_THUMBNAIL: usize = 0;
const SLOT_STREAM: usize = 1;
const SLOT_FILE: usize = 2;
const SLOT_ITEM: usize = 3;

#[repr(C)]
struct ThumbnailVtbl {
    unknown: UnknownVtbl,
    get_thumbnail: unsafe extern "system" fn(*mut c_void, u32, *mut isize, *mut u32) -> Hresult,
}

/// IInitializeWithStream、IInitializeWithFile、IInitializeWithItem 的形状一样：Initialize(一个指针, 打开方式)。
#[repr(C)]
struct InitializeVtbl {
    unknown: UnknownVtbl,
    initialize: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> Hresult,
}

// SAFETY: 函数表里只有函数指针，不变。
unsafe impl Sync for ThumbnailVtbl {}
// SAFETY: 同上。
unsafe impl Sync for InitializeVtbl {}

const fn unknown_vtbl<const SLOT: usize>() -> UnknownVtbl {
    UnknownVtbl {
        query_interface: provider_query_interface::<SLOT>,
        add_ref: provider_add_ref::<SLOT>,
        release: provider_release::<SLOT>,
    }
}

static THUMBNAIL_VTBL: ThumbnailVtbl = ThumbnailVtbl {
    unknown: unknown_vtbl::<SLOT_THUMBNAIL>(),
    get_thumbnail: provider_get_thumbnail,
};
static STREAM_VTBL: InitializeVtbl = InitializeVtbl {
    unknown: unknown_vtbl::<SLOT_STREAM>(),
    initialize: provider_initialize_with_stream,
};
static FILE_VTBL: InitializeVtbl = InitializeVtbl {
    unknown: unknown_vtbl::<SLOT_FILE>(),
    initialize: provider_initialize_with_file,
};
static ITEM_VTBL: InitializeVtbl = InitializeVtbl {
    unknown: unknown_vtbl::<SLOT_ITEM>(),
    initialize: provider_initialize_with_item,
};

impl Provider {
    /// 新建一个对象，引用数为 1；返回它的开头（也就是 IThumbnailProvider 的指针）。
    fn create() -> *mut Provider {
        LIVE_OBJECTS.fetch_add(1, Ordering::Relaxed);
        Box::into_raw(Box::new(Provider {
            thumbnail: &THUMBNAIL_VTBL,
            with_stream: &STREAM_VTBL,
            with_file: &FILE_VTBL,
            with_item: &ITEM_VTBL,
            references: AtomicU32::new(1),
            source: Mutex::new(None),
        }))
    }

    /// 从某个接口指针回到对象。
    ///
    /// # Safety
    /// `this` 是本对象第 `SLOT` 个接口的指针，对象还活着。
    unsafe fn from_slot<'a, const SLOT: usize>(this: *mut c_void) -> &'a Provider {
        // SAFETY: 调用方保证；四个接口指针在对象开头连续排着。
        unsafe { &*this.cast::<*const c_void>().sub(SLOT).cast::<Provider>() }
    }

    fn slot(&self, slot: usize) -> *mut c_void {
        // 对象开头的四个指针连续排着（repr(C)）。
        (std::ptr::from_ref(self).cast::<*const c_void>()).wrapping_add(slot).cast_mut().cast()
    }

    /// 记下系统交来的文件；已经有了就不再收。
    fn set_source(&self, source: Source) -> Hresult {
        let mut current = self.source.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.is_some() {
            return E_ALREADY_INITIALIZED;
        }
        *current = Some(source);
        S_OK
    }

    /// 做缩略图：长边不超过 `side`。
    fn render(&self, side: u32) -> Result<core::decode::DecodedImage, Hresult> {
        let started = std::time::Instant::now();
        let source = self.source.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (extension, result) = match source.as_ref().ok_or(E_UNEXPECTED)? {
            Source::Path(path) => {
                let extension = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase());
                (extension, core::thumbnail::file_thumbnail(path, side))
            }
            Source::Stream(stream) => {
                // SAFETY: 初始化时交来的是 IStream。
                let (extension, size) = unsafe { stream_info(stream) };
                let result = if size > MAX_FILE {
                    Err("文件太大".to_string())
                } else {
                    // SAFETY: 同上。
                    match unsafe { copy_stream_to_temp(stream, extension.as_deref()) } {
                        Ok(temp) => core::thumbnail::file_thumbnail(&temp.0, side),
                        Err(_) => Err("抄到临时文件时出错".to_string()),
                    }
                };
                (extension, result)
            }
        };
        log(|| {
            format!(
                "缩略图 .{} 要 {side}：{}，{} ms",
                extension.as_deref().unwrap_or("（没有扩展名）"),
                match &result {
                    Ok(image) => format!("{}x{}", image.width, image.height),
                    Err(reason) => format!("没做（{reason}）"),
                },
                started.elapsed().as_millis(),
            )
        });
        result.map_err(|_| E_FAIL)
    }
}

unsafe extern "system" fn provider_query_interface<const SLOT: usize>(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: out 可写。
    unsafe { *out = std::ptr::null_mut() };
    if iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: 系统按约定传来本对象的接口指针和有效的接口号。
    let (provider, iid) = unsafe { (Provider::from_slot::<SLOT>(this), *iid) };
    let slot = if iid == IID_UNKNOWN || iid == IID_THUMBNAIL_PROVIDER {
        SLOT_THUMBNAIL
    } else if iid == IID_INITIALIZE_WITH_STREAM {
        SLOT_STREAM
    } else if iid == IID_INITIALIZE_WITH_FILE {
        SLOT_FILE
    } else if iid == IID_INITIALIZE_WITH_ITEM {
        SLOT_ITEM
    } else {
        return E_NOINTERFACE;
    };
    provider.references.fetch_add(1, Ordering::Relaxed);
    // SAFETY: out 可写。
    unsafe { *out = provider.slot(slot) };
    S_OK
}

unsafe extern "system" fn provider_add_ref<const SLOT: usize>(this: *mut c_void) -> u32 {
    // SAFETY: 系统按约定传来本对象的接口指针。
    let provider = unsafe { Provider::from_slot::<SLOT>(this) };
    provider.references.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn provider_release<const SLOT: usize>(this: *mut c_void) -> u32 {
    // SAFETY: 系统按约定传来本对象的接口指针。
    let provider = unsafe { Provider::from_slot::<SLOT>(this) };
    let left = provider.references.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        let pointer = std::ptr::from_ref(provider).cast_mut();
        // 丢掉对象时会放开流（别人的代码）：接住里面的错误。
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: 引用数到 0，没有别人再用；对象是 create 里用 Box 分配的。
            drop(unsafe { Box::from_raw(pointer) });
        }));
        LIVE_OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
    left
}

unsafe extern "system" fn provider_initialize_with_stream(this: *mut c_void, stream: *mut c_void, _mode: u32) -> Hresult {
    guard(|| {
        if stream.is_null() {
            return E_POINTER;
        }
        // SAFETY: 系统按约定传来本对象的接口指针。
        let provider = unsafe { Provider::from_slot::<SLOT_STREAM>(this) };
        // 已经初始化过时不碰这个流。
        if provider.source.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some() {
            return E_ALREADY_INITIALIZED;
        }
        // SAFETY: stream 是有效的 IStream（系统的约定）。
        provider.set_source(Source::Stream(unsafe { ComRef::retain(stream) }))
    })
}

unsafe extern "system" fn provider_initialize_with_file(this: *mut c_void, path: *mut c_void, _mode: u32) -> Hresult {
    guard(|| {
        // SAFETY: path 为空或是以 0 结尾的路径（系统的约定）。
        let Some(path) = (unsafe { wide_to_string(path.cast::<u16>()) }) else {
            return E_POINTER;
        };
        // SAFETY: 系统按约定传来本对象的接口指针。
        let provider = unsafe { Provider::from_slot::<SLOT_FILE>(this) };
        provider.set_source(Source::Path(PathBuf::from(path)))
    })
}

unsafe extern "system" fn provider_initialize_with_item(this: *mut c_void, item: *mut c_void, _mode: u32) -> Hresult {
    guard(|| {
        if item.is_null() {
            return E_POINTER;
        }
        // SAFETY: item 是有效的 IShellItem（系统的约定）；只在这次调用里用，不留着。
        let vtbl = unsafe { &**item.cast::<*const ShellItemVtbl>() };
        let mut name = std::ptr::null_mut();
        // SAFETY: name 可写；成功时是 CoTaskMemAlloc 分配的字符串。
        let result = unsafe { (vtbl.get_display_name)(item, SIGDN_FILESYSPATH, &mut name) };
        if result < 0 {
            // 不在文件系统里的项目（压缩文件夹里的、手机上的等）没有路径：不做。
            return E_FAIL;
        }
        // SAFETY: 成功时 name 是以 0 结尾的字符串，由我们释放。
        let path = unsafe { wide_to_string(name) };
        // SAFETY: 同上。
        unsafe { CoTaskMemFree(name.cast()) };
        let Some(path) = path else {
            return E_FAIL;
        };
        // SAFETY: 系统按约定传来本对象的接口指针。
        let provider = unsafe { Provider::from_slot::<SLOT_ITEM>(this) };
        provider.set_source(Source::Path(PathBuf::from(path)))
    })
}

unsafe extern "system" fn provider_get_thumbnail(this: *mut c_void, side: u32, bitmap: *mut isize, alpha: *mut u32) -> Hresult {
    if bitmap.is_null() || alpha.is_null() {
        return E_POINTER;
    }
    // SAFETY: 两个指针可写。
    unsafe {
        *bitmap = 0;
        *alpha = 0;
    }
    guard(|| {
        // SAFETY: 系统按约定传来本对象的接口指针。
        let provider = unsafe { Provider::from_slot::<SLOT_THUMBNAIL>(this) };
        let image = match provider.render(side) {
            Ok(image) => image,
            Err(error) => return error,
        };
        match to_bitmap(&image) {
            Ok((handle, kind)) => {
                // SAFETY: 两个指针可写。
                unsafe {
                    *bitmap = handle;
                    *alpha = kind;
                }
                S_OK
            }
            Err(error) => error,
        }
    })
}

/// 核心的图（BGRA8 预乘）变成系统要的位图：32 位、从上到下的 DIB，正好是同样的字节顺序。
/// 没有透明像素时告诉系统是不透明的（系统给不透明的缩略图加边框和阴影）。
fn to_bitmap(image: &core::decode::DecodedImage) -> Result<(isize, u32), Hresult> {
    let (width, height) = (image.width as usize, image.height as usize);
    let (Ok(w), Ok(h)) = (i32::try_from(width), i32::try_from(height)) else {
        return Err(E_FAIL);
    };
    if w == 0 || h == 0 || image.stride < width * 4 || image.pixels.len() < image.stride * (height - 1) + width * 4 {
        return Err(E_FAIL);
    }
    let header = BitmapInfoHeader {
        size: std::mem::size_of::<BitmapInfoHeader>() as u32,
        width: w,
        // 负的高度：第一行在最上面。
        height: -h,
        planes: 1,
        bit_count: 32,
        compression: 0,
        size_image: 0,
        x_pels_per_meter: 0,
        y_pels_per_meter: 0,
        colors_used: 0,
        colors_important: 0,
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: header 是完整的 BITMAPINFOHEADER（32 位没有调色板）；bits 可写。
    let handle = unsafe { CreateDIBSection(0, &header, 0, &mut bits, 0, 0) };
    if handle == 0 || bits.is_null() {
        return Err(E_OUTOFMEMORY);
    }
    // SAFETY: 系统给这个位图分配了 width * height * 4 字节。
    let target = unsafe { std::slice::from_raw_parts_mut(bits, width * height * 4) };
    let mut opaque = true;
    for (row, out) in target.chunks_exact_mut(width * 4).enumerate() {
        let source = &image.pixels[row * image.stride..row * image.stride + width * 4];
        out.copy_from_slice(source);
        opaque &= source.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255);
    }
    Ok((handle, if opaque { WTSAT_RGB } else { WTSAT_ARGB }))
}

// ---------------------------------------------------------------------------------------------
// 诊断记录

/// 诊断记录：系统临时目录里有 `Anyview-shell.log` 这个文件时（查问题时手工建一个空的），每次被系统调用往里追加一行；
/// 没有这个文件时什么也不写。
fn log(line: impl FnOnce() -> String) {
    let path = std::env::temp_dir().join("Anyview-shell.log");
    if !path.is_file() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&path) {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = writeln!(file, "{seconds} 进程 {}：{}", std::process::id(), line());
    }
}

// ---------------------------------------------------------------------------------------------
// 类工厂

/// 类工厂：整个进程共用一个，不计引用。
#[repr(C)]
struct Factory {
    vtbl: *const FactoryVtbl,
    /// 新建一个对象（引用数为 1），返回它的 IUnknown。
    create: fn() -> *mut c_void,
}

// SAFETY: 只有一个指向不变的函数表的指针。
unsafe impl Sync for Factory {}

#[repr(C)]
struct FactoryVtbl {
    unknown: UnknownVtbl,
    create_instance: unsafe extern "system" fn(*mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    lock_server: unsafe extern "system" fn(*mut c_void, i32) -> Hresult,
}

// SAFETY: 函数表里只有函数指针，不变。
unsafe impl Sync for FactoryVtbl {}

static FACTORY_VTBL: FactoryVtbl = FactoryVtbl {
    unknown: UnknownVtbl {
        query_interface: factory_query_interface,
        add_ref: factory_add_ref,
        release: factory_release,
    },
    create_instance: factory_create_instance,
    lock_server: factory_lock_server,
};

static THUMBNAIL_FACTORY: Factory = Factory {
    vtbl: &FACTORY_VTBL,
    create: || Provider::create().cast(),
};


unsafe extern "system" fn factory_query_interface(this: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: out 可写。
    unsafe { *out = std::ptr::null_mut() };
    if iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: 有效的接口号（系统的约定）。
    let iid = unsafe { *iid };
    if iid == IID_UNKNOWN || iid == IID_CLASS_FACTORY {
        // SAFETY: out 可写。
        unsafe { *out = this };
        S_OK
    } else {
        E_NOINTERFACE
    }
}

unsafe extern "system" fn factory_add_ref(_this: *mut c_void) -> u32 {
    2
}

unsafe extern "system" fn factory_release(_this: *mut c_void) -> u32 {
    1
}

unsafe extern "system" fn factory_lock_server(_this: *mut c_void, lock: i32) -> Hresult {
    if lock != 0 {
        SERVER_LOCKS.fetch_add(1, Ordering::Relaxed);
    } else {
        SERVER_LOCKS.fetch_sub(1, Ordering::Relaxed);
    }
    S_OK
}

unsafe extern "system" fn factory_create_instance(
    this: *mut c_void,
    outer: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: out 可写。
    unsafe { *out = std::ptr::null_mut() };
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    guard(|| {
        // SAFETY: this 是上面两个静态的工厂之一。
        let object = unsafe { ((*this.cast::<Factory>()).create)() };
        // SAFETY: 刚建的对象，开头是虚函数表的指针（前三项是 IUnknown）；要到接口后放开自己的那一次引用
        // （接口不支持时对象随之销毁）。
        unsafe {
            let vtbl = &**object.cast::<*const UnknownVtbl>();
            let result = (vtbl.query_interface)(object, iid, out);
            (vtbl.release)(object);
            result
        }
    })
}

/// 系统要某个类的工厂。
///
/// # Safety
/// 按 COM 的约定调用：`clsid`、`iid` 指向有效的 GUID，`out` 可写。
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const Guid, iid: *const Guid, out: *mut *mut c_void) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: out 可写。
    unsafe { *out = std::ptr::null_mut() };
    if clsid.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: 调用方保证指向有效的 GUID。
    let clsid = unsafe { *clsid };
    if clsid != CLSID_THUMBNAIL {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory = std::ptr::from_ref(&THUMBNAIL_FACTORY).cast_mut().cast::<c_void>();
    // SAFETY: 工厂是静态的；iid、out 由调用方保证。
    unsafe { factory_query_interface(factory, iid, out) }
}

/// 系统问现在能不能卸载这个 DLL：一直说不能。核心里有常驻的后台线程（解码的线程池），卸载后它们会跑进
/// 已经不在的代码里；承载它的进程（dllhost）空闲一会儿自己就退出了。
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "system" fn DllCanUnloadNow() -> Hresult {
    S_FALSE
}

/// 还活着的对象个数（测试用）。
#[unsafe(no_mangle)]
pub extern "C" fn anyview_shell_live_objects() -> u32 {
    LIVE_OBJECTS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------------------------
// 往注册表里登记、撤销。写什么由核心的 `winshell::registry_plan` 算出，这里照着写，并记下写过什么：
// 整个归我们的项撤销时连里面的东西一起删；写在和别的程序共用的项里的值，登记前先记下原来的值，撤销时恢复
// （原来没有就删掉，因此空出来的项也删掉）。记录放在 `Software\Anyview\Journal` 里。全部在 HKEY_CURRENT_USER 下。

use core::winshell::{Install, RegData, RegPlan};

const HKEY_CLASSES_ROOT: isize = 0x8000_0000_u32 as i32 as isize;
const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
const KEY_READ: u32 = 0x2_0019;
const KEY_ALL_ACCESS: u32 = 0xF_003F;
const REG_SZ: u32 = 1;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: isize,
        sub_key: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        security: *const c_void,
        result: *mut isize,
        disposition: *mut u32,
    ) -> i32;
    fn RegOpenKeyExW(key: isize, sub_key: *const u16, options: u32, access: u32, result: *mut isize) -> i32;
    fn RegCloseKey(key: isize) -> i32;
    fn RegSetValueExW(key: isize, name: *const u16, reserved: u32, kind: u32, data: *const u8, size: u32) -> i32;
    fn RegQueryValueExW(
        key: isize,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
    fn RegDeleteValueW(key: isize, name: *const u16) -> i32;
    fn RegDeleteTreeW(key: isize, sub_key: *const u16) -> i32;
    fn RegDeleteKeyW(key: isize, sub_key: *const u16) -> i32;
    fn RegQueryInfoKeyW(
        key: isize,
        class: *mut u16,
        class_size: *mut u32,
        reserved: *mut u32,
        sub_keys: *mut u32,
        max_sub_key: *mut u32,
        max_class: *mut u32,
        values: *mut u32,
        max_value_name: *mut u32,
        max_value: *mut u32,
        security: *mut u32,
        last_write: *mut u64,
    ) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHChangeNotify(event: i32, flags: u32, item1: *const c_void, item2: *const c_void);
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 一个值原样的内容：类型和字节。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Raw {
    kind: u32,
    bytes: Vec<u8>,
}

impl Raw {
    fn from_data(data: &RegData) -> Self {
        match data {
            RegData::Text(text) => Raw {
                kind: REG_SZ,
                bytes: wide(text).iter().flat_map(|unit| unit.to_le_bytes()).collect(),
            },
            RegData::Number(number) => Raw {
                kind: REG_DWORD,
                bytes: number.to_le_bytes().to_vec(),
            },
        }
    }

    /// 字符串值的内容（不含结尾的 0）；不是字符串时为空。
    fn text(&self) -> Option<String> {
        if self.kind != REG_SZ && self.kind != 2 {
            return None;
        }
        let units: Vec<u16> = self
            .bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|unit| *unit != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    /// 存进记录里的写法：前 4 个字节是类型，后面是内容。
    fn packed(&self) -> Vec<u8> {
        let mut bytes = self.kind.to_le_bytes().to_vec();
        bytes.extend_from_slice(&self.bytes);
        bytes
    }

    fn unpack(raw: &Raw) -> Option<Raw> {
        let (kind, bytes) = raw.bytes.split_first_chunk::<4>()?;
        Some(Raw {
            kind: u32::from_le_bytes(*kind),
            bytes: bytes.to_vec(),
        })
    }
}

/// 打开着的一个注册表项，丢掉时关上。
struct Key(isize);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: 打开成功才有这个值。
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    fn open(root: isize, path: &str, access: u32) -> Option<Key> {
        let mut key = 0;
        // SAFETY: 路径以 0 结尾，key 可写。
        (unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, access, &mut key) } == 0).then_some(Key(key))
    }

    /// 打开当前用户下的一个项，没有就建（连同中间的各级）。
    fn create(path: &str) -> Result<Key, String> {
        let mut key = 0;
        // SAFETY: 路径以 0 结尾，key 可写。
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(path).as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_ALL_ACCESS,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if code == 0 { Ok(Key(key)) } else { Err(format!("建不了注册表项 {path}（错误 {code}）")) }
    }

    fn get(&self, name: &str) -> Option<Raw> {
        let name = wide(name);
        let (mut kind, mut size) = (0, 0);
        // SAFETY: 先问大小；名字以 0 结尾。
        if unsafe { RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut size) } != 0 {
            return None;
        }
        let mut bytes = vec![0_u8; size as usize];
        // SAFETY: 缓冲区的大小如实给出。
        if unsafe { RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null_mut(), &mut kind, bytes.as_mut_ptr(), &mut size) } != 0 {
            return None;
        }
        bytes.truncate(size as usize);
        Some(Raw { kind, bytes })
    }

    fn set(&self, name: &str, value: &Raw) -> Result<(), String> {
        // SAFETY: 名字以 0 结尾，内容的长度如实给出。
        let code = unsafe {
            RegSetValueExW(self.0, wide(name).as_ptr(), 0, value.kind, value.bytes.as_ptr(), value.bytes.len() as u32)
        };
        if code == 0 { Ok(()) } else { Err(format!("写不了注册表的值 {name}（错误 {code}）")) }
    }

    fn set_text(&self, name: &str, text: &str) -> Result<(), String> {
        self.set(name, &Raw::from_data(&RegData::Text(text.to_string())))
    }

    fn delete_value(&self, name: &str) {
        // SAFETY: 名字以 0 结尾。
        unsafe { RegDeleteValueW(self.0, wide(name).as_ptr()) };
    }

    /// 里面有没有东西（子项或值）。问不出来时当作有。
    fn is_empty(&self) -> bool {
        let (mut sub_keys, mut values) = (1, 1);
        // SAFETY: 只要子项数和值的个数，别的都不要。
        let code = unsafe {
            RegQueryInfoKeyW(
                self.0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sub_keys,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut values,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        code == 0 && sub_keys == 0 && values == 0
    }
}

/// 删掉当前用户下的一个项，连里面的东西。
fn delete_tree(path: &str) {
    let path = wide(path);
    // SAFETY: 路径以 0 结尾。先删里面的，再删它自己（不同版本的系统对“连自己一起删”说法不一，两步都做）。
    unsafe {
        RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr());
        RegDeleteKeyW(HKEY_CURRENT_USER, path.as_ptr());
    }
}

/// 撤掉我们的东西以后，把因此空出来的项删掉：只在某个扩展名自己的那一小块里往上删
/// （`Software\Classes\.ext\…`、`Software\Classes\SystemFileAssociations\.ext\…`），别处的项不动。
fn prune_empty(path: &str) {
    let parts: Vec<&str> = path.split('\\').collect();
    let is_extension = |index: usize| parts.get(index).is_some_and(|part| part.starts_with('.'));
    let classes = parts.len() > 2 && parts[0].eq_ignore_ascii_case("Software") && parts[1].eq_ignore_ascii_case("Classes");
    let floor = if classes && is_extension(2) {
        2
    } else if classes && parts[2].eq_ignore_ascii_case("SystemFileAssociations") && is_extension(3) {
        3
    } else {
        return;
    };
    for depth in (floor + 1..=parts.len()).rev() {
        let key = parts[..depth].join("\\");
        let empty = match Key::open(HKEY_CURRENT_USER, &key, KEY_READ) {
            Some(opened) => opened.is_empty(),
            // 已经不在了：接着看上一级。
            None => true,
        };
        if !empty {
            break;
        }
        // SAFETY: 路径以 0 结尾；只删空的项（有子项时系统也不让删）。
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, wide(&key).as_ptr()) };
    }
}

fn journal_key() -> String {
    format!(r"{}\Journal", core::winshell::APP_KEY)
}

/// 记录里的一条：写在共用的项里的一个值，写了什么，原来是什么。
struct Shared {
    key: String,
    name: String,
    written: Raw,
    backup: Option<Raw>,
}

/// 读出上次登记时的记录：整个归我们的项，写在共用的项里的值。
fn read_journal() -> (Vec<String>, Vec<Shared>) {
    let Some(journal) = Key::open(HKEY_CURRENT_USER, &journal_key(), KEY_READ) else {
        return (Vec::new(), Vec::new());
    };
    let owned = journal
        .get("Owned")
        .and_then(|raw| raw.text())
        .map(|text| text.lines().filter(|line| !line.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    let mut shared = Vec::new();
    for index in 0.. {
        let Some(entry) = Key::open(HKEY_CURRENT_USER, &format!(r"{}\Shared\{index}", journal_key()), KEY_READ) else {
            break;
        };
        let (Some(key), Some(name), Some(written)) = (
            entry.get("Key").and_then(|raw| raw.text()),
            entry.get("Name").and_then(|raw| raw.text()),
            entry.get("Written").as_ref().and_then(Raw::unpack),
        ) else {
            continue;
        };
        shared.push(Shared {
            key,
            name,
            written,
            backup: entry.get("Backup").as_ref().and_then(Raw::unpack),
        });
    }
    (owned, shared)
}

/// 撤销上次登记的全部内容（没有登记过时什么也不做）。
fn remove_registered() {
    let (owned, shared) = read_journal();
    for entry in &shared {
        if let Some(key) = Key::open(HKEY_CURRENT_USER, &entry.key, KEY_ALL_ACCESS) {
            // 还是我们写的那个值才动；别的程序（或用户）后来改过的，留给它。
            if key.get(&entry.name).as_ref() == Some(&entry.written) {
                match &entry.backup {
                    Some(previous) => {
                        let _ = key.set(&entry.name, previous);
                    }
                    None => key.delete_value(&entry.name),
                }
            }
        }
        prune_empty(&entry.key);
    }
    // 归我们的项。Anyview 自己的那一项里有这份记录，放到最后删。
    let app = core::winshell::APP_KEY;
    for key in owned.iter().filter(|key| !key.eq_ignore_ascii_case(app)) {
        delete_tree(key);
        if let Some((parent, _)) = key.rsplit_once('\\') {
            prune_empty(parent);
        }
    }
    delete_tree(app);
}

const HKEY_LOCAL_MACHINE: isize = 0x8000_0002_u32 as i32 as isize;

/// 系统里这个值是不是还空着：当前用户的、整台电脑的都没写（或者是空串）。只用于 `Software\Classes\…` 下的项。
/// 系统合并好的那一份（HKEY_CLASSES_ROOT）和两边各自的都看：Wine 里合并的那一份只有整台电脑的。
fn unset_in_classes(key: &str, name: &str) -> bool {
    const PREFIX: &str = r"Software\Classes\";
    let Some(relative) = key
        .get(..PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
        .map(|_| &key[PREFIX.len()..])
    else {
        return false;
    };
    let unset = |root: isize, path: &str| match Key::open(root, path, KEY_READ).and_then(|opened| opened.get(name)) {
        None => true,
        Some(raw) => raw.text().is_some_and(|text| text.is_empty()),
    };
    unset(HKEY_CLASSES_ROOT, relative) && unset(HKEY_CURRENT_USER, key) && unset(HKEY_LOCAL_MACHINE, key)
}

/// 按清单登记：先撤掉上次登记的，再照新的清单写。
fn apply_plan(plan: &RegPlan) -> Result<(), String> {
    remove_registered();
    // 先把记录的开头写好（归我们的项），之后每动一个共用的值，都先记下再写：中途被打断，下次也撤得干净。
    let journal = Key::create(&journal_key())?;
    journal.set_text("Owned", &plan.owned_keys.join("\n"))?;
    let mut index = 0;
    for value in &plan.values {
        let written = Raw::from_data(&value.data);
        if plan.is_owned(value) {
            Key::create(&value.key)?.set(&value.name, &written)?;
            continue;
        }
        if value.only_if_unset && !unset_in_classes(&value.key, &value.name) {
            continue;
        }
        let key = Key::create(&value.key)?;
        let entry = Key::create(&format!(r"{}\Shared\{index}", journal_key()))?;
        index += 1;
        entry.set_text("Key", &value.key)?;
        entry.set_text("Name", &value.name)?;
        entry.set("Written", &Raw { kind: REG_BINARY, bytes: written.packed() })?;
        if let Some(previous) = key.get(&value.name) {
            entry.set("Backup", &Raw { kind: REG_BINARY, bytes: previous.packed() })?;
        }
        key.set(&value.name, &written)?;
    }
    Ok(())
}

/// 告诉资源管理器文件类型的登记变了（图标、“打开方式”、缩略图重新看一遍）。
fn notify_shell() {
    // SAFETY: SHCNE_ASSOCCHANGED 不带参数。
    unsafe { SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null()) };
}

/// 登记时记下的安装情况；没有登记过时为空。
pub fn installed() -> Option<Install> {
    let app = Key::open(HKEY_CURRENT_USER, core::winshell::APP_KEY, KEY_READ)?;
    let dir = app.get("InstallDir")?.text().filter(|dir| !dir.is_empty())?;
    let size_kb = app
        .get("SizeKb")
        .filter(|raw| raw.kind == REG_DWORD)
        .and_then(|raw| raw.bytes.first_chunk::<4>().map(|bytes| u32::from_le_bytes(*bytes)))
        .unwrap_or(0);
    Some(Install {
        dir,
        version: app.get("Version").and_then(|raw| raw.text()).unwrap_or_default(),
        size_kb,
    })
}

/// 把 Anyview 登记到系统（安装时、设置变了以后）。
pub fn register(install: &Install, settings: &core::settings::Settings, system: core::Lang) -> Result<(), String> {
    let plan = core::winshell::registry_plan(install, settings, system);
    let result = apply_plan(&plan);
    notify_shell();
    result
}

/// 从系统里撤掉 Anyview 的登记（卸载时）。`stop_hosts`：顺带结束正加载着这个 DLL 的缩略图进程，好让文件删得掉。
pub fn unregister(stop_hosts: bool) {
    remove_registered();
    notify_shell();
    if stop_hosts {
        stop_thumbnail_hosts();
    }
}

// ---- 结束加载着我们的 DLL 的缩略图进程 ----

#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    process_id: u32,
    default_heap: usize,
    module_id: u32,
    threads: u32,
    parent_process_id: u32,
    priority: i32,
    flags: u32,
    exe_file: [u16; 260],
}

#[repr(C)]
struct ModuleEntry {
    size: u32,
    module_id: u32,
    process_id: u32,
    global_usage: u32,
    process_usage: u32,
    base_address: *mut u8,
    base_size: u32,
    module: isize,
    module_name: [u16; 256],
    exe_path: [u16; 260],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
    fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry) -> i32;
    fn Module32FirstW(snapshot: isize, entry: *mut ModuleEntry) -> i32;
    fn Module32NextW(snapshot: isize, entry: *mut ModuleEntry) -> i32;
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> isize;
    fn TerminateProcess(process: isize, code: u32) -> i32;
    fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32;
    fn CloseHandle(handle: isize) -> i32;
}

fn wide_array_to_string(text: &[u16]) -> String {
    let len = text.iter().position(|unit| *unit == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..len])
}

/// 结束正加载着我们这个 DLL 的 dllhost.exe（资源管理器做缩略图用的单独进程，结束了它需要时会再开）。
/// 升级、卸载时文件被它占着换不掉、删不掉。只动 dllhost，别的进程不碰。返回结束了几个。
pub fn stop_thumbnail_hosts() -> u32 {
    let Some(dll) = core::heif::module_dir().map(|dir| dir.join("anyview_ffi.dll").display().to_string()) else {
        return 0;
    };
    const INVALID: isize = -1;
    let mut stopped = 0;
    // SAFETY: 按文档调用工具帮助库：快照用完关掉，结构的大小如实给出。
    unsafe {
        let processes = CreateToolhelp32Snapshot(0x2, 0);
        if processes == INVALID {
            return 0;
        }
        let mut process: ProcessEntry = std::mem::zeroed();
        process.size = std::mem::size_of::<ProcessEntry>() as u32;
        let mut more = Process32FirstW(processes, &mut process) != 0;
        while more {
            let name = wide_array_to_string(&process.exe_file);
            if name.eq_ignore_ascii_case("dllhost.exe") && process.process_id != std::process::id() {
                // 这个进程加载的模块里有没有我们的 DLL（按完整路径比）。
                let modules = CreateToolhelp32Snapshot(0x8 | 0x10, process.process_id);
                let mut ours = false;
                if modules != INVALID {
                    let mut module: ModuleEntry = std::mem::zeroed();
                    module.size = std::mem::size_of::<ModuleEntry>() as u32;
                    let mut next = Module32FirstW(modules, &mut module) != 0;
                    while next && !ours {
                        ours = wide_array_to_string(&module.exe_path).eq_ignore_ascii_case(&dll);
                        next = Module32NextW(modules, &mut module) != 0;
                    }
                    CloseHandle(modules);
                }
                if ours {
                    // PROCESS_TERMINATE | SYNCHRONIZE
                    let handle = OpenProcess(0x1 | 0x10_0000, 0, process.process_id);
                    if handle != 0 {
                        if TerminateProcess(handle, 0) != 0 {
                            WaitForSingleObject(handle, 3000);
                            stopped += 1;
                        }
                        CloseHandle(handle);
                    }
                }
            }
            more = Process32NextW(processes, &mut process) != 0;
        }
        CloseHandle(processes);
    }
    stopped
}
