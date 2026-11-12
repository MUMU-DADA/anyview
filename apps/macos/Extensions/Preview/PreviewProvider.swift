import CoreGraphics
import Foundation
import QuickLookUI
import AnyviewCore

/// 访达里按空格的预览：和缩略图一样，只管系统自己不认识的格式（类型由编译时的类型调查决定）。
/// 这是一个按数据给出的 Quick Look 预览扩展（QLIsDataBasedPreview），放在 Anyview.app/Contents/PlugIns/ 里，
/// 系统在另一个进程、沙盒里运行它。用核心的同一个函数（make_file_thumbnail）解码，长边最多 4096 像素（够满屏看），
/// 画进 Quick Look 给的画布。
@objc(PreviewProvider)
final class PreviewProvider: QLPreviewProvider, QLPreviewingController {
    /// 不是重写父类的方法（QLPreviewProvider 自己没有它）：是 QLPreviewingController 协议里的可选方法。
    /// 写法同 Xcode 的"Quick Look 预览扩展"模板（async 形式，系统按 Objective-C 的 completionHandler 形式调用）。
    /// macOS 上写成 override 加 completionHandler 时报"没有可重写的方法"。
    func providePreview(for request: QLFilePreviewRequest) async throws -> QLPreviewReply {
        guard let handle = makeFileThumbnail(path: request.fileURL.path, maxSide: 4096),
              let image = Self.cgImage(handle)
        else {
            throw NSError(domain: "local.anyview.Anyview.Preview", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "Anyview 解不了这个文件"])
        }
        // 按点算的大小：一个像素一个点（同“预览”的实际大小），长边超过 2048 点时缩小，免得 Retina 上建两倍大的位图。
        let longest = CGFloat(max(image.width, image.height))
        let fit = min(1, 2048 / longest)
        let size = CGSize(width: (CGFloat(image.width) * fit).rounded(), height: (CGFloat(image.height) * fit).rounded())
        return QLPreviewReply(contextSize: size, isBitmap: true, drawUsing: { context, _ in
            context.interpolationQuality = .high
            // 画满 Quick Look 给的画布：按位图的实际像素换算到当前坐标（见缩略图扩展的 fullCanvas）。
            let pixels = context.width > 0 && context.height > 0
                ? CGRect(x: 0, y: 0, width: context.width, height: context.height)
                : CGRect(origin: .zero, size: size)
            context.draw(image, in: pixels.applying(context.ctm.inverted()))
        })
    }

    /// 核心给的 BGRA8 预乘图像 → CGImage（复制一份像素）。色彩空间：带 ICC 时按它解释，否则为 sRGB。
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
