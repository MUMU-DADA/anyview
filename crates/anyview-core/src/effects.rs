//! 图片效果（ARCHITECTURE"效果与透明"）：反相、Gamma 补偿，以及按显示器的 ICC 配置文件转换颜色
//! （设置"使用监视器的 ICC 设置"，；Windows、Linux 的 X11）。
//! 作用于显示用的缓冲区，不改原图；切换时由核心重新生成，交互路径和精确重绘共用结果。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::decode::DecodedImage;

/// Gamma 补偿的 γ：大于 1 时提亮暗部。
pub const GAMMA: f64 = 1.5;

/// 登记过的显示器配置文件的编号（内容的哈希，见 [`register_display_profile`]）。
pub type DisplayProfileId = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Effects {
    pub invert: bool,
    pub gamma: bool,
    /// 把 sRGB 转到这个显示器配置文件（最后一步，在反相、Gamma 之后）；为空时不转。
    pub display: Option<DisplayProfileId>,
}

impl Effects {
    pub const NONE: Self = Self {
        invert: false,
        gamma: false,
        display: None,
    };

    pub fn is_none(self) -> bool {
        self == Self::NONE
    }
}

/// sRGB → 显示器配置文件的转换，按编号存（各窗口共用；一台机器上只有几块显示器，不清理）。
fn display_transforms() -> &'static Mutex<HashMap<DisplayProfileId, Arc<qcms::Transform>>> {
    static TRANSFORMS: OnceLock<Mutex<HashMap<DisplayProfileId, Arc<qcms::Transform>>>> = OnceLock::new();
    TRANSFORMS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 登记显示器的配置文件，返回它的编号。读不懂的配置文件、与 sRGB 一样的（转了也不变）返回空：不用转。
pub fn register_display_profile(icc: &[u8]) -> Option<DisplayProfileId> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    icc.hash(&mut hasher);
    let id = hasher.finish();
    let mut transforms = display_transforms().lock().unwrap_or_else(|e| e.into_inner());
    if transforms.contains_key(&id) {
        return Some(id);
    }
    let transform = crate::color::srgb_to_display(icc)?;
    transforms.insert(id, Arc::new(transform));
    Some(id)
}

fn display_transform(id: DisplayProfileId) -> Option<Arc<qcms::Transform>> {
    display_transforms()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
}

/// 每个颜色值（不预乘）经过效果后的值：先反相，再做 Gamma 补偿（Gamma 作为最后的显示校正）。
fn lookup_table(effects: Effects) -> [u8; 256] {
    let mut table = [0u8; 256];
    for (value, slot) in table.iter_mut().enumerate() {
        let mut x = value as f64 / 255.0;
        if effects.invert {
            x = 1.0 - x;
        }
        if effects.gamma {
            x = x.powf(1.0 / GAMMA);
        }
        *slot = (x * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    table
}

/// 按效果生成新的显示缓冲区（BGRA8 预乘，尺寸与信息不变；超大图的代理纹理一并处理）。没有效果时原样复制。
///
/// 图片信息不变（GIF 的背景色等元数据是原样的：动图的每一帧都从原图的信息开始，界面要背景色时由
/// [`apply_to_color`] 按当前效果换算，见 `AppCore::alpha_fill`）。超大图的代理纹理从处理好的原图重新缩小
/// （先缩小再做非线性的 Gamma、显示器转换，与精确重绘从处理好的原图缩小的结果不同，静止后画面会跳）。
pub fn apply(image: &DecodedImage, effects: Effects) -> DecodedImage {
    let mut pixels = image.pixels.clone();
    let table = (effects.invert || effects.gamma).then(|| lookup_table(effects));
    let transform = effects.display.and_then(display_transform);
    if let Some(table) = &table {
        apply_table(&mut pixels, table);
    }
    if let Some(transform) = &transform {
        crate::color::transform_premultiplied_bgra(transform, &mut pixels);
    }
    let processed = DecodedImage {
        width: image.width,
        height: image.height,
        stride: image.stride,
        pixels,
        info: image.info.clone(),
        proxy: None,
        vector: image.vector.clone(),
        generation: image.generation.clone(),
    };
    if image.proxy.is_some() {
        crate::proxy::attach(processed)
    } else {
        processed
    }
}

/// 一种颜色（0xRRGGBB）按效果换算：GIF 文件里写的背景色（透明处显示它）与图像的颜色一致。
pub fn apply_to_color(rgb: u32, effects: Effects) -> u32 {
    if effects.is_none() {
        return rgb;
    }
    let mut px = [(rgb & 0xFF) as u8, (rgb >> 8 & 0xFF) as u8, (rgb >> 16 & 0xFF) as u8, 255];
    if effects.invert || effects.gamma {
        apply_table(&mut px, &lookup_table(effects));
    }
    if let Some(transform) = effects.display.and_then(display_transform) {
        crate::color::transform_premultiplied_bgra(&transform, &mut px);
    }
    u32::from(px[2]) << 16 | u32::from(px[1]) << 8 | u32::from(px[0])
}

/// 每个颜色通道查表（BGRA8 预乘）。
fn apply_table(pixels: &mut [u8], table: &[u8; 256]) {
    let (chunks, _) = pixels.as_chunks_mut::<4>();
    for pixel in chunks {
        let alpha = pixel[3];
        match alpha {
            // 全透明：预乘后的颜色都是 0，效果不改变它。
            0 => {}
            255 => {
                for channel in &mut pixel[..3] {
                    *channel = table[usize::from(*channel)];
                }
            }
            _ => {
                // 半透明：先还原成不预乘的颜色，查表后再预乘回去。
                let a = u32::from(alpha);
                for channel in &mut pixel[..3] {
                    let straight = ((u32::from(*channel) * 255 + a / 2) / a).min(255);
                    let mapped = u32::from(table[straight as usize]);
                    *channel = ((mapped * a + 127) / 255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ImageInfo;

    fn image(pixels: Vec<u8>) -> DecodedImage {
        let width = (pixels.len() / 4) as u32;
        DecodedImage::new(
            width,
            1,
            pixels,
            ImageInfo {
                format: "PNG".into(),
                pixel_format: "RGBA32".into(),
                file_size: 0,
                icc_profile: None,
                modified: None,
                has_alpha: true,
                decode_ms: 0.0,
                shooting: None,
                gif_background: None,
            },
        )
    }

    #[test]
    fn invert_and_gamma_on_opaque_pixels() {
        // BGRA：一个黑色、一个白色、一个中灰。
        let source = image(vec![0, 0, 0, 255, 255, 255, 255, 255, 64, 128, 192, 255]);
        let inverted = apply(
            &source,
            Effects {
                invert: true,
                ..Effects::NONE
            },
        );
        assert_eq!(&inverted.pixels[..8], &[255, 255, 255, 255, 0, 0, 0, 255]);
        assert_eq!(&inverted.pixels[8..], &[191, 127, 63, 255]);
        let brighter = apply(
            &source,
            Effects {
                gamma: true,
                ..Effects::NONE
            },
        );
        // 暗部提亮：64 → 255·(64/255)^(1/1.5) ≈ 101；黑白不变。
        assert_eq!(&brighter.pixels[..8], &[0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(brighter.pixels[8], 101);
        assert!(apply(&source, Effects::NONE).pixels == source.pixels);
    }

    /// 显示器配置文件：测试用的线性 Gamma 配置文件当"显示器"，sRGB 的 128 转过去约为 55（线性 0.216）；
    /// 与 sRGB 一样的配置文件不用转；半透明的像素转完仍是预乘的。
    #[test]
    fn display_profile_converts_from_srgb() {
        assert!(register_display_profile(&crate::color::srgb_test_profile()).is_none());
        assert!(register_display_profile(b"not a profile").is_none());
        let id = register_display_profile(&crate::color::linear_srgb_test_profile()).unwrap();
        assert_eq!(register_display_profile(&crate::color::linear_srgb_test_profile()), Some(id));
        let source = image(vec![128, 128, 128, 255, 0, 0, 0, 0, 64, 64, 64, 128, 255, 255, 255, 255]);
        let shown = apply(
            &source,
            Effects {
                display: Some(id),
                ..Effects::NONE
            },
        );
        assert!((53..=57).contains(&shown.pixels[0]), "{:?}", &shown.pixels[..4]);
        assert_eq!(shown.pixels[3], 255);
        assert_eq!(&shown.pixels[4..8], &[0, 0, 0, 0]);
        // 半透明（预乘 64/128 = 不预乘 128）：转成约 55，再预乘回去约 27。
        assert!((25..=29).contains(&shown.pixels[8]) && shown.pixels[11] == 128, "{:?}", &shown.pixels[8..12]);
        assert!(shown.pixels[12] >= 254);
        // 反相之后再转：白色反相成黑色，黑色不变。
        let both = apply(
            &source,
            Effects {
                invert: true,
                display: Some(id),
                ..Effects::NONE
            },
        );
        assert!(both.pixels[12] <= 1);
    }

    /// GIF 的背景色跟着效果换算（反相后白变黑）；图片信息里的原值不变，动图的帧不会重复换算。
    #[test]
    fn gif_background_follows_the_effects() {
        let invert = Effects {
            invert: true,
            ..Effects::NONE
        };
        assert_eq!(apply_to_color(0xFFFFFF, invert), 0x000000);
        assert_eq!(apply_to_color(0x123456, Effects::NONE), 0x123456);
        let mut source = image(vec![255, 255, 255, 255]);
        source.info.gif_background = Some(0xFFFFFF);
        assert_eq!(apply(&source, invert).info.gif_background, Some(0xFFFFFF));
    }

    #[test]
    fn transparency_is_kept_and_colors_stay_premultiplied() {
        // 全透明不变；半透明的白色（预乘后为 128）反相后变成黑色（0）。
        let source = image(vec![0, 0, 0, 0, 128, 128, 128, 128]);
        let inverted = apply(
            &source,
            Effects {
                invert: true,
                ..Effects::NONE
            },
        );
        assert_eq!(inverted.pixels, vec![0, 0, 0, 0, 0, 0, 0, 128]);
        for (out, alpha) in inverted.pixels.chunks(4).map(|p| (p[0], p[3])) {
            assert!(out <= alpha, "预乘后的颜色不能超过 Alpha");
        }
    }
}
