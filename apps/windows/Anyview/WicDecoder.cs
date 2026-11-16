// 系统解码器：HEIC、HEIF、AVIF、JPEG XR 交给 Windows 自带的 WIC（Windows.Graphics.Imaging），
// 相当于 macOS 的 ImageIO。HEIC 要系统装了"HEIF 图像扩展"和 HEVC 扩展，AVIF 要"AV1 视频扩展"；没装时解不了，
// 核心照常提示打不开。核心在后台线程调用，这里同步等结果。
using System;
using System.Collections.Concurrent;
using System.IO;
using System.Runtime.InteropServices.WindowsRuntime;
using Anyview.Core;
using Windows.Graphics.Imaging;
using Windows.Storage;
using Windows.Storage.Streams;

namespace Anyview;

internal sealed class WicDecoder : PlatformDecoder
{
    public static readonly string[] Extensions = { "heic", "heif", "hif", "heics", "avif", "avifs", "jxr", "wdp", "hdp" };

    /// <summary>
    /// 最近一次解不了的原因（按完整路径记；解出来了就删掉）。系统解不了时核心改用自己的解码器或报不支持，
    /// 原因只在这里：自检记进报告。
    /// </summary>
    private static readonly ConcurrentDictionary<string, string> s_errors = new(StringComparer.OrdinalIgnoreCase);

    public static string? ErrorFor(string path)
    {
        try
        {
            return s_errors.TryGetValue(Path.GetFullPath(path), out var error) ? error : null;
        }
        catch (Exception)
        {
            return null;
        }
    }

    private static void Remember(string path, string? error)
    {
        try
        {
            var key = Path.GetFullPath(path);
            if (error is null)
            {
                s_errors.TryRemove(key, out _);
            }
            else
            {
                s_errors[key] = error;
            }
        }
        catch (Exception)
        {
        }
    }

    private static string? Safe(Func<string?> read)
    {
        try
        {
            return read();
        }
        catch (Exception)
        {
            return "?";
        }
    }

    private static string Describe(Exception error, string? stage) =>
        $"{stage}：{error.GetType().Name} 0x{error.HResult:X8} {error.Message.Trim()}";

    /// <summary>useIcc：设置"使用文件内的 ICC"。WIC 解码时就换成 sRGB，关掉时不做色彩管理。</summary>
    public PlatformImage? Decode(string path, bool autoOrient, bool useIcc)
    {
        var stage = "打开文件";
        try
        {
            using var stream = Open(path);
            stage = "找解码器";
            var decoder = BitmapDecoder.CreateAsync(stream).AsTask().GetAwaiter().GetResult();
            var name = Safe(() => decoder.DecoderInformation?.FriendlyName);
            stage = $"取第一帧（{name}）";
            var frame = decoder.GetFrameAsync(0).AsTask().GetAwaiter().GetResult();
            // 原来的像素格式只用来说明出错的地方：读不出来也不影响解码。
            stage = $"取像素（{name}，{Safe(() => frame.BitmapPixelFormat.ToString())}，{Safe(() => frame.BitmapAlphaMode.ToString())}）";
            var image = Convert(frame, name, autoOrient, useIcc);
            Remember(path, image is null ? $"{stage}：像素数和宽高对不上" : null);
            return image;
        }
        catch (Exception error)
        {
            Remember(path, Describe(error, stage));
            return null;
        }
    }

    public uint FrameCount(string path)
    {
        try
        {
            using var stream = Open(path);
            var decoder = BitmapDecoder.CreateAsync(stream).AsTask().GetAwaiter().GetResult();
            return decoder.FrameCount;
        }
        catch (Exception)
        {
            return 0;
        }
    }

    public PlatformFrame? DecodeFrame(string path, uint index)
    {
        try
        {
            using var stream = Open(path);
            var decoder = BitmapDecoder.CreateAsync(stream).AsTask().GetAwaiter().GetResult();
            if (index >= decoder.FrameCount)
            {
                return null;
            }
            var frame = decoder.GetFrameAsync(index).AsTask().GetAwaiter().GetResult();
            var image = Convert(frame, decoder.DecoderInformation?.FriendlyName, false, true);
            return image is null ? null : new PlatformFrame(image, 100);
        }
        catch (Exception)
        {
            return null;
        }
    }

    /// <summary>直接按路径打开（不经过 StorageFile 的代理，快一些）。</summary>
    private static IRandomAccessStream Open(string path) =>
        FileRandomAccessStream.OpenAsync(Path.GetFullPath(path), FileAccessMode.Read).AsTask().GetAwaiter().GetResult();

    private static PlatformImage? Convert(BitmapFrame frame, string? name, bool autoOrient, bool useIcc)
    {
        var transform = new BitmapTransform();
        var orientation = autoOrient ? ExifOrientationMode.RespectExifOrientation : ExifOrientationMode.IgnoreExifOrientation;
        PixelDataProvider data;
        if (!useIcc)
        {
            data = frame.GetPixelDataAsync(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied, transform,
                orientation, ColorManagementMode.DoNotColorManage).AsTask().GetAwaiter().GetResult();
        }
        else
        {
            try
            {
                data = frame.GetPixelDataAsync(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied, transform,
                    orientation, ColorManagementMode.ColorManageToSRgb).AsTask().GetAwaiter().GetResult();
            }
            catch (Exception)
            {
                // 有的解码器做不了色彩管理（没有色彩信息的格式也可能报错）：不做色彩管理再取一次。
                data = frame.GetPixelDataAsync(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied, transform,
                    orientation, ColorManagementMode.DoNotColorManage).AsTask().GetAwaiter().GetResult();
            }
        }
        var pixels = data.DetachPixelData();
        // 摆正后宽高可能对调：按像素数推回来。
        uint width = frame.OrientedPixelWidth, height = frame.OrientedPixelHeight;
        if (!autoOrient)
        {
            width = frame.PixelWidth;
            height = frame.PixelHeight;
        }
        if ((ulong)width * height * 4 != (ulong)pixels.Length)
        {
            (width, height) = (height, width);
            if ((ulong)width * height * 4 != (ulong)pixels.Length)
            {
                return null;
            }
        }
        // 有没有透明按取出来的像素看（读不出原来的透明方式时也一样）。
        var hasAlpha = false;
        for (var i = 3; i < pixels.Length; i += 4)
        {
            if (pixels[i] != 255)
            {
                hasAlpha = true;
                break;
            }
        }
        var format = Format(name);
        return new PlatformImage(width, height, pixels, format, hasAlpha ? "RGBA32" : "RGB24", hasAlpha, null);
    }

    /// <summary>格式名：WIC 解码器的友好名称里挑出常见的写法。</summary>
    private static string Format(string? name)
    {
        var n = (name ?? "").ToUpperInvariant();
        if (n.Contains("HEIF") || n.Contains("HEIC")) return "HEIC";
        if (n.Contains("AV1") || n.Contains("AVIF")) return "AVIF";
        if (n.Contains("WMPHOTO") || n.Contains("JPEG XR") || n.Contains("JPEG-XR")) return "JPEG XR";
        return string.IsNullOrEmpty(name) ? "WIC" : name!;
    }
}
