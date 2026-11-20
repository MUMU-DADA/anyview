# 图片格式支持

本文列出 Anyview 支持的图片格式、压缩包格式、动画格式与解码实现方案。

## 解码原则

1. **纯 Rust 优先**：尽可能使用 Rust 解码器，确保 macOS、Windows、Linux 三端行为一致。
2. **系统解码器补充**：在 macOS 上借助 ImageIO、在 Windows 上借助 WIC、在 Linux 上按需加载系统 `libheif`，及时获得最新相机 RAW 与 HEIC/AVIF 支持。
3. **按文件头识别格式**：浏览列表按扩展名过滤，实际解码以文件魔数（文件头）为准。
4. **HDR 与色彩管理**：
   - OpenEXR、Radiance HDR、HDR/广色域 AVIF 均映射到标准动态范围显示（PQ/HLG AVIF 做色调映射与广色域转换）。
   - 图片内嵌 ICC：macOS 交由 ColorSync 匹配显示器；Windows 与 Linux 由核心使用 `qcms` 转换至目标色彩空间（无 ICC 时按 sRGB 处理）。

## 格式与解码方案

| 格式 | 扩展名 | 解码实现 |
| --- | --- | --- |
| JPEG | `.jpg` `.jpeg` `.jpe` `.jfif` | `zune-jpeg`（YCbCr 快速路径直接出 BGRA；灰度/CMYK 等经 `image`）；大图支持 EXIF 内嵌缩略图快速预览 |
| PNG / APNG | `.png` `.apng` | `png`（支持静态与动画） |
| GIF | `.gif` | `gif`（支持动画播放与逐帧查看） |
| WebP | `.webp` | `image-webp`（支持静态与动画） |
| BMP | `.bmp` `.dib` | `image` |
| TIFF | `.tif` `.tiff` | `tiff`（支持多页） |
| TGA | `.tga` | `image` |
| PNM 系列 | `.pbm` `.pgm` `.ppm` `.pnm` `.pam` | `image` |
| QOI | `.qoi` | `qoi` |
| ICO / CUR | `.ico` `.cur` | `image`（选取最大尺寸帧） |
| HEIC / HEIF / HIF | `.heic` `.heif` `.hif` `.heics` | macOS：ImageIO（含 HEIF 序列动画）；Windows：优先 WIC（需系统 HEIF/HEVC 扩展），回退至可选 `heif.dll`（libheif + libde265）；Linux：运行时动态加载系统 `libheif` |
| AVIF（含动图） | `.avif` `.avifs` | macOS：ImageIO（静态与动画）；Windows/Linux：优先系统解码，不支持或播放动图时回退至核心纯 Rust 解码器（`mp4parse` + `rav1d`） |
| PSD / PSB | `.psd` `.psb` | 自研解码器：支持合成预览（原始/PackBits，1/8/16/32 位，灰度/索引/RGB/CMYK/双色调/Lab）及无兼容合成图时的图层自主合成（含不透明度、像素蒙版、矢量蒙版） |
| DDS | `.dds` | 自研头解析（旧式与 DX10、8 位索引色）+ `bcdec_rs`（BC1–BC7 块解码） |
| OpenEXR | `.exr` | `exr`（经 `image`），线性通道映射至 sRGB |
| Radiance HDR | `.hdr` | `image` HDR 解码器，与 EXR 一致映射至 sRGB |
| JPEG XL（含动图） | `.jxl` | `jxl-oxide`（支持静态与动画） |
| SVG / SVGZ | `.svg` `.svgz` | `resvg`；缩放时由精确重绘按屏幕物理像素重新矢量渲染；禁止读取外部网络/文件资源 |
| PCX | `.pcx` | 自研解码器 |
| RAW（内嵌预览） | 见下 | 自研 TIFF/容器结构解析，提取相机内嵌最大 JPEG 预览并应用方向 |
| RAW（完整解码） | 见下 | `LibRaw`（CDDL-1.0，源码编入核心；macOS 优先尝试 ImageIO），用于无内嵌预览或关闭“显示已有缩略图”时 |
| JPEG 2000 | `.jp2` `.j2k` `.j2c` `.jpc` `.jpx` `.jpf` | `hayro-jpeg2000`（纯 Rust） |
| JPEG XR | `.jxr` `.wdp` `.hdp` | `jxrlib`（BSD-2-Clause，源码编入核心，scRGB 转换至 sRGB；Windows 优先 WIC，失败时回退 `jxrlib`） |
| Clip Studio Paint | `.clip` | 读取内嵌 SQLite 数据库 `CanvasPreview` 表中的整幅画布 PNG 预览（经 `rusqlite`） |

支持的常见 RAW 扩展名包括：`.dng` `.cr2` `.cr3` `.crw` `.nef` `.nrw` `.orf` `.rw2` `.pef` `.sr2` `.arw` `.srf` `.raf` `.3fr` `.fff` `.iiq` `.rwl` `.srw` `.x3f` `.erf` `.mrw` `.mos` `.kdc` `.dcr` 等（含 Canon CR3、Apple ProRAW 与 JPEG XL 压缩的 DNG 1.7 等）。

## 压缩包浏览

支持将 **ZIP**（`.zip` `.cbz`）、**7Z**（`.7z` `.cb7`）、**RAR**（`.rar` `.cbr`）作为普通文件夹直接浏览，包内支持的图片格式（含动图与系统解码格式）与磁盘文件一致，并支持记录每个压缩包的上次阅读位置。

## 动图播放

GIF、APNG、WebP、JPEG XL 动画由三端共用的核心解码器逐帧解出；macOS 的动态 AVIF 与 HEIF 序列由 ImageIO 逐帧解码；Linux 与 Windows 的动态 AVIF 由核心纯 Rust 解码器逐帧解码。

## 测试样例

- `crates/anyview-fixtures` 可自动生成合成测试图集（覆盖各格式、EXIF 方向、透明图、动图、超大图、损坏文件与压缩包）。
- 开放许可真实照片与 CC0 RAW 样例的元数据及 SHA-256 清单位于 `tools/testdata/real-images.json` 与 `tools/testdata/raw-samples.json`，可通过 `python3 -I tools/testdata/fetch.py [--raw]` 按需下载到被 Git 忽略的 `tests/fixtures/` 目录进行校验。
