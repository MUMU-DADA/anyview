//! AI 生成的图片里记的生成信息：提示词、反向提示词、参数，看图时在“生成信息”面板里显示、复制。
//!
//! 读的地方：PNG 的文字块（tEXt、zTXt、iTXt；只读认得的关键字）；JPEG、WebP 的 EXIF“用户注释”（Stable Diffusion WebUI
//! 存成 JPEG、WebP 时写在这里）和 JPEG 的注释段。认的写法：
//! - Stable Diffusion WebUI、Forge、SD.Next：`parameters`，提示词、`Negative prompt:` 一行、参数行
//!   （`Steps: 20, Sampler: …`），按 A1111 自己的规则拆。
//! - ComfyUI：`prompt`（执行的节点图，JSON）：从采样器顺着连线找到正向、反向的文字编码节点，读出文字；种子、步数、
//!   CFG、采样器、模型、LoRA、尺寸也顺着连线找。找不到采样器时把所有文字编码节点的文字列出来。
//! - NovelAI：`Comment`（JSON）、`Description`、`Source`。
//! - InvokeAI：`invokeai_metadata`（3.0 起）、`sd-metadata`（2.x）。
//! - SwarmUI、Fooocus：`parameters` 写成 JSON 的。
//!
//! 不解码图像。坏文件、恶意文件的防护：PNG 只认几个关键字的文字块（别的不解压），解压后的文字也计入总量（最多 32 MB）；
//! 只读到图像数据为止，再看一眼文件末尾；节点图按节点去重、总步数和输出都有上限；显示的文字太长时截断（复制原文照旧完整）。
//! 没有关键字说明的文字（EXIF 的用户注释、JPEG 的注释段）别的程序也会写（“Created with GIMP”、手机厂商的标记），
//! 只认确实像生成信息的（[`from_untagged`]）。

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::{Map, Value};

/// 一块文字最多读多少（ComfyUI 的节点图有几百 KB）。
const MAX_TEXT: usize = 8 << 20;
/// 一个文件里的文字加起来最多读多少（压缩的按解压后算）。
const MAX_TOTAL: usize = 32 << 20;
/// 认得的文字块关键字：只读这些（XMP、“Raw profile”等别的块不解压、不留），同一个关键字只留第一个。
const KEYWORDS: &[&str] = &[
    "parameters",
    "prompt",
    "Software",
    "Comment",
    "Description",
    "Source",
    "invokeai_metadata",
    "sd-metadata",
];
/// PNG 图像数据之后只看文件末尾这么多（个别程序把文字块写在图像数据后面）。
const PNG_TAIL: u64 = 64 << 10;
/// 显示的文字最多多长（字节）；复制原文照旧是完整的。
const MAX_SHOWN: usize = 64 << 10;

/// 生成信息。参数的名字照生成工具自己的写法（如 Steps、Sampler、CFG scale），不翻译。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GenerationInfo {
    /// 生成工具，如 "ComfyUI"、"Stable Diffusion WebUI"。
    pub tool: String,
    pub prompt: String,
    /// 反向提示词；没有时为空。
    pub negative: String,
    /// 参数：(名字, 值)，按显示的先后。
    pub params: Vec<(String, String)>,
    /// 文件里原样的那段文字（“复制原文”用）。
    pub raw: String,
}

impl GenerationInfo {
    /// 显示用的文字截到合理的长度（恶意文件里几 MB 的“工具名”、提示词会卡住界面）；原文不截。
    fn limited(mut self) -> Self {
        clip(&mut self.tool, 80);
        clip(&mut self.prompt, MAX_SHOWN);
        clip(&mut self.negative, MAX_SHOWN);
        self.params.truncate(200);
        for (name, value) in &mut self.params {
            clip(name, 200);
            clip(value, 4096);
        }
        self
    }
}

/// 超过 `max` 字节时在字符边界截断，加上“…”。
fn clip(text: &mut String, max: usize) {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
}

/// “生成信息”面板上的文字（三端共用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelTexts {
    pub title: String,
    /// 生成工具那一行，`{tool}` 换成工具的名字。
    pub tool: String,
    pub prompt: String,
    pub negative: String,
    pub params: String,
    pub copy: String,
    /// 复制文件里原样的那段文字。
    pub copy_raw: String,
    /// 复制以后按钮上短暂显示的字。
    pub copied: String,
    /// 当前图片没有生成信息时面板上的一行字。
    pub empty: String,
}

pub fn panel_texts(lang: crate::i18n::Lang) -> PanelTexts {
    PanelTexts {
        title: tr!(lang, "生成信息", "Generation Info").to_string(),
        tool: tr!(lang, "生成工具：{tool}", "Generated with: {tool}").to_string(),
        prompt: tr!(lang, "提示词", "Prompt").to_string(),
        negative: tr!(lang, "反向提示词", "Negative prompt").to_string(),
        params: tr!(lang, "参数", "Parameters").to_string(),
        copy: tr!(lang, "复制", "Copy").to_string(),
        copy_raw: tr!(lang, "复制原文", "Copy Raw Text").to_string(),
        copied: tr!(lang, "已复制", "Copied").to_string(),
        empty: tr!(lang, "这张图片里没有 AI 生成信息。", "This image has no AI generation info.").to_string(),
    }
}

/// 读一个文件里的生成信息；不是 PNG、JPEG、WebP，或者没有认得的生成信息时为空。
pub fn read(path: &Path) -> Option<GenerationInfo> {
    let mut file = BufReader::with_capacity(64 << 10, File::open(path).ok()?);
    let mut head = [0u8; 12];
    file.read_exact(&mut head).ok()?;
    let info = if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        let found = png_texts(&mut file);
        from_texts(&found.texts).or_else(|| found.exif.and_then(|e| from_exif(&e)))
    } else if head.starts_with(&[0xFF, 0xD8]) {
        let (exif, comment) = jpeg_segments(&mut file);
        exif.and_then(|e| from_exif(&e))
            .or_else(|| comment.and_then(|c| from_untagged(&c)))
    } else if head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        webp_exif(&mut file).and_then(|e| from_exif(&e))
    } else {
        None
    };
    info.map(GenerationInfo::limited)
}

// ---- 读文件 ----

/// PNG 里读到的：认得的文字块（关键字, 文字）和 eXIf 块。
#[derive(Default)]
struct PngTexts {
    texts: Vec<(String, String)>,
    exif: Option<Vec<u8>>,
    /// 还能读多少（压缩的按解压后算）。
    budget: usize,
}

impl PngTexts {
    /// 这一块要不要：eXIf（第一个），或者认得的、还没有的关键字的文字块。
    fn wants(&self, kind: &[u8; 4], keyword: &[u8]) -> bool {
        if kind == b"eXIf" {
            return self.exif.is_none();
        }
        let keyword = latin1(keyword);
        KEYWORDS.contains(&keyword.as_str()) && !self.texts.iter().any(|(k, _)| *k == keyword)
    }

    fn take(&mut self, kind: &[u8; 4], data: Vec<u8>) {
        self.budget = self.budget.saturating_sub(data.len());
        if kind == b"eXIf" {
            self.exif = Some(data);
        } else if let Some((keyword, text)) = png_text(kind, &data, self.budget) {
            self.budget = self.budget.saturating_sub(text.len());
            self.texts.push((keyword, text));
        }
    }
}

/// PNG 的文字块：从头读到图像数据（IDAT）为止，只读认得的关键字（先看关键字，别的块整块跳过）；再看一眼文件末尾。
fn png_texts(file: &mut BufReader<File>) -> PngTexts {
    let mut found = PngTexts {
        budget: MAX_TOTAL,
        ..PngTexts::default()
    };
    if file.seek(SeekFrom::Start(8)).is_err() {
        return found;
    }
    loop {
        let mut header = [0u8; 8];
        if file.read_exact(&mut header).is_err() {
            return found;
        }
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let kind = [header[4], header[5], header[6], header[7]];
        match &kind {
            b"IEND" => return found,
            b"IDAT" => break,
            b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" => {
                // 关键字最多 79 个字节：先读开头看是不是要的，不要的整块跳过（不读、不解压）。
                let prefix = length.min(80);
                let mut data = vec![0u8; prefix];
                if file.read_exact(&mut data).is_err() {
                    return found;
                }
                let keyword_end = data.iter().position(|&b| b == 0).unwrap_or(prefix);
                if found.wants(&kind, &data[..keyword_end]) && length <= MAX_TEXT && length <= found.budget {
                    data.resize(length, 0);
                    if file.read_exact(&mut data[prefix..]).is_err() {
                        return found;
                    }
                    found.take(&kind, data);
                    if file.seek_relative(4).is_err() {
                        return found;
                    }
                } else if file.seek_relative((length - prefix) as i64 + 4).is_err() {
                    return found;
                }
            }
            _ => {
                if file.seek_relative(length as i64 + 4).is_err() {
                    return found;
                }
            }
        }
    }
    png_tail(file, &mut found);
    found
}

/// 图像数据之后：只读文件末尾 [`PNG_TAIL`]，按“长度、类型、内容、CRC”认出里面的文字块（CRC 对得上才算）。
fn png_tail(file: &mut BufReader<File>, found: &mut PngTexts) {
    let Ok(here) = file.stream_position() else { return };
    let Ok(end) = file.seek(SeekFrom::End(0)) else { return };
    let start = end.saturating_sub(PNG_TAIL).max(here);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut tail = Vec::new();
    if file.by_ref().take(end - start).read_to_end(&mut tail).is_err() {
        return;
    }
    let mut at = 4;
    while at + 8 <= tail.len() {
        let kind: [u8; 4] = tail[at..at + 4].try_into().unwrap_or_default();
        if matches!(&kind, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf") {
            let length = u32::from_be_bytes([tail[at - 4], tail[at - 3], tail[at - 2], tail[at - 1]]) as usize;
            let data_end = at + 4 + length;
            if data_end + 4 <= tail.len() {
                let mut crc = flate2::Crc::new();
                crc.update(&tail[at..data_end]);
                let stored = u32::from_be_bytes([tail[data_end], tail[data_end + 1], tail[data_end + 2], tail[data_end + 3]]);
                if crc.sum() == stored {
                    let data = &tail[at + 4..data_end];
                    let keyword_end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
                    if found.wants(&kind, &data[..keyword_end]) && length <= found.budget {
                        found.take(&kind, data.to_vec());
                    }
                    at = data_end + 4 + 4;
                    continue;
                }
            }
        }
        at += 1;
    }
}

/// 一个文字块 → (关键字, 文字)。tEXt、zTXt 规定是 Latin-1，很多程序其实写的是 UTF-8：先按 UTF-8 读。
/// 压缩的最多解压 `budget` 字节（和 [`MAX_TEXT`] 取小的）。
fn png_text(kind: &[u8; 4], data: &[u8], budget: usize) -> Option<(String, String)> {
    let zero = data.iter().position(|&b| b == 0)?;
    let keyword = latin1(&data[..zero]);
    let rest = &data[zero + 1..];
    let limit = budget.min(MAX_TEXT);
    let text = match kind {
        b"tEXt" => loose_text(rest),
        b"zTXt" => loose_text(&inflate(rest.get(1..)?, limit)?),
        _ => {
            // iTXt：压缩标志、压缩方法、语言\0、翻译后的关键字\0、文字（UTF-8）。
            let (&compressed, rest) = rest.split_first()?;
            let rest = rest.get(1..)?;
            let language_end = rest.iter().position(|&b| b == 0)?;
            let rest = &rest[language_end + 1..];
            let translated_end = rest.iter().position(|&b| b == 0)?;
            let body = &rest[translated_end + 1..];
            if compressed == 1 {
                String::from_utf8_lossy(&inflate(body, limit)?).into_owned()
            } else {
                String::from_utf8_lossy(body).into_owned()
            }
        }
    };
    Some((keyword, text))
}

/// 解压 zlib，最多 `limit` 字节（解压炸弹不会把内存用光）。
fn inflate(data: &[u8], limit: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(limit as u64)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// 按 UTF-8 读；不是 UTF-8 时：大部分是的（个别坏字节，比如截断了）按 UTF-8 读、坏的换掉，否则按 Latin-1。
fn loose_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(e) if e.valid_up_to() * 10 >= bytes.len() * 9 => String::from_utf8_lossy(bytes).into_owned(),
        Err(_) => latin1(bytes),
    }
}

/// JPEG 开头的 EXIF（APP1 “Exif\0\0” 之后的 TIFF 数据）和注释段（COM），读到图像数据（SOS）为止。
fn jpeg_segments(file: &mut BufReader<File>) -> (Option<Vec<u8>>, Option<String>) {
    let (mut exif, mut comment) = (None, None);
    if file.seek(SeekFrom::Start(2)).is_err() {
        return (exif, comment);
    }
    for _ in 0..64 {
        let mut marker = [0u8; 4];
        if file.read_exact(&mut marker).is_err() || marker[0] != 0xFF {
            break;
        }
        let kind = marker[1];
        if kind == 0xDA || kind == 0xD9 {
            break;
        }
        let length = usize::from(u16::from_be_bytes([marker[2], marker[3]])).saturating_sub(2);
        let wanted = (kind == 0xE1 && exif.is_none()) || (kind == 0xFE && comment.is_none());
        if !wanted {
            if file.seek_relative(length as i64).is_err() {
                break;
            }
            continue;
        }
        let mut data = vec![0u8; length];
        if file.read_exact(&mut data).is_err() {
            break;
        }
        match kind {
            0xE1 if data.starts_with(b"Exif\0\0") => exif = Some(data[6..].to_vec()),
            0xFE => comment = Some(loose_text(&data)),
            _ => {}
        }
    }
    (exif, comment)
}

/// WebP 的 EXIF 块（有的程序在前面多写了 “Exif\0\0”）。
fn webp_exif(file: &mut BufReader<File>) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(12)).ok()?;
    for _ in 0..64 {
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let padded = length + (length & 1);
        if &header[..4] == b"EXIF" && length <= MAX_TEXT {
            let mut data = vec![0u8; length];
            file.read_exact(&mut data).ok()?;
            return Some(match data.strip_prefix(b"Exif\0\0") {
                Some(rest) => rest.to_vec(),
                None => data,
            });
        }
        file.seek_relative(padded as i64).ok()?;
    }
    None
}

fn from_exif(exif: &[u8]) -> Option<GenerationInfo> {
    let comment = crate::exif::Exif::parse(exif)?.user_comment()?;
    from_untagged(&comment)
}

// ---- 各种写法 ----

/// PNG 的文字块里认得的生成信息。
fn from_texts(texts: &[(String, String)]) -> Option<GenerationInfo> {
    let get = |key: &str| {
        texts
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    if let Some(prompt) = get("prompt")
        && let Some(info) = comfy(prompt)
    {
        return Some(info);
    }
    if let Some(parameters) = get("parameters")
        && let Some(info) = from_parameters(parameters)
    {
        return Some(info);
    }
    if get("Software").is_some_and(|s| s.starts_with("NovelAI"))
        && let Some(info) = novelai(get("Comment"), get("Description"), get("Source"))
    {
        return Some(info);
    }
    if let Some(metadata) = get("invokeai_metadata")
        && let Some(info) = invoke(metadata)
    {
        return Some(info);
    }
    get("sd-metadata").and_then(invoke_v2)
}

/// `parameters` 那一段（PNG 的文字块里有这个关键字，本身就说明是生成参数）：JSON 的（SwarmUI、Fooocus），
/// 不是 JSON 的（NovelAI 风格的 `{{masterpiece}}` 开头的提示词也是）按 A1111 的文字拆。
fn from_parameters(text: &str) -> Option<GenerationInfo> {
    let text = text.trim_matches(|c: char| c == '\0' || c == '\u{feff}' || c.is_whitespace());
    if text.starts_with('{')
        && let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(info) = json_parameters(&value, text)
    {
        return Some(info);
    }
    a1111(text)
}

/// 没有关键字说明的文字（EXIF 的用户注释、JPEG 的注释段）：别的程序也往这里写东西（“Created with GIMP”、手机厂商的
/// 标记），只认确实像生成信息的——JSON 的生成参数，或者 A1111 的写法里有参数行（Steps、Sampler、Seed、CFG scale 之一）
/// 或 `Negative prompt:` 一行。
fn from_untagged(text: &str) -> Option<GenerationInfo> {
    let text = text.trim_matches(|c: char| c == '\0' || c == '\u{feff}' || c.is_whitespace());
    if text.starts_with('{')
        && let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(info) = json_parameters(&value, text)
    {
        return Some(info);
    }
    let info = a1111(text)?;
    let known = info
        .params
        .iter()
        .any(|(k, _)| matches!(k.as_str(), "Steps" | "Sampler" | "Seed" | "CFG scale"));
    (known || !info.negative.is_empty()).then_some(info)
}

/// Stable Diffusion WebUI的写法：提示词；`Negative prompt:` 开头的反向提示词；参数行（以 `Steps: ` 开头、
/// 有 3 个以上“名字: 值”的最后一行；没有时按 A1111 自己的判断：最后一行有 3 个以上）。参数行后面的行（如 Dynamic
/// Prompts 的 `Template:`）也当参数。
fn a1111(text: &str) -> Option<GenerationInfo> {
    if text.is_empty() {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .rposition(|l| l.starts_with("Steps: ") && parse_params(l).len() >= 3)
        .or_else(|| (parse_params(lines.last()?).len() >= 3).then(|| lines.len() - 1));
    let (body, params) = match at {
        Some(i) => {
            let mut params = parse_params(lines[i]);
            for extra in &lines[i + 1..] {
                if let Some((name, value)) = extra.split_once(": ")
                    && !name.trim().is_empty()
                {
                    params.push((name.trim().to_string(), value.trim().to_string()));
                }
            }
            (&lines[..i], params)
        }
        None => (&lines[..], Vec::new()),
    };
    let negative_at = body.iter().position(|l| l.starts_with("Negative prompt:"));
    let (prompt, negative) = match negative_at {
        Some(i) => {
            let negative = body[i..].join("\n");
            let negative = negative.trim_start_matches("Negative prompt:").trim().to_string();
            (body[..i].join("\n"), negative)
        }
        None => (body.join("\n"), String::new()),
    };
    let value = |name: &str| params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let tool = match (value("App"), value("Version")) {
        (Some(app), _) => app.to_string(),
        (None, Some(version)) if version.starts_with('f') => "Stable Diffusion WebUI Forge".to_string(),
        _ => "Stable Diffusion WebUI".to_string(),
    };
    if prompt.trim().is_empty() && negative.is_empty() && params.is_empty() {
        return None;
    }
    Some(GenerationInfo {
        tool,
        prompt: prompt.trim().to_string(),
        negative,
        params,
        raw: text.to_string(),
    })
}

/// A1111 的参数行，照它自己的正则 `\s*(\w[\w \-/]+):\s*("(?:\\.|[^\\"])+"|[^,]*)(?:,|$)` 一个个找（findall）：
/// 名字是字母数字开头的一串；值是 JSON 写法的带引号字符串（里面可以有逗号、冒号，按 JSON 解开转义），或者到下一个逗号为止；
/// 认不出的片段跳过，接着往后找。
fn parse_params(line: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = line.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut params = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let mut j = i;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        let name_start = j;
        if j < chars.len() && word(chars[j]) {
            j += 1;
            while j < chars.len() && (word(chars[j]) || matches!(chars[j], ' ' | '-' | '/')) {
                j += 1;
            }
        }
        if j - name_start < 2 || j >= chars.len() || chars[j] != ':' {
            i = start + 1;
            continue;
        }
        let name: String = chars[name_start..j].iter().collect();
        j += 1;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        // 带引号的值：到没有转义的那个引号为止，后面必须是逗号或者结尾；不然按不带引号的值读。
        let mut value = None;
        if j < chars.len() && chars[j] == '"' {
            let mut k = j + 1;
            while k < chars.len() && chars[k] != '"' {
                k += if chars[k] == '\\' { 2 } else { 1 };
            }
            if k < chars.len() && k > j + 1 && (k + 1 == chars.len() || chars[k + 1] == ',') {
                let quoted: String = chars[j..=k].iter().collect();
                value = Some(serde_json::from_str::<String>(&quoted).unwrap_or_else(|_| chars[j + 1..k].iter().collect()));
                j = k + 1;
            }
        }
        let value = match value {
            Some(v) => v,
            None => {
                let mut k = j;
                while k < chars.len() && chars[k] != ',' {
                    k += 1;
                }
                let v: String = chars[j..k].iter().collect();
                j = k;
                v
            }
        };
        if j < chars.len() && chars[j] == ',' {
            j += 1;
        }
        params.push((name.trim().to_string(), value.trim().to_string()));
        i = j;
    }
    params
}

/// 写成 JSON 的 `parameters`：SwarmUI（`sui_image_params`）、Fooocus（`Prompt`、`Negative Prompt`）。
fn json_parameters(value: &Value, raw: &str) -> Option<GenerationInfo> {
    let object = value.as_object()?;
    if let Some(swarm) = object.get("sui_image_params").and_then(Value::as_object) {
        let mut info = from_object(swarm, &["prompt"], &["negativeprompt"], raw)?;
        info.tool = "SwarmUI".to_string();
        return Some(info);
    }
    let mut info = from_object(object, &["Prompt", "prompt"], &["Negative Prompt", "negative_prompt"], raw)?;
    let version = object.get("Version").and_then(Value::as_str).unwrap_or("");
    info.tool = if version.starts_with("Fooocus") { "Fooocus" } else { "Stable Diffusion" }.to_string();
    Some(info)
}

/// JSON 对象 → 生成信息：提示词、反向提示词按给的几个名字找，别的简单值都列成参数（按名字排）。
fn from_object(object: &Map<String, Value>, prompt: &[&str], negative: &[&str], raw: &str) -> Option<GenerationInfo> {
    let find = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| object.get(*n).and_then(Value::as_str))
            .map(str::to_string)
    };
    let prompt_text = find(prompt)?;
    let negative_text = find(negative).unwrap_or_default();
    let params = object
        .iter()
        .filter(|(k, _)| !prompt.contains(&k.as_str()) && !negative.contains(&k.as_str()))
        .filter_map(|(k, v)| scalar(v).map(|s| (k.clone(), s)))
        .collect();
    Some(GenerationInfo {
        tool: String::new(),
        prompt: prompt_text,
        negative: negative_text,
        params,
        raw: raw.to_string(),
    })
}

/// 简单的值（文字、数字、真假）写成文字；对象、数组、空不算。
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// NovelAI：`Comment` 里的 JSON（prompt、uc、steps、scale、seed、sampler、width、height；4.x 的在 v4_prompt 里），
/// 没有时用 `Description` 当提示词；模型在 `Source`。
fn novelai(comment: Option<&str>, description: Option<&str>, source: Option<&str>) -> Option<GenerationInfo> {
    let json: Option<Value> = comment.and_then(|c| serde_json::from_str(c).ok());
    let object = json.as_ref().and_then(Value::as_object);
    let field = |key: &str| object.and_then(|o| o.get(key));
    let caption = |key: &str| {
        field(key)
            .and_then(|v| v.pointer("/caption/base_caption"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let prompt = field("prompt")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| caption("v4_prompt"))
        .or_else(|| description.map(str::to_string))?;
    let negative = field("uc")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| caption("v4_negative_prompt"))
        .unwrap_or_default();
    let mut params = Vec::new();
    if let Some(model) = source {
        params.push(("Model".to_string(), model.to_string()));
    }
    for (label, key) in [("Steps", "steps"), ("Sampler", "sampler"), ("CFG scale", "scale"), ("Seed", "seed"), ("Noise schedule", "noise_schedule")] {
        if let Some(value) = field(key).and_then(scalar) {
            params.push((label.to_string(), value));
        }
    }
    if let (Some(w), Some(h)) = (field("width").and_then(scalar), field("height").and_then(scalar)) {
        params.push(("Size".to_string(), format!("{w}x{h}")));
    }
    Some(GenerationInfo {
        tool: "NovelAI".to_string(),
        prompt,
        negative,
        params,
        raw: comment.unwrap_or_default().to_string(),
    })
}

/// InvokeAI 3.0 起的 `invokeai_metadata`。
fn invoke(text: &str) -> Option<GenerationInfo> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    let prompt = object.get("positive_prompt").and_then(Value::as_str)?.to_string();
    let negative = object
        .get("negative_prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut params = Vec::new();
    let model = object.get("model").and_then(|m| {
        m.get("name")
            .or_else(|| m.get("model_name"))
            .and_then(Value::as_str)
            .or_else(|| m.as_str())
            .map(str::to_string)
    });
    if let Some(model) = model {
        params.push(("Model".to_string(), model));
    }
    for (label, key) in [("Seed", "seed"), ("Steps", "steps"), ("CFG scale", "cfg_scale"), ("Scheduler", "scheduler"), ("Denoising strength", "strength")] {
        if let Some(value) = object.get(key).and_then(scalar) {
            params.push((label.to_string(), value));
        }
    }
    if let (Some(w), Some(h)) = (object.get("width").and_then(scalar), object.get("height").and_then(scalar)) {
        params.push(("Size".to_string(), format!("{w}x{h}")));
    }
    Some(GenerationInfo {
        tool: "InvokeAI".to_string(),
        prompt,
        negative,
        params,
        raw: text.to_string(),
    })
}

/// InvokeAI 2.x 的 `sd-metadata`：提示词在 image.prompt（文字，或 [{prompt, weight}] 的数组）。
fn invoke_v2(text: &str) -> Option<GenerationInfo> {
    let value: Value = serde_json::from_str(text).ok()?;
    let image = value.get("image")?;
    let prompt = match image.get("prompt")? {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("prompt").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let mut params = Vec::new();
    if let Some(model) = value.get("model_weights").and_then(Value::as_str) {
        params.push(("Model".to_string(), model.to_string()));
    }
    for (label, key) in [("Seed", "seed"), ("Steps", "steps"), ("CFG scale", "cfg_scale"), ("Sampler", "sampler")] {
        if let Some(v) = image.get(key).and_then(scalar) {
            params.push((label.to_string(), v));
        }
    }
    if let (Some(w), Some(h)) = (image.get("width").and_then(scalar), image.get("height").and_then(scalar)) {
        params.push(("Size".to_string(), format!("{w}x{h}")));
    }
    Some(GenerationInfo {
        tool: "InvokeAI".to_string(),
        prompt,
        negative: String::new(),
        params,
        raw: text.to_string(),
    })
}

// ---- ComfyUI ----

/// ComfyUI 的 `prompt`：节点编号 → {class_type, inputs}；inputs 里的连线写成 [节点编号, 输出序号]。
struct Graph<'a> {
    nodes: &'a Map<String, Value>,
    /// 一共走了多少步：很多条路通到同一个节点时也不会按路径重复走上指数级的步数（恶意文件）。
    steps: Cell<usize>,
}

/// 顺着连线最多走多少层（节点图里有环的话也会停下）、一共最多走多少步、拼出来的文字最多多长。
const MAX_DEPTH: usize = 24;
const MAX_STEPS: usize = 20_000;
const MAX_JOINED: usize = 256 << 10;

impl<'a> Graph<'a> {
    fn new(nodes: &'a Map<String, Value>) -> Self {
        Self {
            nodes,
            steps: Cell::new(0),
        }
    }

    /// 再走一步；步数用完时为假（之后什么都不再找）。
    fn step(&self) -> bool {
        let steps = self.steps.get() + 1;
        self.steps.set(steps);
        steps <= MAX_STEPS
    }

    fn node(&self, id: &str) -> Option<&'a Map<String, Value>> {
        self.nodes.get(id).and_then(Value::as_object)
    }

    fn class(&self, id: &str) -> &'a str {
        self.node(id)
            .and_then(|n| n.get("class_type"))
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    fn inputs(&self, id: &str) -> Option<&'a Map<String, Value>> {
        self.node(id).and_then(|n| n.get("inputs")).and_then(Value::as_object)
    }

    fn input(&self, id: &str, key: &str) -> Option<&'a Value> {
        self.inputs(id).and_then(|i| i.get(key))
    }

    /// 连线指向的节点编号和它的第几个输出。
    fn link(value: &Value) -> Option<(String, u64)> {
        let array = value.as_array()?;
        if array.len() != 2 {
            return None;
        }
        let slot = array[1].as_u64()?;
        let id = match &array[0] {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return None,
        };
        Some((id, slot))
    }

    fn target(value: &Value) -> Option<String> {
        Self::link(value).map(|(id, _)| id)
    }

    /// 一个输入的值：直接写着的，或者顺着连线到一个“值”节点（数字、文字的原语节点等）。
    fn value(&self, id: &str, key: &str, depth: usize) -> Option<String> {
        let value = self.input(id, key)?;
        if let Some(target) = Self::target(value) {
            return (depth < MAX_DEPTH).then(|| self.literal(&target, depth + 1)).flatten();
        }
        scalar(value)
    }

    /// “值”节点的输出：第一个直接写着的简单值（value、text、string、seed 等）。
    fn literal(&self, id: &str, depth: usize) -> Option<String> {
        if !self.step() {
            return None;
        }
        let inputs = self.inputs(id)?;
        for key in ["value", "text", "string", "seed", "noise_seed", "int", "float", "number"] {
            if let Some(v) = inputs.get(key) {
                if let Some(target) = Self::target(v) {
                    if depth < MAX_DEPTH {
                        return self.literal(&target, depth + 1);
                    }
                } else if let Some(s) = scalar(v) {
                    return Some(s);
                }
            }
        }
        None
    }

    /// 顺着条件（conditioning）的连线找到文字编码节点，读出文字（每个节点只看一次，相同的文字只留一份）。
    /// `side` 是从引导器、ControlNet 之类节点往回找时走哪一边（"positive" 或 "negative"）。
    fn texts(&self, id: &str, side: &str, depth: usize, seen: &mut HashSet<String>, out: &mut Vec<String>) {
        if depth > MAX_DEPTH || !seen.insert(id.to_string()) || !self.step() || out.len() >= 64 {
            return;
        }
        let Some(inputs) = self.inputs(id) else { return };
        let class = self.class(id);
        // 置零的条件（Flux 的反向常这样接）：没有文字，不顺着往回找（不然会把正向的文字当成反向的）。
        if class == "ConditioningZeroOut" {
            return;
        }
        if class.contains("TextEncode") {
            let mut memo = HashMap::new();
            for key in ["text", "text_g", "text_l", "t5xxl", "clip_l", "prompt"] {
                let Some(value) = inputs.get(key) else { continue };
                let text = match Self::link(value) {
                    Some((target, slot)) => self.string(&target, slot, depth + 1, &mut memo),
                    None => value.as_str().map(str::to_string),
                };
                if let Some(text) = text.filter(|t| !t.trim().is_empty())
                    && !out.contains(&text)
                {
                    out.push(text);
                }
            }
            return;
        }
        // 把条件改一改再往下传的节点（合并、ControlNet、设区域等）：顺着条件的输入往回找。
        for (key, value) in inputs {
            let follow = key == side || key.starts_with("conditioning");
            if follow && let Some(target) = Self::target(value) {
                self.texts(&target, side, depth + 1, seen, out);
            }
        }
    }

    /// 文字节点的第 `slot` 个输出（文字原语、字符串拼接、提示词模板等）。同一个节点的同一个输出只算一次。
    /// 同时有正向、反向两个文字输入的（如提示词模板）按输出的序号取：0 是正向，1 是反向；有填好的文字
    /// （populated_text）时只用它；拼接节点按它的分隔符连起来。
    fn string(&self, id: &str, slot: u64, depth: usize, memo: &mut HashMap<(String, u64), Option<String>>) -> Option<String> {
        let key = (id.to_string(), slot);
        if let Some(known) = memo.get(&key) {
            return known.clone();
        }
        if depth > MAX_DEPTH || !self.step() {
            return None;
        }
        memo.insert(key.clone(), None);
        let inputs = self.inputs(id)?;
        let textual = |name: &str| {
            ["text", "string", "value", "prompt", "wildcard_text", "populated_text"]
                .iter()
                .any(|k| name == *k || name.starts_with(&format!("{k}_")))
        };
        let mut names: Vec<&String> = inputs.keys().filter(|k| textual(k)).collect();
        if names.iter().any(|k| *k == "populated_text") {
            names.retain(|k| *k == "populated_text");
        }
        let positive: Vec<&String> = names.iter().copied().filter(|k| k.contains("positive")).collect();
        let negative: Vec<&String> = names.iter().copied().filter(|k| k.contains("negative")).collect();
        if !positive.is_empty() && !negative.is_empty() {
            names = if slot == 1 { negative } else { positive };
        }
        let mut parts = Vec::new();
        for name in names {
            let value = &inputs[name.as_str()];
            let part = match Self::link(value) {
                Some((target, slot)) => self.string(&target, slot, depth + 1, memo),
                None => value.as_str().map(str::to_string),
            };
            parts.extend(part);
        }
        let delimiter = inputs.get("delimiter").and_then(Value::as_str).unwrap_or("");
        let mut joined = parts.join(delimiter);
        clip(&mut joined, MAX_JOINED);
        let result = (!joined.is_empty()).then_some(joined);
        memo.insert(key, result.clone());
        result
    }

    /// 模型：顺着 model 的连线找加载模型的节点（路上的 LoRA 也记下来）。
    fn model(&self, id: &str, depth: usize, loras: &mut Vec<String>) -> Option<String> {
        if depth > MAX_DEPTH || !self.step() {
            return None;
        }
        let inputs = self.inputs(id)?;
        for key in ["ckpt_name", "unet_name", "model_name"] {
            if let Some(name) = inputs.get(key).and_then(Value::as_str) {
                return Some(name.to_string());
            }
        }
        if let Some(lora) = inputs.get("lora_name").and_then(Value::as_str) {
            loras.push(lora.to_string());
        }
        let target = Self::target(inputs.get("model")?)?;
        self.model(&target, depth + 1, loras)
    }

    /// 潜空间图像的尺寸（EmptyLatentImage 之类）。
    fn size(&self, id: &str, depth: usize) -> Option<String> {
        if depth > MAX_DEPTH || !self.step() {
            return None;
        }
        let (width, height) = (self.value(id, "width", depth), self.value(id, "height", depth));
        if let (Some(w), Some(h)) = (width, height) {
            return Some(format!("{w}x{h}"));
        }
        let target = Self::target(self.input(id, "samples").or_else(|| self.input(id, "latent"))?)?;
        self.size(&target, depth + 1)
    }

    /// 这个采样器是不是从空白的潜空间图像开始的（从头生成的那一步，不是放大、精修的第二遍）。
    fn starts_from_empty(&self, sampler: &str) -> bool {
        let mut id = match self.input(sampler, "latent_image").and_then(Self::target) {
            Some(id) => id,
            None => return false,
        };
        for _ in 0..MAX_DEPTH {
            if !self.step() {
                return false;
            }
            let class = self.class(&id);
            if class.starts_with("Empty") && class.contains("Latent") {
                return true;
            }
            match self.input(&id, "samples").or_else(|| self.input(&id, "latent")).and_then(Self::target) {
                Some(next) => id = next,
                None => return false,
            }
        }
        false
    }
}

/// 采样器节点：有 latent_image 输入的（KSampler、KSamplerAdvanced、SamplerCustom、SamplerCustomAdvanced）。
/// 主要的那个：从空白潜空间开始的；没有时取 denoise 为 1（或没写）的；都不是时取第一个。
fn comfy(text: &str) -> Option<GenerationInfo> {
    let value: Value = serde_json::from_str(text).ok()?;
    let nodes = value.as_object()?;
    let graph = Graph::new(nodes);
    let mut ids: Vec<&String> = nodes.keys().collect();
    // 按编号排（数字的按数值），结果不随文件里的顺序变。
    ids.sort_by_key(|id| (id.parse::<u64>().unwrap_or(u64::MAX), id.to_string()));
    let samplers: Vec<&String> = ids
        .iter()
        .copied()
        .filter(|id| graph.input(id, "latent_image").is_some())
        .collect();
    let main = samplers
        .iter()
        .copied()
        .find(|id| graph.starts_from_empty(id))
        .or_else(|| {
            samplers.iter().copied().find(|id| {
                graph
                    .value(id, "denoise", 0)
                    .and_then(|d| d.parse::<f64>().ok())
                    .is_none_or(|d| d >= 0.999)
            })
        })
        .or_else(|| samplers.first().copied());
    let (mut prompt, mut negative) = (Vec::new(), Vec::new());
    let mut params = Vec::new();
    if let Some(sampler) = main {
        // SamplerCustomAdvanced：正向、反向和 CFG 在引导器上；种子在噪声节点上；步数、调度器在 sigmas 节点上；采样器另一个节点。
        let guider = graph.input(sampler, "guider").and_then(Graph::target);
        let holder = guider.as_deref().unwrap_or(sampler);
        if let Some(target) = graph.input(holder, "positive").and_then(Graph::target) {
            graph.texts(&target, "positive", 0, &mut HashSet::new(), &mut prompt);
        } else if let Some(target) = graph.input(holder, "conditioning").and_then(Graph::target) {
            graph.texts(&target, "positive", 0, &mut HashSet::new(), &mut prompt);
        }
        if let Some(target) = graph.input(holder, "negative").and_then(Graph::target) {
            graph.texts(&target, "negative", 0, &mut HashSet::new(), &mut negative);
        }
        let linked = |key: &str| graph.input(sampler, key).and_then(Graph::target);
        let mut loras = Vec::new();
        let model = graph
            .input(holder, "model")
            .and_then(Graph::target)
            .and_then(|m| graph.model(&m, 0, &mut loras));
        if let Some(model) = model {
            params.push(("Model".to_string(), model));
        }
        if !loras.is_empty() {
            loras.reverse();
            params.push(("LoRA".to_string(), loras.join(", ")));
        }
        let seed = graph
            .value(sampler, "seed", 0)
            .or_else(|| graph.value(sampler, "noise_seed", 0))
            .or_else(|| linked("noise").and_then(|n| graph.value(&n, "noise_seed", 0)));
        let sigmas = linked("sigmas");
        let steps = graph
            .value(sampler, "steps", 0)
            .or_else(|| sigmas.as_deref().and_then(|s| graph.value(s, "steps", 0)));
        let cfg = graph.value(holder, "cfg", 0);
        let sampler_name = graph
            .value(sampler, "sampler_name", 0)
            .or_else(|| linked("sampler").and_then(|s| graph.value(&s, "sampler_name", 0)));
        let scheduler = graph
            .value(sampler, "scheduler", 0)
            .or_else(|| sigmas.as_deref().and_then(|s| graph.value(s, "scheduler", 0)));
        let denoise = graph.value(sampler, "denoise", 0);
        let size = linked("latent_image").and_then(|l| graph.size(&l, 0));
        for (label, value) in [
            ("Seed", seed),
            ("Steps", steps),
            ("CFG scale", cfg),
            ("Sampler", sampler_name),
            ("Scheduler", scheduler),
            ("Denoise", denoise),
            ("Size", size),
        ] {
            if let Some(value) = value {
                params.push((label.to_string(), value));
            }
        }
    }
    if prompt.is_empty() {
        // 找不到采样器（或连线走不通）：把所有文字编码节点的文字列出来（相同的只留一份）。
        let mut seen = HashSet::new();
        for id in &ids {
            if graph.class(id).contains("TextEncode") {
                graph.texts(id, "positive", 0, &mut seen, &mut prompt);
            }
        }
    }
    if prompt.is_empty() && params.is_empty() {
        return None;
    }
    Some(GenerationInfo {
        tool: "ComfyUI".to_string(),
        prompt: prompt.join("\n\n"),
        negative: negative.join("\n\n"),
        params,
        raw: text.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A1111: &str = "masterpiece, 1girl, \"quoted\", (best quality:1.2)\nsecond line\nNegative prompt: lowres, bad hands\nworst quality\nSteps: 28, Sampler: DPM++ 2M Karras, CFG scale: 7, Seed: 1234567, Size: 512x768, Model hash: abc123, Model: anything-v5, Lora hashes: \"detail: 1f2e, style: 9a8b\", Version: v1.10.1";

    #[test]
    fn reads_a1111_parameters() {
        let info = from_parameters(A1111).unwrap();
        assert_eq!(info.tool, "Stable Diffusion WebUI");
        assert_eq!(info.prompt, "masterpiece, 1girl, \"quoted\", (best quality:1.2)\nsecond line");
        assert_eq!(info.negative, "lowres, bad hands\nworst quality");
        let names: Vec<&str> = info.params.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["Steps", "Sampler", "CFG scale", "Seed", "Size", "Model hash", "Model", "Lora hashes", "Version"]);
        assert_eq!(info.params[1].1, "DPM++ 2M Karras");
        assert_eq!(info.params[7].1, "detail: 1f2e, style: 9a8b", "带引号的值里有逗号、冒号");
        assert_eq!(info.raw, A1111);
    }

    #[test]
    fn a1111_without_negative_or_params() {
        let info = from_parameters("just a prompt, with commas").unwrap();
        assert_eq!((info.prompt.as_str(), info.negative.as_str()), ("just a prompt, with commas", ""));
        assert!(info.params.is_empty(), "不到 3 个“名字: 值”的最后一行是提示词");
        let forge = from_parameters("a cat\nSteps: 20, Sampler: Euler a, CFG scale: 5, Version: f2.0.1v1.10.1").unwrap();
        assert_eq!(forge.tool, "Stable Diffusion WebUI Forge");
        let next = from_parameters("a cat\nSteps: 20, Sampler: Euler a, CFG scale: 5, App: SD.Next").unwrap();
        assert_eq!(next.tool, "SD.Next");
        assert!(from_parameters("  \0").is_none());
    }

    /// ComfyUI：KSampler → 正向、反向文字（经过 ControlNet），模型经过两个 LoRA，尺寸在 EmptyLatentImage，种子在原语节点上。
    #[test]
    fn reads_comfyui_graphs() {
        let graph = r#"{
            "3": {"class_type": "KSampler", "inputs": {"seed": ["10", 0], "steps": 30, "cfg": 6.5, "sampler_name": "dpmpp_2m", "scheduler": "karras", "denoise": 1, "model": ["12", 0], "positive": ["11", 0], "negative": ["7", 0], "latent_image": ["5", 0]}},
            "4": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "sdxl_base.safetensors"}},
            "5": {"class_type": "EmptyLatentImage", "inputs": {"width": 1024, "height": 1216, "batch_size": 1}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "a red fox in snow", "clip": ["4", 1]}},
            "7": {"class_type": "CLIPTextEncode", "inputs": {"text": "blurry", "clip": ["4", 1]}},
            "10": {"class_type": "PrimitiveNode", "inputs": {"value": 42}},
            "11": {"class_type": "ControlNetApply", "inputs": {"conditioning": ["6", 0], "control_net": ["20", 0], "strength": 1}},
            "12": {"class_type": "LoraLoader", "inputs": {"lora_name": "fur.safetensors", "model": ["13", 0], "clip": ["4", 1]}},
            "13": {"class_type": "LoraLoader", "inputs": {"lora_name": "snow.safetensors", "model": ["4", 0], "clip": ["4", 1]}},
            "20": {"class_type": "ControlNetLoader", "inputs": {"control_net_name": "canny"}},
            "30": {"class_type": "KSampler", "inputs": {"seed": 1, "steps": 10, "cfg": 5, "denoise": 0.4, "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["5", 0]}}
        }"#;
        let info = comfy(graph).unwrap();
        assert_eq!(info.tool, "ComfyUI");
        assert_eq!(info.prompt, "a red fox in snow");
        assert_eq!(info.negative, "blurry");
        let get = |k: &str| info.params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("Model"), Some("sdxl_base.safetensors"));
        assert_eq!(get("LoRA"), Some("snow.safetensors, fur.safetensors"));
        assert_eq!((get("Seed"), get("Steps"), get("CFG scale")), (Some("42"), Some("30"), Some("6.5")));
        assert_eq!((get("Sampler"), get("Scheduler"), get("Size")), (Some("dpmpp_2m"), Some("karras"), Some("1024x1216")));
    }

    /// SamplerCustomAdvanced（Flux 常用）：文字在 CLIPTextEncode → FluxGuidance → BasicGuider 上，种子在 RandomNoise，
    /// 步数在 BasicScheduler，采样器在 KSamplerSelect，模型是 UNETLoader。
    #[test]
    fn reads_comfyui_custom_samplers() {
        let graph = r#"{
            "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "flux1-dev.safetensors"}},
            "2": {"class_type": "CLIPTextEncode", "inputs": {"text": ["9", 0], "clip": ["3", 0]}},
            "4": {"class_type": "FluxGuidance", "inputs": {"guidance": 3.5, "conditioning": ["2", 0]}},
            "5": {"class_type": "BasicGuider", "inputs": {"model": ["1", 0], "conditioning": ["4", 0]}},
            "6": {"class_type": "RandomNoise", "inputs": {"noise_seed": 987}},
            "7": {"class_type": "BasicScheduler", "inputs": {"scheduler": "simple", "steps": 20, "denoise": 1, "model": ["1", 0]}},
            "8": {"class_type": "KSamplerSelect", "inputs": {"sampler_name": "euler"}},
            "9": {"class_type": "StringConcatenate", "inputs": {"string_a": "a lighthouse", "string_b": ", at dusk"}},
            "10": {"class_type": "SamplerCustomAdvanced", "inputs": {"noise": ["6", 0], "guider": ["5", 0], "sampler": ["8", 0], "sigmas": ["7", 0], "latent_image": ["11", 0]}},
            "11": {"class_type": "EmptySD3LatentImage", "inputs": {"width": 896, "height": 1152}}
        }"#;
        let info = comfy(graph).unwrap();
        assert_eq!(info.prompt, "a lighthouse, at dusk");
        assert_eq!(info.negative, "");
        let get = |k: &str| info.params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("Model"), Some("flux1-dev.safetensors"));
        assert_eq!((get("Seed"), get("Steps"), get("Sampler"), get("Scheduler")), (Some("987"), Some("20"), Some("euler"), Some("simple")));
        assert_eq!(get("Size"), Some("896x1152"));
        // 没有采样器时把文字编码节点的文字都列出来。
        let loose = comfy(r#"{"5": {"class_type": "CLIPTextEncode", "inputs": {"text": "only text"}}}"#).unwrap();
        assert_eq!(loose.prompt, "only text");
        assert!(comfy("not json").is_none());
        assert!(comfy(r#"{"1": {"class_type": "SaveImage", "inputs": {}}}"#).is_none());
    }

    #[test]
    fn reads_novelai_invoke_swarm_and_fooocus() {
        let comment = r#"{"prompt": "1girl, sunset", "uc": "lowres", "steps": 28, "scale": 5.0, "seed": 77, "sampler": "k_euler_ancestral", "width": 832, "height": 1216}"#;
        let texts = vec![
            ("Software".to_string(), "NovelAI".to_string()),
            ("Comment".to_string(), comment.to_string()),
            ("Source".to_string(), "NovelAI Diffusion V4.5".to_string()),
        ];
        let nai = from_texts(&texts).unwrap();
        assert_eq!((nai.tool.as_str(), nai.prompt.as_str(), nai.negative.as_str()), ("NovelAI", "1girl, sunset", "lowres"));
        assert_eq!(nai.params[0], ("Model".to_string(), "NovelAI Diffusion V4.5".to_string()));
        assert!(nai.params.contains(&("Size".to_string(), "832x1216".to_string())));
        let v4 = novelai(Some(r#"{"v4_prompt": {"caption": {"base_caption": "a fox"}}, "v4_negative_prompt": {"caption": {"base_caption": "bad"}}}"#), None, None).unwrap();
        assert_eq!((v4.prompt.as_str(), v4.negative.as_str()), ("a fox", "bad"));

        let invoke_text = r#"{"positive_prompt": "castle", "negative_prompt": "fog", "seed": 5, "steps": 30, "cfg_scale": 7.5, "scheduler": "euler", "width": 768, "height": 512, "model": {"name": "juggernaut"}}"#;
        let invoke = from_texts(&[("invokeai_metadata".to_string(), invoke_text.to_string())]).unwrap();
        assert_eq!((invoke.tool.as_str(), invoke.prompt.as_str(), invoke.negative.as_str()), ("InvokeAI", "castle", "fog"));
        assert_eq!(invoke.params[0], ("Model".to_string(), "juggernaut".to_string()));
        let v2 = from_texts(&[("sd-metadata".to_string(), r#"{"model_weights": "sd-1.5", "image": {"prompt": [{"prompt": "a boat", "weight": 1}], "seed": 3, "steps": 50}}"#.to_string())]).unwrap();
        assert_eq!(v2.prompt, "a boat");

        let swarm = from_parameters(r#"{"sui_image_params": {"prompt": "a tree", "negativeprompt": "ugly", "model": "sdxl", "seed": 9, "steps": 25}}"#).unwrap();
        assert_eq!((swarm.tool.as_str(), swarm.prompt.as_str(), swarm.negative.as_str()), ("SwarmUI", "a tree", "ugly"));
        assert!(swarm.params.contains(&("seed".to_string(), "9".to_string())));
        let fooocus = from_parameters(r#"{"Prompt": "a bird", "Negative Prompt": "", "Seed": "123", "Version": "Fooocus v2.5.5"}"#).unwrap();
        assert_eq!((fooocus.tool.as_str(), fooocus.prompt.as_str()), ("Fooocus", "a bird"));
    }

    /// 别的程序写在 JPEG 注释段、EXIF 用户注释里的普通文字不当成生成信息（避免将 GIMP 或手机厂商标记误判为提示词）。
    #[test]
    fn ordinary_comments_are_not_generation_info() {
        for text in ["Created with GIMP", "CREATOR: gd-jpeg v1.0 (using IJG JPEG v80), quality = 82", "oplus_32", "Steps: 1"] {
            assert!(from_untagged(text).is_none(), "{text}");
        }
        assert!(from_untagged(samples::A1111).is_some());
        assert!(from_untagged("a cat\nNegative prompt: dog").is_some());
        // PNG 的 parameters 关键字本身就说明是生成参数：只有提示词也认。
        assert!(from_parameters("a cat").is_some());
        // JPEG 的注释段里写着普通文字：不认；写着 A1111 的参数：认。
        let dir = std::env::temp_dir().join(format!("anyview-generation-com-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut image = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3]))
            .write_to(&mut image, image::ImageFormat::Jpeg)
            .unwrap();
        let image = image.into_inner();
        for (name, comment, wanted) in [("gimp.jpg", "Created with GIMP", false), ("a1111.jpg", samples::A1111, true)] {
            let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xFE];
            bytes.extend_from_slice(&((comment.len() + 2) as u16).to_be_bytes());
            bytes.extend_from_slice(comment.as_bytes());
            bytes.extend_from_slice(&image[2..]);
            std::fs::write(dir.join(name), bytes).unwrap();
            assert_eq!(read(&dir.join(name)).is_some(), wanted, "{name}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A1111 的各种写法：`{` 开头的提示词（NovelAI 风格的加权）、参数行后面的 Template 行、带转义的引号值、
    /// 认不出的片段（JSON 写的 Hashes）跳过、接着找后面的参数。
    #[test]
    fn a1111_edge_cases() {
        let braces = from_parameters("{{masterpiece}}, 1girl\nSteps: 20, Sampler: Euler, CFG scale: 7").unwrap();
        assert_eq!(braces.prompt, "{{masterpiece}}, 1girl");
        assert_eq!(braces.params.len(), 3);
        let template = a1111("a cat\nNegative prompt: dog\nSteps: 20, Sampler: Euler, CFG scale: 7\nTemplate: a __animal__\nNegative Template: dog").unwrap();
        assert_eq!((template.prompt.as_str(), template.negative.as_str()), ("a cat", "dog"));
        assert!(template.params.contains(&("Template".to_string(), "a __animal__".to_string())), "{:?}", template.params);
        let params = parse_params(r#"Steps: 20, Lora hashes: "a\"b, c: 1", Hashes: {"vae": "x", "m": "y"}, Sampler: Euler a, CFG scale: 7"#);
        let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("Lora hashes").as_deref(), Some("a\"b, c: 1"), "{params:?}");
        assert_eq!(get("Sampler").as_deref(), Some("Euler a"), "认不出的片段之后接着找：{params:?}");
        assert_eq!(get("CFG scale").as_deref(), Some("7"));
        assert!(parse_params("just, some, words").is_empty());
    }

    /// 恶意的节点图：很多条路通到同一个节点（每个节点 4 个条件输入都连到下一个）、字符串拼接层层翻倍：
    /// 很快就结束，文字有上限（避免按路径重复遍历导致指数级开销）。
    #[test]
    fn hostile_comfy_graphs_stay_bounded() {
        let mut nodes = serde_json::Map::new();
        // 20 层（在层数上限以内）：按路径走是 4^20 步，拼出来的文字按路径翻倍是 2^20 × 1000 个字。
        for i in 0..20 {
            let next = serde_json::json!([format!("{}", i + 1), 0]);
            nodes.insert(format!("{i}"), serde_json::json!({"class_type": "ConditioningCombine", "inputs": {
                "conditioning_1": next, "conditioning_2": next, "conditioning_3": next, "conditioning_4": next}}));
        }
        nodes.insert("20".into(), serde_json::json!({"class_type": "CLIPTextEncode", "inputs": {"text": ["40", 0]}}));
        for i in 40..60 {
            let next = serde_json::json!([format!("{}", i + 1), 0]);
            nodes.insert(format!("{i}"), serde_json::json!({"class_type": "StringConcatenate", "inputs": {"string_a": next, "string_b": next}}));
        }
        nodes.insert("60".into(), serde_json::json!({"class_type": "PrimitiveString", "inputs": {"value": "x".repeat(1000)}}));
        nodes.insert("99".into(), serde_json::json!({"class_type": "KSampler", "inputs": {"positive": ["0", 0], "negative": ["0", 0], "latent_image": ["98", 0]}}));
        let text = serde_json::Value::Object(nodes).to_string();
        let started = std::time::Instant::now();
        let info = comfy(&text).expect("层数以内的能读出来");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        assert!(info.prompt.len() <= MAX_JOINED + 4, "{}", info.prompt.len());
        assert!(info.prompt.starts_with("xxx"));
    }

    /// ComfyUI：提示词模板这类节点的第 0、1 个输出分别是正向、反向；填好的通配符文字优先；拼接按分隔符；
    /// 主要的采样器是从空白潜空间开始的那个（编号更小的精修采样器不算）。
    #[test]
    fn comfy_outputs_and_main_sampler() {
        let graph = r#"{
            "1": {"class_type": "KSamplerAdvanced", "inputs": {"noise_seed": 2, "steps": 10, "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["3", 0], "model": ["4", 0]}},
            "3": {"class_type": "KSampler", "inputs": {"seed": 42, "steps": 30, "cfg": 7, "denoise": 1, "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["5", 0], "model": ["4", 0]}},
            "4": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "base.safetensors"}},
            "5": {"class_type": "EmptyLatentImage", "inputs": {"width": 832, "height": 1216}},
            "6": {"class_type": "CLIPTextEncodeSDXL", "inputs": {"text_g": ["8", 0], "text_l": ["9", 0]}},
            "7": {"class_type": "CLIPTextEncodeSDXL", "inputs": {"text_g": ["8", 1], "text_l": ""}},
            "8": {"class_type": "SDXLPromptStyler", "inputs": {"text_positive": "a castle", "text_negative": "blurry", "style": "base"}},
            "9": {"class_type": "StringConcatenate", "inputs": {"string_a": ["10", 0], "string_b": "at night", "delimiter": ", "}},
            "10": {"class_type": "ImpactWildcardProcessor", "inputs": {"wildcard_text": "__weather__", "populated_text": "foggy"}}
        }"#;
        let info = comfy(graph).unwrap();
        assert_eq!(info.prompt, "a castle\n\nfoggy, at night");
        assert_eq!(info.negative, "blurry");
        let get = |k: &str| info.params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!((get("Seed"), get("Steps"), get("Size")), (Some("42"), Some("30"), Some("832x1216")));
    }

    /// PNG 的文字块写在图像数据之后（个别程序这样写）：文件末尾的也认；EXIF 的用户注释没有编码前缀、写在第一个目录里也认。
    #[test]
    fn late_png_chunks_and_plain_exif_comments() {
        let dir = std::env::temp_dir().join(format!("anyview-generation-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = samples::png(&[]);
        let iend = png.len() - 12;
        let chunk = samples::text("parameters", samples::A1111);
        png.splice(iend..iend, chunk);
        std::fs::write(dir.join("late.png"), &png).unwrap();
        assert_eq!(read(&dir.join("late.png")).unwrap().prompt, samples::PROMPT);
        std::fs::remove_dir_all(&dir).ok();
        // IFD0 里一项 ASCII 的用户注释（Fooocus 的写法），没有 8 个字节的编码前缀。
        let text = samples::A1111.as_bytes();
        let mut tiff = b"II*\0\x08\0\0\0\x01\0".to_vec();
        tiff.extend_from_slice(&0x9286u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&(text.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&26u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(text);
        let comment = crate::exif::Exif::parse(&tiff).unwrap().user_comment().unwrap();
        assert_eq!(comment, samples::A1111);
    }

    /// 从真的文件里读：PNG 的 tEXt、zTXt、iTXt；JPEG、WebP 的 EXIF 用户注释（UTF-16，两种字节序）。
    #[test]
    fn reads_files() {
        let dir = std::env::temp_dir().join(format!("anyview-generation-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, data) in samples::all() {
            std::fs::write(dir.join(name), data).unwrap();
        }
        for name in ["text.png", "ztxt.png", "itxt.png"] {
            let info = read(&dir.join(name)).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(info.prompt, samples::PROMPT, "{name}");
            assert_eq!(info.negative, "lowres", "{name}");
        }
        let comfy = read(&dir.join("comfy.png")).unwrap();
        assert_eq!((comfy.tool.as_str(), comfy.prompt.as_str()), ("ComfyUI", "a red fox in snow"));
        for name in ["exif_be.jpg", "exif_le.jpg", "exif.webp"] {
            let info = read(&dir.join(name)).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(info.prompt, samples::PROMPT, "{name}");
        }
        assert!(read(&dir.join("plain.png")).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// 测试用的小文件：1x1 的 PNG、JPEG、WebP，各带一种写法的生成信息（核心的单元测试、anyview-fixtures 的测试图片共用）。
#[doc(hidden)]
pub mod samples {
    use std::io::Write;

    pub const PROMPT: &str = "a cozy cabin, warm light, 雪";
    pub const A1111: &str = "a cozy cabin, warm light, 雪\nNegative prompt: lowres\nSteps: 20, Sampler: Euler a, CFG scale: 7, Seed: 42, Size: 512x512, Model: test-model";
    const COMFY: &str = r#"{"3": {"class_type": "KSampler", "inputs": {"seed": 42, "steps": 20, "cfg": 7, "sampler_name": "euler", "scheduler": "normal", "denoise": 1, "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["5", 0]}}, "4": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "v1-5.safetensors"}}, "5": {"class_type": "EmptyLatentImage", "inputs": {"width": 512, "height": 512, "batch_size": 1}}, "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "a red fox in snow", "clip": ["4", 1]}}, "7": {"class_type": "CLIPTextEncode", "inputs": {"text": "blurry", "clip": ["4", 1]}}}"#;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
        out
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    /// 1x1 的灰色 PNG，图像数据之前插进这些块。
    pub fn png(extra: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]));
        for c in extra {
            out.extend_from_slice(c);
        }
        out.extend(chunk(b"IDAT", &zlib(&[0, 128])));
        out.extend(chunk(b"IEND", &[]));
        out
    }

    pub fn text(keyword: &str, value: &str) -> Vec<u8> {
        chunk(b"tEXt", &[keyword.as_bytes(), &[0], value.as_bytes()].concat())
    }

    fn ztxt(keyword: &str, value: &str) -> Vec<u8> {
        chunk(b"zTXt", &[keyword.as_bytes(), &[0, 0], &zlib(value.as_bytes())].concat())
    }

    fn itxt(keyword: &str, value: &str) -> Vec<u8> {
        chunk(b"iTXt", &[keyword.as_bytes(), &[0, 1, 0, 0, 0], &zlib(value.as_bytes())].concat())
    }

    /// EXIF（TIFF）：IFD0 只有 Exif 目录的指针，Exif 目录只有用户注释（UNICODE，UTF-16）。
    fn exif(little: bool, comment: &str) -> Vec<u8> {
        let u16b = |v: u16| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = |v: u32| if little { v.to_le_bytes() } else { v.to_be_bytes() };
        let mut body = b"UNICODE\0".to_vec();
        for unit in comment.encode_utf16() {
            body.extend_from_slice(&u16b(unit));
        }
        let mut out = Vec::new();
        out.extend_from_slice(if little { b"II*\0" } else { b"MM\0*" });
        out.extend_from_slice(&u32b(8));
        // IFD0（8）：1 项，Exif 目录在 26。
        out.extend_from_slice(&u16b(1));
        out.extend_from_slice(&u16b(0x8769));
        out.extend_from_slice(&u16b(4));
        out.extend_from_slice(&u32b(1));
        out.extend_from_slice(&u32b(26));
        out.extend_from_slice(&u32b(0));
        // Exif 目录（26）：1 项，用户注释在 44。
        out.extend_from_slice(&u16b(1));
        out.extend_from_slice(&u16b(0x9286));
        out.extend_from_slice(&u16b(7));
        out.extend_from_slice(&u32b(body.len() as u32));
        out.extend_from_slice(&u32b(44));
        out.extend_from_slice(&u32b(0));
        out.extend_from_slice(&body);
        out
    }

    /// 1x1 的 JPEG，开头的 APP1 是带用户注释的 EXIF。
    fn jpeg(little: bool) -> Vec<u8> {
        let mut image = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(1, 1, image::Rgb([128, 128, 128]))
            .write_to(&mut image, image::ImageFormat::Jpeg)
            .unwrap();
        let image = image.into_inner();
        let payload = [b"Exif\0\0".as_slice(), &exif(little, A1111)].concat();
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&image[2..]);
        out
    }

    /// 1x1 的 WebP（无损），加上 EXIF 块（扩展格式 VP8X）。
    fn webp() -> Vec<u8> {
        let mut image = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(1, 1, image::Rgba([128, 128, 128, 255]))
            .write_to(&mut image, image::ImageFormat::WebP)
            .unwrap();
        let image = image.into_inner();
        let vp8l = &image[12..];
        let exif = exif(true, A1111);
        let mut chunks = Vec::new();
        let mut vp8x = vec![0u8; 10];
        vp8x[0] = 0x08; // 有 EXIF
        chunks.extend_from_slice(b"VP8X");
        chunks.extend_from_slice(&10u32.to_le_bytes());
        chunks.extend_from_slice(&vp8x);
        chunks.extend_from_slice(vp8l);
        chunks.extend_from_slice(b"EXIF");
        chunks.extend_from_slice(&(exif.len() as u32).to_le_bytes());
        chunks.extend_from_slice(&exif);
        if exif.len() % 2 == 1 {
            chunks.push(0);
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&((chunks.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(&chunks);
        out
    }

    /// 文件名 → 内容。
    pub fn all() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("text.png", png(&[text("parameters", A1111)])),
            ("ztxt.png", png(&[ztxt("parameters", A1111)])),
            ("itxt.png", png(&[itxt("parameters", A1111)])),
            ("comfy.png", png(&[text("prompt", COMFY), text("workflow", "{}")])),
            ("exif_be.jpg", jpeg(false)),
            ("exif_le.jpg", jpeg(true)),
            ("exif.webp", webp()),
            ("plain.png", png(&[text("Software", "test")])),
        ]
    }
}
