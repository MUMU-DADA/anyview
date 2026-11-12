import CoreGraphics
import Foundation
import QuickLookThumbnailing
import AnyviewCore

/// 访达缩略图：系统自己不认识的格式（QOI、CLIP、JPEG XR 等，具体由编译时的类型调查决定，
/// 见 tools/macos/编译测试.command 和 apps/macos/Tools/TypeSurvey）由 Anyview 的核心解码、缩小，交给 Quick Look。
///
/// 这是一个 Quick Look 缩略图扩展（app extension），放在 Anyview.app/Contents/PlugIns/ 里，系统在另一个进程里运行它，
/// 按沙盒规则只能读要做缩略图的那个文件。常见格式（JPEG、PNG、HEIC、RAW 等）仍由系统自己生成，这里不声明。
/// 只用核心的一个函数（make_file_thumbnail），不建窗口、不读设置。
@objc(ThumbnailProvider)
final class ThumbnailProvider: QLThumbnailProvider {
    override func provideThumbnail(for request: QLFileThumbnailRequest,
                                   _ handler: @escaping (QLThumbnailReply?, Error?) -> Void) {
        let scale = max(request.scale, 1)
        let maxPoints = max(request.maximumSize.width, request.maximumSize.height)
        let maxPixels = UInt32(min(max((maxPoints * scale).rounded(), 16), 2048))
        guard let handle = makeFileThumbnail(path: request.fileURL.path, maxSide: maxPixels),
              let image = Self.cgImage(handle)
        else {
            handler(nil, NSError(domain: "local.anyview.Anyview.Thumbnail", code: 1,
                                 userInfo: [NSLocalizedDescriptionKey: "Anyview 解不了这个文件"]))
            return
        }
        // 按点算的大小：保持比例放进 maximumSize，不放大。
        let width = CGFloat(image.width) / scale
        let height = CGFloat(image.height) / scale
        let fit = min(request.maximumSize.width / width, request.maximumSize.height / height, 1)
        let size = CGSize(width: max(1, (width * fit).rounded()), height: max(1, (height * fit).rounded()))
        handler(QLThumbnailReply(contextSize: size, drawing: { context in
            context.interpolationQuality = .high
            context.draw(image, in: Self.fullCanvas(context, points: size, scale: scale))
            return true
        }), nil)
    }

    /// 画满 Quick Look 给的画布：它按 contextSize × scale 建位图，坐标却不跟着缩放，按点画只占左下四分之一。
    /// 按位图的实际像素，换算到当前坐标（坐标若已缩放也对）。
    /// 先试过取裁剪范围（boundingBoxOfClipPath），这个画布上取不到，退回了按点画，2 倍时照样只画四分之一。
    static func fullCanvas(_ context: CGContext, points size: CGSize, scale: CGFloat) -> CGRect {
        let pixels = context.width > 0 && context.height > 0
            ? CGRect(x: 0, y: 0, width: context.width, height: context.height)
            : CGRect(x: 0, y: 0, width: size.width * scale, height: size.height * scale)
        return pixels.applying(context.ctm.inverted())
    }

    /// 核心给的 BGRA8 预乘图像 → CGImage（复制一份像素：缩略图很小，用完句柄就可以放掉）。
    /// 色彩空间：带 ICC 时按它解释（macOS 的核心不转色，与主程序相同），否则为 sRGB。
    static func cgImage(_ handle: ImageHandle) -> CGImage? {
        guard let base = UnsafeRawPointer(bitPattern: UInt(handle.pixelAddress())) else { return nil }
        let data = Data(bytes: base, count: Int(handle.byteLen()))
        guard let provider = CGDataProvider(data: data as CFData) else { return nil }
        let space = handle.iccProfile().flatMap { CGColorSpace(iccData: $0 as CFData) }
            .flatMap { $0.model == .rgb ? $0 : nil }
            ?? CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        let bitmapInfo = CGBitmapInfo(
            rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        )
        return CGImage(
            width: Int(handle.width()),
            height: Int(handle.height()),
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: Int(handle.stride()),
            space: space,
            bitmapInfo: bitmapInfo,
            provider: provider,
            decode: nil,
            shouldInterpolate: true,
            intent: .defaultIntent
        )
    }
}
