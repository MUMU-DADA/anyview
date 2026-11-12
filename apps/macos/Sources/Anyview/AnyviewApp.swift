import AppKit
import AnyviewCore

@main
enum AnyviewApp {
    static func main() {
        // 尽早读取时钟，作为启动耗时的起点。
        _ = LaunchClock.mainStart
        let options = LaunchOptions.parse(CommandLine.arguments)
        MainActor.assumeIsolated {
            LaunchClock.mark("main")
            let app = NSApplication.shared
            LaunchClock.mark("appCreated")
            let delegate = AppDelegate(options: options)
            app.delegate = delegate
            app.setActivationPolicy(.regular)
            // NSApplication.delegate 是弱引用，运行期间要保证 delegate 存活。
            withExtendedLifetime(delegate) {
                app.run()
            }
        }
    }
}

/// 进程内的单调时钟起点，用于测量启动耗时。
/// 启动过程中各阶段打上时间点（从 main 开始算的毫秒），自检报告里写出，看时间花在哪里。
@MainActor
enum LaunchClock {
    nonisolated static let mainStart: UInt64 = DispatchTime.now().uptimeNanoseconds
    private(set) static var marks: [(name: String, ms: Double)] = []

    nonisolated static func millisecondsSinceMainStart() -> Double {
        Double(DispatchTime.now().uptimeNanoseconds - mainStart) / 1_000_000
    }

    /// 记一个时间点（同名的只记第一次）。
    static func mark(_ name: String) {
        guard !marks.contains(where: { $0.name == name }) else { return }
        marks.append((name, millisecondsSinceMainStart()))
    }
}

/// 命令行参数。从访达打开的文件不经过这里，由 application(_:open:) 处理。
struct LaunchOptions {
    /// 自检模式：写出报告和截图后退出（tools/macos/编译测试.command 使用）。
    var selfTestDir: URL?
    /// 启动后打开的图片或文件夹。
    var openPath: String?
    /// 自检时依次执行的动作 ID，如 rotate_cw,zoom_in,next_image（也接受 next、prev、first、last、forward10、back10）。
    var steps: [String] = []
    /// 自检时只在 100% 时核对画面（真实照片）。
    var sampleAt100 = false
    /// 设置文件（--settings）。自检时没给就用报告目录里的 settings.toml，不碰你自己的设置。
    var settingsPath: String?

    static func parse(_ arguments: [String]) -> LaunchOptions {
        var options = LaunchOptions()
        var index = 1
        while index < arguments.count {
            let argument = arguments[index]
            let value: String? = index + 1 < arguments.count ? arguments[index + 1] : nil
            switch argument {
            case "--self-test":
                if let value {
                    options.selfTestDir = URL(fileURLWithPath: value, isDirectory: true)
                }
                index += 2
            case "--open":
                options.openPath = value
                index += 2
            case "--steps":
                options.steps = (value ?? "").split(separator: ",").map { actionID(for: String($0)) }.filter { !$0.isEmpty }
                index += 2
            case "--sample-at-100":
                options.sampleAt100 = true
                index += 1
            case "--settings":
                options.settingsPath = value
                index += 2
            default:
                // 跳过系统附加的"-NSKey 值"形式的参数。
                index += argument.hasPrefix("-NS") || argument.hasPrefix("-Apple") ? 2 : 1
            }
        }
        if options.settingsPath == nil, let dir = options.selfTestDir {
            options.settingsPath = dir.appendingPathComponent("settings.toml").path
        }
        return options
    }

    /// 自检步骤名 → 动作 ID（保留常用简写）。
    static func actionID(for step: String) -> String {
        let name = step.trimmingCharacters(in: .whitespaces)
        switch name {
        case "next": return "next_image"
        case "prev": return "prev_image"
        case "first": return "first_image"
        case "last": return "last_image"
        case "forward10": return "forward_10"
        case "back10": return "back_10"
        default: return name
        }
    }
}
