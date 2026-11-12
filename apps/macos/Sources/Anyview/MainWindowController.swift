import AppKit
import AnyviewCore

/// 主窗口：画布、空态、右上角通知、中央倍率提示。
/// 处理核心事件；按键、菜单、鼠标手势都交给核心执行动作，这里只把结果落到界面上。
final class MainWindowController: NSWindowController, NSWindowDelegate {
    let canvas: ImageCanvasView
    let emptyView: EmptyStateView
    private let noticeLabel: NSTextField
    private let zoomLabel: NSTextField
    private var noticeSerial = 0
    private var zoomSerial = 0
    private let core: CoreBridge
    /// 界面语言：设置里选的，或跟随系统；改了设置后立即换。
    private(set) var lang: Lang
    /// 界面相关的设置（背景色、隐藏控件等），设置生效后重新读。
    private(set) var uiSettings: UiSettings
    /// 右键菜单的动作交给这个窗口（多个窗口各自弹自己的菜单）。
    private lazy var contextActions: MenuActions = {
        let actions = MenuActions(core: { [weak self] in self?.core.core })
        actions.onAction = { [weak self] id in
            self?.invoke(id)
        }
        return actions
    }()
    /// 控制栏、边缘箭头、左上角图片信息（MENUS）。
    let overlays: OverlayViews

    private(set) var generation: UInt64 = 0
    private(set) var currentName = ""
    private(set) var currentIndex: UInt32?
    private(set) var currentTotal: UInt32?

    /// 需要应用层完成的操作（打开面板、退出、全屏等）。
    var onUiRequest: ((UiRequest) -> Void)?
    /// 窗口关掉了（还有别的窗口时由应用放掉它）。
    var onClosed: (() -> Void)?
    /// 窗口到了前面（菜单栏的动作交给它）。
    var onBecameActive: (() -> Void)?
    /// 窗口已经关掉（最后一个窗口关掉后留着，点程序坞图标时再显示）。
    private(set) var isClosed = false
    /// 新开的窗口等第一张图出来、按图片定好大小再显示（窗口一出现就是图片的大小）。
    private var showWhenImageReady = false
    /// 用户在拖动标题栏移动窗口（松开后吸附边缘）。
    private var userMoving = false
    /// 正在等用户松开鼠标（只等一次）。
    private var moveCheckPending = false
    /// 设置"显示标题栏"现在的状态。
    private(set) var titleBarAlwaysShown = false
    /// 用户拖进窗口打开的文件（记入"打开最近使用"）。
    var onOpenedByUser: ((URL) -> Void)?
    /// 自检时为真：删除不弹确认框、不在废纸篓里留东西，剪贴板写到自检专用的剪贴板。
    var isSelfTest = false
    /// 最近一次复制到剪贴板时写入的类型（自检核对用）。
    private(set) var lastCopyTypes: [String] = []
    /// 自检时读回的剪贴板图像正中存的数值（RGB）和图像带的色彩空间。
    private(set) var lastCopyCenter: [Int]?
    private(set) var lastCopyColorSpace: String?
    /// 自检用的钩子。到达首尾时返回 true 表示自检已处理，不弹对话框。
    var onBoundary: ((Bool) -> Bool)?
    /// 自检用：图片被安全上限拦下时不弹对话框（返回 true），由自检的下一步充当用户的选择。
    var onLimit: (() -> Bool)?
    var onPong: (() -> Void)?
    var onImageShown: ((ImageHandle) -> Void)?
    var onPreviewShown: (() -> Void)?
    /// 自检用：动图换了一帧。
    var onAnimationFrame: (() -> Void)?
    /// 自检用：复制、移动完成的提示。
    var onNotice: ((String) -> Void)?
    /// 交给外部的操作（编辑器、桌面图片、打印）：自检时只记下来，不真的做。
    var onExternal: ((String, String) -> Void)?
    /// 窗口里有没有图片变了（阻止屏保）。
    var onDisplayChanged: (() -> Void)?
    var onProblem: ((String) -> Void)?
    /// 标题栏的显示与隐藏。
    let titlebar: TitlebarControl
    /// 最大化：窗口占满屏幕可用区域，切图时保持，图片按原始大小或缩到放得下。
    private(set) var maximized = false
    /// 最大化之前的窗口位置：关掉"根据图片大小调节窗口大小"时，还原回到这里。
    private var frameBeforeMaximize: NSRect?
    /// 通知的位置：右上角，或设置关掉"在边角显示通知信息"时在中央。
    private var noticeCorner: [NSLayoutConstraint] = []
    private var noticeCenter: [NSLayoutConstraint] = []
    /// 即时全屏：没有动画，窗口盖满当前屏幕，菜单栏和程序坞自动隐藏。
    private(set) var instantFullScreen = false
    private var frameBeforeFullScreen: NSRect?
    private var presentationBeforeFullScreen: NSApplication.PresentationOptions = []
    private var autosaveNameBeforeFullScreen = ""
    /// 程序最近一次设定的窗口大小（点）；用户手动拖动窗口后为空。自检核对窗口实际大小用。
    private(set) var expectedContentSize: NSSize?
    /// 最近一次按图片调整的窗口大小（点，自检报告用）。
    private(set) var lastFitSize: NSSize?
    /// 记住窗口位置与大小用的名字。
    static let frameName = "AnyviewMainWindow"
    /// 打开着的选择图片窗口（每个主窗口一个）。
    private(set) var picker: PickerWindowController?
    /// 标题栏隐藏时在顶端显示的文件名。
    let filenameLabel = NSTextField(labelWithString: "")
    /// 别的进程改了设置文件、已经重新读：交给应用刷新所有窗口。
    var onSettingsReloaded: (([String]) -> Void)?
    /// 这一代次的切图要做的翻页效果：Current 时向核心要，图像送达时开始。
    private var pendingTurn: (generation: UInt64, turn: AnyviewCore.Transition)?
    /// 翻页音效：第一次放时用核心合成的 WAV 建好。
    private lazy var pageSound: NSSound? = NSSound(data: AnyviewCore.pageSoundWav())
    /// 放过几次翻页音效、用的哪种方式（自检记录；自检时不出声）。
    private(set) var soundLog: [String] = []

    init(core: CoreBridge, lang: Lang) {
        // 先用局部变量算好，再给存储属性赋值，避免在初始化完成前读取 self。
        let frame = NSRect(x: 0, y: 0, width: 900, height: 600)
        let canvas = ImageCanvasView(frame: frame)
        let emptyView = EmptyStateView(
            hint: emptyStateHint(platform: .macos, lang: lang),
            status: "\(uiText(key: .appName, lang: lang)) \(coreVersion())"
        )
        let noticeLabel = NSTextField(labelWithString: "")
        let overlays = OverlayViews()
        let zoomLabel = NSTextField(labelWithString: "")
        let titlebar = TitlebarControl()
        self.core = core
        self.lang = lang
        self.uiSettings = core.core.uiSettings()
        self.canvas = canvas
        self.emptyView = emptyView
        self.noticeLabel = noticeLabel
        self.overlays = overlays
        self.zoomLabel = zoomLabel
        self.titlebar = titlebar

        let container = NSView(frame: frame)
        container.wantsLayer = true
        canvas.frame = container.bounds
        canvas.autoresizingMask = [.width, .height]
        emptyView.frame = container.bounds
        emptyView.autoresizingMask = [.width, .height]
        container.addSubview(canvas)
        container.addSubview(emptyView)
        // 标题栏下面的深色渐变，在图片之上、悬浮控件之下。
        titlebar.install(in: container)

        noticeLabel.font = .systemFont(ofSize: 13)
        noticeLabel.textColor = NSColor(white: 0.95, alpha: 1)
        noticeLabel.drawsBackground = true
        noticeLabel.backgroundColor = NSColor(white: 0, alpha: 0.62)
        noticeLabel.isHidden = true
        noticeLabel.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(noticeLabel)

        zoomLabel.font = .systemFont(ofSize: 20, weight: .medium)
        zoomLabel.textColor = NSColor(white: 0.95, alpha: 1)
        zoomLabel.drawsBackground = true
        zoomLabel.backgroundColor = NSColor(white: 0, alpha: 0.55)
        zoomLabel.isHidden = true
        zoomLabel.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(zoomLabel)
        // 标题栏隐藏时显示的文件名：白字带阴影，顶端居中，与标题栏的文字同一高度。
        filenameLabel.font = .systemFont(ofSize: 13, weight: .medium)
        filenameLabel.textColor = NSColor(white: 0.96, alpha: 1)
        filenameLabel.alignment = .center
        filenameLabel.lineBreakMode = .byTruncatingMiddle
        let filenameShadow = NSShadow()
        filenameShadow.shadowColor = NSColor(white: 0, alpha: 0.85)
        filenameShadow.shadowBlurRadius = 3
        filenameShadow.shadowOffset = NSSize(width: 0, height: -1)
        filenameLabel.shadow = filenameShadow
        filenameLabel.isHidden = true
        filenameLabel.translatesAutoresizingMaskIntoConstraints = false
        filenameLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        container.addSubview(filenameLabel)
        NSLayoutConstraint.activate([
            filenameLabel.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            filenameLabel.topAnchor.constraint(equalTo: container.topAnchor, constant: 6),
            filenameLabel.widthAnchor.constraint(lessThanOrEqualTo: container.widthAnchor, constant: -160),
        ])
        overlays.install(in: container)
        overlays.titlebar = titlebar
        // 内容铺满整个窗口（标题栏透明）：靠上的控件放在标题栏下面（安全区域）。
        let noticeCorner = [
            noticeLabel.topAnchor.constraint(equalTo: container.safeAreaLayoutGuide.topAnchor, constant: 12),
            noticeLabel.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -16),
        ]
        let noticeCenter = [
            noticeLabel.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            noticeLabel.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ]
        NSLayoutConstraint.activate([
            zoomLabel.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            zoomLabel.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])

        // 标题栏透明、内容铺到标题栏下面，标题栏与控制栏一起自动隐藏。
        // 深色外观：标题文字在图片上面的深色渐变上看得清，与深灰背景一致。
        let window = AnyviewWindow(
            contentRect: frame,
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.titlebarAppearsTransparent = true
        window.appearance = NSAppearance(named: .darkAqua)
        window.title = uiText(key: .appName, lang: lang)
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 480, height: 360)
        window.contentView = container
        window.center()
        super.init(window: window)
        self.noticeCorner = noticeCorner
        self.noticeCenter = noticeCenter
        titlebar.attach(to: window)
        window.delegate = self
        titlebar.onShownChanged = { [weak self] in
            self?.refreshFilenameLabel()
        }
        // 绿色按钮、菜单"进入全屏幕"、⌃⌘F 都走程序自己的即时全屏。
        window.onToggleFullScreen = { [weak self] in
            self?.toggleFullScreen()
        }

        canvas.core = core.core
        canvas.onKey = { [weak self] event in
            self?.handleKey(event) ?? false
        }
        canvas.onOpenURL = { [weak self] url in
            self?.onOpenedByUser?(url)
            self?.core.open(path: url.path(percentEncoded: false))
        }
        canvas.onAction = { [weak self] id in
            self?.invoke(id)
        }
        canvas.onZoomChanged = { [weak self] in
            self?.showZoomHint()
        }
        canvas.makeContextMenu = { [weak self] in
            self?.contextMenu()
        }
        canvas.windowMovable = { [weak self] in
            self?.windowMovable ?? false
        }
        canvas.snapOrigin = { [weak self] frame in
            self?.snappedOrigin(for: frame)
        }
        canvas.onOutcome = { [weak self] outcome in
            self?.apply(outcome)
        }
        overlays.onAction = { [weak self] id in
            self?.invoke(id)
        }
        overlays.strip.onJump = { [weak self] index in
            self?.core.core.dispatch(command: .navigate(navigation: .jump(index: index)))
        }
        // 指针在窗口里移动时，按核心的区域规则显示控制栏和边缘箭头。
        container.addTrackingArea(NSTrackingArea(
            rect: .zero,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil
        ))
        core.onEvent = { [weak self] event in
            self?.handle(event)
        }
        window.makeFirstResponder(canvas)
        applyUiSettings()
    }

    /// 让界面相关的设置生效（启动时、设置窗口点"确定"后、"总在最前"切换后）：背景色、透明背景、
    /// 隐藏的控件、悬停提示、字号、通知位置、双击最大化、总在最前、界面语言。
    func applyUiSettings(lang newLang: Lang? = nil) {
        let settings = core.core.uiSettings()
        uiSettings = settings
        if let newLang, newLang != lang {
            lang = newLang
            emptyView.hint = emptyStateHint(platform: .macos, lang: newLang)
            updateTitle()
        }
        let background = Self.color(settings.backgroundColor)
        canvas.setBackground(background)
        canvas.refreshAlphaFill()
        refreshBackgroundImage(color: background)
        NSLayoutConstraint.deactivate(settings.cornerNotices ? noticeCenter : noticeCorner)
        NSLayoutConstraint.activate(settings.cornerNotices ? noticeCorner : noticeCenter)
        if settings.hideZoomInfo {
            zoomLabel.isHidden = true
        }
        window?.level = settings.alwaysOnTop ? .floating : .normal
        applyTitleBar(settings.showTitleBar)
        refreshFilenameLabel()
        overlays.applySettings(core: core.core, lang: lang, settings: settings)
        refreshOverlays()
    }

    /// 背景图片：解码好了才有；有时空态的视图透明，露出画布上的背景图片。
    private func refreshBackgroundImage(color: NSColor? = nil) {
        let image = core.core.backgroundImage()
        canvas.setBackgroundImage(image)
        emptyView.background = image == nil ? (color ?? Self.color(uiSettings.backgroundColor)) : .clear
    }

    /// 标题栏隐藏时显示文件名（设置或 ⇧Tab）：有图片、标题栏不是一直显示、现在藏起来了（或在全屏）时显示。
    func refreshFilenameLabel() {
        let show = uiSettings.titleFilename && !currentName.isEmpty && !titleBarAlwaysShown
            && (!titlebar.isShown || instantFullScreen)
        filenameLabel.stringValue = currentName
        filenameLabel.isHidden = !show
    }

    /// 设置"显示标题栏"：打开时是普通的带标题栏的窗口，标题栏一直显示，图片在标题栏下面；
    /// 关掉时是无边框的样子：图片铺到透明的标题栏下面，标题栏自动隐藏。
    /// 改了以后窗口按当前图片重新定大小：内容区仍是图片的大小，标题栏另加。
    private func applyTitleBar(_ shown: Bool) {
        guard let window, shown != titleBarAlwaysShown else { return }
        titleBarAlwaysShown = shown
        if shown {
            window.styleMask.remove(.fullSizeContentView)
            window.titlebarAppearsTransparent = false
        } else {
            window.styleMask.insert(.fullSizeContentView)
            window.titlebarAppearsTransparent = true
        }
        titlebar.setAlwaysShown(shown)
        // 最大化、全屏时窗口外框不变，内容区少了（或多了）标题栏的高度。
        if !followCurrentImage(), expectedContentSize != nil {
            expectedContentSize = contentSize(forFrame: window.frame)
        }
    }

    /// 0xRRGGBB → sRGB 颜色。
    static func color(_ rgb: UInt32) -> NSColor {
        NSColor(
            srgbRed: CGFloat((rgb >> 16) & 0xFF) / 255,
            green: CGFloat((rgb >> 8) & 0xFF) / 255,
            blue: CGFloat(rgb & 0xFF) / 255,
            alpha: 1
        )
    }

    /// 自检：现在挡在画面上的控件（内容视图坐标，y 向上）：悬浮界面、标题栏下的渐变，以及倍率提示和通知。
    var coveredRects: [NSRect] {
        var rects = overlays.coveredRects
        for label in [zoomLabel, noticeLabel, filenameLabel] where !label.isHidden {
            rects.append(label.frame.insetBy(dx: -4, dy: -4))
        }
        return rects
    }

    /// 悬浮界面跟着核心的状态刷新。
    func refreshOverlays() {
        overlays.refresh(core: core.core, index: currentIndex, total: currentTotal, lang: lang)
        // 开着的“生成信息”面板跟着这个窗口的当前图片换内容。
        if let panel = GenerationPanelController.current, panel.source === self {
            panel.refresh()
        }
    }

    override func mouseMoved(with event: NSEvent) {
        // 自检时不理真的鼠标（用户的指针正好停在窗口上会让控制栏出来， avif 那一项因此误报）；
        // 自检要模拟指针时直接调用 overlays.pointerMoved（hover_bottom）。
        guard !isSelfTest, let container = window?.contentView else { return }
        let point = container.convert(event.locationInWindow, from: nil)
        let height = Double(container.bounds.height)
        overlays.pointerMoved(
            x: Double(point.x),
            y: height - Double(point.y),
            width: Double(container.bounds.width),
            height: height,
            core: core.core
        )
    }

    override func mouseExited(with event: NSEvent) {
        guard !isSelfTest else { return }
        overlays.pointerExited(core: core.core)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    private func text(_ key: TextKey) -> String {
        uiText(key: key, lang: lang)
    }

    private func handle(_ event: Event) {
        switch event {
        case .pong:
            if let ms = core.lastPingMilliseconds {
                let link = text(.coreLinkOk).replacingOccurrences(of: "{ms}", with: String(format: "%.2f", ms))
                emptyView.status = "\(text(.appName)) \(coreVersion()) · \(link)"
            }
            onPong?()
        case let .current(generation, _, name, index, total, _):
            self.generation = generation
            // 翻页效果和音效：音效马上放，效果等新图像送达时做。
            if let turn = core.core.transitionFor(generation: generation) {
                pendingTurn = (generation, turn)
                if turn.sound {
                    playPageSound()
                }
            } else {
                pendingTurn = nil
            }
            Perf.beginShow()
            currentName = name
            currentIndex = index
            currentTotal = total
            updateTitle()
            refreshOverlays()
        case let .collection(generation, index, total):
            if generation == self.generation {
                currentIndex = index
                currentTotal = total
                updateTitle()
                refreshOverlays()
            }
            // 排序、文件夹变了：选择图片窗口重新载入列表。
            picker?.reload()
        case let .thumbnail(path, image):
            picker?.thumbnailArrived(path: path, image: image)
        case .backgroundChanged:
            refreshBackgroundImage()
        case let .settingsReloaded(changed):
            onSettingsReloaded?(changed)
        case let .imageReady(generation, _, image):
            if generation == self.generation {
                startTurn(generation)
                // 每切到一张新图片，窗口都跟着图片变大小。
                followImage(width: image.width(), height: image.height())
                canvas.show(image)
                Perf.imageShown(decodeMs: image.decodeMs(), width: image.width(), height: image.height())
                emptyView.isHidden = true
                window?.makeFirstResponder(canvas)
                refreshOverlays()
                showIfWaiting()
                onDisplayChanged?()
                onImageShown?(image)
            }
        case let .imagePreview(generation, _, image, width, height):
            if generation == self.generation {
                startTurn(generation)
                followImage(width: width, height: height)
                canvas.showPreview(image, width: width, height: height)
                Perf.previewShown()
                emptyView.isHidden = true
                window?.makeFirstResponder(canvas)
                refreshOverlays()
                showIfWaiting()
                onPreviewShown?()
            }
        case let .imageUpdated(generation, _, image):
            if generation == self.generation {
                canvas.replaceImage(image)
                // 清晰图替换预览后，图片信息才有完整内容。
                refreshOverlays()
                Perf.imageShown(decodeMs: image.decodeMs(), width: image.width(), height: image.height())
                onImageShown?(image)
            }
        case let .animationFrame(generation, _, image, _, _):
            // 动图的一帧：只换图像，更新动画信息控制。
            if generation == self.generation {
                canvas.replaceImage(image)
                overlays.refreshAnimation(core: core.core)
                onAnimationFrame?()
            }
        case let .exactReady(generation, signature, x, y, image, renderMs):
            if generation == self.generation {
                canvas.showExact(image, x: x, y: y, signature: signature, renderMs: renderMs)
                Perf.exactShown(renderMs: renderMs)
            }
        case let .imageFailed(generation, _, message):
            if generation == self.generation {
                canvas.show(nil)
                refreshOverlays()
                showIfWaiting()
                showNotice(text(.noticeImageFailed).replacingOccurrences(of: "{message}", with: message))
                onProblem?(message)
            }
        case let .imageBlocked(generation, path):
            if generation == self.generation {
                canvas.show(nil)
                refreshOverlays()
                showIfWaiting()
                askLimit(path: path)
            }
        case let .openFailed(_, message):
            showIfWaiting()
            showNotice(text(.noticeOpenFailed).replacingOccurrences(of: "{message}", with: message))
            onProblem?(message)
        case let .notice(kind):
            // 提示的文字由核心给出。复制、移动完成是正常的结果；其余在自检里算问题。
            let message = AnyviewCore.noticeText(kind: kind, lang: lang)
            showNotice(message)
            switch kind {
            case .copied, .moved:
                onNotice?(message)
            default:
                onProblem?("\(kind)")
            }
        case let .boundary(forward, archive):
            askBoundary(forward: forward, archive: archive)
        case let .closed(generation):
            self.generation = generation
            canvas.show(nil)
            onDisplayChanged?()
            emptyView.isHidden = false
            currentName = ""
            currentIndex = nil
            currentTotal = nil
            updateTitle()
            refreshOverlays()
        }
    }

    // MARK: 窗口跟着图片
    // 默认情况下窗口就是图片本身：每切到一张图片（打开、翻页、随机、换文件夹……）窗口都变成图片的大小，
    // 屏幕放不下时按比例缩到可用区域以内（大小由核心算）。手动放大缩小、翻转不改变窗口；旋转后窗口跟着转。
    // 用户拖动窗口边缘后，窗口按用户的大小，直到下一张图。最大化、全屏时窗口不变，退出后回到当前图片的大小。
    // 内容铺满整个窗口，窗口外框就是内容区的大小。

    /// 窗口现在跟不跟着图片变大小：普通窗口才跟（关掉"根据图片大小调节窗口大小"时核心不给大小）。
    private var followsImage: Bool {
        windowMovable
    }

    /// 窗口现在能不能移动、改大小：最大化、全屏时不能。
    var windowMovable: Bool {
        guard let window else { return false }
        return !maximized && !instantFullScreen && !window.styleMask.contains(.fullScreen)
    }

    /// 屏幕可用区域（去掉菜单栏和程序坞）。
    private var visibleFrame: NSRect? {
        (window?.screen ?? NSScreen.main)?.visibleFrame
    }

    /// 窗口外框比内容区高出多少：显示标题栏时是标题栏的高度；无边框时内容铺满整个窗口，为 0。
    private var chromeHeight: CGFloat {
        guard let window, titleBarAlwaysShown, window.styleMask.contains(.titled) else { return 0 }
        return max(window.frame.height - window.contentRect(forFrameRect: window.frame).height, 0)
    }

    /// 窗口外框为 frame 时内容区的大小。
    private func contentSize(forFrame frame: NSRect) -> NSSize {
        NSSize(width: frame.width, height: frame.height - chromeHeight)
    }

    /// 内容区最大能有多大：可用区域去掉标题栏。
    private func available(in visible: NSRect) -> NSSize {
        contentSize(forFrame: visible)
    }

    /// 切到新图片：窗口变成这张图片的大小。
    private func followImage(width: UInt32, height: UInt32) {
        guard followsImage, let visible = visibleFrame else { return }
        let room = available(in: visible)
        guard let size = core.core.windowSizeForImage(
            width: width,
            height: height,
            availableWidth: Double(room.width),
            availableHeight: Double(room.height)
        ) else { return }
        setWindowSize(NSSize(width: size.width, height: size.height), within: visible)
    }

    /// 窗口回到当前图片（按现在的方向）的大小：退出最大化、全屏时用。窗口不跟着图片时返回 false。
    @discardableResult
    private func followCurrentImage() -> Bool {
        guard followsImage, let visible = visibleFrame else { return false }
        let room = available(in: visible)
        guard let size = core.core.windowSizeForView(
            availableWidth: Double(room.width),
            availableHeight: Double(room.height)
        ) else { return false }
        setWindowSize(NSSize(width: size.width, height: size.height), within: visible)
        return true
    }

    /// 动作执行后：核心说要变（旋转）时窗口跟着变。
    func followAfterAction(_ id: String) {
        guard followsImage, let visible = visibleFrame else { return }
        let room = available(in: visible)
        guard let size = core.core.windowSizeAfterAction(
            id: id,
            availableWidth: Double(room.width),
            availableHeight: Double(room.height)
        ) else { return }
        setWindowSize(NSSize(width: size.width, height: size.height), within: visible)
    }

    /// 设置窗口内容区的大小（显示标题栏时窗口另加标题栏的高度）：窗口中心尽量不动，整个窗口留在可用区域以内。
    /// 不做动画（翻页要快）。
    private func setWindowSize(_ target: NSSize, within visible: NSRect) {
        guard let window else { return }
        var frame = window.frame
        let size = NSSize(width: target.width, height: target.height + chromeHeight)
        frame.origin = NSPoint(x: frame.midX - size.width / 2, y: frame.midY - size.height / 2)
        frame.size = size
        frame.origin.x = min(max(frame.origin.x, visible.minX), visible.maxX - frame.width)
        frame.origin.y = min(max(frame.origin.y, visible.minY), visible.maxY - frame.height)
        lastFitSize = target
        expectedContentSize = target
        window.setFrame(frame, display: true, animate: false)
    }

    /// 最大化、还原：最大化时窗口占满屏幕可用区域，切图时保持；还原时回到当前图片的大小。
    /// 系统的"缩放"（绿色按钮、双击标题栏）也走这里。
    func toggleMaximize() {
        guard let window, !instantFullScreen, !window.styleMask.contains(.fullScreen) else { return }
        if maximized {
            maximized = false
            // 未启用“根据图片大小调节窗口大小”时，回到最大化之前的位置和大小。
            if !followCurrentImage(), let frame = frameBeforeMaximize {
                expectedContentSize = contentSize(forFrame: frame)
                window.setFrame(frame, display: true, animate: false)
            }
        } else if let visible = visibleFrame {
            maximized = true
            frameBeforeMaximize = window.frame
            expectedContentSize = available(in: visible)
            window.setFrame(visible, display: true, animate: false)
        }
    }

    /// 自检：把窗口设成指定大小（模拟用户拖动窗口边缘），之后核对窗口确实是这个大小。
    func setContentSizeForTest(_ size: NSSize) {
        guard let visible = visibleFrame else { return }
        maximized = false
        setWindowSize(size, within: visible)
    }

    /// 进入、退出全屏（回车、绿色按钮、⌃⌘F）。
    func toggleFullScreen() {
        if instantFullScreen {
            exitFullScreen()
        } else {
            enterFullScreen()
        }
    }

    /// 即时全屏：没有动画，窗口去掉标题栏、盖满当前屏幕（含菜单栏的位置），菜单栏和程序坞自动隐藏
    /// （指针移到屏幕顶端、底端时出现）。窗口不跟着图片变。
    private func enterFullScreen() {
        guard let window = window as? AnyviewWindow, !instantFullScreen,
              let screen = window.screen ?? NSScreen.main
        else { return }
        frameBeforeFullScreen = window.frame
        presentationBeforeFullScreen = NSApp.presentationOptions
        // 全屏时的窗口位置不记，退出后恢复记录。
        autosaveNameBeforeFullScreen = window.frameAutosaveName
        if !autosaveNameBeforeFullScreen.isEmpty {
            _ = window.setFrameAutosaveName("")
        }
        instantFullScreen = true
        titlebar.setFullScreen(true)
        NSApp.presentationOptions = [.autoHideDock, .autoHideMenuBar]
        window.coversScreen = true
        window.styleMask.remove(.titled)
        expectedContentSize = screen.frame.size
        window.setFrame(screen.frame, display: true, animate: false)
        window.makeFirstResponder(canvas)
    }

    /// 退出即时全屏：窗口回到原来的位置，再回到当前图片的大小；之前是最大化的就还是最大化。
    func exitFullScreen() {
        guard let window = window as? AnyviewWindow, instantFullScreen else { return }
        instantFullScreen = false
        window.styleMask.insert(.titled)
        window.coversScreen = false
        NSApp.presentationOptions = presentationBeforeFullScreen
        if let frame = frameBeforeFullScreen {
            expectedContentSize = contentSize(forFrame: frame)
            window.setFrame(frame, display: true, animate: false)
        }
        if !autosaveNameBeforeFullScreen.isEmpty {
            _ = window.setFrameAutosaveName(autosaveNameBeforeFullScreen)
        }
        titlebar.setFullScreen(false)
        followCurrentImage()
        window.makeFirstResponder(canvas)
        refreshOverlays()
    }

    func windowShouldZoom(_ window: NSWindow, toFrame newFrame: NSRect) -> Bool {
        toggleMaximize()
        return false
    }

    /// 用户拖动了窗口边缘：窗口按用户的大小，不再算最大化。
    func windowDidEndLiveResize(_ notification: Notification) {
        maximized = false
        expectedContentSize = nil
    }

    func windowDidExitFullScreen(_ notification: Notification) {
        followCurrentImage()
    }

    // MARK: 多个窗口

    func windowDidBecomeKey(_ notification: Notification) {
        onBecameActive?()
    }

    /// 别的窗口（生成信息面板、设置）到了前面：进度条只在这个窗口在前面时收到“移出”，悬停的样子收起来。
    func windowDidResignKey(_ notification: Notification) {
        overlays.resetHover()
    }

    /// 最小化、取回：阻止显示器睡眠只在窗口看得见时生效。
    func windowDidMiniaturize(_ notification: Notification) {
        overlays.resetHover()
        onDisplayChanged?()
    }

    func windowDidDeminiaturize(_ notification: Notification) {
        onDisplayChanged?()
    }

    /// 窗口现在看不看得见（开着、没有最小化、程序没有被隐藏）。
    var isOnScreen: Bool {
        guard let window else { return false }
        return window.isVisible && !window.isMiniaturized && !NSApp.isHidden
    }

    func windowWillClose(_ notification: Notification) {
        // 全屏时关窗口：先退出全屏。菜单栏、程序坞的自动隐藏是整个程序的，不恢复的话别的窗口也一直没有菜单栏；
        // 最后一个窗口关掉后留着，再显示时也不该还是盖满屏幕的样子。
        exitFullScreen()
        isClosed = true
        showWhenImageReady = false
        picker?.close()
        // 最后一个窗口关掉后留着，再显示时悬停效果不能还亮着（指针早走了，收不到“移出”）。
        overlays.resetHover()
        // “生成信息”面板显示的是这个窗口的图片：跟着关掉（不然一直显示这里的提示词，不跟别的窗口）。
        if let panel = GenerationPanelController.current, panel.source === self {
            panel.close()
        }
        onClosed?()
    }

    /// 这一代次的新图像到了：有翻页效果就开始（先复制旧画面，再换图）。
    private func startTurn(_ generation: UInt64) {
        guard let pending = pendingTurn else { return }
        pendingTurn = nil
        if pending.generation == generation {
            canvas.beginTransition(effect: pending.turn.effect, forward: pending.turn.forward,
                                   durationMs: pending.turn.durationMs)
        }
    }

    /// 翻页音效。自检时只记下来，不出声。
    private func playPageSound() {
        guard let sound = pageSound else {
            soundLog.append("unavailable")
            return
        }
        if isSelfTest {
            soundLog.append("NSSound（自检不出声）")
            return
        }
        sound.stop()
        sound.play()
        soundLog.append("NSSound")
    }

    /// 打开选择图片窗口；已经打开时放到最前面。
    func showPicker() {
        if let picker {
            picker.showWindow(nil)
            picker.window?.makeKeyAndOrderFront(nil)
            return
        }
        let picker = PickerWindowController(core: core.core, lang: lang)
        // 窗口关掉时还在它自己的方法里：等这次事件处理完再放掉它。
        picker.onClosed = { [weak self] in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.picker = nil
                }
            }
        }
        self.picker = picker
        picker.place(over: window)
        picker.showWindow(nil)
        picker.window?.makeKeyAndOrderFront(nil)
    }

    override func showWindow(_ sender: Any?) {
        isClosed = false
        super.showWindow(sender)
        onDisplayChanged?()
    }

    var appCore: AppCore {
        core.core
    }

    /// 窗口里有没有图片（从访达打开时优先用空着的窗口）。
    var hasImage: Bool {
        core.core.currentPath() != nil
    }

    /// 打开图片文件或文件夹。
    func open(_ url: URL) {
        core.open(path: url.path(percentEncoded: false))
    }

    /// 新开的窗口：等第一张图出来、窗口按图片定好大小再显示；读得慢时 0.3 秒后先显示。
    func showAfterFirstImage() {
        showWhenImageReady = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            MainActor.assumeIsolated {
                self?.showIfWaiting()
            }
        }
    }

    private func showIfWaiting() {
        guard showWhenImageReady else { return }
        showWhenImageReady = false
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    /// 新窗口的位置：中心在 anchor 窗口中心的右下方 24 点，之后按图片定大小时中心不动。
    func place(near anchor: MainWindowController) {
        guard let window, let other = anchor.window else { return }
        let size = window.frame.size
        var origin = NSPoint(x: other.frame.midX + 24 - size.width / 2, y: other.frame.midY - 24 - size.height / 2)
        if let visible = other.screen?.visibleFrame {
            origin.x = min(max(origin.x, visible.minX), visible.maxX - size.width)
            origin.y = min(max(origin.y, visible.minY), visible.maxY - size.height)
        }
        window.setFrameOrigin(origin)
    }

    /// 窗口位置记录用的名字是不是在这个窗口上（全屏时暂时拿下来的也算）。
    var ownsFrameName: Bool {
        window?.frameAutosaveName == Self.frameName || autosaveNameBeforeFullScreen == Self.frameName
    }

    /// 这个窗口要被放掉了：交出窗口位置记录的名字，停止监听。
    func discard() {
        if instantFullScreen {
            autosaveNameBeforeFullScreen = ""
        }
        if window?.frameAutosaveName == Self.frameName {
            _ = window?.setFrameAutosaveName("")
        }
        titlebar.detach()
    }

    /// 接过窗口位置记录：原来记着位置的窗口关掉了，改记这个窗口。
    func adoptFrameName() {
        if instantFullScreen {
            autosaveNameBeforeFullScreen = Self.frameName
        } else if let window, window.frameAutosaveName.isEmpty {
            _ = window.setFrameAutosaveName(Self.frameName)
        }
    }

    // MARK: 吸附边缘

    /// 拖动窗口时吸附屏幕边缘：给出窗口应在的原点；设置关掉时为空。屏幕取窗口中心所在的那块。
    func snappedOrigin(for frame: NSRect) -> NSPoint? {
        let center = NSPoint(x: frame.midX, y: frame.midY)
        let screen = NSScreen.screens.first { $0.frame.contains(center) } ?? window?.screen ?? NSScreen.main
        guard let area = screen?.visibleFrame,
              let point = core.core.snapWindow(
                  frame: WindowFrame(x: Double(frame.minX), y: Double(frame.minY),
                                     width: Double(frame.width), height: Double(frame.height)),
                  area: WindowFrame(x: Double(area.minX), y: Double(area.minY),
                                    width: Double(area.width), height: Double(area.height))
              )
        else { return nil }
        return NSPoint(x: point.x, y: point.y)
    }

    /// 用户开始拖动标题栏（系统负责移动窗口）。
    func windowWillMove(_ notification: Notification) {
        userMoving = true
    }

    /// 窗口移动了：用户拖标题栏的，等松开鼠标后吸附边缘。
    func windowDidMove(_ notification: Notification) {
        guard userMoving, !moveCheckPending else { return }
        moveCheckPending = true
        finishMoveWhenReleased()
    }

    private func finishMoveWhenReleased() {
        if NSEvent.pressedMouseButtons & 1 != 0 {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
                MainActor.assumeIsolated {
                    self?.finishMoveWhenReleased()
                }
            }
            return
        }
        moveCheckPending = false
        finishUserMove()
    }

    /// 拖完窗口：离屏幕边缘够近就贴上去（设置"拖动窗口时吸附边缘"）。自检也用它。
    func finishUserMove() {
        userMoving = false
        guard let window, windowMovable, let origin = snappedOrigin(for: window.frame),
              origin != window.frame.origin
        else { return }
        window.setFrameOrigin(origin)
    }

    /// 直接启动（没有图片）时的窗口：固定 900x600 点（屏幕放不下时缩小），在系统选的屏幕
    /// （有菜单栏、正在用的那块，`NSScreen.main`）的可用区域里居中。之后照常记录位置。
    func placeAtLaunchDefault() {
        guard let window, let visible = (NSScreen.main ?? window.screen)?.visibleFrame else { return }
        let size = NSSize(width: min(Self.launchSize.width, visible.width),
                          height: min(Self.launchSize.height, visible.height))
        window.setFrame(
            NSRect(x: visible.midX - size.width / 2, y: visible.midY - size.height / 2,
                   width: size.width, height: size.height),
            display: false
        )
        _ = window.setFrameAutosaveName(Self.frameName)
    }

    /// 直接启动时的默认窗口大小。
    static let launchSize = NSSize(width: 900, height: 600)

    /// 记住窗口位置与大小：用系统记录的窗口位置；第一次启动约占屏幕可用区域的 70%，居中。
    /// 自检时不调用（每次都是 900x600 点，结果可以比较）。
    func restoreFrame() {
        guard let window else { return }
        if !window.setFrameUsingName(Self.frameName), let visible = (window.screen ?? NSScreen.main)?.visibleFrame {
            let size = NSSize(width: max(visible.width * 0.7, 480), height: max(visible.height * 0.7, 360))
            window.setFrame(
                NSRect(x: visible.midX - size.width / 2, y: visible.midY - size.height / 2,
                       width: size.width, height: size.height),
                display: false
            )
        }
        _ = window.setFrameAutosaveName(Self.frameName)
    }

    private func updateTitle() {
        refreshFilenameLabel()
        if currentName.isEmpty {
            window?.title = text(.appName)
        } else if let index = currentIndex, let total = currentTotal {
            window?.title = "\(currentName) [\(index + 1)/\(total)]"
        } else {
            window?.title = currentName
        }
    }

    /// 正在显示的通知文字（自检记录用）。
    var noticeText: String? {
        noticeLabel.isHidden ? nil : noticeLabel.stringValue.trimmingCharacters(in: .whitespaces)
    }

    func showNotice(_ message: String) {
        noticeSerial += 1
        let serial = noticeSerial
        noticeLabel.stringValue = "  \(message)  "
        noticeLabel.isHidden = false
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in
            MainActor.assumeIsolated {
                if let self, self.noticeSerial == serial {
                    self.noticeLabel.isHidden = true
                }
            }
        }
    }

    /// 执行动作（菜单、按键、手势、自检都走这里）。旋转后窗口跟着图片转。
    func invoke(_ id: String) {
        apply(core.core.invokeAction(id: id))
        followAfterAction(id)
    }

    /// 把动作的结果落到界面上。
    func apply(_ outcome: ActionOutcome) {
        switch outcome {
        case .unavailable, .started:
            break
        case let .viewChanged(showZoom):
            canvas.refreshLayout()
            if showZoom {
                showZoomHint()
            }
        case .overlayChanged:
            refreshOverlays()
        case let .ui(request):
            onUiRequest?(request)
        }
    }

    /// 到达首尾时询问：转到首张（末张）、转到下一个（上一个）文件夹（压缩包里是压缩包），或取消。
    private func askBoundary(forward: Bool, archive: Bool) {
        if onBoundary?(forward) == true {
            return
        }
        guard let window else { return }
        let prompt = boundaryPrompt(forward: forward, archive: archive, lang: lang)
        let alert = Self.alert(prompt)
        alert.beginSheetModal(for: window) { [weak self] response in
            MainActor.assumeIsolated {
                guard let self else { return }
                switch response {
                case .alertFirstButtonReturn: self.invoke(forward ? "first_image" : "last_image")
                case .alertSecondButtonReturn: self.invoke(forward ? "next_container" : "prev_container")
                default: break
                }
            }
        }
    }

    /// 图片被安全上限拦下了：问用户要不要仍然打开。
    private func askLimit(path: String) {
        if onLimit?() == true {
            return
        }
        guard let window else { return }
        // 默认按钮（↩，也管 ⎋）是"取消"：先加的按钮是默认按钮，所以"取消"在前、"仍然打开"在后。
        let prompt = limitPrompt(lang: lang)
        let alert = NSAlert()
        alert.messageText = prompt.message
        alert.addButton(withTitle: prompt.buttons.last ?? "")
        let open = alert.addButton(withTitle: prompt.buttons.first ?? "")
        open.keyEquivalent = ""
        alert.beginSheetModal(for: window) { [weak self] response in
            MainActor.assumeIsolated {
                guard let self else { return }
                if response == .alertSecondButtonReturn {
                    self.core.core.openWithoutLimits(path: path)
                } else {
                    self.core.core.keepLimits(path: path)
                }
            }
        }
    }

    /// 对话框：第一个按钮是默认按钮（↩），最后一个是取消（⎋）。
    private static func alert(_ prompt: Prompt) -> NSAlert {
        let alert = NSAlert()
        alert.messageText = prompt.message
        for title in prompt.buttons {
            alert.addButton(withTitle: title)
        }
        alert.buttons.last?.keyEquivalent = String(UnicodeScalar(UInt8(27)))
        return alert
    }

    /// 移到废纸篓：先确认（设置"删除文件时确认"），成功后告诉核心，由核心显示下一张。
    func trash(path: String, name: String, confirm: Bool) {
        let url = URL(fileURLWithPath: path)
        let perform = { [weak self] in
            guard let self else { return }
            do {
                var resulting: NSURL?
                try FileManager.default.trashItem(at: url, resultingItemURL: &resulting)
                if self.isSelfTest, let trashed = resulting as URL? {
                    // 自检时不在废纸篓里留东西。
                    try? FileManager.default.removeItem(at: trashed)
                }
                self.core.core.dispatch(command: .removed(path: path))
            } catch {
                self.showNotice(self.text(.noticeTrashFailed).replacingOccurrences(of: "{message}", with: error.localizedDescription))
            }
        }
        if isSelfTest || !confirm {
            perform()
            return
        }
        guard let window else { return }
        let alert = Self.alert(trashPrompt(platform: .macos, lang: lang, name: name))
        alert.beginSheetModal(for: window) { response in
            MainActor.assumeIsolated {
                if response == .alertFirstButtonReturn {
                    perform()
                }
            }
        }
    }

    /// 用图像编辑器打开：用设置里的程序打开这个文件；没有设置时提示去设置。
    func openInEditor(path: String, editor: String) {
        if editor.isEmpty {
            showNotice(text(.noticeEditorMissing))
            onExternal?("editor", "")
            return
        }
        if isSelfTest {
            onExternal?("editor", "\(editor) \(path)")
            return
        }
        let configuration = NSWorkspace.OpenConfiguration()
        NSWorkspace.shared.open(
            [URL(fileURLWithPath: path)],
            withApplicationAt: URL(fileURLWithPath: editor),
            configuration: configuration
        ) { [weak self] _, error in
            guard let error else { return }
            let message = error.localizedDescription
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { return }
                    self.showNotice(self.text(.noticeEditorFailed).replacingOccurrences(of: "{message}", with: message))
                }
            }
        }
    }

    /// 设为桌面图片：设到窗口所在的屏幕。
    func setWallpaper(path: String) {
        if isSelfTest {
            onExternal?("wallpaper", path)
            return
        }
        guard let screen = window?.screen ?? NSScreen.main else { return }
        do {
            try NSWorkspace.shared.setDesktopImageURL(URL(fileURLWithPath: path), for: screen, options: [:])
            showNotice(text(.noticeWallpaperSet))
        } catch {
            showNotice(text(.noticeFileFailed).replacingOccurrences(of: "{message}", with: error.localizedDescription))
        }
    }

    /// 打印：原图（按 EXIF 方向，不含反相等效果）按纸张等比放到最大、居中，用系统的打印对话框。
    func printImage(name: String) {
        if isSelfTest {
            onExternal?("print", name)
            return
        }
        guard let window, let handle = core.core.originalImage() ?? canvas.currentHandle,
              let cgImage = ImageCanvasView.makeCGImage(handle)
        else { return }
        let size = NSSize(width: cgImage.width, height: cgImage.height)
        let view = NSImageView(frame: NSRect(origin: .zero, size: size))
        view.image = NSImage(cgImage: cgImage, size: size)
        view.imageScaling = .scaleProportionallyUpOrDown
        let info = (NSPrintInfo.shared.copy() as? NSPrintInfo) ?? NSPrintInfo()
        info.horizontalPagination = .fit
        info.verticalPagination = .fit
        info.isHorizontallyCentered = true
        info.isVerticallyCentered = true
        info.orientation = cgImage.width > cgImage.height ? .landscape : .portrait
        let operation = NSPrintOperation(view: view, printInfo: info)
        operation.jobTitle = name
        operation.runModal(for: window, delegate: nil, didRun: nil, contextInfo: nil)
    }

    /// 复制到剪贴板：原图（按 EXIF 方向，不含反相、Gamma 等效果；超大图也是原图，不是代理纹理）加文件；
    /// 粘贴到访达得到文件，粘贴到编辑器得到图像。原图还没解码好时不复制。
    func copyImage(path: String) {
        guard let handle = core.core.originalImage(), let cgImage = ImageCanvasView.makeCGImage(handle) else { return }
        // 经 NSBitmapImageRep 放进剪贴板：生成的 TIFF 带着图像的色彩空间（sRGB 或图片自己的 ICC），粘贴到别的程序颜色不变。
        let rep = NSBitmapImageRep(cgImage: cgImage)
        let image = NSImage(size: NSSize(width: cgImage.width, height: cgImage.height))
        image.addRepresentation(rep)
        let pasteboard = isSelfTest ? NSPasteboard(name: NSPasteboard.Name("local.anyview.selftest")) : NSPasteboard.general
        pasteboard.clearContents()
        let items: [NSPasteboardWriting] = [URL(fileURLWithPath: path) as NSURL, image]
        pasteboard.writeObjects(items)
        lastCopyTypes = (pasteboard.pasteboardItems ?? []).flatMap { $0.types.map(\.rawValue) }
        if isSelfTest {
            // 自检核对剪贴板里的图像是原图：读回 TIFF，按它自己带的色彩空间把正中那个像素画到 1x1 的 sRGB 画布上
            // （就是别的程序粘贴后看到的颜色）。不用 colorAt：它按通用 RGB 解释数值， Mac 实测偏亮约 15。
            lastCopyCenter = nil
            lastCopyColorSpace = nil
            if let data = pasteboard.data(forType: .tiff), let rep = NSBitmapImageRep(data: data), let pasted = rep.cgImage,
               let srgb = CGColorSpace(name: CGColorSpace.sRGB) {
                lastCopyColorSpace = pasted.colorSpace?.name.map { $0 as String } ?? rep.colorSpace.localizedName
                var pixel = [UInt8](repeating: 0, count: 4)
                pixel.withUnsafeMutableBytes { buffer in
                    guard let context = CGContext(
                        data: buffer.baseAddress, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4,
                        space: srgb, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
                    ) else { return }
                    context.interpolationQuality = .none
                    let (w, h) = (CGFloat(pasted.width), CGFloat(pasted.height))
                    // 图像正中（从上往下第 height/2 行）那个像素落到画布的 (0, 0)；画布原点在左下角。
                    context.draw(pasted, in: CGRect(x: -CGFloat(pasted.width / 2),
                                                    y: -CGFloat(pasted.height - 1 - pasted.height / 2),
                                                    width: w, height: h))
                }
                lastCopyCenter = [Int(pixel[0]), Int(pixel[1]), Int(pixel[2])]
            }
            pasteboard.releaseGlobally()
        }
        showNotice(text(.noticeCopied))
    }

    /// 右键菜单，由核心的数据生成；快捷键按键盘映射显示实际生效的按键（ACTIONS 规则 7）。
    func contextMenu() -> NSMenu {
        MenuBuilder.build(core.core.menuEntries(menu: .context, lang: lang), actions: contextActions, mainMenu: false)
    }

    /// 从键盘映射打开右键菜单（动作 context_menu）：在画面中央弹出。
    func showContextMenu() {
        let center = NSPoint(x: canvas.bounds.midX, y: canvas.bounds.midY)
        _ = contextMenu().popUp(positioning: nil, at: center, in: canvas)
    }

    /// 缩放倍率提示：在画面中央显示约 1 秒；设置"不显示缩放信息控制"时不显示。
    private func showZoomHint() {
        guard !uiSettings.hideZoomInfo, let layout = core.core.viewLayout() else { return }
        zoomSerial += 1
        let serial = zoomSerial
        zoomLabel.stringValue = "  \(zoomText(scale: layout.zoom))  "
        zoomLabel.isHidden = false
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            MainActor.assumeIsolated {
                if let self, self.zoomSerial == serial {
                    self.zoomLabel.isHidden = true
                }
            }
        }
    }

    /// 按键交给核心按 macOS 键位解析（docs/ACTIONS.md）；没有绑定的按键返回 false，按系统惯例处理。
    private func handleKey(_ event: NSEvent) -> Bool {
        guard let key = KeyNames.name(of: event),
              let id = core.core.actionForKey(key: key, modifiers: KeyNames.modifiers(of: event))
        else {
            return false
        }
        invoke(id)
        return true
    }
}

/// 空态视图：深灰背景加居中提示。不接收鼠标，拖放和键盘交给下面的画布。
final class EmptyStateView: NSView {
    /// 默认背景色 #202020，可在设置中修改。
    static let defaultBackground = NSColor(srgbRed: 32.0 / 255.0, green: 32.0 / 255.0, blue: 32.0 / 255.0, alpha: 1)

    var background = EmptyStateView.defaultBackground {
        didSet { needsDisplay = true }
    }

    private let hintLabel: NSTextField
    private let statusLabel: NSTextField

    init(hint: String, status: String) {
        hintLabel = NSTextField(labelWithString: hint)
        statusLabel = NSTextField(labelWithString: status)
        super.init(frame: NSRect(x: 0, y: 0, width: 900, height: 600))

        hintLabel.font = .systemFont(ofSize: 15)
        hintLabel.textColor = NSColor(white: 0.62, alpha: 1)
        statusLabel.font = .systemFont(ofSize: 12)
        statusLabel.textColor = NSColor(white: 0.43, alpha: 1)

        let stack = NSStackView(views: [hintLabel, statusLabel])
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    /// 有背景图片时背景是透明的，露出下面画布上的背景图片。
    override var isOpaque: Bool { background.alphaComponent >= 1 }

    override func draw(_ dirtyRect: NSRect) {
        guard background.alphaComponent > 0 else { return }
        background.setFill()
        dirtyRect.fill()
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }

    var status: String {
        get { statusLabel.stringValue }
        set { statusLabel.stringValue = newValue }
    }

    var hint: String {
        get { hintLabel.stringValue }
        set { hintLabel.stringValue = newValue }
    }
}

/// 主窗口：全屏走程序自己的即时全屏；即时全屏时窗口可以盖住菜单栏的位置。
/// 去掉标题栏（全屏时）后仍然要能接收键盘（无标题栏的窗口默认不能成为主窗口、关键窗口）。
final class AnyviewWindow: NSWindow {
    var onToggleFullScreen: (() -> Void)?
    /// 即时全屏中：窗口盖满整个屏幕，不按"标题栏留在菜单栏下面"的规则挪动。
    var coversScreen = false

    override func toggleFullScreen(_ sender: Any?) {
        if let onToggleFullScreen {
            onToggleFullScreen()
        } else {
            super.toggleFullScreen(sender)
        }
    }

    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect {
        coversScreen ? frameRect : super.constrainFrameRect(frameRect, to: screen)
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
}
