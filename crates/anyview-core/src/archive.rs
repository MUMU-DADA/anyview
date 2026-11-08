//! 压缩包：ZIP/CBZ、7Z/CB7、RAR/CBR 里的图片当成一个文件夹浏览。
//!
//! 包里的图片用"虚拟路径"表示：压缩包文件的路径后面接上包内的路径，如 `/x/漫画.zip/第1话/001.jpg`。
//! 核心的其余部分（浏览集合、缓存、事件）照常按路径工作；要读文件时由这里拆开虚拟路径，
//! 把那一项解到临时文件夹，交给解码器的是这个临时文件（各种解码器、动图、快速预览都不用改）。
//!
//! ZIP 可以按名字直接读，用到哪一项解哪一项。7Z、RAR 常是固实压缩（只能从头依次解），打开时在后台
//! 依次把图片解到临时文件夹，读的时候等它解到。压缩包不再用到时（浏览集合换了）删掉临时文件夹。

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::formats;

/// 压缩包的种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zip,
    SevenZ,
    Rar,
}

/// 能浏览的压缩包的扩展名（小写、不带点），供打开面板筛选。
pub const EXTENSIONS: &[&str] = &["zip", "cbz", "7z", "cb7", "rar", "cbr"];

/// 按扩展名认压缩包：zip、cbz、7z、cb7、rar、cbr。
pub fn kind(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "zip" | "cbz" => Some(Kind::Zip),
        "7z" | "cb7" => Some(Kind::SevenZ),
        "rar" | "cbr" => Some(Kind::Rar),
        _ => None,
    }
}

/// 是不是压缩包的文件名（按扩展名）。
pub fn is_archive_name(name: &str) -> bool {
    kind(Path::new(name)).is_some()
}

/// 包里的一张图片。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// 包内路径，用 "/" 分隔，如 "第1话/001.jpg"。
    pub name: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug)]
pub enum ArchiveError {
    Io(io::Error),
    /// 加了密码（不支持带密码的压缩包）。
    Password,
    /// 读不了（不是这种压缩包、损坏、不支持的压缩方法）。
    Corrupt(String),
    /// 包里没有这一项。
    NotFound,
    /// 不是认得的压缩包。
    NotArchive,
    /// 7Z、RAR 的后台解压没能开始。
    StartFailed,
    /// 等后台解压等得太久。
    TimedOut,
}

impl ArchiveError {
    /// 显示给用户的原因：自己的话按界面语言，系统、库给的说明原样放进去。
    pub fn message(&self) -> crate::i18n::Message {
        use crate::i18n::Message;
        match self {
            ArchiveError::Io(e) => Message::raw(e.to_string()),
            ArchiveError::Password => msg!("压缩包有密码", "password protected"),
            ArchiveError::Corrupt(m) => Message::raw(m.clone()),
            ArchiveError::NotFound => msg!("压缩包里没有这一项", "not found in the archive"),
            ArchiveError::NotArchive => msg!("不是压缩包", "not an archive"),
            ArchiveError::StartFailed => msg!("没能开始解压", "cannot start extracting"),
            ArchiveError::TimedOut => msg!("解压超时", "timed out"),
        }
    }
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message().text(crate::i18n::Lang::En))
    }
}

impl std::error::Error for ArchiveError {}

impl ArchiveError {
    /// 是被解压的上限拦下的，不是压缩包坏了。
    pub fn is_too_large(&self) -> bool {
        matches!(self, ArchiveError::Io(e) if is_too_large(e))
    }
}

const TOO_LARGE: &str = "解出来太大";

fn is_too_large(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::InvalidData && e.get_ref().is_some_and(|inner| inner.to_string() == TOO_LARGE)
}

/// 用户对这个压缩包选了"仍然打开"：它开着的话解压不再设上限，后台解压因为上限停下、跳过的接着解。
pub(crate) fn lift_limits(path: &Path) {
    let open = OPEN
        .get()
        .and_then(|registry| lock(registry).get(path).and_then(Weak::upgrade));
    if let Some(archive) = open {
        archive.store.unlimited.store(true, Ordering::Relaxed);
        archive.resume();
    }
}

impl From<io::Error> for ArchiveError {
    fn from(e: io::Error) -> Self {
        ArchiveError::Io(e)
    }
}

/// 拆开虚拟路径：压缩包文件和包内路径（"/" 分隔）。不是压缩包里的图片时为空。
/// 从根往下找第一个扩展名是压缩包、而且是文件（不是文件夹）的上级。
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    let parts: Vec<Component> = path.components().collect();
    let mut prefix = PathBuf::new();
    for (i, part) in parts.iter().enumerate() {
        prefix.push(part);
        if i + 1 == parts.len() {
            break;
        }
        if kind(&prefix).is_some() && prefix.is_file() {
            let inner: Vec<String> = parts[i + 1..]
                .iter()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            return Some((prefix, inner.join("/")));
        }
    }
    None
}

/// 压缩包里一项的虚拟路径。
pub fn entry_path(archive: &Path, name: &str) -> PathBuf {
    let mut path = archive.to_path_buf();
    for part in name.split('/').filter(|p| !p.is_empty()) {
        path.push(part);
    }
    path
}

/// 包里的这一项算不算图片：受支持的图片格式；跳过隐藏文件、macOS 打包时附带的 `__MACOSX` 等。
fn wanted(name: &str) -> bool {
    let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
    let Some(last) = parts.last() else {
        return false;
    };
    if parts.iter().any(|p| p.starts_with('.') || *p == "__MACOSX") {
        return false;
    }
    formats::is_supported_name(last)
}

/// 打开着的压缩包（同一进程里共用；没有窗口用到时自动关掉、删掉临时文件）。
static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Archive>>>> = OnceLock::new();
static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 临时文件夹的上级：系统临时目录里的 `anyview-archives`。
fn temp_root() -> PathBuf {
    std::env::temp_dir().join("anyview-archives")
}

/// 打开一个压缩包（已经打开着就用那个）：列出里面的图片；7Z、RAR 同时开始在后台依次解出。
pub fn open(path: &Path) -> Result<Arc<Archive>, ArchiveError> {
    let registry = OPEN.get_or_init(|| {
        cleanup_stale();
        Mutex::new(HashMap::new())
    });
    if let Some(open) = lock(registry).get(path).and_then(Weak::upgrade) {
        return Ok(open);
    }
    let kind = kind(path).ok_or(ArchiveError::NotArchive)?;
    let items = match kind {
        Kind::Zip => list_zip(path)?,
        Kind::SevenZ => list_7z(path)?,
        Kind::Rar => list_rar(path)?,
    };
    let dir = temp_root().join(format!(
        "{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let index: HashMap<String, usize> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.name.clone(), i))
        .collect();
    let store = Arc::new(Store::default());
    lock(&store.state).background = matches!(kind, Kind::SevenZ | Kind::Rar);
    // 用户对这个压缩包选过"仍然打开"：不设上限。
    store
        .unlimited
        .store(crate::decode::is_trusted(path), Ordering::Relaxed);
    let zip = match kind {
        Kind::Zip => {
            let file = File::open(path)?;
            let zip = zip::ZipArchive::new(BufReader::new(file))
                .map_err(|e| ArchiveError::Corrupt(e.to_string()))?;
            Some(Mutex::new(zip))
        }
        _ => None,
    };
    let archive = Arc::new(Archive {
        path: path.to_path_buf(),
        kind,
        items,
        index,
        dir: dir.clone(),
        store: Arc::clone(&store),
        zip,
    });
    // 7Z、RAR：在后台依次解出（固实压缩只能这样读）。
    if matches!(kind, Kind::SevenZ | Kind::Rar) {
        archive.start_extraction()?;
    }
    lock(registry).insert(path.to_path_buf(), Arc::downgrade(&archive));
    Ok(archive)
}

/// 虚拟路径对应的、可以交给解码器的文件：压缩包里的图片解到临时文件夹后的路径；普通文件原样返回。
/// 解不出时为错误。`size`、`modified` 是包里记的大小和时间。
pub fn materialize(path: &Path) -> Result<Materialized, ArchiveError> {
    let Some((archive, name)) = split(path) else {
        return Ok(Materialized {
            file: path.to_path_buf(),
            entry: None,
            _archive: None,
        });
    };
    let open = open(&archive)?;
    let file = open.file(&name)?;
    let item = open.items[open.index[&name]].clone();
    Ok(Materialized {
        file,
        entry: Some(item),
        _archive: Some(open),
    })
}

/// [`materialize`] 的结果。
pub struct Materialized {
    /// 真正的文件（压缩包里的图片是解出的临时文件）。
    pub file: PathBuf,
    /// 压缩包里的图片：包里记的名字、大小和时间。
    pub entry: Option<Item>,
    /// 用着的时候压缩包不关（关了临时文件就删了）。
    _archive: Option<Arc<Archive>>,
}

/// 一个打开着的压缩包。
pub struct Archive {
    pub path: PathBuf,
    pub kind: Kind,
    /// 包里的图片（按包里的顺序）。
    pub items: Vec<Item>,
    index: HashMap<String, usize>,
    /// 解出的临时文件放在这里。
    dir: PathBuf,
    store: Arc<Store>,
    zip: Option<Mutex<zip::ZipArchive<BufReader<File>>>>,
}

impl fmt::Debug for Archive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Archive")
            .field("path", &self.path)
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

/// 解出的临时文件：`<临时文件夹>/<序号>/<原来的文件名>`（保留文件名，复制到剪贴板、交给编辑器时名字不变）。
fn item_file(dir: &Path, index: usize, name: &str) -> PathBuf {
    let base = name.rsplit('/').next().unwrap_or(name);
    dir.join(index.to_string())
        .join(temp_name(base, cfg!(windows)).as_ref())
}

/// 临时文件的名字。Windows 的文件名里不能有 `< > : " \ | ? *` 和控制字符，换成 "_"：不换的话 "C:x.png"
/// 接到路径后面会跑出临时文件夹，"a:b.png" 写进的是文件 a 的另一个数据流。别的系统上这些是普通字符，名字原样。
fn temp_name(base: &str, windows: bool) -> std::borrow::Cow<'_, str> {
    let bad = |c: char| matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*') || c.is_control();
    if windows && base.contains(bad) {
        base.chars().map(|c| if bad(c) { '_' } else { c }).collect::<String>().into()
    } else {
        base.into()
    }
}

/// 包里原来的名字（旧的 Windows 工具打的包用反斜杠）与列表里的名字（都换成了 "/"）是同一项。
fn same_entry(raw: &str, listed: &str) -> bool {
    raw.len() == listed.len()
        && raw
            .bytes()
            .zip(listed.bytes())
            .all(|(r, l)| r == l || (r == b'\\' && l == b'/'))
}

impl Archive {
    /// 在后台线程里把还没解出的图片依次解出来（7Z、RAR）。解的过程中用户选了"仍然打开"（[`lift_limits`]）、
    /// 而前面有图片因为上限跳过了，就再走一遍。
    fn start_extraction(&self) -> Result<(), ArchiveError> {
        let (path, kind, dir) = (self.path.clone(), self.kind, self.dir.clone());
        let index = self.index.clone();
        let store = Arc::clone(&self.store);
        thread::Builder::new()
            .name("anyview-archive".into())
            .spawn(move || {
                loop {
                    let wanted: HashMap<String, usize> = {
                        let state = lock(&store.state);
                        index
                            .iter()
                            .filter(|(name, _)| !state.files.contains_key(*name))
                            .map(|(name, i)| (name.clone(), *i))
                            .collect()
                    };
                    let result = match kind {
                        Kind::SevenZ => extract_7z(&path, &wanted, &dir, &store),
                        _ => extract_rar(&path, &wanted, &dir, &store),
                    };
                    let error = result.err().map(|e| match e {
                        ArchiveError::Password => "password".to_string(),
                        other => other.to_string(),
                    });
                    if !store.finish_or_rerun(error, &dir) {
                        break;
                    }
                }
            })
            .map(|_| ())
            .map_err(|_| ArchiveError::StartFailed)
    }

    /// 上限解除了（[`lift_limits`]）：后台解压里因为上限没解出来的图片接着解。
    fn resume(&self) {
        {
            let mut state = lock(&self.store.state);
            if !state.background || state.cancelled || !state.limited {
                return;
            }
            if !state.finished {
                // 还在解：这一遍完了再走一遍。
                state.rerun = true;
                return;
            }
            state.finished = false;
            state.error = None;
            state.limited = false;
        }
        if self.start_extraction().is_err() {
            self.store
                .finish(Some("cannot start extracting".into()), &self.dir);
        }
    }

    /// 包里一项解出的临时文件。ZIP 现解；7Z、RAR 等后台解到（最多等 2 分钟）。
    pub fn file(&self, name: &str) -> Result<PathBuf, ArchiveError> {
        let index = *self.index.get(name).ok_or(ArchiveError::NotFound)?;
        if let Some(zip) = &self.zip {
            if let Some(path) = lock(&self.store.state).files.get(name) {
                return Ok(path.clone());
            }
            let target = item_file(&self.dir, index, name);
            {
                let mut zip = lock(zip);
                // 等锁期间别的线程可能已经解出来了。
                if let Some(path) = lock(&self.store.state).files.get(name) {
                    return Ok(path.clone());
                }
                // 列表里的名字把反斜杠换成了 "/"（旧的 Windows 工具打的包）：包里按原来的名字找。
                let raw = match zip.index_for_name(name) {
                    Some(_) => None,
                    None => zip
                        .file_names()
                        .find(|raw| same_entry(raw, name))
                        .map(str::to_string),
                };
                let mut entry = zip.by_name(raw.as_deref().unwrap_or(name)).map_err(zip_error)?;
                let limit = self.store.allowance();
                if entry.size() > limit {
                    return Err(too_large().into());
                }
                let written = write_file(&target, &mut entry, limit)?;
                self.store.spent(written);
            }
            lock(&self.store.state)
                .files
                .insert(name.to_string(), target.clone());
            return Ok(target);
        }
        self.store.wait(name)
    }

    /// 包里这一项的信息。
    pub fn item(&self, name: &str) -> Option<&Item> {
        self.index.get(name).map(|&i| &self.items[i])
    }
}

impl Drop for Archive {
    fn drop(&mut self) {
        if let Some(registry) = OPEN.get() {
            let mut open = lock(registry);
            if open.get(&self.path).is_some_and(|w| w.strong_count() == 0) {
                open.remove(&self.path);
            }
        }
        self.store.cancel(&self.dir);
    }
}

/// 解出的文件和后台解压的状态。
#[derive(Default)]
struct Store {
    state: Mutex<StoreState>,
    ready: Condvar,
    /// 用户选了"仍然打开"：不设上限。
    unlimited: std::sync::atomic::AtomicBool,
}

/// 单个压缩包条目解压上限（2 GB）：防止极小条目解压出数十 GB（“压缩包炸弹”），
/// 不设上限会把临时目录（Linux 上常在内存里）写满。比这大的图片本来也解码不了。
const MAX_ENTRY_BYTES: u64 = 2 << 30;
/// 单个压缩包累计解压总量上限（16 GB）；超过后剩余图片不再读取。
const MAX_TOTAL_BYTES: u64 = 16 << 30;

fn too_large() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, TOO_LARGE)
}

#[derive(Default)]
struct StoreState {
    /// 已经解出了多少字节。
    written: u64,
    /// 后台解压时有图片因为上限没解出来。
    limited: bool,
    /// 后台解压这一遍完了再走一遍（上限解除时它还在解）。
    rerun: bool,
    files: HashMap<String, PathBuf>,
    /// 后台解压结束了（解完、出错或取消）。
    finished: bool,
    error: Option<String>,
    /// 压缩包不用了：后台解压停下，临时文件夹删掉。
    cancelled: bool,
    /// 有后台解压（7Z、RAR）。
    background: bool,
}

impl Store {
    /// 下一项最多还能解多少字节（单项上限和整个压缩包剩下的额度里小的那个）。
    fn allowance(&self) -> u64 {
        if self.unlimited.load(Ordering::Relaxed) {
            return u64::MAX;
        }
        MAX_ENTRY_BYTES.min(MAX_TOTAL_BYTES.saturating_sub(lock(&self.state).written))
    }

    fn note_limited(&self, error: &io::Error) {
        if is_too_large(error) {
            lock(&self.state).limited = true;
        }
    }

    fn spent(&self, bytes: u64) {
        let mut state = lock(&self.state);
        state.written = state.written.saturating_add(bytes);
    }

    fn publish(&self, name: &str, file: PathBuf) {
        lock(&self.state).files.insert(name.to_string(), file);
        self.ready.notify_all();
    }

    fn cancelled(&self) -> bool {
        lock(&self.state).cancelled
    }

    /// 后台解压结束；已经不用了就删掉临时文件夹（与 [`Store::cancel`] 只有一方删）。
    fn finish(&self, error: Option<String>, dir: &Path) {
        self.finish_locked(lock(&self.state), error, dir);
    }

    fn finish_locked(&self, mut state: MutexGuard<'_, StoreState>, error: Option<String>, dir: &Path) {
        state.finished = true;
        if state.error.is_none() {
            state.error = error;
        }
        if state.cancelled {
            let _ = fs::remove_dir_all(dir);
        }
        drop(state);
        self.ready.notify_all();
    }

    /// 后台解压走完了一遍。期间上限解除了、要求再走一遍（[`Archive::resume`]）的话返回 true，由调用方再走；
    /// 否则结束（同 [`Store::finish`]），返回 false。"要不要再走"和"结束"在同一次加锁里定：分两次的话，
    /// 再走一遍的要求正好落在中间时就没有人理了（那几张图片再也解不出来）。
    fn finish_or_rerun(&self, error: Option<String>, dir: &Path) -> bool {
        let mut state = lock(&self.state);
        let again = std::mem::take(&mut state.rerun) && !state.cancelled;
        if again {
            state.limited = false;
            return true;
        }
        self.finish_locked(state, error, dir);
        false
    }

    /// 压缩包不用了：没有后台解压（ZIP）或它已经结束时马上删掉临时文件夹，否则由它结束时删。
    fn cancel(&self, dir: &Path) {
        let mut state = lock(&self.state);
        state.cancelled = true;
        if !state.background || state.finished {
            let _ = fs::remove_dir_all(dir);
        }
        drop(state);
        self.ready.notify_all();
    }

    fn wait(&self, name: &str) -> Result<PathBuf, ArchiveError> {
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let mut state = lock(&self.state);
        loop {
            if let Some(path) = state.files.get(name) {
                return Ok(path.clone());
            }
            if state.finished {
                return Err(match &state.error {
                    _ if state.limited => ArchiveError::Io(too_large()),
                    Some(e) if e == "password" => ArchiveError::Password,
                    Some(e) => ArchiveError::Corrupt(e.clone()),
                    None => ArchiveError::NotFound,
                });
            }
            if state.cancelled {
                return Err(ArchiveError::NotFound);
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err(ArchiveError::TimedOut);
            }
            state = self
                .ready
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }
}

/// 把一项解到临时文件，返回写了多少字节。超过 `limit` 字节的不要（删掉写了一半的），报错。
fn write_file(target: &Path, data: &mut dyn Read, limit: u64) -> io::Result<u64> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = target.with_extension("partial");
    let written = {
        let mut file = File::create(&partial)?;
        let written = io::copy(&mut data.take(limit.saturating_add(1)), &mut file)?;
        file.flush()?;
        written
    };
    if written > limit {
        let _ = fs::remove_file(&partial);
        return Err(too_large());
    }
    fs::rename(&partial, target)?;
    Ok(written)
}

fn zip_error(e: zip::result::ZipError) -> ArchiveError {
    let text = e.to_string();
    if text.to_ascii_lowercase().contains("password") {
        ArchiveError::Password
    } else {
        ArchiveError::Corrupt(text)
    }
}

fn list_zip(path: &Path) -> Result<Vec<Item>, ArchiveError> {
    let mut zip = zip::ZipArchive::new(BufReader::new(File::open(path)?))
        .map_err(|e| ArchiveError::Corrupt(e.to_string()))?;
    let mut items = Vec::new();
    let mut encrypted = 0;
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i).map_err(zip_error)?;
        if entry.is_dir() || !wanted(entry.name()) {
            continue;
        }
        encrypted += usize::from(entry.encrypted());
        let modified = entry.last_modified().and_then(|t| {
            local_time(
                i32::from(t.year()),
                t.month().into(),
                t.day().into(),
                t.hour().into(),
                t.minute().into(),
                t.second().into(),
            )
        });
        items.push(Item {
            name: entry.name().replace('\\', "/"),
            size: entry.size(),
            modified,
        });
    }
    // 图片全都加了密码：整个压缩包当作打不开（只有几张加密时，那几张读的时候出错）。
    if encrypted > 0 && encrypted == items.len() {
        return Err(ArchiveError::Password);
    }
    Ok(items)
}

fn list_7z(path: &Path) -> Result<Vec<Item>, ArchiveError> {
    let reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(sevenz_error)?;
    let items = reader
        .archive()
        .files
        .iter()
        .filter(|f| !f.is_directory && f.has_stream && wanted(&f.name))
        .map(|f| Item {
            name: f.name.replace('\\', "/"),
            size: f.size,
            modified: f
                .has_last_modified_date
                .then(|| nt_time(u64::from(f.last_modified_date)))
                .flatten(),
        })
        .collect();
    Ok(items)
}

fn sevenz_error(e: sevenz_rust2::Error) -> ArchiveError {
    match e {
        sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) => {
            ArchiveError::Password
        }
        other => ArchiveError::Corrupt(other.to_string()),
    }
}

/// Windows 文件时间（1601 年起的 100 纳秒数）→ 系统时间。
fn nt_time(value: u64) -> Option<SystemTime> {
    const EPOCH_DIFF_SECS: u64 = 11_644_473_600;
    let secs = (value / 10_000_000).checked_sub(EPOCH_DIFF_SECS)?;
    Some(UNIX_EPOCH + Duration::new(secs, ((value % 10_000_000) * 100) as u32))
}

fn local_time(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Option<SystemTime> {
    use chrono::TimeZone;
    chrono::Local
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .earliest()
        .map(SystemTime::from)
}

/// DOS 时间（RAR 用）→ 系统时间（本地时间）。
fn dos_time(value: u32) -> Option<SystemTime> {
    let date = value >> 16;
    let time = value & 0xFFFF;
    local_time(
        ((date >> 9) & 0x7F) as i32 + 1980,
        (date >> 5) & 0x0F,
        date & 0x1F,
        time >> 11,
        (time >> 5) & 0x3F,
        (time & 0x1F) * 2,
    )
}

fn rar_error(e: unrar::error::UnrarError) -> ArchiveError {
    use unrar::error::Code;
    match e.code {
        Code::MissingPassword | Code::BadPassword => ArchiveError::Password,
        _ => ArchiveError::Corrupt(e.to_string()),
    }
}

fn list_rar(path: &Path) -> Result<Vec<Item>, ArchiveError> {
    let listing = unrar::Archive::new(path)
        .open_for_listing()
        .map_err(rar_error)?;
    let mut items = Vec::new();
    for header in listing {
        let header = header.map_err(rar_error)?;
        let name = header.filename.to_string_lossy().replace('\\', "/");
        if !header.is_file() || !wanted(&name) {
            continue;
        }
        if header.is_encrypted() {
            return Err(ArchiveError::Password);
        }
        items.push(Item {
            name,
            size: header.unpacked_size,
            modified: dos_time(header.file_time),
        });
    }
    Ok(items)
}

fn extract_7z(
    path: &Path,
    wanted: &HashMap<String, usize>,
    dir: &Path,
    store: &Store,
) -> Result<(), ArchiveError> {
    let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(sevenz_error)?;
    reader
        .for_each_entries(|entry, data| {
            if store.cancelled() {
                return Ok(false);
            }
            let name = entry.name.replace('\\', "/");
            match wanted.get(&name) {
                Some(&index) => {
                    let target = item_file(dir, index, &name);
                    let written = write_file(&target, data, store.allowance())
                        .inspect_err(|e| store.note_limited(e))?;
                    store.spent(written);
                    store.publish(&name, target);
                }
                // 固实压缩：不要的项也要读过去。
                None => {
                    io::copy(data, &mut io::sink())?;
                }
            }
            Ok(true)
        })
        .map_err(sevenz_error)
}

fn extract_rar(
    path: &Path,
    wanted: &HashMap<String, usize>,
    dir: &Path,
    store: &Store,
) -> Result<(), ArchiveError> {
    let mut archive = unrar::Archive::new(path)
        .open_for_processing()
        .map_err(rar_error)?;
    while let Some(header) = archive.read_header().map_err(rar_error)? {
        if store.cancelled() {
            return Ok(());
        }
        let name = header.entry().filename.to_string_lossy().replace('\\', "/");
        archive = match wanted.get(&name) {
            // 文件头写的大小超过上限的不解（跳过，后面的照常）。直接解到文件，不先整个读进内存。
            Some(&index)
                if header.entry().is_file() && header.entry().unpacked_size <= store.allowance() =>
            {
                let target = item_file(dir, index, &name);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let partial = target.with_extension("partial");
                let rest = header.extract_to(&partial).map_err(rar_error)?;
                let written = fs::metadata(&partial)?.len();
                if written > store.allowance() {
                    let _ = fs::remove_file(&partial);
                    lock(&store.state).limited = true;
                    return Err(too_large().into());
                }
                fs::rename(&partial, &target)?;
                store.spent(written);
                store.publish(&name, target);
                rest
            }
            other => {
                if other.is_some() && header.entry().is_file() {
                    lock(&store.state).limited = true; // 要的图片，因为上限跳过了
                }
                header.skip().map_err(rar_error)?
            }
        };
    }
    Ok(())
}

/// 删掉以前留下的临时文件夹（程序没有正常退出时）：Linux 上进程已经不在的；其他平台超过一天的。
fn cleanup_stale() {
    let Ok(entries) = fs::read_dir(temp_root()) else {
        return;
    };
    let me = std::process::id().to_string();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let pid = name.split('-').next().unwrap_or("");
        if pid == me {
            continue;
        }
        let gone = if cfg!(target_os = "linux") {
            !Path::new("/proc").join(pid).exists()
        } else {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(24 * 3600))
        };
        if gone {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// 关掉所有压缩包（程序退出前）：停下后台解压、删掉临时文件夹。
pub fn shutdown() {
    let Some(registry) = OPEN.get() else {
        return;
    };
    let open: Vec<Arc<Archive>> = lock(registry).values().filter_map(Weak::upgrade).collect();
    for archive in open {
        archive.store.cancel(&archive.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_safe_on_windows_and_untouched_elsewhere() {
        // Windows：不能有的字符换成 "_"，名字还认得出，也不会跑出临时文件夹、写进别的数据流。
        assert_eq!(temp_name("C:evil.png", true), "C_evil.png");
        assert_eq!(temp_name("2024-01-01T12:30:45.png", true), "2024-01-01T12_30_45.png");
        assert_eq!(temp_name("a\\b?.png", true), "a_b_.png");
        assert_eq!(temp_name("第1话 001.jpg", true), "第1话 001.jpg");
        // 别的系统上冒号是普通字符：复制出去、交给编辑器时名字不变。
        assert_eq!(temp_name("2024-01-01T12:30:45.png", false), "2024-01-01T12:30:45.png");
        let dir = Path::new("tmp");
        assert_eq!(item_file(dir, 3, "ch1/001.png"), dir.join("3").join("001.png"));
    }

    /// 解出来超过上限的不要，也不留下写了一半的文件；整个压缩包的额度用完后不再解。
    #[test]
    fn oversized_entries_are_refused() {
        let dir = std::env::temp_dir().join(format!("anyview-archive-limit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let target = dir.join("0").join("a.png");
        let data = vec![7u8; 1000];
        assert_eq!(write_file(&target, &mut data.as_slice(), 1000).unwrap(), 1000);
        assert_eq!(fs::read(&target).unwrap(), data);
        let other = dir.join("1").join("b.png");
        let error = write_file(&other, &mut data.as_slice(), 999).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!other.exists() && !other.with_extension("partial").exists());
        // 读不完的数据（炸弹）也只写到上限为止。
        assert!(write_file(&other, &mut io::repeat(0), 4096).is_err());
        assert!(!other.exists() && !other.with_extension("partial").exists());
        let _ = fs::remove_dir_all(&dir);

        let store = Store::default();
        assert_eq!(store.allowance(), MAX_ENTRY_BYTES);
        store.spent(MAX_TOTAL_BYTES - 5);
        assert_eq!(store.allowance(), 5);
        store.spent(100);
        assert_eq!(store.allowance(), 0);
        // 上限拦下的错误认得出来（界面据此问用户，而不是报压缩包坏了）；普通的读写错误不是。
        assert!(ArchiveError::Io(too_large()).is_too_large());
        assert!(!ArchiveError::Io(io::Error::new(io::ErrorKind::InvalidData, "bad")).is_too_large());
        assert!(!ArchiveError::Corrupt("x".into()).is_too_large());
        // 后台解压因为上限没解出来的图片：等它的一方得到"太大"，不是"包里没有"。
        store.note_limited(&too_large());
        store.finish(None, Path::new("/nonexistent/anyview-test"));
        assert!(store.wait("a.png").unwrap_err().is_too_large());
        // 用户选了"仍然打开"：不再有上限。
        store.unlimited.store(true, Ordering::Relaxed);
        assert_eq!(store.allowance(), u64::MAX);
    }

    /// 后台解压走完一遍时：期间要求过再走一遍（上限解除了）就不结束、再走；没有才结束。
    #[test]
    fn a_pass_that_was_asked_to_run_again_does_not_finish() {
        let nowhere = Path::new("/nonexistent/anyview-test");
        let store = Store::default();
        {
            let mut state = lock(&store.state);
            state.limited = true;
            state.rerun = true;
        }
        assert!(store.finish_or_rerun(Some("解出来太大".into()), nowhere));
        {
            let state = lock(&store.state);
            assert!(!state.finished && !state.limited && !state.rerun && state.error.is_none());
        }
        assert!(!store.finish_or_rerun(None, nowhere));
        assert!(lock(&store.state).finished);
        // 压缩包已经不用了：不再走，结束。
        let store = Store::default();
        {
            let mut state = lock(&store.state);
            state.rerun = true;
            state.cancelled = true;
        }
        assert!(!store.finish_or_rerun(None, nowhere));
        assert!(lock(&store.state).finished);
    }

    #[test]
    fn listed_names_match_raw_names_with_backslashes() {
        assert!(same_entry("ch1\\001.png", "ch1/001.png"));
        assert!(same_entry("ch1/001.png", "ch1/001.png"));
        assert!(!same_entry("ch1/001.png", "ch1\\001.png"), "列表里的名字不会有反斜杠");
        assert!(!same_entry("ch1\\001.png", "ch1/002.png"));
        assert!(!same_entry("ch1\\001.png", "ch1/001.pn"));
    }
}

