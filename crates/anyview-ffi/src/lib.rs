//! Anyview 核心的跨语言接口。
//!
//! 这里只做类型转换，不放业务逻辑：业务规则都在 `anyview-core` 中实现一次。
//! Linux 界面直接依赖 `anyview-core`，不经过本 crate。

use std::sync::Arc;

use anyview_core as core;

/// 零复制像素访问与轻量 C 接口。
pub mod capi;
pub mod diag;
/// Windows 的系统集成：缩略图提供程序，往注册表里登记、撤销。
#[cfg(windows)]
pub mod shell;

uniffi::setup_scaffolding!();

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

impl From<Platform> for core::Platform {
    fn from(value: Platform) -> Self {
        match value {
            Platform::Macos => core::Platform::Macos,
            Platform::Windows => core::Platform::Windows,
            Platform::Linux => core::Platform::Linux,
        }
    }
}

/// 界面语言（核心的 `Lang`）。
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    ZhHans,
    En,
    ZhHant,
    Ja,
    Ko,
    Es,
    Fr,
    De,
    PtBr,
    Ru,
    It,
    Tr,
    Pl,
    Nl,
    Vi,
    Id,
    Th,
    Uk,
    Hi,
}

impl From<Lang> for core::Lang {
    fn from(value: Lang) -> Self {
        match value {
            Lang::ZhHans => core::Lang::ZhHans,
            Lang::En => core::Lang::En,
            Lang::ZhHant => core::Lang::ZhHant,
            Lang::Ja => core::Lang::Ja,
            Lang::Ko => core::Lang::Ko,
            Lang::Es => core::Lang::Es,
            Lang::Fr => core::Lang::Fr,
            Lang::De => core::Lang::De,
            Lang::PtBr => core::Lang::PtBr,
            Lang::Ru => core::Lang::Ru,
            Lang::It => core::Lang::It,
            Lang::Tr => core::Lang::Tr,
            Lang::Pl => core::Lang::Pl,
            Lang::Nl => core::Lang::Nl,
            Lang::Vi => core::Lang::Vi,
            Lang::Id => core::Lang::Id,
            Lang::Th => core::Lang::Th,
            Lang::Uk => core::Lang::Uk,
            Lang::Hi => core::Lang::Hi,
        }
    }
}

impl From<core::Lang> for Lang {
    fn from(value: core::Lang) -> Self {
        match value {
            core::Lang::ZhHans => Lang::ZhHans,
            core::Lang::En => Lang::En,
            core::Lang::ZhHant => Lang::ZhHant,
            core::Lang::Ja => Lang::Ja,
            core::Lang::Ko => Lang::Ko,
            core::Lang::Es => Lang::Es,
            core::Lang::Fr => Lang::Fr,
            core::Lang::De => Lang::De,
            core::Lang::PtBr => Lang::PtBr,
            core::Lang::Ru => Lang::Ru,
            core::Lang::It => Lang::It,
            core::Lang::Tr => Lang::Tr,
            core::Lang::Pl => Lang::Pl,
            core::Lang::Nl => Lang::Nl,
            core::Lang::Vi => Lang::Vi,
            core::Lang::Id => Lang::Id,
            core::Lang::Th => Lang::Th,
            core::Lang::Uk => Lang::Uk,
            core::Lang::Hi => Lang::Hi,
        }
    }
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Navigation {
    Next,
    Prev,
    First,
    Last,
    Forward10,
    Back10,
    /// 跳到指定序号（从 0 开始；进度条）。
    Jump {
        index: u32,
    },
}

impl From<Navigation> for core::Navigation {
    fn from(value: Navigation) -> Self {
        match value {
            Navigation::Next => core::Navigation::Next,
            Navigation::Prev => core::Navigation::Prev,
            Navigation::First => core::Navigation::First,
            Navigation::Last => core::Navigation::Last,
            Navigation::Forward10 => core::Navigation::Forward10,
            Navigation::Back10 => core::Navigation::Back10,
            Navigation::Jump { index } => core::Navigation::Jump(index as usize),
        }
    }
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Ping {
        token: u64,
    },
    Open {
        path: String,
    },
    Navigate {
        navigation: Navigation,
    },
    Close,
    /// 界面已把文件移到废纸篓或回收站。
    Removed {
        path: String,
    },
}

impl From<Command> for core::Command {
    fn from(value: Command) -> Self {
        match value {
            Command::Ping { token } => core::Command::Ping { token },
            Command::Open { path } => core::Command::Open { path },
            Command::Navigate { navigation } => core::Command::Navigate(navigation.into()),
            Command::Close => core::Command::Close,
            Command::Removed { path } => core::Command::Removed { path },
        }
    }
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    AtFirst,
    AtLast,
    EmptyFolder,
    LastFolder,
    FirstFolder,
    NoRandomBack,
    /// 压缩包加了密码。
    ArchivePassword,
    EmptyArchive,
    /// 复制、移动到图片文件夹完成。
    Copied { folder: String, name: String },
    Moved { folder: String, name: String },
    FolderNotSet,
    FileFailed { message: String },
}

impl From<core::NoticeKind> for NoticeKind {
    fn from(value: core::NoticeKind) -> Self {
        use core::NoticeKind as N;
        match value {
            N::AtFirst => NoticeKind::AtFirst,
            N::AtLast => NoticeKind::AtLast,
            N::EmptyFolder => NoticeKind::EmptyFolder,
            N::LastFolder => NoticeKind::LastFolder,
            N::FirstFolder => NoticeKind::FirstFolder,
            N::NoRandomBack => NoticeKind::NoRandomBack,
            N::ArchivePassword => NoticeKind::ArchivePassword,
            N::EmptyArchive => NoticeKind::EmptyArchive,
            N::Copied { folder, name } => NoticeKind::Copied { folder, name },
            N::Moved { folder, name } => NoticeKind::Moved { folder, name },
            N::FolderNotSet => NoticeKind::FolderNotSet,
            N::FileFailed { message } => NoticeKind::FileFailed { message },
        }
    }
}

impl From<NoticeKind> for core::NoticeKind {
    fn from(value: NoticeKind) -> Self {
        use core::NoticeKind as N;
        match value {
            NoticeKind::AtFirst => N::AtFirst,
            NoticeKind::AtLast => N::AtLast,
            NoticeKind::EmptyFolder => N::EmptyFolder,
            NoticeKind::LastFolder => N::LastFolder,
            NoticeKind::FirstFolder => N::FirstFolder,
            NoticeKind::NoRandomBack => N::NoRandomBack,
            NoticeKind::ArchivePassword => N::ArchivePassword,
            NoticeKind::EmptyArchive => N::EmptyArchive,
            NoticeKind::Copied { folder, name } => N::Copied { folder, name },
            NoticeKind::Moved { folder, name } => N::Moved { folder, name },
            NoticeKind::FolderNotSet => N::FolderNotSet,
            NoticeKind::FileFailed { message } => N::FileFailed { message },
        }
    }
}

/// 提示的文字（界面直接显示）。
#[uniffi::export]
pub fn notice_text(kind: NoticeKind, lang: Lang) -> String {
    core::NoticeKind::from(kind).text(lang.into())
}

/// 解码好的图片：8 位 BGRA、预乘 Alpha。像素由 Rust 持有；界面通过地址直接读取，不复制。
/// 只要这个对象还活着，`pixel_address` 指向的内存就有效且不会变。
#[derive(uniffi::Object, Debug)]
pub struct ImageHandle {
    inner: Arc<core::DecodedImage>,
}

#[uniffi::export]
impl ImageHandle {
    pub fn width(&self) -> u32 {
        self.inner.width
    }

    pub fn height(&self) -> u32 {
        self.inner.height
    }

    /// 每行字节数。
    pub fn stride(&self) -> u32 {
        self.inner.stride as u32
    }

    pub fn byte_len(&self) -> u64 {
        self.inner.pixels.len() as u64
    }

    /// 像素数据的内存地址。
    pub fn pixel_address(&self) -> u64 {
        self.inner.pixels.as_ptr() as u64
    }

    pub fn format_name(&self) -> String {
        self.inner.info.format.clone()
    }

    pub fn pixel_format(&self) -> String {
        self.inner.info.pixel_format.clone()
    }

    pub fn file_size(&self) -> u64 {
        self.inner.info.file_size
    }

    /// 原图带透明通道：透明处画棋盘格。
    pub fn has_alpha(&self) -> bool {
        self.inner.info.has_alpha
    }

    /// 像素所在色彩空间的 ICC（macOS 交给 ColorSync；为空表示 sRGB）。
    pub fn icc_profile(&self) -> Option<Vec<u8>> {
        self.inner
            .info
            .icc_profile
            .as_ref()
            .map(|p| p.as_ref().clone())
    }

    pub fn decode_ms(&self) -> f64 {
        self.inner.info.decode_ms
    }

    /// 交给 GPU 的纹理：超大图是缩小的代理（拉伸到 width x height 显示），其他图片就是自己。
    pub fn texture(&self) -> Arc<ImageHandle> {
        Arc::new(ImageHandle {
            inner: self
                .inner
                .proxy
                .clone()
                .unwrap_or_else(|| Arc::clone(&self.inner)),
        })
    }

    /// 带代理纹理（超大图）。
    pub fn has_proxy(&self) -> bool {
        self.inner.proxy.is_some()
    }
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum Event {
    Pong {
        token: u64,
    },
    /// 当前图片变了。opened：由"打开"引起（不是浏览中切图），图像送达时按图片调整窗口大小。
    Current {
        generation: u64,
        path: String,
        name: String,
        index: Option<u32>,
        total: Option<u32>,
        opened: bool,
    },
    Collection {
        generation: u64,
        index: Option<u32>,
        total: u32,
    },
    ImageReady {
        generation: u64,
        path: String,
        image: Arc<ImageHandle>,
    },
    /// 快速预览：image 是小缩略图，拉伸到原图尺寸 width x height 显示，像 ImageReady 一样复位查看状态；
    /// 清晰图随后以 ImageUpdated 送达。
    ImagePreview {
        generation: u64,
        path: String,
        image: Arc<ImageHandle>,
        width: u32,
        height: u32,
    },
    /// 同一张图片的显示缓冲区换了（反相、Gamma，或清晰图替换快速预览）：只换纹理，不复位查看状态。
    ImageUpdated {
        generation: u64,
        path: String,
        image: Arc<ImageHandle>,
    },
    /// 动图的一帧：只换纹理，不复位查看状态。index 从 0 起；count 是总帧数（第一遍没播完时为空）。
    AnimationFrame {
        generation: u64,
        path: String,
        image: Arc<ImageHandle>,
        index: u32,
        count: Option<u32>,
    },
    /// 精确重绘完成：image 放在视口物理像素 (x, y) 处；signature 与 view_signature 相同时才有效。
    ExactReady {
        generation: u64,
        signature: u64,
        x: i64,
        y: i64,
        image: Arc<ImageHandle>,
        render_ms: f64,
    },
    ImageFailed {
        generation: u64,
        path: String,
        message: String,
    },
    OpenFailed {
        path: String,
        message: String,
    },
    Notice {
        kind: NoticeKind,
    },
    /// 当前图片被安全上限拦下了：界面用 limit_prompt 问用户，"仍然打开"调 open_without_limits，
    /// "取消"调 keep_limits。
    ImageBlocked {
        generation: u64,
        path: String,
    },
    /// 到达首尾：界面询问转到首张（末张）、下一个（上一个）文件夹，或取消。
    /// archive 为真时在压缩包里，对话框写"压缩包"。
    Boundary {
        forward: bool,
        archive: bool,
    },
    Closed {
        generation: u64,
    },
    /// 选择图片窗口要的缩略图好了（picker_want_thumbnails）；生成失败时 image 为空。
    Thumbnail {
        path: String,
        image: Option<Arc<ImageHandle>>,
    },
    /// 背景图片解码好了或换了：重新取 background_image。
    BackgroundChanged,
    /// 别的进程改了设置文件，已经重新读、生效：像"确定"之后那样刷新界面。
    SettingsReloaded {
        changed: Vec<String>,
    },
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

impl From<core::Event> for Event {
    fn from(value: core::Event) -> Self {
        match value {
            core::Event::Pong { token } => Event::Pong { token },
            core::Event::Current {
                generation,
                path,
                name,
                index,
                total,
                opened,
            } => Event::Current {
                generation,
                path: path.display().to_string(),
                name,
                index: index.map(to_u32),
                total: total.map(to_u32),
                opened,
            },
            core::Event::Collection {
                generation,
                index,
                total,
            } => Event::Collection {
                generation,
                index: index.map(to_u32),
                total: to_u32(total),
            },
            core::Event::ImageReady {
                generation,
                path,
                image,
            } => Event::ImageReady {
                generation,
                path: path.display().to_string(),
                image: Arc::new(ImageHandle { inner: image }),
            },
            core::Event::ImagePreview {
                generation,
                path,
                image,
                width,
                height,
            } => Event::ImagePreview {
                generation,
                path: path.display().to_string(),
                image: Arc::new(ImageHandle { inner: image }),
                width,
                height,
            },
            core::Event::ImageUpdated {
                generation,
                path,
                image,
            } => Event::ImageUpdated {
                generation,
                path: path.display().to_string(),
                image: Arc::new(ImageHandle { inner: image }),
            },
            core::Event::AnimationFrame {
                generation,
                path,
                image,
                index,
                count,
            } => Event::AnimationFrame {
                generation,
                path: path.display().to_string(),
                image: Arc::new(ImageHandle { inner: image }),
                index: index as u32,
                count: count.map(|c| c as u32),
            },
            core::Event::ExactReady {
                generation,
                signature,
                x,
                y,
                image,
                render_ms,
            } => Event::ExactReady {
                generation,
                signature,
                x,
                y,
                image: Arc::new(ImageHandle { inner: image }),
                render_ms,
            },
            core::Event::ImageFailed {
                generation,
                path,
                message,
            } => Event::ImageFailed {
                generation,
                path: path.display().to_string(),
                message,
            },
            core::Event::ImageBlocked { generation, path } => Event::ImageBlocked {
                generation,
                path: path.display().to_string(),
            },
            core::Event::OpenFailed { path, message } => Event::OpenFailed { path, message },
            core::Event::Notice(kind) => Event::Notice { kind: kind.into() },
            core::Event::Boundary { forward, archive } => Event::Boundary { forward, archive },
            core::Event::Closed { generation } => Event::Closed { generation },
            core::Event::Thumbnail { path, image } => Event::Thumbnail {
                path: path.display().to_string(),
                image: image.map(|inner| Arc::new(ImageHandle { inner })),
            },
            core::Event::BackgroundChanged => Event::BackgroundChanged,
            core::Event::SettingsReloaded { changed } => Event::SettingsReloaded {
                changed: changed.into_iter().map(String::from).collect(),
            },
        }
    }
}

/// 系统解码器，由界面实现（macOS：ImageIO）。核心在后台线程调用，实现方必须线程安全。
#[uniffi::export(with_foreign)]
pub trait PlatformDecoder: Send + Sync {
    /// 解码成 BGRA8 预乘的图像；`auto_orient` 为真时按 EXIF 方向摆正（设置"根据 EXIF 信息自动旋转图片"）。
    /// `use_icc`：设置"使用文件内的 ICC"。返回内嵌 ICC 的系统（macOS）不用管它，核心按它决定用不用；
    /// 解码时就换成 sRGB 的系统（Windows 的 WIC）关掉时不做色彩管理。不能解码时返回空。
    fn decode(&self, path: String, auto_orient: bool, use_icc: bool) -> Option<PlatformImage>;

    /// 帧数（静态图为 1，读不了为 0）：动态 AVIF、HEICS 等。
    fn frame_count(&self, path: String) -> u32;

    /// 第 index 帧（BGRA8 预乘，按画布大小）和显示时长（毫秒，文件里写的原样）；不能解码时返回空。
    fn decode_frame(&self, path: String, index: u32) -> Option<PlatformFrame>;
}

/// 动图的一帧（系统解码器给出）。
#[derive(uniffi::Record, Clone, Debug)]
pub struct PlatformFrame {
    pub image: PlatformImage,
    pub delay_ms: u32,
}

/// 系统解码器的结果。
#[derive(uniffi::Record, Clone, Debug)]
pub struct PlatformImage {
    pub width: u32,
    pub height: u32,
    /// BGRA8 预乘，按行连续存放（每行 width × 4 字节）。
    pub pixels: Vec<u8>,
    /// 格式名，如 HEIC、AVIF。
    pub format: String,
    /// 像素格式，如 RGB24、RGBA32。
    pub pixel_format: String,
    pub has_alpha: bool,
    /// 像素所在色彩空间的 ICC；为空表示 sRGB。
    pub icc_profile: Option<Vec<u8>>,
    /// EXIF 里的拍摄信息；系统解码器读得到时给出。
    #[uniffi(default = None)]
    pub shooting: Option<ShootingInfo>,
}

/// 拍摄信息：相机厂商、型号、镜头、焦距（毫米）、35 毫米等效焦距、光圈（f 数）、曝光时间（秒）、ISO、
/// 拍摄时间（EXIF 原样，如 "2024:06:22 08:40:39"）。没有的项为空。
#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct ShootingInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<f64>,
    pub focal_length_35mm: Option<u32>,
    pub f_number: Option<f64>,
    pub exposure_time: Option<f64>,
    pub iso: Option<u32>,
    pub taken: Option<String>,
}

impl From<ShootingInfo> for core::exif::ShootingInfo {
    fn from(s: ShootingInfo) -> Self {
        let text = |t: Option<String>| t.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let positive = |v: Option<f64>| v.filter(|v| v.is_finite() && *v > 0.0);
        Self {
            make: text(s.make),
            model: text(s.model),
            lens: text(s.lens),
            focal_length: positive(s.focal_length),
            focal_length_35mm: s.focal_length_35mm.filter(|v| *v > 0),
            f_number: positive(s.f_number),
            exposure_time: positive(s.exposure_time),
            iso: s.iso.filter(|v| *v > 0),
            taken: text(s.taken),
        }
    }
}

/// 窗口内容区的大小（点）。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq)]
pub struct WindowSize {
    pub width: f64,
    pub height: f64,
}

struct DecoderBridge(Arc<dyn PlatformDecoder>);

impl core::decode::PlatformDecode for DecoderBridge {
    fn decode(
        &self,
        path: &std::path::Path,
        auto_orient: bool,
        use_icc: bool,
    ) -> Option<core::decode::PlatformImage> {
        let image = self
            .0
            .decode(path.display().to_string(), auto_orient, use_icc)?;
        Some(platform_image(image))
    }

    fn frame_count(&self, path: &std::path::Path) -> u32 {
        self.0.frame_count(path.display().to_string())
    }

    fn decode_frame(
        &self,
        path: &std::path::Path,
        index: u32,
    ) -> Option<(core::decode::PlatformImage, u32)> {
        let frame = self.0.decode_frame(path.display().to_string(), index)?;
        Some((platform_image(frame.image), frame.delay_ms))
    }
}

fn platform_image(image: PlatformImage) -> core::decode::PlatformImage {
    core::decode::PlatformImage {
        width: image.width,
        height: image.height,
        pixels: image.pixels,
        format: image.format,
        pixel_format: image.pixel_format,
        has_alpha: image.has_alpha,
        icc_profile: image.icc_profile,
        shooting: image.shooting.map(Into::into),
    }
}

/// 登记系统解码器和它负责的扩展名（如 heic、avif）。只能登记一次，返回是否成功。
#[uniffi::export]
pub fn register_platform_decoder(
    decoder: Arc<dyn PlatformDecoder>,
    extensions: Vec<String>,
) -> bool {
    core::decode::register_platform_decoder(Arc::new(DecoderBridge(decoder)), extensions)
}

/// 由界面实现的唤醒回调。核心会在后台线程调用，实现方负责转到主线程再读取事件。
#[uniffi::export(with_foreign)]
pub trait CoreListener: Send + Sync {
    fn wake(&self);
}

struct ListenerBridge(Arc<dyn CoreListener>);

impl core::CoreListener for ListenerBridge {
    fn wake(&self) {
        self.0.wake();
    }
}

#[derive(uniffi::Object)]
pub struct AppCore {
    inner: core::AppCore,
}

#[uniffi::export]
impl AppCore {
    #[uniffi::constructor]
    pub fn new(listener: Arc<dyn CoreListener>) -> Arc<Self> {
        Arc::new(Self {
            inner: core::AppCore::new(Arc::new(ListenerBridge(listener))),
        })
    }

    /// 同一程序里再开一个窗口：与这个窗口共用设置、图片缓存和后台线程。
    pub fn new_window(&self, listener: Arc<dyn CoreListener>) -> Arc<Self> {
        Arc::new(Self {
            inner: self.inner.new_window(Arc::new(ListenerBridge(listener))),
        })
    }

    /// 同一程序里开着的窗口数（含这个，自检用）。
    pub fn window_count(&self) -> u32 {
        self.inner.window_count() as u32
    }

    /// 拖动窗口时吸附屏幕边缘：设置关掉时为空；否则返回窗口新的原点（不用动时与原来相同）。
    pub fn snap_window(&self, frame: WindowFrame, area: WindowFrame) -> Option<ScreenPoint> {
        self.inner
            .snap_window(
                (frame.x, frame.y, frame.width, frame.height),
                (area.x, area.y, area.width, area.height),
            )
            .map(|(x, y)| ScreenPoint { x, y })
    }

    /// 提交命令后立即返回；结果通过事件送回。
    pub fn dispatch(&self, command: Command) {
        self.inner.dispatch(command.into());
    }

    /// 取出当前全部事件。应在界面主线程、收到唤醒后调用。
    pub fn drain_events(&self) -> Vec<Event> {
        self.inner
            .drain_events()
            .into_iter()
            .map(Event::from)
            .collect()
    }

    /// 执行动作（菜单、按键、按钮都走这里）。
    pub fn invoke_action(&self, id: String) -> ActionOutcome {
        self.inner.invoke_action(&id).into()
    }

    /// 按默认键位执行按键；没有绑定时返回空。`key` 是有名字的按键（Left、PageDown、Enter…）
    /// 或按键输入的字符（含 ⇧ 的效果，如 `+`）。
    pub fn invoke_key(&self, key: String, modifiers: KeyModifiers) -> Option<ActionOutcome> {
        self.inner
            .invoke_key(&key, modifiers.bits())
            .map(ActionOutcome::from)
    }

    /// 按键对应的动作 ID；没有绑定时为空。界面查出动作后用 invoke_action 执行（执行后可按动作调整窗口）。
    pub fn action_for_key(&self, key: String, modifiers: KeyModifiers) -> Option<String> {
        self.inner
            .action_for_key(&key, modifiers.bits())
            .map(str::to_string)
    }

    pub fn action_state(&self, id: String) -> ActionState {
        let state = self.inner.action_state(&id);
        ActionState {
            enabled: state.enabled,
            checked: state.checked,
        }
    }

    pub fn view_layout(&self) -> Option<ViewLayout> {
        self.inner.view_layout().map(ViewLayout::from)
    }

    /// 画布大小（物理像素）变了。
    pub fn view_set_viewport(&self, width: f64, height: f64) -> Option<ViewLayout> {
        self.inner
            .view_set_viewport(width, height)
            .map(ViewLayout::from)
    }

    /// 开始显示一张新图片。
    pub fn view_show_image(&self, width: u32, height: u32) -> Option<ViewLayout> {
        self.inner
            .view_show_image(width, height)
            .map(ViewLayout::from)
    }

    pub fn view_clear(&self) {
        self.inner.view_clear();
    }

    pub fn view_pan_by(&self, dx: f64, dy: f64) -> Option<ViewLayout> {
        self.inner.view_pan_by(dx, dy).map(ViewLayout::from)
    }

    pub fn view_zoom_by(&self, factor: f64, x: f64, y: f64) -> Option<ViewLayout> {
        self.inner.view_zoom_by(factor, x, y).map(ViewLayout::from)
    }

    pub fn view_zoom_step(&self, direction: i32, x: f64, y: f64) -> Option<ViewLayout> {
        self.inner
            .view_zoom_step(direction, x, y)
            .map(ViewLayout::from)
    }

    pub fn view_toggle_fit_actual(&self, x: f64, y: f64) -> Option<ViewLayout> {
        self.inner
            .view_toggle_fit_actual(x, y)
            .map(ViewLayout::from)
    }

    /// 画面签名：精确重绘的结果只在签名相同时有效。
    pub fn view_signature(&self) -> u64 {
        self.inner.view_signature()
    }

    /// 精确重绘：交互停止约 100 ms 后调用；需要时结果以 ExactReady 送达。
    pub fn request_exact(&self) {
        self.inner.request_exact();
    }

    /// 当前画面是否需要精确重绘（自检用：超大图的代理被放大时等它盖上再截图）。
    pub fn needs_exact(&self) -> bool {
        self.inner.needs_exact()
    }

    /// 当前图片的原图（按 EXIF 方向，不含反相等效果；打印用）；还没解码好时为空。
    pub fn original_image(&self) -> Option<Arc<ImageHandle>> {
        self.inner
            .original_image()
            .map(|inner| Arc::new(ImageHandle { inner }))
    }

    /// 这个窗口所在显示器的 ICC 配置文件（"使用监视器的 ICC 设置"，；Windows 用，macOS 由系统负责）。
    /// 窗口出现、换到别的显示器时传入；没有配置文件时传空。设置打开时画面按它转换颜色（随后以 ImageUpdated 送达）。
    /// 返回画面的颜色是否要变。
    pub fn set_display_profile(&self, icc: Option<Vec<u8>>) -> bool {
        self.inner.set_display_profile(icc)
    }

    /// 正在显示的动图的播放状态（动画信息控制）；不是动图时为空。
    pub fn animation(&self) -> Option<AnimationInfo> {
        self.inner.animation().map(|a| AnimationInfo {
            index: a.index as u32,
            count: a.count.map(|c| c as u32),
            playing: a.playing,
        })
    }

    /// 这一代次的切图要做的翻页效果和音效：只有翻页切过来的才有；效果为"无"且不放音效时为空。
    /// 界面在 Current 事件时放音效，在这一代次的图像送达时做效果。
    pub fn transition_for(&self, generation: u64) -> Option<Transition> {
        self.inner.transition_for(generation).map(|t| Transition {
            effect: t.effect.into(),
            forward: t.forward,
            duration_ms: t.duration_ms,
            sound: t.sound,
        })
    }

    /// 背景图片（设置"更改默认背景图片"）：按"填满并裁切"铺在图片后面，空态也用；没有时用背景色。
    /// 第一次要时在后台解码，好了发 BackgroundChanged。
    pub fn background_image(&self) -> Option<Arc<ImageHandle>> {
        self.inner
            .background_image()
            .map(|inner| Arc::new(ImageHandle { inner }))
    }

    /// 帮助窗口的标题。
    pub fn help_title(&self, kind: HelpKind, lang: Lang) -> String {
        core::help::HelpKind::from(kind).title(lang.into()).to_string()
    }

    /// 帮助窗口里的一段文字：只有"开源许可"有，界面放在可以选中复制、能滚动的文字框里；别的帮助窗口为空。
    pub fn help_text(&self, kind: HelpKind, lang: Lang) -> Option<String> {
        self.inner.help_text(kind.into(), lang.into())
    }

    /// 帮助窗口的内容：分组标题和两列的行。
    pub fn help_rows(&self, kind: HelpKind, lang: Lang) -> Vec<HelpRow> {
        self.inner
            .help_rows(kind.into(), lang.into())
            .into_iter()
            .map(|r| HelpRow {
                heading: r.heading,
                left: r.left,
                right: r.right,
            })
            .collect()
    }

    /// 当前图片里的 AI 生成信息；没有，或者还没解码好时为空。换了图片（Current、ImageReady 事件）时重新取。
    pub fn generation_info(&self) -> Option<GenerationInfo> {
        self.inner.generation_info().map(|g| GenerationInfo {
            tool: g.tool.clone(),
            prompt: g.prompt.clone(),
            negative: g.negative.clone(),
            params: g
                .params
                .iter()
                .map(|(name, value)| GenerationParam {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            raw: g.raw.clone(),
        })
    }

    /// 当前图片还在读（还没解码好，也没有读不了）：“生成信息”面板这时不换内容，等读好了（ImageReady）再换。
    pub fn generation_pending(&self) -> bool {
        self.inner.generation_pending()
    }

    /// “生成信息”面板上的文字。
    pub fn generation_texts(&self, lang: Lang) -> GenerationTexts {
        let t = core::generation::panel_texts(lang.into());
        GenerationTexts {
            title: t.title,
            tool: t.tool,
            prompt: t.prompt,
            negative: t.negative,
            params: t.params,
            copy: t.copy,
            copy_raw: t.copy_raw,
            copied: t.copied,
            empty: t.empty,
        }
    }

    /// 选择图片窗口的列表；文件夹还没扫描完时为空。排序、刷新后收到 Collection 事件时重新取。
    pub fn picker_list(&self, lang: Lang) -> Option<PickerList> {
        self.inner.picker_list(lang.into()).map(|list| PickerList {
            rows: list
                .rows
                .into_iter()
                .map(|r| PickerRow {
                    path: r.path,
                    name: r.name,
                    modified: r.modified,
                    size: r.size,
                })
                .collect(),
            current: list.current.map(to_u32),
            sort_label: list.sort_label,
            sort_options: list
                .sort_options
                .into_iter()
                .map(|o| PickerSortOption {
                    action: o.action,
                    title: o.title,
                    checked: o.checked,
                })
                .collect(),
        })
    }

    /// 选择图片窗口的"确定"、双击：切到第 index 张。
    pub fn picker_choose(&self, index: u32) {
        self.inner.picker_choose(index as usize);
    }

    /// 已经生成好的缩略图；没有时为空。
    pub fn picker_thumbnail(&self, path: String) -> Option<Arc<ImageHandle>> {
        self.inner
            .picker_thumbnail(&path)
            .map(|inner| Arc::new(ImageHandle { inner }))
    }

    /// 现在看得见的那几张（替换上一次的）：没有缩略图的在后台生成，好了发 Thumbnail 事件。
    pub fn picker_want_thumbnails(&self, paths: Vec<String>) {
        self.inner.picker_want_thumbnails(paths);
    }

    /// "隐藏缩略图导航"勾选框（随时记住）。
    pub fn picker_hides_thumbnails(&self) -> bool {
        self.inner.picker_hides_thumbnails()
    }

    pub fn set_picker_hides_thumbnails(&self, hide: bool) {
        self.inner.set_picker_hides_thumbnails(hide);
    }

    /// 这张图片的透明处怎么画。
    pub fn alpha_fill(&self, image: Arc<ImageHandle>) -> AlphaFill {
        match self.inner.alpha_fill(&image.inner) {
            core::settings::AlphaFill::Checker => AlphaFill::Checker,
            core::settings::AlphaFill::Solid(rgb) => AlphaFill::Solid { rgb },
        }
    }

    /// 窗口跟着图片：切到这张新图片时窗口内容区应有的大小（点）。available：屏幕可用区域（点）。
    /// 关掉"根据图片大小调节窗口大小"时为空：窗口不变。
    pub fn window_size_for_image(
        &self,
        width: u32,
        height: u32,
        available_width: f64,
        available_height: f64,
    ) -> Option<WindowSize> {
        self.inner
            .window_size_for_image(width, height, (available_width, available_height))
            .map(|(width, height)| WindowSize { width, height })
    }

    /// 窗口是否跟着图片变。
    pub fn window_follows_image(&self) -> bool {
        self.inner.window_follows_image()
    }

    /// 当前图片按现在的方向应有的窗口大小（退出最大化、全屏时用）；没有图片时为空。
    pub fn window_size_for_view(
        &self,
        available_width: f64,
        available_height: f64,
    ) -> Option<WindowSize> {
        self.inner
            .window_size_for_view((available_width, available_height))
            .map(|(width, height)| WindowSize { width, height })
    }

    /// 执行动作后窗口要不要跟着变（旋转后跟着图片转）；要变时给出新的大小。
    pub fn window_size_after_action(
        &self,
        id: String,
        available_width: f64,
        available_height: f64,
    ) -> Option<WindowSize> {
        self.inner
            .window_size_after_action(&id, (available_width, available_height))
            .map(|(width, height)| WindowSize { width, height })
    }

    /// 左键拖动是否移动窗口：图片完整显示、窗口能移动时移动窗口；否则平移图片。
    pub fn left_drag_moves_window(&self, window_movable: bool) -> bool {
        self.inner.left_drag_moves_window(window_movable)
    }

    /// 窗口所在屏幕的缩放比例（一个点对应的物理像素，Retina 为 2）：100% 按它换算。
    /// 建好画布、换到别的屏幕时调用，在设置画布大小之前。
    pub fn view_set_pixels_per_point(&self, pixels_per_point: f64) -> Option<ViewLayout> {
        self.inner
            .view_set_pixels_per_point(pixels_per_point)
            .map(ViewLayout::from)
    }

    /// 图片缓存现在占用的字节数。
    pub fn cache_bytes(&self) -> u64 {
        self.inner.cache_bytes()
    }

    /// 被上限拦下的图片（ImageBlocked），用户选了"仍然打开"：这个文件之后不设上限，重新读。
    pub fn open_without_limits(&self, path: String) {
        self.inner.open_without_limits(std::path::Path::new(&path));
    }

    /// 被上限拦下的图片，用户选了"取消"：这一次运行里不再问。
    pub fn keep_limits(&self, path: String) {
        self.inner.keep_limits(std::path::Path::new(&path));
    }

    /// 当前图片的路径。
    pub fn current_path(&self) -> Option<String> {
        self.inner.current_path().map(|p| p.display().to_string())
    }

    /// 左上角图片信息的各行文字。
    pub fn info_lines(&self, lang: Lang) -> Vec<String> {
        self.inner.info_lines(lang.into())
    }

    // ---- 设置（docs/SETTINGS.md）----

    /// 读设置文件并应用：启动时、打开第一张图片之前调用一次。path 为空时用平台的默认位置。
    pub fn load_settings(&self, path: Option<String>) -> SettingsLoad {
        self.inner
            .load_settings(path.map(std::path::PathBuf::from))
            .into()
    }

    /// 启动时设置文件坏了的提示（只给一次）；没有问题时为空。
    pub fn settings_notice(&self, lang: Lang) -> Option<String> {
        self.inner.settings_notice(lang.into())
    }

    /// 设置文件的位置。
    pub fn settings_path(&self) -> Option<String> {
        self.inner.settings_path().map(|p| p.display().to_string())
    }

    /// 界面语言：设置为"跟随系统"时用 system。
    pub fn ui_lang(&self, system: Lang) -> Lang {
        self.inner.ui_lang(system.into()).into()
    }

    /// 界面要按设置显示的东西（背景色、隐藏控件等）。
    pub fn ui_settings(&self) -> UiSettings {
        self.inner.ui_settings().into()
    }

    /// 一项设置现在的值，写成文字（自检用）：开关为 true/false，颜色为 #RRGGBB，选项为 ID。
    pub fn setting_text(&self, key: String) -> Option<String> {
        use core::settings::Value;
        Some(match self.inner.setting(&key)? {
            Value::Bool(b) => b.to_string(),
            Value::Choice(id) => id,
            Value::Int(n) => n.to_string(),
            Value::Color(c) => core::settings::color_text(c),
            Value::Text(t) | Value::Invalid(t) => t,
            Value::Mapping { enabled, action } => format!("{enabled} {action}"),
            Value::KeyMappings(rows) => format!("{} rows", rows.len()),
        })
    }

    /// 打开设置窗口：把当前设置复制成草稿。窗口已经打开时保留草稿。
    pub fn settings_begin(&self) {
        self.inner.settings_begin();
    }

    /// 关闭设置窗口而不确定：丢掉草稿。
    pub fn settings_cancel(&self) {
        self.inner.settings_cancel();
    }

    /// 左侧分类列表（只含这个平台有设置项的页）。
    pub fn settings_pages(&self, lang: Lang) -> Vec<SettingsPageInfo> {
        self.inner
            .settings_pages(lang.into())
            .into_iter()
            .map(|(id, title)| SettingsPageInfo {
                id: id.to_string(),
                title: title.to_string(),
            })
            .collect()
    }

    /// 一页的内容（按草稿的值）。
    pub fn settings_page(&self, page: String, lang: Lang) -> Option<SettingsPage> {
        self.inner
            .settings_page(&page, lang.into())
            .map(SettingsPage::from)
    }

    /// 改草稿里的一项；返回是否接受。之后重新取这一页，更新各项的可用状态。
    pub fn settings_set(&self, key: String, input: SettingsInput) -> bool {
        self.inner.settings_set(&key, input.into())
    }

    /// "重置"：这一页的草稿恢复为默认值。
    pub fn settings_reset_page(&self, page: String) {
        self.inner.settings_reset_page(&page);
    }

    /// 有没有没确定的修改。
    pub fn settings_dirty(&self) -> bool {
        self.inner.settings_dirty()
    }

    /// "确定"：校验、生效、保存。校验失败时返回出错的页、项和提示（按 lang），草稿保留。
    pub fn settings_apply(&self, lang: Lang) -> SettingsApplyResult {
        match self.inner.settings_apply() {
            Ok(applied) => SettingsApplyResult::Applied {
                changed: applied.changed.iter().map(|k| k.to_string()).collect(),
                save_error: applied.save_error,
            },
            Err(invalid) => SettingsApplyResult::Invalid {
                page: invalid.page.id().to_string(),
                key: invalid.key.to_string(),
                row: invalid.row.map(|r| r as u32),
                message: invalid.message(core::Platform::current(), lang.into()),
            },
        }
    }

    /// 设置窗口里的按钮：要界面做的事；项不可用时为空。
    pub fn settings_button(&self, key: String, button: String) -> Option<SettingsCommand> {
        self.inner.settings_button(&key, &button).map(Into::into)
    }

    /// 退出前调用：打开了"保存缩放状态"时记下显示方式和缩放锁定。
    pub fn remember_view_state(&self) {
        self.inner.remember_view_state();
    }

    /// 程序退出前：记下查看状态，关掉所有压缩包（删掉解出的临时文件）。
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    /// 设置 › 书签的"删除"：忘掉所有压缩包内的位置（界面先确认）。
    pub fn clear_archive_positions(&self) {
        self.inner.clear_archive_positions();
    }

    /// 记住了几个压缩包内的位置（自检用）。
    pub fn archive_positions_count(&self) -> u32 {
        self.inner.archive_positions_count() as u32
    }

    /// 最近打开的文件、文件夹、压缩包（Windows 菜单栏的"打开最近使用"），最近的在最前。
    pub fn recent_files(&self) -> Vec<String> {
        self.inner.recent_files()
    }

    /// "打开最近使用"里的"清除菜单"。
    pub fn clear_recent(&self) {
        self.inner.clear_recent();
    }

    /// 键盘设置里热键旁小按钮列出的按键：首项"清除"（ID 为空），其余 ID 为按键名称，可直接用作 KeyRowKey 的 key。
    pub fn settings_special_keys(&self, lang: Lang) -> Vec<SettingsOption> {
        options(self.inner.settings_special_keys(lang.into()))
    }

    // ---- 菜单与提示（按键盘映射，ACTIONS 规则 7）----

    /// 右键菜单或 macOS 菜单栏的数据：快捷键是现在实际生效的按键。设置改了以后重新取。
    pub fn menu_entries(&self, menu: MenuKind, lang: Lang) -> Vec<MenuEntry> {
        convert_menu(
            self.inner.menu(menu.into(), lang.into()),
            core::Platform::current(),
        )
    }

    /// 按钮的悬停提示：名称加实际生效的快捷键。
    pub fn action_tooltip(&self, id: String, lang: Lang) -> String {
        self.inner.tooltip(&id, lang.into())
    }

    // ---- 鼠标（设置"鼠标"页）----

    /// 中键单击、右键、侧键、滚轮现在做什么。
    pub fn mouse_action(&self, input: MouseInput) -> MouseResult {
        self.inner.mouse_action(input.into()).into()
    }

    /// 左键单击要等松开才能决定：按下时先不拖动窗口，移动超过几个点才算拖动，没动就松开才算单击。
    pub fn left_click_matters(&self) -> bool {
        self.inner.left_click_matters()
    }

    /// 左键单击（没有拖动就松开）要执行的动作；x_fraction：点在画面宽度上的位置（0–1）。
    pub fn left_click_action(&self, x_fraction: f64) -> Option<String> {
        self.inner.left_click_action(x_fraction).map(str::to_string)
    }

    /// 左键双击（第二次按下时）要执行的动作。
    pub fn left_double_click_action(&self, x_fraction: f64) -> Option<String> {
        self.inner
            .left_double_click_action(x_fraction)
            .map(str::to_string)
    }

    /// 触控板双指滑动：手势结束时判断是否翻页，翻页时返回执行结果。dx、dy 为画面移动方向（物理像素）。
    pub fn view_swipe(&self, phase: SwipePhase, dx: f64, dy: f64) -> Option<ActionOutcome> {
        let phase = match phase {
            SwipePhase::Began => core::SwipePhase::Began,
            SwipePhase::Changed => core::SwipePhase::Changed,
            SwipePhase::Ended => core::SwipePhase::Ended,
            SwipePhase::Cancelled => core::SwipePhase::Cancelled,
        };
        self.inner
            .view_swipe(phase, dx, dy)
            .map(ActionOutcome::from)
    }

    /// 图片像素坐标 → 画布物理像素坐标（自检用来核对画面）。
    pub fn view_image_to_screen(&self, u: f64, v: f64) -> Option<ScreenPoint> {
        self.inner
            .view_image_to_screen(u, v)
            .map(|(x, y)| ScreenPoint { x, y })
    }

    pub fn view_screen_to_image(&self, x: f64, y: f64) -> Option<ScreenPoint> {
        self.inner
            .view_screen_to_image(x, y)
            .map(|(x, y)| ScreenPoint { x, y })
    }
}

/// 修饰键。`command` 是 macOS 的 ⌘。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub command: bool,
}

impl KeyModifiers {
    fn bits(self) -> u32 {
        use core::actions::{ALT, CMD, CTRL, SHIFT};
        [
            (self.shift, SHIFT),
            (self.control, CTRL),
            (self.alt, ALT),
            (self.command, CMD),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |acc, (_, bit)| acc | bit)
    }

    fn from_bits(bits: u32) -> Self {
        use core::actions::{ALT, CMD, CTRL, SHIFT};
        Self {
            shift: bits & SHIFT != 0,
            control: bits & CTRL != 0,
            alt: bits & ALT != 0,
            command: bits & CMD != 0,
        }
    }
}

/// 绘制参数（物理像素）。图片中心在 (center_x, center_y)；先按 width×height 摆正，
/// 顺时针旋转 rotation 度，再按屏幕方向翻转。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq)]
pub struct ViewLayout {
    pub center_x: f64,
    pub center_y: f64,
    pub width: f64,
    pub height: f64,
    /// 物理倍率：一个图片像素对应的物理像素数。
    pub scale: f64,
    /// 用户看到的倍率（100% = 1）：倍率提示等都用它。
    pub zoom: f64,
    pub rotation: u16,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
    /// 平滑显示；为假时用最近邻（像素边缘处理"无"）。
    pub smooth: bool,
}

impl From<core::Layout> for ViewLayout {
    fn from(l: core::Layout) -> Self {
        Self {
            center_x: l.center_x,
            center_y: l.center_y,
            width: l.width,
            height: l.height,
            scale: l.scale,
            zoom: l.zoom,
            rotation: l.rotation,
            flip_horizontal: l.flip_horizontal,
            flip_vertical: l.flip_vertical,
            smooth: l.smooth,
        }
    }
}

#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

/// 哪一个帮助窗口。
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpKind {
    Shortcuts,
    Formats,
    /// 开源许可：没有表，只有 `help_text` 给的一段文字。
    Licenses,
}

impl From<core::help::HelpKind> for HelpKind {
    fn from(value: core::help::HelpKind) -> Self {
        match value {
            core::help::HelpKind::Shortcuts => Self::Shortcuts,
            core::help::HelpKind::Formats => Self::Formats,
            core::help::HelpKind::Licenses => Self::Licenses,
        }
    }
}

impl From<HelpKind> for core::help::HelpKind {
    fn from(value: HelpKind) -> Self {
        match value {
            HelpKind::Shortcuts => Self::Shortcuts,
            HelpKind::Formats => Self::Formats,
            HelpKind::Licenses => Self::Licenses,
        }
    }
}

/// AI 生成的图片里记的生成信息。参数的名字照生成工具自己的写法，不翻译。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct GenerationInfo {
    /// 生成工具，如 "ComfyUI"。
    pub tool: String,
    pub prompt: String,
    /// 反向提示词；没有时为空。
    pub negative: String,
    pub params: Vec<GenerationParam>,
    /// 文件里原样的那段文字（“复制原文”）。
    pub raw: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct GenerationParam {
    pub name: String,
    pub value: String,
}

/// “生成信息”面板上的文字。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct GenerationTexts {
    pub title: String,
    /// 生成工具那一行，{tool} 换成工具的名字。
    pub tool: String,
    pub prompt: String,
    pub negative: String,
    pub params: String,
    pub copy: String,
    pub copy_raw: String,
    pub copied: String,
    pub empty: String,
}

/// 帮助窗口的一行：分组标题（heading 为真，只有左边一列），或左右两列。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct HelpRow {
    pub heading: bool,
    pub left: String,
    pub right: String,
}

/// 翻页效果（设置"翻页效果"）。
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageEffect {
    None,
    Page,
    Fade,
    Slide,
}

impl From<core::transition::PageEffect> for PageEffect {
    fn from(value: core::transition::PageEffect) -> Self {
        use core::transition::PageEffect as E;
        match value {
            E::None => Self::None,
            E::Page => Self::Page,
            E::Fade => Self::Fade,
            E::Slide => Self::Slide,
        }
    }
}

impl From<PageEffect> for core::transition::PageEffect {
    fn from(value: PageEffect) -> Self {
        match value {
            PageEffect::None => Self::None,
            PageEffect::Page => Self::Page,
            PageEffect::Fade => Self::Fade,
            PageEffect::Slide => Self::Slide,
        }
    }
}

/// 这一次切图要做的效果和音效。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub effect: PageEffect,
    /// 往后翻（下一张、跳到后面、随机、下一个文件夹）为真。
    pub forward: bool,
    pub duration_ms: u32,
    pub sound: bool,
}

/// 一层画面在某一时刻的样子：不透明度、水平平移（画布宽度的倍数）、绕画布左边竖直轴转的角度（度，
/// 负数表示右边缘朝观看者翻起）、盖在上面的黑色的不透明度。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq)]
pub struct LayerParams {
    pub opacity: f64,
    pub shift: f64,
    pub angle: f64,
    pub shade: f64,
}

impl From<core::transition::LayerParams> for LayerParams {
    fn from(p: core::transition::LayerParams) -> Self {
        Self {
            opacity: p.opacity,
            shift: p.shift,
            angle: p.angle,
            shade: p.shade,
        }
    }
}

/// 某一时刻两层画面的样子。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq)]
pub struct TransitionFrame {
    pub old: LayerParams,
    pub new: LayerParams,
    pub old_on_top: bool,
}

/// 翻页效果在进度 progress（0–1，按时间算）时两层画面的样子。
#[uniffi::export]
pub fn transition_frame(effect: PageEffect, forward: bool, progress: f64) -> TransitionFrame {
    let f = core::transition::frame(effect.into(), forward, progress);
    TransitionFrame {
        old: f.old.into(),
        new: f.new.into(),
        old_on_top: f.old_on_top,
    }
}

/// 透视的视点距离：画布宽度的几倍（翻页效果）。
#[uniffi::export]
pub fn transition_perspective() -> f64 {
    core::transition::PERSPECTIVE
}

/// 翻页音效：合成的翻纸声，WAV。
#[uniffi::export]
pub fn page_sound_wav() -> Vec<u8> {
    core::transition::page_sound_wav().to_vec()
}

/// 选择图片窗口的一行。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct PickerRow {
    /// 浏览集合里的路径（请求缩略图时用）。
    pub path: String,
    /// 显示的名字：文件夹（压缩包）名加文件名。
    pub name: String,
    pub modified: String,
    pub size: String,
}

/// 选择图片窗口的列表。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct PickerList {
    pub rows: Vec<PickerRow>,
    /// 当前图片所在的行（打开窗口时选中它）。
    pub current: Option<u32>,
    /// 排序按钮上的文字，如"按名称排序"。
    pub sort_label: String,
    /// 排序按钮的菜单：文件名、修改日期、大小、类型（单选）和降序。
    pub sort_options: Vec<PickerSortOption>,
}

/// 排序菜单的一项：执行的动作（交给 invoke_action）、文字、是否勾选。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct PickerSortOption {
    pub action: String,
    pub title: String,
    pub checked: bool,
}

/// 动图的播放状态。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationInfo {
    pub index: u32,
    pub count: Option<u32>,
    pub playing: bool,
}

/// 透明处画什么：棋盘格，或纯色 0xRRGGBB。
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaFill {
    Checker,
    Solid { rgb: u32 },
}

/// 窗口或屏幕区域：原点和大小（点）。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq)]
pub struct WindowFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum UiRequest {
    OpenFileDialog,
    OpenFolderDialog,
    Quit,
    /// Esc：开着别的窗口时只关这个窗口，这是最后一个窗口时退出程序。
    CloseOrQuit,
    ToggleFullscreen,
    Minimize,
    ToggleMaximize,
    ShowAbout,
    ShowContextMenu,
    CloseWindow,
    HideApp,
    HideOthers,
    ShowAll,
    /// 移到废纸篓，成功后发 Command::Removed。confirm 为真时先确认（设置"删除文件时确认"）。
    TrashFile {
        path: String,
        name: String,
        confirm: bool,
    },
    /// 在访达中显示并选中。
    RevealFile {
        path: String,
    },
    /// 把当前图片和文件放进剪贴板。
    CopyImage {
        path: String,
    },
    /// macOS"打开最近使用 › 清除菜单"。
    ClearRecent,
    /// 打开设置窗口（已经打开时放到最前面）。
    ShowSettings,
    /// 界面相关的设置变了（如"总在最前"）：重新读 ui_settings。
    ApplySettings,
    /// 用图像编辑器打开：editor 为空时提示去设置。
    OpenInEditor { path: String, editor: String },
    /// 设为桌面图片。
    SetWallpaper { path: String },
    /// 打印当前图片。
    Print { name: String },
    /// 打开选择图片窗口（已经打开时放到最前面）。
    ShowPicker,
    /// 打开帮助窗口：内容从 help_rows 取。
    ShowHelp { kind: HelpKind },
    /// 打开“生成信息”面板（已经打开时放到最前面）：内容从 generation_info 取，换了图片时重新取。
    ShowGenerationInfo,
}

impl From<core::UiRequest> for UiRequest {
    fn from(value: core::UiRequest) -> Self {
        use core::UiRequest as R;
        match value {
            R::OpenFileDialog => Self::OpenFileDialog,
            R::OpenFolderDialog => Self::OpenFolderDialog,
            R::Quit => Self::Quit,
            R::CloseOrQuit => Self::CloseOrQuit,
            R::ToggleFullscreen => Self::ToggleFullscreen,
            R::Minimize => Self::Minimize,
            R::ToggleMaximize => Self::ToggleMaximize,
            R::ShowAbout => Self::ShowAbout,
            R::ShowContextMenu => Self::ShowContextMenu,
            R::CloseWindow => Self::CloseWindow,
            R::HideApp => Self::HideApp,
            R::HideOthers => Self::HideOthers,
            R::ShowAll => Self::ShowAll,
            R::TrashFile {
                path,
                name,
                confirm,
            } => Self::TrashFile {
                path,
                name,
                confirm,
            },
            R::RevealFile { path } => Self::RevealFile { path },
            R::CopyImage { path } => Self::CopyImage { path },
            R::ClearRecent => Self::ClearRecent,
            R::ShowSettings => Self::ShowSettings,
            R::ApplySettings => Self::ApplySettings,
            R::OpenInEditor { path, editor } => Self::OpenInEditor { path, editor },
            R::SetWallpaper { path } => Self::SetWallpaper { path },
            R::Print { name } => Self::Print { name },
            R::ShowPicker => Self::ShowPicker,
            R::ShowHelp { kind } => Self::ShowHelp { kind: kind.into() },
            R::ShowGenerationInfo => Self::ShowGenerationInfo,
        }
    }
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    /// 现在不能执行（没有图片，或尚未实现）。
    Unavailable,
    /// 已开始，结果由事件送回。
    Started,
    /// 查看参数变了：取 view_layout 重绘；show_zoom 为真时短暂显示倍率。
    ViewChanged {
        show_zoom: bool,
    },
    /// 悬浮界面的开关变了（图片信息、控制栏锁定）：按 action_state 更新显示。
    OverlayChanged,
    Ui {
        request: UiRequest,
    },
}

impl From<core::ActionOutcome> for ActionOutcome {
    fn from(value: core::ActionOutcome) -> Self {
        match value {
            core::ActionOutcome::Unavailable => Self::Unavailable,
            core::ActionOutcome::Started => Self::Started,
            core::ActionOutcome::ViewChanged { show_zoom } => Self::ViewChanged { show_zoom },
            core::ActionOutcome::OverlayChanged => Self::OverlayChanged,
            core::ActionOutcome::Ui(request) => Self::Ui {
                request: request.into(),
            },
        }
    }
}

#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionState {
    pub enabled: bool,
    pub checked: bool,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Context,
    MenuBar,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRole {
    Normal,
    App,
    Services,
    Edit,
    Window,
    Help,
    /// macOS 的"打开最近使用"。
    Recent,
}

/// 菜单上显示的快捷键。`key` 的写法同 invoke_key；`label` 是按平台格式化好的文字。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: String,
    pub modifiers: KeyModifiers,
    pub label: String,
}

/// 菜单项，按顺序排成扁平列表；子菜单用 SubmenuStart、SubmenuEnd 括起来。
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Action {
        id: String,
        title: String,
        shortcut: Option<Shortcut>,
        checkable: bool,
    },
    Separator,
    SubmenuStart {
        title: String,
        role: MenuRole,
    },
    SubmenuEnd,
}

/// 右键菜单或 macOS 菜单栏的数据，快捷键按默认键位。界面用 AppCore.menu_entries
/// （按键盘映射显示实际生效的按键）。
#[uniffi::export]
pub fn menu_entries(menu: MenuKind, platform: Platform, lang: Lang) -> Vec<MenuEntry> {
    let platform: core::Platform = platform.into();
    convert_menu(
        core::actions::menu(menu.into(), platform, lang.into()),
        platform,
    )
}

impl From<MenuKind> for core::actions::MenuKind {
    fn from(value: MenuKind) -> Self {
        match value {
            MenuKind::Context => Self::Context,
            MenuKind::MenuBar => Self::MenuBar,
        }
    }
}

fn convert_menu(
    entries: Vec<core::actions::MenuEntry>,
    platform: core::Platform,
) -> Vec<MenuEntry> {
    use core::actions::{MenuEntry as E, MenuRole as R};
    entries
        .into_iter()
        .map(|entry| match entry {
            E::Action {
                id,
                title,
                shortcut,
                checkable,
            } => MenuEntry::Action {
                id: id.to_string(),
                title,
                shortcut: shortcut.map(|chord| Shortcut {
                    label: chord.label(platform),
                    modifiers: KeyModifiers::from_bits(chord.mods),
                    key: chord.key,
                }),
                checkable,
            },
            E::Separator => MenuEntry::Separator,
            E::SubmenuStart { title, role } => MenuEntry::SubmenuStart {
                title,
                role: match role {
                    R::Normal => MenuRole::Normal,
                    R::App => MenuRole::App,
                    R::Services => MenuRole::Services,
                    R::Edit => MenuRole::Edit,
                    R::Window => MenuRole::Window,
                    R::Help => MenuRole::Help,
                    R::Recent => MenuRole::Recent,
                },
            },
            E::SubmenuEnd => MenuEntry::SubmenuEnd,
        })
        .collect()
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipePhase {
    Began,
    Changed,
    Ended,
    Cancelled,
}

/// 指针所在的悬停区域。
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoverZone {
    None,
    Left,
    Right,
    Bar,
}

/// 悬浮界面显示与淡出延时（毫秒）。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayTiming {
    pub fade_in_ms: u32,
    pub fade_out_ms: u32,
    pub bar_hide_delay_ms: u32,
    pub arrow_idle_ms: u32,
}

#[uniffi::export]
pub fn overlay_timing() -> OverlayTiming {
    use core::overlay as o;
    OverlayTiming {
        fade_in_ms: o::FADE_IN_MS,
        fade_out_ms: o::FADE_OUT_MS,
        bar_hide_delay_ms: o::BAR_HIDE_DELAY_MS,
        arrow_idle_ms: o::ARROW_IDLE_MS,
    }
}

/// 指针在内容区的位置（点，原点在左上）属于哪个悬停区域。
#[uniffi::export]
pub fn hover_zone(x: f64, y: f64, width: f64, height: f64) -> HoverZone {
    match core::overlay::hover_zone(x, y, width, height) {
        core::overlay::HoverZone::None => HoverZone::None,
        core::overlay::HoverZone::Left => HoverZone::Left,
        core::overlay::HoverZone::Right => HoverZone::Right,
        core::overlay::HoverZone::Bar => HoverZone::Bar,
    }
}

/// 进度条滑块位置（0–1）。
#[uniffi::export]
pub fn progress_fraction(index: u32, total: u32) -> f64 {
    core::overlay::progress_fraction(index as usize, total as usize)
}

/// 进度条上指针位置（0–1）对应的序号（从 0 开始）。
#[uniffi::export]
pub fn progress_index(fraction: f64, total: u32) -> Option<u32> {
    core::overlay::progress_index(fraction, total as usize).map(|i| i as u32)
}

/// 进度提示，如 841/1862。
#[uniffi::export]
pub fn position_text(index: u32, total: u32) -> String {
    core::overlay::position_text(index as usize, total as usize)
}

/// 按钮的悬停提示，如"打开图片 (⌘O)"。
#[uniffi::export]
pub fn action_tooltip(id: String, platform: Platform, lang: Lang) -> String {
    core::actions::tooltip(&id, platform.into(), lang.into())
}

/// 对话框的文字：说明和按钮（从左到右，最后一个是取消）。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub message: String,
    pub buttons: Vec<String>,
}

impl From<core::i18n::Prompt> for Prompt {
    fn from(p: core::i18n::Prompt) -> Self {
        Self {
            message: p.message,
            buttons: p.buttons,
        }
    }
}

/// 到达首尾时的询问：按钮依次为转到首张（末张）、转到下一个（上一个）文件夹、取消。
#[uniffi::export]
pub fn boundary_prompt(forward: bool, archive: bool, lang: Lang) -> Prompt {
    core::i18n::boundary_prompt(forward, archive, lang.into()).into()
}

/// 图片被安全上限拦下时的询问：按钮为仍然打开、取消。
#[uniffi::export]
pub fn limit_prompt(lang: Lang) -> Prompt {
    core::i18n::limit_prompt(lang.into()).into()
}

/// 删除前的确认：按钮为确认、取消。
#[uniffi::export]
pub fn trash_prompt(platform: Platform, lang: Lang, name: String) -> Prompt {
    core::i18n::trash_prompt(platform.into(), lang.into(), &name).into()
}

/// 交互停止多久后请求精确重绘（毫秒）。
#[uniffi::export]
pub fn exact_render_delay_ms() -> u32 {
    core::exact::DELAY_MS
}

/// 缩放倍率提示的文字，如 150%。
#[uniffi::export]
pub fn zoom_text(scale: f64) -> String {
    core::zoom_text(scale)
}

/// 适合窗口的显示位置，参数和结果都用屏幕物理像素。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq)]
pub struct FitRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[uniffi::export]
pub fn fit_rect(image_width: u32, image_height: u32, view_width: f64, view_height: f64) -> FitRect {
    let r = core::fit_rect(image_width, image_height, view_width, view_height);
    FitRect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

/// 能浏览的压缩包的扩展名（小写，不带点），供打开面板筛选。
#[uniffi::export]
pub fn archive_extensions() -> Vec<String> {
    core::archive::EXTENSIONS
        .iter()
        .map(|e| e.to_string())
        .collect()
}

/// 文件管理器的缩略图（macOS 的访达缩略图扩展用）：长边不超过 `max_side` 像素，不放大；
/// 解不了时为空。不读设置、不用缓存，可以在任何线程调用。
#[uniffi::export]
pub fn make_file_thumbnail(path: String, max_side: u32) -> Option<Arc<ImageHandle>> {
    core::thumbnail::file_thumbnail(std::path::Path::new(&path), max_side)
        .ok()
        .map(|image| {
            Arc::new(ImageHandle {
                inner: Arc::new(image),
            })
        })
}

/// 当前能打开的扩展名（小写，不带点），供打开面板筛选。
#[uniffi::export]
pub fn supported_extensions() -> Vec<String> {
    core::formats::supported_extensions()
        .into_iter()
        .map(String::from)
        .collect()
}

#[uniffi::export]
pub fn core_version() -> String {
    core::version().to_string()
}

#[uniffi::export]
pub fn empty_state_hint(platform: Platform, lang: Lang) -> String {
    core::empty_state_hint(platform.into(), lang.into())
}

/// Windows 版界面启动时调用：这个程序是不是用安装程序装的（它所在的文件夹就是登记在系统里的那个）。
/// 设置窗口按它决定上下文菜单页显不显示、格式关联的按钮能不能用。
#[uniffi::export]
pub fn set_windows_installed(installed: bool) {
    core::settings::set_windows_installed(installed);
}

/// Windows 免安装版：程序所在的文件夹里有 portable.txt 时，设置文件放在旁边的 data 文件夹里（返回它的路径）；
/// 没有时为空（用系统的用户文件夹）。
#[uniffi::export]
pub fn windows_portable_settings_path(program_dir: String) -> Option<String> {
    core::settings::portable_path(std::path::Path::new(&program_dir)).map(|p| p.display().to_string())
}

/// 读设置文件（只读，不改、不备份）：path 为空时用平台的默认位置；没有、读不了时用默认值。
#[cfg(windows)]
fn shell_settings(path: Option<String>) -> core::settings::Settings {
    path.map(std::path::PathBuf::from)
        .or_else(|| core::settings::default_path(core::Platform::Windows))
        .and_then(|path| core::settings::read_file(&path))
        .map(|(settings, _)| settings)
        .unwrap_or_else(core::settings::Settings::defaults)
}

/// Windows：把 Anyview 登记到系统（“打开方式”和默认应用、资源管理器的缩略图和右键菜单、“应用和功能”里的
/// 卸载项），按 settings_path 这个设置文件里的开关。安装程序装完文件后让 Anyview.exe 调它。出错时返回原因。
/// 别的系统上什么也不做。
#[uniffi::export]
pub fn shell_register(
    install_dir: String,
    version: String,
    size_kb: u32,
    settings_path: Option<String>,
    system: Lang,
) -> Option<String> {
    #[cfg(windows)]
    {
        let install = core::winshell::Install {
            dir: install_dir,
            version,
            size_kb,
        };
        shell::register(&install, &shell_settings(settings_path), system.into()).err()
    }
    #[cfg(not(windows))]
    {
        let _ = (install_dir, version, size_kb, settings_path, system);
        None
    }
}

/// Windows：设置变了以后按新的开关重新登记（安装位置、版本用上次登记时记下的）。没有登记过时什么也不做。
#[uniffi::export]
pub fn shell_refresh(settings_path: Option<String>, system: Lang) -> Option<String> {
    #[cfg(windows)]
    {
        let install = shell::installed()?;
        shell::register(&install, &shell_settings(settings_path), system.into()).err()
    }
    #[cfg(not(windows))]
    {
        let _ = (settings_path, system);
        None
    }
}

/// Windows：从系统里撤掉 Anyview 的登记，写在别的程序也用的地方的值恢复原样（卸载时）。
/// stop_hosts：顺带结束正加载着核心 DLL 的缩略图进程（dllhost），好让文件删得掉。
#[uniffi::export]
pub fn shell_unregister(stop_hosts: bool) {
    #[cfg(windows)]
    shell::unregister(stop_hosts);
    #[cfg(not(windows))]
    let _ = stop_hosts;
}

/// Windows：登记在系统里的安装位置；没有登记过时为空。
#[uniffi::export]
pub fn shell_installed_dir() -> Option<String> {
    #[cfg(windows)]
    {
        shell::installed().map(|install| install.dir)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Windows：结束正加载着核心 DLL 的缩略图进程（升级前调用，文件才换得掉）。返回结束了几个。
#[uniffi::export]
pub fn shell_stop_hosts() -> u32 {
    #[cfg(windows)]
    {
        shell::stop_thumbnail_hosts()
    }
    #[cfg(not(windows))]
    {
        0
    }
}

#[uniffi::export]
pub fn lang_from_locale(tag: String) -> Lang {
    core::Lang::from_locale(&tag).into()
}

/// 系统的偏好语言（按先后）里第一种有的界面语言；都没有用英文。
#[uniffi::export]
pub fn lang_from_locales(tags: Vec<String>) -> Lang {
    core::Lang::from_locales(tags.iter().map(String::as_str)).into()
}

/// "关于"里程序名、版本下面的一行："使用 MIT 许可证"。
#[uniffi::export]
pub fn license_line(lang: Lang) -> String {
    core::licenses::about_line(lang.into()).to_string()
}

/// 界面语言的语言标签（BCP 47，如 `ja`、`zh-Hant`）。界面告诉系统文字是哪种语言用：
/// Windows 的 WinUI 按它挑字体（日文用日文字体、繁体用繁体字体）。
#[uniffi::export]
pub fn lang_code(lang: Lang) -> String {
    core::Lang::from(lang).code().to_string()
}

/// 读设置文件的结果。
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum SettingsLoad {
    Loaded,
    /// 还没有设置文件，使用默认值。
    Missing,
    /// 文件坏了，使用默认值；旧文件改名备份为 backup（备份失败时为空）。
    Recovered {
        backup: Option<String>,
        reason: String,
    },
}

impl From<core::settings::LoadStatus> for SettingsLoad {
    fn from(value: core::settings::LoadStatus) -> Self {
        use core::settings::LoadStatus as S;
        match value {
            S::Loaded => Self::Loaded,
            S::Missing => Self::Missing,
            S::Recovered { backup, reason } => Self::Recovered {
                backup: backup.map(|b| b.display().to_string()),
                reason,
            },
        }
    }
}

/// 32 位图片透明处的背景。
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaBackground {
    Black,
    White,
    /// 棋盘格。
    Grid,
}

impl From<core::settings::AlphaBackground> for AlphaBackground {
    fn from(value: core::settings::AlphaBackground) -> Self {
        use core::settings::AlphaBackground as A;
        match value {
            A::Black => Self::Black,
            A::White => Self::White,
            A::Grid => Self::Grid,
        }
    }
}

/// 指针移到控制栏的按钮上时的样子（macOS 26 起）。
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHoverEffect {
    /// 控制栏的玻璃在按钮处鼓起一个圆包，图标浮起一点。
    Bulge,
    /// 一颗透明的玻璃水珠盖在按钮上，图标在里面放大。
    Droplet,
    /// 按钮下面一块淡色的底。
    Highlight,
}

impl From<core::settings::BarHoverEffect> for BarHoverEffect {
    fn from(value: core::settings::BarHoverEffect) -> Self {
        use core::settings::BarHoverEffect as E;
        match value {
            E::Bulge => Self::Bulge,
            E::Droplet => Self::Droplet,
            E::Highlight => Self::Highlight,
        }
    }
}

/// 界面要按设置显示的东西。
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiSettings {
    /// 背景色 0xRRGGBB。
    pub background_color: u32,
    pub alpha_background: AlphaBackground,
    pub hide_control_bar: bool,
    pub hide_side_arrows: bool,
    /// 不显示缩放倍率提示。
    pub hide_zoom_info: bool,
    pub hide_tooltips: bool,
    pub small_info_font: bool,
    /// 通知显示在右上角；关掉时在中央。
    pub corner_notices: bool,
    pub double_click_maximize: bool,
    pub always_on_top: bool,
    /// 不记录最近打开的文件和文件夹。
    pub no_recent: bool,
    /// 一直显示标题栏（macOS）。
    pub show_title_bar: bool,
    /// 不显示动画信息控制。
    pub hide_animation_info: bool,
    /// 看图时阻止屏幕保护程序和显示器睡眠。
    pub no_screensaver: bool,
    /// 拖动窗口时吸附屏幕边缘（macOS）。
    pub snap_to_edges: bool,
    /// 从访达打开图片时交给已经开着的窗口，不开新窗口。
    pub reuse_window: bool,
    /// 标题栏隐藏时在窗口顶端显示文件名（macOS、Windows）。
    pub title_filename: bool,
    /// 不显示窗口顶端的菜单栏（Windows；别的平台总是 true）。
    pub hide_menu_bar: bool,
    /// 控制栏的悬停效果（macOS；别的平台总是淡色亮底）。
    pub bar_hover_effect: BarHoverEffect,
}

impl From<core::UiSettings> for UiSettings {
    fn from(s: core::UiSettings) -> Self {
        Self {
            background_color: s.background_color,
            alpha_background: s.alpha_background.into(),
            hide_control_bar: s.hide_control_bar,
            hide_side_arrows: s.hide_side_arrows,
            hide_zoom_info: s.hide_zoom_info,
            hide_tooltips: s.hide_tooltips,
            small_info_font: s.small_info_font,
            corner_notices: s.corner_notices,
            double_click_maximize: s.double_click_maximize,
            always_on_top: s.always_on_top,
            no_recent: s.no_recent,
            show_title_bar: s.show_title_bar,
            hide_animation_info: s.hide_animation_info,
            no_screensaver: s.no_screensaver,
            snap_to_edges: s.snap_to_edges,
            reuse_window: s.reuse_window,
            title_filename: s.title_filename,
            hide_menu_bar: s.hide_menu_bar,
            bar_hover_effect: s.bar_hover_effect.into(),
        }
    }
}

/// 设置窗口左侧的一个分类。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SettingsPageInfo {
    pub id: String,
    pub title: String,
}

/// 下拉框选项或按钮：ID 和文字。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SettingsOption {
    pub id: String,
    pub title: String,
}

fn options(list: Vec<(String, String)>) -> Vec<SettingsOption> {
    list.into_iter()
        .map(|(id, title)| SettingsOption { id, title })
        .collect()
}

/// 自己设的快捷键的一行（只列用过的行）。
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct KeyMappingRow {
    /// 在映射表里是第几行（从 0 开始），改这一行时传回。
    pub row: u32,
    pub enabled: bool,
    /// 按键的显示文字；还没设时为空。
    pub key_label: String,
    /// 动作 ID；空表示还没选。
    pub action: String,
    /// 提示："将替代：某动作"或"与第 N 行相同"；没有时为空。
    pub note: String,
}

/// 鼠标的按键和滚轮（设置"鼠标"页）。左键的单击、双击另有专门的接口。
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseInput {
    Left,
    Middle,
    Right,
    /// 侧键 1（后退键，"X 键"）。
    Back,
    /// 侧键 2（前进键，"Y 键"）。
    Forward,
    WheelUp,
    WheelDown,
}

impl From<MouseInput> for core::settings::MouseInput {
    fn from(value: MouseInput) -> Self {
        use core::settings::MouseInput as M;
        match value {
            MouseInput::Left => M::Left,
            MouseInput::Middle => M::Middle,
            MouseInput::Right => M::Right,
            MouseInput::Back => M::Back,
            MouseInput::Forward => M::Forward,
            MouseInput::WheelUp => M::WheelUp,
            MouseInput::WheelDown => M::WheelDown,
        }
    }
}

/// 鼠标按键现在做什么。
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum MouseResult {
    /// 没有勾选映射：按默认处理（右键弹出菜单、侧键和滚轮翻页、中键单击不做事）。
    Default,
    /// 执行这个动作。
    Action { id: String },
    /// 勾选了"无操作"：什么也不做。
    Nothing,
}

impl From<core::settings::MouseBinding> for MouseResult {
    fn from(value: core::settings::MouseBinding) -> Self {
        use core::settings::MouseBinding as B;
        match value {
            B::Default => Self::Default,
            B::Action(id) => Self::Action { id: id.to_string() },
            B::Nothing => Self::Nothing,
        }
    }
}

/// 设置项的控件（在这一行的右边；说明文字占整行）。
#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum SettingsControl {
    /// 开关。反着说的项已经翻过来了：界面直接显示 on，改了把开关的状态传回。
    Toggle { on: bool },
    /// 下拉框。
    Choice {
        options: Vec<SettingsOption>,
        selected: String,
    },
    /// 整数输入框加单位。
    Number { text: String, unit: String },
    /// 一排预设色块加"选取…"（打开系统取色器）；颜色为 0xRRGGBB。
    Color {
        value: u32,
        presets: Vec<u32>,
        choose: String,
    },
    /// 路径输入框加按钮。
    Path {
        text: String,
        buttons: Vec<SettingsOption>,
    },
    /// 一排按钮（在这一行的右边），点击即执行。
    Buttons { buttons: Vec<SettingsOption> },
    /// 说明文字（文字就是标签，可能有换行），占卡片里的一整行。
    Note,
    /// 鼠标映射：一个下拉框。首项"默认"（ID 为 "default"），其次"无操作"（ID 为空），其余是动作。
    Mapping {
        selected: String,
        options: Vec<SettingsOption>,
    },
    /// 自己设的快捷键：只列用过的行，下面一个"添加快捷键"。
    KeyMappings {
        rows: Vec<KeyMappingRow>,
        options: Vec<SettingsOption>,
        /// "添加快捷键"用的空行：把它勾选上（KeyRowEnabled）它就出现在列表里。
        add_row: Option<u32>,
    },
}

impl From<core::settings::Control> for SettingsControl {
    fn from(value: core::settings::Control) -> Self {
        use core::settings::Control as C;
        match value {
            C::Toggle(on) => Self::Toggle { on },
            C::Choice {
                options: list,
                selected,
            } => Self::Choice {
                options: options(list),
                selected,
            },
            C::Number { text, unit } => Self::Number { text, unit },
            C::Color {
                value,
                presets,
                choose,
            } => Self::Color {
                value,
                presets,
                choose,
            },
            C::Path { text, buttons } => Self::Path {
                text,
                buttons: options(buttons),
            },
            C::Buttons(buttons) => Self::Buttons {
                buttons: options(buttons),
            },
            C::Note => Self::Note,
            C::Mapping {
                selected,
                options: list,
            } => Self::Mapping {
                selected,
                options: options(list),
            },
            C::KeyMappings {
                rows,
                options: list,
                add_row,
            } => Self::KeyMappings {
                rows: rows
                    .into_iter()
                    .map(|r| KeyMappingRow {
                        row: r.row as u32,
                        enabled: r.enabled,
                        key_label: r.key_label,
                        action: r.action,
                        note: r.note,
                    })
                    .collect(),
                options: options(list),
                add_row: add_row.map(|r| r as u32),
            },
        }
    }
}

/// 设置窗口里的一项：一张卡片里的一行。
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SettingsItem {
    /// 设置项的键（`页.名称`），改草稿、按按钮时传回。
    pub key: String,
    pub label: String,
    /// 标题下面的灰色说明；没有时为空。
    pub detail: String,
    /// 标题后面灰色显示的快捷键；没有时为空。
    pub shortcut: String,
    /// 在第几张卡片里（`SettingsPage::sections` 的下标）。
    pub section: u32,
    /// 缩进层级（从属于上一项的行）。
    pub indent: u8,
    /// 可以改（已经实现，依赖的开关也满足）；否则灰显。
    pub enabled: bool,
    /// 开关是翻过来说的（设置文件里存的是"不显示……"）。只给自检把存的值换成开关的状态。
    pub inverted: bool,
    pub control: SettingsControl,
}

/// 设置窗口的一页。
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SettingsPage {
    pub id: String,
    /// 左侧列表里的名称，也是页顶上的大标题。
    pub title: String,
    /// 大标题下面的一段说明（键盘、鼠标页）；没有时为空。
    pub lead: String,
    /// 各张卡片上面的组名（没有组名的卡片为空）。
    pub sections: Vec<String>,
    pub items: Vec<SettingsItem>,
}

impl From<core::SettingsPageView> for SettingsPage {
    fn from(page: core::SettingsPageView) -> Self {
        Self {
            id: page.id.to_string(),
            title: page.title.to_string(),
            lead: page.lead.to_string(),
            sections: page.sections,
            items: page
                .items
                .into_iter()
                .map(|item| SettingsItem {
                    key: item.key.to_string(),
                    label: item.label,
                    detail: item.detail,
                    shortcut: item.shortcut,
                    section: item.section as u32,
                    indent: item.indent,
                    enabled: item.enabled,
                    inverted: item.inverted,
                    control: item.control.into(),
                })
                .collect(),
        }
    }
}

/// 界面传来的修改。
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum SettingsInput {
    /// 开关的状态（翻过来说的项也直接传开关的状态）。
    Bool {
        value: bool,
    },
    Choice {
        id: String,
    },
    /// 数字、路径输入框里的文字。
    Text {
        text: String,
    },
    /// 0xRRGGBB。
    Color {
        value: u32,
    },
    /// 鼠标映射的下拉框选了哪一项（"default"、空或动作 ID）。
    MappingChoice {
        id: String,
    },
    /// 键盘映射表第 row 行（从 0 开始）的勾选。
    KeyRowEnabled {
        row: u32,
        value: bool,
    },
    /// 第 row 行录入的按键：名称同 action_for_key 的 key，加上修饰键；名称为空表示清除。
    KeyRowKey {
        row: u32,
        key: String,
        modifiers: KeyModifiers,
    },
    /// 第 row 行选的动作；空表示还没选。
    KeyRowAction {
        row: u32,
        id: String,
    },
    /// 删掉第 row 行。
    KeyRowRemove {
        row: u32,
    },
}

impl From<SettingsInput> for core::settings::Input {
    fn from(value: SettingsInput) -> Self {
        use core::settings::Input as I;
        match value {
            SettingsInput::Bool { value } => I::Bool(value),
            SettingsInput::Choice { id } => I::Choice(id),
            SettingsInput::Text { text } => I::Text(text),
            SettingsInput::Color { value } => I::Color(value),
            SettingsInput::MappingChoice { id } => I::MappingChoice(id),
            SettingsInput::KeyRowEnabled { row, value } => I::KeyRowEnabled {
                row: row as usize,
                on: value,
            },
            SettingsInput::KeyRowKey {
                row,
                key,
                modifiers,
            } => I::KeyRowKey {
                row: row as usize,
                key,
                mods: modifiers.bits(),
            },
            SettingsInput::KeyRowAction { row, id } => I::KeyRowAction {
                row: row as usize,
                action: id,
            },
            SettingsInput::KeyRowRemove { row } => I::KeyRowRemove { row: row as usize },
        }
    }
}

/// "确定"的结果。
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum SettingsApplyResult {
    /// 已生效。save_error：写文件失败的原因（设置已经生效，下次启动不会保留）。
    Applied {
        changed: Vec<String>,
        save_error: Option<String>,
    },
    /// 校验失败：切到 page，标出 key（映射表还有 row 行，从 0 开始），显示 message。
    Invalid {
        page: String,
        key: String,
        row: Option<u32>,
        message: String,
    },
}

/// 设置窗口里的按钮要界面做的事。
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum SettingsCommand {
    /// 设为支持的图片类型的默认程序。
    SetDefaultViewer,
    /// 在访达或文件管理器中打开文件夹。
    OpenFolder { path: String },
    /// 选文件夹，选好后用 settings_set 写进 key。
    ChooseFolder { key: String },
    /// 选文件，选好后写进 key。
    ChooseFile { key: String },
    /// 打开系统取色器，选好后写进 key。
    ChooseColor { key: String },
    /// 选程序（图像编辑器），选好后写进 key。
    ChooseApp { key: String },
    /// 核心已经改了草稿：重新显示这一页。
    Refresh,
    /// 先确认，再调用 clear_archive_positions。
    ConfirmClearArchivePositions,
    /// 换回设为默认之前的默认程序（只在 Linux 的设置窗口里有，macOS、Windows 不会收到）。
    RestoreDefaultViewer,
    /// 登记、取消登记文件管理器的缩略图生成器（只在 Linux 的 Flatpak 版里有，macOS、Windows 不会收到）。
    RegisterThumbnailer,
    UnregisterThumbnailer,
    /// Windows 的“格式关联”：打开系统设置的“默认应用”。只有 Windows 会收到。
    OpenDefaultApps,
    /// Windows 的“经典方法”：打开系统里按程序逐个扩展名选默认程序的那一页。只有 Windows 会收到。
    OpenAssociationsByApp,
}

impl From<core::settings::ButtonCommand> for SettingsCommand {
    fn from(value: core::settings::ButtonCommand) -> Self {
        use core::settings::ButtonCommand as B;
        match value {
            B::SetDefaultViewer => Self::SetDefaultViewer,
            B::OpenFolder { path } => Self::OpenFolder { path },
            B::ChooseFolder { key } => Self::ChooseFolder { key },
            B::ChooseFile { key } => Self::ChooseFile { key },
            B::ChooseColor { key } => Self::ChooseColor { key },
            B::ChooseApp { key } => Self::ChooseApp { key },
            B::Refresh => Self::Refresh,
            B::ConfirmClearArchivePositions => Self::ConfirmClearArchivePositions,
            B::RestoreDefaultViewer => Self::RestoreDefaultViewer,
            B::RegisterThumbnailer => Self::RegisterThumbnailer,
            B::UnregisterThumbnailer => Self::UnregisterThumbnailer,
            B::OpenDefaultApps => Self::OpenDefaultApps,
            B::OpenAssociationsByApp => Self::OpenAssociationsByApp,
        }
    }
}

/// 界面文本键，对应 `anyview_core::i18n::Key`。
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKey {
    AppName,
    EmptyStateHint,
    CoreLinkOk,
    NoticeAtFirst,
    NoticeAtLast,
    NoticeEmptyFolder,
    NoticeOpenFailed,
    NoticeImageFailed,
    OpenDialogTitle,
    ImageFilterName,
    InfoName,
    InfoSize,
    InfoModified,
    InfoImage,
    InfoClose,
    InfoCopy,
    BarLock,
    BarPrevTip,
    BarNextTip,
    BarScanning,
    NoticeLastFolder,
    NoticeFirstFolder,
    NoticeNoRandomBack,
    NoticeTrashFailed,
    NoticeCopied,
    NoticeSettingsSaveFailed,
    SettingsTitle,
    SettingsReset,
    SettingsOk,
    SettingsDiscardMessage,
    SettingsDiscard,
    SettingsKeepEditing,
    SettingsDefaultViewerDone,
    SettingsDefaultViewerFailed,
    SettingsPressKey,
    SettingsKeyReserved,
    SettingsAddShortcut,
    SettingsKeyUnset,
    SettingsRemoveShortcut,
    NoticeArchivePassword,
    NoticeEmptyArchive,
    NoticeCopiedTo,
    NoticeMoved,
    NoticeFolderNotSet,
    NoticeFileFailed,
    NoticeWallpaperSet,
    NoticeEditorMissing,
    NoticeEditorFailed,
    PromptClearPositions,
    ButtonDelete,
    ButtonCancel,
    PickerTitle,
    PickerColumnName,
    PickerColumnModified,
    PickerColumnSize,
    PickerHideThumbnails,
    PickerOk,
}

impl From<TextKey> for core::i18n::Key {
    fn from(value: TextKey) -> Self {
        match value {
            TextKey::AppName => core::i18n::Key::AppName,
            TextKey::EmptyStateHint => core::i18n::Key::EmptyStateHint,
            TextKey::CoreLinkOk => core::i18n::Key::CoreLinkOk,
            TextKey::NoticeAtFirst => core::i18n::Key::NoticeAtFirst,
            TextKey::NoticeAtLast => core::i18n::Key::NoticeAtLast,
            TextKey::NoticeEmptyFolder => core::i18n::Key::NoticeEmptyFolder,
            TextKey::NoticeOpenFailed => core::i18n::Key::NoticeOpenFailed,
            TextKey::NoticeImageFailed => core::i18n::Key::NoticeImageFailed,
            TextKey::OpenDialogTitle => core::i18n::Key::OpenDialogTitle,
            TextKey::ImageFilterName => core::i18n::Key::ImageFilterName,
            TextKey::InfoName => core::i18n::Key::InfoName,
            TextKey::InfoSize => core::i18n::Key::InfoSize,
            TextKey::InfoModified => core::i18n::Key::InfoModified,
            TextKey::InfoImage => core::i18n::Key::InfoImage,
            TextKey::InfoClose => core::i18n::Key::InfoClose,
            TextKey::InfoCopy => core::i18n::Key::InfoCopy,
            TextKey::BarLock => core::i18n::Key::BarLock,
            TextKey::BarPrevTip => core::i18n::Key::BarPrevTip,
            TextKey::BarNextTip => core::i18n::Key::BarNextTip,
            TextKey::BarScanning => core::i18n::Key::BarScanning,
            TextKey::NoticeLastFolder => core::i18n::Key::NoticeLastFolder,
            TextKey::NoticeFirstFolder => core::i18n::Key::NoticeFirstFolder,
            TextKey::NoticeNoRandomBack => core::i18n::Key::NoticeNoRandomBack,
            TextKey::NoticeTrashFailed => core::i18n::Key::NoticeTrashFailed,
            TextKey::NoticeCopied => core::i18n::Key::NoticeCopied,
            TextKey::NoticeSettingsSaveFailed => core::i18n::Key::NoticeSettingsSaveFailed,
            TextKey::SettingsTitle => core::i18n::Key::SettingsTitle,
            TextKey::SettingsReset => core::i18n::Key::SettingsReset,
            TextKey::SettingsOk => core::i18n::Key::SettingsOk,
            TextKey::SettingsDiscardMessage => core::i18n::Key::SettingsDiscardMessage,
            TextKey::SettingsDiscard => core::i18n::Key::SettingsDiscard,
            TextKey::SettingsKeepEditing => core::i18n::Key::SettingsKeepEditing,
            TextKey::SettingsDefaultViewerDone => core::i18n::Key::SettingsDefaultViewerDone,
            TextKey::SettingsDefaultViewerFailed => core::i18n::Key::SettingsDefaultViewerFailed,
            TextKey::SettingsPressKey => core::i18n::Key::SettingsPressKey,
            TextKey::SettingsKeyReserved => core::i18n::Key::SettingsKeyReserved,
            TextKey::SettingsAddShortcut => core::i18n::Key::SettingsAddShortcut,
            TextKey::SettingsKeyUnset => core::i18n::Key::SettingsKeyUnset,
            TextKey::SettingsRemoveShortcut => core::i18n::Key::SettingsRemoveShortcut,
            TextKey::NoticeArchivePassword => core::i18n::Key::NoticeArchivePassword,
            TextKey::NoticeEmptyArchive => core::i18n::Key::NoticeEmptyArchive,
            TextKey::NoticeCopiedTo => core::i18n::Key::NoticeCopiedTo,
            TextKey::NoticeMoved => core::i18n::Key::NoticeMoved,
            TextKey::NoticeFolderNotSet => core::i18n::Key::NoticeFolderNotSet,
            TextKey::NoticeFileFailed => core::i18n::Key::NoticeFileFailed,
            TextKey::NoticeWallpaperSet => core::i18n::Key::NoticeWallpaperSet,
            TextKey::NoticeEditorMissing => core::i18n::Key::NoticeEditorMissing,
            TextKey::NoticeEditorFailed => core::i18n::Key::NoticeEditorFailed,
            TextKey::PromptClearPositions => core::i18n::Key::PromptClearPositions,
            TextKey::ButtonDelete => core::i18n::Key::ButtonDelete,
            TextKey::ButtonCancel => core::i18n::Key::ButtonCancel,
            TextKey::PickerTitle => core::i18n::Key::PickerTitle,
            TextKey::PickerColumnName => core::i18n::Key::PickerColumnName,
            TextKey::PickerColumnModified => core::i18n::Key::PickerColumnModified,
            TextKey::PickerColumnSize => core::i18n::Key::PickerColumnSize,
            TextKey::PickerHideThumbnails => core::i18n::Key::PickerHideThumbnails,
            TextKey::PickerOk => core::i18n::Key::PickerOk,
        }
    }
}

/// 取界面文本。占位符（如 `{ms}`）由调用方替换。
#[uniffi::export]
pub fn ui_text(key: TextKey, lang: Lang) -> String {
    core::i18n::text(key.into(), lang.into()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    struct TestListener(Mutex<mpsc::Sender<()>>);

    impl CoreListener for TestListener {
        fn wake(&self) {
            let _ = self.0.lock().unwrap().send(());
        }
    }

    fn app() -> (Arc<AppCore>, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        (AppCore::new(Arc::new(TestListener(Mutex::new(tx)))), rx)
    }

    #[test]
    fn settings_flow_through_ffi_types() {
        let dir = std::env::temp_dir().join(format!("anyview-ffi-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("settings.toml");
        let (app, _rx) = app();
        assert_eq!(
            app.load_settings(Some(file.display().to_string())),
            SettingsLoad::Missing
        );
        let pages = app.settings_pages(Lang::ZhHans);
        assert_eq!(pages[0].title, "一般");
        assert!(pages.iter().all(|p| p.id != "context_menu"));
        app.settings_begin();
        let browse = app.settings_page("browse".into(), Lang::ZhHans).unwrap();
        assert_eq!(browse.title, "浏览");
        assert_eq!(browse.sections, ["窗口", "画面上显示", "背景"]);
        let bar = browse
            .items
            .iter()
            .find(|i| i.key == "browse.hide_control_bar")
            .unwrap();
        assert_eq!((bar.label.as_str(), bar.inverted), ("控制栏", true));
        assert_eq!(bar.control, SettingsControl::Toggle { on: true });
        let color = browse
            .items
            .iter()
            .find(|i| i.key == "browse.background_color")
            .unwrap();
        assert!(matches!(
            &color.control,
            SettingsControl::Color { value: 0x202020, presets, .. } if presets.len() == 15
        ));
        assert!(app.settings_set(
            "browse.background_color".into(),
            SettingsInput::Color { value: 0x336699 }
        ));
        assert!(app.settings_set(
            "image.cache_mb".into(),
            SettingsInput::Text { text: "abc".into() }
        ));
        assert!(app.settings_dirty());
        match app.settings_apply(Lang::ZhHans) {
            SettingsApplyResult::Invalid {
                page, key, message, ..
            } => {
                assert_eq!((page.as_str(), key.as_str()), ("image", "image.cache_mb"));
                assert!(message.contains("100"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        app.settings_reset_page("image".into());
        match app.settings_apply(Lang::ZhHans) {
            SettingsApplyResult::Applied {
                changed,
                save_error,
            } => {
                assert_eq!(changed, ["browse.background_color"]);
                assert_eq!(save_error, None);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(app.ui_settings().background_color, 0x336699);
        assert_eq!(
            app.setting_text("browse.background_color".into())
                .as_deref(),
            Some("#336699")
        );
        assert!(std::fs::read_to_string(&file).unwrap().contains("#336699"));
        assert_eq!(app.ui_lang(Lang::En), Lang::En);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ping_pong_through_ffi_types() {
        let (app, rx) = app();
        app.dispatch(Command::Ping { token: 7 });
        rx.recv_timeout(Duration::from_secs(5)).expect("no wake");
        let events = app.drain_events();
        assert!(
            matches!(events.as_slice(), [Event::Pong { token: 7 }]),
            "{events:?}"
        );
    }

    #[test]
    fn image_handle_exposes_stable_pixels() {
        let dir = std::env::temp_dir().join(format!("anyview-ffi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("one.png");
        image::RgbaImage::from_pixel(5, 3, image::Rgba([255, 0, 0, 255]))
            .save(&file)
            .unwrap();

        let (app, rx) = app();
        app.dispatch(Command::Open {
            path: file.display().to_string(),
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let image = loop {
            let _ = rx.recv_timeout(Duration::from_millis(200));
            if let Some(image) = app.drain_events().into_iter().find_map(|e| match e {
                Event::ImageReady { image, .. } => Some(image),
                _ => None,
            }) {
                break image;
            }
            assert!(Instant::now() < deadline, "没有收到图像");
        };
        assert_eq!(
            (
                image.width(),
                image.height(),
                image.stride(),
                image.byte_len()
            ),
            (5, 3, 20, 60)
        );
        assert_eq!(image.format_name(), "PNG");
        // 地址指向的就是 BGRA 数据：红色 → [0, 0, 255, 255]。
        let bytes = unsafe { std::slice::from_raw_parts(image.pixel_address() as *const u8, 4) };
        assert_eq!(bytes, &[0, 0, 255, 255]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn menus_and_actions_cross_the_ffi() {
        let bar = menu_entries(MenuKind::MenuBar, Platform::Macos, Lang::ZhHans);
        assert!(matches!(
            bar.first(),
            Some(MenuEntry::SubmenuStart {
                role: MenuRole::App,
                ..
            })
        ));
        let open_folder = bar.iter().find_map(|e| match e {
            MenuEntry::Action { id, shortcut, .. } if id == "open_folder" => shortcut.clone(),
            _ => None,
        });
        assert_eq!(
            open_folder,
            Some(Shortcut {
                key: "O".into(),
                modifiers: KeyModifiers {
                    shift: true,
                    command: true,
                    ..Default::default()
                },
                label: "⇧⌘O".into(),
            })
        );
        let (app, _rx) = app();
        assert_eq!(
            app.invoke_action("open_file".into()),
            ActionOutcome::Ui {
                request: UiRequest::OpenFileDialog
            }
        );
        assert!(app.action_state("fit_window".into()).checked);
        assert_eq!(app.view_layout(), None);
        assert_eq!(zoom_text(1.5), "150%");
        assert_eq!(
            app.invoke_action("toggle_info".into()),
            ActionOutcome::OverlayChanged
        );
        assert_eq!(hover_zone(10.0, 300.0, 900.0, 600.0), HoverZone::Left);
        assert_eq!(progress_index(0.5, 9), Some(4));
        assert_eq!(position_text(840, 1862), "841/1862");
        assert_eq!(
            action_tooltip("open_file".into(), Platform::Macos, Lang::ZhHans),
            "打开图片 (⌘O)"
        );
        assert_eq!(overlay_timing().fade_in_ms, 150);
        assert_eq!(app.view_swipe(SwipePhase::Ended, 0.0, 0.0), None);
    }

    #[test]
    fn licenses_cross_the_ffi() {
        let (app, _rx) = app();
        assert_eq!(app.help_title(HelpKind::Licenses, Lang::ZhHans), "开源许可");
        assert!(app.help_rows(HelpKind::Licenses, Lang::En).is_empty());
        assert_eq!(app.help_text(HelpKind::Shortcuts, Lang::En), None);
        let text = app.help_text(HelpKind::Licenses, Lang::En).unwrap();
        assert!(text.contains("MIT License") && text.contains("third-party software notices"));
        assert_eq!(
            app.invoke_action("help_licenses".into()),
            ActionOutcome::Ui {
                request: UiRequest::ShowHelp {
                    kind: HelpKind::Licenses
                }
            }
        );
    }

    #[test]
    fn helpers() {
        assert!(!core_version().is_empty());
        assert!(empty_state_hint(Platform::Macos, Lang::ZhHans).contains("⌘O"));
        assert_eq!(lang_from_locale("zh-Hans-CN".into()), Lang::ZhHans);
        assert_eq!(lang_code(Lang::Ja), "ja");
        assert_eq!(license_line(Lang::En), "Released under the MIT License");
        assert_eq!(lang_code(Lang::ZhHant), "zh-Hant");
        assert_eq!(lang_from_locale(lang_code(Lang::PtBr)), Lang::PtBr);
        assert_eq!(ui_text(TextKey::AppName, Lang::ZhHans), "Anyview");
        assert!(supported_extensions().iter().any(|e| e == "jpg"));
        assert_eq!(
            fit_rect(4000, 3000, 1800.0, 1200.0),
            FitRect {
                x: 100.0,
                y: 0.0,
                width: 1600.0,
                height: 1200.0
            }
        );
    }
}
