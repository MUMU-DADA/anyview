import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers
import AnyviewCore

/// macOS 的系统解码器（docs/FORMATS.md）：用 ImageIO 解 HEIC、HEIF、AVIF，含动态 AVIF、HEICS 的各帧。
/// RAW 关掉"显示已有缩略图"时也由它完整解码。
/// 核心在后台线程调用；ImageIO 可以在任意线程使用。唯一的可变状态是正在播放的动图的图像源（加锁）。
/// HDR 图片（如带增益图的 HEIC）先按普通亮度（SDR）显示。
final class ImageIODecoder: PlatformDecoder, @unchecked Sendable {
    /// 交给 ImageIO 的扩展名。
    static let extensions = ["heic", "heif", "hif", "heics", "avif", "avifs"]

    /// 正在播放的动图的图像源：各帧依次取，留着它不用每一帧都重新打开、重新解析文件（帧之间有依赖的格式
    /// 还得从头解起）。每播一遍（取第 0 帧时）重新打开一次：文件改过的话下一遍就是新的。
    private let frameLock = NSLock()
    private var frameSource: (path: String, source: CGImageSource)?

    private func source(forFrame index: UInt32, of path: String) -> CGImageSource? {
        frameLock.lock()
        defer { frameLock.unlock() }
        if index != 0, let kept = frameSource, kept.path == path {
            return kept.source
        }
        guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil) else {
            frameSource = nil
            return nil
        }
        frameSource = (path, source)
        return source
    }

    /// autoOrient：按 EXIF 方向摆正（设置"根据 EXIF 信息自动旋转图片"）。
    /// useIcc（设置"使用文件内的 ICC"）由核心处理：这里照样交出内嵌的 ICC，核心按设置取舍。
    func decode(path: String, autoOrient: Bool, useIcc: Bool) -> PlatformImage? {
        let url = URL(fileURLWithPath: path)
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil), CGImageSourceGetCount(source) > 0 else {
            return nil
        }
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any] ?? [:]
        let pixelWidth = properties[kCGImagePropertyPixelWidth] as? Int ?? 0
        let pixelHeight = properties[kCGImagePropertyPixelHeight] as? Int ?? 0
        guard pixelWidth > 0, pixelHeight > 0 else { return nil }
        // 用缩略图接口按原尺寸取图：同时按 EXIF 方向摆正（设置关掉时不摆正）。
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: autoOrient,
            kCGImageSourceThumbnailMaxPixelSize: max(pixelWidth, pixelHeight),
            kCGImageSourceShouldCacheImmediately: true,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else {
            return nil
        }
        guard let bitmap = Self.bitmap(image) else { return nil }
        return PlatformImage(
            width: UInt32(image.width),
            height: UInt32(image.height),
            pixels: bitmap.pixels,
            format: Self.formatName(source),
            pixelFormat: bitmap.pixelFormat,
            hasAlpha: bitmap.hasAlpha,
            iccProfile: bitmap.icc,
            shooting: Self.shooting(properties)
        )
    }

    /// 帧数：动态 AVIF、HEICS 等多帧的文件；读不了时为 0。
    func frameCount(path: String) -> UInt32 {
        let url = URL(fileURLWithPath: path)
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return 0 }
        return UInt32(CGImageSourceGetCount(source))
    }

    /// 第 index 帧（按画布大小）和它的时长。
    func decodeFrame(path: String, index: UInt32) -> PlatformFrame? {
        // 解好的帧不让图像源自己留着（它一直开着，留着就是整段动画的内存）。
        let options = [kCGImageSourceShouldCache: false] as CFDictionary
        guard let source = source(forFrame: index, of: path),
              Int(index) < CGImageSourceGetCount(source),
              let image = CGImageSourceCreateImageAtIndex(source, Int(index), options),
              let bitmap = Self.bitmap(image)
        else {
            return nil
        }
        let properties = CGImageSourceCopyPropertiesAtIndex(source, Int(index), nil) as? [String: Any] ?? [:]
        let frame = PlatformImage(
            width: UInt32(image.width),
            height: UInt32(image.height),
            pixels: bitmap.pixels,
            format: Self.formatName(source),
            pixelFormat: bitmap.pixelFormat,
            hasAlpha: bitmap.hasAlpha,
            iccProfile: bitmap.icc,
            shooting: nil
        )
        return PlatformFrame(image: frame, delayMs: Self.delayMs(properties))
    }

    /// 一帧的时长（毫秒）：AVIF 序列、HEIF 序列、GIF、PNG、WebP 的字典里写的，优先用不设下限的那个值。
    private static func delayMs(_ properties: [String: Any]) -> UInt32 {
        for key in ["{AVIS}", "{HEICS}", "{GIF}", "{PNG}", "{WebP}"] {
            guard let dict = properties[key] as? [String: Any] else { continue }
            let seconds = (dict["UnclampedDelayTime"] as? NSNumber)?.doubleValue
                ?? (dict["DelayTime"] as? NSNumber)?.doubleValue
            if let seconds {
                return UInt32(max(0, seconds * 1000).rounded())
            }
        }
        return 0
    }

    /// 画进 BGRA8 预乘的位图：保留图片自己的 RGB 色彩空间（交给 ColorSync）；
    /// 灰度等其他空间、或不能用作 8 位位图的空间，改用 sRGB。
    private static func bitmap(_ image: CGImage) -> (pixels: Data, hasAlpha: Bool, pixelFormat: String, icc: Data?)? {
        let width = image.width
        let height = image.height
        guard width > 0, height > 0, let srgb = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
        var space = srgb
        if let own = image.colorSpace, own.model == .rgb {
            space = own
        }
        let bitmapInfo = CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        func makeContext(_ space: CGColorSpace) -> CGContext? {
            CGContext(
                data: nil,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: width * 4,
                space: space,
                bitmapInfo: bitmapInfo
            )
        }
        var made = makeContext(space)
        if made == nil {
            space = srgb
            made = makeContext(srgb)
        }
        guard let context = made else { return nil }
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        guard let data = context.data else { return nil }
        let pixels = Data(bytes: data, count: width * height * 4)
        let alpha = image.alphaInfo
        let hasAlpha = !(alpha == .none || alpha == .noneSkipFirst || alpha == .noneSkipLast)
        let pixelFormat = "\(hasAlpha ? "RGBA" : "RGB")\(image.bitsPerComponent * (hasAlpha ? 4 : 3))"
        let isSRGB = (space.name as String?) == (CGColorSpace.sRGB as String)
        let icc: Data? = isSRGB ? nil : (space.copyICCData() as Data?)
        return (pixels, hasAlpha, pixelFormat, icc)
    }

    /// 拍摄信息：取自 ImageIO 的 TIFF、EXIF 字典；没有的项为空，核心负责过滤和排版。
    private static func shooting(_ properties: [CFString: Any]) -> ShootingInfo {
        let tiff = properties[kCGImagePropertyTIFFDictionary] as? [CFString: Any] ?? [:]
        let exif = properties[kCGImagePropertyExifDictionary] as? [CFString: Any] ?? [:]
        func number(_ value: Any?) -> Double? {
            (value as? NSNumber)?.doubleValue
        }
        return ShootingInfo(
            make: tiff[kCGImagePropertyTIFFMake] as? String,
            model: tiff[kCGImagePropertyTIFFModel] as? String,
            lens: exif[kCGImagePropertyExifLensModel] as? String,
            focalLength: number(exif[kCGImagePropertyExifFocalLength]),
            focalLength35mm: (exif[kCGImagePropertyExifFocalLenIn35mmFilm] as? NSNumber)?.uint32Value,
            fNumber: number(exif[kCGImagePropertyExifFNumber]),
            exposureTime: number(exif[kCGImagePropertyExifExposureTime]),
            iso: (exif[kCGImagePropertyExifISOSpeedRatings] as? [NSNumber])?.first?.uint32Value,
            taken: exif[kCGImagePropertyExifDateTimeOriginal] as? String
        )
    }

    private static func formatName(_ source: CGImageSource) -> String {
        guard let type = CGImageSourceGetType(source) as String? else { return "ImageIO" }
        switch type {
        case "public.heic", "public.heics": return "HEIC"
        case "public.heif": return "HEIF"
        case "public.avif", "public.avis": return "AVIF"
        default: return UTType(type)?.preferredFilenameExtension?.uppercased() ?? type
        }
    }
}
