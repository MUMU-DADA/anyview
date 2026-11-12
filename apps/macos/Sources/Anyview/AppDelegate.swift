import AppKit
import IOKit.pwr_mgt
import UniformTypeIdentifiers
import AnyviewCore

/// 整个类明确放在主线程上：只实现了 NSApplicationDelegate 时，Swift 只把协议规定的方法当作主线程方法，
/// 自己加的方法调用主线程上的类型时会编译失败。
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let options: LaunchOptions
    /// 系统语言（设置为"跟随系统"时用它）。
    private let systemLang: Lang
    /// 界面语言：设置里选的，或跟随系统。
    private var lang: Lang
    /// 启动时建好的第一个窗口的核心；第一个窗口建好后交给它，这里不再留着。
    private var launchCore: CoreBridge?
    /// 所有主窗口（从访达打开图片时开新窗口）。最后一个窗口关掉后留着（藏起来），
    /// 点程序坞图标或再打开图片时重新显示。
    private var controllers: [MainWindowController] = []
    /// 最近在前面的主窗口：设置窗口在前面时，菜单栏的动作交给它。
    private weak var lastActive: MainWindowController?
    private var settingsController: SettingsWindowController?
    /// 打开着的帮助窗口。
    private var helpControllers: [HelpWindowController] = []
    /// “生成信息”面板：一个程序一个，跟着打开它的那个看图窗口。
    private var generationPanel: GenerationPanelController?
    private var menuActions: MenuActions?
    private let recentMenu = RecentMenuController()
    private var selfTest: SelfTest?
    /// 命令行的 --open 在启动时就交给了核心：交出的时刻。
    private var earlyOpenAt: UInt64?
    /// 启动完成（第一个窗口已经显示）。之前从访达打开的图片交给第一个窗口。
    private var launched = false
    /// 启动过程中从访达收到了要打开的图片（不是直接启动）。
    private var openedAtLaunch = false
    /// 阻止屏保：向系统申请的"不要让显示器睡眠"；没有申请时为 0。
    private var sleepAssertion: IOPMAssertionID = 0

    /// 核心在建窗口之前就建好：命令行或访达给了图片时马上开始读，与启动、建窗口同时进行；
    /// 核心送来的事件先存在 CoreBridge 里，窗口准备好后再一起交给界面。
    init(options: LaunchOptions) {
        self.options = options
        // HEIC、HEIF、AVIF 交给系统的 ImageIO；要在打开任何图片之前登记。
        _ = registerPlatformDecoder(decoder: ImageIODecoder(), extensions: ImageIODecoder.extensions)
        let bridge = CoreBridge()
        // 先读设置：EXIF 方向、ICC、快速预览等影响第一张图的解码。
        _ = bridge.core.loadSettings(path: options.settingsPath)
        // 偏好语言里第一种有翻译的：如"瑞典语、德语"时用德语。
        let system = langFromLocales(tags: Locale.preferredLanguages)
        systemLang = system
        lang = bridge.core.uiLang(system: system)
        launchCore = bridge
        super.init()
        if let path = options.openPath {
            earlyOpenAt = DispatchTime.now().uptimeNanoseconds
            bridge.open(path: path)
        }
        LaunchClock.mark("coreReady")
    }

    func applicationWillFinishLaunching(_ notification: Notification) {
        LaunchClock.mark("willFinishLaunching")
        // 只给测试用（截图核对亮色、暗色外观）：ANYVIEW_APPEARANCE=light|dark 让程序用这种外观，不改系统设置。
        switch ProcessInfo.processInfo.environment["ANYVIEW_APPEARANCE"] {
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
        default: break
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        LaunchClock.mark("didFinishLaunching")
        // 不用窗口标签页，菜单里也就不会出现标签页相关的项目。
        NSWindow.allowsAutomaticWindowTabbing = false
        guard let core = launchCore else { return }
        launchCore = nil
        // 菜单栏的动作交给前面的主窗口。
        let actions = MenuActions(core: { [weak self] in self?.activeController?.appCore })
        actions.onAction = { [weak self] id in
            // 生成信息面板、帮助、设置在前面时，⌘W、⌘M 关的是它们自己（不是后面的看图窗口）。设置窗口关的时候
            // 照常问要不要保存改动（windowShouldClose）。
            if let key = NSApp.keyWindow, !(key.windowController is MainWindowController) {
                switch id {
                case "close_window":
                    key.performClose(nil)
                    return
                case "minimize":
                    key.miniaturize(nil)
                    return
                default:
                    break
                }
            }
            self?.activeController?.invoke(id)
        }
        menuActions = actions
        let controller = makeController(core: core)
        recentMenu.onOpen = { [weak self] url in
            self?.openFromUser(url, in: nil)
        }
        // 窗口的位置：直接启动（没有图片）时固定大小、在系统选的屏幕上居中；带着图片启动时
        // 按上次的位置，大小随后跟着图片。自检时不用，每次都从 900x600 点开始。
        if options.selfTestDir == nil {
            if options.openPath != nil || openedAtLaunch {
                controller.restoreFrame()
            } else {
                controller.placeAtLaunchDefault()
            }
        }
        LaunchClock.mark("windowControllerReady")
        buildMainMenu()
        LaunchClock.mark("menuBuilt")

        if let reportDir = options.selfTestDir {
            let test = SelfTest(
                reportDir: reportDir,
                controller: controller,
                core: core,
                openPath: options.openPath,
                steps: options.steps,
                sampleAt100: options.sampleAt100,
                openDispatchedAt: earlyOpenAt
            )
            selfTest = test
            test.settingsWindow = { [weak self] in
                self?.settingsController
            }
            test.appWindows = { [weak self] in
                self?.controllers ?? []
            }
            test.finderOpen = { [weak self] url in
                self?.finderOpen(url)
            }
            test.preventingSleep = { [weak self] in
                self?.preventingSleep ?? false
            }
            test.helpWindows = { [weak self] in
                self?.helpControllers ?? []
            }
            test.generationPanel = { [weak self] in
                self?.generationPanel
            }
            test.start()
        } else {
            core.ping()
        }
        // 设置文件坏了时提示一次。
        if let notice = core.core.settingsNotice(lang: lang) {
            controller.showNotice(notice)
        }
        // 窗口和自检都准备好了：把启动期间存着的事件（如已经解码好的图片）交给界面。在窗口出现之前送出，
        // 按图片调整窗口大小和画图都赶在窗口第一次显示之前完成，不会先闪一下别的大小。
        core.startDelivering()
        controller.showWindow(nil)
        NSApp.activate()
        launched = true
        LaunchClock.mark("windowShown")
    }

    /// 建一个主窗口（第一个窗口用启动时的核心；之后的与它共用设置、缓存和后台线程）。
    private func makeController(core: CoreBridge) -> MainWindowController {
        let controller = MainWindowController(core: core, lang: lang)
        controller.onUiRequest = { [weak self, weak controller] request in
            guard let self, let controller else { return }
            self.handle(request, from: controller)
        }
        controller.isSelfTest = options.selfTestDir != nil
        controller.onOpenedByUser = { [weak self] url in
            self?.noteRecent(url)
        }
        controller.onBecameActive = { [weak self, weak controller] in
            self?.lastActive = controller
        }
        controller.onClosed = { [weak self, weak controller] in
            guard let self, let controller else { return }
            self.windowClosed(controller)
            self.updateSleepAssertion()
        }
        controller.onDisplayChanged = { [weak self] in
            self?.updateSleepAssertion()
        }
        controller.onSettingsReloaded = { [weak self] _ in
            self?.settingsReloaded()
        }
        controllers.append(controller)
        lastActive = controller
        return controller
    }

    /// 再开一个窗口：位置在当前窗口的右下方，等第一张图出来再显示。
    private func newWindow() -> MainWindowController {
        let anchor = activeController
        let bridge: CoreBridge
        if let sibling = controllers.first?.appCore {
            bridge = CoreBridge(sibling: sibling)
        } else {
            // 不会发生（最后一个窗口关掉后也留着）；万一没有窗口，就重新读设置。
            bridge = CoreBridge()
            _ = bridge.core.loadSettings(path: options.settingsPath)
        }
        let controller = makeController(core: bridge)
        if let anchor {
            controller.place(near: anchor)
        }
        bridge.startDelivering()
        return controller
    }

    /// 菜单栏的动作交给哪个窗口：前面的主窗口；设置窗口在前面时，交给最近在前面的主窗口。
    private var activeController: MainWindowController? {
        if let key = NSApp.keyWindow?.windowController as? MainWindowController {
            return key
        }
        if let main = NSApp.mainWindow?.windowController as? MainWindowController {
            return main
        }
        if let last = lastActive, controllers.contains(where: { $0 === last }) {
            return last
        }
        return controllers.last
    }

    /// 还开着的主窗口（关掉后留着的不算）。
    private var openControllers: [MainWindowController] {
        controllers.filter { !$0.isClosed }
    }

    /// 主窗口关掉了：还有别的窗口时放掉它（连同它的图片和核心）；是最后一个时留着，点程序坞图标时再显示。
    /// 关窗口的过程中还在窗口自己的方法里：等这次事件处理完再放掉。
    private func windowClosed(_ controller: MainWindowController) {
        guard openControllers.contains(where: { $0 !== controller }) else { return }
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated {
                guard let self, controller.isClosed else { return }
                let owned = controller.ownsFrameName
                controller.discard()
                self.controllers.removeAll { $0 === controller }
                if owned {
                    self.openControllers.last?.adoptFrameName()
                }
            }
        }
    }

    /// 菜单栏由核心的数据生成（菜单栏"）；设置生效后重新生成（语言、键盘映射，ACTIONS 规则 7）。
    private func buildMainMenu() {
        guard let actions = menuActions, let core = activeController?.appCore else { return }
        NSApp.mainMenu = MenuBuilder.build(
            core.menuEntries(menu: .menuBar, lang: lang),
            actions: actions,
            mainMenu: true,
            recent: recentMenu
        )
    }

    /// 退出前记下查看状态（"保存缩放状态"，按前面那个窗口的），关掉所有压缩包（删掉解出的临时文件）。
    func applicationWillTerminate(_ notification: Notification) {
        activeController?.appCore.shutdown()
        if sleepAssertion != 0 {
            IOPMAssertionRelease(sleepAssertion)
            sleepAssertion = 0
        }
    }

    /// 程序被隐藏（⌘H）、再显示：阻止显示器睡眠只在窗口看得见时生效。
    func applicationDidHide(_ notification: Notification) {
        updateSleepAssertion()
    }

    func applicationDidUnhide(_ notification: Notification) {
        updateSleepAssertion()
    }

    /// 设置"看图时阻止屏幕保护程序和显示器睡眠"：打开、而且有看得见的窗口在显示图片时向系统申请，否则撤销。
    private func updateSleepAssertion() {
        let wanted = (controllers.first?.appCore.uiSettings().noScreensaver ?? false)
            && openControllers.contains { $0.hasImage && $0.isOnScreen }
        if wanted && sleepAssertion == 0 {
            var assertion: IOPMAssertionID = 0
            let result = IOPMAssertionCreateWithName(
                "PreventUserIdleDisplaySleep" as CFString, // kIOPMAssertionTypePreventUserIdleDisplaySleep
                IOPMAssertionLevel(kIOPMAssertionLevelOn),
                "Anyview: viewing images" as CFString,
                &assertion
            )
            if result == kIOReturnSuccess {
                sleepAssertion = assertion
            }
        } else if !wanted && sleepAssertion != 0 {
            IOPMAssertionRelease(sleepAssertion)
            sleepAssertion = 0
        }
    }

    /// 自检用：现在有没有在阻止显示器睡眠。
    var preventingSleep: Bool {
        sleepAssertion != 0
    }

    /// 打开设置窗口（已经打开时放到最前面）。设置由各窗口共用。
    private func showSettings(from controller: MainWindowController) {
        if let existing = settingsController {
            existing.showWindow(nil)
            existing.window?.makeKeyAndOrderFront(nil)
            return
        }
        let settings = SettingsWindowController(core: controller.appCore, lang: lang)
        settings.onApplied = { [weak self] applied in
            self?.settingsApplied(applied)
        }
        // 窗口关掉时还在设置窗口自己的方法里：等这次事件处理完再放掉它。
        settings.onClosed = { [weak self] in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.settingsController = nil
                }
            }
        }
        settingsController = settings
        // 自检时校验失败不弹提示框（只记下来）。
        settings.quiet = options.selfTestDir != nil
        settings.showWindow(nil)
        settings.window?.makeKeyAndOrderFront(nil)
    }

    /// 设置点了"确定"、已经生效：刷新所有窗口；换了语言时重建菜单栏；写文件失败时提示。
    private func settingsApplied(_ applied: SettingsAppliedInfo) {
        guard let core = activeController?.appCore else { return }
        let newLang = core.uiLang(system: systemLang)
        lang = newLang
        for controller in controllers {
            controller.applyUiSettings(lang: newLang)
        }
        buildMainMenu()
        updateSleepAssertion()
        if let error = applied.saveError {
            activeController?.showNotice(
                uiText(key: .noticeSettingsSaveFailed, lang: lang).replacingOccurrences(of: "{message}", with: error)
            )
        }
        selfTest?.settingsApplied(applied)
    }

    /// 别的进程改了设置文件、核心已经重新读：像"确定"之后那样刷新所有窗口和菜单栏。
    private func settingsReloaded() {
        guard let core = activeController?.appCore else { return }
        let newLang = core.uiLang(system: systemLang)
        lang = newLang
        for controller in controllers {
            controller.applyUiSettings(lang: newLang)
        }
        buildMainMenu()
        updateSleepAssertion()
    }

    /// 帮助窗口（"帮助"菜单）：同一种已经开着时放到最前面。
    private func showHelp(_ kind: HelpKind, from controller: MainWindowController) {
        if let existing = helpControllers.first(where: { $0.kind == kind }) {
            existing.showWindow(nil)
            existing.window?.makeKeyAndOrderFront(nil)
            return
        }
        let help = HelpWindowController(kind: kind, core: controller.appCore, lang: lang)
        // 窗口关掉时还在它自己的方法里：等这次事件处理完再放掉它。
        help.onClosed = { [weak self, weak help] in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.helpControllers.removeAll { $0 === help }
                }
            }
        }
        helpControllers.append(help)
        help.showWindow(nil)
        help.window?.makeKeyAndOrderFront(nil)
    }

    /// “生成信息”面板：已经开着时换成这个窗口的当前图片、放到最前面。
    private func showGenerationInfo(from controller: MainWindowController) {
        if let panel = generationPanel {
            panel.show(from: controller, lang: lang)
            return
        }
        let panel = GenerationPanelController(source: controller, lang: lang)
        // 窗口关掉时还在它自己的方法里：等这次事件处理完再放掉它。
        panel.onClosed = { [weak self, weak panel] in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    if let self, self.generationPanel === panel {
                        self.generationPanel = nil
                    }
                }
            }
        }
        generationPanel = panel
        panel.show(from: controller, lang: lang)
    }

    /// 从访达打开（"打开方式"、拖到程序坞图标）。
    /// 启动过程中收到时，核心已经建好，马上开始读图；窗口随后出现。
    func application(_ application: NSApplication, open urls: [URL]) {
        guard let url = urls.first else { return }
        LaunchClock.mark("openRequested")
        guard launched else {
            openedAtLaunch = true
            noteRecent(url)
            launchCore?.open(path: url.path(percentEncoded: false))
            return
        }
        _ = finderOpen(url)
    }

    /// 程序开着时从访达打开：有空着的窗口就用它；设置"从访达打开图片时复用当前窗口"打开时用前面的窗口；
    /// 否则开新窗口。返回用的窗口和用法（自检记录）。
    @discardableResult
    private func finderOpen(_ url: URL) -> (controller: MainWindowController, how: String) {
        let target: MainWindowController
        let how: String
        if openControllers.isEmpty, let hidden = controllers.last {
            // 最后一个窗口关掉后留着的：重新显示它。
            target = hidden
            how = "reopened"
        } else if let active = activeController, !active.isClosed, !active.hasImage {
            target = active
            how = "empty"
        } else if let empty = openControllers.first(where: { !$0.hasImage }) {
            target = empty
            how = "empty"
        } else if let active = activeController, active.appCore.uiSettings().reuseWindow {
            target = active
            how = "reused"
        } else {
            let controller = newWindow()
            noteRecent(url)
            controller.open(url)
            controller.showAfterFirstImage()
            NSApp.activate()
            return (controller, "new")
        }
        openFromUser(url, in: target)
        NSApp.activate()
        return (target, how)
    }

    /// 打开用户选的文件或文件夹，并记入"打开最近使用"。没有指定窗口时用前面的窗口。
    private func openFromUser(_ url: URL, in target: MainWindowController?) {
        noteRecent(url)
        guard let controller = target ?? activeController else { return }
        controller.showWindow(nil)
        controller.window?.makeKeyAndOrderFront(nil)
        controller.open(url)
    }

    /// 记入系统的最近文档（"打开最近使用"）；自检时不记，免得在 Mac 上留下记录。
    /// 设置"不保存浏览过的文件夹路径"打开时也不记。
    private func noteRecent(_ url: URL) {
        guard options.selfTestDir == nil, let core = controllers.first?.appCore ?? launchCore?.core,
              !core.uiSettings().noRecent
        else { return }
        NSDocumentController.shared.noteNewRecentDocumentURL(url)
    }

    /// 关闭最后一个窗口后应用继续运行。
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    /// 点击程序坞图标时，如果没有可见窗口就重新显示主窗口。
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag {
            (openControllers.last ?? controllers.last)?.showWindow(nil)
        }
        return true
    }

    /// 退出程序。自检时交给自检记录（不真的退出，报告写完后自检自己结束）。
    private func quit() {
        if let test = selfTest {
            test.quitRequested()
            return
        }
        NSApp.terminate(nil)
    }

    /// 动作中需要应用层完成的部分（核心返回的 UiRequest）。`controller` 是执行动作的窗口。
    private func handle(_ request: UiRequest, from controller: MainWindowController) {
        let window = controller.window
        switch request {
        case .openFileDialog:
            presentOpenPanel(directories: false, for: controller)
        case .openFolderDialog:
            presentOpenPanel(directories: true, for: controller)
        case .quit:
            quit()
        case .closeOrQuit:
            // Esc：开着别的窗口时只关这个窗口；这是最后一个窗口时退出程序。
            if openControllers.contains(where: { $0 !== controller }) {
                window?.close()
            } else {
                quit()
            }
        case .toggleFullscreen:
            controller.toggleFullScreen()
        case .minimize:
            window?.miniaturize(nil)
        case .toggleMaximize:
            controller.toggleMaximize()
        case .showAbout:
            // 系统的"关于"面板；下面一行写许可证，许可证原文在"帮助 › 开源许可"。
            NSApp.orderFrontStandardAboutPanel(options: [
                .credits: NSAttributedString(string: licenseLine(lang: lang), attributes: [
                    .font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize),
                    .foregroundColor: NSColor.secondaryLabelColor,
                ]),
            ])
        case .showContextMenu:
            controller.showContextMenu()
        case .closeWindow:
            window?.performClose(nil)
        case .hideApp:
            NSApp.hide(nil)
        case .hideOthers:
            NSApp.hideOtherApplications(nil)
        case .showAll:
            NSApp.unhideAllApplications(nil)
        case let .trashFile(path, name, confirm):
            controller.trash(path: path, name: name, confirm: confirm)
        case let .revealFile(path):
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
        case let .copyImage(path):
            controller.copyImage(path: path)
        case let .openInEditor(path, editor):
            controller.openInEditor(path: path, editor: editor)
        case let .setWallpaper(path):
            controller.setWallpaper(path: path)
        case let .print(name):
            controller.printImage(name: name)
        case .clearRecent:
            NSDocumentController.shared.clearRecentDocuments(nil)
        case .showSettings:
            showSettings(from: controller)
        case .showPicker:
            controller.showPicker()
        case let .showHelp(kind):
            showHelp(kind, from: controller)
        case .showGenerationInfo:
            showGenerationInfo(from: controller)
        case .applySettings:
            // 设置是各窗口共用的（如"总在最前"）。
            for each in controllers {
                each.applyUiSettings()
            }
        }
    }

    private func presentOpenPanel(directories: Bool, for controller: MainWindowController) {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseFiles = !directories
        panel.canChooseDirectories = directories
        if !directories {
            // 图片和压缩包。
            panel.allowedContentTypes = (supportedExtensions() + archiveExtensions())
                .compactMap { UTType(filenameExtension: $0) }
        }
        panel.begin { [weak self, weak controller] response in
            MainActor.assumeIsolated {
                guard response == .OK, let url = panel.url, let self else { return }
                self.openFromUser(url, in: controller)
            }
        }
    }
}
