//! raw/：RAW 内嵌预览的测试文件。不是真的 RAW 数据：按各家文件结构写出带 JPEG 预览的容器，内容由公式确定。
//!
//! 预览图都是四色块（左上红、右上绿、左下蓝、右下黄，见 `quadrants`），各张尺寸不同，测试能看出选中的是哪一张。
//! 填充的"RAW 数据"不含 0xFF，不会凑出 JPEG 标记。
//!
//! | 文件 | 结构 | 选中的预览 | 方向 | 显示尺寸 |
//! | --- | --- | --- | --- | --- |
//! | a_tiff.nef | 大端 TIFF：IFD1 缩略图 160x120；SubIFD 预览 640x480（0x201/0x202）；SubIFD RAW（不压缩的假数据） | 640x480 | 1 | 640x480 |
//! | b_canon.cr2 | 小端 TIFF + "CR"：IFD0 单条带 JPEG 800x600；IFD1 缩略图 160x120；IFD3 无损 JPEG2000x1500 不能算 | 800x600 | 1 | 800x600 |
//! | c_olympus.orf | "IIRO"：IFD1 缩略图 160x120；1024x768 只在 EXIF 的厂商注释里（要整文件查找） | 1024x768 | 1 | 1024x768 |
//! | d_panasonic.rw2 | "IIU\0"：IFD0 的 JpgFromRaw（0x2E） | 1200x800 | 1 | 1200x800 |
//! | e_fuji.raf | RAF 文件头写的 JPEG 位置；JPEG 自己的 EXIF 写方向和拍摄信息 | 1080x720 | 8 | 720x1080 |
//! | f_canon.cr3 | ISO BMFF：THMB 160x120、PRVW 1200x800、第一条轨道 1800x1200；CMT1 写方向，CMT2 写拍摄参数 | 1800x1200 | 3 | 1800x1200 |
//! | g_rotated.dng | 小端 TIFF：IFD0 不压缩的小缩略图；SubIFD RAW 是无损 JPEG 2000x1500（不能算）；SubIFD 预览 | 1024x768 | 6 | 768x1024 |
//! | h_unknown.x3f | 认不出的结构：JPEG 夹在填充数据中间（要整文件查找）；没有拍摄信息 | 1100x700 | 1 | 1100x700 |
//! | i_minolta.mrw | "\0MRM" 的 TTW 块里是大端 TIFF（方向、拍摄信息）；预览在厂商注释里（要整文件查找） | 640x480 | 8 | 480x640 |
//!
//! 除 h_unknown.x3f 外都写了相机型号和拍摄参数（快门、光圈、ISO、拍摄时间）。

use std::fs;
use std::path::Path;

use crate::{encode_jpeg, ensure_dir, quadrants};

/// CR3：moov 里佳能元数据的 uuid 盒。
const CANON_UUID: [u8; 16] = [
    0x85, 0xC0, 0xB6, 0x87, 0x82, 0x0F, 0x11, 0xE0, 0x81, 0x11, 0xF4, 0xCE, 0x46, 0x2B, 0x6A, 0x48,
];
/// CR3：顶层装 PRVW 预览的 uuid 盒。
const PREVIEW_UUID: [u8; 16] = [
    0xEA, 0xF4, 0x2B, 0x5E, 0x1C, 0x98, 0x4B, 0x88, 0xB9, 0xFB, 0xB7, 0xDC, 0x40, 0x6E, 0x4D, 0x16,
];

/// 生成 raw/ 下的文件，返回文件数。
pub fn generate(dir: &Path) -> crate::Result<usize> {
    ensure_dir(dir)?;
    let files = [
        ("a_tiff.nef", nef()?),
        ("b_canon.cr2", cr2()?),
        ("c_olympus.orf", orf()?),
        ("d_panasonic.rw2", rw2()?),
        ("e_fuji.raf", raf()?),
        ("f_canon.cr3", cr3()?),
        ("g_rotated.dng", dng()?),
        ("h_unknown.x3f", x3f()?),
        ("i_minolta.mrw", mrw()?),
    ];
    for (name, bytes) in &files {
        fs::write(dir.join(name), bytes)?;
    }
    Ok(files.len())
}

/// 预览图：w x h 的四色块 JPEG。
fn preview(width: u32, height: u32) -> crate::Result<Vec<u8>> {
    encode_jpeg(&quadrants(width, height), 90)
}

/// 假的 RAW 数据：不含 0xFF。
fn filler(len: usize, seed: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7 + seed) % 251) as u8).collect()
}

/// RAW 数据常用的无损 JPEG：结构完整，但不是能显示的预览图。
fn lossless_jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut j = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x14, 0x00, 1];
    j.extend_from_slice(&[0; 15]);
    j.push(0);
    j.extend_from_slice(&[0xFF, 0xC3, 0x00, 0x0B, 14]);
    j.extend_from_slice(&height.to_be_bytes());
    j.extend_from_slice(&width.to_be_bytes());
    j.extend_from_slice(&[1, 1, 0x11, 0]);
    j.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 1, 1, 0x00, 1, 0, 0]);
    j.extend_from_slice(&filler(6000, 3));
    j.extend_from_slice(&[0xFF, 0xD9]);
    j
}

/// TIFF 目录项的值。
enum Val<'a> {
    Short(u16),
    Long(u32),
    Longs(Vec<u32>),
    Rational(u32, u32),
    Ascii(&'a str),
    Bytes(&'a [u8]),
    /// UNDEFINED，值是已经写进文件的一段（位置、长度）。
    At(u32, u32),
}

/// 简单的 TIFF 写入器：先放数据块，再从下往上写目录（子目录、下一个目录先写，偏移量都已知），
/// 最后在文件头填上 IFD0 的位置。偏移量相对于 TIFF 头。
struct TiffWriter {
    bytes: Vec<u8>,
    little: bool,
}

impl TiffWriter {
    /// `order` 为 b"II" 或 b"MM"；`magic` 为 42（标准 TIFF），或 ORF（0x4F52）、RW2（0x55）的魔数。
    fn new(order: &[u8; 2], magic: u16) -> Self {
        let mut w = Self {
            bytes: order.to_vec(),
            little: order == b"II",
        };
        let magic = w.u16(magic);
        w.bytes.extend_from_slice(&magic);
        w.bytes.extend_from_slice(&[0; 4]);
        w
    }

    fn u16(&self, v: u16) -> [u8; 2] {
        if self.little { v.to_le_bytes() } else { v.to_be_bytes() }
    }

    fn u32(&self, v: u32) -> [u8; 4] {
        if self.little { v.to_le_bytes() } else { v.to_be_bytes() }
    }

    fn align(&mut self) {
        if self.bytes.len() % 2 == 1 {
            self.bytes.push(0);
        }
    }

    /// 放一段数据，返回它的位置。
    fn blob(&mut self, data: &[u8]) -> u32 {
        self.align();
        let at = self.bytes.len() as u32;
        self.bytes.extend_from_slice(data);
        at
    }

    /// 写一个目录（项按标签排序，放不进 4 字节的值紧跟在目录后面），返回它的位置。
    fn ifd(&mut self, mut entries: Vec<(u16, Val<'_>)>, next: u32) -> u32 {
        entries.sort_by_key(|(tag, _)| *tag);
        self.align();
        let at = self.bytes.len();
        let count = self.u16(entries.len() as u16);
        self.bytes.extend_from_slice(&count);
        let table = self.bytes.len();
        self.bytes.resize(table + 12 * entries.len(), 0);
        let next = self.u32(next);
        self.bytes.extend_from_slice(&next);
        for (i, (tag, value)) in entries.iter().enumerate() {
            let (kind, count, data): (u16, u32, Vec<u8>) = match value {
                Val::Short(v) => (3, 1, self.u16(*v).to_vec()),
                Val::Long(v) => (4, 1, self.u32(*v).to_vec()),
                Val::Longs(vs) => (4, vs.len() as u32, vs.iter().flat_map(|v| self.u32(*v)).collect()),
                Val::Rational(n, d) => (5, 1, [self.u32(*n), self.u32(*d)].concat()),
                Val::Ascii(s) => {
                    let mut d = s.as_bytes().to_vec();
                    d.push(0);
                    (2, d.len() as u32, d)
                }
                Val::Bytes(b) => (1, b.len() as u32, b.to_vec()),
                Val::At(_, len) => (7, *len, Vec::new()),
            };
            let field = match value {
                Val::At(offset, _) => self.u32(*offset),
                _ if data.len() <= 4 => {
                    let mut f = [0u8; 4];
                    f[..data.len()].copy_from_slice(&data);
                    f
                }
                _ => {
                    let offset = self.blob(&data);
                    self.u32(offset)
                }
            };
            let p = table + 12 * i;
            let (tag, kind, count) = (self.u16(*tag), self.u16(kind), self.u32(count));
            self.bytes[p..p + 2].copy_from_slice(&tag);
            self.bytes[p + 2..p + 4].copy_from_slice(&kind);
            self.bytes[p + 4..p + 8].copy_from_slice(&count);
            self.bytes[p + 8..p + 12].copy_from_slice(&field);
        }
        at as u32
    }

    /// 在文件头填上 IFD0 的位置，返回整个 TIFF。
    fn finish(mut self, ifd0: u32) -> Vec<u8> {
        let at = self.u32(ifd0);
        self.bytes[4..8].copy_from_slice(&at);
        self.bytes
    }
}

/// IFD0 的厂商、型号、方向和 EXIF 目录指针。
fn camera<'a>(make: &'a str, model: &'a str, orientation: u16, exif: u32) -> Vec<(u16, Val<'a>)> {
    vec![
        (0x10F, Val::Ascii(make)),
        (0x110, Val::Ascii(model)),
        (0x112, Val::Short(orientation)),
        (0x8769, Val::Long(exif)),
    ]
}

/// EXIF 目录：快门（秒，分数）、光圈（分数）、ISO、拍摄时间。
fn shot(exposure: (u32, u32), f_number: (u32, u32), iso: u16, taken: &str) -> Vec<(u16, Val<'_>)> {
    vec![
        (0x829A, Val::Rational(exposure.0, exposure.1)),
        (0x829D, Val::Rational(f_number.0, f_number.1)),
        (0x8827, Val::Short(iso)),
        (0x9003, Val::Ascii(taken)),
    ]
}

/// 缩略图目录：JPEG（压缩方式 6）的位置和长度。
fn thumbnail_ifd(t: &mut TiffWriter, jpeg: &[u8], next: u32) -> u32 {
    let at = t.blob(jpeg);
    t.ifd(
        vec![
            (0x103, Val::Short(6)),
            (0x201, Val::Long(at)),
            (0x202, Val::Long(jpeg.len() as u32)),
        ],
        next,
    )
}

/// Nikon NEF：大端 TIFF。IFD0 → IFD1（缩略图）；IFD0 的 SubIFD：预览（0x201/0x202）、RAW（不压缩）。
fn nef() -> crate::Result<Vec<u8>> {
    let big = preview(640, 480)?;
    let mut t = TiffWriter::new(b"MM", 42);
    let big_at = t.blob(&big);
    let raw = filler(64 * 32 * 2, 1);
    let raw_at = t.blob(&raw);
    let exif = t.ifd(shot((1, 250), (56, 10), 200, "2026:11:05 10:00:00"), 0);
    let preview_ifd = t.ifd(
        vec![
            (0xFE, Val::Long(1)),
            (0x103, Val::Short(6)),
            (0x201, Val::Long(big_at)),
            (0x202, Val::Long(big.len() as u32)),
        ],
        0,
    );
    let raw_ifd = t.ifd(
        vec![
            (0xFE, Val::Long(0)),
            (0x100, Val::Long(64)),
            (0x101, Val::Long(32)),
            (0x102, Val::Short(16)),
            (0x103, Val::Short(1)),
            (0x111, Val::Long(raw_at)),
            (0x117, Val::Long(raw.len() as u32)),
        ],
        0,
    );
    let ifd1 = thumbnail_ifd(&mut t, &preview(160, 120)?, 0);
    let mut ifd0 = camera("NIKON CORPORATION", "NIKON D850", 1, exif);
    ifd0.push((0xFE, Val::Long(1)));
    ifd0.push((0x14A, Val::Longs(vec![preview_ifd, raw_ifd])));
    let ifd0 = t.ifd(ifd0, ifd1);
    Ok(t.finish(ifd0))
}

/// Canon CR2：小端 TIFF，文件头后面是 "CR"、版本和 RAW 目录的位置。IFD0 的单条带是全尺寸 JPEG；
/// IFD1 缩略图；IFD2 不压缩的小图；IFD3 是 RAW（无损 JPEG）。
fn cr2() -> crate::Result<Vec<u8>> {
    let big = preview(800, 600)?;
    let lossless = lossless_jpeg(2000, 1500);
    let mut t = TiffWriter::new(b"II", 42);
    t.bytes.extend_from_slice(b"CR\x02\0");
    t.bytes.extend_from_slice(&[0; 4]);
    let big_at = t.blob(&big);
    let raw_at = t.blob(&lossless);
    let rgb = filler(8 * 6 * 3, 2);
    let rgb_at = t.blob(&rgb);
    let exif = t.ifd(shot((1, 200), (8, 1), 100, "2026:11:05 08:00:00"), 0);
    let ifd3 = t.ifd(
        vec![
            (0x103, Val::Short(6)),
            (0x111, Val::Long(raw_at)),
            (0x117, Val::Long(lossless.len() as u32)),
        ],
        0,
    );
    let ifd2 = t.ifd(
        vec![
            (0x100, Val::Long(8)),
            (0x101, Val::Long(6)),
            (0x103, Val::Short(1)),
            (0x111, Val::Long(rgb_at)),
            (0x117, Val::Long(rgb.len() as u32)),
        ],
        ifd3,
    );
    let ifd1 = thumbnail_ifd(&mut t, &preview(160, 120)?, ifd2);
    let mut ifd0 = camera("Canon", "Canon EOS 5D Mark IV", 1, exif);
    ifd0.extend([
        (0x100, Val::Long(800)),
        (0x101, Val::Long(600)),
        (0x103, Val::Short(6)),
        (0x111, Val::Long(big_at)),
        (0x117, Val::Long(big.len() as u32)),
    ]);
    let ifd0 = t.ifd(ifd0, ifd1);
    let mut bytes = t.finish(ifd0);
    bytes[12..16].copy_from_slice(&ifd3.to_le_bytes());
    Ok(bytes)
}

/// Olympus ORF："IIRO"。IFD1 只有小缩略图；大的预览放在 EXIF 目录的厂商注释（0x927C）里。
fn orf() -> crate::Result<Vec<u8>> {
    let mut t = TiffWriter::new(b"II", 0x4F52);
    let mut note = b"OLYMPUS\0II\x03\0".to_vec();
    note.extend_from_slice(&filler(100, 3));
    note.extend_from_slice(&preview(1024, 768)?);
    note.extend_from_slice(&filler(50, 4));
    let note_at = t.blob(&note);
    let mut exif = shot((1, 320), (4, 1), 250, "2026:11:05 12:00:00");
    exif.push((0x927C, Val::At(note_at, note.len() as u32)));
    let exif = t.ifd(exif, 0);
    let ifd1 = thumbnail_ifd(&mut t, &preview(160, 120)?, 0);
    let ifd0 = t.ifd(camera("OLYMPUS CORPORATION", "E-M1MarkIII", 1, exif), ifd1);
    Ok(t.finish(ifd0))
}

/// Panasonic RW2："IIU\0"。预览是 IFD0 里 JpgFromRaw（0x2E）的值。
fn rw2() -> crate::Result<Vec<u8>> {
    let big = preview(1200, 800)?;
    let mut t = TiffWriter::new(b"II", 0x55);
    let big_at = t.blob(&big);
    let exif = t.ifd(shot((1, 160), (28, 10), 320, "2026:11:05 13:00:00"), 0);
    let mut ifd0 = camera("Panasonic", "DC-S5M2", 1, exif);
    ifd0.push((0x2E, Val::At(big_at, big.len() as u32)));
    let ifd0 = t.ifd(ifd0, 0);
    Ok(t.finish(ifd0))
}

/// Fujifilm RAF：文件头第 84、88 字节（大端）是 JPEG 的位置和长度；方向和拍摄信息在 JPEG 自己的 EXIF 里。
fn raf() -> crate::Result<Vec<u8>> {
    let mut t = TiffWriter::new(b"II", 42);
    let exif = t.ifd(shot((1, 1000), (28, 10), 125, "2026:11:05 07:45:00"), 0);
    let ifd0 = t.ifd(camera("FUJIFILM", "X-T5", 8, exif), 0);
    let tiff = t.finish(ifd0);
    let mut app1 = vec![0xFF, 0xE1];
    app1.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);
    let jpeg = crate::insert_app1(&preview(1080, 720)?, &app1);
    let cfa = filler(4096, 5);
    let jpeg_at = 160u32;
    let cfa_at = jpeg_at + jpeg.len() as u32;
    let mut out = b"FUJIFILMCCD-RAW 0201FF129502".to_vec();
    let mut model = b"X-T5".to_vec();
    model.resize(32, 0);
    out.extend_from_slice(&model);
    out.extend_from_slice(b"0100");
    out.resize(84, 0);
    for value in [jpeg_at, jpeg.len() as u32, cfa_at, 0, cfa_at, cfa.len() as u32] {
        out.extend_from_slice(&value.to_be_bytes());
    }
    out.resize(jpeg_at as usize, 0);
    out.extend_from_slice(&jpeg);
    out.extend_from_slice(&cfa);
    Ok(out)
}

/// ISO BMFF 的盒子：长度、类型、内容。
fn bmff(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = ((8 + body.len()) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(body);
    b
}

/// Canon CR3：ftyp、moov（佳能 uuid：CNCV、CMT1、CMT2、THMB；第一条轨道的 stsz、co64）、预览 uuid（PRVW）、mdat（全尺寸 JPEG）。
fn cr3() -> crate::Result<Vec<u8>> {
    let thumb = preview(160, 120)?;
    let prvw = preview(1200, 800)?;
    let full = preview(1800, 1200)?;
    let cmt1 = {
        let mut t = TiffWriter::new(b"II", 42);
        let ifd0 = t.ifd(
            vec![
                (0x10F, Val::Ascii("Canon")),
                (0x110, Val::Ascii("Canon EOS R5")),
                (0x112, Val::Short(3)),
            ],
            0,
        );
        t.finish(ifd0)
    };
    let cmt2 = {
        let mut t = TiffWriter::new(b"II", 42);
        let ifd0 = t.ifd(shot((1, 125), (4, 1), 400, "2026:11:05 09:30:00"), 0);
        t.finish(ifd0)
    };
    let mut canon = CANON_UUID.to_vec();
    canon.extend(bmff(b"CNCV", b"CanonCR3_001/00.10.00/00.00.00"));
    canon.extend(bmff(b"CMT1", &cmt1));
    canon.extend(bmff(b"CMT2", &cmt2));
    // THMB：版本和标志、宽、高、JPEG 长度、两个未用的 16 位数，然后是 JPEG。
    let mut thmb = vec![0u8; 4];
    thmb.extend_from_slice(&160u16.to_be_bytes());
    thmb.extend_from_slice(&120u16.to_be_bytes());
    thmb.extend_from_slice(&(thumb.len() as u32).to_be_bytes());
    thmb.extend_from_slice(&[0, 1, 0, 0]);
    thmb.extend_from_slice(&thumb);
    canon.extend(bmff(b"THMB", &thmb));
    // 第一条轨道：一个样本，长度在 stsz，位置在 co64（先写 0，下面补上）。
    let stsz = bmff(
        b"stsz",
        &[[0u8; 4], 0u32.to_be_bytes(), 1u32.to_be_bytes(), (full.len() as u32).to_be_bytes()].concat(),
    );
    let co64 = bmff(b"co64", &[&[0u8; 4][..], &1u32.to_be_bytes(), &0u64.to_be_bytes()].concat());
    let trak = bmff(b"trak", &bmff(b"mdia", &bmff(b"minf", &bmff(b"stbl", &[stsz, co64].concat()))));
    let mut moov = bmff(b"moov", &[bmff(b"uuid", &canon), trak].concat());
    // PRVW：4 个未用的字节、1、宽、高、1、JPEG 长度，然后是 JPEG。预览 uuid 盒里 UUID 之后先有 8 个字节。
    let mut prvw_box = 0u32.to_be_bytes().to_vec();
    for value in [1u16, 1200, 800, 1] {
        prvw_box.extend_from_slice(&value.to_be_bytes());
    }
    prvw_box.extend_from_slice(&(prvw.len() as u32).to_be_bytes());
    prvw_box.extend_from_slice(&prvw);
    let mut preview_uuid = PREVIEW_UUID.to_vec();
    preview_uuid.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    preview_uuid.extend(bmff(b"PRVW", &prvw_box));
    let preview_uuid = bmff(b"uuid", &preview_uuid);
    let ftyp = bmff(b"ftyp", &[&b"crx "[..], &1u32.to_be_bytes(), b"crx ", b"isom"].concat());
    // 全尺寸 JPEG 在 mdat 开头；co64 是 moov 的最后 8 个字节。
    let full_at = ftyp.len() + moov.len() + preview_uuid.len() + 8;
    let n = moov.len();
    moov[n - 8..].copy_from_slice(&(full_at as u64).to_be_bytes());
    let mdat = bmff(b"mdat", &[full, filler(8192, 6)].concat());
    Ok([ftyp, moov, preview_uuid, mdat].concat())
}

/// DNG：小端 TIFF。IFD0 是不压缩的小缩略图（方向 6）；SubIFD：RAW（无损 JPEG）、预览（JPEG，YCbCr）。
fn dng() -> crate::Result<Vec<u8>> {
    let big = preview(1024, 768)?;
    let lossless = lossless_jpeg(2000, 1500);
    let mut t = TiffWriter::new(b"II", 42);
    let rgb = filler(16 * 12 * 3, 7);
    let rgb_at = t.blob(&rgb);
    let raw_at = t.blob(&lossless);
    let big_at = t.blob(&big);
    let exif = t.ifd(shot((1, 60), (18, 10), 64, "2026:11:05 11:11:11"), 0);
    let raw_ifd = t.ifd(
        vec![
            (0xFE, Val::Long(0)),
            (0x100, Val::Long(2000)),
            (0x101, Val::Long(1500)),
            (0x103, Val::Short(7)),
            (0x106, Val::Short(32803)),
            (0x111, Val::Long(raw_at)),
            (0x117, Val::Long(lossless.len() as u32)),
        ],
        0,
    );
    let preview_ifd = t.ifd(
        vec![
            (0xFE, Val::Long(1)),
            (0x100, Val::Long(1024)),
            (0x101, Val::Long(768)),
            (0x103, Val::Short(7)),
            (0x106, Val::Short(6)),
            (0x111, Val::Long(big_at)),
            (0x117, Val::Long(big.len() as u32)),
        ],
        0,
    );
    let mut ifd0 = camera("Apple", "iPhone 15 Pro", 6, exif);
    ifd0.extend([
        (0xFE, Val::Long(1)),
        (0x100, Val::Long(16)),
        (0x101, Val::Long(12)),
        (0x103, Val::Short(1)),
        (0x111, Val::Long(rgb_at)),
        (0x117, Val::Long(rgb.len() as u32)),
        (0x14A, Val::Longs(vec![raw_ifd, preview_ifd])),
        (0xC612, Val::Bytes(&[1, 6, 0, 0])),
    ]);
    let ifd0 = t.ifd(ifd0, 0);
    Ok(t.finish(ifd0))
}

/// 认不出的结构（Sigma X3F 的文件头）：JPEG 夹在填充数据中间，只有整文件查找能找到。
fn x3f() -> crate::Result<Vec<u8>> {
    let mut out = b"FOVb".to_vec();
    out.extend_from_slice(&0x0002_0003u32.to_le_bytes());
    out.extend_from_slice(&filler(504, 8));
    out.extend_from_slice(&preview(1100, 700)?);
    out.extend_from_slice(&filler(2048, 9));
    out.extend_from_slice(b"SECd");
    Ok(out)
}

/// MRW 的一块：名字、长度（大端）、内容。
fn mrw_block(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [&name[..], &(body.len() as u32).to_be_bytes(), body].concat()
}

/// Minolta MRW："\0MRM" 加文件头长度；PRD、TTW（大端 TIFF：方向 8、拍摄信息，厂商注释里有预览）、WBG 三块；后面是 RAW 数据。
fn mrw() -> crate::Result<Vec<u8>> {
    let mut t = TiffWriter::new(b"MM", 42);
    let note = [&b"MLT0"[..], &filler(64, 10), &preview(640, 480)?, &filler(32, 11)].concat();
    let note_at = t.blob(&note);
    let mut exif = shot((1, 500), (11, 1), 800, "2026:11:05 06:30:00");
    exif.push((0x927C, Val::At(note_at, note.len() as u32)));
    let exif = t.ifd(exif, 0);
    let ifd0 = t.ifd(camera("KONICA MINOLTA", "DYNAX 7D", 8, exif), 0);
    let ttw = t.finish(ifd0);
    let blocks = [
        mrw_block(b"\0PRD", &filler(24, 12)),
        mrw_block(b"\0TTW", &ttw),
        mrw_block(b"\0WBG", &filler(12, 13)),
    ]
    .concat();
    let mut out = b"\0MRM".to_vec();
    out.extend_from_slice(&(blocks.len() as u32).to_be_bytes());
    out.extend_from_slice(&blocks);
    out.extend_from_slice(&filler(4096, 14));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyview_core::decode::decode_file;
    use anyview_core::raw::{exif_tiff, find_preview};
    use image::DynamicImage;
    use image::metadata::Orientation;

    struct Expected {
        name: &'static str,
        /// 选中的预览图的尺寸（存储方向）。
        preview: (u32, u32),
        orientation: u16,
        /// 按方向摆正后的显示尺寸。
        shown: (u32, u32),
        model: Option<&'static str>,
        iso: Option<u32>,
    }

    const EXPECTED: [Expected; 9] = [
        Expected {
            name: "a_tiff.nef",
            preview: (640, 480),
            orientation: 1,
            shown: (640, 480),
            model: Some("NIKON D850"),
            iso: Some(200),
        },
        Expected {
            name: "b_canon.cr2",
            preview: (800, 600),
            orientation: 1,
            shown: (800, 600),
            model: Some("Canon EOS 5D Mark IV"),
            iso: Some(100),
        },
        Expected {
            name: "c_olympus.orf",
            preview: (1024, 768),
            orientation: 1,
            shown: (1024, 768),
            model: Some("E-M1MarkIII"),
            iso: Some(250),
        },
        Expected {
            name: "d_panasonic.rw2",
            preview: (1200, 800),
            orientation: 1,
            shown: (1200, 800),
            model: Some("DC-S5M2"),
            iso: Some(320),
        },
        Expected {
            name: "e_fuji.raf",
            preview: (1080, 720),
            orientation: 8,
            shown: (720, 1080),
            model: Some("X-T5"),
            iso: Some(125),
        },
        Expected {
            name: "f_canon.cr3",
            preview: (1800, 1200),
            orientation: 3,
            shown: (1800, 1200),
            model: Some("Canon EOS R5"),
            iso: Some(400),
        },
        Expected {
            name: "g_rotated.dng",
            preview: (1024, 768),
            orientation: 6,
            shown: (768, 1024),
            model: Some("iPhone 15 Pro"),
            iso: Some(64),
        },
        Expected {
            name: "h_unknown.x3f",
            preview: (1100, 700),
            orientation: 1,
            shown: (1100, 700),
            model: None,
            iso: None,
        },
        Expected {
            name: "i_minolta.mrw",
            preview: (640, 480),
            orientation: 8,
            shown: (480, 640),
            model: Some("DYNAX 7D"),
            iso: Some(800),
        },
    ];

    #[test]
    fn previews_are_found_and_shown_upright() {
        let dir =
            std::env::temp_dir().join(format!("anyview-fixtures-raw-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(generate(&dir).unwrap(), EXPECTED.len());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), EXPECTED.len());
        for e in &EXPECTED {
            let path = dir.join(e.name);
            let data = fs::read(&path).unwrap();
            let found = find_preview(&data).unwrap_or_else(|| panic!("{}：没找到预览", e.name));
            assert_eq!(
                (found.width, found.height, found.orientation),
                (e.preview.0, e.preview.1, e.orientation),
                "{}",
                e.name
            );
            let jpeg = &data[found.offset..found.offset + found.len];
            assert!(jpeg.starts_with(&[0xFF, 0xD8]), "{}", e.name);
            let decoded = decode_file(&path).unwrap_or_else(|err| panic!("{}：{err}", e.name));
            assert_eq!((decoded.width, decoded.height), e.shown, "{}", e.name);
            let extension = e.name.rsplit('.').next().unwrap().to_ascii_uppercase();
            assert_eq!(decoded.info.format, format!("{extension}/JPEG"), "{}", e.name);
            // 四个色块的中心与按 EXIF 方向摆正的原图一致。
            let mut upright = DynamicImage::ImageRgb8(quadrants(e.preview.0, e.preview.1));
            upright.apply_orientation(Orientation::from_exif(e.orientation as u8).unwrap());
            let upright = upright.to_rgb8();
            let (w, h) = e.shown;
            for (x, y) in [(w / 4, h / 4), (3 * w / 4, h / 4), (w / 4, 3 * h / 4), (3 * w / 4, 3 * h / 4)] {
                let want = upright.get_pixel(x, y).0;
                let at = y as usize * decoded.stride + x as usize * 4;
                let px = &decoded.pixels[at..at + 4];
                let got = [px[2], px[1], px[0]];
                assert!(
                    got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 10),
                    "{} ({x},{y})：{got:?}，应为 {want:?}",
                    e.name
                );
            }
            let shooting = decoded.info.shooting.as_ref();
            assert_eq!(shooting.and_then(|s| s.model.as_deref()), e.model, "{}", e.name);
            assert_eq!(shooting.and_then(|s| s.iso), e.iso, "{}", e.name);
        }
        // CR3 的拍摄信息是 CMT1（相机）和 CMT2（拍摄参数）合成的；RAF 交给 JPEG 自己的 EXIF。
        let cr3 = fs::read(dir.join("f_canon.cr3")).unwrap();
        let merged = exif_tiff(&cr3).unwrap();
        let info = anyview_core::exif::Exif::parse(&merged).unwrap().shooting();
        assert_eq!(info.make.as_deref(), Some("Canon"));
        assert_eq!(info.exposure_time, Some(1.0 / 125.0));
        assert_eq!(info.f_number, Some(4.0));
        assert_eq!(info.taken.as_deref(), Some("2026:11:05 09:30:00"));
        assert!(exif_tiff(&fs::read(dir.join("e_fuji.raf")).unwrap()).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncated_fixtures_never_panic() {
        // 每个文件截在各处（文件头附近逐字节，后面按步长）都不出错。
        for (name, bytes) in [
            ("nef", nef().unwrap()),
            ("cr2", cr2().unwrap()),
            ("orf", orf().unwrap()),
            ("rw2", rw2().unwrap()),
            ("raf", raf().unwrap()),
            ("cr3", cr3().unwrap()),
            ("dng", dng().unwrap()),
            ("mrw", mrw().unwrap()),
        ] {
            let cuts = (0..bytes.len().min(400)).chain((400..bytes.len()).step_by(997));
            for cut in cuts {
                let part = &bytes[..cut];
                if let Some(p) = find_preview(part) {
                    assert!(p.offset + p.len <= cut, "{name} {cut}");
                }
                let _ = exif_tiff(part);
            }
        }
    }
}
