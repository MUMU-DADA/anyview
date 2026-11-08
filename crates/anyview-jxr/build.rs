//! 从源代码编译 jxrlib（third_party/jxrlib）和本 crate 的 C 外壳 src/shim.c。
//! 文件列表来自 jxrlib 自己的 Makefile（SRC_SYS、SRC_DEC、SRC_ENC、SRC_GLUE；编码器也要，胶水层引用了它）。
//! 用 jxrlib 的跨平台写法（`__ANSI__`）。Windows（mingw 交叉编译）也编译：系统的 WIC 解不了的 JPEG XR 由它来解
//! （Windows 10 上 WIC 两张测试图都解不了）。jxrlib 见到 WIN32 这个宏就走 MSVC 专用的写法，
//! 这里去掉它（mingw 另外定义的 _WIN32 照旧，系统头文件要用），其余的差别在 src/compat.h 里补。
//! 万一某台机器上编译不过（如新版的编译器又多了一种报错），不让整个程序编不出来：
//! 打一条警告，JPEG XR 在那台机器上报"不支持"（cfg `jxr_unavailable`）。
use std::path::PathBuf;

const SOURCES: &[&str] = &[
    "image/sys/adapthuff.c",
    "image/sys/image.c",
    "image/sys/strcodec.c",
    "image/sys/strPredQuant.c",
    "image/sys/strTransform.c",
    "image/sys/perfTimerANSI.c",
    "image/decode/decode.c",
    "image/decode/postprocess.c",
    "image/decode/segdec.c",
    "image/decode/strdec.c",
    "image/decode/strInvTransform.c",
    "image/decode/strPredQuantDec.c",
    "image/decode/JXRTranscode.c",
    "image/encode/encode.c",
    "image/encode/segenc.c",
    "image/encode/strenc.c",
    "image/encode/strFwdTransform.c",
    "image/encode/strPredQuantEnc.c",
    "jxrgluelib/JXRGlue.c",
    "jxrgluelib/JXRMeta.c",
    "jxrgluelib/JXRGluePFC.c",
    "jxrgluelib/JXRGlueJxr.c",
];

fn main() {
    println!("cargo:rerun-if-changed=src/shim.c");
    println!("cargo::rustc-check-cfg=cfg(jxr_unavailable)");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/jxrlib");
    println!("cargo:rerun-if-changed={}", root.display());
    let compat = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/compat.h");
    println!("cargo:rerun-if-changed={}", compat.display());
    let mut build = cc::Build::new();
    build
        // 补上游缺的声明（见 src/compat.h），不改上游的代码。
        .flag("-include")
        .flag(compat.to_str().expect("路径不是 UTF-8"))
        .files(SOURCES.iter().map(|s| root.join(s)))
        .file("src/shim.c")
        .include(&root)
        .include(root.join("common/include"))
        .include(root.join("image/sys"))
        .include(root.join("jxrgluelib"))
        .define("__ANSI__", None)
        .define("DISABLE_PERF_MEASUREMENT", None)
        // 上游的代码警告很多（老式写法），不是我们改得了的。
        .warnings(false)
        .flag_if_supported("-w")
        .opt_level(2);
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        build.flag("-UWIN32");
    }
    if let Err(error) = build.try_compile("anyview_jxr") {
        println!("cargo:warning=jxrlib 编译不过，这台机器上的 JPEG XR 不可用：{error}");
        println!("cargo:rustc-cfg=jxr_unavailable");
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=m");
    }
}
