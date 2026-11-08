//! 查看计算（同步、纯计算）。坐标和尺寸都用屏幕物理像素，原点在视口左上角。
//! 倍率的 100%是一个图片像素对应一个点（逻辑像素）：高分屏（如 Retina 的 2 倍）上，100% 时一个图片像素
//! 占 2×2 个物理像素，图片的实际大小与系统里其他程序一致。`Layout::scale` 是物理倍率（物理像素 / 图片像素），
//! `Layout::zoom` 是显示给用户的倍率（100% = 1）。
//!
//! 规则：显示方式适配；缩放档位、中心、100% 与缩放锁定；旋转为相对操作、菜单显示绝对角度；
//! 切图后旋转、翻转复位，锁定时保持倍率。

/// 放大、缩小时跳转的倍率档位，按用户看到的倍率（100% = 1）。
pub const ZOOM_STEPS: &[f64] = &[
    0.01, 0.02, 0.03, 0.05, 0.08, 0.10, 0.15, 0.20, 0.25, 0.33, 0.50, 0.67, 1.0, 1.5, 2.0, 3.0,
    4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0,
];
/// 用户看到的倍率的上下限（1% 到 3200%）。
pub const MIN_SCALE: f64 = 0.01;
pub const MAX_SCALE: f64 = 32.0;
/// 上下左右平移一次移动视口尺寸的比例。
const PAN_STEP: f64 = 0.15;
/// 双指轻扫翻页：手势结束时横向累计位移至少为视口宽度的这个比例。
const SWIPE_FRACTION: f64 = 0.08;

/// 触控板手势的阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwipePhase {
    Began,
    Changed,
    Ended,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// "适合窗口"的位置：大图等比缩小到完整显示，小图按原始大小居中。结果取整到整像素。
pub fn fit_rect(image_width: u32, image_height: u32, view_width: f64, view_height: f64) -> Rect {
    if image_width == 0 || image_height == 0 || view_width <= 0.0 || view_height <= 0.0 {
        return Rect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
    }
    let (iw, ih) = (f64::from(image_width), f64::from(image_height));
    let scale = (view_width / iw).min(view_height / ih).min(1.0);
    let width = (iw * scale).round().max(1.0);
    let height = (ih * scale).round().max(1.0);
    Rect {
        x: ((view_width - width) / 2.0).round(),
        y: ((view_height - height) / 2.0).round(),
        width,
        height,
    }
}

/// 缩放倍率提示的文字，如 `150%`、`33%`；小于 10% 时保留一位小数。
pub fn zoom_text(scale: f64) -> String {
    let percent = (scale * 1000.0).round() / 10.0;
    if percent >= 10.0 || percent.fract() == 0.0 {
        format!("{}%", percent.round())
    } else {
        format!("{percent:.1}%")
    }
}

/// 显示方式：单选，切图后继续生效。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayMode {
    FitWindow,
    Original,
    FitWidth,
}

/// 窗口内容区的最小尺寸（点）：很小的图片按原始大小居中，四周是背景色。
pub const MIN_WINDOW: (f64, f64) = (480.0, 360.0);

/// 窗口跟着图片：窗口内容区应有的大小（点，取整）。
/// 图片的原始大小（100%）就是一个图片像素一个点；适合窗口时，放不下就按比例缩到可用区域以内，
/// 窗口与图片同比例、不留边；适合宽度时宽度不超过可用区域；原始大小时宽、高分别不超过可用区域。
/// 结果不小于最小尺寸，也不超过可用区域。
pub fn window_size_for_image(
    image: (u32, u32),
    mode: DisplayMode,
    available: (f64, f64),
) -> (f64, f64) {
    let (aw, ah) = (available.0.max(MIN_WINDOW.0), available.1.max(MIN_WINDOW.1));
    let (w, h) = (f64::from(image.0), f64::from(image.1));
    if w <= 0.0 || h <= 0.0 {
        return MIN_WINDOW;
    }
    let (fw, fh) = match mode {
        DisplayMode::FitWindow => {
            let s = (aw / w).min(ah / h).min(1.0);
            (w * s, h * s)
        }
        DisplayMode::FitWidth => {
            let s = (aw / w).min(1.0);
            (w * s, h * s)
        }
        DisplayMode::Original => (w, h),
    };
    (
        fw.round().clamp(MIN_WINDOW.0, aw),
        fh.round().clamp(MIN_WINDOW.1, ah),
    )
}

/// 吸附边缘：窗口的边离屏幕可用区域的边不超过这么多点时贴上去。
pub const SNAP_DISTANCE: f64 = 12.0;

/// 拖动窗口时吸附屏幕边缘：窗口的某条边离可用区域（去掉菜单栏、程序坞、任务栏）对应的边
/// 不超过 [`SNAP_DISTANCE`] 时，把窗口挪到正好贴齐。左右、上下分别判断；窗口比可用区域还宽（高）时
/// 这个方向不动。`frame`、`area` 是 (x, y, 宽, 高)，y 朝哪个方向都可以（macOS 向上、Windows 向下）。
/// 返回窗口新的原点。
pub fn snap_to_edges(frame: (f64, f64, f64, f64), area: (f64, f64, f64, f64)) -> (f64, f64) {
    fn axis(pos: f64, len: f64, start: f64, area_len: f64) -> f64 {
        if len > area_len {
            return pos;
        }
        let end = start + area_len;
        if (pos - start).abs() <= SNAP_DISTANCE {
            start
        } else if (pos + len - end).abs() <= SNAP_DISTANCE {
            end - len
        } else {
            pos
        }
    }
    (
        axis(frame.0, frame.2, area.0, area.2),
        axis(frame.1, frame.3, area.1, area.3),
    )
}

/// 像素边缘处理：无 = 最近邻；减淡 = 按插值算法平滑；减淡+锐化 = 平滑后再锐化。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdgeMode {
    None,
    Soften,
    SoftenSharpen,
}

/// 插值算法：Bilinear 只走 GPU；Bicubic、Lanczos 在静止后由精确重绘负责。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Interpolation {
    Bilinear,
    Bicubic,
    Lanczos,
}

/// 界面绘制所需的全部参数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// 图片中心在视口中的位置。
    pub center_x: f64,
    pub center_y: f64,
    /// 旋转前的显示尺寸（图片宽高乘以倍率）。
    pub width: f64,
    pub height: f64,
    /// 一个图片像素对应的物理像素数（物理倍率）。
    pub scale: f64,
    /// 用户看到的倍率（100% = 1）：物理倍率除以一个点对应的物理像素数。
    pub zoom: f64,
    /// 顺时针角度：0、90、180、270。先旋转，再按屏幕方向翻转。
    pub rotation: u16,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
    /// 平滑显示（双线性、缩小时三线性）；为假时用最近邻（像素边缘处理"无"）。
    pub smooth: bool,
}

impl Layout {
    /// 图片像素坐标 → 视口物理像素坐标。
    pub fn image_to_screen(
        &self,
        image_width: u32,
        image_height: u32,
        u: f64,
        v: f64,
    ) -> (f64, f64) {
        let mut x = (u - f64::from(image_width) / 2.0) * self.scale;
        let mut y = (v - f64::from(image_height) / 2.0) * self.scale;
        // 屏幕坐标 y 向下，顺时针旋转 90° 时 (x, y) → (-y, x)。
        for _ in 0..self.rotation / 90 {
            (x, y) = (-y, x);
        }
        if self.flip_horizontal {
            x = -x;
        }
        if self.flip_vertical {
            y = -y;
        }
        (x + self.center_x, y + self.center_y)
    }

    /// 视口物理像素坐标 → 图片像素坐标（可能在图片范围外）。
    pub fn screen_to_image(
        &self,
        image_width: u32,
        image_height: u32,
        x: f64,
        y: f64,
    ) -> (f64, f64) {
        let (mut x, mut y) = (x - self.center_x, y - self.center_y);
        if self.flip_horizontal {
            x = -x;
        }
        if self.flip_vertical {
            y = -y;
        }
        for _ in 0..self.rotation / 90 {
            (x, y) = (y, -x);
        }
        (
            x / self.scale + f64::from(image_width) / 2.0,
            y / self.scale + f64::from(image_height) / 2.0,
        )
    }
}

#[derive(Clone, Debug)]
pub struct ViewState {
    pub mode: DisplayMode,
    pub edge: EdgeMode,
    pub interpolation: Interpolation,
    /// 拉伸小图（默认关）。
    pub stretch_small: bool,
    /// 缩放锁定。
    pub zoom_lock: bool,
    /// 图片大于窗口时显示图片顶部（默认开）。
    pub top_first: bool,
    rotation: u16,
    flip_horizontal: bool,
    flip_vertical: bool,
    image: Option<(u32, u32)>,
    viewport: (f64, f64),
    scale: f64,
    /// 图片中心相对视口中心的偏移。
    offset: (f64, f64),
    /// 用户缩放过：倍率不再跟随显示方式。
    custom_scale: bool,
    /// 左上角图片信息是否显示（默认关）。
    pub info_visible: bool,
    /// 控制栏锁定常驻。
    pub bar_locked: bool,
    /// 正在进行的双指滑动累计的位移。
    swipe: (f64, f64),
    /// 一个点对应的物理像素数（Retina 为 2）。100% 的物理倍率就是它。
    ppp: f64,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            mode: DisplayMode::FitWindow,
            edge: EdgeMode::Soften,
            interpolation: Interpolation::Bilinear,
            stretch_small: false,
            zoom_lock: false,
            top_first: true,
            rotation: 0,
            flip_horizontal: false,
            flip_vertical: false,
            image: None,
            viewport: (0.0, 0.0),
            scale: 1.0,
            offset: (0.0, 0.0),
            custom_scale: false,
            info_visible: false,
            bar_locked: false,
            swipe: (0.0, 0.0),
            ppp: 1.0,
        }
    }
}

impl ViewState {
    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    pub fn flip_horizontal(&self) -> bool {
        self.flip_horizontal
    }

    pub fn flip_vertical(&self) -> bool {
        self.flip_vertical
    }

    pub fn has_image(&self) -> bool {
        self.image.is_some()
    }

    pub fn layout(&self) -> Option<Layout> {
        let (w, h) = self.image?;
        let (vw, vh) = self.viewport;
        let width = f64::from(w) * self.scale;
        let height = f64::from(h) * self.scale;
        let (dw, dh) = self.displayed_size();
        // 让显示区域的左上角落在整像素上，100% 时像素对齐、不发虚。
        let left = (vw / 2.0 + self.offset.0 - dw / 2.0).round();
        let top = (vh / 2.0 + self.offset.1 - dh / 2.0).round();
        Some(Layout {
            center_x: left + dw / 2.0,
            center_y: top + dh / 2.0,
            width,
            height,
            scale: self.scale,
            zoom: self.scale / self.ppp,
            rotation: self.rotation,
            flip_horizontal: self.flip_horizontal,
            flip_vertical: self.flip_vertical,
            smooth: self.edge != EdgeMode::None,
        })
    }

    /// 当前图片的像素尺寸。
    /// 窗口应有的大小（点）。`image` 是要显示的图片，`rotated` 为真时宽高互换（转了 90° 或 270°）。
    /// 锁定缩放时按锁定的倍率算，各边截到可用区域以内；否则按显示方式（[`window_size_for_image`]）。
    pub fn window_size_for(
        &self,
        image: (u32, u32),
        rotated: bool,
        available: (f64, f64),
    ) -> (f64, f64) {
        let image = if rotated { (image.1, image.0) } else { image };
        if !self.zoom_lock {
            return window_size_for_image(image, self.mode, available);
        }
        let (aw, ah) = (available.0.max(MIN_WINDOW.0), available.1.max(MIN_WINDOW.1));
        let zoom = self.scale / self.ppp;
        let width = f64::from(image.0) * zoom;
        let height = f64::from(image.1) * zoom;
        (
            width.round().clamp(MIN_WINDOW.0, aw),
            height.round().clamp(MIN_WINDOW.1, ah),
        )
    }

    /// 当前图片按现在的方向（旋转后）应有的窗口大小；没有图片时为空。
    pub fn window_size_for_current(&self, available: (f64, f64)) -> Option<(f64, f64)> {
        let image = self.image?;
        Some(self.window_size_for(image, self.rotation % 180 == 90, available))
    }

    /// 图片（旋转后、按现在的倍率）超出画布：至少一个方向比画布大（超过半个像素）。没有图片时为假。
    pub fn image_exceeds_viewport(&self) -> bool {
        if self.image.is_none() {
            return false;
        }
        let (dw, dh) = self.displayed_size();
        let (vw, vh) = self.viewport;
        dw > vw + 0.5 || dh > vh + 0.5
    }

    /// 一个点对应的物理像素数（界面从窗口所在的屏幕取，Retina 为 2）。
    pub fn pixels_per_point(&self) -> f64 {
        self.ppp
    }

    /// 窗口换到了缩放比例不同的屏幕上（或第一次知道比例）：100% 的物理倍率随之改变。
    /// 手动缩放过的保持用户看到的倍率不变；否则按显示方式重新计算。
    pub fn set_pixels_per_point(&mut self, ppp: f64) -> Option<Layout> {
        if !ppp.is_finite() || ppp <= 0.0 || (ppp - self.ppp).abs() < 1e-9 {
            return self.layout();
        }
        if self.custom_scale {
            self.scale *= ppp / self.ppp;
        }
        self.ppp = ppp;
        self.refit()
    }

    pub fn image_size(&self) -> Option<(u32, u32)> {
        self.image
    }

    /// 是否需要精确重绘（见 docs/ARCHITECTURE.md）：像素边缘为"减淡"且插值为 Bicubic 或 Lanczos，
    /// 或者为"减淡+锐化"时需要；"无"（最近邻）和 100%（像素一一对应）时 GPU 显示就是准确的。
    pub fn needs_exact(&self) -> bool {
        self.image.is_some()
            && self.edge != EdgeMode::None
            && (self.interpolation != Interpolation::Bilinear
                || self.edge == EdgeMode::SoftenSharpen)
            && (self.scale - 1.0).abs() > 1e-9
    }

    /// 同上，另外考虑超大图的代理纹理：`proxy_ratio` 是代理与原图的尺寸比。
    /// 倍率大于这个比例时代理被放大显示、不够清晰，总要从原图精确重绘（像素边缘为"无"时取最近的像素）。
    pub fn needs_exact_with(&self, proxy_ratio: Option<f64>) -> bool {
        self.needs_exact()
            || (self.image.is_some() && proxy_ratio.is_some_and(|r| self.scale > r + 1e-9))
    }

    /// 画面的签名：位置、倍率、方向、视口、像素边缘和插值都相同时相同。
    /// 精确重绘的结果只在签名不变时有效，交互一开始签名就变了，界面据此立即撤下覆盖层。
    pub fn signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        if let Some(layout) = self.layout() {
            for value in [
                layout.center_x,
                layout.center_y,
                layout.width,
                layout.height,
                layout.scale,
            ] {
                value.to_bits().hash(&mut hasher);
            }
            (
                layout.rotation,
                layout.flip_horizontal,
                layout.flip_vertical,
            )
                .hash(&mut hasher);
        }
        self.viewport.0.to_bits().hash(&mut hasher);
        self.viewport.1.to_bits().hash(&mut hasher);
        (self.image, self.edge, self.interpolation).hash(&mut hasher);
        hasher.finish()
    }

    /// 视口大小（物理像素）。
    pub fn viewport(&self) -> (f64, f64) {
        self.viewport
    }

    /// 当前倍率是否就是显示方式算出的倍率（没有手动缩放过）。
    pub fn follows_mode(&self) -> bool {
        !self.custom_scale
    }

    pub fn set_viewport(&mut self, width: f64, height: f64) -> Option<Layout> {
        let had_size = self.viewport.0 > 0.0 && self.viewport.1 > 0.0;
        self.viewport = (width.max(0.0), height.max(0.0));
        if !self.custom_scale {
            self.scale = self.mode_scale();
        }
        // 画布还没有尺寸时就开始显示了图片（启动时提前打开）：拿到第一个尺寸后再按新图片摆放。
        if !had_size && self.image.is_some() {
            self.reset_position();
        } else {
            self.clamp_offset();
        }
        self.layout()
    }

    /// 界面开始显示一张新图片：旋转、翻转复位；锁定缩放时保持倍率，否则按显示方式重新计算。
    pub fn set_image(&mut self, width: u32, height: u32) -> Option<Layout> {
        self.image = Some((width, height));
        self.rotation = 0;
        self.flip_horizontal = false;
        self.flip_vertical = false;
        if self.zoom_lock {
            self.custom_scale = true;
        } else {
            self.custom_scale = false;
            self.scale = self.mode_scale();
        }
        self.reset_position();
        self.layout()
    }

    pub fn clear_image(&mut self) {
        self.image = None;
    }

    pub fn set_mode(&mut self, mode: DisplayMode) -> Option<Layout> {
        self.mode = mode;
        self.custom_scale = false;
        self.scale = self.mode_scale();
        self.reset_position();
        self.layout()
    }

    pub fn toggle_stretch_small(&mut self) -> Option<Layout> {
        self.stretch_small = !self.stretch_small;
        self.refit()
    }

    pub fn toggle_zoom_lock(&mut self) -> Option<Layout> {
        self.zoom_lock = !self.zoom_lock;
        self.layout()
    }

    /// 启动时恢复缩放锁定（"保存缩放状态"）：锁定在用户看到的倍率 `zoom`（1 = 100%）。
    /// 之后换到不同缩放比例的屏幕时保持这个倍率。
    pub fn restore_zoom_lock(&mut self, zoom: f64) {
        if !zoom.is_finite() || zoom <= 0.0 {
            return;
        }
        self.zoom_lock = true;
        self.custom_scale = true;
        self.scale = (zoom * self.ppp).clamp(MIN_SCALE * self.ppp, MAX_SCALE * self.ppp);
    }

    /// 缩放到指定倍率，锚点（物理像素）下的图片位置保持不动；没有锚点时以视口中心为准。
    pub fn zoom_to(&mut self, scale: f64, anchor: Option<(f64, f64)>) -> Option<Layout> {
        self.image?;
        let scale = scale.clamp(MIN_SCALE * self.ppp, MAX_SCALE * self.ppp);
        let (vw, vh) = self.viewport;
        let (ax, ay) = anchor.unwrap_or((vw / 2.0, vh / 2.0));
        let (cx, cy) = (vw / 2.0 + self.offset.0, vh / 2.0 + self.offset.1);
        let k = scale / self.scale;
        let (ncx, ncy) = (ax - (ax - cx) * k, ay - (ay - cy) * k);
        self.offset = (ncx - vw / 2.0, ncy - vh / 2.0);
        self.scale = scale;
        self.custom_scale = true;
        self.clamp_offset();
        self.layout()
    }

    /// 按档位放大（direction > 0）或缩小（direction < 0）。
    pub fn zoom_step(&mut self, direction: i32, anchor: Option<(f64, f64)>) -> Option<Layout> {
        let current = self.scale / self.ppp;
        let target = if direction > 0 {
            ZOOM_STEPS
                .iter()
                .copied()
                .find(|&s| s > current * 1.0001)
                .unwrap_or(MAX_SCALE)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|&s| s < current * 0.9999)
                .unwrap_or(MIN_SCALE)
        };
        self.zoom_to(target * self.ppp, anchor)
    }

    /// 100%：一个图片像素一个点。
    pub fn zoom_actual(&mut self, anchor: Option<(f64, f64)>) -> Option<Layout> {
        self.zoom_to(self.ppp, anchor)
    }

    /// 触控板双指轻点两下：在"适合窗口"与 100% 之间切换，100% 时以指针为中心。
    pub fn toggle_fit_actual(&mut self, anchor: Option<(f64, f64)>) -> Option<Layout> {
        self.image?;
        let at_fit = self.mode == DisplayMode::FitWindow && !self.custom_scale;
        if at_fit && (self.scale - self.ppp).abs() > 1e-9 {
            self.zoom_actual(anchor)
        } else {
            self.set_mode(DisplayMode::FitWindow)
        }
    }

    /// 每次增减 1 个百分点。
    pub fn zoom_percent(&mut self, delta: i32) -> Option<Layout> {
        let percent = (self.scale / self.ppp * 100.0).round() + f64::from(delta);
        self.zoom_to(percent / 100.0 * self.ppp, None)
    }

    /// 连续缩放（捏合、按住 ⌘/Ctrl 滚动），以指针为中心。
    pub fn zoom_by(&mut self, factor: f64, anchor: Option<(f64, f64)>) -> Option<Layout> {
        if !factor.is_finite() || factor <= 0.0 {
            return self.layout();
        }
        self.zoom_to(self.scale * factor, anchor)
    }

    /// 旋转：相对当前角度增加 `degrees`（顺时针为正）。
    pub fn rotate_by(&mut self, degrees: i32) -> Option<Layout> {
        self.rotation = (i32::from(self.rotation) + degrees).rem_euclid(360) as u16;
        self.refit_and_recenter()
    }

    /// "不旋转"：复位到 0°。
    pub fn reset_rotation(&mut self) -> Option<Layout> {
        self.rotation = 0;
        self.refit_and_recenter()
    }

    pub fn toggle_flip_horizontal(&mut self) -> Option<Layout> {
        self.flip_horizontal = !self.flip_horizontal;
        self.layout()
    }

    pub fn toggle_flip_vertical(&mut self) -> Option<Layout> {
        self.flip_vertical = !self.flip_vertical;
        self.layout()
    }

    /// 拖动平移（物理像素）。图片小于视口的方向上保持居中。
    pub fn pan_by(&mut self, dx: f64, dy: f64) -> Option<Layout> {
        self.offset = (self.offset.0 + dx, self.offset.1 + dy);
        self.clamp_offset();
        self.layout()
    }

    /// 按键平移：移动视口的一部分。参数为方向（-1、0、1），向下、向右看为正。
    pub fn pan_step(&mut self, x: i32, y: i32) -> Option<Layout> {
        let (vw, vh) = self.viewport;
        // 向下看 = 图片向上移。
        self.pan_by(-f64::from(x) * vw * PAN_STEP, -f64::from(y) * vh * PAN_STEP)
    }

    /// 双指左右轻扫翻页：图片宽度没有超出窗口时，手势结束时横向位移足够大、
    /// 且明显大于纵向位移，就翻页。dx、dy 为画面移动的方向（物理像素，向左为负）。
    /// 返回 1 表示下一张，-1 表示上一张，0 表示不翻页。
    pub fn swipe(&mut self, phase: SwipePhase, dx: f64, dy: f64) -> i32 {
        match phase {
            SwipePhase::Began | SwipePhase::Cancelled => {
                self.swipe = (0.0, 0.0);
                0
            }
            SwipePhase::Changed => {
                self.swipe = (self.swipe.0 + dx, self.swipe.1 + dy);
                0
            }
            SwipePhase::Ended => {
                let (sx, sy) = std::mem::take(&mut self.swipe);
                let fits = self.image.is_some() && self.displayed_size().0 <= self.viewport.0 + 0.5;
                if !fits || sx.abs() < self.viewport.0 * SWIPE_FRACTION || sx.abs() < 2.0 * sy.abs()
                {
                    0
                } else if sx < 0.0 {
                    1
                } else {
                    -1
                }
            }
        }
    }

    /// 旋转后的图片尺寸（未缩放，图片像素）。
    fn rotated_image_size(&self) -> (f64, f64) {
        let Some((w, h)) = self.image else {
            return (0.0, 0.0);
        };
        if self.rotation % 180 == 90 {
            (f64::from(h), f64::from(w))
        } else {
            (f64::from(w), f64::from(h))
        }
    }

    /// 旋转后在屏幕上占的尺寸。
    fn displayed_size(&self) -> (f64, f64) {
        let (w, h) = self.rotated_image_size();
        (w * self.scale, h * self.scale)
    }

    /// 显示方式算出的物理倍率。100%（原始大小）是一个图片像素一个点，即 `ppp` 个物理像素；
    /// 适合窗口、适合宽度不超过 100%（"拉伸小图"打开时才放大）。
    fn mode_scale(&self) -> f64 {
        let (iw, ih) = self.rotated_image_size();
        let (vw, vh) = self.viewport;
        if iw <= 0.0 || ih <= 0.0 || vw <= 0.0 || vh <= 0.0 {
            return self.ppp;
        }
        let fit = match self.mode {
            DisplayMode::FitWindow => (vw / iw).min(vh / ih),
            DisplayMode::FitWidth => vw / iw,
            DisplayMode::Original => return self.ppp,
        };
        let fit = if self.stretch_small {
            fit
        } else {
            fit.min(self.ppp)
        };
        fit.clamp(MIN_SCALE * self.ppp, MAX_SCALE * self.ppp)
    }

    fn refit(&mut self) -> Option<Layout> {
        if !self.custom_scale {
            self.scale = self.mode_scale();
        }
        self.clamp_offset();
        self.layout()
    }

    fn refit_and_recenter(&mut self) -> Option<Layout> {
        if !self.custom_scale {
            self.scale = self.mode_scale();
        }
        self.reset_position();
        self.layout()
    }

    /// 起始位置：居中；图片比视口高且开启“图片大于窗口时显示图片顶部”时，从顶部开始显示。
    fn reset_position(&mut self) {
        self.offset = (0.0, 0.0);
        let (_, dh) = self.displayed_size();
        let vh = self.viewport.1;
        if self.top_first && dh > vh {
            self.offset.1 = (dh - vh) / 2.0;
        }
        self.clamp_offset();
    }

    /// 图片大于视口的方向上，不让图片边缘离开视口边缘；小于视口时居中。
    fn clamp_offset(&mut self) {
        let (dw, dh) = self.displayed_size();
        let (vw, vh) = self.viewport;
        let max_x = ((dw - vw) / 2.0).max(0.0);
        let max_y = ((dh - vh) / 2.0).max(0.0);
        self.offset = (
            self.offset.0.clamp(-max_x, max_x),
            self.offset.1.clamp(-max_y, max_y),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(image: (u32, u32), viewport: (f64, f64)) -> ViewState {
        let mut v = ViewState::default();
        v.set_viewport(viewport.0, viewport.1);
        v.set_image(image.0, image.1);
        v
    }

    #[test]
    fn fit_rect_shrinks_large_and_keeps_small() {
        assert_eq!(
            fit_rect(4000, 3000, 1800.0, 1200.0),
            Rect {
                x: 100.0,
                y: 0.0,
                width: 1600.0,
                height: 1200.0
            }
        );
        assert_eq!(
            fit_rect(64, 48, 1800.0, 1200.0),
            Rect {
                x: 868.0,
                y: 576.0,
                width: 64.0,
                height: 48.0
            }
        );
        assert_eq!(fit_rect(0, 10, 100.0, 100.0).width, 0.0);
    }

    #[test]
    fn window_follows_the_image_within_screen_limits() {
        let screen = (1512.0, 945.0);
        // 原始大小是一个图片像素一个点：4000x3000 的照片放不下，按比例缩到可用区域以内。
        assert_eq!(
            window_size_for_image((4000, 3000), DisplayMode::FitWindow, screen),
            (1260.0, 945.0)
        );
        // 放得下的图按原始大小；不小于 480x360 点。
        assert_eq!(
            window_size_for_image((800, 500), DisplayMode::FitWindow, screen),
            (800.0, 500.0)
        );
        assert_eq!(
            window_size_for_image((300, 200), DisplayMode::FitWindow, screen),
            (480.0, 360.0)
        );
        // 原始大小：宽、高分别截到可用区域；适合宽度：宽度截到可用区域、高度按比例再截。
        assert_eq!(
            window_size_for_image((4000, 1000), DisplayMode::Original, screen),
            (1512.0, 945.0)
        );
        assert_eq!(
            window_size_for_image((1512, 2000), DisplayMode::FitWidth, screen),
            (1512.0, 945.0)
        );
        assert_eq!(
            window_size_for_image((2000, 600), DisplayMode::FitWidth, screen),
            (1512.0, 454.0)
        );
        // 屏幕比最小尺寸还小、或没有尺寸时不出错。
        assert_eq!(
            window_size_for_image((0, 0), DisplayMode::FitWindow, screen),
            MIN_WINDOW
        );
        assert_eq!(
            window_size_for_image((4000, 3000), DisplayMode::FitWindow, (100.0, 100.0)),
            (480.0, 360.0)
        );
    }

    #[test]
    fn window_follows_rotation_and_locked_zoom() {
        let screen = (1512.0, 945.0);
        let mut v = view((800, 500), (1800.0, 1200.0));
        // 横图 800x500；转 90° 后窗口是竖的 500x800 点。
        assert_eq!(v.window_size_for_current(screen), Some((800.0, 500.0)));
        v.rotate_by(90);
        assert_eq!(v.window_size_for_current(screen), Some((500.0, 800.0)));
        v.rotate_by(90);
        assert_eq!(v.window_size_for_current(screen), Some((800.0, 500.0)));
        // 竖长的图：竖着装不下（缩到可用区域以内，宽度不小于最小尺寸），转过来以后放得下。
        let mut tall = view((500, 1500), (900.0, 600.0));
        assert_eq!(tall.window_size_for_current(screen), Some((480.0, 945.0)));
        tall.rotate_by(90);
        assert_eq!(tall.window_size_for_current(screen), Some((1500.0, 500.0)));
        // 锁定缩放：按锁定的倍率（用户看到的倍率）算，各边截到可用区域以内。
        v.set_pixels_per_point(2.0);
        v.zoom_lock = true;
        v.scale = 4.0; // 200%
        assert_eq!(
            v.window_size_for((800, 500), false, screen),
            (1512.0, 945.0)
        );
        v.scale = 1.0; // 50%
        assert_eq!(v.window_size_for((800, 500), false, screen), (480.0, 360.0));
        assert!(
            ViewState::default()
                .window_size_for_current(screen)
                .is_none()
        );
    }

    #[test]
    fn image_exceeds_viewport_only_when_zoomed_past_it() {
        // 窗口跟着图片时图片完整显示：左键拖动移动窗口；放大到超出画布后平移图片。
        let mut v = view((800, 500), (800.0, 500.0));
        assert!(!v.image_exceeds_viewport());
        v.zoom_step(1, None);
        assert!(v.image_exceeds_viewport());
        v.zoom_step(-1, None);
        v.zoom_step(-1, None);
        assert!(!v.image_exceeds_viewport());
        // 竖长的图在横的画布里：超出的方向只要有一个就算。
        let tall = view((400, 3000), (400.0, 300.0));
        assert!(!tall.image_exceeds_viewport()); // 适合窗口：缩小到放得下
        assert!(!ViewState::default().image_exceeds_viewport());
    }

    #[test]
    fn hundred_percent_is_one_image_pixel_per_point_on_hidpi() {
        // Retina（一个点 2 个物理像素）：视口 1600x1200 物理像素 = 800x600 点。
        let mut v = view((600, 400), (1600.0, 1200.0));
        v.set_pixels_per_point(2.0);
        let l = v.layout().unwrap();
        // 小图适合窗口时停在 100%：一个图片像素占 2x2 个物理像素，600x400 的图显示成 1200x800 物理像素。
        assert_eq!((l.scale, l.zoom), (2.0, 1.0));
        assert_eq!((l.width, l.height), (1200.0, 800.0));
        assert_eq!(zoom_text(l.zoom), "100%");
        // 档位、1% 微调、100% 都按用户看到的倍率。
        let l = v.zoom_step(1, None).unwrap();
        assert_eq!((l.scale, l.zoom), (3.0, 1.5));
        let l = v.zoom_percent(1).unwrap();
        assert!((l.zoom - 1.51).abs() < 1e-9, "{l:?}");
        assert_eq!(v.zoom_actual(None).unwrap().scale, 2.0);
        // 换到 1 倍屏：手动缩放过的保持用户看到的倍率。
        v.zoom_step(1, None);
        let l = v.set_pixels_per_point(1.0).unwrap();
        assert_eq!((l.scale, l.zoom), (1.5, 1.5));
        // 原始大小 = 100%。
        let l = v.set_mode(DisplayMode::Original).unwrap();
        assert_eq!((l.scale, l.zoom), (1.0, 1.0));
        // 大图适合窗口：物理倍率按视口算，用户看到的倍率是它的一半。
        let mut big = view((4000, 3000), (1600.0, 1200.0));
        big.set_pixels_per_point(2.0);
        let l = big.layout().unwrap();
        assert_eq!(l.scale, 0.4);
        assert!((l.zoom - 0.2).abs() < 1e-12);
        // 双指轻点两下：在适合窗口与 100%（物理倍率 2）之间切换。
        assert_eq!(big.toggle_fit_actual(None).unwrap().scale, 2.0);
        assert_eq!(big.toggle_fit_actual(None).unwrap().scale, 0.4);
    }

    #[test]
    fn image_shown_before_the_canvas_has_a_size_is_placed_when_it_gets_one() {
        // 启动时提前打开：图片先到、画布后有尺寸。结果应与先有尺寸再显示相同（大图从顶部显示）。
        let mut early = ViewState::default();
        early.set_image(1000, 4000);
        let late = early.set_viewport(800.0, 600.0).unwrap();
        let normal = view((1000, 4000), (800.0, 600.0)).layout().unwrap();
        assert_eq!(late, normal);
        // 之后改变窗口大小只做边界约束，不再复位。
        early.pan_by(0.0, -100.0);
        let moved = early.layout().unwrap();
        assert_eq!(
            early.set_viewport(820.0, 600.0).unwrap().center_y,
            moved.center_y
        );
    }

    #[test]
    fn fit_window_matches_fit_rect() {
        let l = view((4000, 3000), (1800.0, 1200.0)).layout().unwrap();
        assert_eq!(
            (l.center_x, l.center_y, l.width, l.height),
            (900.0, 600.0, 1600.0, 1200.0)
        );
        let small = view((64, 48), (1800.0, 1200.0)).layout().unwrap();
        assert_eq!((small.width, small.scale), (64.0, 1.0));
    }

    #[test]
    fn stretch_small_and_modes() {
        let mut v = view((100, 50), (1000.0, 1000.0));
        assert_eq!(v.toggle_stretch_small().unwrap().scale, 10.0);
        assert_eq!(v.set_mode(DisplayMode::Original).unwrap().scale, 1.0);
        let mut tall = view((1000, 4000), (800.0, 600.0));
        let l = tall.set_mode(DisplayMode::FitWidth).unwrap();
        assert_eq!(l.scale, 0.8);
        // 高于窗口时从顶部开始：图片上边缘在 0。
        assert_eq!(l.center_y - 4000.0 * 0.8 / 2.0, 0.0);
    }

    #[test]
    fn zoom_steps_follow_the_ladder_and_keep_the_anchor() {
        let mut v = view((1000, 1000), (500.0, 500.0));
        assert_eq!(v.layout().unwrap().scale, 0.5);
        assert_eq!(v.zoom_step(1, None).unwrap().scale, 0.67);
        assert_eq!(v.zoom_step(1, None).unwrap().scale, 1.0);
        assert_eq!(v.zoom_step(-1, None).unwrap().scale, 0.67);
        // 以左上角附近为锚点放大，锚点下的图片位置不动。
        let mut v = view((1000, 1000), (1000.0, 1000.0));
        let before = v.layout().unwrap();
        let anchor = (100.0, 100.0);
        let image_x = (anchor.0 - (before.center_x - before.width / 2.0)) / before.scale;
        let after = v.zoom_to(2.0, Some(anchor)).unwrap();
        let image_x_after = (anchor.0 - (after.center_x - after.width / 2.0)) / after.scale;
        assert!(
            (image_x - image_x_after).abs() < 1.0,
            "{image_x} vs {image_x_after}"
        );
    }

    #[test]
    fn zoom_percent_and_limits() {
        let mut v = view((100, 100), (1000.0, 1000.0));
        assert_eq!(v.zoom_percent(1).unwrap().scale, 1.01);
        assert_eq!(v.zoom_to(1000.0, None).unwrap().scale, MAX_SCALE);
        assert_eq!(v.zoom_to(0.0, None).unwrap().scale, MIN_SCALE);
    }

    #[test]
    fn rotation_is_relative_and_resets_on_new_image() {
        let mut v = view((4000, 2000), (1000.0, 1000.0));
        assert_eq!(v.layout().unwrap().scale, 0.25);
        let l = v.rotate_by(90).unwrap();
        assert_eq!((l.rotation, l.scale), (90, 0.25));
        assert_eq!(v.rotate_by(90).unwrap().rotation, 180);
        assert_eq!(v.rotate_by(-270).unwrap().rotation, 270);
        assert_eq!(v.reset_rotation().unwrap().rotation, 0);
        v.rotate_by(90);
        v.toggle_flip_horizontal();
        let l = v.set_image(10, 10).unwrap();
        assert_eq!((l.rotation, l.flip_horizontal), (0, false));
    }

    #[test]
    fn rotation_refits_tall_images() {
        let mut v = view((2000, 1000), (1000.0, 1000.0));
        assert_eq!(v.layout().unwrap().scale, 0.5);
        // 转 90° 后宽高互换，适合窗口仍是 0.5（1000 高 → 2000 高需缩到 0.5）。
        assert_eq!(v.rotate_by(90).unwrap().scale, 0.5);
        let mut v = view((3000, 1000), (1000.0, 2000.0));
        assert_eq!(v.layout().unwrap().scale, 1.0 / 3.0);
        assert!((v.rotate_by(90).unwrap().scale - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn zoom_lock_keeps_scale_across_images() {
        let mut v = view((1000, 1000), (500.0, 500.0));
        v.zoom_to(2.0, None);
        // 没有锁定：按显示方式重新计算（400 小于窗口，适合窗口为 100%）。
        assert_eq!(v.set_image(400, 400).unwrap().scale, 1.0);
        v.zoom_to(2.0, None);
        v.toggle_zoom_lock();
        assert_eq!(v.set_image(800, 800).unwrap().scale, 2.0);
    }

    #[test]
    fn pan_is_clamped_and_small_images_stay_centered() {
        let mut v = view((2000, 2000), (1000.0, 1000.0));
        v.set_mode(DisplayMode::Original);
        // 原始大小 2000 > 1000，起始位置在顶部，水平居中。
        let l = v.layout().unwrap();
        assert_eq!((l.center_x, l.center_y), (500.0, 1000.0));
        let l = v.pan_by(-5000.0, -5000.0).unwrap();
        assert_eq!((l.center_x, l.center_y), (0.0, 0.0));
        let l = v.pan_step(0, -1).unwrap(); // 向上看：图片下移
        assert_eq!(l.center_y, 150.0);
        let mut small = view((100, 100), (1000.0, 1000.0));
        assert_eq!(small.pan_by(300.0, 300.0).unwrap().center_x, 500.0);
    }

    #[test]
    fn mapping_follows_rotation_then_screen_flip() {
        // 600x400 的图放在 1000x1000 视口，100%，居中于 (500, 500)。
        let mut v = view((600, 400), (1000.0, 1000.0));
        let top_middle = (300.0, 0.0);
        let l = v.layout().unwrap();
        assert_eq!(
            l.image_to_screen(600, 400, top_middle.0, top_middle.1),
            (500.0, 300.0)
        );
        // 顺时针 90°：图片的上边转到右边。
        let l = v.rotate_by(90).unwrap();
        assert_eq!(
            l.image_to_screen(600, 400, top_middle.0, top_middle.1),
            (700.0, 500.0)
        );
        // 再水平翻转（按屏幕方向）：上边到了左边。
        let l = v.toggle_flip_horizontal().unwrap();
        assert_eq!(
            l.image_to_screen(600, 400, top_middle.0, top_middle.1),
            (300.0, 500.0)
        );
        // 反向映射回到原处。
        for &(u, v_) in &[(0.0, 0.0), (600.0, 0.0), (123.0, 321.0)] {
            let (x, y) = l.image_to_screen(600, 400, u, v_);
            let (u2, v2) = l.screen_to_image(600, 400, x, y);
            assert!((u - u2).abs() < 1e-9 && (v_ - v2).abs() < 1e-9);
        }
    }

    #[test]
    fn smart_magnify_toggles_fit_and_actual() {
        let mut v = view((2000, 2000), (1000.0, 1000.0));
        assert_eq!(v.toggle_fit_actual(Some((0.0, 0.0))).unwrap().scale, 1.0);
        assert_eq!(v.toggle_fit_actual(None).unwrap().scale, 0.5);
    }

    #[test]
    fn swipe_turns_pages_only_when_the_image_fits() {
        let mut v = view((400, 300), (1000.0, 1000.0));
        let gesture = |v: &mut ViewState, dx: f64, dy: f64| {
            v.swipe(SwipePhase::Began, 0.0, 0.0);
            v.swipe(SwipePhase::Changed, dx / 2.0, dy / 2.0);
            v.swipe(SwipePhase::Changed, dx / 2.0, dy / 2.0);
            v.swipe(SwipePhase::Ended, 0.0, 0.0)
        };
        assert_eq!(gesture(&mut v, -200.0, 10.0), 1);
        assert_eq!(gesture(&mut v, 200.0, 0.0), -1);
        assert_eq!(gesture(&mut v, -50.0, 0.0), 0, "太短");
        assert_eq!(gesture(&mut v, -200.0, 150.0), 0, "斜着滑");
        // 放大到超出窗口宽度后，双指滑动只平移、不翻页。
        v.zoom_to(4.0, None);
        assert_eq!(gesture(&mut v, -200.0, 0.0), 0);
    }

    #[test]
    fn exact_rendering_follows_edge_and_interpolation() {
        let mut v = view((1000, 1000), (500.0, 500.0));
        assert!(!v.needs_exact(), "默认减淡 + Bilinear 只走 GPU");
        v.interpolation = Interpolation::Lanczos;
        assert!(v.needs_exact());
        v.edge = EdgeMode::None;
        assert!(!v.needs_exact(), "无：最近邻");
        v.edge = EdgeMode::SoftenSharpen;
        v.interpolation = Interpolation::Bilinear;
        assert!(v.needs_exact(), "减淡+锐化");
        v.zoom_to(1.0, None);
        assert!(!v.needs_exact(), "100% 像素一一对应");
        let before = v.signature();
        v.pan_by(10.0, 0.0);
        v.zoom_to(1.5, None);
        assert_ne!(before, v.signature());
        let same = v.signature();
        assert_eq!(same, v.signature());
    }

    #[test]
    fn zoom_text_formats_percentages() {
        assert_eq!(zoom_text(1.5), "150%");
        assert_eq!(zoom_text(1.0 / 3.0), "33%");
        assert_eq!(zoom_text(0.05), "5%");
        assert_eq!(zoom_text(0.0267), "2.7%");
        assert_eq!(zoom_text(32.0), "3200%");
    }

    #[test]
    fn edge_mode_controls_smoothing() {
        let mut v = view((10, 10), (100.0, 100.0));
        assert!(v.layout().unwrap().smooth);
        v.edge = EdgeMode::None;
        assert!(!v.layout().unwrap().smooth);
    }

    #[test]
    fn resize_refits_unless_custom() {
        let mut v = view((1000, 1000), (500.0, 500.0));
        assert_eq!(v.set_viewport(250.0, 250.0).unwrap().scale, 0.25);
        v.zoom_to(1.0, None);
        assert_eq!(v.set_viewport(500.0, 500.0).unwrap().scale, 1.0);
    }

    #[test]
    fn windows_snap_to_the_nearest_screen_edges() {
        // 可用区域 (0, 40) 起 1440x860（下面 40 点是程序坞）。
        let area = (0.0, 40.0, 1440.0, 860.0);
        // 离左边 7 点、离上边 9 点：贴到左上角。
        assert_eq!(
            snap_to_edges((7.0, 291.0, 600.0, 600.0), area),
            (0.0, 300.0)
        );
        // 离右边 12 点（正好在范围内）、离下边 30 点：只贴右边。
        assert_eq!(
            snap_to_edges((828.0, 70.0, 600.0, 400.0), area),
            (840.0, 70.0)
        );
        // 都远：不动。
        assert_eq!(
            snap_to_edges((100.0, 200.0, 600.0, 400.0), area),
            (100.0, 200.0)
        );
        // 出界一点也吸回来；比可用区域还高时上下不动。
        assert_eq!(snap_to_edges((-5.0, 30.0, 600.0, 900.0), area), (0.0, 30.0));
    }
}
