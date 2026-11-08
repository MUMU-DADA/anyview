//! EXIF（TIFF 结构）的读取：内嵌缩略图和拍摄信息。
//! 只读需要的几项；越界、类型不符的项直接跳过，认不出的结构返回空，不会因为坏数据出错。

/// 拍摄信息：相机、镜头、焦距、光圈、快门、ISO、拍摄时间。没有的项为空。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShootingInfo {
    /// 厂商（Make），如 "NIKON CORPORATION"。
    pub make: Option<String>,
    /// 型号（Model），如 "NIKON D300"。
    pub model: Option<String>,
    /// 镜头型号（LensModel）。
    pub lens: Option<String>,
    /// 焦距（毫米）。
    pub focal_length: Option<f64>,
    /// 35 毫米等效焦距（毫米）。
    pub focal_length_35mm: Option<u32>,
    /// 光圈值（f 数）。
    pub f_number: Option<f64>,
    /// 曝光时间（秒）。
    pub exposure_time: Option<f64>,
    pub iso: Option<u32>,
    /// 拍摄时间（DateTimeOriginal，原样，如 "2024:06:22 08:40:39"）。
    pub taken: Option<String>,
}

impl ShootingInfo {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// 没有任何一项时为空。
    pub fn non_empty(self) -> Option<Self> {
        (!self.is_empty()).then_some(self)
    }
}

/// 一段 EXIF 数据（从 TIFF 头 "II*\0" 或 "MM\0*" 开始）。
pub struct Exif<'a> {
    data: &'a [u8],
    little: bool,
    ifd0: usize,
}

/// 目录里的一项：标签、类型、个数，以及值所在的位置（4 字节以内的值就在项里）。
#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    at: usize,
}

const MAX_ENTRIES: usize = 512;

impl<'a> Exif<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        let little = match data.get(..4)? {
            [0x49, 0x49, 42, 0] => true,
            [0x4D, 0x4D, 0, 42] => false,
            _ => return None,
        };
        let mut exif = Self {
            data,
            little,
            ifd0: 0,
        };
        exif.ifd0 = exif.u32_at(4)? as usize;
        exif.u16_at(exif.ifd0)?;
        Some(exif)
    }

    fn u16_at(&self, offset: usize) -> Option<u16> {
        let b = self.data.get(offset..offset.checked_add(2)?)?;
        Some(if self.little {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    fn u32_at(&self, offset: usize) -> Option<u32> {
        let b = self.data.get(offset..offset.checked_add(4)?)?;
        let bytes = [b[0], b[1], b[2], b[3]];
        Some(if self.little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    /// 一个目录里的各项。
    fn entries(&self, ifd: usize) -> Vec<Entry> {
        let Some(count) = self.u16_at(ifd) else {
            return Vec::new();
        };
        (0..usize::from(count).min(MAX_ENTRIES))
            .filter_map(|i| {
                let p = ifd + 2 + 12 * i;
                let tag = self.u16_at(p)?;
                let kind = self.u16_at(p + 2)?;
                let count = self.u32_at(p + 4)?;
                let unit: u64 = match kind {
                    1 | 2 | 6 | 7 => 1,
                    3 | 8 => 2,
                    4 | 9 | 11 => 4,
                    5 | 10 | 12 => 8,
                    _ => return None,
                };
                let size = unit.checked_mul(u64::from(count))?;
                let at = if size <= 4 {
                    p + 8
                } else {
                    self.u32_at(p + 8)? as usize
                };
                // 值必须整个落在数据里。
                self.data.get(at..at.checked_add(size as usize)?)?;
                Some(Entry {
                    tag,
                    kind,
                    count,
                    at,
                })
            })
            .collect()
    }

    fn find(&self, ifd: usize, tag: u16) -> Option<Entry> {
        self.entries(ifd).into_iter().find(|e| e.tag == tag)
    }

    /// 下一个目录的位置（目录末尾的 4 字节）。
    fn next_ifd(&self, ifd: usize) -> Option<usize> {
        let count = usize::from(self.u16_at(ifd)?);
        let next = self.u32_at(ifd + 2 + 12 * count)? as usize;
        (next != 0 && next != ifd).then_some(next)
    }

    fn number(&self, e: Entry) -> Option<u32> {
        match e.kind {
            3 => self.u16_at(e.at).map(u32::from),
            4 => self.u32_at(e.at),
            _ => None,
        }
    }

    fn rational(&self, e: Entry) -> Option<f64> {
        let (n, d) = (self.u32_at(e.at)?, self.u32_at(e.at + 4)?);
        let value = match e.kind {
            5 => f64::from(n) / f64::from(d),
            10 => f64::from(n as i32) / f64::from(d as i32),
            _ => return None,
        };
        (d != 0 && value.is_finite() && value > 0.0).then_some(value)
    }

    fn text(&self, e: Entry) -> Option<String> {
        if e.kind != 2 {
            return None;
        }
        let bytes = self.data.get(e.at..e.at + e.count as usize)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let text = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    /// 第二个目录（IFD1）里的 JPEG 缩略图。
    pub fn thumbnail(&self) -> Option<&'a [u8]> {
        let ifd1 = self.next_ifd(self.ifd0)?;
        let offset = self.number(self.find(ifd1, 0x0201)?)? as usize;
        let length = self.number(self.find(ifd1, 0x0202)?)? as usize;
        self.data.get(offset..offset.checked_add(length)?)
    }

    /// Exif 目录的位置。
    fn exif_ifd(&self) -> Option<usize> {
        self.find(self.ifd0, 0x8769)
            .and_then(|e| self.number(e))
            .map(|v| v as usize)
            .filter(|&v| v != self.ifd0)
    }

    /// “用户注释”（0x9286）：在 Exif 目录里（Fooocus 写在第一个目录里，也找）。开头 8 个字节写编码（ASCII、UNICODE、
    /// JIS 或全 0），后面是文字；没有这 8 个字节的（有的程序直接写文字）整段都是文字。UNICODE 是 UTF-16，字节序各家写得
    /// 不一样（Stable Diffusion WebUI 总按大端写）：看哪一种的高位字节多是 0。AI 生成的图片把生成参数写在这里。
    pub fn user_comment(&self) -> Option<String> {
        let entry = self
            .exif_ifd()
            .and_then(|ifd| self.find(ifd, 0x9286))
            .or_else(|| self.find(self.ifd0, 0x9286))?;
        if !matches!(entry.kind, 2 | 7) {
            return None;
        }
        let bytes = self.data.get(entry.at..entry.at + entry.count as usize)?;
        let known = [b"ASCII\0\0\0".as_slice(), b"UNICODE\0", b"JIS\0\0\0\0\0", &[0u8; 8]];
        let (code, body) = match bytes.get(..8) {
            Some(code) if known.contains(&code) => (code, &bytes[8..]),
            _ => (&[][..], bytes),
        };
        let text = if code == b"UNICODE\0" {
            let units = body.as_chunks::<2>().0;
            let high_zero_be = units.iter().filter(|u| u[0] == 0).count();
            let high_zero_le = units.iter().filter(|u| u[1] == 0).count();
            let big = high_zero_be >= high_zero_le;
            let words: Vec<u16> = units
                .iter()
                .map(|u| if big { u16::from_be_bytes(*u) } else { u16::from_le_bytes(*u) })
                .collect();
            String::from_utf16_lossy(&words)
        } else {
            String::from_utf8_lossy(body).into_owned()
        };
        let text = text
            .trim_matches(|c: char| c == '\0' || c == '\u{feff}' || c.is_whitespace())
            .to_string();
        (!text.is_empty()).then_some(text)
    }

    /// 拍摄信息。
    pub fn shooting(&self) -> ShootingInfo {
        let mut info = ShootingInfo {
            make: self.find(self.ifd0, 0x010F).and_then(|e| self.text(e)),
            model: self.find(self.ifd0, 0x0110).and_then(|e| self.text(e)),
            ..ShootingInfo::default()
        };
        let Some(exif_ifd) = self
            .find(self.ifd0, 0x8769)
            .and_then(|e| self.number(e))
            .map(|v| v as usize)
            .filter(|&v| v != self.ifd0)
        else {
            return info;
        };
        for e in self.entries(exif_ifd) {
            match e.tag {
                0x829A => info.exposure_time = self.rational(e),
                0x829D => info.f_number = self.rational(e),
                0x8827 => info.iso = self.number(e).filter(|&v| v > 0),
                0x9003 => info.taken = self.text(e),
                0x920A => info.focal_length = self.rational(e),
                0xA405 => info.focal_length_35mm = self.number(e).filter(|&v| v > 0),
                0xA434 => info.lens = self.text(e),
                _ => {}
            }
        }
        info
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 测试用的 EXIF：小端或大端，IFD0（厂商、型号、Exif 目录指针），Exif 目录（各项拍摄参数），可选的 IFD1 缩略图。
    pub fn sample(big_endian: bool, thumbnail: Option<&[u8]>) -> Vec<u8> {
        let mut w = Writer::new(big_endian);
        w.entries(
            8,
            &[
                (0x010F, Value::Text("NIKON CORPORATION")),
                (0x0110, Value::Text("NIKON D300")),
                (0x8769, Value::Pointer),
            ],
            thumbnail.is_some(),
        );
        let exif_at = w.len();
        w.patch_pointer(exif_at);
        w.entries(
            exif_at,
            &[
                (0x829A, Value::Rational(1, 250)),
                (0x829D, Value::Rational(56, 10)),
                (0x8827, Value::Short(200)),
                (0x9003, Value::Text("2012:11:10 13:22:46")),
                (0x920A, Value::Rational(3000, 10)),
                (0xA405, Value::Short(450)),
                (0xA434, Value::Text("70.0-300.0 mm f/4.0-5.6")),
            ],
            false,
        );
        if let Some(thumb) = thumbnail {
            let ifd1 = w.len();
            w.patch_next(ifd1);
            let data_at = ifd1 + 2 + 2 * 12 + 4;
            w.entries(
                ifd1,
                &[
                    (0x0201, Value::Long(data_at as u32)),
                    (0x0202, Value::Long(thumb.len() as u32)),
                ],
                false,
            );
            w.bytes.extend_from_slice(thumb);
        }
        w.bytes
    }

    enum Value<'a> {
        Text(&'a str),
        Short(u16),
        Long(u32),
        Rational(u32, u32),
        Pointer,
    }

    /// 简单的 TIFF 写入器：每个目录后面紧跟它的长值。
    struct Writer {
        bytes: Vec<u8>,
        big: bool,
        pointer_at: Option<usize>,
        next_at: Option<usize>,
    }

    impl Writer {
        fn new(big: bool) -> Self {
            let mut bytes = if big { b"MM\0*".to_vec() } else { b"II*\0".to_vec() };
            bytes.extend_from_slice(&if big { 8u32.to_be_bytes() } else { 8u32.to_le_bytes() });
            Self {
                bytes,
                big,
                pointer_at: None,
                next_at: None,
            }
        }
        fn len(&self) -> usize {
            self.bytes.len()
        }
        fn u16(&self, v: u16) -> [u8; 2] {
            if self.big { v.to_be_bytes() } else { v.to_le_bytes() }
        }
        fn u32(&self, v: u32) -> [u8; 4] {
            if self.big { v.to_be_bytes() } else { v.to_le_bytes() }
        }
        fn put32(&mut self, at: usize, v: u32) {
            let b = self.u32(v);
            self.bytes[at..at + 4].copy_from_slice(&b);
        }
        fn patch_pointer(&mut self, target: usize) {
            let at = self.pointer_at.take().unwrap();
            self.put32(at, target as u32);
        }
        fn patch_next(&mut self, target: usize) {
            let at = self.next_at.take().unwrap();
            self.put32(at, target as u32);
        }
        fn entries(&mut self, at: usize, items: &[(u16, Value)], has_next: bool) {
            assert_eq!(at, self.len());
            let count = self.u16(items.len() as u16);
            self.bytes.extend_from_slice(&count);
            let table = self.len();
            self.bytes.resize(table + 12 * items.len() + 4, 0);
            if has_next {
                self.next_at = Some(table + 12 * items.len());
            }
            for (i, (tag, value)) in items.iter().enumerate() {
                let p = table + 12 * i;
                let (kind, count, data): (u16, u32, Vec<u8>) = match value {
                    Value::Text(t) => {
                        let mut d = t.as_bytes().to_vec();
                        d.push(0);
                        (2, d.len() as u32, d)
                    }
                    Value::Short(v) => (3, 1, self.u16(*v).to_vec()),
                    Value::Long(v) => (4, 1, self.u32(*v).to_vec()),
                    Value::Rational(n, d) => {
                        let mut b = self.u32(*n).to_vec();
                        b.extend_from_slice(&self.u32(*d));
                        (5, 1, b)
                    }
                    Value::Pointer => {
                        self.pointer_at = Some(p + 8);
                        (4, 1, vec![0; 4])
                    }
                };
                let tag = self.u16(*tag);
                let kind_b = self.u16(kind);
                let count_b = self.u32(count);
                self.bytes[p..p + 2].copy_from_slice(&tag);
                self.bytes[p + 2..p + 4].copy_from_slice(&kind_b);
                self.bytes[p + 4..p + 8].copy_from_slice(&count_b);
                if data.len() <= 4 {
                    self.bytes[p + 8..p + 8 + data.len()].copy_from_slice(&data);
                } else {
                    let offset = self.len() as u32;
                    self.put32(p + 8, offset);
                    self.bytes.extend_from_slice(&data);
                }
            }
        }
    }

    #[test]
    fn reads_shooting_info_in_both_byte_orders() {
        for big in [false, true] {
            let data = sample(big, None);
            let info = Exif::parse(&data).unwrap().shooting();
            assert_eq!(info.make.as_deref(), Some("NIKON CORPORATION"));
            assert_eq!(info.model.as_deref(), Some("NIKON D300"));
            assert_eq!(info.lens.as_deref(), Some("70.0-300.0 mm f/4.0-5.6"));
            assert_eq!(info.focal_length, Some(300.0));
            assert_eq!(info.focal_length_35mm, Some(450));
            assert_eq!(info.f_number, Some(5.6));
            assert_eq!(info.exposure_time, Some(0.004));
            assert_eq!(info.iso, Some(200));
            assert_eq!(info.taken.as_deref(), Some("2012:11:10 13:22:46"));
        }
    }

    #[test]
    fn finds_the_thumbnail_in_ifd1() {
        let data = sample(true, Some(b"\xFF\xD8thumb"));
        assert_eq!(Exif::parse(&data).unwrap().thumbnail(), Some(&b"\xFF\xD8thumb"[..]));
        assert!(Exif::parse(&sample(false, None)).unwrap().thumbnail().is_none());
    }

    #[test]
    fn broken_data_is_ignored() {
        assert!(Exif::parse(b"II*\0").is_none());
        assert!(Exif::parse(b"XX*\0\x08\0\0\0").is_none());
        // 目录指向自己、值越界、截断：都不出错，只是读不到。
        let mut looped = b"II*\0\x08\0\0\0\0\0".to_vec();
        looped.extend_from_slice(&8u32.to_le_bytes());
        let exif = Exif::parse(&looped).unwrap();
        assert!(exif.thumbnail().is_none() && exif.shooting().is_empty());
        let data = sample(false, None);
        for cut in [10, 30, 60, data.len() - 5] {
            if let Some(exif) = Exif::parse(&data[..cut]) {
                let _ = exif.shooting();
                let _ = exif.thumbnail();
            }
        }
    }
}
