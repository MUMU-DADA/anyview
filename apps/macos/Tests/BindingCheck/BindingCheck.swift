import Foundation
import AnyviewCore

/// Swift 接口测试：通过 UniFFI 调用 Rust 核心，检查同步函数、枚举转换、后台线程唤醒，
/// 以及打开、切换、取像素的完整往返。不依赖 XCTest（只装 Xcode 命令行工具时没有 XCTest）。
/// 用法：binding-check [测试图片目录]；全部通过时退出码为 0。
@main
enum BindingCheck {
    static func main() {
        var failures = 0
        func check(_ name: String, _ passed: Bool, _ detail: @autoclosure () -> String = "") {
            if passed {
                print("✓ \(name)")
            } else {
                failures += 1
                let extra = detail()
                print("✗ \(name)\(extra.isEmpty ? "" : "：\(extra)")")
            }
        }

        let version = coreVersion()
        check("核心版本号不为空", !version.isEmpty, version)

        let hint = emptyStateHint(platform: .macos, lang: .zhHans)
        check("macOS 空态提示用 ⌘O", hint == "将图片拖到窗口中，或按 ⌘O 打开图片", hint)
        let linuxHint = emptyStateHint(platform: .linux, lang: .en)
        check("Linux 空态提示用 F2", linuxHint.contains("F2"), linuxHint)

        check("zh-Hans-CN 对应简体中文", langFromLocale(tag: "zh-Hans-CN") == .zhHans)
        check("en-US 对应英文", langFromLocale(tag: "en-US") == .en)
        check("界面文本带占位符", uiText(key: .coreLinkOk, lang: .zhHans).contains("{ms}"))
        check("应用名", uiText(key: .appName, lang: .en) == "Anyview")
        check("支持的扩展名含 jpg", supportedExtensions().contains("jpg"))
        let fit = fitRect(imageWidth: 4000, imageHeight: 3000, viewWidth: 1800, viewHeight: 1200)
        check("适合窗口的计算", fit == FitRect(x: 100, y: 0, width: 1600, height: 1200), "\(fit)")

        // 动作目录与菜单数据。
        let bar = menuEntries(menu: .menuBar, platform: .macos, lang: .zhHans)
        if case let .submenuStart(title, role)? = bar.first {
            check("菜单栏第一项是应用菜单", role == .app && title == "Anyview", title)
        } else {
            check("菜单栏第一项是应用菜单", false, "\(String(describing: bar.first))")
        }
        var openFolderLabel = ""
        for entry in bar {
            if case let .action(id, _, shortcut, _) = entry, id == "open_folder" {
                openFolderLabel = shortcut?.label ?? ""
            }
        }
        check("打开文件夹的快捷键 ⇧⌘O", openFolderLabel == "⇧⌘O", openFolderLabel)
        let contextTitles = menuEntries(menu: .context, platform: .macos, lang: .zhHans).compactMap { entry -> String? in
            if case let .action(_, title, _, _) = entry {
                return title
            }
            return nil
        }
        check(
            "右键菜单用 macOS 措辞",
            contextTitles.contains("移到废纸篓") && contextTitles.contains("在访达中显示"),
            contextTitles.joined(separator: "、")
        )
        check("缩放提示文字", zoomText(scale: 1.5) == "150%", zoomText(scale: 1.5))

        let actionCore = AppCore(listener: TestListener(probe: WakeProbe()))
        let none = KeyModifiers(shift: false, control: false, alt: false, command: false)
        let command = KeyModifiers(shift: false, control: false, alt: false, command: true)
        check("打开图片的动作交给界面", actionCore.invokeAction(id: "open_file") == .ui(request: .openFileDialog))
        check("⌘O 解析为打开图片", actionCore.invokeKey(key: "o", modifiers: command) == .ui(request: .openFileDialog))
        check("⌘= 是放大（没有图片时不可用）", actionCore.invokeKey(key: "=", modifiers: command) == .unavailable)
        check("没有绑定的按键返回空", actionCore.invokeKey(key: "q", modifiers: none) == nil)
        check(
            "回车进入、退出全屏，Esc 关窗口或退出，G 选择图片",
            actionCore.actionForKey(key: "Enter", modifiers: none) == "fullscreen"
                && actionCore.actionForKey(key: "Escape", modifiers: none) == "close_or_quit"
                && actionCore.actionForKey(key: "g", modifiers: none) == "select_image"
        )
        check("默认适合窗口", actionCore.actionState(id: "fit_window") == ActionState(enabled: true, checked: true))
        check("没有图片时没有绘制参数", actionCore.viewLayout() == nil)

        // 悬浮界面的规则与文字。
        check("左边 1/6 是箭头区", hoverZone(x: 10, y: 300, width: 900, height: 600) == .left)
        check("底部是控制栏区", hoverZone(x: 450, y: 590, width: 900, height: 600) == .bar)
        check("进度条一半处是第 5 张（共 9 张）", progressIndex(fraction: 0.5, total: 9) == 4)
        check("进度提示", positionText(index: 840, total: 1862) == "841/1862")
        check("按钮提示带快捷键", actionTooltip(id: "open_file", platform: .macos, lang: .zhHans) == "打开图片 (⌘O)")
        check("淡入 150 ms", overlayTiming().fadeInMs == 150)
        check("图片信息开关", actionCore.invokeAction(id: "toggle_info") == .overlayChanged)

        // 到达首尾的询问与删除确认的文字。
        let boundary = boundaryPrompt(forward: true, archive: false, lang: .zhHans)
        check("末张询问的按钮", boundary.buttons == ["转到首张", "转到下一个文件夹", "取消"], boundary.buttons.joined(separator: "、"))
        let archiveBoundary = boundaryPrompt(forward: true, archive: true, lang: .zhHans)
        check("压缩包末张询问的按钮", archiveBoundary.buttons[1] == "转到下一个压缩包", archiveBoundary.buttons.joined(separator: "、"))
        check(
            "复制完成的提示",
            noticeText(kind: .copied(folder: "图片", name: "a (2).png"), lang: .zhHans) == "已复制到“图片”：a (2).png"
        )
        let trash = trashPrompt(platform: .macos, lang: .zhHans, name: "a.png")
        check("删除确认用废纸篓", trash.buttons.first == "移到废纸篓" && trash.message.contains("a.png"), trash.message)
        check("没有图片时没有当前路径", actionCore.currentPath() == nil)
        check("没有图片时不能删除", actionCore.invokeAction(id: "delete_file") == .unavailable)
        check("精确重绘在静止 100 ms 后", exactRenderDelayMs() == 100)
        check("没有图片时不需要精确重绘", !actionCore.needsExact())

        // 设置（docs/SETTINGS.md）：读文件、分类、草稿、校验、确定后生效并保存。
        let settingsDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("anyview-binding-settings-\(ProcessInfo.processInfo.processIdentifier)")
        try? FileManager.default.removeItem(at: settingsDir)
        let settingsFile = settingsDir.appendingPathComponent("settings.toml")
        let settingsCore = AppCore(listener: TestListener(probe: WakeProbe()))
        check("设置文件还不存在时用默认值", settingsCore.loadSettings(path: settingsFile.path) == .missing)
        let settingsPages = settingsCore.settingsPages(lang: .zhHans)
        check(
            "设置的分类（macOS 没有上下文菜单页）",
            settingsPages.first?.title == "一般" && !settingsPages.contains { $0.id == "context_menu" },
            settingsPages.map(\.title).joined(separator: "、")
        )
        settingsCore.settingsBegin()
        let topItem = settingsCore.settingsPage(page: "general", lang: .zhHans)?.items.first {
            $0.key == "general.always_on_top"
        }
        check(
            "总在最前的键位按 macOS 显示（标题后面灰色的快捷键）",
            topItem?.label == "总在最前" && topItem?.shortcut == "⇧⌘A",
            "\(topItem?.label ?? "") \(topItem?.shortcut ?? "")"
        )
        // 重新设计的设置窗口：浏览页分成三组；"控制栏"开关是翻过来说的（存的是"不显示控制栏"）。
        let browsePage = settingsCore.settingsPage(page: "browse", lang: .zhHans)
        check(
            "浏览页分成三组",
            browsePage?.sections == ["窗口", "画面上显示", "背景"],
            browsePage?.sections.joined(separator: "、") ?? ""
        )
        let barItem = browsePage?.items.first { $0.key == "browse.hide_control_bar" }
        var barOn = false
        if case let .toggle(on)? = barItem?.control {
            barOn = on
        }
        check(
            "控制栏开关翻过来说",
            barItem?.label == "控制栏" && barItem?.inverted == true && barOn,
            barItem?.label ?? ""
        )
        check(
            "改草稿",
            settingsCore.settingsSet(key: "image.cache_mb", input: .text(text: "50")) && settingsCore.settingsDirty()
        )
        var invalidPage = ""
        if case let .invalid(page, key, _, _) = settingsCore.settingsApply(lang: .zhHans), key == "image.cache_mb" {
            invalidPage = page
        }
        check("缓存大小填 50 时校验失败，停在图像处理页", invalidPage == "image", invalidPage)
        _ = settingsCore.settingsSet(key: "image.cache_mb", input: .text(text: "512"))
        _ = settingsCore.settingsSet(key: "browse.background_color", input: .color(value: 0x336699))
        var appliedCount = -1
        if case let .applied(changed, saveError) = settingsCore.settingsApply(lang: .zhHans), saveError == nil {
            appliedCount = changed.count
        }
        check("确定后生效并保存", appliedCount == 2, "\(appliedCount)")
        check("界面设置里的背景色", settingsCore.uiSettings().backgroundColor == 0x336699)
        let savedText = (try? String(contentsOf: settingsFile, encoding: .utf8)) ?? ""
        check("设置文件已写出", savedText.contains("#336699"), savedText)
        // 键盘、鼠标映射：映射优先于默认键位；菜单显示实际生效的按键；鼠标按键按映射执行。
        let noMods = KeyModifiers(shift: false, control: false, alt: false, command: false)
        check("PageDown 来自默认勾选的映射", settingsCore.actionForKey(key: "PageDown", modifiers: noMods) == "next_container")
        settingsCore.settingsBegin()
        _ = settingsCore.settingsSet(key: "keyboard.mappings", input: .keyRowEnabled(row: 2, value: true))
        _ = settingsCore.settingsSet(
            key: "keyboard.mappings",
            input: .keyRowKey(row: 2, key: "Right", modifiers: noMods)
        )
        _ = settingsCore.settingsSet(key: "keyboard.mappings", input: .keyRowAction(row: 2, id: "zoom_out"))
        let reserved = settingsCore.settingsSet(
            key: "keyboard.mappings",
            input: .keyRowKey(row: 3, key: "Tab", modifiers: command)
        )
        check("⌘Tab 由系统使用，不能映射", !reserved)
        // 鼠标每个按键一个下拉框：选了动作就是启用映射。
        _ = settingsCore.settingsSet(key: "mouse.wheel_down", input: .mappingChoice(id: "zoom_in"))
        if case .applied = settingsCore.settingsApply(lang: .zhHans) {
            check("映射确定后生效", settingsCore.actionForKey(key: "Right", modifiers: noMods) == "zoom_out")
        } else {
            check("映射确定后生效", false)
        }
        var nextShortcut = "?"
        for entry in settingsCore.menuEntries(menu: .context, lang: .zhHans) {
            if case let .action(id, _, shortcut, _) = entry, id == "next_image" {
                nextShortcut = shortcut?.label ?? ""
            }
        }
        check("→ 被映射占用后，右键菜单的\"下一张\"不再显示 →", nextShortcut.isEmpty, nextShortcut)
        check("滚轮向下映射为放大", settingsCore.mouseAction(input: .wheelDown) == .action(id: "zoom_in"))
        check("右键默认弹出菜单", settingsCore.mouseAction(input: .right) == .default)
        check("默认双击最大化", settingsCore.leftDoubleClickAction(xFraction: 0.5) == "maximize")
        // 多个窗口：新窗口与原来的共用设置；Esc 是"关窗口，最后一个时退出"；吸附边缘、显示标题栏按设置。
        let second = settingsCore.newWindow(listener: TestListener(probe: WakeProbe()))
        check(
            "新窗口共用设置",
            second.uiSettings().backgroundColor == 0x336699 && settingsCore.windowCount() == 2,
            "\(second.windowCount())"
        )
        check("Esc 关窗口或退出", second.actionForKey(key: "Escape", modifiers: noMods) == "close_or_quit")
        let nearLeft = WindowFrame(x: 5, y: 100, width: 600, height: 400)
        let screenArea = WindowFrame(x: 0, y: 0, width: 1440, height: 900)
        check("吸附边缘默认不开", second.snapWindow(frame: nearLeft, area: screenArea) == nil)
        settingsCore.settingsBegin()
        _ = settingsCore.settingsSet(key: "general.snap_window", input: .bool(value: true))
        _ = settingsCore.settingsSet(key: "browse.show_main_window", input: .bool(value: true))
        _ = settingsCore.settingsSet(key: "general.single_instance", input: .bool(value: true))
        if case .applied = settingsCore.settingsApply(lang: .zhHans) {
            check(
                "打开吸附边缘后贴到左边",
                second.snapWindow(frame: nearLeft, area: screenArea) == ScreenPoint(x: 0, y: 100)
            )
            let ui = second.uiSettings()
            check("显示标题栏、复用窗口在另一个窗口也生效", ui.showTitleBar && ui.snapToEdges && ui.reuseWindow)
        } else {
            check("窗口设置确定后生效", false)
        }
        try? FileManager.default.removeItem(at: settingsDir)

        // 系统解码器：Swift 实现的解码器由核心在后台线程调用（Mac 上的 HEIC、AVIF 走这条路）。
        check("登记系统解码器", registerPlatformDecoder(decoder: FakeDecoder(), extensions: ["anyviewtest"]))
        check("登记后扩展名可以打开", supportedExtensions().contains("anyviewtest"))
        let fakeFile = FileManager.default.temporaryDirectory.appendingPathComponent("anyview-binding.anyviewtest")
        try? Data("x".utf8).write(to: fakeFile)
        let decoderProbe = WakeProbe()
        let decoderCore = AppCore(listener: TestListener(probe: decoderProbe))
        decoderCore.dispatch(command: .`open`(path: fakeFile.path))
        let fakeReady = waitFor(decoderCore, decoderProbe, seconds: 10) { event in
            if case .imageReady = event { return true }
            return false
        }
        if case let .imageReady(_, _, image)? = fakeReady {
            check("系统解码器的结果送达", image.formatName() == "FAKE" && image.width() == 2 && image.height() == 1)
        } else {
            check("系统解码器的结果送达", false, "\(String(describing: fakeReady))")
        }
        try? FileManager.default.removeItem(at: fakeFile)
        check("Lanczos 已可用", actionCore.actionState(id: "interp_lanczos").enabled)

        // 单次往返：核心在后台线程唤醒，唤醒后取出的事件应恰好是对应的 Pong。
        let probe = WakeProbe()
        let core = AppCore(listener: TestListener(probe: probe))
        core.dispatch(command: .ping(token: 42))
        let pong = waitFor(core, probe, seconds: 5) { event in
            if case .pong(let token) = event { return token == 42 }
            return false
        }
        check("Ping 后 5 秒内收到 Pong", pong != nil)
        check("唤醒来自后台线程", probe.wokeOnMainThread == false, "wokeOnMainThread=\(String(describing: probe.wokeOnMainThread))")

        // 合并唤醒不丢事件：连发 200 个 Ping，全部 Pong 都要收到。
        let many = WakeProbe()
        let busy = AppCore(listener: TestListener(probe: many))
        let count: UInt64 = 200
        for token in 0..<count {
            busy.dispatch(command: .ping(token: token))
        }
        var tokens = Set<UInt64>()
        let deadline = Date().addingTimeInterval(10)
        while tokens.count < Int(count) && Date() < deadline {
            _ = many.semaphore.wait(timeout: .now() + 0.5)
            for event in busy.drainEvents() {
                if case .pong(let token) = event {
                    tokens.insert(token)
                }
            }
        }
        check("200 个 Ping 全部收到 Pong", tokens == Set(0..<count), "收到 \(tokens.count) 个")

        // 打开文件夹、取像素、切换（需要测试图片目录）。
        if CommandLine.arguments.count > 1 {
            let sortDir = URL(fileURLWithPath: CommandLine.arguments[1]).appendingPathComponent("sort").path
            let viewerProbe = WakeProbe()
            let viewer = AppCore(listener: TestListener(probe: viewerProbe))
            viewer.dispatch(command: .`open`(path: sortDir))
            let first = waitFor(viewer, viewerProbe, seconds: 10) { event in
                if case .imageReady = event { return true }
                return false
            }
            if case let .imageReady(_, path, image)? = first {
                check("打开文件夹后显示第一张 1.png", path.hasSuffix("/1.png"), path)
                check("图像尺寸 32x32", image.width() == 32 && image.height() == 32)
                check("每行字节数与数据长度", image.stride() == 128 && image.byteLen() == 4096)
                if let base = UnsafeRawPointer(bitPattern: UInt(image.pixelAddress())) {
                    let alpha = base.load(fromByteOffset: 3, as: UInt8.self)
                    check("像素可直接读取（不透明）", alpha == 255, "alpha=\(alpha)")
                } else {
                    check("像素地址有效", false)
                }
                let texture = image.texture()
                check(
                    "普通图片的纹理就是它自己（没有代理）",
                    !image.hasProxy() && texture.width() == 32 && texture.pixelAddress() == image.pixelAddress()
                )
            } else {
                check("打开文件夹后收到图像", false, "\(String(describing: first))")
            }
            // 查看计算：32x32 的图放进 100x100 的视口，按原始大小居中；顺时针旋转后左上角到了右上。
            _ = viewer.viewSetViewport(width: 100, height: 100)
            let layout = viewer.viewShowImage(width: 32, height: 32)
            check("小图按原始大小居中", layout?.scale == 1 && layout?.centerX == 50, "\(String(describing: layout))")
            check(
                "顺时针旋转后菜单勾选跟着变",
                viewer.invokeAction(id: "rotate_cw") == .viewChanged(showZoom: false)
                    && viewer.actionState(id: "rotate_cw").checked
            )
            let before = viewer.viewSignature()
            _ = viewer.viewZoomBy(factor: 2, x: 50, y: 50)
            check("缩放后画面签名改变", viewer.viewSignature() != before)
            _ = viewer.viewZoomBy(factor: 0.5, x: 50, y: 50)
            let corner = viewer.viewImageToScreen(u: 0, v: 0)
            check("旋转后图片左上角在右上", corner == ScreenPoint(x: 66, y: 34), "\(String(describing: corner))")
            let infoLines = viewer.infoLines(lang: .zhHans)
            check("图片信息第一行带序号", infoLines.first?.hasPrefix("文件名: 1.png [1/") == true, infoLines.joined(separator: " | "))
            check("图片信息有四行", infoLines.count == 4, "\(infoLines.count)")
            viewer.dispatch(command: .navigate(navigation: .next))
            let second = waitFor(viewer, viewerProbe, seconds: 10) { event in
                if case .imageReady = event { return true }
                return false
            }
            if case let .imageReady(_, path, _)? = second {
                check("下一张是 2.png（自然排序）", path.hasSuffix("/2.png"), path)
            } else {
                check("切换后收到图像", false, "\(String(describing: second))")
            }
            // 快速预览：600 万像素的 JPEG 先送 EXIF 缩略图（拉伸到原图尺寸），清晰图随后以 imageUpdated 送达。
            let previewPath = URL(fileURLWithPath: CommandLine.arguments[1])
                .appendingPathComponent("preview/thumbnail_3000x2000.jpg").path
            viewer.dispatch(command: .`open`(path: previewPath))
            // 一直等到清晰图；途中见到的预览记下来（两个事件可能在同一批里取出）。
            var preview: Event?
            let sharp = waitFor(viewer, viewerProbe, seconds: 10) { event in
                if case .imagePreview = event { preview = event }
                if case .imageUpdated = event { return true }
                return false
            }
            if case let .imagePreview(_, _, image, width, height)? = preview {
                check(
                    "快速预览：缩略图 160x107，原图 3000x2000",
                    image.width() == 160 && image.height() == 107 && width == 3000 && height == 2000,
                    "\(image.width())x\(image.height())，\(width)x\(height)"
                )
            } else {
                check("大 JPEG 先送快速预览", false, "\(String(describing: preview))")
            }
            if case let .imageUpdated(_, _, full)? = sharp {
                check("清晰图随后以 imageUpdated 送达", full.width() == 3000 && full.height() == 2000)
            } else {
                check("清晰图随后以 imageUpdated 送达", false, "\(String(describing: sharp))")
            }
        } else {
            print("（没有给测试图片目录，跳过打开与切换的检查）")
        }

        print(failures == 0 ? "全部通过" : "\(failures) 项没通过")
        exit(failures == 0 ? 0 : 1)
    }

    /// 取出事件直到出现满足条件的一个；超时返回 nil。
    static func waitFor(_ core: AppCore, _ probe: WakeProbe, seconds: Double, _ match: (Event) -> Bool) -> Event? {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            for event in core.drainEvents() where match(event) {
                return event
            }
            _ = probe.semaphore.wait(timeout: .now() + 0.2)
        }
        return nil
    }
}

/// 测试用的系统解码器：不管文件内容，返回 2x1 的图像。
final class FakeDecoder: PlatformDecoder, @unchecked Sendable {
    func decode(path: String, autoOrient: Bool, useIcc: Bool) -> PlatformImage? {
        PlatformImage(
            width: 2,
            height: 1,
            pixels: Data([1, 2, 3, 255, 4, 5, 6, 255]),
            format: "FAKE",
            pixelFormat: "RGB24",
            hasAlpha: false,
            iccProfile: nil
        )
    }

    func frameCount(path: String) -> UInt32 {
        1
    }

    func decodeFrame(path: String, index: UInt32) -> PlatformFrame? {
        nil
    }
}

/// 记录唤醒情况。写在唤醒线程、读在主线程，信号量保证先写后读。
final class WakeProbe: @unchecked Sendable {
    let semaphore = DispatchSemaphore(value: 0)
    var wokeOnMainThread: Bool?
}

private final class TestListener: CoreListener, @unchecked Sendable {
    private let probe: WakeProbe

    init(probe: WakeProbe) {
        self.probe = probe
    }

    func wake() {
        if probe.wokeOnMainThread == nil {
            probe.wokeOnMainThread = Thread.isMainThread
        }
        probe.semaphore.signal()
    }
}
