//! 开源许可：Anyview 自己的 MIT 许可证，和这个系统的程序带的第三方软件的许可证原文。
//! 第三方的清单由 `tools/licenses/generate.sh` 生成（cargo-about 列出编进程序的 Rust 库，再加上 C、C++ 写的库
//! 和各系统随程序带的运行库），放在 `crates/anyview-core/licenses/<系统>.txt`；这里按编译的目标系统编进程序，
//! 帮助 › 开源许可（Linux、Windows 的"关于"里也有按钮）显示。安装包里另带 LICENSE.txt、THIRD-PARTY-NOTICES.txt。

use crate::i18n::Lang;

/// Anyview 自己的许可证（仓库根目录的 LICENSE）。
pub const LICENSE: &str = include_str!("../../../LICENSE");

/// 这个系统的程序带的第三方软件的许可证（英文原文）。
#[cfg(target_os = "windows")]
pub const THIRD_PARTY: &str = include_str!("../licenses/windows.txt");
#[cfg(target_os = "macos")]
pub const THIRD_PARTY: &str = include_str!("../licenses/macos.txt");
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const THIRD_PARTY: &str = include_str!("../licenses/linux.txt");

/// "关于"里程序名、版本下面的一行。
pub fn about_line(lang: Lang) -> &'static str {
    tr!(lang, "使用 MIT 许可证", "Released under the MIT License")
}

/// "开源许可"窗口开头的说明。
pub fn intro(lang: Lang) -> &'static str {
    tr!(
        lang,
        "Anyview 使用 MIT 许可证，可以自由使用、修改和分享。它包含的其他开源软件保留各自的许可证，下面是这些许可证的原文。",
        "Anyview is released under the MIT License: you may use, change and share it freely. The software from other projects that it includes keeps its own license; the original texts follow."
    )
}

/// "开源许可"窗口的全文：说明、Anyview 的许可证、第三方软件的许可证。
pub fn text(lang: Lang) -> String {
    format!(
        "{}\n\n{}\n\n{}\n",
        intro(lang),
        LICENSE.trim_end(),
        THIRD_PARTY.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [(&str, &str); 3] = [
        ("windows", include_str!("../licenses/windows.txt")),
        ("macos", include_str!("../licenses/macos.txt")),
        ("linux", include_str!("../licenses/linux.txt")),
    ];

    /// Cargo.lock 的 FNV-1a 64 位指纹（换行统一成 \n），算法与 tools/licenses/generate.py 相同。
    fn fingerprint(data: &[u8]) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut previous_cr = false;
        for &byte in data {
            // \r\n 当成 \n：先不算 \r，看下一个字节是不是 \n。
            if previous_cr && byte != b'\n' {
                h ^= u64::from(b'\r');
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            previous_cr = byte == b'\r';
            if previous_cr {
                continue;
            }
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        if previous_cr {
            h ^= u64::from(b'\r');
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }

    #[test]
    fn fingerprint_matches_the_generator() {
        // generate.py 对同样的字节算出的值（空串、"a"、带 \r\n 的和只带 \n 的相同）。
        assert_eq!(fingerprint(b""), "cbf29ce484222325");
        assert_eq!(fingerprint(b"a"), "af63dc4c8601ec8c");
        assert_eq!(fingerprint(b"x\r\ny\n"), fingerprint(b"x\ny\n"));
        assert_ne!(fingerprint(b"x\ry"), fingerprint(b"xy"));
    }

    #[test]
    fn notices_are_generated_from_the_current_cargo_lock() {
        let lock = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
            .expect("读 Cargo.lock");
        let line = format!("Cargo.lock fingerprint: {}", fingerprint(&lock));
        for (name, text) in ALL {
            assert!(
                text.contains(&line),
                "crates/anyview-core/licenses/{name}.txt 不是按现在的 Cargo.lock 生成的，重新生成：\
                 bash tools/licenses/generate.sh"
            );
        }
    }

    #[test]
    fn notices_list_what_each_program_includes() {
        assert!(LICENSE.starts_with("MIT License"));
        for (name, text) in ALL {
            for want in [
                "LibRaw",
                "jxrlib",
                "UnRAR",
                "Rust standard library",
                "Apache License",
                "MIT License",
                "zip ",
                "unrar_sys ",
            ] {
                assert!(text.contains(want), "{name}.txt 里没有 {want}");
            }
        }
        let windows = ALL[0].1;
        for want in [
            "libheif",
            "MinGW-w64 runtime",
            "Windows App SDK",
            "WebView2",
            "Win2D",
            ".NET",
            "uniffi-bindgen-cs",
            "NSIS",
        ] {
            assert!(windows.contains(want), "windows.txt 里没有 {want}");
        }
        assert!(ALL[1].1.contains("UniFFI"));
        assert!(ALL[2].1.contains("GTK 4"));
        // 别的系统的东西不混进来。
        assert!(!ALL[1].1.contains("Windows App SDK") && !ALL[2].1.contains("Windows App SDK"));
    }

    #[test]
    fn window_text_starts_with_the_intro_and_anyview_license() {
        let text = text(Lang::ZhHans);
        assert!(text.starts_with(intro(Lang::ZhHans)));
        let anyview = text.find("Copyright (c) 2026 Anyview contributors").unwrap();
        let third_party = text.find("third-party software notices").unwrap();
        assert!(anyview < third_party);
        assert!(super::text(Lang::En).starts_with("Anyview is released under the MIT License"));
    }
}
