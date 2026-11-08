/*
 * Anyview 的 JPEG XR 外壳：用 jxrlib（third_party/jxrlib，BSD）在内存里解码一张图，
 * 统一换成 8 位的 RGB、RGBA 或灰度交给 Rust。做法与 jxrlib 自带的 JxrDecApp 相同：
 * 按文件的像素格式设置解码器，再用 jxrlib 的格式转换器（PKFormatConverter）转成目标格式。
 * 高动态范围的格式（半精度、单精度浮点、定点、RGBE）由转换器按 scRGB → sRGB 换算，超出范围的截断（与 OpenEXR 的处理相同）。
 * CMYK 由解码器直接转成 RGB（JxrDecApp 的做法）。预乘透明度的（32bppPBGRA、PRGBA）转成 PRGBA 后在这里换回不预乘。
 * 容器里写的方向（旋转、镜像）由 jxrlib 解码时直接摆正。
 * 交进来的文件已经由 Rust 那边（crates/anyview-core/src/jxr.rs）重新拼过：只留解码要用的几项，元数据（EXIF、GPS、XMP、
 * 描述信息、ICC 等）都去掉了——jxrlib 解析元数据的代码有越界写、无限递归的问题，这样根本不会用到。
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "JXRGlue.h"

typedef struct {
    unsigned int width;
    unsigned int height;
    /* 1 灰度，3 RGB，4 RGBA（未预乘）。 */
    unsigned int channels;
    /* 文件里的像素格式，如 "RGB24"、"RGBA64Half"。 */
    char pixel_format[32];
    /* width * height * channels 字节，一行接一行。 */
    unsigned char *pixels;
} AnyviewJxrImage;

static const char *depth_name(BITDEPTH_BITS depth)
{
    switch (depth) {
    case BD_16F: return "Half";
    case BD_32F: return "Float";
    case BD_16S:
    case BD_32S: return "Fixed";
    default: return "";
    }
}

/* 文件里的像素格式的名字（图片信息里显示）。 */
static void format_name(const PKPixelInfo *info, char *out, size_t size)
{
    const char *kind;
    int alpha = (info->grBit & PK_pixfmtHasAlpha) != 0;
    switch (info->cfColorFormat) {
    case Y_ONLY: kind = alpha ? "GrayA" : "Gray"; break;
    case CMYK: kind = alpha ? "CMYKA" : "CMYK"; break;
    case CF_RGBE: kind = "RGBE"; break;
    case NCOMPONENT: kind = "N"; break;
    default: kind = alpha ? "RGBA" : "RGB"; break;
    }
    snprintf(out, size, "%s%u%s", kind, (unsigned int)info->cbitUnit, depth_name(info->bdBitDepth));
}

/* 每行的字节数（与 jxrlib 的 PKImageEncode_WriteSource 相同的算法）。 */
static size_t row_bytes(const PKPixelInfo *info, size_t width)
{
    if (info->bdBitDepth == BD_1) {
        return (info->cbitUnit * width + 7) >> 3;
    }
    return ((info->cbitUnit + 7) >> 3) * width;
}

void anyview_jxr_free(AnyviewJxrImage *image)
{
    if (image == NULL) {
        return;
    }
    free(image->pixels);
    image->pixels = NULL;
}

/*
 * 解码。成功返回 0，image 里是结果（用完调 anyview_jxr_free）；失败返回 jxrlib 的错误码（负数）或 1（太大、内存不够），
 * error 里是原因。max_bytes 是核心最后要分配的 RGBA 缓冲区（每像素 4 字节）的上限：超过时在解码之前就报错。
 */
int anyview_jxr_decode(const unsigned char *data, size_t size, size_t max_bytes, AnyviewJxrImage *image,
                       char *error, size_t error_size)
{
    ERR err = WMP_errSuccess;
    struct WMPStream *stream = NULL;
    PKImageDecode *decoder = NULL;
    PKFormatConverter *converter = NULL;
    PKPixelFormatGUID source;
    PKPixelFormatGUID target;
    PKPixelInfo info;
    PKPixelInfo target_info;
    I32 width = 0, height = 0;
    size_t stride = 0, out_row = 0;
    unsigned char *buffer = NULL;
    PKRect rect;
    int alpha, gray, premultiplied;
    size_t y;

    memset(image, 0, sizeof(*image));
    if (error_size > 0) {
        error[0] = 0;
    }
    /* jxrlib 的内存流只读这块数据，不会改它。 */
    Call(CreateWS_Memory(&stream, (void *)data, size));
    Call(PKImageDecode_Create_WMP(&decoder));
    err = decoder->Initialize(decoder, stream);
    /* 解码器释放时一起关掉流。 */
    decoder->fStreamOwner = !0;
    stream = NULL;
    Call(err);

    Call(decoder->GetPixelFormat(decoder, &source));
    info.pGUIDPixFmt = &source;
    Call(PixelFormatLookup(&info, LOOKUP_FORWARD));
    format_name(&info, image->pixel_format, sizeof(image->pixel_format));
    alpha = (info.grBit & PK_pixfmtHasAlpha) != 0;
    premultiplied = alpha && (info.grBit & PK_pixfmtPreMul) != 0;
    gray = info.cfColorFormat == Y_ONLY && !alpha;

    /* 与 JxrDecApp 相同的解码器设置：按文件的格式整幅解码，带透明通道时连同透明通道。 */
    decoder->WMP.wmiSCP.uAlphaMode = alpha ? 2 : 0;
    decoder->WMP.wmiI.cfColorFormat = info.cfColorFormat;
    decoder->WMP.wmiI.bdBitDepth = info.bdBitDepth;
    decoder->WMP.wmiI.cBitsPerUnit = info.cbitUnit;
    if (info.cfColorFormat == CMYK && !alpha) {
        /* CMYK → RGB 由解码器自己转（JxrDecApp 的做法）。 */
        decoder->WMP.wmiI.cfColorFormat = CF_RGB;
        decoder->guidPixFormat = GUID_PKPixelFormat24bppRGB;
        decoder->WMP.wmiI.bRGB = 1;
        source = GUID_PKPixelFormat24bppRGB;
        info.pGUIDPixFmt = &source;
        Call(PixelFormatLookup(&info, LOOKUP_FORWARD));
        decoder->WMP.wmiI.bdBitDepth = info.bdBitDepth;
        decoder->WMP.wmiI.cBitsPerUnit = info.cbitUnit;
    }
    decoder->WMP.wmiI.cThumbnailWidth = decoder->WMP.wmiI.cWidth;
    decoder->WMP.wmiI.cThumbnailHeight = decoder->WMP.wmiI.cHeight;
    decoder->WMP.wmiI.bSkipFlexbits = FALSE;
    decoder->WMP.wmiI.cROILeftX = 0;
    decoder->WMP.wmiI.cROITopY = 0;
    decoder->WMP.wmiI.cROIWidth = decoder->WMP.wmiI.cWidth;
    decoder->WMP.wmiI.cROIHeight = decoder->WMP.wmiI.cHeight;
    /* 方向用容器里写的（Initialize 已经设好；没写时是 O_NONE），GetSize 给出摆正以后的宽高。 */
    decoder->WMP.wmiI.cPostProcStrength = 0;
    decoder->WMP.wmiSCP.bVerbose = FALSE;

    Call(decoder->GetSize(decoder, &width, &height));
    if (width <= 0 || height <= 0) {
        snprintf(error, error_size, "size|%d|%d", (int)width, (int)height);
        err = WMP_errFail;
        goto Cleanup;
    }
    /* 核心最后按 RGBA 分配：先按每像素 4 字节检查，太大的不解。 */
    if ((size_t)height > max_bytes / 4 / (size_t)width) {
        snprintf(error, error_size, "large|%d|%d", (int)width, (int)height);
        err = WMP_errOutOfMemory;
        goto Cleanup;
    }

    target = alpha ? (premultiplied ? GUID_PKPixelFormat32bppPRGBA : GUID_PKPixelFormat32bppRGBA)
                   : (gray ? GUID_PKPixelFormat8bppGray : GUID_PKPixelFormat24bppRGB);
    Call(PKCodecFactory_CreateFormatConverter(&converter));
    err = converter->Initialize(converter, decoder, NULL, target);
    if (Failed(err)) {
        snprintf(error, error_size, "format|%s", image->pixel_format);
        goto Cleanup;
    }
    target_info.pGUIDPixFmt = &target;
    Call(PixelFormatLookup(&target_info, LOOKUP_FORWARD));

    /* 转换在同一块缓冲区里原地做：每行按两种格式里宽的那个留。 */
    stride = row_bytes(&info, (size_t)width);
    if (row_bytes(&target_info, (size_t)width) > stride) {
        stride = row_bytes(&target_info, (size_t)width);
    }
    image->channels = alpha ? 4 : (gray ? 1 : 3);
    out_row = (size_t)width * image->channels;
    if ((size_t)height > max_bytes / stride || (size_t)height > max_bytes / out_row) {
        snprintf(error, error_size, "large|%d|%d", (int)width, (int)height);
        err = WMP_errOutOfMemory;
        goto Cleanup;
    }
    buffer = (unsigned char *)calloc((size_t)height, stride);
    image->pixels = (unsigned char *)malloc(out_row * (size_t)height);
    if (buffer == NULL || image->pixels == NULL) {
        snprintf(error, error_size, "memory|%d|%d", (int)width, (int)height);
        err = WMP_errOutOfMemory;
        goto Cleanup;
    }
    rect.X = 0;
    rect.Y = 0;
    rect.Width = width;
    rect.Height = height;
    Call(converter->Copy(converter, &rect, buffer, (U32)stride));
    for (y = 0; y < (size_t)height; ++y) {
        memcpy(image->pixels + y * out_row, buffer + y * stride, out_row);
    }
    if (premultiplied) {
        /* 预乘的换回不预乘（核心统一预乘）。 */
        size_t i, count = (size_t)width * (size_t)height;
        for (i = 0; i < count; ++i) {
            unsigned char *px = image->pixels + i * 4;
            unsigned int a = px[3];
            if (a > 0 && a < 255) {
                unsigned int c;
                for (c = 0; c < 3; ++c) {
                    unsigned int v = (px[c] * 255u + a / 2) / a;
                    px[c] = (unsigned char)(v > 255 ? 255 : v);
                }
            }
        }
    }
    image->width = (unsigned int)width;
    image->height = (unsigned int)height;

Cleanup:
    free(buffer);
    if (converter != NULL) {
        converter->Release(&converter);
    }
    if (decoder != NULL) {
        decoder->Release(&decoder);
    }
    if (stream != NULL) {
        stream->Close(&stream);
    }
    if (Failed(err)) {
        if (error_size > 0 && error[0] == 0) {
            snprintf(error, error_size, "library|%d", (int)err);
        }
        anyview_jxr_free(image);
        return err < 0 ? (int)err : 1;
    }
    return 0;
}
