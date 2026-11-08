//! 超大图的代理纹理（ARCHITECTURE"超大图"）：任一边超过 GPU 纹理上限的图片，
//! 交给 GPU 的是缩小的代理，界面把它拉伸到原图尺寸显示；查看计算仍按原图。
//! 放大到代理的分辨率不够时（倍率大于代理与原图之比），由精确重绘从原图补上清晰的画面。

use std::sync::Arc;

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::decode::DecodedImage;

/// GPU 纹理的边长上限：Apple 芯片的 Metal、常见的 OpenGL、Direct3D 11 都是 16384。
pub const MAX_TEXTURE: u32 = 16384;
/// 代理纹理的长边：够缩小看全图；放大看细节时由精确重绘负责。
pub const PROXY_LONG_SIDE: u32 = 8192;

/// 这样的尺寸要不要代理。
pub fn needed(width: u32, height: u32) -> bool {
    width > MAX_TEXTURE || height > MAX_TEXTURE
}

/// 超大图附上代理纹理；其他图片原样返回。
pub fn attach(mut image: DecodedImage) -> DecodedImage {
    if image.proxy.is_none() && needed(image.width, image.height) {
        image.proxy = make(&image).map(Arc::new);
    }
    image
}

/// 代理与原图的尺寸比（没有代理时为空）。
pub fn ratio(image: &DecodedImage) -> Option<f64> {
    image
        .proxy
        .as_ref()
        .map(|p| f64::from(p.width) / f64::from(image.width.max(1)))
}

fn make(image: &DecodedImage) -> Option<DecodedImage> {
    let long = image.width.max(image.height);
    let ratio = f64::from(PROXY_LONG_SIDE) / f64::from(long);
    let width = ((f64::from(image.width) * ratio).round() as u32).max(1);
    let height = ((f64::from(image.height) * ratio).round() as u32).max(1);
    let src = ImageRef::new(image.width, image.height, &image.pixels, PixelType::U8x4).ok()?;
    let mut dst = Image::new(width, height, PixelType::U8x4);
    // 缓冲区已是预乘 Alpha，直接插值。缩小时双线性卷积按比例展宽，相当于面积平均。
    let options = ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
        .use_alpha(false);
    Resizer::new().resize(&src, &mut dst, &options).ok()?;
    Some(DecodedImage::new(
        width,
        height,
        dst.into_vec(),
        image.info.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ImageInfo;

    fn image(width: u32, height: u32) -> DecodedImage {
        let pixels = (0..width * height)
            .flat_map(|i| [(i % 200) as u8, 50, 100, 255])
            .collect();
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

    #[test]
    fn only_oversized_images_get_a_proxy() {
        assert!(!needed(16384, 16384));
        assert!(needed(16385, 10) && needed(10, 20000));
        let normal = attach(image(300, 200));
        assert!(normal.proxy.is_none() && ratio(&normal).is_none());
        let wide = attach(image(16800, 40));
        let proxy = wide.proxy.as_ref().expect("应有代理");
        assert_eq!((proxy.width, proxy.height), (8192, 20));
        assert_eq!(proxy.stride, 8192 * 4);
        let r = ratio(&wide).unwrap();
        assert!((r - 8192.0 / 16800.0).abs() < 1e-9);
        // 占用的内存算上代理。
        assert_eq!(wide.byte_len(), (16800 * 40 + 8192 * 20) * 4);
        // 颜色保持（不透明处 Alpha 仍为 255）。
        assert!(proxy.pixels.chunks(4).all(|p| p[3] == 255 && p[1] == 50));
    }
}
