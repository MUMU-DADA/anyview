import Foundation
import ImageIO
import UniformTypeIdentifiers

/// 类型调查：编译时在这台 Mac 上决定访达缩略图扩展声明哪些类型。
/// 读 Info.plist 模板里按扩展名登记的全部格式和导入的类型声明，逐个扩展名看：
/// - 系统的 ImageIO 能解码的（访达自己会生成缩略图）不声明；RAW 即使 ImageIO 不列，Quick Look 也用相机的预览自己会做，不声明；
/// - 但 ImageIO 列着、却解不了本项目某张测试图片的类型要声明（如 BC7 的 DDS、JPEG 2000 码流、32 位 PSD 等）。
///   同一种类型只能整个接管，系统能解的那些也由 Anyview 做。
///   RAW（测试图片不是真的 RAW）和只有系统能解的 HEIC 不在此列；
/// - SVG 不声明：macOS 把 .svgz（gzip 压缩的 SVG）也算作 public.svg-image，系统读不了压缩的（预览报 "Document is
///   empty"），但系统内置 SVG 扩展优先级更高，声明了也没用，SVGZ 在访达里的缩略图、预览属于 macOS 的限制；
/// - 是 Anyview 导入的类型时用它（不依赖系统有没有登记过这个程序）；导入的类型属于 ImageIO 能解码的类型
///   （如 jfif 属于 public.jpeg）时也声明：系统的图片缩略图扩展不认 local.anyview.* 这个类型名，不做；
/// - 系统里没有类型（dyn.）的声明不了，记下来。
///
/// 用法：TypeSurvey <Info.plist 模板> <缩略图扩展声明的类型（每行一个）> <调查结果（文字）> [测试图片目录] [预览扩展声明的类型]
/// 预览扩展不声明属于系统能解类型的导入类型（APNG、JFIF）：系统自己的预览能做（APNG 还会动），缩略图扩展才需要。
@main
struct TypeSurvey {
    static func main() {
        let arguments = CommandLine.arguments
        guard (4...6).contains(arguments.count),
              let plist = NSDictionary(contentsOfFile: arguments[1]) as? [String: Any]
        else {
            FileHandle.standardError.write(Data("用法：TypeSurvey <Info.plist> <types.txt> <report.txt> [fixtures]\n".utf8))
            exit(2)
        }
        let samples = arguments.count >= 5 ? Self.samples(in: arguments[4]) : [:]
        // 预览扩展不声明的（见上）。
        var previewSkip: Set<String> = []
        // 核心自己解不了、只能交给系统的（macOS 上 HEIC 只用 ImageIO）：系统解不了也不接管。
        let systemOnly: Set<String> = ["heic", "heif", "hif", "heics"]
        // 最常见的格式一直由系统做：就算某张测试图片 ImageIO 解不了，也不为它接管全部 JPEG、PNG。
        let keepSystem: Set<String> = [
            "public.jpeg", "public.png", "com.compuserve.gif", "public.tiff", "com.microsoft.bmp",
            "org.webmproject.webp", "public.heic", "public.heif",
        ]
        let documentTypes = plist["CFBundleDocumentTypes"] as? [[String: Any]] ?? []
        let extensions = documentTypes
            .filter { ($0["CFBundleTypeName"] as? String) == "Image File" }
            .flatMap { $0["CFBundleTypeExtensions"] as? [String] ?? [] }
            .map { $0.lowercased() }
        var imported: [String: (id: String, conforms: [String])] = [:]
        for declaration in plist["UTImportedTypeDeclarations"] as? [[String: Any]] ?? [] {
            guard let id = declaration["UTTypeIdentifier"] as? String else { continue }
            let tags = declaration["UTTypeTagSpecification"] as? [String: Any] ?? [:]
            let conforms = declaration["UTTypeConformsTo"] as? [String] ?? []
            for ext in tags["public.filename-extension"] as? [String] ?? [] {
                imported[ext.lowercased()] = (id, conforms)
            }
        }
        let decodable = Set((CGImageSourceCopyTypeIdentifiers() as? [String]) ?? [])
        // 太笼统的类型不算"ImageIO 能解码"（public.image 不代表每种图片都能解）。
        let generic: Set<String> = ["public.image", "public.data", "public.item", "public.content"]
        var claims: [String] = []
        var lines: [String] = []
        for ext in extensions {
            if let entry = imported[ext] {
                let id = entry.id
                // 属于系统能解的类型（jfif 属于 public.jpeg、apng 属于 public.png）也要声明：访达把文件实际的类型名
                // （我们导入的 local.anyview.*）交给系统的图片缩略图扩展，它不认这个名字就不做。
                if !claims.contains(id) { claims.append(id) }
                if let parent = entry.conforms.first(where: { decodable.contains($0) && !generic.contains($0) }) {
                    previewSkip.insert(id)
                    lines.append("\(ext)\t\(id)\t缩略图由 Anyview 生成（导入的类型，属于 \(parent)，系统的图片扩展不认这个类型名）；预览由系统做")
                    continue
                }
                // 别的程序可能给这个扩展名登记了它自己的类型（导出的类型比我们导入的优先），那时文件实际是那种类型：
                // 系统解不了的话一并声明，不然访达不会找到这个扩展。
                if let actual = UTType(filenameExtension: ext), !actual.isDynamic, actual.identifier != id,
                   !decodable.contains(actual.identifier) {
                    if !claims.contains(actual.identifier) { claims.append(actual.identifier) }
                    lines.append("\(ext)\t\(id) + \(actual.identifier)\tAnyview 生成（导入的类型，另有别的程序登记的类型）")
                } else {
                    lines.append("\(ext)\t\(id)\tAnyview 生成（导入的类型）")
                }
                continue
            }
            guard let type = UTType(filenameExtension: ext) else {
                lines.append("\(ext)\t-\t没有类型")
                continue
            }
            if type.isDynamic {
                lines.append("\(ext)\t\(type.identifier)\t系统里没有类型，声明不了")
            } else if decodable.contains(type.identifier) {
                let failed = type.conforms(to: .rawImage) || systemOnly.contains(ext) || keepSystem.contains(type.identifier)
                    ? [] : (samples[ext] ?? []).filter { !Self.imageIODecodes($0) }
                if let first = failed.first {
                    if !claims.contains(type.identifier) { claims.append(type.identifier) }
                    let more = failed.count > 1 ? " 等 \(failed.count) 个" : ""
                    lines.append("\(ext)\t\(type.identifier)\tAnyview 生成（系统解不了测试图片 \(first.lastPathComponent)\(more)）")
                } else {
                    let tested = samples[ext].map { "，测试图片 \($0.count) 个都能解" } ?? ""
                    lines.append("\(ext)\t\(type.identifier)\t系统解码\(tested)")
                }
            } else if type.conforms(to: .rawImage) || type.conforms(to: .svg) {
                // ImageIO 的列表里没有、Quick Look 却自己会做的：RAW 用相机内嵌的预览，
                // SVG 用系统自己的（声明了也是系统优先，见上）。不抢系统的。
                lines.append("\(ext)\t\(type.identifier)\t系统生成（Quick Look 自己会做）")
            } else {
                if !claims.contains(type.identifier) { claims.append(type.identifier) }
                lines.append("\(ext)\t\(type.identifier)\tAnyview 生成")
            }
        }
        do {
            try (claims.joined(separator: "\n") + "\n").write(toFile: arguments[2], atomically: true, encoding: .utf8)
            try (lines.joined(separator: "\n") + "\n").write(toFile: arguments[3], atomically: true, encoding: .utf8)
            if arguments.count == 6 {
                let previewClaims = claims.filter { !previewSkip.contains($0) }
                try (previewClaims.joined(separator: "\n") + "\n").write(toFile: arguments[5], atomically: true, encoding: .utf8)
            }
        } catch {
            FileHandle.standardError.write(Data("写不了结果：\(error)\n".utf8))
            exit(1)
        }
        print("访达缩略图扩展声明 \(claims.count) 种类型：\(claims.joined(separator: " "))")
    }

    /// 测试图片里每个扩展名（小写）有哪些文件。跳过故意弄坏的 broken/、扩展名与内容不符的 misc/、压缩包、
    /// 1 万张的 many/ 和超大图 huge/。
    static func samples(in directory: String) -> [String: [URL]] {
        let skip: Set<String> = ["broken", "misc", "archives", "many", "huge", "limits"]
        var result: [String: [URL]] = [:]
        guard let walker = FileManager.default.enumerator(
            at: URL(fileURLWithPath: directory), includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) else { return result }
        for case let url as URL in walker {
            if (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true {
                if skip.contains(url.lastPathComponent) { walker.skipDescendants() }
                continue
            }
            let ext = url.pathExtension.lowercased()
            if !ext.isEmpty { result[ext, default: []].append(url) }
        }
        return result
    }

    /// ImageIO 能不能把这个文件解成图（Quick Look 给系统认识的图片格式做缩略图用的就是它）。
    static func imageIODecodes(_ url: URL) -> Bool {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return false }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceThumbnailMaxPixelSize: 256,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) != nil
    }
}
