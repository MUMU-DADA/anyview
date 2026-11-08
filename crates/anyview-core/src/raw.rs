//! RAW 的相机内嵌预览"）：不解 RAW 数据本身，
//! 只从文件结构里找出相机写进去的最大一张 JPEG 预览，以及 RAW 自己的 EXIF 方向和拍摄信息。
//!
//! 找预览图的办法：
//! - TIFF 结构（DNG、CR2、NEF、NRW、ARW、SRF、SR2、PEF、ERF、3FR、FFF、DCR、KDC、MOS、IIQ、SRW），以及文件头魔数
//!   不同的 ORF（"IIRO""IIRS""MMOR"）、RW2 和 RWL（"IIU\0"）：从 IFD0 起走目录链、SubIFD（0x14A）和 EXIF 目录（0x8769）。
//!   候选是 JPEGInterchangeFormat/Length（0x201/0x202）、压缩方式为 JPEG（6、7）的单条带（0x111/0x117），
//!   以及 RW2 的 JpgFromRaw（0x2E）。方向取 IFD0 的 0x112。
//! - RAF：文件头第 84、88 字节（大端）写的 JPEG 位置和长度。
//! - CR3（ISO BMFF，ftyp 为 "crx "）：佳能 uuid 盒里的 THMB、预览 uuid 盒里的 PRVW、第一条轨道（全尺寸 JPEG，
//!   位置从 stsz 和 co64/stco 读）。方向在 CMT1里，拍摄参数在 CMT2（EXIF 目录）里。
//! - MRW：TTW 块里的 TIFF（方向、拍摄信息）；预览图在厂商注释里，靠下面的整文件查找。
//! - 其他（CRW、X3F 等），或者按结构只找到小图（长边不到 [`SCAN_BELOW`]）时：在整个文件里找 JPEG
//!   （从 SOI 读各段到 SOS，再在熵编码数据里找 EOI，跳过 FF00 填充和 RSTn），与结构里找到的一起比。
//!
//! 只认 8 位的基线、扩展顺序和渐进式 JPEG（SOF0/1/2）：RAW 数据本身常用的无损 JPEG，以及分层、
//! 算术编码的都不算。"最大"指像素最多。RAW 结构里没有写方向时（如 RAF），用选中那张 JPEG 自己 EXIF 里的方向。
//! 坏文件不会出错或卡住：读取都检查边界；目录、盒子、段的个数有上限；整文件查找有工作量上限。

use std::borrow::Cow;

/// RAW 的扩展名（小写，不带点）。
pub const EXTENSIONS: &[&str] = &[
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "orf", "rw2", "pef", "sr2", "arw", "srf", "raf", "3fr",
    "fff", "iiq", "rwl", "srw", "x3f", "erf", "mrw", "mos", "kdc", "dcr",
];

/// 扩展名（小写，不带点）是不是 RAW。
pub fn is_raw_extension(extension: &str) -> bool {
    EXTENSIONS.contains(&extension)
}

/// 找到的预览图：在文件里的位置、尺寸，以及 RAW 的 EXIF 方向（1–8，没有时为 1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preview {
    pub offset: usize,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub orientation: u16,
}

/// 按文件结构找到的预览图长边不到这个值时，再在整个文件里找一遍
/// （大的预览可能放在厂商注释里，如 ORF、PEF、MRW）。
pub const SCAN_BELOW: u32 = 1000;

/// 最多读多少个 TIFF 目录、每个目录最多读多少项、一项最多读多少个数值。
const MAX_IFDS: usize = 64;
const MAX_ENTRIES: usize = 1024;
const MAX_VALUES: usize = 64;
/// CR3 最多读多少个盒子。
const MAX_BOXES: usize = 4096;
/// 整个文件里找 JPEG 时最多收多少张。
const MAX_SCANNED: usize = 64;
/// 只读 JPEG 文件头（找 SOF）时最多读多少段。
const HEADER_SEGMENTS: usize = 4096;
/// 给 ORF、RW2 复制 TIFF 开头时，只带上不超过这么大的值（大的是预览图、厂商注释，读拍摄信息用不到）。
const SMALL_VALUE: usize = 64 * 1024;
/// TIFF 头的魔数：标准的 42，ORF 的 "RO" "RS"，RW2、RWL 的 0x55。
const TIFF_MAGICS: [u16; 4] = [42, 0x4F52, 0x5352, 0x55];
/// CR3：moov 里佳能元数据的 uuid 盒（CMT1–CMT4、THMB）。
const CANON_UUID: [u8; 16] = [
    0x85, 0xC0, 0xB6, 0x87, 0x82, 0x0F, 0x11, 0xE0, 0x81, 0x11, 0xF4, 0xCE, 0x46, 0x2B, 0x6A, 0x48,
];
/// CR3：顶层装 PRVW 预览的 uuid 盒。
const PREVIEW_UUID: [u8; 16] = [
    0xEA, 0xF4, 0x2B, 0x5E, 0x1C, 0x98, 0x4B, 0x88, 0xB9, 0xFB, 0xB7, 0xDC, 0x40, 0x6E, 0x4D, 0x16,
];

/// 文件里最大的一张能显示的 JPEG 预览（基线或渐进式；RAW 数据本身用的无损 JPEG 不算）。
pub fn find_preview(data: &[u8]) -> Option<Preview> {
    let Found {
        mut candidates,
        orientation,
    } = structure(data);
    let small = candidates
        .iter()
        .map(Candidate::long_side)
        .max()
        .is_none_or(|side| side < SCAN_BELOW);
    if small {
        scan(data, &mut candidates);
    }
    let best = largest(&candidates)?;
    let orientation = orientation
        .or_else(|| jpeg_orientation(data.get(best.offset..best.offset + best.len)?))
        .unwrap_or(1);
    Some(Preview {
        offset: best.offset,
        len: best.len,
        width: best.width,
        height: best.height,
        orientation,
    })
}

/// 给 [`crate::exif::Exif::parse`] 读拍摄信息用的 TIFF 数据（标准的 TIFF 文件头，IFD0 里有厂商、型号和 EXIF 目录）。
/// 文件本身就是标准 TIFF 时直接借用；文件头魔数不同（ORF、RW2）时复制开头一段并改成标准魔数；找不到时为空。
pub fn exif_tiff(data: &[u8]) -> Option<Cow<'_, [u8]>> {
    match container(data)? {
        Container::Tiff(tiff) if tiff.magic == 42 => Some(Cow::Borrowed(data)),
        Container::Tiff(tiff) => Some(Cow::Owned(standard_copy(&tiff))),
        Container::Mrw(tiff, end) => data.get(tiff.base..end).map(Cow::Borrowed),
        Container::Cr3 => cr3_exif(data),
        Container::Raf => None,
    }
}

// ---------------------------------------------------------------------------------------------
// 候选和文件结构

/// 一张候选的 JPEG：在文件里的位置和它自己 SOF 里写的尺寸。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Candidate {
    offset: usize,
    len: usize,
    width: u32,
    height: u32,
}

impl Candidate {
    fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    fn long_side(&self) -> u32 {
        self.width.max(self.height)
    }
}

/// 像素最多的一张；一样多时取先找到的（按结构找到的排在前面）。
fn largest(candidates: &[Candidate]) -> Option<Candidate> {
    let mut best: Option<Candidate> = None;
    for c in candidates {
        if best.is_none_or(|b| c.pixels() > b.pixels()) {
            best = Some(*c);
        }
    }
    best
}

/// 按文件结构找到的候选和方向。
#[derive(Default)]
struct Found {
    candidates: Vec<Candidate>,
    orientation: Option<u16>,
}

enum Container<'a> {
    /// TIFF 结构（含 ORF、RW2 的魔数），从文件开头起。
    Tiff(Tiff<'a>),
    /// MRW：TTW 块里的 TIFF 和这个块的结尾。
    Mrw(Tiff<'a>, usize),
    Raf,
    Cr3,
}

fn container(data: &[u8]) -> Option<Container<'_>> {
    if let Some(tiff) = Tiff::new(data, 0) {
        Some(Container::Tiff(tiff))
    } else if data.starts_with(b"FUJIFILMCCD-RAW") {
        Some(Container::Raf)
    } else if data.get(4..8) == Some(&b"ftyp"[..]) && data.get(8..12) == Some(&b"crx "[..]) {
        Some(Container::Cr3)
    } else if data.starts_with(b"\0MRM") {
        mrw(data).map(|(tiff, end)| Container::Mrw(tiff, end))
    } else {
        None
    }
}

fn structure(data: &[u8]) -> Found {
    let mut found = Found::default();
    match container(data) {
        Some(Container::Tiff(tiff) | Container::Mrw(tiff, _)) => tiff_previews(&tiff, &mut found),
        Some(Container::Raf) => {
            if let (Some(offset), Some(len)) = (be_u32(data, 84), be_u32(data, 88)) {
                push(data, &mut found.candidates, offset as usize, len as usize);
            }
        }
        Some(Container::Cr3) => cr3_previews(data, &mut found),
        None => {}
    }
    found
}

/// 记下一张候选：`data[offset..offset + len]` 是 8 位的基线或渐进式 JPEG 时才算。
fn push(data: &[u8], out: &mut Vec<Candidate>, offset: usize, len: usize) {
    let Some(jpeg) = offset.checked_add(len).and_then(|end| data.get(offset..end)) else {
        return;
    };
    let mut budget = HEADER_SEGMENTS;
    if let Some((width, height, _)) = walk_jpeg(jpeg, 0, false, &mut budget)
        && plausible(width, height, len)
    {
        out.push(Candidate {
            offset,
            len,
            width,
            height,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// TIFF

/// `data[base..]` 开头的一个 TIFF 结构。偏移量都相对于 `base`。
#[derive(Clone, Copy)]
struct Tiff<'a> {
    data: &'a [u8],
    base: usize,
    little: bool,
    magic: u16,
    ifd0: usize,
}

/// 目录里的一项。
#[derive(Clone, Copy, Debug)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    /// 这一项在目录里的位置。
    pos: usize,
    /// 值的位置（4 字节以内的值就在项里）。
    at: usize,
    /// 值的总字节数。
    size: usize,
}

impl<'a> Tiff<'a> {
    fn new(data: &'a [u8], base: usize) -> Option<Self> {
        let head = data.get(base..base.checked_add(8)?)?;
        let little = match &head[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let mut tiff = Self {
            data,
            base,
            little,
            magic: 0,
            ifd0: 0,
        };
        tiff.magic = tiff.u16(2)?;
        if !TIFF_MAGICS.contains(&tiff.magic) {
            return None;
        }
        tiff.ifd0 = tiff.u32(4)? as usize;
        Some(tiff)
    }

    fn bytes(&self, at: usize, len: usize) -> Option<&'a [u8]> {
        let start = self.base.checked_add(at)?;
        self.data.get(start..start.checked_add(len)?)
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let b = self.bytes(at, 2)?;
        Some(match self.little {
            true => u16::from_le_bytes([b[0], b[1]]),
            false => u16::from_be_bytes([b[0], b[1]]),
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.bytes(at, 4)?;
        let bytes = [b[0], b[1], b[2], b[3]];
        Some(match self.little {
            true => u32::from_le_bytes(bytes),
            false => u32::from_be_bytes(bytes),
        })
    }

    /// 一个目录里的各项；值越界、类型不认识的项跳过。
    fn entries(&self, ifd: usize) -> Vec<Entry> {
        let Some(count) = self.u16(ifd) else {
            return Vec::new();
        };
        (0..usize::from(count).min(MAX_ENTRIES))
            .filter_map(|i| {
                let pos = ifd.checked_add(2 + 12 * i)?;
                let tag = self.u16(pos)?;
                let kind = self.u16(pos + 2)?;
                let count = self.u32(pos + 4)?;
                let unit: u64 = match kind {
                    1 | 2 | 6 | 7 => 1,
                    3 | 8 => 2,
                    4 | 9 | 11 | 13 => 4,
                    5 | 10 | 12 => 8,
                    _ => return None,
                };
                let size = usize::try_from(unit * u64::from(count)).ok()?;
                let at = match size <= 4 {
                    true => pos + 8,
                    false => self.u32(pos + 8)? as usize,
                };
                self.bytes(at, size)?;
                Some(Entry {
                    tag,
                    kind,
                    count,
                    pos,
                    at,
                    size,
                })
            })
            .collect()
    }

    /// SHORT、LONG、IFD 类型的各个数值（最多 [`MAX_VALUES`] 个）。
    fn values(&self, e: &Entry) -> Vec<usize> {
        let unit = match e.kind {
            3 => 2,
            4 | 13 => 4,
            _ => return Vec::new(),
        };
        (0..(e.count as usize).min(MAX_VALUES))
            .filter_map(|i| match unit {
                2 => self.u16(e.at + 2 * i).map(usize::from),
                _ => self.u32(e.at + 4 * i).map(|v| v as usize),
            })
            .collect()
    }

    fn value(&self, e: &Entry) -> Option<usize> {
        self.values(e).first().copied()
    }

    /// 目录末尾指向的下一个目录。
    fn next_ifd(&self, ifd: usize) -> Option<usize> {
        let count = usize::from(self.u16(ifd)?);
        let next = self.u32(ifd + 2 + 12 * count)? as usize;
        (next >= 8).then_some(next)
    }

    /// IFD0 里的方向（0x112）。
    fn orientation(&self) -> Option<u16> {
        let entry = self
            .entries(self.ifd0)
            .into_iter()
            .find(|e| e.tag == 0x112)?;
        valid_orientation(self.value(&entry)?)
    }
}

fn valid_orientation(value: usize) -> Option<u16> {
    (1..=8).contains(&value).then_some(value as u16)
}

/// TIFF 结构里的预览：IFD0 起的目录链、SubIFD、EXIF 目录。
fn tiff_previews(tiff: &Tiff, found: &mut Found) {
    found.orientation = tiff.orientation();
    let mut pending = vec![tiff.ifd0];
    let mut seen: Vec<usize> = Vec::new();
    while let Some(ifd) = pending.pop() {
        if ifd < 8 || seen.contains(&ifd) || seen.len() >= MAX_IFDS {
            continue;
        }
        seen.push(ifd);
        let entries = tiff.entries(ifd);
        let find = |tag: u16| entries.iter().find(|e| e.tag == tag);
        let number = |tag: u16| find(tag).and_then(|e| tiff.value(e));
        let mut add = |offset: usize, len: usize| {
            if let Some(offset) = tiff.base.checked_add(offset) {
                push(tiff.data, &mut found.candidates, offset, len);
            }
        };
        // JPEGInterchangeFormat / Length：缩略图和多数相机的预览图。
        if let (Some(offset), Some(len)) = (number(0x201), number(0x202)) {
            add(offset, len);
        }
        // 压缩方式为 JPEG 的单条带：CR2 的 IFD0（全尺寸 JPEG）、DNG 的预览 SubIFD。
        if matches!(number(0x103), Some(6 | 7))
            && let (Some(strips), Some(counts)) = (find(0x111), find(0x117))
            && strips.count == 1
            && counts.count == 1
            && let (Some(offset), Some(len)) = (tiff.value(strips), tiff.value(counts))
        {
            add(offset, len);
        }
        // RW2、RWL 的 JpgFromRaw：值本身就是 JPEG。
        if let Some(e) = find(0x2E)
            && e.kind == 7
        {
            add(e.at, e.size);
        }
        for tag in [0x14A, 0x8769] {
            if let Some(e) = find(tag) {
                pending.extend(tiff.values(e));
            }
        }
        if let Some(next) = tiff.next_ifd(ifd) {
            pending.push(next);
        }
    }
}

/// ORF、RW2：复制读拍摄信息要用的开头一段（IFD0、EXIF 目录和它们不大的值），魔数改成标准的 42。
fn standard_copy(tiff: &Tiff) -> Vec<u8> {
    let mut end = 8;
    let mut cover = |ifd: usize| {
        if let Some(count) = tiff.u16(ifd) {
            end = end.max(ifd + 2 + 12 * usize::from(count) + 4);
        }
        for e in tiff.entries(ifd) {
            if e.size <= SMALL_VALUE {
                end = end.max(e.at + e.size);
            }
        }
    };
    cover(tiff.ifd0);
    if let Some(exif) = tiff
        .entries(tiff.ifd0)
        .iter()
        .find(|e| e.tag == 0x8769)
        .and_then(|e| tiff.value(e))
    {
        cover(exif);
    }
    let available = tiff.data.len().saturating_sub(tiff.base);
    let mut out = tiff.bytes(0, end.min(available)).unwrap_or_default().to_vec();
    if out.len() >= 4 {
        out[2..4].copy_from_slice(&if tiff.little { [42, 0] } else { [0, 42] });
    }
    out
}

/// MRW：文件头（"\0MRM" + 长度）里的各块，TTW 块是 TIFF。返回 TIFF 和这个块的结尾。
fn mrw(data: &[u8]) -> Option<(Tiff<'_>, usize)> {
    let header_end = 8usize
        .checked_add(be_u32(data, 4)? as usize)?
        .min(data.len());
    let mut at = 8;
    for _ in 0..64 {
        let name = data.get(at..at + 4)?;
        let body = at + 8;
        let end = body.checked_add(be_u32(data, at + 4)? as usize)?;
        if name == b"\0TTW" {
            return Tiff::new(data, body).map(|tiff| (tiff, end.min(data.len())));
        }
        if end >= header_end {
            return None;
        }
        at = end;
    }
    None
}

// ---------------------------------------------------------------------------------------------
// CR3（ISO BMFF）

#[derive(Clone, Copy, Debug)]
struct BmffBox {
    kind: [u8; 4],
    body: usize,
    end: usize,
}

/// `from..to` 这一层的盒子。`budget` 是还能读的盒子数。
fn boxes(data: &[u8], from: usize, to: usize, budget: &mut usize) -> Vec<BmffBox> {
    let to = to.min(data.len());
    let mut out = Vec::new();
    let mut at = from;
    while at.checked_add(8).is_some_and(|header| header <= to) && *budget > 0 {
        *budget -= 1;
        let Some(size) = be_u32(data, at) else {
            break;
        };
        let Some(kind) = data
            .get(at + 4..at + 8)
            .and_then(|k| <[u8; 4]>::try_from(k).ok())
        else {
            break;
        };
        let (body, end) = match size {
            0 => (at + 8, to),
            1 => {
                let Some(large) = be_u64(data, at + 8).and_then(|v| usize::try_from(v).ok()) else {
                    break;
                };
                (at + 16, at.saturating_add(large))
            }
            n => (at + 8, at.saturating_add(n as usize)),
        };
        if end > to || body > end {
            break;
        }
        out.push(BmffBox { kind, body, end });
        at = end;
    }
    out
}

fn uuid_is(data: &[u8], b: &BmffBox, uuid: &[u8; 16]) -> bool {
    b.body
        .checked_add(16)
        .and_then(|end| data.get(b.body..end))
        == Some(&uuid[..])
}

/// moov 里佳能 uuid 盒的各个子盒（CNCV、CMT1–CMT4、THMB）和 moov 里的第一条轨道。
fn cr3_moov(data: &[u8], budget: &mut usize) -> (Vec<BmffBox>, Option<BmffBox>) {
    let mut canon = Vec::new();
    let mut track = None;
    for top in boxes(data, 0, data.len(), budget) {
        if &top.kind != b"moov" {
            continue;
        }
        for b in boxes(data, top.body, top.end, budget) {
            match &b.kind {
                b"uuid" if canon.is_empty() && uuid_is(data, &b, &CANON_UUID) => {
                    canon = boxes(data, b.body + 16, b.end, budget);
                }
                b"trak" if track.is_none() => track = Some(b),
                _ => {}
            }
        }
        break;
    }
    (canon, track)
}

fn cr3_previews(data: &[u8], found: &mut Found) {
    let mut budget = MAX_BOXES;
    let (canon, track) = cr3_moov(data, &mut budget);
    for b in &canon {
        match &b.kind {
            b"CMT1" => found.orientation = Tiff::new(data, b.body).and_then(|t| t.orientation()),
            b"THMB" => push_embedded(data, &mut found.candidates, b),
            _ => {}
        }
    }
    // 第一条轨道：全尺寸 JPEG。
    if let Some(track) = track
        && let Some((offset, len)) = first_sample(data, &track, &mut budget)
    {
        push(data, &mut found.candidates, offset, len);
    }
    // 预览 uuid 盒：UUID 之后 8 个字节，再是 PRVW 盒。
    for top in boxes(data, 0, data.len(), &mut budget) {
        if &top.kind == b"uuid" && uuid_is(data, &top, &PREVIEW_UUID) {
            for b in boxes(data, top.body + 24, top.end, &mut budget) {
                if &b.kind == b"PRVW" {
                    push_embedded(data, &mut found.candidates, &b);
                }
            }
        }
    }
}

/// THMB、PRVW：盒子开头几个字段（尺寸、长度）之后就是 JPEG，一直到盒子结尾。
fn push_embedded(data: &[u8], out: &mut Vec<Candidate>, b: &BmffBox) {
    let Some(head) = data.get(b.body..b.end.min(b.body + 64)) else {
        return;
    };
    if let Some(at) = head.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]) {
        let offset = b.body + at;
        push(data, out, offset, b.end - offset);
    }
}

/// 轨道的第一个样本：stsz 里的长度、co64 或 stco 里的位置。
fn first_sample(data: &[u8], track: &BmffBox, budget: &mut usize) -> Option<(usize, usize)> {
    let mut current = *track;
    for kind in [b"mdia", b"minf", b"stbl"] {
        current = boxes(data, current.body, current.end, budget)
            .into_iter()
            .find(|b| &b.kind == kind)?;
    }
    let mut size = None;
    let mut offset = None;
    for b in boxes(data, current.body, current.end, budget) {
        match &b.kind {
            // 版本和标志、统一的样本长度（0 表示各不相同）、样本数，然后是各样本的长度。
            b"stsz" => {
                let uniform = be_u32(data, b.body + 4)?;
                size = match uniform {
                    0 if be_u32(data, b.body + 8)? > 0 => be_u32(data, b.body + 12),
                    0 => None,
                    n => Some(n),
                };
            }
            // 版本和标志、个数，然后是各块的位置。
            b"co64" if be_u32(data, b.body + 4)? > 0 => {
                offset = be_u64(data, b.body + 8).and_then(|v| usize::try_from(v).ok());
            }
            b"stco" if be_u32(data, b.body + 4)? > 0 => {
                offset = be_u32(data, b.body + 8).map(|v| v as usize);
            }
            _ => {}
        }
    }
    Some((offset?, size? as usize))
}

/// CR3 的拍摄信息：CMT1（IFD0：厂商、型号）加上 CMT2（EXIF 目录），合成一份 TIFF；只有 CMT1 时只有相机名。
fn cr3_exif(data: &[u8]) -> Option<Cow<'_, [u8]>> {
    let mut budget = MAX_BOXES;
    let (canon, _) = cr3_moov(data, &mut budget);
    let body = |kind: &[u8; 4]| {
        canon
            .iter()
            .find(|b| &b.kind == kind)
            .and_then(|b| data.get(b.body..b.end))
    };
    let one = body(b"CMT1")?;
    let first = Tiff::new(one, 0).filter(|t| t.magic == 42)?;
    let second = body(b"CMT2")
        .and_then(|two| Tiff::new(two, 0))
        .filter(|t| t.magic == 42 && t.little == first.little);
    Some(match second.and_then(|second| merge_exif(&first, &second)) {
        Some(merged) => Cow::Owned(merged),
        None => Cow::Borrowed(one),
    })
}

/// CMT1 原样放在前面；CMT2 接在后面，把它 IFD0 里放在别处的值的偏移挪过去；
/// 最后写一个新的 IFD0：CMT1 IFD0 的各项，加上指向 CMT2 目录的 0x8769。
fn merge_exif(first: &Tiff, second: &Tiff) -> Option<Vec<u8>> {
    let little = first.little;
    let put = |out: &mut Vec<u8>, at: usize, value: u32| {
        let bytes = if little {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        };
        out[at..at + 4].copy_from_slice(&bytes);
    };
    let mut out = first.data.to_vec();
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let shift = out.len();
    out.extend_from_slice(second.data);
    for e in second.entries(second.ifd0) {
        if e.size > 4 {
            put(&mut out, shift + e.pos + 8, u32::try_from(e.at + shift).ok()?);
        }
    }
    let exif_ifd = u32::try_from(shift + second.ifd0).ok()?;
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let ifd0 = out.len();
    let entries: Vec<&[u8]> = first
        .entries(first.ifd0)
        .iter()
        .filter(|e| e.tag != 0x8769)
        .filter_map(|e| first.bytes(e.pos, 12))
        .collect();
    let count = u16::try_from(entries.len() + 1).ok()?;
    out.extend_from_slice(&if little {
        count.to_le_bytes()
    } else {
        count.to_be_bytes()
    });
    for entry in entries {
        out.extend_from_slice(entry);
    }
    let pointer = out.len();
    out.extend_from_slice(&[0; 12]);
    let (tag, kind, one) = match little {
        true => (0x8769u16.to_le_bytes(), 4u16.to_le_bytes(), 1u32.to_le_bytes()),
        false => (0x8769u16.to_be_bytes(), 4u16.to_be_bytes(), 1u32.to_be_bytes()),
    };
    out[pointer..pointer + 2].copy_from_slice(&tag);
    out[pointer + 2..pointer + 4].copy_from_slice(&kind);
    out[pointer + 4..pointer + 8].copy_from_slice(&one);
    put(&mut out, pointer + 8, exif_ifd);
    out.extend_from_slice(&[0; 4]); // 没有下一个目录
    put(&mut out, 4, u32::try_from(ifd0).ok()?);
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// JPEG

/// 花掉一些工作量；不够时清零并返回空（整文件查找就此停下）。
fn spend(budget: &mut usize, amount: usize) -> Option<()> {
    match budget.checked_sub(amount) {
        Some(left) => {
            *budget = left;
            Some(())
        }
        None => {
            *budget = 0;
            None
        }
    }
}

/// 从 `start`（SOI）开始读一张 JPEG 的各段，给出 SOF 里的宽、高和总长度。
/// 只认 8 位的 SOF0/1/2；遇到其他 SOF（无损、分层、算术编码）就不要。
/// `to_end` 为假时读到 SOF 就停（长度给 0）；为真时一直读到 EOI，跳过熵编码数据里的 FF00 填充和 RSTn。
/// 每读一段、每扫一个字节都从 `budget` 里扣，用完时放弃。
fn walk_jpeg(
    data: &[u8],
    start: usize,
    to_end: bool,
    budget: &mut usize,
) -> Option<(u32, u32, usize)> {
    if data.get(start..start.checked_add(2)?)? != [0xFF, 0xD8] {
        return None;
    }
    let mut p = start + 2;
    let mut size: Option<(u32, u32)> = None;
    loop {
        spend(budget, 1)?;
        if *data.get(p)? != 0xFF {
            return None;
        }
        // 段前可以有多个 FF 填充。
        let mut q = p + 1;
        while *data.get(q)? == 0xFF {
            spend(budget, 1)?;
            q += 1;
        }
        let code = data[q];
        p = q + 1;
        match code {
            0xD9 => return size.map(|(w, h)| (w, h, p - start)),
            0xD8 | 0x00 => return None,
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*data.get(p)?, *data.get(p + 1)?]));
        if len < 2 {
            return None;
        }
        let body = data.get(p + 2..p + len)?;
        match code {
            0xC0..=0xC2 => {
                if size.is_some() {
                    return None;
                }
                let (w, h) = sof_size(body)?;
                if !to_end {
                    return Some((w, h, 0));
                }
                size = Some((w, h));
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            0xDA => {
                size?;
                p += len;
                // 熵编码数据：到下一个不是 FF00 填充、也不是 RSTn 的标记为止。
                loop {
                    let skip = data.get(p..)?.iter().position(|&b| b == 0xFF)?;
                    spend(budget, skip + 1)?;
                    p += skip;
                    match *data.get(p + 1)? {
                        0x00 | 0xD0..=0xD7 => p += 2,
                        0xFF => p += 1,
                        _ => break,
                    }
                }
                continue;
            }
            _ => {}
        }
        p += len;
    }
}

/// SOF 段的内容：8 位精度，1、3、4 个分量，每个分量的采样因子 1–4，长度与分量数相符。
fn sof_size(body: &[u8]) -> Option<(u32, u32)> {
    let [precision, h1, h0, w1, w0, components, ..] = *body else {
        return None;
    };
    let n = usize::from(components);
    if precision != 8 || !matches!(n, 1 | 3 | 4) || body.len() != 6 + 3 * n {
        return None;
    }
    let sampling_ok = (0..n).all(|i| {
        let s = body[6 + 3 * i + 1];
        (1..=4).contains(&(s >> 4)) && (1..=4).contains(&(s & 0x0F))
    });
    let (width, height) = (
        u32::from(u16::from_be_bytes([w1, w0])),
        u32::from(u16::from_be_bytes([h1, h0])),
    );
    (sampling_ok && width > 0 && height > 0).then_some((width, height))
}

/// 尺寸与数据量大致相符：每 2000 个像素至少 1 字节（防止随机数据凑出一个巨大的"预览图"）。
fn plausible(width: u32, height: u32, len: usize) -> bool {
    (len as u64).saturating_mul(2000) >= u64::from(width) * u64::from(height)
}

/// 在整个文件里找 JPEG（SOI 开头、EOI 结尾），加到 `out` 里。总工作量约为文件大小的两倍，坏文件也很快结束。
fn scan(data: &[u8], out: &mut Vec<Candidate>) {
    let mut budget = data.len().saturating_mul(2).saturating_add(1 << 20);
    let mut from = 0;
    let mut kept = 0;
    while kept < MAX_SCANNED && budget > 0 {
        let Some(start) = find_soi(data, from) else {
            break;
        };
        match walk_jpeg(data, start, true, &mut budget) {
            Some((width, height, len)) if plausible(width, height, len) => {
                out.push(Candidate {
                    offset: start,
                    len,
                    width,
                    height,
                });
                kept += 1;
                // 跳过这一张（它 EXIF 里的缩略图更小，不用再找）。
                from = start + len;
            }
            _ => from = start + 1,
        }
    }
}

/// 从 `from` 起下一个 FF D8 FF（SOI 后面紧跟一个标记）的位置。
fn find_soi(data: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    loop {
        at += data.get(at..)?.iter().position(|&b| b == 0xFF)?;
        match data.get(at + 1..at + 3) {
            Some([0xD8, 0xFF]) => return Some(at),
            Some(_) => at += 1,
            None => return None,
        }
    }
}

/// JPEG 自己 EXIF里的方向。
fn jpeg_orientation(jpeg: &[u8]) -> Option<u16> {
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut p = 2;
    for _ in 0..64 {
        if *jpeg.get(p)? != 0xFF {
            return None;
        }
        let code = *jpeg.get(p + 1)?;
        if code == 0xFF {
            p += 1;
            continue;
        }
        if matches!(code, 0xD9 | 0xDA) {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([*jpeg.get(p + 2)?, *jpeg.get(p + 3)?]));
        if code == 0xE1
            && let Some(tiff) = jpeg
                .get(p + 4..p + 2 + len)
                .and_then(|body| body.strip_prefix(b"Exif\0\0"))
        {
            return Tiff::new(tiff, 0)?.orientation();
        }
        p += 2 + len;
    }
    None
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be_u64(data: &[u8], at: usize) -> Option<u64> {
    let b = data.get(at..at.checked_add(8)?)?;
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(b);
    Some(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jpeg::tests::encode_jpeg;

    /// 不含 0xFF 的填充字节（不会凑出 JPEG 标记）。
    fn filler(len: usize, seed: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 7 + seed) % 251) as u8).collect()
    }

    /// RAW 数据常用的无损 JPEG：文件结构完整，但不能当预览。
    fn lossless_jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x14, 0x00, 1];
        j.extend_from_slice(&[0; 15]);
        j.push(0);
        j.extend_from_slice(&[0xFF, 0xC3, 0x00, 0x0B, 14]);
        j.extend_from_slice(&height.to_be_bytes());
        j.extend_from_slice(&width.to_be_bytes());
        j.extend_from_slice(&[1, 1, 0x11, 0]);
        j.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 1, 1, 0x00, 1, 0, 0]);
        j.extend_from_slice(&filler(4000, 3));
        j.extend_from_slice(&[0xFF, 0xD9]);
        j
    }

    /// 小端 TIFF：IFD0（方向、0x201/0x202 → `thumb`）→ IFD1（JPEG 压缩的单条带 → `strip`）。
    fn tiff_with(magic: &[u8; 4], orientation: u16, thumb: &[u8], strip: &[u8]) -> Vec<u8> {
        let mut t = magic.to_vec();
        t.extend_from_slice(&[0; 4]);
        let thumb_at = t.len() as u32;
        t.extend_from_slice(thumb);
        let strip_at = t.len() as u32;
        t.extend_from_slice(strip);
        if t.len() % 2 == 1 {
            t.push(0);
        }
        let entry = |t: &mut Vec<u8>, tag: u16, kind: u16, value: u32| {
            t.extend_from_slice(&tag.to_le_bytes());
            t.extend_from_slice(&kind.to_le_bytes());
            t.extend_from_slice(&1u32.to_le_bytes());
            t.extend_from_slice(&value.to_le_bytes());
        };
        let ifd1 = t.len() as u32;
        t.extend_from_slice(&3u16.to_le_bytes());
        entry(&mut t, 0x103, 3, 7);
        entry(&mut t, 0x111, 4, strip_at);
        entry(&mut t, 0x117, 4, strip.len() as u32);
        t.extend_from_slice(&0u32.to_le_bytes());
        let ifd0 = t.len() as u32;
        t.extend_from_slice(&3u16.to_le_bytes());
        entry(&mut t, 0x112, 3, u32::from(orientation));
        entry(&mut t, 0x201, 4, thumb_at);
        entry(&mut t, 0x202, 4, thumb.len() as u32);
        t.extend_from_slice(&ifd1.to_le_bytes());
        t[4..8].copy_from_slice(&ifd0.to_le_bytes());
        t
    }

    #[test]
    fn walks_segments_stuffing_and_restart_markers() {
        // SOI、SOF0（16x8，1 个分量）、SOS、带 FF00 填充、RST0 和 FF 填充的熵编码数据、EOI。
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 8, 0, 8, 0, 16, 1, 1, 0x11, 0];
        j.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 1, 1, 0x00, 0, 63, 0]);
        j.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, 0xFF, 0xFF, 0xD9]);
        let mut budget = 1000;
        assert_eq!(walk_jpeg(&j, 0, true, &mut budget), Some((16, 8, j.len())));
        let mut budget = 1000;
        assert_eq!(walk_jpeg(&j, 0, false, &mut budget), Some((16, 8, 0)));
        // 截断：找不到 EOI 就不算。
        let mut budget = 1000;
        assert_eq!(walk_jpeg(&j[..j.len() - 1], 0, true, &mut budget), None);
        // 无损 JPEG不算。
        let mut budget = 1000;
        assert_eq!(walk_jpeg(&lossless_jpeg(64, 48), 0, true, &mut budget), None);
    }

    #[test]
    fn scanning_finds_the_largest_jpeg_and_skips_lossless_data() {
        let small = encode_jpeg(64, 48, 80);
        let large = encode_jpeg(96, 64, 80);
        let mut data = filler(300, 1);
        data.extend_from_slice(&lossless_jpeg(3000, 2000));
        data.extend_from_slice(&filler(200, 2));
        let small_at = data.len();
        data.extend_from_slice(&small);
        data.extend_from_slice(&filler(100, 3));
        let large_at = data.len();
        data.extend_from_slice(&large);
        data.extend_from_slice(&filler(500, 4));
        let preview = find_preview(&data).unwrap();
        assert_eq!(
            preview,
            Preview {
                offset: large_at,
                len: large.len(),
                width: 96,
                height: 64,
                orientation: 1,
            }
        );
        let mut found = Vec::new();
        scan(&data, &mut found);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].offset, found[0].len), (small_at, small.len()));
        assert!(exif_tiff(&data).is_none());
    }

    #[test]
    fn tiff_structure_wins_when_large_enough() {
        // 结构里的预览长边已够 SCAN_BELOW：不再整文件查找，所以没登记的更大 JPEG 不会被选中。
        let thumb = encode_jpeg(32, 24, 80);
        let strip = encode_jpeg(1024, 16, 80);
        let mut data = tiff_with(b"II*\0", 6, &thumb, &strip);
        data.extend_from_slice(&encode_jpeg(2000, 16, 80));
        let preview = find_preview(&data).unwrap();
        assert_eq!((preview.width, preview.height, preview.orientation), (1024, 16, 6));
        assert!(data[preview.offset..].starts_with(&strip));
        assert_eq!(preview.len, strip.len());
        assert!(matches!(exif_tiff(&data), Some(Cow::Borrowed(_))));
        // 结构里只有小图时整文件查找，没登记的大图也能找到。
        let small_strip = encode_jpeg(200, 16, 80);
        let mut data = tiff_with(b"II*\0", 1, &thumb, &small_strip);
        data.extend_from_slice(&encode_jpeg(2000, 16, 80));
        let preview = find_preview(&data).unwrap();
        assert_eq!((preview.width, preview.height, preview.orientation), (2000, 16, 1));
    }

    #[test]
    fn orf_and_rw2_headers_count_as_tiff_and_are_patched_for_exif() {
        let thumb = encode_jpeg(32, 24, 80);
        let strip = encode_jpeg(1200, 16, 80);
        for magic in [b"IIRO", b"IIRS", b"IIU\0"] {
            let data = tiff_with(magic, 8, &thumb, &strip);
            let preview = find_preview(&data).unwrap();
            assert_eq!((preview.width, preview.orientation), (1200, 8), "{magic:?}");
            let copy = exif_tiff(&data).unwrap();
            assert!(matches!(copy, Cow::Owned(_)));
            assert!(copy.starts_with(b"II*\0"));
            assert!(crate::exif::Exif::parse(&copy).is_some());
        }
    }

    #[test]
    fn preview_orientation_falls_back_to_the_jpegs_own_exif() {
        let jpeg = crate::jpeg::insert_exif_thumbnail(
            &encode_jpeg(120, 90, 80),
            &encode_jpeg(16, 12, 80),
            6,
            true,
        );
        let mut data = filler(64, 5);
        data.extend_from_slice(&jpeg);
        let preview = find_preview(&data).unwrap();
        assert_eq!((preview.width, preview.height, preview.orientation), (120, 90, 6));
    }

    /// xorshift：可重复的伪随机字节。
    fn noise(len: usize, mut state: u64) -> Vec<u8> {
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn garbage_and_truncated_files_do_not_panic() {
        let thumb = encode_jpeg(32, 24, 80);
        let strip = encode_jpeg(64, 48, 80);
        let tiff = tiff_with(b"MM\0*", 3, &thumb, &strip);
        for cut in 0..tiff.len() {
            let _ = find_preview(&tiff[..cut]);
            let _ = exif_tiff(&tiff[..cut]);
        }
        let heads: [&[u8]; 8] = [
            b"II*\0",
            b"MM\0*",
            b"IIRO",
            b"IIU\0",
            b"FUJIFILMCCD-RAW 0201",
            b"\0\0\0\x18ftypcrx ",
            b"\0MRM",
            b"\xFF\xD8\xFF\xC0",
        ];
        for (i, head) in heads.iter().enumerate() {
            for len in [0usize, 7, 64, 4096] {
                let mut data = head.to_vec();
                data.extend_from_slice(&noise(len, 0x9E37_79B9 + i as u64 * 31 + len as u64));
                let _ = find_preview(&data);
                let _ = exif_tiff(&data);
            }
        }
        let _ = find_preview(&noise(1 << 20, 7));
    }

    /// 整文件查找的耗时：64 MB 的随机数据，预览图在最后。手动运行：
    /// `cargo test -p anyview-core --release -- --ignored raw_scan_64mb --nocapture`
    #[test]
    #[ignore]
    fn raw_scan_64mb_cost() {
        let mut data = b"IIRO".to_vec();
        data.extend_from_slice(&noise(64 << 20, 11));
        data.extend_from_slice(&encode_jpeg(1600, 1200, 80));
        let start = std::time::Instant::now();
        let preview = find_preview(&data).unwrap();
        println!(
            "整文件查找 64 MB：{:.1} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!((preview.width, preview.height), (1600, 1200));
    }

    #[test]
    fn pathological_segment_chains_stay_linear() {
        // 每 6 字节一个 SOI，各段首尾相连一直到文件末尾：不限工作量时是平方级的。
        let data = [0xFF, 0xD8, 0xFF, 0xFE, 0x00, 0x04].repeat(300_000);
        let start = std::time::Instant::now();
        assert_eq!(find_preview(&data), None);
        assert!(start.elapsed().as_secs() < 20, "{:?}", start.elapsed());
    }
}
