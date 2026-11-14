//! 图片画布（ARCHITECTURE"画布与渲染"的交互路径）：把解码好的 BGRA 缓冲区交给 GTK 作纹理，
//! 按核心算出的绘制参数（物理像素）由 GPU 合成：平移、缩放、旋转、翻转都只是变换，不改像素。

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::time::Duration;

use anyview_core::settings::AlphaFill;
use anyview_core::transition::{self, LayerParams, PageEffect, TransitionFrame};
use anyview_core::{AppCore, DecodedImage};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ImageCanvas {
        pub core: RefCell<Option<Arc<AppCore>>>,
        pub image: RefCell<Option<Arc<DecodedImage>>>,
        pub texture: RefCell<Option<gdk::Texture>>,
        /// 精确重绘的结果，签名与当前画面相同时盖在上面。
        pub exact: RefCell<Option<super::Exact>>,
        /// 最近一次安排精确重绘时的画面签名（静止 100 ms 后请求）。
        pub scheduled: Cell<u64>,
        /// 透明处的背景（设置"为32位图片使用透明背景"）；空表示棋盘格。
        pub alpha_solid: Cell<Option<gdk::RGBA>>,
        /// 正在做的翻页效果：旧画面的快照和进度。
        pub transition: RefCell<Option<super::Transition>>,
        /// 自测用：下一次翻页效果停在这个进度（0–1），不随时间走，方便截图核对。
        pub hold: Cell<Option<f64>>,
        /// 背景图片：按"填满并裁切"铺在图片后面，空态也画。
        pub background: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ImageCanvas {
        const NAME: &'static str = "AnyviewImageCanvas";
        type Type = super::ImageCanvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ImageCanvas {
        fn dispose(&self) {
            // 右键菜单以画布为父控件，销毁时一并解除。
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for ImageCanvas {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            // 自定义控件要在分配尺寸时摆放作为子控件的弹出菜单（GTK4 的要求）。
            let mut child = self.obj().first_child();
            while let Some(widget) = child {
                if let Some(popover) = widget.downcast_ref::<gtk::Popover>() {
                    popover.present();
                }
                child = widget.next_sibling();
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            // 背景图片：按"填满并裁切"铺满画布，翻页时也不动。
            if let Some(background) = self.background.borrow().as_ref() {
                let (w, h) = (widget.width() as f32, widget.height() as f32);
                let (bw, bh) = (background.width() as f32, background.height() as f32);
                if w > 0.0 && h > 0.0 && bw > 0.0 && bh > 0.0 {
                    let k = (w / bw).max(h / bh);
                    let (dw, dh) = (bw * k, bh * k);
                    snapshot.push_clip(&graphene::Rect::new(0.0, 0.0, w, h));
                    snapshot.append_scaled_texture(
                        background,
                        gsk::ScalingFilter::Trilinear,
                        &graphene::Rect::new((w - dw) / 2.0, (h - dh) / 2.0, dw, dh),
                    );
                    snapshot.pop();
                }
            }
            let transition = self.transition.borrow().as_ref().map(|t| {
                (t.frame(widget.frame_clock().map(|c| c.frame_time())), t.old.clone(), t.old_size)
            });
            let Some((frame, old, old_size)) = transition else {
                self.draw_image(snapshot);
                return;
            };
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            // 软件渲染（cairo）画不了三维变换：翻页改为以左边为轴的水平压扁（软件后备画法）。
            let flat = widget
                .native()
                .and_then(|n| n.renderer())
                .is_some_and(|r| r.is::<gsk::CairoRenderer>());
            let draw_old = |s: &gtk::Snapshot| {
                super::draw_layer(s, frame.old, w, h, flat, |s| {
                    // 旧画面按它原来的大小居中（窗口跟着新图片变了大小时，屏幕上的位置不变）。
                    s.translate(&graphene::Point::new((w - old_size.0) / 2.0, (h - old_size.1) / 2.0));
                    old.snapshot(s, f64::from(old_size.0), f64::from(old_size.1));
                });
            };
            let draw_new =
                |s: &gtk::Snapshot| super::draw_layer(s, frame.new, w, h, flat, |s| self.draw_image(s));
            if frame.old_on_top {
                draw_new(snapshot);
                draw_old(snapshot);
            } else {
                draw_old(snapshot);
                draw_new(snapshot);
            }
        }
    }

    impl ImageCanvas {
        /// 画当前图片（平移、缩放、旋转、翻转、透明背景、精确重绘的覆盖层）。
        pub fn draw_image(&self, snapshot: &gtk::Snapshot) {
            let (Some(texture), Some(core)) =
                (self.texture.borrow().clone(), self.core.borrow().clone())
            else {
                return;
            };
            let widget = self.obj();
            let scale = widget.physical_scale();
            // 100% 是一个图片像素一个点：先告诉核心一个逻辑像素几个物理像素。
            core.view_set_pixels_per_point(scale);
            let Some(layout) = core.view_set_viewport(
                f64::from(widget.width()) * scale,
                f64::from(widget.height()) * scale,
            ) else {
                return;
            };
            // 先移到图片中心，再按屏幕方向翻转，再顺时针旋转：画面 = 平移 · 翻转 · 旋转 · 图片。
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                (layout.center_x / scale) as f32,
                (layout.center_y / scale) as f32,
            ));
            if layout.flip_horizontal || layout.flip_vertical {
                snapshot.scale(
                    if layout.flip_horizontal { -1.0 } else { 1.0 },
                    if layout.flip_vertical { -1.0 } else { 1.0 },
                );
            }
            if layout.rotation != 0 {
                snapshot.rotate(f32::from(layout.rotation));
            }
            let (w, h) = (
                (layout.width / scale) as f32,
                (layout.height / scale) as f32,
            );
            let filter = if !layout.smooth {
                gsk::ScalingFilter::Nearest
            } else if layout.scale < 1.0 {
                gsk::ScalingFilter::Trilinear
            } else {
                gsk::ScalingFilter::Linear
            };
            // 带透明通道的图片先铺背景：默认棋盘格（#FFFFFF 与 #C8C8C8，每格 8 点，不随缩放变化），
            // 设置为黑色或白色时铺纯色。
            let has_alpha = self
                .image
                .borrow()
                .as_ref()
                .is_some_and(|i| i.info.has_alpha);
            if let (true, Some(color)) = (has_alpha, self.alpha_solid.get()) {
                snapshot.append_color(&color, &graphene::Rect::new(-w / 2.0, -h / 2.0, w, h));
            } else if has_alpha {
                let area = graphene::Rect::new(-w / 2.0, -h / 2.0, w, h);
                let tile = graphene::Rect::new(-w / 2.0, -h / 2.0, 16.0, 16.0);
                let gray = gdk::RGBA::new(200.0 / 255.0, 200.0 / 255.0, 200.0 / 255.0, 1.0);
                snapshot.push_repeat(&area, Some(&tile));
                snapshot.append_color(&gdk::RGBA::WHITE, &tile);
                snapshot.append_color(
                    &gray,
                    &graphene::Rect::new(-w / 2.0 + 8.0, -h / 2.0, 8.0, 8.0),
                );
                snapshot.append_color(
                    &gray,
                    &graphene::Rect::new(-w / 2.0, -h / 2.0 + 8.0, 8.0, 8.0),
                );
                snapshot.pop();
            }
            snapshot.append_scaled_texture(
                &texture,
                filter,
                &graphene::Rect::new(-w / 2.0, -h / 2.0, w, h),
            );
            snapshot.restore();

            // 精确重绘的结果与屏幕物理像素一一对应，签名不同（画面已变）时不画，并等静止后重新请求。
            let signature = core.view_signature();
            let exact = self.exact.borrow();
            if let Some(exact) = exact.as_ref().filter(|e| e.signature == signature) {
                let bounds = graphene::Rect::new(
                    (exact.x as f64 / scale) as f32,
                    (exact.y as f64 / scale) as f32,
                    (f64::from(exact.texture.width()) / scale) as f32,
                    (f64::from(exact.texture.height()) / scale) as f32,
                );
                snapshot.append_scaled_texture(
                    &exact.texture,
                    gsk::ScalingFilter::Nearest,
                    &bounds,
                );
            } else if self.scheduled.replace(signature) != signature {
                let weak = widget.downgrade();
                let delay = Duration::from_millis(u64::from(anyview_core::exact::DELAY_MS));
                glib::timeout_add_local_once(delay, move || {
                    let Some(canvas) = weak.upgrade() else { return };
                    let core = canvas.imp().core.borrow().clone();
                    if let Some(core) = core
                        && core.view_signature() == signature
                    {
                        core.request_exact();
                    }
                });
            }
        }
    }
}

/// 正在做的翻页效果：旧画面的快照、开始的时刻（第一次画时记下）和持续时间。
pub struct Transition {
    pub old: gdk::Paintable,
    /// 快照时画布的大小（逻辑像素）。
    pub old_size: (f32, f32),
    pub effect: PageEffect,
    pub forward: bool,
    pub duration_us: i64,
    pub started_us: Cell<Option<i64>>,
    /// 自测用：停在这个进度。
    pub hold: Option<f64>,
}

impl Transition {
    /// 进度（0–1）。`now_us` 是这一帧的时刻（画面时钟）。
    pub fn progress(&self, now_us: Option<i64>) -> f64 {
        if let Some(p) = self.hold {
            return p;
        }
        let Some(now) = now_us else { return 0.0 };
        let started = self.started_us.get().unwrap_or_else(|| {
            self.started_us.set(Some(now));
            now
        });
        ((now - started) as f64 / self.duration_us.max(1) as f64).clamp(0.0, 1.0)
    }

    fn frame(&self, now_us: Option<i64>) -> TransitionFrame {
        transition::frame(self.effect, self.forward, self.progress(now_us))
    }
}

/// 按翻页效果的参数画一层：平移、以画布左边为轴的透视翻转、不透明度、变暗。
fn draw_layer(
    snapshot: &gtk::Snapshot,
    p: LayerParams,
    w: f32,
    h: f32,
    flat: bool,
    content: impl FnOnce(&gtk::Snapshot),
) {
    if p.opacity <= 0.0 {
        return;
    }
    snapshot.save();
    if p.shift != 0.0 {
        snapshot.translate(&graphene::Point::new(p.shift as f32 * w, 0.0));
    }
    if p.angle != 0.0 && flat {
        // 没有透视：按转过的角度把宽度压到 cos 倍（转轴在左边）。
        let squeeze = (p.angle.to_radians().cos() as f32).max(0.0);
        if squeeze <= 0.001 {
            snapshot.restore();
            return;
        }
        snapshot.scale(squeeze, 1.0);
    } else if p.angle != 0.0 {
        // 透视以画布中心为视点；转轴是画布左边的竖直线。
        snapshot.translate(&graphene::Point::new(w / 2.0, h / 2.0));
        snapshot.perspective(transition::PERSPECTIVE as f32 * w);
        snapshot.translate(&graphene::Point::new(-w / 2.0, 0.0));
        snapshot.rotate_3d(p.angle as f32, &graphene::Vec3::y_axis());
        snapshot.translate(&graphene::Point::new(0.0, -h / 2.0));
    }
    let faded = p.opacity < 1.0;
    if faded {
        snapshot.push_opacity(p.opacity);
    }
    // 变暗：颜色乘一个系数，透明的地方仍然透明（不会把窗口背景也压暗）。
    let shaded = p.shade > 0.0;
    if shaded {
        let k = (1.0 - p.shade) as f32;
        let matrix = graphene::Matrix::from_float([
            k, 0.0, 0.0, 0.0, //
            0.0, k, 0.0, 0.0, //
            0.0, 0.0, k, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]);
        snapshot.push_color_matrix(&matrix, &graphene::Vec4::new(0.0, 0.0, 0.0, 0.0));
    }
    content(snapshot);
    if shaded {
        snapshot.pop();
    }
    if faded {
        snapshot.pop();
    }
    snapshot.restore();
}

/// 精确重绘的结果：放在视口物理像素 (x, y) 处。
pub struct Exact {
    pub texture: gdk::Texture,
    pub x: i64,
    pub y: i64,
    pub signature: u64,
    pub render_ms: f64,
}

glib::wrapper! {
    pub struct ImageCanvas(ObjectSubclass<imp::ImageCanvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ImageCanvas {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl ImageCanvas {
    /// 透明处的背景：按当前图片由核心决定（GIF 与 32 位图片各有设置）。
    /// 显示图片时、设置生效后调用。
    pub fn refresh_alpha_fill(&self) {
        let core = self.imp().core.borrow().clone();
        let image = self.imp().image.borrow().clone();
        let fill = match (core, image) {
            (Some(core), Some(image)) => core.alpha_fill(&image),
            _ => AlphaFill::Checker,
        };
        let solid = match fill {
            AlphaFill::Checker => None,
            AlphaFill::Solid(rgb) => Some(gdk::RGBA::new(
                ((rgb >> 16) & 0xFF) as f32 / 255.0,
                ((rgb >> 8) & 0xFF) as f32 / 255.0,
                (rgb & 0xFF) as f32 / 255.0,
                1.0,
            )),
        };
        if self.imp().alpha_solid.replace(solid) != solid {
            self.queue_draw();
        }
    }

    pub fn set_core(&self, core: Arc<AppCore>) {
        *self.imp().core.borrow_mut() = Some(core);
    }

    /// 显示一张新图片；传 None 清空。查看状态（倍率、旋转等）由核心按切图与锁定规则处理。
    /// 超大图的纹理是缩小的代理，按原图尺寸拉伸显示。
    pub fn set_image(&self, image: Option<&Arc<DecodedImage>>) {
        match image {
            Some(image) => self.show(image, image.width, image.height),
            None => {
                if let Some(core) = self.imp().core.borrow().clone() {
                    core.view_clear();
                }
                *self.imp().texture.borrow_mut() = None;
                *self.imp().image.borrow_mut() = None;
                *self.imp().exact.borrow_mut() = None;
                self.queue_draw();
            }
        }
    }

    /// 快速预览：小缩略图拉伸到原图尺寸显示，查看状态按新图片处理；清晰图随后由 `replace_image` 换上。
    pub fn set_preview(&self, preview: &Arc<DecodedImage>, width: u32, height: u32) {
        self.show(preview, width, height);
    }

    fn show(&self, image: &Arc<DecodedImage>, width: u32, height: u32) {
        if let Some(core) = self.imp().core.borrow().clone() {
            let scale = self.physical_scale();
            core.view_set_pixels_per_point(scale);
            core.view_set_viewport(
                f64::from(self.width()) * scale,
                f64::from(self.height()) * scale,
            );
            core.view_show_image(width, height);
        }
        *self.imp().texture.borrow_mut() = Some(texture_from(display_buffer(image)));
        *self.imp().image.borrow_mut() = Some(Arc::clone(image));
        *self.imp().exact.borrow_mut() = None;
        self.imp().scheduled.set(0);
        self.refresh_alpha_fill();
        self.queue_draw();
    }

    /// 同一张图片换了显示缓冲区（反相、Gamma，或清晰图替换快速预览）：只换纹理，查看状态不变。
    pub fn replace_image(&self, image: &Arc<DecodedImage>) {
        *self.imp().texture.borrow_mut() = Some(texture_from(display_buffer(image)));
        *self.imp().image.borrow_mut() = Some(Arc::clone(image));
        *self.imp().exact.borrow_mut() = None;
        self.imp().scheduled.set(0);
        self.refresh_alpha_fill();
        self.queue_draw();
    }

    /// 盖上精确重绘的结果。
    pub fn set_exact(
        &self,
        image: &Arc<DecodedImage>,
        x: i64,
        y: i64,
        signature: u64,
        render_ms: f64,
    ) {
        *self.imp().exact.borrow_mut() = Some(Exact {
            texture: texture_from(image),
            x,
            y,
            signature,
            render_ms,
        });
        self.queue_draw();
    }

    /// 背景图片：传 None 时用背景色（窗口的样式）。
    pub fn set_background(&self, image: Option<&Arc<DecodedImage>>) {
        *self.imp().background.borrow_mut() = image.map(texture_from);
        self.queue_draw();
    }

    /// 自测用：现在有没有背景图片。
    pub fn has_background(&self) -> bool {
        self.imp().background.borrow().is_some()
    }

    /// 开始翻页效果：先把现在的画面拍成快照，之后由 `set_image` 等换上新图片，几百毫秒内两层叠着画。
    /// 画布上没有图片（空态）时不做。
    pub fn begin_transition(&self, effect: PageEffect, forward: bool, duration_ms: u32) {
        let imp = self.imp();
        if effect == PageEffect::None || imp.texture.borrow().is_none() || !self.is_mapped() {
            return;
        }
        let old = gtk::WidgetPaintable::new(Some(self)).current_image();
        let hold = imp.hold.get();
        *imp.transition.borrow_mut() = Some(Transition {
            old,
            old_size: (self.width() as f32, self.height() as f32),
            effect,
            forward,
            duration_us: i64::from(duration_ms) * 1000,
            started_us: Cell::new(None),
            hold,
        });
        if hold.is_some() {
            self.queue_draw();
            return;
        }
        self.add_tick_callback(|canvas, clock| {
            canvas.queue_draw();
            let done = canvas
                .imp()
                .transition
                .borrow()
                .as_ref()
                .is_none_or(|t| t.hold.is_none() && t.progress(Some(clock.frame_time())) >= 1.0);
            if done {
                canvas.imp().transition.borrow_mut().take();
                canvas.queue_draw();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// 正在做的翻页效果（自测用）：效果和现在的进度。
    pub fn transition_state(&self) -> Option<(PageEffect, bool, f64)> {
        let clock = self.frame_clock().map(|c| c.frame_time());
        self.imp()
            .transition
            .borrow()
            .as_ref()
            .map(|t| (t.effect, t.forward, t.progress(clock)))
    }

    /// 自测用：现在设定的停住的进度。
    pub fn held_progress(&self) -> Option<f64> {
        self.imp().hold.get()
    }

    /// 自测用：下一次翻页效果停在 `progress`；传 None 时取消，并结束停着的效果。
    pub fn hold_transition(&self, progress: Option<f64>) {
        self.imp().hold.set(progress);
        if progress.is_none() {
            let held = self
                .imp()
                .transition
                .borrow()
                .as_ref()
                .is_some_and(|t| t.hold.is_some());
            if held {
                self.imp().transition.borrow_mut().take();
            }
            self.queue_draw();
        }
    }

    /// 精确重绘的结果正盖在画面上（签名与当前相同），自测用：返回位置、尺寸和耗时。
    pub fn exact_shown(&self) -> Option<(i64, i64, i32, i32, f64)> {
        let core = self.imp().core.borrow().clone()?;
        let exact = self.imp().exact.borrow();
        let exact = exact.as_ref()?;
        (exact.signature == core.view_signature()).then(|| {
            (
                exact.x,
                exact.y,
                exact.texture.width(),
                exact.texture.height(),
                exact.render_ms,
            )
        })
    }

    pub fn image(&self) -> Option<Arc<DecodedImage>> {
        self.imp().image.borrow().clone()
    }

    /// 一个逻辑像素对应的物理像素数（支持分数缩放）。
    pub fn physical_scale(&self) -> f64 {
        self.native()
            .and_then(|native| native.surface())
            .map(|surface| surface.scale())
            .filter(|s| *s > 0.0)
            .unwrap_or_else(|| f64::from(self.scale_factor().max(1)))
    }
}

/// 让 GTK 直接引用核心的像素缓冲区（不复制）；纹理释放时一并释放这份引用。
struct PixelOwner(Arc<DecodedImage>);

impl AsRef<[u8]> for PixelOwner {
    fn as_ref(&self) -> &[u8] {
        &self.0.pixels
    }
}

/// 交给 GPU 的缓冲区：超大图用代理，其他图片就是自己。
fn display_buffer(image: &Arc<DecodedImage>) -> &Arc<DecodedImage> {
    image.proxy.as_ref().unwrap_or(image)
}

/// 图像的纹理（不复制像素：纹理持有图像的引用）。
pub fn texture_from(image: &Arc<DecodedImage>) -> gdk::Texture {
    let bytes = glib::Bytes::from_owned(PixelOwner(Arc::clone(image)));
    gdk::MemoryTexture::new(
        image.width as i32,
        image.height as i32,
        gdk::MemoryFormat::B8g8r8a8Premultiplied,
        &bytes,
        image.stride,
    )
    .upcast()
}
