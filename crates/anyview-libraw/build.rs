//! 从源代码编译 LibRaw（third_party/libraw）和本 crate 的 C++ 外壳 src/shim.cpp。
//! 文件列表来自 LibRaw 自己的 Makefile.dist 的 LIB_OBJECTS（不含几个占位文件）。
//! 不用 OpenMP、zlib、libjpeg、RawSpeed、DNG SDK：极少数用 deflate 或有损 JPEG 压缩的 DNG 解不了，仍可显示内嵌预览。
use std::path::PathBuf;

const SOURCES: &[&str] = &[
    "src/libraw_datastream.cpp",
    "src/libraw_c_api.cpp",
    "src/tables/cameralist.cpp",
    "src/decoders/fuji_compressed.cpp",
    "src/decoders/crx.cpp",
    "src/decoders/pana8.cpp",
    "src/decoders/fp_dng.cpp",
    "src/decoders/decoders_libraw.cpp",
    "src/decoders/olympus14.cpp",
    "src/decoders/sonycc.cpp",
    "src/decoders/sony_arw6.cpp",
    "src/decompressors/losslessjpeg.cpp",
    "src/decoders/unpack.cpp",
    "src/decoders/unpack_thumb.cpp",
    "src/integration/rawspeed_glue.cpp",
    "src/integration/dngsdk_glue.cpp",
    "src/tables/colorconst.cpp",
    "src/utils/utils_libraw.cpp",
    "src/utils/init_close_utils.cpp",
    "src/utils/decoder_info.cpp",
    "src/utils/open.cpp",
    "src/utils/phaseone_processing.cpp",
    "src/utils/thumb_utils.cpp",
    "src/write/tiff_writer.cpp",
    "src/preprocessing/subtract_black.cpp",
    "src/postprocessing/postprocessing_utils.cpp",
    "src/postprocessing/dcraw_process.cpp",
    "src/preprocessing/raw2image.cpp",
    "src/postprocessing/mem_image.cpp",
    "src/x3f/x3f_utils_patched.cpp",
    "src/x3f/x3f_parse_process.cpp",
    "src/utils/read_utils.cpp",
    "src/utils/curves.cpp",
    "src/utils/utils_dcraw.cpp",
    "src/tables/colordata.cpp",
    "src/decoders/canon_600.cpp",
    "src/decoders/decoders_dcraw.cpp",
    "src/decoders/decoders_libraw_dcrdefs.cpp",
    "src/decoders/generic.cpp",
    "src/decoders/kodak_decoders.cpp",
    "src/decoders/dng.cpp",
    "src/decoders/smal.cpp",
    "src/decoders/load_mfbacks.cpp",
    "src/metadata/sony.cpp",
    "src/metadata/nikon.cpp",
    "src/metadata/samsung.cpp",
    "src/metadata/cr3_parser.cpp",
    "src/metadata/canon.cpp",
    "src/metadata/epson.cpp",
    "src/metadata/olympus.cpp",
    "src/metadata/leica.cpp",
    "src/metadata/fuji.cpp",
    "src/metadata/adobepano.cpp",
    "src/metadata/pentax.cpp",
    "src/metadata/p1.cpp",
    "src/metadata/makernotes.cpp",
    "src/metadata/exif_gps.cpp",
    "src/metadata/kodak.cpp",
    "src/metadata/tiff.cpp",
    "src/metadata/ciff.cpp",
    "src/metadata/mediumformat.cpp",
    "src/metadata/minolta.cpp",
    "src/metadata/identify_tools.cpp",
    "src/metadata/hasselblad_model.cpp",
    "src/metadata/normalize_model.cpp",
    "src/metadata/identify.cpp",
    "src/metadata/misc_parsers.cpp",
    "src/tables/wblists.cpp",
    "src/postprocessing/postprocessing_aux.cpp",
    "src/postprocessing/postprocessing_utils_dcrdefs.cpp",
    "src/postprocessing/aspect_ratio.cpp",
    "src/demosaic/misc_demosaic.cpp",
    "src/demosaic/xtrans_demosaic.cpp",
    "src/demosaic/ahd_demosaic.cpp",
    "src/demosaic/dht_demosaic.cpp",
    "src/demosaic/aahd_demosaic.cpp",
    "src/demosaic/dcb_demosaic.cpp",
    "src/write/file_write.cpp",
    "src/preprocessing/ext_preprocess.cpp",
    "src/write/apply_profile.cpp",
];

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../../third_party/libraw");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        // 用编译器默认的 GNU 方言：mingw 在严格的 c++14 下不给 M_PI 等常量。
        .define("_USE_MATH_DEFINES", None)
        .include(&root)
        .define("LIBRAW_NODLL", None)
        .define("LIBRAW_LIBRARY_BUILD", None)
        .opt_level(2)
        .warnings(false)
        .extra_warnings(false)
        .flag_if_supported("-w")
        .file("src/shim.cpp");
    for source in SOURCES {
        build.file(root.join(source));
    }
    build.compile("anyview_libraw");
    // LibRaw 在 Windows 上用 winsock 的 htonl 等。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=ws2_32");
    }
    println!("cargo:rerun-if-changed=src/shim.cpp");
    println!("cargo:rerun-if-changed=../../third_party/libraw");
}
