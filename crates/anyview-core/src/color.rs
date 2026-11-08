//! 色彩管理：默认使用图片内嵌的 ICC。
//! macOS 把 ICC 交给系统（ColorSync 按显示器匹配，保留广色域）；Linux、Windows 在核心里转成 sRGB 再显示。

use std::sync::Arc;

/// 四通道像素的排列（Alpha 都在最后）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelOrder {
    Rgba,
    Bgra,
}

/// 内嵌 ICC 的处理：macOS 保留给 ColorSync（返回配置文件）；其他平台把像素转成 sRGB（返回 None；
/// 认不出的配置文件按 sRGB 显示）。像素不预乘。
pub fn manage(
    pixels: &mut [u8],
    icc: Option<Vec<u8>>,
    order: ChannelOrder,
) -> Option<Arc<Vec<u8>>> {
    let profile = icc?;
    match profile.get(16..20) {
        Some(b"RGB ") => {}
        // 灰度图带灰度配置文件（Photoshop 存的灰度图常见）：像素已经展开成 R=G=B，按灰度转。
        // 三个平台都在核心里转（界面只认 RGB 的配置文件）。像素不是灰度的（彩色图片带了灰度配置文件，
        // 文件写错了）不用它，按 sRGB 显示。
        Some(b"GRAY") => {
            let mut grays = Vec::with_capacity(pixels.len() / 4);
            for px in pixels.as_chunks::<4>().0 {
                if px[0] != px[1] || px[1] != px[2] {
                    return None;
                }
                grays.push(px[0]);
            }
            samples_to_srgb(&grays, SampleKind::Gray, &profile, pixels);
            return None;
        }
        // CMYK、Lab 等的配置文件对不上 RGB 像素：不用，按 sRGB 显示。
        _ => return None,
    }
    if converts_in_core() {
        to_srgb(pixels, &profile, order);
        None
    } else {
        Some(Arc::new(profile))
    }
}

/// 把不预乘的像素从内嵌 ICC 转成 sRGB（原地）。不认识的配置文件原样保留，返回 false。
/// 与 sRGB 一致的配置文件（相机、手机照片大多带着）不逐像素转换：先转一组探测色，都不变就跳过。
/// 大图分块多线程转换。
pub fn to_srgb(pixels: &mut [u8], icc: &[u8], order: ChannelOrder) -> bool {
    // 只认 RGB 的配置文件：灰度的会被当成"灰度 + Alpha"两个字节一个像素来读，画面全乱。
    if icc.get(16..20) != Some(b"RGB ") {
        return false;
    }
    let Some(input) = qcms::Profile::new_from_slice(icc, false) else {
        return false;
    };
    let mut output = qcms::Profile::new_sRGB();
    output.precache_output_transform();
    let data_type = match order {
        ChannelOrder::Rgba => qcms::DataType::RGBA8,
        ChannelOrder::Bgra => qcms::DataType::BGRA8,
    };
    let Some(transform) =
        qcms::Transform::new(&input, &output, data_type, qcms::Intent::Perceptual)
    else {
        return false;
    };
    if is_identity(&transform) {
        return true;
    }
    const CHUNK_PIXELS: usize = 1 << 20;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8);
    let pixel_count = pixels.len() / 4;
    if threads == 1 || pixel_count < 2 * CHUNK_PIXELS {
        transform.apply(pixels);
        return true;
    }
    let chunk_bytes = pixel_count.div_ceil(threads) * 4;
    std::thread::scope(|scope| {
        for chunk in pixels.chunks_mut(chunk_bytes) {
            let transform = &transform;
            scope.spawn(move || transform.apply(chunk));
        }
    });
    true
}

/// sRGB → 显示器配置文件的转换（"使用监视器的 ICC 设置"），BGRA8、感知意图。读不懂的配置文件、
/// 不是 RGB 显示器的、与 sRGB 一样的（转了也不变）返回空。
pub fn srgb_to_display(icc: &[u8]) -> Option<qcms::Transform> {
    let mut output = qcms::Profile::new_from_slice(icc, false)?;
    output.precache_output_transform();
    let input = qcms::Profile::new_sRGB();
    let transform = qcms::Transform::new(&input, &output, qcms::DataType::BGRA8, qcms::Intent::Perceptual)?;
    // 作输出时 qcms 的反向曲线有取整：与 sRGB 一样的配置文件纯色也会差 2（实测），差 2 以内算不用转。
    (!is_identity_within(&transform, 2)).then_some(transform)
}

/// 按转换改 BGRA8 预乘的像素（原地）：半透明的先还原成不预乘的颜色，转完再预乘回去；全透明的保持 0。
/// 大图分块多线程（与 [`to_srgb`] 相同）。
pub fn transform_premultiplied_bgra(transform: &qcms::Transform, pixels: &mut [u8]) {
    let convert = |chunk: &mut [u8]| {
        let partial = chunk.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
        if partial {
            for p in chunk.as_chunks_mut::<4>().0 {
                let a = u32::from(p[3]);
                if a != 0 && a != 255 {
                    for c in &mut p[..3] {
                        *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                    }
                }
            }
        }
        transform.apply(chunk);
        if partial {
            for p in chunk.as_chunks_mut::<4>().0 {
                let a = u32::from(p[3]);
                if a != 255 {
                    for c in &mut p[..3] {
                        *c = ((u32::from(*c) * a + 127) / 255) as u8;
                    }
                }
            }
        }
    };
    const CHUNK_PIXELS: usize = 1 << 20;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8);
    let pixel_count = pixels.len() / 4;
    let whole = pixel_count * 4;
    let pixels = &mut pixels[..whole];
    if threads == 1 || pixel_count < 2 * CHUNK_PIXELS {
        convert(pixels);
        return;
    }
    let chunk_bytes = pixel_count.div_ceil(threads) * 4;
    std::thread::scope(|scope| {
        for chunk in pixels.chunks_mut(chunk_bytes) {
            let convert = &convert;
            scope.spawn(move || convert(chunk));
        }
    });
}

/// 不是 RGB 的像素：界面只认 RGB，CMYK、灰度的配置文件只能在解码时用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    /// 每像素 4 个字节，按 ICC 的约定：0 = 没有油墨。
    Cmyk,
    /// 每像素 1 个字节。
    Gray,
}

/// 按内嵌的配置文件把 CMYK 或灰度转成 sRGB，写进 `rgba`（不预乘）的 R、G、B，Alpha 不动。
/// 三个平台都在核心里转（macOS 的 ColorSync 只拿得到 RGB 像素）。配置文件读不了、颜色空间对不上时
/// 不改 `rgba`，返回 false，由调用方改用简单的办法。大图分块多线程。
pub fn samples_to_srgb(samples: &[u8], kind: SampleKind, icc: &[u8], rgba: &mut [u8]) -> bool {
    let (space, data_type, channels): (&[u8], _, usize) = match kind {
        SampleKind::Cmyk => (b"CMYK", qcms::DataType::CMYK, 4),
        SampleKind::Gray => (b"GRAY", qcms::DataType::Gray8, 1),
    };
    let pixel_count = rgba.len() / 4;
    if icc.get(16..20) != Some(space) || samples.len() != pixel_count * channels {
        return false;
    }
    let Some(input) = qcms::Profile::new_from_slice(icc, false) else {
        return false;
    };
    let mut output = qcms::Profile::new_sRGB();
    output.precache_output_transform();
    let Some(transform) = qcms::Transform::new_to(
        &input,
        &output,
        data_type,
        qcms::DataType::RGB8,
        qcms::Intent::Perceptual,
    ) else {
        return false;
    };
    const BLOCK: usize = 1 << 16;
    let convert = |src: &[u8], dst: &mut [u8]| {
        let mut rgb = vec![0u8; BLOCK.min(dst.len() / 4) * 3];
        for (s, d) in src.chunks(BLOCK * channels).zip(dst.chunks_mut(BLOCK * 4)) {
            let rgb = &mut rgb[..d.len() / 4 * 3];
            transform.convert(s, rgb);
            for (px, c) in d
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(rgb.as_chunks::<3>().0)
            {
                px[..3].copy_from_slice(c);
            }
        }
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8);
    if threads == 1 || pixel_count < 2 * BLOCK {
        convert(samples, rgba);
        return true;
    }
    let per_thread = pixel_count.div_ceil(threads);
    std::thread::scope(|scope| {
        for (src, dst) in samples
            .chunks(per_thread * channels)
            .zip(rgba.chunks_mut(per_thread * 4))
        {
            let convert = &convert;
            scope.spawn(move || convert(src, dst));
        }
    });
    true
}

/// 转换是否几乎不改变颜色：灰阶、原色、二次色和一些中间色，每个通道最多差 1。
/// 通道顺序不影响判断（探测色对 R、B 对称地取遍）。
fn is_identity(transform: &qcms::Transform) -> bool {
    is_identity_within(transform, 1)
}

/// 一组探测色（6×6×6）转换后每个通道最多差 `tolerance`。
fn is_identity_within(transform: &qcms::Transform, tolerance: u8) -> bool {
    const LEVELS: [u8; 6] = [0, 51, 102, 153, 204, 255];
    let mut probe = Vec::with_capacity(LEVELS.len().pow(3) * 4);
    for r in LEVELS {
        for g in LEVELS {
            for b in LEVELS {
                probe.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }
    let original = probe.clone();
    transform.apply(&mut probe);
    probe
        .iter()
        .zip(&original)
        .all(|(a, b)| a.abs_diff(*b) <= tolerance)
}

/// 本平台是否在核心里把内嵌 ICC 转成 sRGB（macOS 交给 ColorSync，不在这里转）。
pub fn converts_in_core() -> bool {
    !cfg!(target_os = "macos")
}

/// 测试用的 ICC 配置文件：sRGB 原色、线性 Gamma（1.0）。灰度 128 按它解释，转成 sRGB 后约为 188，
/// 可以据此确认转换确实发生了。ICC v2 显示器配置文件，矩阵加曲线。
#[doc(hidden)]
pub fn linear_srgb_test_profile() -> Vec<u8> {
    test_profile(false)
}

/// 测试用：与 sRGB 相同的配置文件（sRGB 原色加 sRGB 的参数曲线）。
#[doc(hidden)]
pub fn srgb_test_profile() -> Vec<u8> {
    test_profile(true)
}

/// 测试用的线性灰度配置文件（Gamma 1.0）：中灰 128 转成 sRGB 是 188 左右。
pub fn linear_gray_test_profile() -> Vec<u8> {
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", description_tag(b"Linear gray (Anyview test)\0")),
        (b"wtpt", xyz(0.9642, 1.0, 0.8249)),
        (b"kTRC", linear_curve_tag()),
    ];
    assemble_profile(b"GRAY", &tags)
}

fn s15(value: f64) -> [u8; 4] {
    ((value * 65536.0).round() as i32).to_be_bytes()
}

fn xyz(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut tag = b"XYZ \0\0\0\0".to_vec();
    for v in [x, y, z] {
        tag.extend_from_slice(&s15(v));
    }
    tag
}

/// 曲线类型，只有一个 Gamma 值（u8Fixed8）1.0。
fn linear_curve_tag() -> Vec<u8> {
    let mut tag = b"curv\0\0\0\0".to_vec();
    tag.extend_from_slice(&1u32.to_be_bytes());
    tag.extend_from_slice(&0x0100u16.to_be_bytes());
    tag
}

fn description_tag(text: &[u8]) -> Vec<u8> {
    let mut tag = b"desc\0\0\0\0".to_vec();
    tag.extend_from_slice(&(text.len() as u32).to_be_bytes());
    tag.extend_from_slice(text);
    // Unicode 与 ScriptCode 部分留空。
    tag.extend_from_slice(&[0; 4 + 4 + 2 + 1 + 67]);
    tag
}

fn test_profile(srgb_curve: bool) -> Vec<u8> {
    // 线性：曲线类型，只有一个 Gamma 值（u8Fixed8）1.0。sRGB：参数曲线第 3 种（IEC 61966-2-1 的分段函数）。
    let curve = if srgb_curve {
        let mut tag = b"para\0\0\0\0".to_vec();
        tag.extend_from_slice(&3u16.to_be_bytes());
        tag.extend_from_slice(&[0, 0]);
        for v in [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045] {
            tag.extend_from_slice(&s15(v));
        }
        tag
    } else {
        linear_curve_tag()
    };
    let description = description_tag(if srgb_curve {
        b"sRGB (Anyview test)\0"
    } else {
        b"Linear sRGB (Anyview test)\0"
    });
    // D50 下的 sRGB 原色（与标准 sRGB 配置文件相同）和白点。
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", description),
        (b"wtpt", xyz(0.9642, 1.0, 0.8249)),
        (b"rXYZ", xyz(0.436_074_7, 0.222_504_5, 0.013_932_2)),
        (b"gXYZ", xyz(0.385_064_9, 0.716_878_6, 0.097_104_5)),
        (b"bXYZ", xyz(0.143_080_4, 0.060_616_9, 0.714_173_3)),
        (b"rTRC", curve.clone()),
        (b"gTRC", curve.clone()),
        (b"bTRC", curve),
    ];
    assemble_profile(b"RGB ", &tags)
}

/// 配置文件头（v2.1，显示器类，PCS 为 XYZ，D50 白点）加标签表和标签。
fn assemble_profile(space: &[u8; 4], tags: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let table_len = 4 + 12 * tags.len();
    let mut offset = 128 + table_len;
    let mut table = (tags.len() as u32).to_be_bytes().to_vec();
    let mut data = Vec::new();
    for (signature, tag) in tags {
        table.extend_from_slice(*signature);
        table.extend_from_slice(&(offset as u32).to_be_bytes());
        table.extend_from_slice(&(tag.len() as u32).to_be_bytes());
        data.extend_from_slice(tag);
        // 每个标签按 4 字节对齐。
        while data.len() % 4 != 0 {
            data.push(0);
        }
        offset = 128 + table_len + data.len();
    }
    let size = 128 + table.len() + data.len();
    let mut header = vec![0u8; 128];
    header[0..4].copy_from_slice(&(size as u32).to_be_bytes());
    header[8..12].copy_from_slice(&0x0210_0000u32.to_be_bytes());
    header[12..16].copy_from_slice(b"mntr");
    header[16..20].copy_from_slice(space);
    header[20..24].copy_from_slice(b"XYZ ");
    header[36..40].copy_from_slice(b"acsp");
    for (i, v) in [0.9642, 1.0, 0.8249].into_iter().enumerate() {
        header[68 + i * 4..72 + i * 4].copy_from_slice(&s15(v));
    }
    [header, table, data].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_profile_brightens_mid_gray_in_srgb() {
        let profile = linear_srgb_test_profile();
        let mut pixels = vec![128, 128, 128, 255, 0, 0, 0, 255, 255, 255, 255, 128];
        assert!(to_srgb(&mut pixels, &profile, ChannelOrder::Rgba));
        // 线性 0.502 → sRGB 0.736 ≈ 188；黑白不变；Alpha 不变。
        for c in &pixels[..3] {
            assert!((186..=190).contains(c), "{pixels:?}");
        }
        assert_eq!(&pixels[4..8], &[0, 0, 0, 255]);
        assert!(pixels[8] >= 253 && pixels[11] == 128, "{pixels:?}");
    }

    #[test]
    fn a_gray_profile_on_rgba_pixels_is_converted_as_gray() {
        let profile = linear_gray_test_profile();
        let original = vec![128, 128, 128, 255, 0, 0, 0, 255, 255, 255, 255, 128, 64, 64, 64, 255];
        // 直接当 RGB 配置文件用：不转（以前会把像素读乱）。
        let mut pixels = original.clone();
        assert!(!to_srgb(&mut pixels, &profile, ChannelOrder::Rgba));
        assert_eq!(pixels, original);
        // manage：按灰度转，每个像素还在原来的位置，Alpha 不变，不再带配置文件。
        for order in [ChannelOrder::Rgba, ChannelOrder::Bgra] {
            let mut pixels = original.clone();
            assert!(manage(&mut pixels, Some(profile.clone()), order).is_none());
            for c in &pixels[..3] {
                assert!((186..=190).contains(c), "{pixels:?}");
            }
            assert_eq!(&pixels[4..8], &[0, 0, 0, 255]);
            assert!(pixels[8] >= 253 && pixels[11] == 128, "{pixels:?}");
            assert!(pixels[12] > 64 && pixels[12] == pixels[13] && pixels[13] == pixels[14]);
            assert_eq!(pixels[15], 255);
        }
        // 彩色的像素带了灰度配置文件（文件写错了）：不用它，像素不动。
        let colorful = vec![200, 30, 30, 255, 128, 128, 128, 255];
        let mut pixels = colorful.clone();
        assert!(manage(&mut pixels, Some(profile.clone()), ChannelOrder::Rgba).is_none());
        assert_eq!(pixels, colorful);
        // CMYK 的配置文件对不上：像素不动，不带配置文件。
        let mut cmyk = profile.clone();
        cmyk[16..20].copy_from_slice(b"CMYK");
        let mut pixels = original.clone();
        assert!(manage(&mut pixels, Some(cmyk), ChannelOrder::Rgba).is_none());
        assert_eq!(pixels, original);
    }

    /// 转换耗时：1200 万像素。手动运行：
    /// `cargo test -p anyview-core --release -- --ignored icc_12mp --nocapture`
    #[test]
    #[ignore]
    fn icc_12mp_cost() {
        let profile = linear_srgb_test_profile();
        let mut pixels: Vec<u8> = (0..4000 * 3000 * 4).map(|i| (i % 251) as u8).collect();
        let start = std::time::Instant::now();
        assert!(to_srgb(&mut pixels, &profile, ChannelOrder::Bgra));
        println!(
            "ICC → sRGB，1200 万像素：{:.1} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }

    #[test]
    fn unknown_profiles_are_left_alone() {
        let mut pixels = vec![10, 20, 30, 255];
        assert!(!to_srgb(&mut pixels, b"not a profile", ChannelOrder::Rgba));
        assert_eq!(pixels, [10, 20, 30, 255]);
    }

    #[test]
    fn srgb_profiles_are_recognized_and_skipped() {
        let profile = srgb_test_profile();
        let input = qcms::Profile::new_from_slice(&profile, false).unwrap();
        let mut output = qcms::Profile::new_sRGB();
        output.precache_output_transform();
        let transform = qcms::Transform::new(
            &input,
            &output,
            qcms::DataType::BGRA8,
            qcms::Intent::Perceptual,
        )
        .unwrap();
        assert!(is_identity(&transform));
        // 跳过时像素一点不变（逐像素转换会有 ±1 的舍入差）。
        let original: Vec<u8> = (0..4096u32).map(|i| (i * 37 % 256) as u8).collect();
        let mut pixels = original.clone();
        assert!(to_srgb(&mut pixels, &profile, ChannelOrder::Bgra));
        assert_eq!(pixels, original);
        // 线性配置文件不能当成 sRGB。
        let linear = qcms::Profile::new_from_slice(&linear_srgb_test_profile(), false).unwrap();
        let transform =
            qcms::Transform::new(&linear, &output, qcms::DataType::RGBA8, qcms::Intent::Perceptual)
                .unwrap();
        assert!(!is_identity(&transform));
    }

    #[test]
    fn threaded_conversion_matches_single_pass() {
        let profile = linear_srgb_test_profile();
        let original: Vec<u8> = (0..(3 << 20) * 4).map(|i: u32| (i % 253) as u8).collect();
        let mut threaded = original.clone();
        assert!(to_srgb(&mut threaded, &profile, ChannelOrder::Rgba));
        let input = qcms::Profile::new_from_slice(&profile, false).unwrap();
        let mut output = qcms::Profile::new_sRGB();
        output.precache_output_transform();
        let transform =
            qcms::Transform::new(&input, &output, qcms::DataType::RGBA8, qcms::Intent::Perceptual)
                .unwrap();
        let mut single = original;
        transform.apply(&mut single);
        assert!(threaded == single);
    }
}
