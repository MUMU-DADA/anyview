//! 装进系统的文字：Windows 安装程序里自己写的几句、macOS 访达“种类”一栏的文件类型名和两个访达扩展的名字、
//! Linux 给系统补的文件类型的说明。它们不在程序运行时显示，而是写进各系统的文件里；和界面文字一样，中英文写在这里，
//! 别的语言在 `locales/`（同一张总表、同样翻译和检查）。下面的测试按这里生成这些文件，并核对它们是最新的：
//!
//! - `tools/windows/installer/languages.nsh`：安装程序的 19 种语言（向导本身的按钮、标题用 NSIS 自带的译文）；
//! - `apps/macos/Resources/Localized/<程序包>/<语言>.lproj/InfoPlist.strings`：编译测试.command 复制进程序和两个扩展；
//! - `apps/linux/data/mime/local.anyview.Anyview.xml` 里各语言的 `<comment>`；
//! - `tools/windows/portable.txt`：Windows 免安装版里的说明，19 种语言各一段。
//!
//! 改了文字以后：`ANYVIEW_UPDATE_MESSAGES=1 cargo test -p anyview-core` 重新生成总表和这些文件；补齐翻译后再跑一次。

use crate::i18n::Lang;

/// Windows 安装程序（`tools/windows/installer/anyview.nsi`）里自己写的文字：(NSIS 里的名字, 文字)。
/// `{version}` 换成版本号，`{folder}` 换成设置所在的文件夹；欢迎页的最后接 NSIS 自带的“点击‘下一步’继续”。
pub fn installer_texts(lang: Lang) -> [(&'static str, &'static str); 10] {
    [
        (
            "TextWelcome",
            tr!(
                lang,
                "这个向导会把 Anyview {version} 装到这台电脑上：只装给你这个用户，不需要管理员权限。",
                "This wizard installs Anyview {version} for your user account. No administrator rights are needed."
            ),
        ),
        (
            "TextFinish",
            tr!(
                lang,
                "Anyview 已经装好。\n\nWindows 不允许程序自己成为默认程序：勾上下面第二项，会打开系统的“默认应用”设置，在“照片查看器”里选 Anyview 就行。",
                "Anyview has been installed.\n\nWindows doesn’t let a program make itself the default: tick the second box and Settings opens on Default apps, where you pick Anyview under Photo viewer."
            ),
        ),
        ("TextRun", tr!(lang, "运行 Anyview", "Run Anyview")),
        (
            "TextDefaultApps",
            tr!(lang, "设为默认看图软件（打开系统设置）", "Make Anyview the default viewer (opens Settings)"),
        ),
        ("TextMain", tr!(lang, "Anyview（必需）", "Anyview (required)")),
        ("TextStartMenu", tr!(lang, "“开始”菜单快捷方式", "Start menu shortcut")),
        ("TextDesktop", tr!(lang, "桌面快捷方式", "Desktop shortcut")),
        (
            "TextRunning",
            tr!(lang, "Anyview 正在运行。请先关闭它，再点“重试”。", "Anyview is running. Close it, then click Retry."),
        ),
        (
            "TextRegisterFailed",
            tr!(
                lang,
                "文件已经装好，但没能把 Anyview 登记到 Windows（“打开方式”、缩略图等）。仍然可以从它的文件夹里直接运行。",
                "The files were installed, but Anyview could not be registered with Windows (Open with, thumbnails). You can still run it from its folder."
            ),
        ),
        (
            "TextPurge",
            tr!(
                lang,
                "同时删除 Anyview 的设置和记住的位置吗？\n（{folder}）",
                "Also delete Anyview’s settings and remembered positions?\n({folder})"
            ),
        ),
    ]
}

/// Windows 免安装版放在程序旁边的 `portable.txt` 里的说明：这个文件在，设置等就存在旁边的 data 文件夹里。
pub fn portable_note(lang: Lang) -> &'static str {
    tr!(
        lang,
        "Anyview 免安装版：这个文件在的时候，设置、最近打开的文件和压缩包里看到的位置都存在旁边的 data 文件夹里，不写进系统。不要了就把整个文件夹删掉，什么都不会留下。删掉这个文件，Anyview 就改用系统里你的用户文件夹。",
        "Anyview portable: while this file is here, settings, recent files and archive positions are kept in the data folder next to it, and nothing is written to the system. To remove Anyview, delete the whole folder; nothing is left behind. If you delete this file, Anyview uses your user folder instead."
    )
}

/// NSIS 里这种语言的名字（`Contrib/Language files` 里的文件名；`${LANG_<大写>}` 是它的编号）。
pub fn nsis_language(lang: Lang) -> &'static str {
    match lang {
        Lang::ZhHans => "SimpChinese",
        Lang::En => "English",
        Lang::ZhHant => "TradChinese",
        Lang::Ja => "Japanese",
        Lang::Ko => "Korean",
        Lang::Es => "Spanish",
        Lang::Fr => "French",
        Lang::De => "German",
        Lang::PtBr => "PortugueseBR",
        Lang::Ru => "Russian",
        Lang::It => "Italian",
        Lang::Tr => "Turkish",
        Lang::Pl => "Polish",
        Lang::Nl => "Dutch",
        Lang::Vi => "Vietnamese",
        Lang::Id => "Indonesian",
        Lang::Th => "Thai",
        Lang::Uk => "Ukrainian",
        Lang::Hi => "Hindi",
    }
}

/// macOS 访达“种类”一栏显示的文件类型名：Info.plist 里各个 CFBundleTypeName、UTTypeDescription，按出现的先后。
/// Info.plist 里写的是英文（不改它：类型调查按 "Image File" 找文档类型），各语言的 InfoPlist.strings 以英文原文为键。
pub fn mac_type_names(lang: Lang) -> [&'static str; 15] {
    [
        tr!(lang, "图片", "Image"),
        tr!(lang, "图片文件", "Image File"),
        tr!(lang, "漫画压缩包", "Comic Archive"),
        tr!(lang, "QOI 图像", "QOI Image"),
        tr!(lang, "Clip Studio Paint 插画", "Clip Studio Paint Illustration"),
        tr!(lang, "JPEG XR 图像", "JPEG XR Image"),
        tr!(lang, "JFIF 图像", "JFIF Image"),
        tr!(lang, "APNG 动画图像", "Animated PNG Image"),
        tr!(lang, "Portable Anymap 图像", "Portable Anymap Image"),
        tr!(lang, "Windows 光标", "Windows Cursor"),
        tr!(lang, "PCX 图像", "PCX Image"),
        tr!(lang, "JPEG 2000 码流", "JPEG 2000 Codestream"),
        tr!(lang, "AVIF 图像序列", "AVIF Image Sequence"),
        tr!(lang, "柯达 KDC RAW 图像", "Kodak KDC Raw Image"),
        tr!(lang, "适马 X3F RAW 图像", "Sigma X3F Raw Image"),
    ]
}

/// macOS 两个访达扩展在“系统设置”的扩展列表里显示的名字（CFBundleDisplayName）：缩略图、预览。
pub fn mac_extension_names(lang: Lang) -> [&'static str; 2] {
    [
        tr!(lang, "Anyview 缩略图", "Anyview Thumbnails"),
        tr!(lang, "Anyview 预览", "Anyview Preview"),
    ]
}

/// Linux 给系统补的两种文件类型（`apps/linux/data/mime/local.anyview.Anyview.xml`）的说明：(MIME 类型, 说明)。
pub fn mime_comments(lang: Lang) -> [(&'static str, &'static str); 2] {
    [
        ("image/x-portable-arbitrarymap", tr!(lang, "PAM 图像", "PAM image")),
        (
            "image/x-clip-studio-paint",
            tr!(lang, "Clip Studio Paint 插画", "Clip Studio Paint illustration"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::ALL_LANGS;
    use std::path::{Path, PathBuf};

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// 生成的文件：设了 ANYVIEW_UPDATE_MESSAGES 时写进去；然后核对和按代码生成的一样。
    fn check_generated(path: &Path, text: &str, problems: &mut Vec<String>) {
        if std::env::var_os("ANYVIEW_UPDATE_MESSAGES").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        if std::fs::read_to_string(path).unwrap_or_default() != text {
            problems.push(path.display().to_string());
        }
    }

    fn report(problems: Vec<String>) {
        assert!(
            problems.is_empty(),
            "这些文件过时了：用 ANYVIEW_UPDATE_MESSAGES=1 cargo test -p anyview-core 重新生成：{problems:#?}"
        );
    }

    /// 英文在最前（系统语言对不上时用第一个），其余按设置里的顺序。
    fn installer_order() -> Vec<Lang> {
        std::iter::once(Lang::En)
            .chain(ALL_LANGS.iter().copied().filter(|&l| l != Lang::En))
            .collect()
    }

    /// NSIS 字符串里的写法：`$` 写成 `$$`，引号 `$\"`，换行 `$\r$\n`；占位符换成 NSIS 的。
    fn nsis_text(name: &str, text: &str) -> String {
        let mut out = String::new();
        for c in text.chars() {
            match c {
                '$' => out.push_str("$$"),
                '"' => out.push_str("$\\\""),
                '\n' => out.push_str("$\\r$\\n"),
                c => out.push(c),
            }
        }
        let mut out = out
            .replace("{version}", "${APP_VERSION}")
            .replace("{folder}", "$APPDATA\\Anyview");
        if name == "TextWelcome" {
            out.push_str("$\\r$\\n$\\r$\\n$_CLICK");
        }
        out
    }

    fn installer_languages_nsh() -> String {
        let mut out = String::from(
            "\u{feff}; Windows 安装程序的语言：由核心的测试按 crates/anyview-core/src/packaging.rs 生成，不要手改\n\
             ; （ANYVIEW_UPDATE_MESSAGES=1 cargo test -p anyview-core）。向导本身的按钮、标题用 NSIS 自带的译文。\n\
             ; 第一个（英文）是系统语言对不上时用的。\n",
        );
        let order = installer_order();
        for &lang in &order {
            out.push_str(&format!("!insertmacro MUI_LANGUAGE \"{}\"\n", nsis_language(lang)));
        }
        for (index, (name, _)) in installer_texts(Lang::En).into_iter().enumerate() {
            out.push('\n');
            for &lang in &order {
                let text = installer_texts(lang)[index].1;
                out.push_str(&format!(
                    "LangString {name} ${{LANG_{}}} \"{}\"\n",
                    nsis_language(lang).to_uppercase(),
                    nsis_text(name, text)
                ));
            }
        }
        out
    }

    #[test]
    fn installer_languages_file_is_current() {
        let mut problems = Vec::new();
        let path = repo().join("tools/windows/installer/languages.nsh");
        check_generated(&path, &installer_languages_nsh(), &mut problems);
        report(problems);
    }

    /// 免安装版的说明：英文在最前，其余按设置里的顺序，每种语言一段；UTF-8 带 BOM、Windows 的换行（记事本）。
    fn portable_txt() -> String {
        let mut out = String::from("\u{feff}");
        for (i, &lang) in installer_order().iter().enumerate() {
            if i > 0 {
                out.push_str("\r\n");
            }
            out.push_str(portable_note(lang));
            out.push_str("\r\n");
        }
        out
    }

    #[test]
    fn portable_note_file_is_current() {
        let mut problems = Vec::new();
        check_generated(&repo().join("tools/windows/portable.txt"), &portable_txt(), &mut problems);
        report(problems);
    }

    #[test]
    fn installer_uses_every_text_and_the_language_file() {
        let script = std::fs::read_to_string(repo().join("tools/windows/installer/anyview.nsi")).unwrap();
        assert!(script.contains("!include \"languages.nsh\""), "anyview.nsi 要用生成的语言文件");
        assert!(!script.contains("LangString "), "安装程序的文字只写在 packaging.rs 里");
        for (name, _) in installer_texts(Lang::En) {
            assert!(script.contains(&format!("$({name})")), "anyview.nsi 没用到 {name}");
        }
    }

    #[test]
    fn installer_text_escaping() {
        assert_eq!(nsis_text("TextPurge", "a\n({folder})"), "a$\\r$\\n($APPDATA\\Anyview)");
        assert_eq!(nsis_text("TextRun", "\"$5\""), "$\\\"$$5$\\\"");
        assert!(nsis_text("TextWelcome", "Anyview {version}").ends_with("${APP_VERSION}$\\r$\\n$\\r$\\n$_CLICK"));
    }

    /// .strings 文件里的字符串写法。
    fn strings_literal(text: &str) -> String {
        let mut out = String::from("\"");
        for c in text.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    const MAC_BUNDLES: [&str; 3] = ["Anyview", "AnyviewThumbnail", "AnyviewPreview"];

    /// 某个程序包里这种语言的 InfoPlist.strings。
    fn mac_info_plist_strings(bundle: &str, lang: Lang) -> String {
        let pairs: Vec<(&str, &str)> = match bundle {
            "Anyview" => mac_type_names(Lang::En).into_iter().zip(mac_type_names(lang)).collect(),
            "AnyviewThumbnail" => vec![("CFBundleDisplayName", mac_extension_names(lang)[0])],
            _ => vec![("CFBundleDisplayName", mac_extension_names(lang)[1])],
        };
        let mut out = String::from(
            "/* 由核心的测试按 crates/anyview-core/src/packaging.rs 生成，不要手改。 */\n",
        );
        for (key, value) in pairs {
            out.push_str(&format!("{} = {};\n", strings_literal(key), strings_literal(value)));
        }
        out
    }

    #[test]
    fn mac_strings_files_are_current() {
        let base = repo().join("apps/macos/Resources/Localized");
        let mut problems = Vec::new();
        for bundle in MAC_BUNDLES {
            let mut expected = Vec::new();
            for &lang in ALL_LANGS.iter().filter(|&&l| l != Lang::En) {
                let dir = format!("{}.lproj", lang.code());
                let path = base.join(bundle).join(&dir).join("InfoPlist.strings");
                check_generated(&path, &mac_info_plist_strings(bundle, lang), &mut problems);
                expected.push(dir);
            }
            // 没有多出来的语言（去掉的语言要连文件夹一起删）。
            let Ok(entries) = std::fs::read_dir(base.join(bundle)) else { continue };
            for entry in entries {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                if !expected.contains(&name) {
                    problems.push(format!("多出来的 {bundle}/{name}"));
                }
            }
        }
        report(problems);
    }

    /// Info.plist 里某个键后面的那些字符串值。
    fn plist_values(plist: &str, key: &str) -> Vec<String> {
        let marker = format!("<key>{key}</key>");
        plist
            .split(&marker)
            .skip(1)
            .filter_map(|rest| {
                let rest = rest.trim_start().strip_prefix("<string>")?;
                Some(rest[..rest.find("</string>")?].to_string())
            })
            .collect()
    }

    #[test]
    fn mac_type_names_match_info_plist() {
        let plist = std::fs::read_to_string(repo().join("apps/macos/Resources/Info.plist")).unwrap();
        let mut names = plist_values(&plist, "CFBundleTypeName");
        names.extend(plist_values(&plist, "UTTypeDescription"));
        names.sort();
        let mut ours: Vec<String> = mac_type_names(Lang::En).iter().map(|s| s.to_string()).collect();
        ours.sort();
        assert_eq!(names, ours, "Info.plist 的类型名改了：同步改 packaging.rs 的 mac_type_names");
    }

    #[test]
    fn mac_extension_plists_use_the_english_names() {
        for (file, name) in ["ThumbnailInfo.plist", "PreviewInfo.plist"].into_iter().zip(mac_extension_names(Lang::En)) {
            let plist = std::fs::read_to_string(repo().join("apps/macos/Resources").join(file)).unwrap();
            assert_eq!(plist_values(&plist, "CFBundleDisplayName"), [name], "{file}");
            assert_eq!(plist_values(&plist, "CFBundleDevelopmentRegion"), ["en"], "{file}");
        }
    }

    fn xml_text(text: &str) -> String {
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    /// 文件类型定义：每种类型下面的 `<comment>` 换成英文一行、各语言（按 Linux 的语言写法）各一行，别的照旧。
    fn mime_xml(current: &str) -> String {
        let mut out = String::new();
        for line in current.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("<comment") {
                continue;
            }
            out.push_str(line);
            out.push('\n');
            let Some(rest) = trimmed.strip_prefix("<mime-type type=\"") else { continue };
            let mime = &rest[..rest.find('"').unwrap()];
            let index = mime_comments(Lang::En).iter().position(|(m, _)| *m == mime);
            let index = index.unwrap_or_else(|| panic!("packaging.rs 的 mime_comments 里没有 {mime}"));
            let indent = format!("{}  ", &line[..line.len() - trimmed.len()]);
            out.push_str(&format!("{indent}<comment>{}</comment>\n", xml_text(mime_comments(Lang::En)[index].1)));
            for &lang in ALL_LANGS.iter().filter(|&&l| l != Lang::En) {
                for locale in lang.posix_locales() {
                    out.push_str(&format!(
                        "{indent}<comment xml:lang=\"{locale}\">{}</comment>\n",
                        xml_text(mime_comments(lang)[index].1)
                    ));
                }
            }
        }
        out
    }

    #[test]
    fn mime_comments_are_current() {
        let path = repo().join("apps/linux/data/mime/local.anyview.Anyview.xml");
        let current = std::fs::read_to_string(&path).unwrap();
        let mut problems = Vec::new();
        check_generated(&path, &mime_xml(&current), &mut problems);
        report(problems);
        // 每种类型都有说明。
        for (mime, _) in mime_comments(Lang::En) {
            assert!(current.contains(&format!("<mime-type type=\"{mime}\">")), "{mime}");
        }
    }
}
