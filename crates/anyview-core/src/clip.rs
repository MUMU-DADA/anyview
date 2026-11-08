//! Clip Studio Paint（.clip）：显示文件里保存的整幅画布预览图，不解析图层（FORMATS.md）。
//!
//! 文件结构：开头 `CSFCHUNK`、8 字节文件长度、8 字节第一块的位置（24），之后一块接一块：`CHNK` + 4 字节类型
//! （`Head`、`Exta`、`SQLi`、`Foot`）+ 8 字节长度（大端）+ 数据。`SQLi` 块是一个完整的 SQLite 数据库，
//! 其中 `CanvasPreview` 表的 `ImageData` 是画布的预览图（PNG；画布不大时与画布同样大小，大画布按比例缩小）。
//! 图层的像素在 `Exta` 块里，按图层、按块压缩存放，常常不完整（软件只存了显示需要的部分），所以不用它们。
//!
//! 数据库用 SQLite 本身读（rusqlite，从源代码编译），只读、在内存里打开，不写临时文件；不信任文件里的表结构
//! （trusted_schema 关掉，防御模式打开）。

use crate::i18n::Message;
use rusqlite::{Connection, MAIN_DB, config::DbConfig};

use crate::decode::Raster;

/// 扩展名（小写，不带点）。
pub const EXTENSIONS: &[&str] = &["clip"];

const MAGIC: &[u8] = b"CSFCHUNK";

/// 文件头是不是 Clip Studio Paint 的文件。
pub fn is_clip(head: &[u8]) -> bool {
    head.starts_with(MAGIC)
}

/// 解码：取画布预览图。
pub fn decode(data: &[u8]) -> Result<Raster, Message> {
    let database =
        sqlite_chunk(data).ok_or(msg!("Clip Studio Paint：文件里没有数据库（文件不完整？）", "Clip Studio Paint: no database in the file (is it incomplete?)"))?;
    let png = canvas_preview(database)
        .map_err(|e| msg!("Clip Studio Paint：读不出画布预览图（{e}）", "Clip Studio Paint: can't read the canvas preview ({e})", e = format!("{e}")))?;
    let image = crate::decode::load_limited(&png, image::ImageFormat::Png)
        .map_err(|e| msg!("Clip Studio Paint：预览图解不开（{e}）", "Clip Studio Paint: can't decode the preview ({e})", e = format!("{e}")))?;
    let has_alpha = image.color().has_alpha();
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    // 按尺寸检查（与其他解码器相同的上限）。
    if u64::from(width) * u64::from(height) * 4 > crate::decode::alloc_limit() {
        crate::decode::limit_hit();
        return Err(msg!("图片太大：{width}x{height}", "Image too large: {width}x{height}", width = format!("{width}"), height = format!("{height}")));
    }
    Ok(Raster {
        width,
        height,
        rgba: rgba.into_raw(),
        has_alpha,
        format: "CLIP".to_string(),
        pixel_format: if has_alpha { "RGBA32" } else { "RGB24" }.to_string(),
        icc_profile: None,
    })
}

/// 找到 `SQLi` 块（数据库）。块的长度不对、越过文件结尾时当作没有。
fn sqlite_chunk(data: &[u8]) -> Option<&[u8]> {
    if !data.starts_with(MAGIC) {
        return None;
    }
    let read_u64 = |at: usize| -> Option<usize> {
        let bytes: [u8; 8] = data.get(at..at.checked_add(8)?)?.try_into().ok()?;
        usize::try_from(u64::from_be_bytes(bytes)).ok()
    };
    let mut offset = read_u64(16)?;
    while let Some(header) = data.get(offset..offset.checked_add(16)?) {
        if &header[..4] != b"CHNK" {
            return None;
        }
        let size = read_u64(offset + 8)?;
        let start = offset + 16;
        let end = start.checked_add(size)?;
        let body = data.get(start..end)?;
        if &header[4..8] == b"SQLi" {
            return Some(body);
        }
        offset = end;
    }
    None
}

/// 在内存里只读打开数据库，取最大的那张画布预览图。
///
/// 数据库来自文件，不可信：`CanvasPreview` 必须是普通的表（视图可以写成无穷多行的递归查询，查询永远不结束）；
/// 另外按 SQLite 执行的指令数设上限，超过就中断，不管里面写了什么都会很快结束。
fn canvas_preview(database: &[u8]) -> rusqlite::Result<Vec<u8>> {
    let mut connection = Connection::open_in_memory()?;
    // 文件里的表结构不可信：视图、触发器不能调用有副作用的函数；不允许改坏数据库的写法。
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    connection.deserialize_read_exact(MAIN_DB, database, database.len(), true)?;
    // 每 1000 条指令回调一次，累计 2 亿条（正常的文件只要几千条）就中断。
    let mut budget = 200_000u32;
    connection.progress_handler(
        1000,
        Some(move || {
            budget = budget.saturating_sub(1);
            budget == 0
        }),
    )?;
    let kind: String = connection.query_row(
        "SELECT type FROM sqlite_schema WHERE name = 'CanvasPreview'",
        [],
        |row| row.get(0),
    )?;
    if kind != "table" {
        return Err(rusqlite::Error::InvalidQuery);
    }
    connection.query_row(
        "SELECT ImageData FROM CanvasPreview WHERE ImageData IS NOT NULL \
         ORDER BY ImageWidth * ImageHeight DESC LIMIT 1",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按 Clip Studio Paint 的结构拼一个文件：Head 块、SQLi 块（给定的数据库）、Foot 块。
    fn container(database: &[u8]) -> Vec<u8> {
        let mut chunks = Vec::new();
        for (kind, body) in [
            (b"Head", vec![0u8; 40]),
            (b"SQLi", database.to_vec()),
            (b"Foot", Vec::new()),
        ] {
            chunks.extend_from_slice(b"CHNK");
            chunks.extend_from_slice(kind);
            chunks.extend_from_slice(&(body.len() as u64).to_be_bytes());
            chunks.extend_from_slice(&body);
        }
        let mut file = MAGIC.to_vec();
        file.extend_from_slice(&((24 + chunks.len()) as u64).to_be_bytes());
        file.extend_from_slice(&24u64.to_be_bytes());
        file.extend_from_slice(&chunks);
        file
    }

    fn database_with_preview(png: &[u8]) -> Vec<u8> {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE CanvasPreview (_PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, \
                 CanvasId INTEGER, ImageType INTEGER, ImageWidth INTEGER, ImageHeight INTEGER, ImageData BLOB);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO CanvasPreview (MainId, CanvasId, ImageType, ImageWidth, ImageHeight, ImageData) \
                 VALUES (1, 1, 1, 3, 2, ?1)",
                [png],
            )
            .unwrap();
        connection.serialize(MAIN_DB).unwrap().to_vec()
    }

    fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn reads_the_canvas_preview() {
        let file = container(&database_with_preview(&png(3, 2, [10, 20, 30, 128])));
        assert!(is_clip(&file));
        let raster = decode(&file).unwrap();
        assert_eq!((raster.width, raster.height), (3, 2));
        assert_eq!(&raster.rgba[..4], &[10, 20, 30, 128]);
        assert!(raster.has_alpha);
        assert_eq!(raster.format, "CLIP");
    }

    #[test]
    fn damaged_files_report_errors() {
        let file = container(&database_with_preview(&png(3, 2, [1, 2, 3, 255])));
        // 截断：数据库块越过文件结尾。
        assert!(decode(&file[..file.len() / 2]).is_err());
        // 数据库坏了。
        let mut broken = file.clone();
        let at = broken
            .windows(15)
            .position(|w| w == b"SQLite format 3")
            .unwrap();
        for byte in &mut broken[at + 100..at + 400] {
            *byte = 0xA5;
        }
        assert!(decode(&broken).is_err());
        // 没有预览图的表。
        let empty = Connection::open_in_memory()
            .unwrap()
            .serialize(MAIN_DB)
            .unwrap()
            .to_vec();
        assert!(decode(&container(&empty)).is_err());
        // CanvasPreview 是一个无穷多行的递归视图：拒绝（不会一直查下去）。
        let hostile = {
            let mut connection = Connection::open_in_memory().unwrap();
            connection
                .execute_batch(
                    "CREATE VIEW CanvasPreview AS WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n) \
                     SELECT NULL AS ImageData, i AS ImageWidth, i AS ImageHeight FROM n;",
                )
                .unwrap();
            connection.serialize(MAIN_DB).unwrap().to_vec()
        };
        let started = std::time::Instant::now();
        assert!(decode(&container(&hostile)).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(decode(b"CSFCHUNK").is_err());
    }
}
