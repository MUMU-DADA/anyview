//! 精确重绘（ARCHITECTURE"两条路径"）：交互停止后，按当前倍率对可见区域做高质量重采样，
//! 输出与屏幕物理像素一一对应、已按旋转和翻转摆正的缓冲区；界面把它盖在同一位置。
//!
//! 坐标约定与交互路径相同：屏幕像素 X 的中心 X+0.5 对应图片上 (X+0.5-left)/倍率 处（像素边缘坐标），
//! 所以盖上去时与 GPU 显示的位置完全重合。

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::decode::DecodedImage;
use crate::view::{Interpolation, Layout};

/// 一次精确重绘的结果：`image` 放在视口物理像素 (x, y) 处。
#[derive(Debug)]
pub struct Rendered {
    pub x: i64,
    pub y: i64,
    pub image: DecodedImage,
}

/// 交互停止多久后开始精确重绘（毫秒，ARCHITECTURE"两条路径"）。
pub const DELAY_MS: u32 = 100;

/// 锐化强度（"减淡+锐化"）：反锐化掩模的系数。
pub const SHARPEN_AMOUNT: f32 = 0.6;
/// 打开"使用强锐化滤镜"时的系数（默认为普通的 2 倍）。
pub const STRONG_SHARPEN_AMOUNT: f32 = 1.2;

/// 对可见区域重采样。看不到图片时返回 None。
pub fn render(
    source: &DecodedImage,
    layout: &Layout,
    viewport: (f64, f64),
    interpolation: Interpolation,
    sharpen: Option<f32>,
) -> Option<Rendered> {
    let (w, h) = (f64::from(source.width), f64::from(source.height));
    let swapped = layout.rotation % 180 == 90;
    // 旋转、翻转后的图片（屏幕方向）的尺寸和左上角位置。
    let (rw, rh) = if swapped { (h, w) } else { (w, h) };
    let left = layout.center_x - rw * layout.scale / 2.0;
    let top = layout.center_y - rh * layout.scale / 2.0;
    let x0 = left.max(0.0).floor();
    let y0 = top.max(0.0).floor();
    let x1 = (left + rw * layout.scale).min(viewport.0).ceil();
    let y1 = (top + rh * layout.scale).min(viewport.1).ceil();
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (out_w, out_h) = ((x1 - x0) as u32, (y1 - y0) as u32);

    // 可见区域在"屏幕方向的图片"中的范围（像素边缘坐标），再换回原图方向。
    let s = layout.scale;
    let (mut a0, mut a1) = ((x0 - left) / s, (x1 - left) / s);
    let (mut b0, mut b1) = ((y0 - top) / s, (y1 - top) / s);
    if layout.flip_horizontal {
        (a0, a1) = (rw - a1, rw - a0);
    }
    if layout.flip_vertical {
        (b0, b1) = (rh - b1, rh - b0);
    }
    let (u0, u1, v0, v1) = match layout.rotation {
        90 => (b0, b1, h - a1, h - a0),
        180 => (w - a1, w - a0, h - b1, h - b0),
        270 => (w - b1, w - b0, a0, a1),
        _ => (a0, a1, b0, b1),
    };
    // 浮点误差可能让范围略超出原图，收回到原图以内。
    let (u0, u1) = (u0.max(0.0), u1.min(w));
    let (v0, v1) = (v0.max(0.0), v1.min(h));
    if u1 <= u0 || v1 <= v0 {
        return None;
    }
    let (resized_w, resized_h) = if swapped {
        (out_h, out_w)
    } else {
        (out_w, out_h)
    };

    let src = ImageRef::new(source.width, source.height, &source.pixels, PixelType::U8x4).ok()?;
    let mut dst = Image::new(resized_w, resized_h, PixelType::U8x4);
    let filter = match interpolation {
        Interpolation::Bilinear => FilterType::Bilinear,
        Interpolation::Bicubic => FilterType::CatmullRom,
        Interpolation::Lanczos => FilterType::Lanczos3,
    };
    // 像素边缘为"无"时取最近的像素（只有超大图的代理被放大时才会走到这里）。
    let algorithm = if layout.smooth {
        ResizeAlg::Convolution(filter)
    } else {
        ResizeAlg::Nearest
    };
    // 缓冲区已是预乘 Alpha，直接按预乘值插值即可。
    let options = ResizeOptions::new()
        .resize_alg(algorithm)
        .crop(u0, v0, u1 - u0, v1 - v0)
        .use_alpha(false);
    Resizer::new().resize(&src, &mut dst, &options).ok()?;

    let mut pixels = orient(dst.into_vec(), resized_w, resized_h, layout);
    if let Some(amount) = sharpen {
        sharpen_in_place(&mut pixels, out_w as usize, out_h as usize, amount);
    }
    Some(Rendered {
        x: x0 as i64,
        y: y0 as i64,
        image: DecodedImage::new(out_w, out_h, pixels, source.info.clone()),
    })
}

/// 把原图方向的缓冲区转到屏幕方向：先顺时针旋转，再按屏幕方向翻转（与 Layout 的约定相同）。
fn orient(pixels: Vec<u8>, width: u32, height: u32, layout: &Layout) -> Vec<u8> {
    if layout.rotation == 0 && !layout.flip_horizontal && !layout.flip_vertical {
        return pixels;
    }
    let (w, h) = (width as usize, height as usize);
    let (out_w, out_h) = if layout.rotation % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u8; pixels.len()];
    for y in 0..out_h {
        for x in 0..out_w {
            // 屏幕位置先撤销翻转，再撤销旋转，得到原图方向的位置。
            let rx = if layout.flip_horizontal {
                out_w - 1 - x
            } else {
                x
            };
            let ry = if layout.flip_vertical {
                out_h - 1 - y
            } else {
                y
            };
            let (sx, sy) = match layout.rotation {
                90 => (ry, h - 1 - rx),
                180 => (w - 1 - rx, h - 1 - ry),
                270 => (w - 1 - ry, rx),
                _ => (rx, ry),
            };
            let from = (sy * w + sx) * 4;
            let to = (y * out_w + x) * 4;
            out[to..to + 4].copy_from_slice(&pixels[from..from + 4]);
        }
    }
    out
}

/// 反锐化掩模：结果 = 原值 + 系数 ×（原值 − 3×3 模糊），模糊核为 1-2-1 的高斯核，边缘按复制处理。
/// 对预乘值逐通道处理，颜色不超过 Alpha。可分离卷积加整数运算，按行分给几个线程。
fn sharpen_in_place(pixels: &mut [u8], width: usize, height: usize, amount: f32) {
    if width < 3 || height < 3 {
        return;
    }
    let stride = width * 4;
    // 定点系数：amount × 256。
    let amount_q = (amount * 256.0).round() as i32;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 4);
    let rows_per_chunk = height.div_ceil(threads);

    // 横向：h = 左 + 2×中 + 右。
    let mut horizontal = vec![0u16; pixels.len()];
    std::thread::scope(|scope| {
        for (src, dst) in pixels
            .chunks(rows_per_chunk * stride)
            .zip(horizontal.chunks_mut(rows_per_chunk * stride))
        {
            scope.spawn(move || {
                for (row, out) in src.chunks_exact(stride).zip(dst.chunks_exact_mut(stride)) {
                    for i in 0..stride {
                        let left = if i >= 4 { row[i - 4] } else { row[i] };
                        let right = if i + 4 < stride { row[i + 4] } else { row[i] };
                        out[i] = u16::from(left) + 2 * u16::from(row[i]) + u16::from(right);
                    }
                }
            });
        }
    });

    // 纵向并合成：模糊值的 16 倍 = 上 + 2×中 + 下。
    let horizontal = &horizontal;
    std::thread::scope(|scope| {
        for (chunk_index, rows) in pixels.chunks_mut(rows_per_chunk * stride).enumerate() {
            scope.spawn(move || {
                for (offset, row) in rows.chunks_exact_mut(stride).enumerate() {
                    let y = chunk_index * rows_per_chunk + offset;
                    let line = |r: usize| &horizontal[r * stride..(r + 1) * stride];
                    let (up, mid, down) = (
                        line(y.saturating_sub(1)),
                        line(y),
                        line((y + 1).min(height - 1)),
                    );
                    for x in 0..width {
                        let alpha = i32::from(row[x * 4 + 3]);
                        for c in 0..3 {
                            let i = x * 4 + c;
                            let value = i32::from(row[i]);
                            let blur16 =
                                i32::from(up[i]) + 2 * i32::from(mid[i]) + i32::from(down[i]);
                            let sharpened =
                                value + ((amount_q * (16 * value - blur16) + 2048) >> 12);
                            row[i] = sharpened.clamp(0, alpha) as u8;
                        }
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ImageInfo;
    use crate::view::ViewState;

    /// 平滑的渐变图：每个像素的颜色由坐标决定，便于核对位置。
    fn gradient(width: u32, height: u32) -> DecodedImage {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let b = (x * 255 / (width - 1)) as u8;
                let g = (y * 255 / (height - 1)) as u8;
                pixels.extend_from_slice(&[b, g, 128, 255]);
            }
        }
        DecodedImage::new(
            width,
            height,
            pixels,
            ImageInfo {
                format: "PNG".into(),
                pixel_format: "RGB24".into(),
                file_size: 0,
                icc_profile: None,
                modified: None,
                has_alpha: false,
                decode_ms: 0.0,
                shooting: None,
                gif_background: None,
            },
        )
    }

    /// 精确重绘的每个像素应等于原图在 screen_to_image(像素中心) 处的颜色（渐变图上允许插值误差）。
    fn check_alignment(view: &ViewState, source: &DecodedImage, viewport: (f64, f64)) {
        let layout = view.layout().unwrap();
        let rendered = render(source, &layout, viewport, Interpolation::Lanczos, None).unwrap();
        let out = &rendered.image;
        for &(fx, fy) in &[(0.2, 0.3), (0.5, 0.5), (0.8, 0.7), (0.1, 0.9)] {
            let px = (f64::from(out.width) * fx) as u32;
            let py = (f64::from(out.height) * fy) as u32;
            let (sx, sy) = (
                rendered.x as f64 + f64::from(px) + 0.5,
                rendered.y as f64 + f64::from(py) + 0.5,
            );
            let (u, v) = layout.screen_to_image(source.width, source.height, sx, sy);
            let expected_b = (u / f64::from(source.width - 1) * 255.0).clamp(0.0, 255.0);
            let expected_g = (v / f64::from(source.height - 1) * 255.0).clamp(0.0, 255.0);
            let i = ((py * out.width + px) * 4) as usize;
            let (b, g) = (f64::from(out.pixels[i]), f64::from(out.pixels[i + 1]));
            assert!(
                (b - expected_b).abs() <= 3.0 && (g - expected_g).abs() <= 3.0,
                "旋转 {} 翻转 {}/{} 倍率 {}：({px},{py}) 得到 ({b},{g})，应为 ({expected_b:.1},{expected_g:.1})",
                layout.rotation,
                layout.flip_horizontal,
                layout.flip_vertical,
                layout.scale
            );
        }
    }

    #[test]
    fn output_lines_up_with_the_gpu_mapping_in_every_orientation() {
        let source = gradient(400, 300);
        let viewport = (500.0, 450.0);
        let mut view = ViewState::default();
        view.set_viewport(viewport.0, viewport.1);
        view.set_image(source.width, source.height);
        for zoom in [0.37, 0.8, 1.7] {
            view.zoom_to(zoom, None);
            for rotation in 0..4 {
                for flips in 0..4 {
                    if flips & 1 != 0 {
                        view.toggle_flip_horizontal();
                    }
                    if flips & 2 != 0 {
                        view.toggle_flip_vertical();
                    }
                    check_alignment(&view, &source, viewport);
                    if flips & 1 != 0 {
                        view.toggle_flip_horizontal();
                    }
                    if flips & 2 != 0 {
                        view.toggle_flip_vertical();
                    }
                }
                let _ = rotation;
                view.rotate_by(90);
                view.zoom_to(zoom, None);
            }
        }
    }

    #[test]
    fn only_the_visible_part_is_rendered() {
        let source = gradient(400, 300);
        let mut view = ViewState::default();
        view.set_viewport(200.0, 100.0);
        view.set_image(400, 300);
        view.zoom_to(2.0, None);
        let layout = view.layout().unwrap();
        let rendered = render(
            &source,
            &layout,
            (200.0, 100.0),
            Interpolation::Bicubic,
            Some(SHARPEN_AMOUNT),
        )
        .unwrap();
        assert_eq!((rendered.x, rendered.y), (0, 0));
        assert_eq!((rendered.image.width, rendered.image.height), (200, 100));
    }

    /// 性能基准：静止后精确重绘 4K 可见区域不超过 150 ms。手动运行：
    /// `cargo test -p anyview-core --release -- --ignored exact_4k --nocapture`
    #[test]
    #[ignore]
    fn exact_4k_within_t5() {
        let source = gradient(4000, 3000);
        let viewport = (3840.0, 2160.0);
        for (zoom, interpolation, sharpen) in [
            (None, Interpolation::Lanczos, None),
            (None, Interpolation::Lanczos, Some(SHARPEN_AMOUNT)),
            (Some(2.0), Interpolation::Lanczos, Some(SHARPEN_AMOUNT)),
            (Some(0.5), Interpolation::Bicubic, None),
        ] {
            let mut view = ViewState::default();
            view.set_viewport(viewport.0, viewport.1);
            view.set_image(source.width, source.height);
            if let Some(zoom) = zoom {
                view.zoom_to(zoom, None);
            }
            let layout = view.layout().unwrap();
            let start = std::time::Instant::now();
            let out = render(&source, &layout, viewport, interpolation, sharpen).unwrap();
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            println!(
                "精确重绘 倍率 {:.2} {interpolation:?} 锐化 {sharpen:?}：输出 {}x{}，{ms:.1} ms",
                layout.scale, out.image.width, out.image.height
            );
        }
    }

    #[test]
    fn sharpening_raises_local_contrast() {
        // 一条竖直的明暗分界：锐化后亮侧更亮、暗侧更暗。
        let mut pixels = Vec::new();
        for _y in 0..5 {
            for x in 0..6 {
                let v = if x < 3 { 80 } else { 160 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        sharpen_in_place(&mut pixels, 6, 5, SHARPEN_AMOUNT);
        let at = |x: usize, y: usize| pixels[(y * 6 + x) * 4];
        assert!(at(2, 2) < 80 && at(3, 2) > 160, "{} {}", at(2, 2), at(3, 2));
        assert_eq!(at(0, 2), 80);
    }
}
