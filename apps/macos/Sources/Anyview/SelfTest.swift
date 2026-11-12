import AppKit
import AnyviewCore
import UniformTypeIdentifiers

/// 自检模式（tools/macos/编译测试.command 使用），写出 selftest.json 和截图后退出，退出码 0 表示通过。
/// - 不带 --open：空态自检。收到 Pong 后截图，核对背景色 #202020。
/// - 带 --open：打开路径，再按 --steps 依次执行动作（如 next_image、rotate_cw、zoom_in）。切图的步骤等图像就绪，
///   其他步骤等画面更新，然后截图。每张截图都做"画面核对"：把图片上 16 个点按核心算出的位置映射到屏幕，
///   读截图里的颜色与原图比较，验证平移、缩放、旋转、翻转确实按核心的计算画出来。
///   透明处应显示棋盘格，半透明的点和被悬浮界面挡住的点跳过。每一步还核对悬浮界面：控制栏只在锁定时显示
///   （自检时指针不动），图片信息的开关和文字与核心一致，进度条滑块与序号一致。
///   步骤 `bar_jump:0.5` 表示在进度条一半处单击（与松开鼠标时的处理相同）。
///   到达首尾时的询问不弹对话框：记录下来，由下一步（如 first_image）充当用户的选择。
///   删除（delete_file）不弹确认框，移到废纸篓后立刻从废纸篓删掉（不留东西），等下一张显示后核对文件已不在原处；
///   复制（copy_to_clipboard）写到自检专用的剪贴板（不动你的剪贴板），核对里面同时有文件和图像。
///   步骤 `wait_exact` 等精确重绘盖上画面（最多 3 秒），再截图核对；记录位置和重绘耗时。
///   `hover:动作` 像指针停在控制栏的这个按钮上（悬停高亮，；`hover:strip` 进度条，`hover:none` 都不停）。
///   性能测量：`wait:毫秒` 等一会儿；`idle:毫秒` 测这段时间的 CPU 占用和内存峰值；
///   `open:路径` 在运行中打开新的图片或文件夹。第一张图记录从进程启动到出图的时间（冷启动）。
///   大 JPEG 先显示快速预览时另记出预览的时间；截图等清晰图。
///   超大图（代理纹理）需要精确重绘时，等它盖上再截图（最多 3 秒）。
///   --sample-at-100：只在 100% 时核对画面（真实照片缩小显示时，滤波后的颜色本来就与单个像素不同）。
///   任何提示（如已到末尾）、动作不可用或核对不符都算不通过。
/// 设置（docs/SETTINGS.md）：`settings_open` 打开设置窗口，`settings_page:页` 切页，`settings_set:键=值` 像用户那样
/// 改控件（开关 true/false、下拉框选项 ID、数字和路径的文字、颜色 #RRGGBB），`settings_ok` 点"确定"（要求生效），
/// `settings_ok_invalid` 点"确定"（要求校验失败），`settings_reset` 点"重置"，`settings_cancel` 放弃修改并关窗口。
/// 设置窗口开着时每一步另截一张设置窗口的图，并记下这一页每一项的文字、是否可用和值。
/// `key:按键` 模拟按下一个键（交给核心解析，记下对应的动作）；`hover_bottom` 模拟指针移到窗口底部。
/// 多个窗口：`finder_open:路径` 像从访达打开一样（按规则开新窗口、用空窗口或复用当前窗口），等那个窗口出图；
/// `key_in:窗口序号:按键` 在第几个窗口里按键（如 key_in:1:Escape 关掉第二个窗口）；
/// 吸附边缘：`move_window:距离:snap|stay` 把窗口放到离屏幕可用区域左边、上边各"距离"点处，按拖完标题栏处理，
/// 核对贴上去了（snap）或没动（stay）；`drag_window:距离:snap|stay` 用模拟的鼠标拖动画面移动窗口到同样的位置。
/// Esc 在最后一个窗口里会退出程序：自检时不真的退出，记下这一步后结束自检。
/// 每一步都记下所有窗口的名字、大小、位置和是否一直显示标题栏。
/// 报告里还写出菜单栏和右键菜单的实际内容（标题、快捷键、勾选、灰显）。
/// 结束前模拟在画面中央按下鼠标右键（经过窗口的事件分发），确认右键菜单真的弹出、只弹出一次，再自动关掉。
@MainActor
final class SelfTest {
    private let reportDir: URL
    private let controller: MainWindowController
    private let core: CoreBridge
    private let openPath: String?
    private let steps: [String]
    private let sampleAt100: Bool
    /// 这一步出快速预览的时间（毫秒，从这一步开始算）和从进程启动算的时间。
    private var previewMs: Double?
    private var previewSinceProcess: Double?
    private var records: [[String: Any]] = []
    private var failure: String?
    private var stepStarted = DispatchTime.now().uptimeNanoseconds
    private let started = DispatchTime.now().uptimeNanoseconds
    /// 正在等图像就绪的步骤（打开或切图）。
    private var waitingImage = false
    private var finished = false
    private var currentImage: ImageHandle?
    /// 刚移到废纸篓的文件：下一张显示后核对它已不在原处。
    private var deletedPath: String?
    /// 精确重绘出现的次数和最长耗时（毫秒）。
    private var exactCount = 0
    private var exactWorstMs = 0.0
    /// 现在的设置窗口（由应用给出）。
    var settingsWindow: (() -> SettingsWindowController?)?
    /// 最近一次"确定"生效的结果。
    private var lastApplied: SettingsAppliedInfo?
    /// 模拟了指针移到窗口底部（hover_bottom）：控制栏应当显示（除非设置了不显示）。
    private var hoverBar = false
    /// 这一步要等图像就绪、到时一起写进记录的内容（按键、鼠标步骤执行了翻页时）。
    private var pendingExtra: [String: Any]?
    /// 应用的所有主窗口。
    var appWindows: (() -> [MainWindowController])?
    /// 像从访达打开一样打开：返回用的窗口和用法（new、empty、reused、reopened）。
    var finderOpen: ((URL) -> (controller: MainWindowController, how: String)?)?
    /// 正在执行、可能让程序退出的一步（Esc）：退出时记下它。
    private var pendingQuitRecord: [String: Any]?
    /// 这一步开始以后收到的动图帧数（`wait_frames`）。
    private var framesSeen = 0
    /// 等复制完成的提示（copy_to_folder1 等）。
    private var waitingNotice = false
    /// 最近一次复制、移动完成的提示。
    private var lastNotice: String?
    /// 交给外部的操作（编辑器、桌面图片、打印）：自检时只记下来。
    private var externals: [[String: Any]] = []
    /// 现在有没有在阻止显示器睡眠，由应用给出。
    var preventingSleep: (() -> Bool)?
    /// 应用打开着的帮助窗口。
    var helpWindows: (() -> [HelpWindowController])?
    /// 开着的“生成信息”面板。
    var generationPanel: (() -> GenerationPanelController?)?
    /// 上一张和这一张图片中心的颜色（RGB；核对翻页效果的中间画面）。
    private var previousColor: [Int]?
    private var currentColor: [Int]?

    /// 画面核对的颜色容差（每个通道）。采样点离色块边界较远，只有缩放滤波带来的细小差别。
    private static let colorTolerance = 40

    /// 启动时已经交给核心的打开的时刻；为空时由自检自己打开。
    private let openDispatchedAt: UInt64?

    init(reportDir: URL, controller: MainWindowController, core: CoreBridge, openPath: String?, steps: [String],
         sampleAt100: Bool, openDispatchedAt: UInt64?) {
        self.reportDir = reportDir
        self.controller = controller
        self.core = core
        self.openPath = openPath
        self.steps = steps
        self.sampleAt100 = sampleAt100
        self.openDispatchedAt = openDispatchedAt
    }

    func start() {
        try? FileManager.default.createDirectory(at: reportDir, withIntermediateDirectories: true)
        controller.onPong = { [weak self] in
            self?.pongReceived()
        }
        controller.onImageShown = { [weak self] image in
            self?.imageShown(image)
        }
        controller.onPreviewShown = { [weak self] in
            self?.previewShown()
        }
        controller.onAnimationFrame = { [weak self] in
            self?.framesSeen += 1
        }
        controller.onNotice = { [weak self] message in
            self?.noticeArrived(message)
        }
        controller.onExternal = { [weak self] what, detail in
            self?.externals.append(["what": what, "detail": detail])
        }
        controller.onProblem = { [weak self] message in
            self?.problem(message)
        }
        controller.onBoundary = { [weak self] forward in
            self?.boundaryReached(forward: forward) ?? false
        }
        controller.onLimit = { [weak self] in
            self?.limitReached() ?? false
        }
        core.ping()
        if let openPath {
            waitingImage = true
            if let dispatched = openDispatchedAt {
                // 启动时已经打开了：从那时开始计时。
                stepStarted = dispatched
            } else {
                stepStarted = DispatchTime.now().uptimeNanoseconds
                core.open(path: openPath)
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 120) { [weak self] in
            MainActor.assumeIsolated {
                if let self {
                    self.fail("超时")
                }
            }
        }
    }

    /// 设置窗口的"确定"已经生效。
    func settingsApplied(_ applied: SettingsAppliedInfo) {
        lastApplied = applied
    }

    /// 程序要退出了（Esc 在最后一个窗口里）：记下这一步，写报告后结束。
    func quitRequested() {
        guard !finished else { return }
        var record = pendingQuitRecord ?? ["step": stepLabel, "name": controller.currentName]
        pendingQuitRecord = nil
        record["quit"] = true
        record["windows"] = windowsInfo()
        records.append(record)
        finish()
    }

    /// 所有主窗口的样子。
    private func windowsInfo() -> [[String: Any]] {
        (appWindows?() ?? [controller]).map { each in
            let content = each.window?.contentView?.bounds.size ?? .zero
            let frame = each.window?.frame ?? .zero
            return [
                "name": each.currentName,
                "closed": each.isClosed,
                "visible": each.window?.isVisible ?? false,
                "key": each.window?.isKeyWindow ?? false,
                "contentPt": [content.width, content.height],
                "framePt": [frame.minX, frame.minY, frame.width, frame.height],
                "titleBarAlwaysShown": each.titleBarAlwaysShown,
                "titleBarShown": each.titlebar.isShown,
                "background": String(format: "#%06X", each.uiSettings.backgroundColor),
            ] as [String: Any]
        }
    }

    private func pongReceived() {
        guard openPath == nil else { return }
        // 空态自检：再等 0.3 秒，让窗口完成布局和首帧绘制。
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            MainActor.assumeIsolated {
                if let self {
                    self.finish()
                }
            }
        }
    }

    private var stepLabel: String {
        records.isEmpty ? "open" : steps[records.count - 1]
    }

    /// 快速预览显示出来了：记下时间，继续等清晰图。
    private func previewShown() {
        guard !finished, waitingImage, previewMs == nil else { return }
        previewMs = Self.milliseconds(since: stepStarted)
        if records.isEmpty {
            previewSinceProcess = Self.processAgeMs()
            LaunchClock.mark("firstPreviewShown")
        }
    }

    private func imageShown(_ image: ImageHandle) {
        guard !finished, waitingImage else { return }
        waitingImage = false
        currentImage = image
        previousColor = currentColor
        currentColor = Self.centerColor(image)
        let deletedGone: Any = deletedPath.map { !FileManager.default.fileExists(atPath: $0) as Any } ?? NSNull()
        deletedPath = nil
        var record: [String: Any] = [
            "deletedGone": deletedGone,
            "step": stepLabel,
            "name": controller.currentName,
            "index": controller.currentIndex.map { Int($0) as Any } ?? NSNull(),
            "total": controller.currentTotal.map { Int($0) as Any } ?? NSNull(),
            "msToImage": Self.milliseconds(since: stepStarted),
            "decodeMs": image.decodeMs(),
            "width": Int(image.width()),
            "height": Int(image.height()),
            "format": image.formatName(),
            "pixelFormat": image.pixelFormat(),
            "hasProxy": image.hasProxy(),
        ]
        // 第一张图：从进程启动、从 main 开始到出图的时间（冷启动）。没有的值不写（脚本用 plutil 读不了 null）。
        if records.isEmpty {
            // 启动时的提示（如设置文件坏了）。
            record["notice"] = controller.noticeText ?? ""
            LaunchClock.mark("firstImageShown")
            if let age = Self.processAgeMs() {
                record["msSinceProcessStart"] = age
            }
            record["msSinceMain"] = LaunchClock.millisecondsSinceMainStart()
            if let preview = previewSinceProcess {
                record["previewMsSinceProcessStart"] = preview
            }
        }
        if let preview = previewMs {
            record["msToPreview"] = preview
        }
        if let extra = pendingExtra {
            record.merge(extra) { _, new in new }
            pendingExtra = nil
        }
        previewMs = nil
        previewSinceProcess = nil
        // 窗口跟着图片：记下按这张图调整的窗口大小（截图时统一核对）。
        if let fit = controller.lastFitSize {
            record["fitWindowPt"] = [fit.width, fit.height]
        }
        records.append(record)
        captureLater()
    }

    /// 等窗口完成布局和绘制再截图，然后进行下一步。
    private func captureLater() {
        // 留出淡入（150 ms）的时间。
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { [weak self] in
            MainActor.assumeIsolated {
                self?.captureWhenSettled(started: DispatchTime.now().uptimeNanoseconds)
            }
        }
    }

    /// 超大图（代理纹理）放大显示时 GPU 画面本来就不清晰：等精确重绘盖上再截图（最多 3 秒）。
    private func captureWhenSettled(started: UInt64) {
        guard !finished else { return }
        let proxied = controller.canvas.currentHandle?.hasProxy() ?? false
        // 翻页效果在走（没有停住）时等它做完。
        let turning = controller.canvas.transitionState != nil && controller.canvas.holdTransition == nil
        let pending = turning || (proxied && core.core.needsExact() && controller.canvas.exactShown == nil)
        if !pending || Self.milliseconds(since: started) > 3000 {
            captureStep()
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
            MainActor.assumeIsolated {
                self?.captureWhenSettled(started: started)
            }
        }
    }

    private func captureStep() {
        guard !finished, !records.isEmpty else { return }
        let step = records.count - 1
        let shot = capture(to: reportDir.appendingPathComponent("step-\(step).png"))
        records[step]["screenshot"] = shot.info
        // 窗口左上角的颜色：图片没盖住时就是背景色（设置里可改）。
        records[step]["cornerColor"] = shot.info["cornerColor"] ?? ""
        // 设置窗口开着时另截一张，并记下这一页的内容。
        if let settings = settingsWindow?() {
            records[step]["settings"] = [
                "screenshot": settings.snapshot(to: reportDir.appendingPathComponent("step-\(step)-settings.png")),
                "page": settings.pageSummary(),
                "dirty": core.core.settingsDirty(),
            ] as [String: Any]
        }
        // 选择图片窗口开着时另截一张，并记下它的状态。
        if let picker = controller.picker {
            var info = pickerInfo(picker)
            info["screenshot"] = picker.snapshot(to: reportDir.appendingPathComponent("step-\(step)-picker.png"))
            records[step]["picker"] = info
        }
        records[step]["layout"] = Self.layoutInfo(core.core.viewLayout())
        records[step]["windows"] = windowsInfo()
        records[step]["windowCount"] = Int(core.core.windowCount())
        // 压缩包里不能删除；记住的压缩包位置有几个；最近一次复制、移动的提示；阻止屏保。
        records[step]["deleteEnabled"] = core.core.actionState(id: "delete_file").enabled
        records[step]["archivePositions"] = Int(core.core.archivePositionsCount())
        if let lastNotice {
            records[step]["lastNotice"] = lastNotice
        }
        records[step]["preventingSleep"] = preventingSleep?() ?? false
        // 菜单栏、程序坞的自动隐藏（即时全屏时打开，；整个程序共用）：0 是平常的样子。
        records[step]["presentation"] = Int(NSApp.presentationOptions.rawValue)
        // 动图：播放状态和左下角动画信息控制的文字。
        if let animation = core.core.animation() {
            var info: [String: Any] = ["index": Int(animation.index), "playing": animation.playing]
            if let count = animation.count {
                info["count"] = Int(count)
            }
            records[step]["animation"] = info
        }
        if let text = controller.overlays.animationText {
            records[step]["animationText"] = text
        }
        // 翻页效果停在中间时（transition_hold），画面是两张叠在一起的：按效果核对几个点，不做平常的画面核对。
        let samples = controller.canvas.transitionState.map { transitionCheck(shot.context, state: $0) }
            ?? checkSamples(shot.context)
        records[step]["samples"] = samples.info
        // 翻页音效放过几次、用的哪种方式。
        records[step]["sounds"] = controller.soundLog
        // 背景图片、标题栏隐藏时显示的文件名。
        records[step]["background"] = controller.canvas.hasBackgroundImage
        records[step]["filenameLabel"] = controller.filenameLabel.isHidden ? "" : controller.filenameLabel.stringValue
        let overlay = overlayState()
        records[step]["overlays"] = overlay.info
        // 左键拖动这时会做什么：移动窗口还是平移图片。
        records[step]["leftDragMovesWindow"] = core.core.leftDragMovesWindow(windowMovable: controller.windowMovable)
        // 窗口大小：程序设定过窗口大小时，窗口实际就该是这个大小（切图、旋转、最大化、模拟拖动之后都核对）。
        if let content = controller.window?.contentView?.bounds.size {
            records[step]["windowPt"] = [content.width, content.height]
            if let expected = controller.expectedContentSize {
                records[step]["expectedWindowPt"] = [expected.width, expected.height]
                if abs(expected.width - content.width) > 1 || abs(expected.height - content.height) > 1 {
                    fail("第 \(step) 步窗口大小 \(content.width)x\(content.height)，应为 \(expected.width)x\(expected.height)")
                    return
                }
            }
        }
        guard samples.skipped || (samples.checked > 0 && samples.mismatches == 0) else {
            fail("第 \(step) 步画面核对不符")
            return
        }
        guard overlay.problems.isEmpty else {
            fail("第 \(step) 步悬浮界面不对：\(overlay.problems.joined(separator: "；"))")
            return
        }
        advance()
    }

    /// 执行下一个动作；没有了就结束。
    private func advance() {
        stepStarted = DispatchTime.now().uptimeNanoseconds
        framesSeen = 0
        let index = records.count - 1
        guard index < steps.count else {
            finish()
            return
        }
        let id = steps[index]
        if id == "wait_exact" {
            waitExact(started: DispatchTime.now().uptimeNanoseconds)
            return
        }
        // hover:动作 模拟指针停在控制栏的这个按钮上（悬停高亮，供截图核对；不动真的指针）；
        // hover:strip 停在进度条上，hover:none 都不停。
        if id.hasPrefix("hover:") {
            if controller.overlays.simulateHover(String(id.dropFirst("hover:".count))) {
                records.append(["step": id, "name": controller.currentName])
                captureLater()
            } else {
                fail("没有这个控件：\(id)")
            }
            return
        }
        if id.hasPrefix("wait:"), let ms = Double(id.dropFirst("wait:".count)) {
            DispatchQueue.main.asyncAfter(deadline: .now() + ms / 1000) { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, !self.finished else { return }
                    self.records.append(["step": id, "name": self.controller.currentName])
                    self.captureLater()
                }
            }
            return
        }
        if id.hasPrefix("idle:"), let ms = Double(id.dropFirst("idle:".count)) {
            // 静止不操作时 CPU 占用应接近 0%（没有定时重绘）。
            let before = Self.cpuSeconds()
            DispatchQueue.main.asyncAfter(deadline: .now() + ms / 1000) { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, !self.finished else { return }
                    let percent = (Self.cpuSeconds() - before) / (ms / 1000) * 100
                    self.records.append([
                        "step": id,
                        "name": self.controller.currentName,
                        "idleCpuPercent": percent,
                        "peakRssMb": Self.peakRssMb(),
                        // 静止时的内存占用（与"活动监视器"的"内存"一栏相同）；峰值里还有自检截图等临时内存。
                        "footprintMb": Self.footprintMb() ?? -1,
                        // 图片缓存占用（遵守缓存上限，默认 1024 MB）。
                        "cacheMb": Double(self.core.core.cacheBytes()) / 1_048_576,
                    ])
                    self.captureLater()
                }
            }
            return
        }
        // 被上限拦下后用户的选择：仍然打开（等图片显示）、取消。
        if id == "limit_open" || id == "limit_cancel" {
            guard let path = core.core.currentPath() else {
                fail("没有当前图片")
                return
            }
            if id == "limit_open" {
                waitingImage = true
                core.core.openWithoutLimits(path: path)
            } else {
                core.core.keepLimits(path: path)
                records.append(["step": id, "name": controller.currentName])
                captureLater()
            }
            return
        }
        if id.hasPrefix("open:") {
            waitingImage = true
            core.open(path: String(id.dropFirst("open:".count)))
            return
        }
        // window:宽x高：把窗口设成这个大小（点），模拟用户拖动窗口边缘。
        if id.hasPrefix("window:") {
            let parts = id.dropFirst("window:".count).split(separator: "x").compactMap { Double($0) }
            guard parts.count == 2 else {
                fail("步骤写错了：\(id)")
                return
            }
            controller.setContentSizeForTest(NSSize(width: parts[0], height: parts[1]))
            records.append(["step": id, "name": controller.currentName])
            captureLater()
            return
        }
        if windowStep(id) {
            return
        }
        // add_file:源文件、remove_file:文件名：在当前图片的文件夹里加一个文件、删一个文件，
        // 等浏览集合的张数变了（文件夹变化自动刷新，最多 6 秒）。
        if id.hasPrefix("add_file:") || id.hasPrefix("remove_file:") {
            guard let current = core.core.currentPath() else {
                fail("没有当前文件夹")
                return
            }
            let folder = URL(fileURLWithPath: current).deletingLastPathComponent()
            do {
                if id.hasPrefix("add_file:") {
                    let source = URL(fileURLWithPath: String(id.dropFirst("add_file:".count)))
                    try FileManager.default.copyItem(at: source, to: folder.appendingPathComponent(source.lastPathComponent))
                } else {
                    try FileManager.default.removeItem(at: folder.appendingPathComponent(String(id.dropFirst("remove_file:".count))))
                }
            } catch {
                fail("\(id) 失败：\(error.localizedDescription)")
                return
            }
            waitTotal(before: controller.currentTotal, label: id, started: DispatchTime.now().uptimeNanoseconds)
            return
        }
        // wait_frames:N：等动图再换 N 帧（最多 5 秒），记下看到的帧数。
        if id.hasPrefix("wait_frames:"), let wanted = Int(id.dropFirst("wait_frames:".count)) {
            waitFrames(wanted, label: id, started: DispatchTime.now().uptimeNanoseconds)
            return
        }
        if settingsStep(id) {
            return
        }
        if pickerStep(id) {
            return
        }
        // file_thumbnails:目录：用核心给这个目录里每个支持的文件生成访达缩略图（长边 256，；
        // 与缩略图扩展调用的是同一个函数），记下生成了几张、哪几个失败、最慢多久。
        if id.hasPrefix("file_thumbnails:") {
            let dir = String(id.dropFirst("file_thumbnails:".count))
            let supported = Set(supportedExtensions())
            let names = ((try? FileManager.default.contentsOfDirectory(atPath: dir)) ?? [])
                .filter { supported.contains(($0 as NSString).pathExtension.lowercased()) }
                .sorted()
            var made = 0
            var failed: [String] = []
            var slowest = 0.0
            var sizes: [String: String] = [:]
            for name in names {
                let started = Date()
                if let thumb = makeFileThumbnail(path: (dir as NSString).appendingPathComponent(name), maxSide: 256) {
                    made += 1
                    sizes[name] = "\(thumb.width())x\(thumb.height())"
                } else {
                    failed.append(name)
                }
                slowest = max(slowest, Date().timeIntervalSince(started) * 1000)
            }
            records.append(["step": id, "name": controller.currentName,
                            "thumbnails": ["dir": dir, "total": names.count, "made": made, "failed": failed,
                                           "slowestMs": slowest, "sizes": sizes]])
            captureLater()
            return
        }
        // open_with：每种支持的扩展名都问一次系统（LaunchServices）能用哪些程序打开，核对里面有这一份 Anyview
        // （访达的"打开方式"、"设为默认看图软件"都靠它；Info.plist 的文档类型）。只记录、截图，不改系统设置。
        if id == "open_with" {
            records.append(["step": id, "name": controller.currentName, "openWith": Self.openWithReport()])
            captureLater()
            return
        }
        // help:shortcuts、help:formats、help:licenses 打开帮助窗口，截一张帮助窗口的图、记下行数
        // （开源许可记下文字有多长、有没有 Anyview 的 MIT 许可证），然后关掉它。
        // generation:state 记下“生成信息”面板开着没有、显示的是什么（开着时截面板的图）；generation:close 关掉它。
        // 面板由 key:P（或菜单）打开。
        if id.hasPrefix("generation:") {
            if id == "generation:close" {
                generationPanel?()?.window?.performClose(nil)
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in
                MainActor.assumeIsolated {
                    self?.recordGeneration(id)
                }
            }
            return
        }
        if id.hasPrefix("help:") {
            let action = ["help:shortcuts": "help_shortcuts", "help:licenses": "help_licenses"][id] ?? "help_formats"
            controller.invoke(action)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in
                MainActor.assumeIsolated {
                    self?.recordHelpWindow(id)
                }
            }
            return
        }
        // transition_hold:进度 让下一次翻页效果停在这个进度（截图核对中间画面）；transition_hold:off 取消。
        if id.hasPrefix("transition_hold:") {
            if let progress = Double(id.dropFirst("transition_hold:".count)) {
                controller.canvas.holdTransition = progress
            } else {
                controller.canvas.releaseHeldTransition()
            }
            records.append(["step": id, "name": controller.currentName])
            captureLater()
            return
        }
        if id.hasPrefix("bar_jump:"), let fraction = Double(id.dropFirst("bar_jump:".count)) {
            if controller.overlays.strip.jump(toFraction: fraction) {
                waitingImage = true
            } else {
                fail("进度条还不能跳转：\(id)")
            }
            return
        }
        runAction(id, label: id, extra: [:])
    }

    /// 执行一个动作作为这一步（label 是这一步的写法），按结果等待图像或记录、截图。extra 一起写进记录。
    private func runAction(_ id: String, label: String, extra: [String: Any]) {
        let outcome = core.core.invokeAction(id: id)
        var record: [String: Any] = ["step": label, "name": controller.currentName]
        record.merge(extra) { _, new in new }
        switch outcome {
        case .started where id == "close_image":
            // 关闭图片：回到空态，没有新图像要等。
            records.append(record)
            captureLater()
        case .started where id.hasPrefix("copy_to_folder"):
            // 复制到图片文件夹：在后台复制，等完成的提示。
            waitingNotice = true
            pendingExtra = record
        case .started:
            waitingImage = true
            pendingExtra = extra
        case .viewChanged, .overlayChanged:
            controller.apply(outcome)
            // 与界面执行动作时相同：旋转后窗口跟着图片转。
            controller.followAfterAction(id)
            records.append(record)
            captureLater()
        case let .ui(request):
            switch request {
            case .toggleMaximize:
                controller.toggleMaximize()
                record["maximized"] = controller.maximized
                records.append(record)
                captureLater()
            case .toggleFullscreen:
                // 即时全屏：进出都没有动画，截图时窗口已经是新的大小。
                controller.apply(outcome)
                record["fullscreen"] = controller.instantFullScreen
                records.append(record)
                captureLater()
            case .showGenerationInfo:
                // “生成信息”面板：像用户按 P 一样打开面板，再记录（面板的内容由 generation:state 记）。
                controller.apply(outcome)
                records.append(record)
                captureLater()
            case .openInEditor, .setWallpaper, .print:
                // 编辑器、桌面图片、打印：自检时只记下交给外部的内容。
                controller.apply(outcome)
                record["external"] = externals.last ?? [:]
                records.append(record)
                captureLater()
            case .closeOrQuit:
                // Esc：只有这一个窗口时退出（自检时记下后结束）。
                pendingQuitRecord = record
                controller.apply(outcome)
                guard !finished else { return }
                pendingQuitRecord = nil
                records.append(record)
                captureLater()
            case let .trashFile(path, _, _):
                deletedPath = path
                waitingImage = true
                controller.apply(outcome)
            case .copyImage:
                controller.apply(outcome)
                let types = controller.lastCopyTypes
                let hasFile = types.contains(NSPasteboard.PasteboardType.fileURL.rawValue)
                let hasImage = types.contains(NSPasteboard.PasteboardType.tiff.rawValue)
                    || types.contains(NSPasteboard.PasteboardType.png.rawValue)
                // 剪贴板里的是原图，不是画面上加了反相等效果的图像。
                let copied = controller.lastCopyCenter
                let original = core.core.originalImage().flatMap { Self.centerColor($0) }
                let displayed = controller.canvas.currentHandle.flatMap { Self.centerColor($0) }
                records.append(["step": id, "name": controller.currentName,
                                "clipboard": ["files": hasFile, "image": hasImage, "types": types,
                                              "centerRGB": copied ?? [], "originalCenterRGB": original ?? [],
                                              "displayedCenterRGB": displayed ?? [],
                                              "colorSpace": controller.lastCopyColorSpace ?? ""]])
                guard hasFile, hasImage else {
                    fail("剪贴板内容不全：文件 \(hasFile)，图像 \(hasImage)")
                    return
                }
                guard let copied, let original, zip(copied, original).allSatisfy({ abs($0 - $1) <= 3 }) else {
                    fail("剪贴板里的图像不是原图：正中 \(copied ?? [])，原图 \(original ?? [])，画面 \(displayed ?? [])")
                    return
                }
                captureLater()
            default:
                fail("动作 \(id) 没有执行：\(outcome)")
            }
        case .unavailable:
            fail("动作 \(id) 没有执行：\(outcome)")
        }
    }

    /// 多个窗口、吸附边缘的步骤；不是这类步骤时返回 false。
    private func windowStep(_ id: String) -> Bool {
        // 最小化、取回（窗口看不见时不阻止显示器睡眠）。有动画，等它做完再记；最小化的窗口不截图核对。
        if id == "miniaturize" || id == "deminiaturize" {
            if id == "miniaturize" {
                controller.window?.miniaturize(nil)
            } else {
                controller.window?.deminiaturize(nil)
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, !self.finished else { return }
                    self.records.append([
                        "step": id,
                        "name": self.controller.currentName,
                        "miniaturized": self.controller.window?.isMiniaturized ?? false,
                        "preventingSleep": self.preventingSleep?() ?? false,
                    ])
                    self.advance()
                }
            }
            return true
        }
        if id.hasPrefix("finder_open:") {
            let path = String(id.dropFirst("finder_open:".count))
            guard let result = finderOpen?(URL(fileURLWithPath: path)) else {
                fail("没法模拟从访达打开")
                return true
            }
            let extra: [String: Any] = ["how": result.how]
            if result.controller === controller {
                // 在自检看着的这个窗口里换图：与平常切图一样等图像就绪再截图核对。
                waitingImage = true
                pendingExtra = extra
                return true
            }
            waitForWindow(result.controller, name: URL(fileURLWithPath: path).lastPathComponent, extra: extra,
                          started: DispatchTime.now().uptimeNanoseconds)
            return true
        }
        if id.hasPrefix("key_in:") {
            let parts = id.dropFirst("key_in:".count).split(separator: ":", maxSplits: 1).map(String.init)
            let windows = appWindows?() ?? [controller]
            guard parts.count == 2, let index = Int(parts[0]), index < windows.count,
                  let parsed = SettingsWindowController.parseKey(parts[1])
            else {
                fail("步骤写错了，或没有这个窗口：\(id)")
                return true
            }
            let target = windows[index]
            let action = target.appCore.actionForKey(key: parsed.0, modifiers: parsed.1)
            let record: [String: Any] = ["step": id, "name": controller.currentName, "key": parts[1],
                                         "action": action ?? "", "window": index]
            pendingQuitRecord = record
            if let action {
                target.invoke(action)
            }
            guard !finished else { return true }
            pendingQuitRecord = nil
            records.append(record)
            captureLater()
            return true
        }
        if id.hasPrefix("move_window:") || id.hasPrefix("drag_window:") {
            let dragging = id.hasPrefix("drag_window:")
            let parts = id.split(separator: ":").map(String.init)
            guard parts.count == 3, let distance = Double(parts[1]), ["snap", "stay"].contains(parts[2]),
                  let window = controller.window, let visible = window.screen?.visibleFrame
            else {
                fail("步骤写错了，或窗口不在屏幕上：\(id)")
                return true
            }
            let target = NSPoint(x: visible.minX + distance, y: visible.maxY - distance - window.frame.height)
            if dragging {
                dragWindow(window, by: NSPoint(x: target.x - window.frame.minX, y: target.y - window.frame.minY))
            } else {
                window.setFrameOrigin(target)
                controller.finishUserMove()
            }
            let frame = window.frame
            let left = Double(frame.minX - visible.minX)
            let top = Double(visible.maxY - frame.maxY)
            records.append(["step": id, "name": controller.currentName, "left": left, "top": top])
            let snapped = abs(left) < 0.5 && abs(top) < 0.5
            let stayed = abs(left - distance) < 0.5 && abs(top - distance) < 0.5
            guard parts[2] == "snap" ? snapped : stayed else {
                fail("窗口离可用区域左边 \(left) 点、上边 \(top) 点，应\(parts[2] == "snap" ? "贴边" : "不动")")
                return true
            }
            captureLater()
            return true
        }
        return false
    }

    /// 复制、移动完成的提示：等着它的步骤这时记下、截图。
    private func noticeArrived(_ message: String) {
        lastNotice = message
        guard waitingNotice, !finished else { return }
        waitingNotice = false
        var record = pendingExtra ?? ["step": stepLabel, "name": controller.currentName]
        pendingExtra = nil
        record["notice"] = message
        records.append(record)
        captureLater()
    }

    /// 等浏览集合的张数变了（文件夹变化自动刷新）：每 100 ms 看一次，最多 6 秒。
    private func waitTotal(before: UInt32?, label: String, started: UInt64) {
        guard !finished else { return }
        let elapsed = Self.milliseconds(since: started)
        let total = controller.currentTotal
        if total != before || elapsed > 6000 {
            records.append(["step": label, "name": controller.currentName,
                            "total": total.map { Int($0) as Any } ?? NSNull(), "ms": elapsed])
            guard total != before else {
                fail("6 秒内浏览集合没有变")
                return
            }
            captureLater()
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
            MainActor.assumeIsolated {
                self?.waitTotal(before: before, label: label, started: started)
            }
        }
    }

    /// 等动图再换 wanted 帧：每 20 ms 看一次，最多 5 秒。
    private func waitFrames(_ wanted: Int, label: String, started: UInt64) {
        guard !finished else { return }
        let elapsed = Self.milliseconds(since: started)
        if framesSeen >= wanted || elapsed > 5000 {
            records.append(["step": label, "name": controller.currentName, "framesSeen": framesSeen, "ms": elapsed])
            guard framesSeen >= wanted else {
                fail("5 秒内只换了 \(framesSeen) 帧（应至少 \(wanted) 帧）")
                return
            }
            captureLater()
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.02) { [weak self] in
            MainActor.assumeIsolated {
                self?.waitFrames(wanted, label: label, started: started)
            }
        }
    }

    /// 等另一个窗口出图（从访达打开时开的新窗口）：每 50 ms 看一次，最多 5 秒。
    private func waitForWindow(_ target: MainWindowController, name: String, extra: [String: Any], started: UInt64) {
        guard !finished else { return }
        if target.currentName == name, target.canvas.currentHandle != nil, target.window?.isVisible == true {
            var record: [String: Any] = ["step": stepLabel, "name": controller.currentName,
                                         "openedIn": name, "msToImage": Self.milliseconds(since: started)]
            record.merge(extra) { _, new in new }
            // 新窗口的中心在原来窗口中心的右下方，大小按它的图片。
            if let mine = controller.window?.frame, let theirs = target.window?.frame {
                record["centerOffsetPt"] = [theirs.midX - mine.midX, theirs.midY - mine.midY]
            }
            records.append(record)
            if let content = target.window?.contentView?.bounds.size, let expected = target.expectedContentSize,
               abs(content.width - expected.width) > 1 || abs(content.height - expected.height) > 1 {
                fail("新窗口大小 \(content.width)x\(content.height)，应为 \(expected.width)x\(expected.height)")
                return
            }
            captureLater()
            return
        }
        if Self.milliseconds(since: started) > 5000 {
            fail("新窗口 5 秒内没有显示 \(name)")
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
            MainActor.assumeIsolated {
                self?.waitForWindow(target, name: name, extra: extra, started: started)
            }
        }
    }

    /// 模拟按住画面中央拖动 offset（屏幕坐标的点），经过窗口的事件分发（与真的拖动走同样的代码）。
    private func dragWindow(_ window: NSWindow, by offset: NSPoint) {
        let canvas = controller.canvas
        let startInWindow = canvas.convert(NSPoint(x: canvas.bounds.midX, y: canvas.bounds.midY), to: nil)
        let startOnScreen = window.convertPoint(toScreen: startInWindow)
        func send(_ type: NSEvent.EventType, _ screen: NSPoint) {
            let location = window.convertPoint(fromScreen: screen)
            if let event = NSEvent.mouseEvent(
                with: type, location: location, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1,
                pressure: type == .leftMouseUp ? 0 : 1
            ) {
                window.sendEvent(event)
            }
        }
        send(.leftMouseDown, startOnScreen)
        let steps = 8
        for i in 1...steps {
            let t = CGFloat(i) / CGFloat(steps)
            send(.leftMouseDragged, NSPoint(x: startOnScreen.x + offset.x * t, y: startOnScreen.y + offset.y * t))
        }
        send(.leftMouseUp, NSPoint(x: startOnScreen.x + offset.x, y: startOnScreen.y + offset.y))
    }

    /// 选择图片窗口的步骤；不是这类步骤时返回 false。
    /// `picker` 打开窗口（与按 G 相同），`picker_select:N` 选中第 N 行（与单击、方向键相同），
    /// `picker_sort:动作` 在排序菜单里选一项，`picker_hide:on|off` 勾选"隐藏缩略图导航"，
    /// `picker_ok` 点"确定"（等新图片显示），`picker_cancel` 关窗口（不切换）。
    /// 窗口开着时每一步等看得见的缩略图都生成好（最多 5 秒）再截图。
    private func pickerStep(_ id: String) -> Bool {
        let started = DispatchTime.now().uptimeNanoseconds
        if id == "picker" {
            controller.invoke("select_image")
            guard controller.picker != nil else {
                fail("选择图片窗口没有打开")
                return true
            }
            waitPicker(label: id, started: started)
            return true
        }
        guard id.hasPrefix("picker_") else { return false }
        guard let picker = controller.picker else {
            fail("\(id)：选择图片窗口没有打开")
            return true
        }
        if id.hasPrefix("picker_select:") {
            guard let n = Int(id.dropFirst("picker_select:".count)), n >= 0, n < picker.rowCount else {
                fail("步骤写错了：\(id)")
                return true
            }
            picker.select(n)
            waitPicker(label: id, started: started)
        } else if id.hasPrefix("picker_sort:") {
            guard picker.chooseSort(String(id.dropFirst("picker_sort:".count))) else {
                fail("排序菜单里没有：\(id)")
                return true
            }
            waitPicker(label: id, started: started)
        } else if id.hasPrefix("picker_hide:") {
            picker.setHideThumbnails(id.hasSuffix(":on"))
            waitPicker(label: id, started: started)
        } else if id == "picker_ok" {
            waitingImage = true
            picker.choose()
        } else if id == "picker_cancel" {
            picker.window?.performClose(nil)
            records.append(["step": id, "name": controller.currentName,
                            "index": controller.currentIndex.map { Int($0) as Any } ?? NSNull()])
            captureLater()
        } else {
            fail("步骤写错了：\(id)")
        }
        return true
    }

    /// 等选择图片窗口里看得见的缩略图都生成好（最多 5 秒），再记录、截图。
    private func waitPicker(label: String, started: UInt64) {
        guard !finished else { return }
        var done = true
        if let picker = controller.picker, picker.stripVisible {
            let loaded = picker.thumbnailsLoaded
            done = loaded.visible > 0 && loaded.ready == loaded.visible
        }
        if done || Self.milliseconds(since: started) > 5000 {
            records.append(["step": label, "name": controller.currentName,
                            "index": controller.currentIndex.map { Int($0) as Any } ?? NSNull(),
                            "pickerWaitMs": Self.milliseconds(since: started)])
            captureLater()
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
            MainActor.assumeIsolated {
                self?.waitPicker(label: label, started: started)
            }
        }
    }

    private func pickerInfo(_ picker: PickerWindowController) -> [String: Any] {
        let loaded = picker.thumbnailsLoaded
        var info: [String: Any] = [
            "rows": picker.rowCount,
            "sortLabel": picker.sortLabel,
            "stripVisible": picker.stripVisible,
            "hideSetting": core.core.pickerHidesThumbnails(),
            "thumbnailsReady": loaded.ready,
            "thumbnailsVisible": loaded.visible,
        ]
        if let selected = picker.selectedIndex {
            info["selected"] = selected
            info["selectedName"] = picker.row(selected)?.name ?? ""
        }
        if let first = picker.row(0) {
            info["firstName"] = first.name
            info["firstSize"] = first.size
            info["firstModified"] = first.modified
        }
        return info
    }

    /// 设置和输入相关的步骤；不是这类步骤时返回 false。
    private func settingsStep(_ id: String) -> Bool {
        let window = settingsWindow?()
        let name = controller.currentName
        if id == "settings_open" {
            let outcome = core.core.invokeAction(id: "settings")
            guard outcome == .ui(request: .showSettings) else {
                fail("设置没有打开：\(outcome)")
                return true
            }
            controller.apply(outcome)
            guard settingsWindow?() != nil else {
                fail("设置窗口没有出现")
                return true
            }
            let pages = core.core.settingsPages(lang: controller.lang).map { "\($0.id) \($0.title)" }
            records.append(["step": id, "name": name, "settingsPages": pages, "uiSettings": uiSettingsInfo(),
                            "lang": "\(controller.lang)"])
        } else if id.hasPrefix("settings_page:") {
            let page = String(id.dropFirst("settings_page:".count))
            guard let window else {
                fail("设置窗口没有打开")
                return true
            }
            window.selectPage(page)
            guard window.currentPage == page else {
                fail("没有这一页：\(page)")
                return true
            }
            records.append(["step": id, "name": name])
        } else if id.hasPrefix("settings_button:") {
            // settings_button:键:按钮：按设置项里的按钮（如书签页的"删除"，；自检时不弹确认框）。
            let spec = String(id.dropFirst("settings_button:".count))
            guard let window, let colon = spec.lastIndex(of: ":") else {
                fail("设置窗口没有打开，或步骤写错了：\(id)")
                return true
            }
            window.pressButtonForTest(key: String(spec[..<colon]), button: String(spec[spec.index(after: colon)...]))
            records.append(["step": id, "name": name, "archivePositions": Int(core.core.archivePositionsCount())])
        } else if id.hasPrefix("settings_set:") {
            let assignment = String(id.dropFirst("settings_set:".count))
            guard let window, let equals = assignment.firstIndex(of: "=") else {
                fail("设置窗口没有打开，或步骤写错了：\(id)")
                return true
            }
            let key = String(assignment[..<equals])
            let value = String(assignment[assignment.index(after: equals)...])
            guard window.setForTest(key: key, value: value) else {
                fail("这一页没有 \(key)，或它不能改成 \(value)")
                return true
            }
            records.append(["step": id, "name": name, "dirty": core.core.settingsDirty()])
        } else if id == "settings_ok" || id == "settings_ok_invalid" {
            guard let window else {
                fail("设置窗口没有打开")
                return true
            }
            lastApplied = nil
            let applied = window.ok()
            guard applied == (id == "settings_ok") else {
                fail("点“确定”的结果不对：生效 \(applied)，提示 \(window.lastError ?? "")")
                return true
            }
            var record: [String: Any] = ["step": id, "name": name, "applied": applied,
                                         "uiSettings": uiSettingsInfo(), "lang": "\(controller.lang)"]
            if !applied {
                record["error"] = window.lastError ?? ""
                record["errorPage"] = window.currentPage
            }
            if let info = lastApplied {
                record["changed"] = info.changed
                record["saveError"] = info.saveError ?? ""
            }
            if applied, let path = core.core.settingsPath(),
               let text = try? String(contentsOfFile: path, encoding: .utf8) {
                record["settingsFile"] = path
                record["settingsText"] = text
            }
            records.append(record)
        } else if id == "settings_reset" {
            guard let window else {
                fail("设置窗口没有打开")
                return true
            }
            window.reset()
            records.append(["step": id, "name": name, "dirty": core.core.settingsDirty()])
        } else if id == "settings_cancel" {
            guard let window else {
                fail("设置窗口没有打开")
                return true
            }
            let dirty = core.core.settingsDirty()
            window.discard()
            records.append(["step": id, "name": name, "dirtyBeforeCancel": dirty])
        } else if id.hasPrefix("key:") {
            // 与主窗口的按键处理相同：交给核心解析成动作再执行（键盘映射优先；Esc 退出受设置"按 ESC 退出"控制）。
            // 写法同动作目录，如 key:Cmd+K。
            var spec = String(id.dropFirst("key:".count))
            // 步骤用逗号分隔：逗号、句号键写成 Comma、Period。
            if spec == "Comma" { spec = "," } else if spec == "Period" { spec = "." }
            guard let parsed = SettingsWindowController.parseKey(spec) else {
                fail("按键写错了：\(spec)")
                return true
            }
            let action = core.core.actionForKey(key: parsed.0, modifiers: parsed.1)
            let extra: [String: Any] = ["key": spec, "action": action ?? ""]
            if let action {
                runAction(action, label: id, extra: extra)
                return true
            }
            var record: [String: Any] = ["step": id, "name": name]
            record.merge(extra) { _, new in new }
            records.append(record)
        } else if id.hasPrefix("mouse:") {
            // 鼠标：与真的点击走同样的规则（设置"鼠标"页）。mouse:left:0.9 是在画面九成宽处单击左键，
            // mouse:double:0.5 是双击；其余为 middle、right、back、forward、wheel_up、wheel_down。
            let parts = id.dropFirst("mouse:".count).split(separator: ":").map(String.init)
            let fraction = parts.count > 1 ? Double(parts[1]) ?? 0.5 : 0.5
            var target: MouseTarget = .nothing
            switch parts.first ?? "" {
            case "left":
                target = core.core.leftClickAction(xFraction: fraction).map { .action($0) } ?? .nothing
            case "double":
                target = core.core.leftDoubleClickAction(xFraction: fraction).map { .action($0) } ?? .nothing
            case "middle": target = controller.canvas.mouseTarget(.middle)
            case "right": target = controller.canvas.mouseTarget(.right)
            case "back": target = controller.canvas.mouseTarget(.back)
            case "forward": target = controller.canvas.mouseTarget(.forward)
            case "wheel_up": target = controller.canvas.mouseTarget(.wheelUp)
            case "wheel_down": target = controller.canvas.mouseTarget(.wheelDown)
            default:
                fail("鼠标步骤写错了：\(id)")
                return true
            }
            switch target {
            case let .action(action):
                runAction(action, label: id, extra: ["mouse": parts[0], "action": action])
                return true
            case .contextMenu:
                // 右键菜单是单独的弹出层，这里只记录要弹出菜单，不真的弹出。
                records.append(["step": id, "name": name, "mouse": parts[0], "action": "context_menu"])
            case .nothing:
                records.append(["step": id, "name": name, "mouse": parts[0], "action": ""])
            }
        } else if id == "hover_bottom" {
            guard let content = controller.window?.contentView else {
                fail("没有内容视图")
                return true
            }
            let width = Double(content.bounds.width)
            let height = Double(content.bounds.height)
            controller.overlays.pointerMoved(x: width / 2, y: height - 20, width: width, height: height, core: core.core)
            hoverBar = true
            records.append(["step": id, "name": name])
        } else {
            return false
        }
        captureLater()
        return true
    }

    /// 界面相关的设置（自检报告用）。
    private func uiSettingsInfo() -> [String: Any] {
        let settings = core.core.uiSettings()
        return [
            "backgroundColor": String(format: "#%06X", settings.backgroundColor),
            "alphaBackground": "\(settings.alphaBackground)",
            "hideControlBar": settings.hideControlBar,
            "hideSideArrows": settings.hideSideArrows,
            "hideZoomInfo": settings.hideZoomInfo,
            "hideTooltips": settings.hideTooltips,
            "smallInfoFont": settings.smallInfoFont,
            "cornerNotices": settings.cornerNotices,
            "doubleClickMaximize": settings.doubleClickMaximize,
            "alwaysOnTop": settings.alwaysOnTop,
            "noRecent": settings.noRecent,
            "windowFollowsImage": core.core.windowFollowsImage(),
        ]
    }

    /// 等精确重绘盖上画面：每 50 ms 看一次，最多 3 秒。
    private func waitExact(started: UInt64) {
        guard !finished else { return }
        if let info = controller.canvas.exactShown {
            exactCount += 1
            exactWorstMs = max(exactWorstMs, info.renderMs)
            records.append([
                "step": "wait_exact",
                "name": controller.currentName,
                "exact": ["x": Int(info.x), "y": Int(info.y), "width": info.width, "height": info.height,
                          "renderMs": info.renderMs],
            ])
            captureLater()
            return
        }
        if Self.milliseconds(since: started) > 3000 {
            fail("精确重绘 3 秒内没有出现")
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
            MainActor.assumeIsolated {
                self?.waitExact(started: started)
            }
        }
    }

    /// 到达首尾时的询问：不弹对话框，记录这一步；下一步充当用户的选择。
    private func boundaryReached(forward: Bool) -> Bool {
        guard !finished else { return true }
        guard waitingImage else { return true }
        waitingImage = false
        records.append(["step": stepLabel, "name": controller.currentName, "boundary": forward ? "last" : "first"])
        captureLater()
        return true
    }

    /// 图片被安全上限拦下：记下这一步（blocked），下一步 limit_open、limit_cancel 充当用户的选择。
    private func limitReached() -> Bool {
        guard !finished else { return true }
        guard waitingImage else { return true }
        waitingImage = false
        records.append(["step": stepLabel, "name": controller.currentName, "blocked": true])
        // 这时画面上没有图片（等用户回答），不截图核对，直接做下一步。
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated {
                self?.advance()
            }
        }
        return true
    }

    private func problem(_ message: String) {
        guard openPath != nil else { return }
        fail(message)
    }

    private func fail(_ message: String) {
        if failure == nil {
            failure = message
        }
        finish()
    }

    private func finish() {
        guard !finished else { return }
        finished = true
        let rightClick = checkRightClickMenu()

        let window = controller.window
        let contentSize = window?.contentView?.bounds.size ?? .zero
        var report: [String: Any] = [
            "platform": "macos",
            "coreVersion": coreVersion(),
            "emptyStateHint": emptyStateHint(platform: .macos, lang: .zhHans),
            "pongReceived": core.lastPingMilliseconds != nil,
            "pingRoundTripMs": core.lastPingMilliseconds ?? -1,
            "mainToReportMs": LaunchClock.millisecondsSinceMainStart(),
            // 启动过程的时间点（从 main 开始算的毫秒）和 main 之前的时间（进程启动到 main）。
            "launchTimeline": LaunchClock.marks.map { ["name": $0.name, "ms": $0.ms] as [String: Any] },
            "preMainMs": (Self.processAgeMs() ?? 0) - LaunchClock.millisecondsSinceMainStart(),
            "elapsedMs": Self.milliseconds(since: started),
            "contentSizePt": [contentSize.width, contentSize.height],
            "backingScaleFactor": window?.backingScaleFactor ?? 0,
            "osVersion": ProcessInfo.processInfo.operatingSystemVersionString,
            "displayColorSpace": window?.screen?.colorSpace?.localizedName ?? "unknown",
            "arch": Self.architecture,
            "preferredLanguage": Locale.preferredLanguages.first ?? "",
            "menuBar": NSApp.mainMenu.map { MenuBuilder.describe($0) } ?? [],
            "contextMenu": MenuBuilder.describe(controller.contextMenu()),
            "rightClickMenu": rightClick.info,
        ]

        let ok: Bool
        if openPath == nil {
            let shot = capture(to: reportDir.appendingPathComponent("screenshot.png"))
            report["screenshot"] = shot.info
            ok = core.lastPingMilliseconds != nil && (shot.info["cornerColorOK"] as? Bool ?? false)
        } else {
            report["open"] = openPath ?? ""
            report["steps"] = records
            report["failure"] = failure.map { $0 as Any } ?? NSNull()
            // 给脚本读的纯文字版本（plutil 读不了 null）。
            report["failureText"] = failure ?? ""
            let checked = records.reduce(0) { $0 + (($1["samples"] as? [String: Any])?["checked"] as? Int ?? 0) }
            report["samplesChecked"] = checked
            report["exactCount"] = exactCount
            report["exactWorstMs"] = exactWorstMs
            ok = failure == nil && records.count == steps.count + 1
        }
        report["ok"] = ok && rightClick.ok

        do {
            let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: reportDir.appendingPathComponent("selftest.json"))
            FileHandle.standardOutput.write(data)
            FileHandle.standardOutput.write(Data("\n".utf8))
        } catch {
            FileHandle.standardError.write(Data("写入自检报告失败：\(error)\n".utf8))
        }
        // 自检直接结束进程：先关掉压缩包（删掉解出的临时文件）。
        core.core.shutdown()
        exit(ok && rightClick.ok ? 0 : 1)
    }

    /// 模拟在画面中央按下鼠标右键，交给窗口按正常流程分发（找到画布、弹出菜单）。
    /// 菜单弹出后由探针在菜单自己的事件循环里关掉，这里才会返回。
    private func checkRightClickMenu() -> (ok: Bool, info: [String: Any]) {
        guard let window = controller.window else {
            return (false, ["error": "没有窗口"])
        }
        let canvas = controller.canvas
        let probe = ContextMenuProbe()
        let original = canvas.makeContextMenu
        canvas.makeContextMenu = {
            let menu = original?()
            menu?.delegate = probe
            return menu
        }
        defer {
            canvas.makeContextMenu = original
        }
        let center = canvas.convert(NSPoint(x: canvas.bounds.midX, y: canvas.bounds.midY), to: nil)
        // 右键映射成了别的动作（设置"鼠标"页）时，右键不弹菜单；改用 ⌃ 点按，菜单照样要弹出。
        let rightOpensMenu = canvas.mouseTarget(.right) == .contextMenu
        guard let event = NSEvent.mouseEvent(
            with: rightOpensMenu ? .rightMouseDown : .leftMouseDown,
            location: center,
            modifierFlags: rightOpensMenu ? [] : [.control],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber,
            context: nil,
            eventNumber: 0,
            clickCount: 1,
            pressure: 1
        ) else {
            return (false, ["error": "无法生成右键事件"])
        }
        window.sendEvent(event)
        return (probe.opens == 1, ["opened": probe.opens == 1, "opens": probe.opens, "items": probe.itemCount,
                                   "via": rightOpensMenu ? "right-click" : "control-click"])
    }

    private static func layoutInfo(_ layout: ViewLayout?) -> Any {
        guard let layout else { return NSNull() }
        return [
            "centerX": layout.centerX,
            "centerY": layout.centerY,
            "width": layout.width,
            "height": layout.height,
            "scale": layout.scale,
            "zoom": layout.zoom,
            "rotation": Int(layout.rotation),
            "flipHorizontal": layout.flipHorizontal,
            "flipVertical": layout.flipVertical,
            "smooth": layout.smooth,
        ] as [String: Any]
    }

    /// 悬浮界面的状态，以及与核心状态不一致的地方。
    private func overlayState() -> (info: [String: Any], problems: [String]) {
        let overlays = controller.overlays
        let core = self.core.core
        let locked = core.actionState(id: "toggle_bar_lock").checked
        let infoOn = core.actionState(id: "toggle_info").checked
        let lines = core.infoLines(lang: controller.lang)
        let infoShown = !overlays.info.isHidden
        var problems: [String] = []
        // 自检时指针不动，控制栏只在锁定时（或模拟指针移到底部后）显示；刚解锁时按规则等 1 秒再淡出。
        // 设置"不显示控制栏"时任何时候都不显示。
        let hidden = controller.uiSettings.hideControlBar
        let wanted = !hidden && (locked || hoverBar)
        let barOK = overlays.barFade.isShown == wanted || (!wanted && overlays.barHidePending)
        if !barOK {
            problems.append("控制栏显示=\(overlays.barFade.isShown)，锁定=\(locked)，指针在底部=\(hoverBar)，设置不显示=\(hidden)")
        }
        if infoShown != (infoOn && !lines.isEmpty) {
            problems.append("图片信息显示=\(infoShown)，开关=\(infoOn)")
        }
        if infoShown && overlays.infoLabel.stringValue != lines.joined(separator: "\n") {
            problems.append("图片信息的文字与核心不同")
        }
        var expectedKnob: Double?
        if let index = controller.currentIndex, let total = controller.currentTotal {
            expectedKnob = progressFraction(index: index, total: total)
        }
        if overlays.strip.knobFraction != expectedKnob {
            problems.append("进度条滑块 \(String(describing: overlays.strip.knobFraction))，应为 \(String(describing: expectedKnob))")
        }
        // 标题栏：跟着控制栏显示，没有图片时一直显示；刚该隐藏时按规则等 1 秒再淡出；全屏时不核对。
        var titleShown: Any = NSNull()
        if let titlebar = overlays.titlebar, !titlebar.inFullScreen {
            titleShown = titlebar.isShown
            let expected = titlebar.alwaysShown || overlays.barFade.isShown || !overlays.hasImage
            if titlebar.isShown != expected && !(titlebar.isShown && overlays.titleHidePending) {
                problems.append("标题栏显示=\(titlebar.isShown)，控制栏显示=\(overlays.barFade.isShown)")
            }
        }
        let info: [String: Any] = [
            "barHidden": hidden,
            "barShown": overlays.barFade.isShown,
            "barHidePending": overlays.barHidePending,
            "barLocked": locked,
            "knob": overlays.strip.knobFraction.map { $0 as Any } ?? NSNull(),
            "infoShown": infoShown,
            "infoText": infoShown ? lines : [],
            "titlebarShown": titleShown,
        ]
        return (info, problems)
    }

    /// 画面核对：图片上 4×4 个点（各自位于一个色块的中部）按核心的映射找到屏幕位置，比较颜色。
    /// 这些点都不在窗口里时（如超大图的 100%），改在可见区域里均匀取 4×4 个点，再换回图片坐标。都取像素中心。
    /// 画布铺满内容视图，截图按物理像素绘制，所以核心给出的画布坐标就是截图里的坐标。
    /// 帮助窗口开着：截图、记下标题和行数，然后关掉它。
    private func recordGeneration(_ id: String) {
        guard !finished else { return }
        let step = records.count
        let url = reportDir.appendingPathComponent("step-\(step)-generation.png")
        var state = generationPanel?()?.report(screenshot: url) ?? ["open": false]
        state["enabled"] = core.core.actionState(id: "show_generation_info").enabled
        records.append(["step": id, "name": controller.currentName, "generation": state])
        captureLater()
    }

    private func recordHelpWindow(_ id: String) {
        guard !finished else { return }
        guard let help = helpWindows?().last else {
            fail("帮助窗口没有打开：\(id)")
            return
        }
        let step = records.count
        let headings = help.rows.filter { $0.heading }.count
        let first = help.rows.first { !$0.heading }.map { "\($0.left) \($0.right)" } ?? ""
        records.append(["step": id, "name": controller.currentName, "help": [
            "title": help.window?.title ?? "",
            "headings": headings,
            "rows": help.rows.count - headings,
            "first": first,
            "textLength": help.text?.utf8.count ?? 0,
            "anyviewLicense": help.text?.contains("Copyright (c) 2026 Anyview contributors") ?? false,
            "screenshot": help.snapshot(to: reportDir.appendingPathComponent("step-\(step)-help.png")),
        ] as [String: Any]])
        help.window?.performClose(nil)
        captureLater()
    }

    /// 打开方式（open_with 步骤）。两件事：
    /// 1. 这个程序的 Info.plist 是不是声明了能打开每种支持的扩展名（按系统给这个扩展名的类型判断：属于声明的类型，
    ///    或者在按扩展名登记的列表里）——通过与否看这一项。
    /// 2. 系统（LaunchServices）现在列出的能打开它的程序里有没有 Anyview（按 Bundle ID 认，记下是哪一份、是不是这一次的构建）。
    ///    系统不把临时目录（/private/var/folders）、网络共享里的程序列进“打开方式”，
    ///    装进“应用程序”后才列出；所以一份都没列出时只记录，列出了一部分扩展名才算问题。
    private static func openWithReport() -> [String: Any] {
        let info = Bundle.main.infoDictionary ?? [:]
        let documentTypes = info["CFBundleDocumentTypes"] as? [[String: Any]] ?? []
        let declaredTypes = documentTypes.flatMap { ($0["LSItemContentTypes"] as? [String] ?? []).compactMap { UTType($0) } }
        let declaredExtensions = Set(documentTypes.flatMap { ($0["CFBundleTypeExtensions"] as? [String] ?? []).map { $0.lowercased() } })
        let myID = Bundle.main.bundleIdentifier
        let myBuild = info["CFBundleVersion"] as? String
        var notDeclared: [String] = []
        var missing: [String] = []
        var dynamic: [String] = []
        var types: [String: String] = [:]
        var listed: [String: String] = [:]
        var jpgApps: [String] = []
        for ext in supportedExtensions() {
            let type = UTType(filenameExtension: ext)
            types[ext] = type?.identifier ?? ""
            if type?.isDynamic == true {
                dynamic.append(ext)
            }
            let byType = type.map { t in declaredTypes.contains { t.conforms(to: $0) } } ?? false
            if !byType && !declaredExtensions.contains(ext) {
                notDeclared.append(ext)
            }
            guard let type else {
                missing.append(ext)
                continue
            }
            let apps = NSWorkspace.shared.urlsForApplications(toOpen: type)
            if ext == "jpg" {
                jpgApps = apps.map(\.path)
            }
            guard let copy = apps.first(where: { Bundle(url: $0)?.bundleIdentifier == myID }) else {
                missing.append(ext)
                continue
            }
            let build = Bundle(url: copy)?.infoDictionary?["CFBundleVersion"] as? String ?? "?"
            listed[copy.path] = build == myBuild ? "这一次的构建" : "旧构建 \(build)"
        }
        let count = supportedExtensions().count
        return ["checked": count, "notDeclared": notDeclared, "missing": missing, "listedCount": count - missing.count,
                "dynamic": dynamic, "types": types, "listedCopies": listed, "jpgApps": jpgApps,
                "running": Bundle.main.bundleURL.path,
                // 系统按 Bundle ID 找到的那份 Anyview（没有时为空）：看系统认不认这个程序。
                "systemAppURL": myID.flatMap { NSWorkspace.shared.urlForApplication(withBundleIdentifier: $0)?.path } ?? ""]
    }

    /// 图片中心的颜色（RGB）。
    private static func centerColor(_ image: ImageHandle) -> [Int]? {
        guard let base = UnsafeRawPointer(bitPattern: UInt(image.pixelAddress())) else { return nil }
        let offset = Int(image.height() / 2) * Int(image.stride()) + Int(image.width() / 2) * 4
        let p = base.advanced(by: offset).assumingMemoryBound(to: UInt8.self)
        return [Int(p[2]), Int(p[1]), Int(p[0])]
    }

    /// 翻页效果停在中间时的画面核对：两张图都是纯色（effects/），按效果看几个点的颜色。
    /// 淡出：中间是两种颜色按进度混合；滑动：往后翻左边是旧图、右边是新图（往前翻相反）；
    /// 翻页：往后翻旧的一页以左边为轴翻起（自检停住时是平面压扁的画法），右边露出新图、左边是变暗的旧图。
    private func transitionCheck(_ context: CGContext?,
                                 state: (effect: PageEffect, forward: Bool, progress: Double))
        -> (checked: Int, mismatches: Int, skipped: Bool, info: [String: Any]) {
        guard let context, let data = context.data, let layout = core.core.viewLayout(),
              let old = previousColor, let new = currentColor
        else {
            return (0, 0, false, ["checked": 0, "error": "没有截图、画面参数或新旧两张图的颜色"])
        }
        let eased = state.progress * state.progress * (3 - 2 * state.progress)
        func mix(_ a: [Int], _ b: [Int], _ t: Double) -> [Int] {
            (0..<3).map { Int((Double(a[$0]) * (1 - t) + Double(b[$0]) * t).rounded()) }
        }
        func dark(_ c: [Int]) -> [Int] { mix(c, [0, 0, 0], 0.35 * eased) }
        let first = state.forward ? old : new
        let second = state.forward ? new : old
        let expected: [(Double, [Int])]
        switch state.effect {
        case .fade: expected = [(0.5, mix(old, new, eased))]
        case .slide: expected = [(0.25, first), (0.75, second)]
        case .page: expected = state.forward ? [(0.03, dark(old)), (0.95, new)] : [(0.03, dark(new)), (0.95, old)]
        case .none: expected = []
        }
        let left = layout.centerX - layout.width / 2
        let y = Int(max(0, layout.centerY))
        var points: [[String: Any]] = []
        var mismatches = 0
        for (fx, want) in expected {
            let x = Int(max(0, left + layout.width * fx))
            guard x < context.width, y < context.height else {
                mismatches += 1
                continue
            }
            let p = data.advanced(by: y * context.bytesPerRow + x * 4).assumingMemoryBound(to: UInt8.self)
            let got = [Int(p[2]), Int(p[1]), Int(p[0])]
            let ok = zip(got, want).allSatisfy { abs($0 - $1) <= 30 }
            if !ok {
                mismatches += 1
            }
            points.append(["x": fx, "got": got, "want": want, "ok": ok])
        }
        let effectName: String
        switch state.effect {
        case .fade: effectName = "fade"
        case .slide: effectName = "slide"
        case .page: effectName = "page"
        case .none: effectName = "none"
        }
        return (expected.count, mismatches, false, [
            "checked": expected.count, "mismatches": mismatches,
            "transition": ["effect": effectName, "forward": state.forward, "progress": state.progress, "points": points],
        ])
    }

    private func checkSamples(_ context: CGContext?) -> (checked: Int, mismatches: Int, skipped: Bool, info: [String: Any]) {
        // 空态（关闭了图片）：没有要核对的画面。
        if core.core.currentPath() == nil {
            return (0, 0, true, ["checked": 0, "skipped": "空态"])
        }
        if sampleAt100, let layout = core.core.viewLayout(), abs(layout.zoom - 1) > 1e-9 {
            return (0, 0, true, ["checked": 0, "skipped": "只在 100% 时核对"])
        }
        // 放大显示（物理倍率大于 1，如 Retina 上的 100%）时，屏幕像素的颜色由相邻的图片像素插值而来。
        let magnified = (core.core.viewLayout()?.scale ?? 1) > 1 + 1e-6
        guard let context, let data = context.data, let image = controller.canvas.currentHandle ?? currentImage,
              let base = UnsafeRawPointer(bitPattern: UInt(image.pixelAddress()))
        else {
            return (0, 0, false, ["checked": 0, "error": "没有截图或图片"])
        }
        let width = Double(image.width())
        let height = Double(image.height())
        let stride = Int(image.stride())
        // 悬浮控件挡住的区域（内容视图的点坐标，y 向上）换成截图的像素坐标（原点在左上）。
        let scale = controller.window?.backingScaleFactor ?? 2
        let viewHeight = controller.window?.contentView?.bounds.height ?? 0
        let covered = controller.coveredRects.map { rect in
            CGRect(x: rect.minX * scale, y: (viewHeight - rect.maxY) * scale,
                   width: rect.width * scale, height: rect.height * scale)
        }
        // 图片带 ICC 时，截图（sRGB）里应是经 ColorSync 转换后的颜色：用同样的转换算出期望值。
        let imageSpace = image.iccProfile().flatMap { CGColorSpace(iccData: $0 as CFData) }
        let srgb = CGColorSpace(name: CGColorSpace.sRGB)
        func displayed(_ bgr: [Int]) -> [Int] {
            guard let imageSpace, let srgb else { return bgr }
            let components: [CGFloat] = [CGFloat(bgr[2]) / 255, CGFloat(bgr[1]) / 255, CGFloat(bgr[0]) / 255, 1]
            guard let color = CGColor(colorSpace: imageSpace, components: components),
                  let converted = color.converted(to: srgb, intent: .defaultIntent, options: nil),
                  let c = converted.components, c.count >= 3
            else { return bgr }
            return [Int((c[2] * 255).rounded()), Int((c[1] * 255).rounded()), Int((c[0] * 255).rounded())]
        }
        var checked = 0
        var mismatches: [[String: Any]] = []
        var center: String?
        if let point = core.core.viewImageToScreen(u: width / 2, v: height / 2) {
            let x = Int(point.x)
            let y = Int(point.y)
            if x >= 0, y >= 0, x < context.width, y < context.height {
                let pixel = data.advanced(by: y * context.bytesPerRow + x * 4).assumingMemoryBound(to: UInt8.self)
                center = String(format: "#%02X%02X%02X", pixel[2], pixel[1], pixel[0])
            }
        }
        let margin = 8
        func onScreen(_ u: Double, _ v: Double) -> Bool {
            guard let point = core.core.viewImageToScreen(u: u, v: v) else { return false }
            let x = Int(point.x)
            let y = Int(point.y)
            return x >= margin && y >= margin && x < context.width - margin && y < context.height - margin
        }
        func pixelCenter(_ t: Double) -> Double { t.rounded(.down) + 0.5 }
        let fractions = [0.125, 0.375, 0.625, 0.875]
        var points: [(u: Double, v: Double)] = []
        for fy in fractions {
            for fx in fractions {
                points.append((pixelCenter(fx * width), pixelCenter(fy * height)))
            }
        }
        if !points.contains(where: { onScreen($0.u, $0.v) }) {
            let corners = [(0.0, 0.0), (width, 0.0), (0.0, height), (width, height)]
                .compactMap { core.core.viewImageToScreen(u: $0.0, v: $0.1) }
            if let minX = corners.map(\.x).min(), let maxX = corners.map(\.x).max(),
               let minY = corners.map(\.y).min(), let maxY = corners.map(\.y).max() {
                let left = max(minX, 0)
                let right = min(maxX, Double(context.width))
                let top = max(minY, 0)
                let bottom = min(maxY, Double(context.height))
                points = []
                for fy in fractions {
                    for fx in fractions {
                        let x = left + fx * (right - left)
                        let y = top + fy * (bottom - top)
                        if let p = core.core.viewScreenToImage(x: x, y: y), p.x >= 0, p.y >= 0, p.x < width, p.y < height {
                            points.append((pixelCenter(p.x), pixelCenter(p.y)))
                        }
                    }
                }
            }
        }
        for (u, v) in points {
            guard let point = core.core.viewImageToScreen(u: u, v: v) else { continue }
            let x = Int(point.x)
            let y = Int(point.y)
            guard x >= margin, y >= margin, x < context.width - margin, y < context.height - margin else { continue }
            if covered.contains(where: { $0.contains(CGPoint(x: x, y: y)) }) {
                continue
            }
            // 两边都是 BGRA：截图是 sRGB 位图，图片是解码出的缓冲区。
            let got = data.advanced(by: y * context.bytesPerRow + x * 4).assumingMemoryBound(to: UInt8.self)
            // 原图上应有的颜色范围：缩小或 1:1 时就是这个点的颜色；放大时是截图像素中心附近 2×2 个图片像素的
            // 颜色范围（插值的结果落在它们之间）。不透明处与原图比；全透明处应为棋盘格的两种颜色之一；半透明处跳过。
            var around: [(i: Int, j: Int)] = [(Int(u), Int(v))]
            if magnified, let p = core.core.viewScreenToImage(x: Double(x) + 0.5, y: Double(y) + 0.5) {
                let i0 = min(max(Int((p.x - 0.5).rounded(.down)), 0), Int(width) - 1)
                let j0 = min(max(Int((p.y - 0.5).rounded(.down)), 0), Int(height) - 1)
                let i1 = min(i0 + 1, Int(width) - 1)
                let j1 = min(j0 + 1, Int(height) - 1)
                around = [(i0, j0), (i1, j0), (i0, j1), (i1, j1)]
            }
            let pixels = around.map { base.advanced(by: $0.j * stride + $0.i * 4).assumingMemoryBound(to: UInt8.self) }
            let ranges: [(low: [Int], high: [Int])]
            if pixels.allSatisfy({ $0[3] == 255 }) {
                var low = [255, 255, 255]
                var high = [0, 0, 0]
                for pixel in pixels {
                    let color = displayed([Int(pixel[0]), Int(pixel[1]), Int(pixel[2])])
                    for k in 0..<3 {
                        low[k] = min(low[k], color[k])
                        high[k] = max(high[k], color[k])
                    }
                }
                ranges = [(low, high)]
            } else if pixels.allSatisfy({ $0[3] == 0 }) {
                // 透明处的背景：棋盘格的两种颜色之一，或设置的纯色（GIF 另有设置）。
                switch core.core.alphaFill(image: image) {
                case .checker: ranges = [([255, 255, 255], [255, 255, 255]), ([200, 200, 200], [200, 200, 200])]
                case let .solid(rgb):
                    let color = [Int((rgb >> 16) & 0xFF), Int((rgb >> 8) & 0xFF), Int(rgb & 0xFF)]
                    ranges = [(color, color)]
                }
            } else {
                continue
            }
            checked += 1
            // 截图颜色超出范围的多少（每个通道取最大）；落在范围里为 0。
            func distance(_ range: (low: [Int], high: [Int])) -> Int {
                var worst = 0
                for k in 0..<3 {
                    let value = Int(got[k])
                    worst = max(worst, range.low[k] - value, value - range.high[k])
                }
                return worst
            }
            let diff = ranges.map(distance).min() ?? 0
            if diff > Self.colorTolerance {
                let expected = ranges[0].low
                mismatches.append([
                    "image": [u, v],
                    "screen": [x, y],
                    "expected": String(format: "#%02X%02X%02X", expected[2], expected[1], expected[0]),
                    "got": String(format: "#%02X%02X%02X", got[2], got[1], got[0]),
                ])
            }
        }
        return (checked, mismatches.count, false, ["checked": checked, "mismatches": mismatches,
                                                   "centerColor": center.map { $0 as Any } ?? NSNull(),
                                                   "icc": imageSpace != nil])
    }

    /// 把内容视图的图层树画进 sRGB 位图：包括画布图层里的图片，像素值与界面设定的颜色一致，
    /// 和显示器的色彩配置无关，换到别的机器上看也不偏色。不需要屏幕录制权限。
    private func capture(to url: URL) -> (info: [String: Any], context: CGContext?) {
        guard let window = controller.window, let view = window.contentView, let layer = view.layer else {
            return (["error": "没有内容视图"], nil)
        }
        view.layoutSubtreeIfNeeded()
        view.displayIfNeeded()
        let scale = window.backingScaleFactor
        let width = Int((view.bounds.width * scale).rounded())
        let height = Int((view.bounds.height * scale).rounded())
        let bitmapInfo = CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        guard width > 0, height > 0,
              let space = CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(
                  data: nil,
                  width: width,
                  height: height,
                  bitsPerComponent: 8,
                  bytesPerRow: 0,
                  space: space,
                  bitmapInfo: bitmapInfo
              )
        else {
            return (["error": "无法创建位图"], nil)
        }
        context.scaleBy(x: scale, y: scale)
        layer.render(in: context)
        guard let image = context.makeImage() else {
            return (["error": "无法生成图像"], context)
        }
        let corner = Self.hexColor(in: context, x: 24, y: 24)
        let rep = NSBitmapImageRep(cgImage: image)
        guard let png = rep.representation(using: .png, properties: [:]) else {
            return (["error": "PNG 编码失败"], context)
        }
        do {
            try png.write(to: url)
            return ([
                "file": url.lastPathComponent,
                "pixelsWide": width,
                "pixelsHigh": height,
                "colorSpace": "sRGB",
                // 离左上角 24 像素采样，避开窗口圆角，也不会碰到居中的文字（与 Linux 自测一致）。
                "cornerColor": corner,
                "cornerColorOK": corner == "#202020",
            ], context)
        } catch {
            return (["error": "\(error)"], context)
        }
    }

    /// 读取位图内存里的像素（第一行是图像最上面一行；格式为 BGRA）。
    private static func hexColor(in context: CGContext, x: Int, y: Int) -> String {
        guard let data = context.data else { return "unknown" }
        let offset = y * context.bytesPerRow + x * 4
        let pixel = data.advanced(by: offset).assumingMemoryBound(to: UInt8.self)
        return String(format: "#%02X%02X%02X", pixel[2], pixel[1], pixel[0])
    }

    private static func milliseconds(since start: UInt64) -> Double {
        Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
    }

    /// 本进程启动至今的毫秒数（sysctl 给出的进程启动时间，用于冷启动耗时测量）。
    private static func processAgeMs() -> Double? {
        var info = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
        guard sysctl(&mib, u_int(mib.count), &info, &size, nil, 0) == 0 else { return nil }
        let start = info.kp_proc.p_un.__p_starttime
        var now = timeval()
        gettimeofday(&now, nil)
        return Double(now.tv_sec - start.tv_sec) * 1000 + Double(now.tv_usec - start.tv_usec) / 1000
    }

    /// 本进程用掉的 CPU 时间（用户 + 系统，秒）。
    private static func cpuSeconds() -> Double {
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        func seconds(_ t: timeval) -> Double { Double(t.tv_sec) + Double(t.tv_usec) / 1_000_000 }
        return seconds(usage.ru_utime) + seconds(usage.ru_stime)
    }

    /// 现在的内存占用（MB）：phys_footprint，与"活动监视器"里"内存"一栏相同。
    private static func footprintMb() -> Double? {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
        let result = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard result == KERN_SUCCESS else { return nil }
        return Double(info.phys_footprint) / 1_048_576
    }

    /// 内存峰值（MB；macOS 的 ru_maxrss 以字节计）。
    private static func peakRssMb() -> Double {
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        return Double(usage.ru_maxrss) / 1_048_576
    }

    private static var architecture: String {
        #if arch(arm64)
        return "arm64"
        #elseif arch(x86_64)
        return "x86_64"
        #else
        return "unknown"
        #endif
    }
}

/// 自检用：记录右键菜单弹出的次数，弹出后马上关闭。
@MainActor
final class ContextMenuProbe: NSObject, NSMenuDelegate {
    private(set) var opens = 0
    private(set) var itemCount = 0

    @objc func menuWillOpen(_ menu: NSMenu) {
        opens += 1
        itemCount = menu.items.count
        // 菜单打开后运行自己的事件循环（此时主队列被占着）；用该循环里会触发的定时器关闭菜单。
        menu.perform(#selector(NSMenu.cancelTracking), with: nil, afterDelay: 0.3, inModes: [.eventTracking, .default])
    }
}
