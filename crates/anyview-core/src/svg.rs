//! SVG、SVGZ：用 resvg 渲染（纯 Rust）。
//!
//! - 先按文件写的尺寸（长边超过 [`MAX_SIDE`] 时按比例缩小）渲染成普通图片，窗口、缩放都按这个尺寸。
//! - 放大、缩小或在 Retina 屏上显示时，精确重绘直接按屏幕像素重新渲染可见区域，矢量图始终清晰。
//! - 不读取外部资源：只认 `data:` 内嵌的图片，链接到别的文件的一律不读。
//! - 有文字时才加载系统字体（第一次要几百毫秒，之后复用）。通用字体名（sans-serif 等）对应到这台电脑上装了的字体。

use crate::i18n::Message;
use std::sync::{Arc, OnceLock};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, Options, Tree};

use crate::decode::{DecodedImage, Raster};
use crate::view::Layout;

/// 普通图片的长边上限（像素）：文件写的尺寸更大时按比例缩小，放大看细节时由精确重绘补上。
pub const MAX_SIDE: f32 = 4096.0;

/// 扩展名是 SVG。
pub fn is_svg_extension(extension: &str) -> bool {
    matches!(extension, "svg" | "svgz")
}

fn options(fonts: Option<Arc<usvg::fontdb::Database>>) -> Options<'static> {
    let mut opt = Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            // 链接到别的文件的图片不读。
            resolve_string: Box::new(|_, _| None),
        },
        ..Options::default()
    };
    if let Some(fonts) = fonts {
        opt.fontdb = fonts;
    }
    opt
}

/// 系统字体（第一次用时加载）。
fn system_fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    Arc::clone(FONTS.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        set_generic_families(&mut db);
        Arc::new(db)
    }))
}

/// 通用字体名对应到这台电脑上真有的字体：fontdb 默认按 Windows 的名字（Arial、Times New Roman、Courier New），
/// Linux 上一般没有，写 sans-serif 的文字就整个不显示。usvg 找不到字体时最后用 serif 那一种，
/// 所以 serif 一定要对应到装了的字体（实在没有就用 sans-serif 那一种或随便一种）。
fn set_generic_families(db: &mut usvg::fontdb::Database) {
    let installed: std::collections::HashSet<String> = db
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .collect();
    let pick = |names: &[&str]| names.iter().find(|n| installed.contains(**n)).map(|n| (*n).to_string());
    let sans = pick(&["Arial", "Helvetica", "Liberation Sans", "Noto Sans", "DejaVu Sans", "Cantarell"]);
    let serif = pick(&["Times New Roman", "Times", "Liberation Serif", "Noto Serif", "DejaVu Serif"])
        .or_else(|| sans.clone())
        .or_else(|| installed.iter().min().cloned());
    let mono = pick(&["Courier New", "Menlo", "Courier", "Liberation Mono", "Noto Sans Mono", "DejaVu Sans Mono"]);
    if let Some(name) = sans {
        db.set_sans_serif_family(name);
    }
    if let Some(name) = serif {
        db.set_serif_family(name);
    }
    if let Some(name) = mono {
        db.set_monospace_family(name);
    }
}

/// 文件里有没有文字元素（`<text`，或带命名空间前缀的 `:text`）。不能先不带字体解析、再看有没有文字：
/// 没有字体时 usvg 把排不了版的文字整个丢掉（以前因此 SVG 里的文字一直不显示）。多认了也只是多加载一次字体。
fn mentions_text(xml: &[u8]) -> bool {
    xml.windows(6).any(|w| {
        (w[0] == b'<' || w[0] == b':')
            && &w[1..5] == b"text"
            && matches!(w[5], b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/')
    })
}

/// SVGZ 解压后最多这么大（64 MB）。gzip 能把一串重复的字节压到千分之一：不设上限的话，十几 MB 的文件
/// 解出来十几 GB，把内存用光。正常的 SVG 再复杂也只有几十 MB。
const MAX_SVGZ_XML: u64 = 64 << 20;

/// 现在的上限；用户选了"仍然打开"的不设。
fn svgz_limit() -> u64 {
    if crate::decode::limits_lifted() {
        u64::MAX
    } else {
        MAX_SVGZ_XML.min(crate::decode::alloc_limit())
    }
}

/// 解压 SVGZ（gzip），超过 `limit` 字节的报错。
fn gunzip(data: &[u8], limit: u64) -> Result<Vec<u8>, Message> {
    use std::io::Read;
    let mut xml = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(limit.saturating_add(1))
        .read_to_end(&mut xml)
        .map_err(|e| e.to_string())?;
    if xml.len() as u64 > limit {
        crate::decode::limit_hit();
        return Err(msg!("SVGZ 解压后太大", "The SVGZ is too large once decompressed"));
    }
    Ok(xml)
}

/// 解析：有文字时带上系统字体（SVGZ 先解压再看）。
pub fn parse(data: &[u8]) -> Result<Tree, Message> {
    let xml = if data.starts_with(&[0x1f, 0x8b]) {
        std::borrow::Cow::Owned(gunzip(data, svgz_limit())?)
    } else {
        std::borrow::Cow::Borrowed(data)
    };
    let fonts = mentions_text(&xml).then(system_fonts);
    Tree::from_data(&xml, &options(fonts)).map_err(|e| Message::raw(e.to_string()))
}

/// 普通图片的尺寸：文件写的尺寸，长边不超过 [`MAX_SIDE`]，至少 1 像素。
pub fn base_size(tree: &Tree) -> (u32, u32) {
    let size = tree.size();
    let (w, h) = (size.width().max(1e-3), size.height().max(1e-3));
    let k = (MAX_SIDE / w.max(h)).min(1.0);
    (
        ((w * k).round() as u32).max(1),
        ((h * k).round() as u32).max(1),
    )
}

/// 按普通图片的尺寸渲染。
pub fn decode(data: &[u8]) -> Result<Raster, Message> {
    let tree = parse(data)?;
    let (width, height) = base_size(&tree);
    render_at(&tree, width, height)
}

/// 缩略图：长边缩放到 `max_side`（不超过 [`MAX_SIDE`]）。比文件写的尺寸小的也放大到这个尺寸：
/// 矢量图（如 24x24 的图标）按要的尺寸渲染才清晰，不先按小尺寸渲染再让访达放大。
pub fn render_fit(data: &[u8], max_side: u32) -> Result<Raster, Message> {
    let tree = parse(data)?;
    let size = tree.size();
    let (w, h) = (size.width().max(1e-3), size.height().max(1e-3));
    let k = (max_side.clamp(1, MAX_SIDE as u32) as f32) / w.max(h);
    render_at(&tree, ((w * k).round() as u32).max(1), ((h * k).round() as u32).max(1))
}

/// 把整张图渲染成 `width`x`height`（未预乘的 RGBA）。
fn render_at(tree: &Tree, width: u32, height: u32) -> Result<Raster, Message> {
    let mut rgba = Raster::alloc(width, height)?;
    let mut pixmap = Pixmap::new(width, height).ok_or(msg!("SVG 尺寸不对", "Invalid SVG size"))?;
    let size = tree.size();
    let transform = Transform::from_scale(width as f32 / size.width(), height as f32 / size.height());
    resvg::render(tree, transform, &mut pixmap.as_mut());
    // tiny-skia 是预乘的 RGBA；Raster 要未预乘的。
    for (out, p) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(pixmap.data().as_chunks::<4>().0) {
        let a = p[3];
        if a == 0 {
            continue;
        }
        for c in 0..3 {
            out[c] = ((u32::from(p[c]) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
        }
        out[3] = a;
    }
    Ok(Raster {
        width,
        height,
        rgba,
        has_alpha: true,
        format: "SVG".to_string(),
        pixel_format: "Vector".to_string(),
        icc_profile: None,
    })
}

/// 精确重绘：按当前的位置、倍率、旋转和翻转，把可见区域直接按屏幕像素重新渲染。
/// `source` 是这张 SVG 的普通图片（取尺寸和信息），`data` 是文件内容。看不到图片时为空。
pub fn render_region(
    data: &[u8],
    source: &DecodedImage,
    layout: &Layout,
    viewport: (f64, f64),
) -> Option<crate::exact::Rendered> {
    let (w, h) = (f64::from(source.width), f64::from(source.height));
    let swapped = layout.rotation % 180 == 90;
    let (rw, rh) = if swapped { (h, w) } else { (w, h) };
    let s = layout.scale;
    let left = layout.center_x - rw * s / 2.0;
    let top = layout.center_y - rh * s / 2.0;
    let x0 = left.max(0.0).floor();
    let y0 = top.max(0.0).floor();
    let x1 = (left + rw * s).min(viewport.0).ceil();
    let y1 = (top + rh * s).min(viewport.1).ceil();
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (out_w, out_h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let tree = parse(data).ok()?;
    let size = tree.size();
    // 文件坐标 → 图片像素 → 旋转（顺时针）→ 翻转（屏幕方向）→ 屏幕 → 输出缓冲区。
    let user = Affine::scale(w / f64::from(size.width()), h / f64::from(size.height()));
    let rotate = match layout.rotation {
        90 => Affine([0.0, -1.0, h, 1.0, 0.0, 0.0]),
        180 => Affine([-1.0, 0.0, w, 0.0, -1.0, h]),
        270 => Affine([0.0, 1.0, 0.0, -1.0, 0.0, w]),
        _ => Affine::IDENTITY,
    };
    let flip_h = if layout.flip_horizontal {
        Affine([-1.0, 0.0, rw, 0.0, 1.0, 0.0])
    } else {
        Affine::IDENTITY
    };
    let flip_v = if layout.flip_vertical {
        Affine([1.0, 0.0, 0.0, 0.0, -1.0, rh])
    } else {
        Affine::IDENTITY
    };
    let screen = Affine([s, 0.0, left - x0, 0.0, s, top - y0]);
    let m = screen.then_after(flip_v).then_after(flip_h).then_after(rotate).then_after(user);
    let mut pixmap = Pixmap::new(out_w, out_h)?;
    resvg::render(&tree, m.to_tiny_skia(), &mut pixmap.as_mut());
    // 预乘 RGBA → 预乘 BGRA。
    let mut pixels = pixmap.take();
    for p in pixels.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Some(crate::exact::Rendered {
        x: x0 as i64,
        y: y0 as i64,
        image: DecodedImage::new(out_w, out_h, pixels, source.info.clone()),
    })
}

/// 平面仿射变换 [a b c; d e f]：x' = a·x + b·y + c，y' = d·x + e·y + f。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine([f64; 6]);

impl Affine {
    const IDENTITY: Self = Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    fn scale(x: f64, y: f64) -> Self {
        Self([x, 0.0, 0.0, 0.0, y, 0.0])
    }

    /// 先做 `inner`，再做 `self`。
    fn then_after(self, inner: Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [p, q, r, s, t, u] = inner.0;
        Self([
            a * p + b * s,
            a * q + b * t,
            a * r + b * u + c,
            d * p + e * s,
            d * q + e * t,
            d * r + e * u + f,
        ])
    }

    #[cfg(test)]
    fn apply(self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + b * y + c, d * x + e * y + f)
    }

    fn to_tiny_skia(self) -> Transform {
        let [a, b, c, d, e, f] = self.0;
        Transform::from_row(a as f32, d as f32, b as f32, e as f32, c as f32, f as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ImageInfo;

    /// 四色块的 SVG：左上红、右上绿、左下蓝、右下黄（与测试图片相同的颜色）。
    pub const QUADRANTS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
<rect x="0" y="0" width="100" height="50" fill="#DC2828"/>
<rect x="100" y="0" width="100" height="50" fill="#28AA46"/>
<rect x="0" y="50" width="100" height="50" fill="#285ADC"/>
<rect x="100" y="50" width="100" height="50" fill="#F0C828"/>
</svg>"##;

    fn source(raster: &Raster) -> DecodedImage {
        DecodedImage::new(
            raster.width,
            raster.height,
            vec![0; (raster.width * raster.height * 4) as usize],
            ImageInfo {
                format: "SVG".into(),
                pixel_format: "Vector".into(),
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

    fn bgra(image: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
        let at = y as usize * image.stride + x as usize * 4;
        image.pixels[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn renders_at_the_written_size() {
        let raster = decode(QUADRANTS.as_bytes()).unwrap();
        assert_eq!((raster.width, raster.height), (200, 100));
        let at = |x: usize, y: usize| raster.rgba[(y * 200 + x) * 4..(y * 200 + x) * 4 + 4].to_vec();
        assert_eq!(at(50, 25), [0xDC, 0x28, 0x28, 255]);
        assert_eq!(at(150, 75), [0xF0, 0xC8, 0x28, 255]);
        assert!(decode(b"<svg").is_err());
        assert!(decode(b"not svg at all").is_err());
    }

    #[test]
    fn text_is_drawn_with_installed_fonts() {
        // 以前先不带字体解析、没看到文字就不加载字体，可没有字体时 usvg 已经把文字丢掉了：文字从来没画出来过。
        // sans-serif 要对应到装了的字体（Linux 上没有 Arial）。没有任何字体的电脑上跳过。
        if system_fonts().is_empty() {
            return;
        }
        for family in ["sans-serif", "serif", "monospace", "NoSuchFont"] {
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="60"><rect width="240" height="60" fill="#FFFFFF"/>
<text x="12" y="40" font-family="{family}" font-size="28" fill="#000000">Anyview SVG</text></svg>"##
            );
            let raster = decode(svg.as_bytes()).unwrap();
            let dark = raster.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 128 && p[0] < 100).count();
            assert!(dark > 100, "{family}：文字没画出来（只有 {dark} 个深色像素）");
        }
        assert!(mentions_text(b"<svg><svg:text x='1'>a</svg:text></svg>"));
        assert!(!mentions_text(b"<svg><rect/><textual/></svg>"));
    }

    #[test]
    fn svgz_and_huge_sizes() {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(QUADRANTS.as_bytes()).unwrap();
        let raster = decode(&gz.finish().unwrap()).unwrap();
        assert_eq!((raster.width, raster.height), (200, 100));
        let huge = QUADRANTS.replace(r#"width="200" height="100""#, r#"width="20000" height="10000" viewBox="0 0 200 100""#);
        let raster = decode(huge.as_bytes()).unwrap();
        assert_eq!((raster.width, raster.height), (4096, 2048));
    }

    #[test]
    fn external_files_are_not_read() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10">
<image width="10" height="10" xlink:href="file:///etc/passwd"/><image width="10" height="10" href="/etc/hostname"/></svg>"#;
        let raster = decode(svg.as_bytes()).unwrap();
        assert!(raster.rgba.iter().all(|&b| b == 0), "外部文件不应被读出来画上");
    }

    #[test]
    fn exact_render_follows_zoom_rotation_and_flips() {
        let data = QUADRANTS.as_bytes();
        let raster = decode(data).unwrap();
        let source = source(&raster);
        // 放大 4 倍、居中在 1000x600 的视口里：左上那块的中心 (50,25) → (500-400+200, 300-200+100)。
        let layout = Layout {
            center_x: 500.0,
            center_y: 300.0,
            width: 800.0,
            height: 400.0,
            scale: 4.0,
            zoom: 4.0,
            rotation: 0,
            flip_horizontal: false,
            flip_vertical: false,
            smooth: true,
        };
        let r = render_region(data, &source, &layout, (1000.0, 600.0)).unwrap();
        assert_eq!((r.x, r.y, r.image.width, r.image.height), (100, 100, 800, 400));
        assert_eq!(bgra(&r.image, 200, 100), [0x28, 0x28, 0xDC, 255]); // 红（BGRA）
        assert_eq!(bgra(&r.image, 600, 300), [0x28, 0xC8, 0xF0, 255]); // 黄
        // 顺时针转 90°：左上角变成原图的左下（蓝）。
        let rotated = Layout {
            rotation: 90,
            ..layout
        };
        let r = render_region(data, &source, &rotated, (1000.0, 600.0)).unwrap();
        assert_eq!((r.image.width, r.image.height), (400, 600));
        assert_eq!(bgra(&r.image, 50, 50), [0xDC, 0x5A, 0x28, 255]);
        // 再左右翻转：左上角是原图的左上（红）。
        let flipped = Layout {
            flip_horizontal: true,
            ..rotated
        };
        let r = render_region(data, &source, &flipped, (1000.0, 600.0)).unwrap();
        assert_eq!(bgra(&r.image, 50, 50), [0x28, 0x28, 0xDC, 255]);
        let m = Affine::scale(2.0, 3.0).then_after(Affine([1.0, 0.0, 5.0, 0.0, 1.0, 7.0]));
        assert_eq!(m.apply(1.0, 1.0), (12.0, 24.0));
    }

    /// SVGZ 解压有上限：超过的报错，不把整个内容解出来。
    #[test]
    fn svgz_bombs_are_refused() {
        use std::io::Write;
        let gz = |xml: &[u8]| {
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(xml).unwrap();
            encoder.finish().unwrap()
        };
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>"#;
        assert_eq!(gunzip(&gz(svg), 1024).unwrap(), svg);
        assert_eq!(gunzip(&gz(svg), svg.len() as u64).unwrap(), svg);
        assert!(gunzip(&gz(svg), svg.len() as u64 - 1).is_err());
        assert!(parse(&gz(svg)).is_ok());
        // 80 MB 的空格压缩后只有几十 KB。
        let bomb = gz(&vec![b' '; 80 << 20]);
        assert!(bomb.len() < 1 << 20, "{}", bomb.len());
        assert_eq!(parse(&bomb).err().map(|e| e.text(crate::i18n::Lang::ZhHans)).as_deref(), Some("SVGZ 解压后太大"));
    }
}
