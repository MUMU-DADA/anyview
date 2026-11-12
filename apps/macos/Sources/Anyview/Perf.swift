import Foundation
import os

/// 运行时性能记录：写入系统日志并记录 os_signpost。
/// 查看：在"控制台"里搜索子系统 local.anyview.Anyview（类别 perf）；或用 Instruments 的 os_signpost 工具看
/// "ShowImage"（从切图到清晰图显示）的时间区间，以及 "Preview"、"ExactRender" 事件。不写文件，不联网。
@MainActor
enum Perf {
    static let log = Logger(subsystem: "local.anyview.Anyview", category: "perf")
    static let signposter = OSSignposter(logger: log)
    private static var showing: OSSignpostIntervalState?
    private static var showStarted: UInt64 = 0
    private static var launchLogged = false

    /// 开始切到一张图（核心送来 Current）。
    static func beginShow() {
        if let state = showing {
            signposter.endInterval("ShowImage", state)
        }
        showing = signposter.beginInterval("ShowImage", id: signposter.makeSignpostID())
        showStarted = DispatchTime.now().uptimeNanoseconds
    }

    /// 快速预览显示出来了。
    static func previewShown() {
        signposter.emitEvent("Preview")
        let text = String(format: "%.1f", milliseconds(since: showStarted))
        log.info("preview shown after \(text, privacy: .public) ms")
    }

    /// 清晰图显示出来了。第一次时另记从 main 开始的启动耗时。
    static func imageShown(decodeMs: Double, width: UInt32, height: UInt32) {
        if let state = showing {
            signposter.endInterval("ShowImage", state)
            showing = nil
        }
        let shown = String(format: "%.1f", milliseconds(since: showStarted))
        let decode = String(format: "%.1f", decodeMs)
        log.info("image \(width)x\(height) shown after \(shown, privacy: .public) ms (decode \(decode, privacy: .public) ms)")
        if !launchLogged {
            launchLogged = true
            let launch = String(format: "%.1f", LaunchClock.millisecondsSinceMainStart())
            log.info("first image \(launch, privacy: .public) ms after main")
        }
    }

    /// 精确重绘完成。
    static func exactShown(renderMs: Double) {
        signposter.emitEvent("ExactRender")
        let text = String(format: "%.1f", renderMs)
        log.debug("exact render \(text, privacy: .public) ms")
    }

    private static func milliseconds(since start: UInt64) -> Double {
        Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
    }
}
