//! 智能双页阅览：两张竖图并排成一页显示，横图单独一页。
//!
//! 核心把一组两页合成一张图片交给界面（界面照常显示一张图：缩放、旋转、窗口跟着图片、精确重绘都不用改）。
//! 两页高度不同时，矮的一页按比例放大到同样高。阅读方向决定前一页在左边（左→右）还是右边（左←右，日本漫画）。

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::decode::DecodedImage;

/// 阅读方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// 左→右：前一页在左边。
    Ltr,
    /// 左←右：前一页在右边。
    Rtl,
}

impl Direction {
    pub fn id(self) -> &'static str {
        match self {
            Direction::Ltr => "ltr",
            Direction::Rtl => "rtl",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "ltr" => Some(Direction::Ltr),
            "rtl" => Some(Direction::Rtl),
            _ => None,
        }
    }
}

/// 横图（宽大于高）单独占一页。
pub fn is_landscape(image: &DecodedImage) -> bool {
    image.width > image.height
}

/// 把两页合成一张：`first` 是排在前面的一页。高度取两页中较高的，矮的一页按比例放大（Lanczos）。
pub fn compose(first: &DecodedImage, second: &DecodedImage, direction: Direction) -> DecodedImage {
    let height = first.height.max(second.height);
    let first = fit_height(first, height);
    let second = fit_height(second, height);
    let (left, right) = match direction {
        Direction::Ltr => (&first, &second),
        Direction::Rtl => (&second, &first),
    };
    let width = left.width + right.width;
    let stride = width as usize * 4;
    let mut pixels = vec![0u8; stride * height as usize];
    for (image, x0) in [(left, 0usize), (right, left.width as usize)] {
        let row = image.width as usize * 4;
        for y in 0..image.height as usize {
            let src = &image.pixels[y * image.stride..y * image.stride + row];
            let at = y * stride + x0 * 4;
            pixels[at..at + row].copy_from_slice(src);
        }
    }
    let mut info = first.info.clone();
    info.has_alpha = first.info.has_alpha || second.info.has_alpha;
    info.decode_ms = first.info.decode_ms.max(second.info.decode_ms);
    DecodedImage::new(width, height, pixels, info)
}

/// 按比例缩放到指定高度（已经是这个高度时原样复制）。
fn fit_height(image: &DecodedImage, height: u32) -> DecodedImage {
    if image.height == height {
        return copy(image);
    }
    let width = ((f64::from(image.width) * f64::from(height) / f64::from(image.height)).round() as u32).max(1);
    let packed = packed_pixels(image);
    let resized = ImageRef::new(image.width, image.height, &packed, PixelType::U8x4)
        .ok()
        .and_then(|src| {
            let mut dst = Image::new(width, height, PixelType::U8x4);
            // 缓冲区是预乘 Alpha，直接插值。
            let options = ResizeOptions::new()
                .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
                .use_alpha(false);
            Resizer::new().resize(&src, &mut dst, &options).ok()?;
            Some(dst.into_vec())
        });
    match resized {
        Some(pixels) => DecodedImage::new(width, height, pixels, image.info.clone()),
        None => copy(image),
    }
}

/// 每行紧挨着的像素（去掉行尾的空隙）。
fn packed_pixels(image: &DecodedImage) -> Vec<u8> {
    let row = image.width as usize * 4;
    if image.stride == row {
        return image.pixels.clone();
    }
    (0..image.height as usize)
        .flat_map(|y| image.pixels[y * image.stride..y * image.stride + row].iter().copied())
        .collect()
}

fn copy(image: &DecodedImage) -> DecodedImage {
    DecodedImage::new(image.width, image.height, packed_pixels(image), image.info.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ImageInfo;

    fn page(width: u32, height: u32, color: [u8; 4]) -> DecodedImage {
        let pixels = color.repeat((width * height) as usize);
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

    fn pixel(image: &DecodedImage, x: usize, y: usize) -> [u8; 4] {
        let at = y * image.stride + x * 4;
        image.pixels[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn pages_sit_side_by_side_in_reading_order() {
        let red = [0, 0, 255, 255];
        let blue = [255, 0, 0, 255];
        let first = page(4, 6, red);
        let second = page(4, 6, blue);
        let ltr = compose(&first, &second, Direction::Ltr);
        assert_eq!((ltr.width, ltr.height), (8, 6));
        assert_eq!(pixel(&ltr, 0, 0), red);
        assert_eq!(pixel(&ltr, 7, 5), blue);
        let rtl = compose(&first, &second, Direction::Rtl);
        assert_eq!(pixel(&rtl, 0, 0), blue);
        assert_eq!(pixel(&rtl, 7, 5), red);
    }

    #[test]
    fn the_shorter_page_is_scaled_to_the_same_height() {
        let first = page(4, 8, [10, 20, 30, 255]);
        let second = page(2, 4, [40, 50, 60, 255]);
        let spread = compose(&first, &second, Direction::Ltr);
        assert_eq!((spread.width, spread.height), (8, 8));
        assert_eq!(pixel(&spread, 6, 7), [40, 50, 60, 255]);
        assert!(is_landscape(&page(5, 4, [0; 4])) && !is_landscape(&first));
    }
}
