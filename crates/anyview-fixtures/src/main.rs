//! 生成测试图片，内容由公式确定，每次运行结果相同。
//!
//! 用法：`anyview-fixtures [--out 目录] [--many 数量] [--huge]`
//! 默认输出到 `tests/fixtures/generated/`（不纳入 Git）。

use std::error::Error;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage, imageops};

mod formats;
mod modern;
mod raw;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Options {
    out: PathBuf,
    many: Option<usize>,
    huge: bool,
}

fn parse_args() -> Result<Options> {
    let mut opts = Options {
        out: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/generated"),
        many: None,
        huge: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => opts.out = PathBuf::from(args.next().ok_or("--out 需要一个目录")?),
            "--many" => opts.many = Some(args.next().ok_or("--many 需要一个数量")?.parse()?),
            "--huge" => opts.huge = true,
            "-h" | "--help" => {
                println!("用法：anyview-fixtures [--out 目录] [--many 数量] [--huge]");
                std::process::exit(0);
            }
            other => return Err(format!("未知参数：{other}").into()),
        }
    }
    Ok(opts)
}

fn main() -> Result<()> {
    let opts = parse_args()?;
    let written = generate(&opts.out, opts.many, opts.huge)?;
    println!("已生成 {written} 个文件：{}", opts.out.display());
    Ok(())
}

/// 生成全部测试图片，返回文件数。
fn generate(out: &Path, many: Option<usize>, huge: bool) -> Result<usize> {
    let mut count = 0;
    count += basic(&out.join("basic"))?;
    count += sort_names(&out.join("sort"))?;
    count += exif_orientations(&out.join("exif"))?;
    count += broken(&out.join("broken"))?;
    count += misc(&out.join("misc"))?;
    count += view(&out.join("view"))?;
    count += color(&out.join("color"))?;
    count += preview(&out.join("preview"))?;
    count += animations(&out.join("anim"))?;
    count += archives(&out.join("archives"))?;
    count += pages(&out.join("pages"))?;
    count += formats::generate(&out.join("formats"))?;
    count += raw::generate(&out.join("raw"))?;
    count += modern::generate(&out.join("modern"))?;
    count += generation(&out.join("generation"))?;
    count += effects(&out.join("effects"))?;
    count += limits(&out.join("limits"))?;
    count += thumbs(&out.join("zthumbs"))?;
    count += all_formats(out)?;
    if let Some(n) = many {
        count += many_files(&out.join("many"), n)?;
    }
    if huge {
        count += huge_image(&out.join("huge"))?;
    }
    fs::write(out.join("README.txt"), README)?;
    Ok(count + 1)
}

const README: &str = "\
由 crates/anyview-fixtures 生成的测试图片，可随时删除后重新生成。
basic/  各种格式的常规图片；photo_4000x3000.jpg 用于冷启动与切图性能基准
sort/   测试自然排序的文件名
exif/   EXIF 方向 1–8：正确显示时，左上红、右上绿、左下蓝、右下黄，白色箭头朝上
broken/ 截断、空文件、非图片内容
misc/   扩展名与内容不符的文件
view/   四色块图片（左上红、右上绿、左下蓝、右下黄），用于核对缩放、旋转、翻转后的画面
color/  带 ICC 的图片：灰度 128，配置文件为线性 Gamma 的 sRGB 原色，按 ICC 显示时应约为 188；
        不带 ICC 的灰度 128 和线性 Gamma 的“显示器”配置文件 linear_monitor.icc（转换后约为 55）
preview/ 带 EXIF 内嵌缩略图的 600 万像素 JPEG，用于快速预览
archives/ 压缩包：a_book.cbz（ZIP）、b_book.cb7（固实 7Z）、c_book.cbr（RAR，不压缩），各 3 页 300x400 的
        四色块（第 k 页轮转 k 格），ZIP 里另有子文件夹和 __MACOSX 里的杂项
pages/  双页阅览：01、02 竖图，03 横图，04、05 竖图（四色块轮转）
anim/   动图：a_frames.gif 4 帧（四色块轮转，第 3 帧时长写成 0）、b_frames.apng 3 帧、
        c_transparent.gif 2 帧，中间透明，GIF 背景色为品红 #FF00FF
formats/ 自写解码器：PSD（RGB 8/16/32 位、灰度、位图、CMYK、索引色、带透明）、PSB、DDS（BGRA8、DXT1、DXT5、BC7、
        RGBA16F）、PCX（8 位调色板、24 位），都是 128x96 的四色块
raw/    RAW 内嵌预览：NEF、CR2、ORF、RW2、RAF、CR3、DNG（方向 6）、X3F、MRW 的文件结构加 JPEG 预览（不是真的 RAW 数据），
        预览图是不同尺寸的四色块
modern/ 较新的格式：EXR（含超过普通亮度的）、JPEG XL（含动图）、SVG、SVGZ、带文字的 SVG、AVIF（含动图）、HEIC；
        JPEG XL、AVIF、HEIC 是用开发容器里的编码器从本项目生成的四色块编码好、随代码保存的（crates/anyview-fixtures/data）
effects/ 翻页效果：三张 400x300 的纯色图（红、蓝、绿）
generation/ AI 生成的图片里记的生成信息，1x1：PNG 的 tEXt、zTXt、iTXt 里的 Stable Diffusion WebUI 参数，
        ComfyUI 的节点图（comfy.png），JPEG（大端、小端）、WebP 的 EXIF 用户注释；plain.png 没有生成信息
zthumbs/ 文件管理器缩略图：QOI 的 128x96 四色块（系统一般不认识的格式；其余用 formats/、modern/ 里的）。
        名字以 z 开头，排在 view/ 后面：浏览自检从 sort/ “下一个文件夹”要到 view/
zall/   各种格式放在一起（basic、anim、formats、modern、raw、zthumbs 的副本，另加 TGA、ICO、PPM、PAM），在访达、文件管理器里一眼看出
        哪些格式缺少系统缩略图。RAW 文件名前带有 raw_
many/   （--many）大量小图，用于目录扫描性能
huge/   （--huge）超大图，用于纹理上限和精确重绘
";

/// generation/：带生成信息的小图（核心的单元测试用的同一批，anyview_core::generation::samples）。
fn generation(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    let files = anyview_core::generation::samples::all();
    for (name, bytes) in &files {
        fs::write(dir.join(name), bytes)?;
    }
    Ok(files.len())
}

fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    Ok(())
}

fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality).encode_image(img)?;
    Ok(bytes)
}

/// 平滑渐变加细节纹理，模拟照片的压缩难度。
fn photo_like(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        let fx = x as f32 / width as f32;
        let fy = y as f32 / height as f32;
        let ripple = ((x as f32 * 0.05).sin() * (y as f32 * 0.07).cos() * 24.0) as i32;
        let r = (fx * 200.0) as i32 + 30 + ripple;
        let g = (fy * 180.0) as i32 + 40 - ripple / 2;
        let b = ((1.0 - fx) * (1.0 - fy) * 220.0) as i32 + 20 + ripple / 3;
        Rgb([clamp(r), clamp(g), clamp(b)])
    })
}

fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn basic(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    fs::write(
        dir.join("photo_4000x3000.jpg"),
        encode_jpeg(&photo_like(4000, 3000), 90)?,
    )?;

    // 半透明圆形，透明区域用于检查棋盘格背景。
    let transparent = RgbaImage::from_fn(800, 600, |x, y| {
        let (cx, cy) = (x as f32 - 400.0, y as f32 - 300.0);
        let d = (cx * cx + cy * cy).sqrt();
        if d < 250.0 {
            let alpha = (255.0 * (1.0 - d / 250.0)).clamp(0.0, 255.0) as u8;
            Rgba([230, 80, 60, alpha.max(40)])
        } else {
            Rgba([0, 0, 0, 0])
        }
    });
    transparent.save_with_format(dir.join("transparent_800x600.png"), ImageFormat::Png)?;

    let small = RgbImage::from_fn(64, 48, |x, y| Rgb([(x * 4) as u8, (y * 5) as u8, 160]));
    small.save_with_format(dir.join("small_64x48.png"), ImageFormat::Png)?;

    let plain = photo_like(640, 480);
    plain.save_with_format(dir.join("plain_640x480.bmp"), ImageFormat::Bmp)?;
    plain.save_with_format(dir.join("plain_640x480.tiff"), ImageFormat::Tiff)?;
    plain.save_with_format(dir.join("lossless_640x480.webp"), ImageFormat::WebP)?;

    let palette = RgbaImage::from_fn(320, 240, |x, y| {
        let band = ((x / 40) + (y / 40)) % 4;
        match band {
            0 => Rgba([220, 50, 50, 255]),
            1 => Rgba([50, 160, 70, 255]),
            2 => Rgba([40, 90, 200, 255]),
            _ => Rgba([0, 0, 0, 0]),
        }
    });
    palette.save_with_format(dir.join("palette_320x240.gif"), ImageFormat::Gif)?;
    Ok(7)
}

/// 自然排序：期望顺序为 1, 2, 10, a, B, img2, IMG3, img10, 中文（不区分大小写，数字按数值比较）。
fn sort_names(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    let names = [
        "10.png",
        "1.png",
        "2.png",
        "img10.png",
        "img2.png",
        "IMG3.png",
        "a.png",
        "B.png",
        "中文.png",
    ];
    for (i, name) in names.iter().enumerate() {
        let shade = 40 + (i as u8) * 20;
        let img = RgbImage::from_pixel(32, 32, Rgb([shade, 255 - shade, 128]));
        img.save_with_format(dir.join(name), ImageFormat::Png)?;
    }
    Ok(names.len())
}

/// 正确朝向的参照图：四角颜色各不相同，再加一个朝上的白色箭头。
fn upright_marker() -> RgbImage {
    let (w, h) = (400u32, 300u32);
    RgbImage::from_fn(w, h, |x, y| {
        let arrow_half = (y as i32 - 40).max(0) / 2;
        if (40..160).contains(&y) && (x as i32 - 200).abs() <= arrow_half {
            return Rgb([255, 255, 255]);
        }
        match (x < w / 2, y < h / 2) {
            (true, true) => Rgb([220, 40, 40]),    // 左上 红
            (false, true) => Rgb([40, 180, 60]),   // 右上 绿
            (true, false) => Rgb([40, 80, 220]),   // 左下 蓝
            (false, false) => Rgb([230, 200, 30]), // 右下 黄
        }
    })
}

/// 第 n 种 EXIF 方向下应存储的像素：对参照图做"显示变换"的逆变换。
fn stored_for_orientation(upright: &RgbImage, orientation: u16) -> RgbImage {
    match orientation {
        1 => upright.clone(),
        2 => imageops::flip_horizontal(upright),
        3 => imageops::rotate180(upright),
        4 => imageops::flip_vertical(upright),
        5 => imageops::flip_horizontal(&imageops::rotate90(upright)), // 转置，自逆
        6 => imageops::rotate270(upright),                            // 显示时顺时针转 90°
        7 => imageops::flip_horizontal(&imageops::rotate270(upright)), // 反转置，自逆
        8 => imageops::rotate90(upright),                             // 显示时逆时针转 90°
        _ => unreachable!("EXIF 方向只有 1–8"),
    }
}

fn exif_app1(orientation: u16) -> Vec<u8> {
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II*\0"); // 小端，TIFF 魔数 42
    tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 偏移
    tiff.extend_from_slice(&1u16.to_le_bytes()); // 1 个条目
    tiff.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
    tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_le_bytes()); // 数量 1
    tiff.extend_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]); // 值字段补足 4 字节
    tiff.extend_from_slice(&0u32.to_le_bytes()); // 没有下一个 IFD
    let mut segment = vec![0xFF, 0xE1];
    segment.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    segment.extend_from_slice(b"Exif\0\0");
    segment.extend_from_slice(&tiff);
    segment
}

/// 把 APP1 段插到 SOI（以及紧随其后的 JFIF APP0 段）之后。
fn insert_app1(jpeg: &[u8], app1: &[u8]) -> Vec<u8> {
    assert!(jpeg.starts_with(&[0xFF, 0xD8]), "不是 JPEG");
    let mut pos = 2;
    if jpeg.len() > 6 && jpeg[2] == 0xFF && jpeg[3] == 0xE0 {
        pos = 4 + u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize;
    }
    let mut out = Vec::with_capacity(jpeg.len() + app1.len());
    out.extend_from_slice(&jpeg[..pos]);
    out.extend_from_slice(app1);
    out.extend_from_slice(&jpeg[pos..]);
    out
}

fn exif_orientations(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    let upright = upright_marker();
    for orientation in 1..=8u16 {
        let stored = stored_for_orientation(&upright, orientation);
        let jpeg = insert_app1(&encode_jpeg(&stored, 92)?, &exif_app1(orientation));
        fs::write(dir.join(format!("orientation_{orientation}.jpg")), jpeg)?;
    }
    Ok(8)
}

fn broken(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    let jpeg = encode_jpeg(&photo_like(640, 480), 85)?;
    fs::write(dir.join("truncated.jpg"), &jpeg[..jpeg.len() / 2])?;
    fs::write(dir.join("not_an_image.jpg"), "这不是图片。\n")?;
    fs::write(dir.join("empty.png"), b"")?;
    Ok(3)
}

/// 四个色块：左上红、右上绿、左下蓝、右下黄。界面自测把图片上的点按核心算出的位置映射到屏幕，
/// 核对截图里的颜色，从而验证旋转、翻转的方向和位置。
fn quadrants(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        match (x < width / 2, y < height / 2) {
            (true, true) => Rgb([220, 40, 40]),
            (false, true) => Rgb([40, 170, 70]),
            (true, false) => Rgb([40, 90, 220]),
            (false, false) => Rgb([240, 200, 40]),
        }
    })
}

/// 被安全上限拦下的文件：一张普通的小图，和一个解压后 65 MB 的 SVGZ（上限 64 MB；内容是 4x4 的
/// 红色方块加一大段空白注释，压缩后只有几十 KB）。打开后者时程序先问，选"仍然打开"才显示。
fn limits(dir: &Path) -> Result<usize> {
    use std::io::Write;
    ensure_dir(dir)?;
    quadrants(64, 48).save_with_format(dir.join("a.png"), ImageFormat::Png)?;
    let mut gz = flate2::write::GzEncoder::new(
        fs::File::create(dir.join("b_65mb.svgz"))?,
        flate2::Compression::fast(),
    );
    gz.write_all(br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48"><rect width="64" height="48" fill="#dc2828"/><!--"##)?;
    gz.write_all(&vec![b' '; 65 << 20])?;
    gz.write_all(b"--></svg>")?;
    gz.finish()?;
    Ok(2)
}

fn view(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    quadrants(600, 400).save_with_format(dir.join("quadrants_600x400.png"), ImageFormat::Png)?;
    // 高图：用于"适合宽度"和"图片大于窗口时显示顶部"。
    quadrants(400, 3000).save_with_format(dir.join("tall_400x3000.png"), ImageFormat::Png)?;
    Ok(2)
}

/// 带内嵌 ICC 的图片：灰度 128，配置文件为线性 Gamma 的 sRGB 原色；正确按 ICC 显示时约为 188。
fn color(dir: &Path) -> Result<usize> {
    use image::ImageEncoder;
    ensure_dir(dir)?;
    let gray = RgbImage::from_pixel(64, 64, Rgb([128, 128, 128]));
    let mut png = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut png);
    encoder.set_icc_profile(anyview_core::color::linear_srgb_test_profile())?;
    encoder.write_image(gray.as_raw(), 64, 64, image::ExtendedColorType::Rgb8)?;
    fs::write(dir.join("linear_gray_128.png"), png)?;
    // "使用监视器的 ICC 设置"：不带 ICC 的灰度 128，加上当"显示器"用的线性 Gamma 配置文件
    // （Linux 自测用环境变量 ANYVIEW_TEST_MONITOR_ICC 指过去）；按它转换后约为 55。
    gray.save_with_format(dir.join("gray_128.png"), ImageFormat::Png)?;
    fs::write(dir.join("linear_monitor.icc"), anyview_core::color::linear_srgb_test_profile())?;
    Ok(3)
}

/// 四色块的颜色（左上红、右上绿、左下蓝、右下黄），第 k 帧轮转 k 格。
const QUAD_COLORS: [[u8; 3]; 4] = [[220, 40, 40], [40, 170, 70], [40, 90, 220], [240, 200, 40]];

fn quadrant_index(x: u32, y: u32, width: u32, height: u32, frame: usize) -> u8 {
    let q = match (x < width / 2, y < height / 2) {
        (true, true) => 0,
        (false, true) => 1,
        (true, false) => 2,
        (false, false) => 3,
    };
    ((q + frame) % 4) as u8
}

/// 一页：w x h 的四色块，轮转 k 格（每页不同，画面核对能分清）。
fn page_png(w: u32, h: u32, k: usize) -> Result<Vec<u8>> {
    let image = RgbImage::from_fn(w, h, |x, y| Rgb(QUAD_COLORS[quadrant_index(x, y, w, h, k) as usize]));
    let mut png = Vec::new();
    image.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    Ok(png)
}

/// 压缩包。
fn archives(dir: &Path) -> Result<usize> {
    use std::io::Write;
    ensure_dir(dir)?;
    let pages: Vec<(String, Vec<u8>)> = (0..3)
        .map(|k| Ok((format!("{}.png", k + 1), page_png(300, 400, k)?)))
        .collect::<Result<_>>()?;
    // ZIP（.cbz）：图片放在子文件夹里；另有 macOS 打包时附带的 __MACOSX 和隐藏文件，浏览时应跳过。
    {
        let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("a_book.cbz"))?);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in &pages {
            zip.start_file(format!("pages/{name}"), options)?;
            zip.write_all(data)?;
        }
        zip.start_file("__MACOSX/pages/._1.png", options)?;
        zip.write_all(b"junk")?;
        zip.start_file("pages/.hidden.png", options)?;
        zip.write_all(b"junk")?;
        zip.finish()?;
    }
    // 7Z（.cb7）：固实压缩（只能从头依次解）。
    {
        let mut writer = sevenz_rust2::ArchiveWriter::create(dir.join("b_book.cb7"))?;
        let entries = pages
            .iter()
            .map(|(name, _)| sevenz_rust2::ArchiveEntry::new_file(name))
            .collect();
        let readers = pages
            .iter()
            .map(|(_, data)| sevenz_rust2::SourceReader::from(Cursor::new(data.clone())))
            .collect();
        writer.push_archive_entries(entries, readers)?;
        writer.finish()?;
    }
    // RAR（.cbr）：RAR 4 格式、不压缩（压缩只有 RAR 官方工具能做）。
    fs::write(dir.join("c_book.cbr"), stored_rar(&pages))?;
    Ok(3)
}

/// 不压缩的 RAR 4 压缩包：标记块、主头、每个文件一个文件头加原始数据、结束块。
/// 头的校验值是从"头类型"起到头末尾的 CRC32 的低 16 位。
fn stored_rar(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    fn header(fields: &[u8]) -> Vec<u8> {
        let crc = (crc32(fields) & 0xFFFF) as u16;
        let mut out = crc.to_le_bytes().to_vec();
        out.extend_from_slice(fields);
        out
    }
    let mut out = b"Rar!\x1a\x07\x00".to_vec();
    // 主头：类型 0x73，标志 0，大小 13，两个保留字段。
    let mut main = vec![0x73];
    main.extend_from_slice(&0u16.to_le_bytes());
    main.extend_from_slice(&13u16.to_le_bytes());
    main.extend_from_slice(&[0; 6]);
    out.extend(header(&main));
    for (name, data) in files {
        let size = data.len() as u32;
        let mut file = vec![0x74];
        file.extend_from_slice(&0x8000u16.to_le_bytes()); // 带数据的块
        file.extend_from_slice(&((32 + name.len()) as u16).to_le_bytes());
        file.extend_from_slice(&size.to_le_bytes()); // 压缩后大小
        file.extend_from_slice(&size.to_le_bytes()); // 原始大小
        file.push(0); // 系统：MS-DOS
        file.extend_from_slice(&crc32(data).to_le_bytes());
        file.extend_from_slice(&0x5B21_0000u32.to_le_bytes()); // 2025-09-01 00:00（DOS 时间）
        file.push(20); // 解压需要的版本 2.0
        file.push(0x30); // 方法：不压缩
        file.extend_from_slice(&(name.len() as u16).to_le_bytes());
        file.extend_from_slice(&0x20u32.to_le_bytes()); // 属性：存档
        file.extend_from_slice(name.as_bytes());
        out.extend(header(&file));
        out.extend_from_slice(data);
    }
    let mut end = vec![0x7B];
    end.extend_from_slice(&0x4000u16.to_le_bytes());
    end.extend_from_slice(&7u16.to_le_bytes());
    out.extend(header(&end));
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// 双页阅览：两张竖图、一张横图、两张竖图。
fn pages(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    for (k, (name, w, h)) in [("01", 300, 400), ("02", 300, 400), ("03", 400, 300), ("04", 300, 400), ("05", 300, 400)]
        .into_iter()
        .enumerate()
    {
        fs::write(dir.join(format!("{name}.png")), page_png(w, h, k)?)?;
    }
    Ok(5)
}

/// 动图。
fn animations(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    // GIF 的全局调色板：四色块的颜色、品红（背景色，序号 4）、青、橙；序号 7 用作透明色。
    let mut palette: Vec<u8> = QUAD_COLORS.iter().flatten().copied().collect();
    palette.extend_from_slice(&[255, 0, 255, 0, 200, 200, 250, 140, 30, 0, 0, 0]);
    let write_gif = |name: &str, width: u16, height: u16, frames: Vec<(Vec<u8>, u16, Option<u8>)>| -> Result<()> {
        let mut out = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut out, width, height, &palette)?;
            encoder.set_repeat(gif::Repeat::Infinite)?;
            for (indices, delay, transparent) in frames {
                let mut frame = gif::Frame::from_indexed_pixels(width, height, indices, transparent);
                frame.delay = delay;
                frame.dispose = gif::DisposalMethod::Background;
                encoder.write_frame(&frame)?;
            }
        }
        // gif 库写的背景色序号是 0：改成 4（品红），逻辑屏幕描述符第 11 个字节。
        out[11] = 4;
        fs::write(dir.join(name), out)?;
        Ok(())
    };
    let (w, h) = (240u32, 160u32);
    let frames = (0..4)
        .map(|k| {
            let indices = (0..h)
                .flat_map(|y| (0..w).map(move |x| quadrant_index(x, y, w, h, k)))
                .collect();
            // 时长以 1/100 秒计；第 3 帧写成 0（按 100 ms 播放）。
            (indices, if k == 2 { 0 } else { 10 }, None)
        })
        .collect();
    write_gif("a_frames.gif", w as u16, h as u16, frames)?;
    let (w, h) = (200u32, 150u32);
    let frames = [5u8, 6]
        .iter()
        .map(|&fill| {
            let indices = (0..h)
                .flat_map(|y| {
                    (0..w).map(move |x| {
                        let hole = (50..150).contains(&x) && (38..113).contains(&y);
                        if hole { 7 } else { fill }
                    })
                })
                .collect();
            (indices, 20, Some(7))
        })
        .collect();
    write_gif("c_transparent.gif", w as u16, h as u16, frames)?;

    // APNG：3 帧四色块轮转，每帧 100 ms。
    let (w, h) = (200u32, 120u32);
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(3, 0)?;
        encoder.set_frame_delay(1, 10)?;
        let mut writer = encoder.write_header()?;
        for k in 0..3 {
            let data: Vec<u8> = (0..h)
                .flat_map(|y| {
                    (0..w).flat_map(move |x| QUAD_COLORS[quadrant_index(x, y, w, h, k) as usize])
                })
                .collect();
            writer.write_image_data(&data)?;
        }
        writer.finish()?;
    }
    fs::write(dir.join("b_frames.apng"), out)?;
    Ok(3)
}

/// zall/：汇总各格式测试样本，便于集中检查系统文件管理器的缩略图覆盖情况。
/// 是其他文件夹里那些图片的副本（先生成那些），RAW 的文件名前加 raw_（原名只是 a_、b_ 排序用）。
fn all_formats(out: &Path) -> Result<usize> {
    let dir = out.join("zall");
    ensure_dir(&dir)?;
    let mut count = 0;
    for folder in ["basic", "anim", "formats", "modern", "raw", "zthumbs"] {
        let mut names: Vec<_> = fs::read_dir(out.join(folder))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let target = if folder == "raw" { format!("raw_{name}") } else { name.clone() };
            fs::copy(out.join(folder).join(&name), dir.join(target))?;
            count += 1;
        }
    }
    // 别的文件夹里没有的几种：TGA、ICO、PPM（PNM）、PAM，同样是 128x96 的四色块。
    let q = quadrants(128, 96);
    q.save_with_format(dir.join("quadrants.tga"), ImageFormat::Tga)?;
    // ICO 里的 PNG 要 RGBA（解码器只认 RGBA 的，实际的图标也都是）。
    image::DynamicImage::ImageRgb8(q.clone()).to_rgba8().save_with_format(dir.join("quadrants.ico"), ImageFormat::Ico)?;
    {
        use image::ImageEncoder;
        // 明确写成二进制 PPM：image 的默认 PNM 写法是 PAM，以前 quadrants.ppm 其实是 PAM，
        // 装上 PAM 的类型定义后 GNOME 按内容把它当 PAM。
        let file = fs::File::create(dir.join("quadrants.ppm"))?;
        image::codecs::pnm::PnmEncoder::new(file)
            .with_subtype(image::codecs::pnm::PnmSubtype::Pixmap(image::codecs::pnm::SampleEncoding::Binary))
            .write_image(q.as_raw(), 128, 96, image::ExtendedColorType::Rgb8)?;
        let file = fs::File::create(dir.join("quadrants.pam"))?;
        image::codecs::pnm::PnmEncoder::new(file)
            .with_subtype(image::codecs::pnm::PnmSubtype::ArbitraryMap)
            .write_image(q.as_raw(), 128, 96, image::ExtendedColorType::Rgb8)?;
    }
    Ok(count + 4)
}

/// zthumbs/：文件管理器缩略图的核对：QOI 的四色块（macOS、常见 Linux 桌面都不认识这种格式）。
/// 名字以 z 开头，排在 view/ 后面（浏览自检从 sort/ 的"下一个文件夹"要到 view/）。
fn thumbs(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    quadrants(128, 96).save_with_format(dir.join("quadrants.qoi"), ImageFormat::Qoi)?;
    Ok(1)
}

/// effects/：翻页效果的核对：三张同样大小的纯色图（窗口大小不变，停在一半时按颜色核对）。
fn effects(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    for (name, rgb) in [
        ("1_red.png", [200, 40, 40]),
        ("2_blue.png", [40, 40, 200]),
        ("3_green.png", [40, 160, 40]),
    ] {
        RgbImage::from_pixel(400, 300, Rgb(rgb)).save(dir.join(name))?;
    }
    Ok(3)
}

fn misc(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    let mut png = Vec::new();
    photo_like(320, 240).write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    fs::write(dir.join("png_content_named.jpg"), png)?;
    Ok(1)
}

/// 快速预览：600 万像素的照片，EXIF 里带 160x107 的缩略图。
fn preview(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    let main = photo_like(3000, 2000);
    let small = imageops::thumbnail(&main, 160, 107);
    let jpeg = anyview_core::jpeg::insert_exif_thumbnail(
        &encode_jpeg(&main, 85)?,
        &encode_jpeg(&small, 80)?,
        1,
        false,
    );
    fs::write(dir.join("thumbnail_3000x2000.jpg"), jpeg)?;
    Ok(1)
}

fn many_files(dir: &Path, n: usize) -> Result<usize> {
    ensure_dir(dir)?;
    let width = n.to_string().len().max(5);
    for i in 0..n {
        let img = RgbImage::from_pixel(
            8,
            8,
            Rgb([(i % 251) as u8, (i % 241) as u8, (i % 239) as u8]),
        );
        img.save_with_format(dir.join(format!("{i:0width$}.png")), ImageFormat::Png)?;
    }
    Ok(n)
}

fn huge_image(dir: &Path) -> Result<usize> {
    ensure_dir(dir)?;
    fs::write(
        dir.join("huge_20000x12000.jpg"),
        encode_jpeg(&photo_like(20000, 12000), 85)?,
    )?;
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_files(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .map(|p| if p.is_dir() { count_files(&p) } else { 1 })
            .sum()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("anyview-fixtures-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn generates_decodable_images() {
        let dir = temp_dir("all");
        let n = generate(&dir, Some(3), false).unwrap();
        // 返回的文件数与实际写出的相同（各组自己的测试核对内容）。
        assert_eq!(n, count_files(&dir));
        assert!(n >= 80, "{n}");
        // 三种压缩包都能列出、读出 3 页（RAR 是自己写的，确认 UnRAR 认得）。
        for name in ["a_book.cbz", "b_book.cb7", "c_book.cbr"] {
            let archive = anyview_core::archive::open(&dir.join("archives").join(name)).unwrap();
            assert_eq!(archive.items.len(), 3, "{name}");
            let first = archive.items[0].name.clone();
            let file = archive.file(&first).unwrap();
            assert!(anyview_core::decode::decode_file(&file).is_ok(), "{name}");
        }
        // 快速预览：EXIF 里的缩略图能读出来，尺寸按原图给出。
        let preview = anyview_core::jpeg::preview(&dir.join("preview/thumbnail_3000x2000.jpg"))
            .expect("应有快速预览");
        assert_eq!((preview.width, preview.height), (3000, 2000));
        assert_eq!((preview.image.width, preview.image.height), (160, 107));
        let quadrants = image::open(dir.join("view/quadrants_600x400.png"))
            .unwrap()
            .to_rgb8();
        assert_eq!(quadrants.get_pixel(0, 0).0, [220, 40, 40]);
        assert_eq!(quadrants.get_pixel(599, 399).0, [240, 200, 40]);
        let photo = image::open(dir.join("basic/photo_4000x3000.jpg")).unwrap();
        assert_eq!((photo.width(), photo.height()), (4000, 3000));
        let webp = image::open(dir.join("basic/lossless_640x480.webp")).unwrap();
        assert_eq!((webp.width(), webp.height()), (640, 480));
        // 截断的 JPEG 可能被宽松的解码器部分解码，所以只检查它确实被截短了；
        // 完全无法解码的情况由空文件和非图片内容覆盖。
        let truncated = fs::metadata(dir.join("broken/truncated.jpg"))
            .unwrap()
            .len();
        assert!(truncated > 0 && truncated < 640 * 480 * 3);
        assert!(image::open(dir.join("broken/not_an_image.jpg")).is_err());
        assert!(image::open(dir.join("broken/empty.png")).is_err());
        // 内容是 PNG、扩展名是 .jpg：按内容识别应能解码。
        let misnamed =
            image::load_from_memory(&fs::read(dir.join("misc/png_content_named.jpg")).unwrap())
                .unwrap();
        assert_eq!(misnamed.width(), 320);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn exif_orientation_files_carry_the_tag_and_swap_dimensions() {
        let dir = temp_dir("exif");
        exif_orientations(&dir).unwrap();
        for orientation in 1..=8u16 {
            let bytes = fs::read(dir.join(format!("orientation_{orientation}.jpg"))).unwrap();
            let needle = exif_app1(orientation);
            assert!(
                bytes.windows(needle.len()).any(|w| w == needle),
                "方向 {orientation} 缺少 EXIF"
            );
            let img = image::load_from_memory(&bytes).unwrap();
            let expected = if orientation >= 5 {
                (300, 400)
            } else {
                (400, 300)
            };
            assert_eq!((img.width(), img.height()), expected, "方向 {orientation}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stored_pixels_return_to_upright_after_display_transform() {
        // 显示变换：按 EXIF 规范把存储像素转成正确朝向。
        fn display(stored: &RgbImage, orientation: u16) -> RgbImage {
            match orientation {
                1 => stored.clone(),
                2 => imageops::flip_horizontal(stored),
                3 => imageops::rotate180(stored),
                4 => imageops::flip_vertical(stored),
                5 => imageops::flip_horizontal(&imageops::rotate90(stored)),
                6 => imageops::rotate90(stored),
                7 => imageops::flip_horizontal(&imageops::rotate270(stored)),
                8 => imageops::rotate270(stored),
                _ => unreachable!(),
            }
        }
        let upright = upright_marker();
        for orientation in 1..=8 {
            let shown = display(&stored_for_orientation(&upright, orientation), orientation);
            assert!(shown == upright, "方向 {orientation} 还原失败");
        }
    }
}
