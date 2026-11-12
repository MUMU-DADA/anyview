import AppKit
import QuickLookThumbnailing
import UniformTypeIdentifiers

/// 访达缩略图的检查工具：用访达同样的系统接口（QLThumbnailGenerator）给几个文件要缩略图，
/// 打印文件的实际类型、系统给了哪几种（图标、低品质缩略图、缩略图）、用了多久、缩略图的尺寸和四块颜色。
/// 一键测试临时登记缩略图扩展以后运行它，看系统会不会启动 Anyview 的扩展（比 qlmanage 更接近访达）。
/// 只问系统要图，不改任何设置。
///
/// 用法：ql-probe <每个文件最多等几秒> <文件…>
@main
struct QLProbe {
    static func main() {
        // 输出写进文件时按行写出：万一被一键测试的限时停掉，已经打印的也留得下。
        setvbuf(stdout, nil, _IOLBF, 0)
        var arguments = Array(CommandLine.arguments.dropFirst())
        guard arguments.count >= 2, let limit = Double(arguments.removeFirst()), limit > 0 else {
            FileHandle.standardError.write(Data("用法：ql-probe <秒> <文件…>\n".utf8))
            exit(2)
        }
        var made = 0
        for path in arguments {
            if probe(URL(fileURLWithPath: path), limit: limit) {
                made += 1
            }
        }
        print("结果：\(made)/\(arguments.count) 个文件拿到了缩略图")
        exit(0)
    }

    /// 一个文件：等到系统给出"缩略图"这一种的结果（有图或失败），最多 limit 秒。返回是否拿到了缩略图。
    static func probe(_ url: URL, limit: Double) -> Bool {
        let actual = (try? url.resourceValues(forKeys: [.contentTypeKey]))?.contentType
        var header = "\(url.lastPathComponent)\t类型 \(actual?.identifier ?? "未知")"
        if let byExtension = UTType(filenameExtension: url.pathExtension), byExtension != actual {
            header += "（按扩展名是 \(byExtension.identifier)）"
        }
        print(header)
        // 按 2 倍要（同 Retina 屏上的访达）：。
        let request = QLThumbnailGenerator.Request(
            fileAt: url, size: CGSize(width: 256, height: 256), scale: 2, representationTypes: .all
        )
        let collector = Collector()
        let start = Date()
        QLThumbnailGenerator.shared.generateRepresentations(for: request) { representation, kind, error in
            let ms = Int(Date().timeIntervalSince(start) * 1000)
            let name = QLProbe.label(kind)
            let text: String
            if let representation {
                text = "\(name) \(ms) ms：\(QLProbe.describe(representation.cgImage))"
            } else {
                let reason = error?.localizedDescription ?? "没有说明"
                text = "\(name) \(ms) ms：没有（\(reason)）"
            }
            collector.add(text, isLast: kind == .thumbnail, thumbnail: kind == .thumbnail && representation != nil)
        }
        // 回调可能在主线程上来，等的时候让主线程的 RunLoop 照常转。
        let deadline = Date().addingTimeInterval(limit)
        while Date() < deadline, !collector.state().finished {
            if !RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.05)) {
                Thread.sleep(forTimeInterval: 0.05)
            }
        }
        let state = collector.state()
        for line in state.lines {
            print("  \(line)")
        }
        if !state.finished {
            QLThumbnailGenerator.shared.cancel(request)
            print("  缩略图：\(Int(limit)) 秒内没有回音（已取消）")
        }
        return state.thumbnail
    }

    static func label(_ kind: QLThumbnailRepresentation.RepresentationType) -> String {
        switch kind {
        case .icon: return "图标"
        case .lowQualityThumbnail: return "低品质缩略图"
        case .thumbnail: return "缩略图"
        @unknown default: return "其他（\(kind.rawValue)）"
        }
    }

    /// 尺寸和四块的颜色（测试图片是四色块：左上红、右上绿、左下蓝、右下黄），按 sRGB 读。
    static func describe(_ image: CGImage) -> String {
        let width = image.width, height = image.height
        guard width > 0, height > 0,
              let space = CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(
                  data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                  space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
              )
        else { return "\(width)x\(height)" }
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        guard let data = context.data else { return "\(width)x\(height)" }
        let pixels = data.bindMemory(to: UInt8.self, capacity: width * height * 4)
        // CGContext 的第一行是图片的最上面一行。
        func color(_ fx: Double, _ fy: Double) -> String {
            let x = min(width - 1, Int(Double(width) * fx)), y = min(height - 1, Int(Double(height) * fy))
            let offset = (y * width + x) * 4
            return String(format: "#%02X%02X%02X", pixels[offset], pixels[offset + 1], pixels[offset + 2])
        }
        // 四色块（左上红、右上绿、左下蓝、右下黄）对不对：每块看最大的那个通道。
        func dominant(_ fx: Double, _ fy: Double) -> String {
            let x = min(width - 1, Int(Double(width) * fx)), y = min(height - 1, Int(Double(height) * fy))
            let offset = (y * width + x) * 4
            let (r, g, b) = (Int(pixels[offset]), Int(pixels[offset + 1]), Int(pixels[offset + 2]))
            if r > 150 && g > 150 && b < 120 { return "黄" }
            if r > g + 60 && r > b + 60 { return "红" }
            if g > r + 60 && g > b + 30 { return "绿" }
            if b > r + 60 && b > g + 60 { return "蓝" }
            return "?"
        }
        let blocks = [dominant(0.25, 0.25), dominant(0.75, 0.25), dominant(0.25, 0.75), dominant(0.75, 0.75)].joined()
        return "\(width)x\(height) 左上 \(color(0.25, 0.25)) 右上 \(color(0.75, 0.25)) "
            + "左下 \(color(0.25, 0.75)) 右下 \(color(0.75, 0.75)) 四色块 \(blocks == "红绿蓝黄" ? "对" : blocks)"
    }
}

/// 回调里收集的结果（回调在系统的线程上来，加锁）。
final class Collector: @unchecked Sendable {
    private let lock = NSLock()
    private var lines: [String] = []
    private var finished = false
    private var thumbnail = false

    func add(_ line: String, isLast: Bool, thumbnail: Bool) {
        lock.lock()
        defer { lock.unlock() }
        lines.append(line)
        if isLast {
            finished = true
            self.thumbnail = thumbnail
        }
    }

    func state() -> (lines: [String], finished: Bool, thumbnail: Bool) {
        lock.lock()
        defer { lock.unlock() }
        return (lines, finished, thumbnail)
    }
}
