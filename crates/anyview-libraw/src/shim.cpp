// Anyview 用的 LibRaw 外壳：一次调用把内存里的 RAW 文件显影成 8 位 sRGB。
// 只用 C 接口跟 Rust 打交道，异常都在这里接住。
#include <cstdlib>
#include <cstring>
#include <new>
#include "libraw/libraw.h"

extern "C" {

struct AnyviewRawImage {
  unsigned char *rgb; // width × height × 3，用 anyview_libraw_free 释放
  int width;
  int height;
};

// 返回 0 表示成功；否则把原因写进 error（以 0 结尾）。
int anyview_libraw_decode(const void *data, size_t size, int auto_orient,
                          AnyviewRawImage *out, char *error, size_t error_len) {
  auto fail = [&](const char *message) {
    if (error && error_len) {
      std::strncpy(error, message, error_len - 1);
      error[error_len - 1] = 0;
    }
    return 1;
  };
  out->rgb = nullptr;
  out->width = out->height = 0;
  LibRaw *raw = nullptr;
  try {
    raw = new (std::nothrow) LibRaw(LIBRAW_OPTIONS_NONE);
    if (!raw)
      return fail("out of memory");
    libraw_output_params_t &p = raw->imgdata.params;
    p.use_camera_wb = 1;  // 按相机记下的白平衡
    p.output_color = 1;   // sRGB
    p.output_bps = 8;
    p.user_flip = auto_orient ? -1 : 0; // -1：按文件里的方向摆正；0：不转
    p.user_qual = 2;      // PPG 去马赛克：比默认的 AHD 快得多，看图足够；X-Trans 随之用单遍算法
    raw->set_dataerror_handler(nullptr, nullptr); // 不往终端打印数据错误
    int rc = raw->open_buffer(data, size);
    if (rc == LIBRAW_SUCCESS)
      rc = raw->unpack();
    if (rc == LIBRAW_SUCCESS)
      rc = raw->dcraw_process();
    if (rc != LIBRAW_SUCCESS) {
      fail(libraw_strerror(rc));
      delete raw;
      return 1;
    }
    int errc = 0;
    libraw_processed_image_t *img = raw->dcraw_make_mem_image(&errc);
    if (!img || img->type != LIBRAW_IMAGE_BITMAP || img->bits != 8 ||
        (img->colors != 3 && img->colors != 1)) {
      if (img)
        LibRaw::dcraw_clear_mem(img);
      fail(errc ? libraw_strerror(errc) : "unexpected output");
      delete raw;
      return 1;
    }
    size_t pixels = size_t(img->width) * img->height;
    unsigned char *rgb = static_cast<unsigned char *>(std::malloc(pixels * 3));
    if (!rgb) {
      LibRaw::dcraw_clear_mem(img);
      delete raw;
      return fail("out of memory");
    }
    if (img->colors == 3) {
      std::memcpy(rgb, img->data, pixels * 3);
    } else {
      for (size_t i = 0; i < pixels; i++)
        rgb[i * 3] = rgb[i * 3 + 1] = rgb[i * 3 + 2] = img->data[i];
    }
    out->rgb = rgb;
    out->width = img->width;
    out->height = img->height;
    LibRaw::dcraw_clear_mem(img);
    delete raw;
    return 0;
  } catch (...) {
    delete raw;
    return fail("LibRaw failed");
  }
}

void anyview_libraw_free(unsigned char *rgb) { std::free(rgb); }

const char *anyview_libraw_version(void) { return LibRaw::version(); }

} // extern "C"
