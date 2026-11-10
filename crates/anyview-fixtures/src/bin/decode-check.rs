//! 用别的软件存出来的真实文件检查自写或新接入的解码器（PSD、DDS、PCX、RAW 预览、JPEG 2000、Clip Studio Paint）：
//! 逐个解码，记下结果、格式、尺寸和耗时；PSD 与文件里 Photoshop 自己存的缩略图比对，
//! 旁边有同名 PNG 的（如 Pillow 的 DDS 测试图）与它逐像素比对；再把解码结果拼成总览图，供人眼核对。
//!
//! 用法：`decode-check --out 目录 [--pairs] 文件或目录…`（目录里按扩展名找 PSD、PSB、DDS、PCX、JPEG 2000、CLIP 和各家 RAW）；
//! 差得多的 PSD 另存左右对照图 `diff-序号.png`，加 `--pairs` 时每个有缩略图的 PSD 都存。
//! 加 `--psd-layers` 时只看 PSD：把图层自己合成出来，与文件里存的合成图比（输出 layers.tsv 和对照图 layers-序号.png）。
//! 加 `--full-raw` 时 RAW 不用内嵌预览，按设置"显示已有缩略图"关掉时那样完整解码。
//! 输出：`目录/report.tsv`（每行一个文件），`目录/sheet-N.png`（每张 8×8 格，第 k 个文件在第 k/64+1 张、
//! 第 (k%64)/8 行、第 k%8 列；解不出来的格子是红色）。

use std::fs;
use std::io::Write;
use std::panic;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyview_core::decode::{self, DecodedImage};
use image::imageops::{self, FilterType};
use image::{Rgb, RgbImage, Rgba, RgbaImage};

const CELL: u32 = 200;
const COLS: u32 = 8;
const PER_SHEET: usize = 64;
const EXTENSIONS: &[&str] = &[
    "psd", "psb", "dds", "pcx", "jp2", "j2k", "j2c", "jpc", "jpx", "jpf", "clip", "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2",
    "pef", "orf", "rw2", "rwl", "raf", "erf", "3fr", "fff", "dcr", "kdc", "mos", "iiq", "srw",
    "mrw", "x3f",
];

fn main() {
    let mut out: Option<PathBuf> = None;
    let mut all_pairs = false;
    let mut full_raw = false;
    let mut psd_layers = false;
    let mut inputs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--out" {
            out = args.next().map(PathBuf::from);
        } else if arg == "--pairs" {
            all_pairs = true;
        } else if arg == "--full-raw" {
            full_raw = true;
        } else if arg == "--psd-layers" {
            psd_layers = true;
        } else {
            inputs.push(PathBuf::from(arg));
        }
    }
    let Some(out) = out else {
        eprintln!("用法：decode-check --out 目录 文件或目录…");
        std::process::exit(2);
    };
    let mut files = Vec::new();
    for input in &inputs {
        collect(input, &mut files);
    }
    files.sort();
    fs::create_dir_all(&out).expect("建输出目录");
    if psd_layers {
        check_psd_layers(&files, &out);
        return;
    }
    // 解码器出错时 panic 的信息只记进报告，不打印到终端。
    panic::set_hook(Box::new(|_| {}));

    let mut report = fs::File::create(out.join("report.tsv")).expect("建报告");
    writeln!(
        report,
        "序号\t结果\t格式\t像素格式\t尺寸\t透明\t毫秒\t参照\t平均差\t最大差\t纯色\t像素\t文件"
    )
    .unwrap();
    let mut sheet = new_sheet();
    let (mut ok, mut failed, mut flagged) = (0, 0, 0);
    for (index, path) in files.iter().enumerate() {
        let started = Instant::now();
        let options = decode::DecodeOptions {
            raw_preview: !full_raw,
            ..Default::default()
        };
        let result = panic::catch_unwind(|| decode::decode_file_with(path, options));
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let cell = (index % PER_SHEET) as u32;
        let (x, y) = ((cell % COLS) * CELL, (cell / COLS) * CELL);
        let name = path.display();
        match result {
            Ok(Ok(image)) => {
                ok += 1;
                let flat = flatten(&image, None);
                let thumb = imageops::thumbnail(&flat, CELL - 8, CELL - 8);
                imageops::overlay(
                    &mut sheet,
                    &thumb,
                    (x + (CELL - thumb.width()) / 2).into(),
                    (y + (CELL - thumb.height()) / 2).into(),
                );
                let (reference, mean, max) = compare(path, &image);
                let far = mean.is_some_and(|m| m > 12.0);
                if far {
                    flagged += 1;
                }
                if (far || (all_pairs && mean.is_some()))
                    && let Some(pair) = side_by_side(path, &image)
                {
                    pair.save(out.join(format!("diff-{index}.png")))
                        .expect("写对照图");
                }
                let fmt = |v: Option<f64>| v.map_or(String::new(), |v| format!("{v:.1}"));
                writeln!(
                    report,
                    "{index}\tok\t{}\t{}\t{}x{}\t{}\t{ms:.0}\t{reference}\t{}\t{}\t{}\t{}\t{name}",
                    image.info.format,
                    image.info.pixel_format,
                    image.width,
                    image.height,
                    if image.info.has_alpha { "有" } else { "" },
                    fmt(mean),
                    fmt(max),
                    if uniform(&image) { "纯色" } else { "" },
                    small_pixels(&image),
                )
                .unwrap();
            }
            Ok(Err(error)) => {
                failed += 1;
                fill(&mut sheet, x, y, Rgba([160, 30, 30, 255]));
                writeln!(
                    report,
                    "{index}\t解不出\t{error:?}\t\t\t\t{ms:.0}\t\t\t\t\t\t{name}"
                )
                .unwrap();
            }
            Err(panic) => {
                failed += 1;
                fill(&mut sheet, x, y, Rgba([255, 0, 255, 255]));
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                writeln!(
                    report,
                    "{index}\tpanic\t{message}\t\t\t\t{ms:.0}\t\t\t\t\t\t{name}"
                )
                .unwrap();
            }
        }
        if cell as usize == PER_SHEET - 1 || index + 1 == files.len() {
            sheet
                .save(out.join(format!("sheet-{}.png", index / PER_SHEET + 1)))
                .expect("写总览图");
            sheet = new_sheet();
        }
    }
    println!(
        "{} 个文件：解出 {ok}，解不出 {failed}，与参照差得多的 {flagged}；报告 {}",
        files.len(),
        out.join("report.tsv").display()
    );
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                collect(&entry.path(), files);
            }
        }
    } else if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
    {
        files.push(path.to_path_buf());
    }
}

fn new_sheet() -> RgbaImage {
    RgbaImage::from_pixel(
        CELL * COLS,
        CELL * (PER_SHEET as u32 / COLS),
        Rgba([48, 48, 48, 255]),
    )
}

fn fill(sheet: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>) {
    for dy in 4..CELL - 4 {
        for dx in 4..CELL - 4 {
            sheet.put_pixel(x + dx, y + dy, color);
        }
    }
}

/// 解码结果（BGRA 预乘）铺到底色上：`white` 为空时用棋盘格（看得出透明），否则用白色。
fn flatten(image: &DecodedImage, white: Option<()>) -> RgbaImage {
    let mut out = RgbaImage::new(image.width, image.height);
    for y in 0..image.height {
        let row = &image.pixels[y as usize * image.stride..];
        for x in 0..image.width {
            let p = &row[x as usize * 4..x as usize * 4 + 4];
            let bg: u32 = if white.is_some() || ((x / 8) + (y / 8)) % 2 == 0 {
                255
            } else {
                200
            };
            let a = p[3] as u32;
            let mix = |c: u8| (c as u32 + bg * (255 - a) / 255).min(255) as u8;
            out.put_pixel(x, y, Rgba([mix(p[2]), mix(p[1]), mix(p[0]), 255]));
        }
    }
    out
}

/// 与参照比对：PSD 用 Photoshop 存在文件里的缩略图，否则用旁边的同名 PNG。返回（参照、平均差、最大差）。
fn compare(path: &Path, image: &DecodedImage) -> (&'static str, Option<f64>, Option<f64>) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "psd" || ext == "psb" {
        let Some(data) = fs::read(path).ok() else {
            return ("", None, None);
        };
        // 比不了的情况只记原因：缩略图太小（JPEG 糊成一片）、带透明（Photoshop 的缩略图对透明的画法
        // 与我们铺白底不同）。没有真正合成图的文件由核心合成图层，照常比。
        let composed = psd_has_real_merged_data(&data) == Some(false);
        let Some(thumb) = psd_thumbnail(&data) else {
            return ("", None, None);
        };
        if thumb.width() < 16 || thumb.height() < 16 {
            return ("缩略图太小", None, None);
        }
        if image.info.has_alpha && !composed {
            return ("有透明", None, None);
        }
        let ours = imageops::resize(
            &flatten(image, Some(())),
            thumb.width(),
            thumb.height(),
            FilterType::Triangle,
        );
        let (mean, max) = difference(
            ours.pixels().map(|p| [p[0], p[1], p[2]]),
            thumb.pixels().map(|p| [p[0], p[1], p[2]]),
        );
        return (
            if composed {
                "合成图层"
            } else {
                "PS 缩略图"
            },
            Some(mean),
            Some(max),
        );
    }
    let png = path.with_extension("png");
    let Ok(reference) = image::open(&png) else {
        return ("", None, None);
    };
    let reference = reference.to_rgba8();
    if reference.dimensions() != (image.width, image.height) {
        return ("PNG 尺寸不同", None, None);
    }
    // 都换成预乘的 RGBA 比，透明处的颜色不算。
    let ours = (0..image.height).flat_map(|y| {
        (0..image.width).map(move |x| {
            let i = y as usize * image.stride + x as usize * 4;
            let p = &image.pixels[i..i + 4];
            [p[2], p[1], p[0], p[3]]
        })
    });
    let theirs = reference.pixels().map(|p| {
        let a = p[3] as u32;
        let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
        [m(p[0]), m(p[1]), m(p[2]), p[3]]
    });
    let (mean, max) = difference(ours, theirs);
    ("同名 PNG", Some(mean), Some(max))
}

fn difference<const N: usize>(
    a: impl Iterator<Item = [u8; N]>,
    b: impl Iterator<Item = [u8; N]>,
) -> (f64, f64) {
    let (mut sum, mut max, mut count) = (0u64, 0u8, 0u64);
    for (p, q) in a.zip(b) {
        for c in 0..N {
            let d = p[c].abs_diff(q[c]);
            sum += d as u64;
            max = max.max(d);
            count += 1;
        }
    }
    (sum as f64 / count.max(1) as f64, max as f64)
}

/// 图像资源 1057（版本信息）里的"有真正的合成图"；没有这项资源时为空。
fn psd_has_real_merged_data(data: &[u8]) -> Option<bool> {
    psd_resource(data, 1057)
        .and_then(|body| body.get(4))
        .map(|&b| b != 0)
}

/// 图像资源段里的一项。
fn psd_resource(data: &[u8], wanted: u16) -> Option<&[u8]> {
    let be32 = |at: usize| {
        data.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    if data.get(0..4)? != b"8BPS" {
        return None;
    }
    let mut pos = 26 + 4 + be32(26)?;
    let end = (pos + 4).checked_add(be32(pos)?)?.min(data.len());
    pos += 4;
    while pos + 12 <= end {
        if data.get(pos..pos + 4)? != b"8BIM" {
            return None;
        }
        let id = u16::from_be_bytes([data[pos + 4], data[pos + 5]]);
        let name = 1 + data[pos + 6] as usize;
        let at = pos + 6 + name + name % 2;
        let size = be32(at)?;
        let body = data.get(at + 4..at + 4 + size)?;
        if id == wanted {
            return Some(body);
        }
        pos = at + 4 + size + size % 2;
    }
    None
}

/// Photoshop 存在图像资源里的缩略图（1036：JPEG RGB；1033：Photoshop 4 的 BGR）。
fn psd_thumbnail(data: &[u8]) -> Option<RgbImage> {
    let (body, bgr) = match psd_resource(data, 1036) {
        Some(body) => (body, false),
        None => (psd_resource(data, 1033)?, true),
    };
    let mut thumb = image::load_from_memory_with_format(body.get(28..)?, image::ImageFormat::Jpeg)
        .ok()?
        .to_rgb8();
    if bgr {
        for p in thumb.pixels_mut() {
            *p = Rgb([p[2], p[1], p[0]]);
        }
    }
    Some(thumb)
}

/// 整张图是不是同一个颜色（每个通道最多差 2）：Photoshop 关掉"最大兼容"存的 PSD，合成图是空白的。
fn uniform(image: &DecodedImage) -> bool {
    let (mut lo, mut hi) = ([255u8; 4], [0u8; 4]);
    for y in 0..image.height as usize {
        let row = &image.pixels[y * image.stride..y * image.stride + image.width as usize * 4];
        for p in row.as_chunks::<4>().0 {
            for c in 0..4 {
                lo[c] = lo[c].min(p[c]);
                hi[c] = hi[c].max(p[c]);
            }
        }
    }
    (0..4).all(|c| hi[c].saturating_sub(lo[c]) <= 2)
}

/// 很小的图（不超过 16 个像素）把每个像素写出来（RGBA，预乘），便于不同颜色模式之间比对。
fn small_pixels(image: &DecodedImage) -> String {
    if image.width * image.height > 16 {
        return String::new();
    }
    let mut out = Vec::new();
    for y in 0..image.height as usize {
        let row = &image.pixels[y * image.stride..y * image.stride + image.width as usize * 4];
        for p in row.as_chunks::<4>().0 {
            out.push(format!("{:02x}{:02x}{:02x}{:02x}", p[2], p[1], p[0], p[3]));
        }
    }
    out.join(" ")
}

/// 与 PS 缩略图差得多的 PSD：左边是我们的结果（缩到缩略图大小），右边是缩略图，放大到便于看清。
fn side_by_side(path: &Path, image: &DecodedImage) -> Option<RgbaImage> {
    let thumb = fs::read(path).ok().and_then(|d| psd_thumbnail(&d))?;
    let (w, h) = thumb.dimensions();
    let ours = imageops::resize(&flatten(image, Some(())), w, h, FilterType::Triangle);
    let mut pair = RgbaImage::from_pixel(w * 2 + 4, h, Rgba([255, 0, 255, 255]));
    imageops::overlay(&mut pair, &ours, 0, 0);
    let theirs = RgbaImage::from_fn(w, h, |x, y| {
        let p = thumb.get_pixel(x, y);
        Rgba([p[0], p[1], p[2], 255])
    });
    imageops::overlay(&mut pair, &theirs, (w + 4).into(), 0);
    let scale = (480 / h.max(1)).max(1);
    let filter = if scale >= 4 {
        FilterType::Nearest
    } else {
        FilterType::CatmullRom
    };
    let scale = scale.max(2);
    Some(imageops::resize(
        &pair,
        pair.width() * scale,
        h * scale,
        filter,
    ))
}

/// 图层合成与文件里 Photoshop 存的合成图比：都铺到白底上，比平均差、最大差。
fn check_psd_layers(files: &[PathBuf], out: &Path) {
    use anyview_core::psd;
    panic::set_hook(Box::new(|_| {}));
    let mut report = fs::File::create(out.join("layers.tsv")).expect("建报告");
    writeln!(report, "序号\t结果\t平均差\t最大差\t毫秒\t文件").unwrap();
    let (mut ok, mut far, mut skipped) = (0, 0, 0);
    for (index, path) in files.iter().enumerate() {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "psd" && ext != "psb" {
            continue;
        }
        let Ok(data) = fs::read(path) else { continue };
        if psd_has_real_merged_data(&data) == Some(false) {
            skipped += 1;
            writeln!(report, "{index}\t没有合成图\t\t\t\t{}", path.display()).unwrap();
            continue;
        }
        let started = Instant::now();
        let layered = panic::catch_unwind(|| psd::decode_layers(&data, true));
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let merged = psd::decode(&data);
        let (layered, merged) = match (layered, merged) {
            (Ok(Ok(l)), Ok(m)) if l.width == m.width && l.height == m.height => (l, m),
            (Err(_), _) => {
                writeln!(report, "{index}\tpanic\t\t\t{ms:.0}\t{}", path.display()).unwrap();
                far += 1;
                continue;
            }
            (Ok(Err(e)), _) => {
                writeln!(
                    report,
                    "{index}\t合成不了：{e}\t\t\t{ms:.0}\t{}",
                    path.display()
                )
                .unwrap();
                skipped += 1;
                continue;
            }
            _ => {
                writeln!(
                    report,
                    "{index}\t没有合成图可比\t\t\t{ms:.0}\t{}",
                    path.display()
                )
                .unwrap();
                skipped += 1;
                continue;
            }
        };
        let white = |r: &anyview_core::decode::Raster| -> RgbaImage {
            RgbaImage::from_fn(r.width, r.height, |x, y| {
                let i = (y * r.width + x) as usize * 4;
                let p = &r.rgba[i..i + 4];
                let a = u32::from(p[3]);
                let mix = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
                Rgba([mix(p[0]), mix(p[1]), mix(p[2]), 255])
            })
        };
        let (a, b) = (white(&layered), white(&merged));
        let (mean, max) = difference(
            a.pixels().map(|p| [p[0], p[1], p[2]]),
            b.pixels().map(|p| [p[0], p[1], p[2]]),
        );
        if mean > 6.0 {
            far += 1;
            let (w, h) = a.dimensions();
            let mut pair = RgbaImage::from_pixel(w * 2 + 4, h, Rgba([255, 0, 255, 255]));
            imageops::overlay(&mut pair, &a, 0, 0);
            imageops::overlay(&mut pair, &b, (w + 4).into(), 0);
            let small = imageops::thumbnail(&pair, 960, 480);
            small
                .save(out.join(format!("layers-{index}.png")))
                .expect("写对照图");
        } else {
            ok += 1;
        }
        writeln!(
            report,
            "{index}\tok\t{mean:.1}\t{max:.0}\t{ms:.0}\t{}",
            path.display()
        )
        .unwrap();
    }
    println!(
        "图层合成：与合成图一致 {ok}，差得多 {far}，比不了 {skipped}；报告 {}",
        out.join("layers.tsv").display()
    );
}
