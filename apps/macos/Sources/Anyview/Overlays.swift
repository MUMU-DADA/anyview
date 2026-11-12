import AppKit
import AnyviewCore

// 悬浮界面（docs/MENUS.md）：底部控制栏、左右切图箭头、左上角图片信息。
// 显示规则（指针区域、时间）由核心给出，这里只负责 AppKit 控件和动画。
// 外观用系统的材质和 SF Symbols 图标，采用平台原生风格：macOS 26 起浮在图片上的控制栏、
// 动画信息控制、悬停提示和图片信息的两个按钮是液态玻璃，更早的系统是 HUD 深色磨砂。

/// 液态玻璃。系统的“减少透明度”“增强对比度”由系统自己处理；没有 macOS 26 的系统、
/// 用旧版开发工具（没有 macOS 26 的 SDK）编译的程序用 HUD 深色磨砂。测试时 ANYVIEW_GLASS=0 强制用旧样式。
enum OverlayGlass {
    static let enabled: Bool = {
        guard ProcessInfo.processInfo.environment["ANYVIEW_GLASS"] != "0" else { return false }
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            return true
        }
        #endif
        return false
    }()
}

/// 浮在图片上的底板，content 铺满它：液态玻璃（跟着下面的图片变亮变暗），或旧样式的 HUD 深色磨砂
/// （固定深色外观，里面的颜色按深色取）。里面的文字、图标用系统的语义色（labelColor 等），两种底板上都看得清。
/// interactive：里面有按钮（macOS 27 起点按时玻璃有反应）。
@MainActor
func overlayBackdrop(_ content: NSView, cornerRadius: CGFloat, interactive: Bool) -> NSView {
    content.translatesAutoresizingMaskIntoConstraints = false
    var backdrop: NSView?
    #if compiler(>=6.2)
    if #available(macOS 26.0, *), OverlayGlass.enabled {
        let glass = NSGlassEffectView()
        glass.cornerRadius = cornerRadius
        glass.contentView = content
        // effectIsInteractive 是 macOS 27 加的：按名字设，macOS 26 的 SDK 也编得过。
        if interactive, glass.responds(to: NSSelectorFromString("setEffectIsInteractive:")) {
            glass.setValue(true, forKey: "effectIsInteractive")
        }
        backdrop = glass
    }
    #endif
    let view = backdrop ?? {
        let hud = NSVisualEffectView()
        hud.material = .hudWindow
        hud.blendingMode = .withinWindow
        hud.state = .active
        hud.appearance = NSAppearance(named: .darkAqua)
        hud.wantsLayer = true
        hud.layer?.cornerRadius = cornerRadius
        hud.layer?.masksToBounds = true
        hud.addSubview(content)
        return hud
    }()
    view.translatesAutoresizingMaskIntoConstraints = false
    NSLayoutConstraint.activate([
        content.leadingAnchor.constraint(equalTo: view.leadingAnchor),
        content.trailingAnchor.constraint(equalTo: view.trailingAnchor),
        content.topAnchor.constraint(equalTo: view.topAnchor),
        content.bottomAnchor.constraint(equalTo: view.bottomAnchor),
    ])
    return view
}

/// 单独浮在图片上的按钮做成圆形的玻璃按钮。旧样式时不改，返回假。
@MainActor
func makeGlassButton(_ button: NSButton) -> Bool {
    #if compiler(>=6.2)
    if #available(macOS 26.0, *), OverlayGlass.enabled {
        button.layer?.borderWidth = 0
        button.isBordered = true
        button.bezelStyle = .glass
        button.borderShape = .circle
        button.contentTintColor = nil
        return true
    }
    #endif
    return false
}

/// 淡入淡出。隐藏完成后设为 isHidden，不再接收鼠标。
@MainActor
final class Fader {
    private let view: NSView
    private var serial = 0
    private(set) var isShown = false
    private let timing = overlayTiming()

    init(_ view: NSView) {
        self.view = view
        view.alphaValue = 0
        view.isHidden = true
    }

    func show() {
        guard !isShown else { return }
        isShown = true
        serial += 1
        view.isHidden = false
        let duration = Double(timing.fadeInMs) / 1000
        NSAnimationContext.runAnimationGroup { context in
            context.duration = duration
            view.animator().alphaValue = 1
        }
    }

    /// 马上隐藏，不做动画。
    func hideNow() {
        isShown = false
        serial += 1
        view.alphaValue = 0
        view.isHidden = true
    }

    func hide() {
        guard isShown else { return }
        isShown = false
        serial += 1
        let current = serial
        let duration = Double(timing.fadeOutMs) / 1000
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = duration
            view.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.serial == current else { return }
                self.view.isHidden = true
            }
        })
    }
}

/// 标题栏下面的深色渐变：标题文字铺在图片上时也看得清。不接收鼠标。
final class TitlebarScrim: NSView {
    private let gradient = CAGradientLayer()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        gradient.colors = [NSColor(white: 0, alpha: 0.5).cgColor, NSColor(white: 0, alpha: 0).cgColor]
        // 图层坐标 y 向上：上端深、下端透明。
        gradient.startPoint = CGPoint(x: 0.5, y: 1)
        gradient.endPoint = CGPoint(x: 0.5, y: 0)
        layer?.addSublayer(gradient)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        gradient.frame = bounds
        CATransaction.commit()
    }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// 标题栏：透明、铺在图片上面，与控制栏一起淡入淡出；指针移到窗口顶部时也显示。
/// 没有图片时一直显示；系统全屏时不管（全屏的标题栏由系统在指针移到屏幕顶端时显示）。
/// 隐藏后把标题栏视图设为 isHidden，看不见的关闭按钮不会被点到。
@MainActor
final class TitlebarControl {
    let scrim: TitlebarScrim
    private let scrimFade: Fader
    private weak var window: NSWindow?
    private var serial = 0
    /// 标题栏现在应当显示（淡出过程中已为 false）。
    private(set) var isShown = true
    /// 系统全屏中（标题栏交给系统）。
    private(set) var inFullScreen = false
    /// 渐变只在有图片时显示（空态的背景本来就是深灰）。
    private var hasImage = false
    /// 设置"显示标题栏"：标题栏一直显示、不透明，图片在它下面，不需要渐变。
    private(set) var alwaysShown = false
    private let timing = overlayTiming()
    private var observers: [NSObjectProtocol] = []
    /// 标题栏显示、隐藏或全屏状态变了（标题栏隐藏时显示文件名）。
    var onShownChanged: (() -> Void)?

    init() {
        let scrim = TitlebarScrim()
        self.scrim = scrim
        scrimFade = Fader(scrim)
    }

    /// 渐变放在内容视图顶部：高度为标题栏加 24 点。
    func install(in container: NSView) {
        scrim.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(scrim)
        NSLayoutConstraint.activate([
            scrim.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            scrim.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            scrim.topAnchor.constraint(equalTo: container.topAnchor),
            scrim.bottomAnchor.constraint(equalTo: container.safeAreaLayoutGuide.topAnchor, constant: 24),
        ])
    }

    /// 窗口要被放掉了：不再监听它的通知。
    func detach() {
        for observer in observers {
            NotificationCenter.default.removeObserver(observer)
        }
        observers = []
    }

    /// 设置"显示标题栏"改了。
    func setAlwaysShown(_ on: Bool) {
        alwaysShown = on
        if on {
            reveal(animated: false)
            scrimFade.hideNow()
        } else if isShown && hasImage && !inFullScreen {
            scrimFade.show()
        }
        onShownChanged?()
    }

    func attach(to window: NSWindow) {
        self.window = window
        let center = NotificationCenter.default
        observers.append(center.addObserver(
            forName: NSWindow.willEnterFullScreenNotification, object: window, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                // 进全屏前恢复标题栏，全屏时由系统显示和隐藏。
                self.reveal(animated: false)
                self.inFullScreen = true
                self.scrimFade.hide()
            }
        })
        observers.append(center.addObserver(
            forName: NSWindow.didExitFullScreenNotification, object: window, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.inFullScreen = false
            }
        })
    }

    /// 程序自己的即时全屏：全屏时窗口没有标题栏，渐变也不要；退出后系统新建的标题栏视图默认是显示的，
    /// 按现在的状态（显示或隐藏）重新摆好。
    func setFullScreen(_ on: Bool) {
        inFullScreen = on
        defer { onShownChanged?() }
        if on {
            scrimFade.hideNow()
            return
        }
        if let view = titlebarView {
            view.alphaValue = isShown ? 1 : 0
            view.isHidden = !isShown
        }
        if isShown && hasImage && !alwaysShown {
            scrimFade.show()
        } else {
            scrimFade.hideNow()
        }
    }

    /// 红绿灯和标题文字所在的视图（系统的标题栏视图）。
    private var titlebarView: NSView? {
        window?.standardWindowButton(.closeButton)?.superview
    }

    /// 标题栏的高度（点）。
    var height: CGFloat {
        guard let window else { return 28 }
        return max(window.frame.height - window.contentLayoutRect.height, 0)
    }

    func setHasImage(_ value: Bool) {
        hasImage = value
        if !value {
            scrimFade.hide()
        } else if isShown && !inFullScreen && !alwaysShown {
            scrimFade.show()
        }
    }

    /// 显示或隐藏（淡入淡出动画；animated 为假时马上隐藏）。全屏时不动。
    func set(shown: Bool, animated: Bool = true) {
        guard !inFullScreen, !alwaysShown else { return }
        if shown {
            reveal(animated: true)
        } else if animated {
            conceal()
        } else {
            concealNow()
        }
    }

    /// 马上隐藏（打开图片时：窗口一出现就是无边框的样子）。
    private func concealNow() {
        isShown = false
        serial += 1
        onShownChanged?()
        scrimFade.hideNow()
        guard let view = titlebarView else { return }
        view.alphaValue = 0
        view.isHidden = true
    }

    private func reveal(animated: Bool) {
        serial += 1
        let wasShown = isShown
        isShown = true
        if !wasShown {
            onShownChanged?()
        }
        if hasImage && !inFullScreen && !alwaysShown {
            scrimFade.show()
        }
        guard let view = titlebarView else { return }
        view.isHidden = false
        if !animated {
            view.alphaValue = 1
            return
        }
        guard !wasShown || view.alphaValue < 1 else { return }
        let duration = Double(timing.fadeInMs) / 1000
        NSAnimationContext.runAnimationGroup { context in
            context.duration = duration
            view.animator().alphaValue = 1
        }
    }

    private func conceal() {
        guard isShown else { return }
        isShown = false
        serial += 1
        onShownChanged?()
        let current = serial
        scrimFade.hide()
        guard let view = titlebarView else { return }
        let duration = Double(timing.fadeOutMs) / 1000
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = duration
            view.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.serial == current, !self.isShown, !self.inFullScreen else { return }
                self.titlebarView?.isHidden = true
            }
        })
    }

    /// 自检：渐变正显示时的区域（内容视图坐标）。
    var coveredRect: NSRect? {
        scrim.isHidden ? nil : scrim.frame
    }
}

/// 控制栏上的进度条：画轨道和滑块；悬停时报告指针处的"序号/总数"；单击或拖动，松开后才跳转。
final class ProgressStrip: NSView {
    var index: UInt32? {
        didSet { reportVisual() }
    }
    var total: UInt32? {
        didSet { reportVisual() }
    }
    /// 拖动中、尚未松开的目标序号。
    private(set) var pending: UInt32? {
        didSet { reportVisual() }
    }
    /// 液态玻璃的控制栏（macOS 26 起）上，轨道和滑块由控制栏的玻璃层画（玻璃滑块要盖在轨道上面）：
    /// 这里只管指针，把要画的告诉它；设了这个就不自己画。轨道是本视图坐标里的一条线（中线、宽度）。
    struct Visual: Equatable {
        var track: NSRect
        var fraction: Double?
        var hovering: Bool
        var dragging: Bool
        /// 拖得快时玻璃滑块被拉长多少点。
        var stretch: CGFloat
    }
    var onVisual: ((Visual) -> Void)? {
        didSet {
            needsDisplay = true
            reportVisual()
        }
    }
    private var stretch: CGFloat = 0
    private var stretchSerial = 0
    var onJump: ((UInt32) -> Void)?
    /// 悬停提示：文字和指针在本视图中的 x；文字为 nil 表示隐藏。
    var onHover: ((String?, CGFloat) -> Void)?
    var scanningText = ""
    /// 显示悬停提示（设置"不显示鼠标悬浮工具提示"时为假）。
    var showsTip = true
    private let margin: CGFloat = 8
    /// 指针在进度条上（滑块变大）。
    private var hovering = false
    /// 滑块和轨道变大的程度：0 平时，1 指针在进度条上，1.5 按住拖动。用动画过渡。
    @objc dynamic var knobGrowth: CGFloat = 0 {
        didSet { needsDisplay = true }
    }

    override class func defaultAnimation(forKey key: NSAnimatablePropertyKey) -> Any? {
        key == "knobGrowth" ? CABasicAnimation() : super.defaultAnimation(forKey: key)
    }

    private func grow(to value: CGFloat) {
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            animator().knobGrowth = value
        }
    }

    /// 收起悬停的样子（窗口不在前面、最小化时：这时收不到“移出”，不收的话回来时滑块还是大的）。拖动中不收。
    func resetHover() {
        guard hovering, pending == nil else { return }
        hovering = false
        grow(to: 0)
        reportVisual()
        onHover?(nil, 0)
    }

    /// 自检：像指针停在进度条上（或离开）。
    func hoverForTest(_ inside: Bool) {
        hovering = inside
        grow(to: inside ? 1 : 0)
        reportVisual()
    }

    /// 把要画的告诉玻璃层（没有玻璃层时什么也不做）。
    func reportVisual() {
        guard let onVisual else { return }
        let track = NSRect(x: margin, y: bounds.midY - 2, width: max(bounds.width - 2 * margin, 0), height: 4)
        onVisual(Visual(track: track, fraction: knobFraction, hovering: hovering, dragging: pending != nil,
                        stretch: stretch))
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        reportVisual()
    }

    override var isFlipped: Bool { true }
    override var intrinsicContentSize: NSSize { NSSize(width: 220, height: 16) }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas {
            removeTrackingArea(area)
        }
        addTrackingArea(NSTrackingArea(
            rect: .zero,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil
        ))
    }

    /// 滑块现在的位置（0–1），自测用。
    var knobFraction: Double? {
        guard let total, let index = pending ?? index else { return nil }
        return progressFraction(index: index, total: total)
    }

    // 颜色用语义色，画的时候按本视图的外观取：玻璃底板跟着图片变亮变暗时跟着变。
    override func draw(_ dirtyRect: NSRect) {
        guard onVisual == nil else { return }
        let y = bounds.midY
        let track = NSBezierPath()
        track.move(to: NSPoint(x: margin, y: y))
        track.line(to: NSPoint(x: bounds.width - margin, y: y))
        track.lineWidth = 4 + knobGrowth
        track.lineCapStyle = .round
        NSColor.labelColor.withAlphaComponent(0.25).setStroke()
        track.stroke()
        if let fraction = knobFraction {
            let x = margin + CGFloat(fraction) * (bounds.width - 2 * margin)
            let radius = 6 + 1.5 * knobGrowth
            NSColor.labelColor.setFill()
            NSBezierPath(ovalIn: NSRect(x: x - radius, y: y - radius, width: 2 * radius, height: 2 * radius)).fill()
        }
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }

    /// 事件在进度条上的位置：比例（0–1）和本视图中的 x。
    private func locate(_ event: NSEvent) -> (fraction: Double, x: CGFloat) {
        let point = convert(event.locationInWindow, from: nil)
        let usable = max(bounds.width - 2 * margin, 1)
        let fraction = min(max((point.x - margin) / usable, 0), 1)
        return (Double(fraction), point.x)
    }

    private func hoverText(_ fraction: Double) -> String {
        guard let total, total > 0 else { return scanningText }
        guard let index = pending ?? progressIndex(fraction: fraction, total: total) else { return scanningText }
        return positionText(index: index, total: total)
    }

    override func mouseMoved(with event: NSEvent) {
        let spot = locate(event)
        onHover?(showsTip ? hoverText(spot.fraction) : nil, spot.x)
    }

    override func mouseEntered(with event: NSEvent) {
        hovering = true
        grow(to: pending == nil ? 1 : 1.5)
        reportVisual()
    }

    override func mouseExited(with event: NSEvent) {
        hovering = false
        if pending == nil {
            grow(to: 0)
        }
        reportVisual()
        onHover?(nil, 0)
    }

    override func mouseDown(with event: NSEvent) {
        grow(to: 1.5)
        updatePending(event)
    }

    override func mouseDragged(with event: NSEvent) {
        // 拖得越快玻璃滑块拉得越长（每次移动的点数，最多 16 点；减弱动态效果时不拉长）。
        stretch = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : min(abs(event.deltaX) * 1.5, 16)
        updatePending(event)
        // 停住不动 80 毫秒就缩回去。
        stretchSerial += 1
        let serial = stretchSerial
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.stretchSerial == serial, self.stretch != 0 else { return }
                self.stretch = 0
                self.reportVisual()
            }
        }
    }

    override func mouseUp(with event: NSEvent) {
        stretch = 0
        if let target = pending {
            pending = nil
            onJump?(target)
        }
        grow(to: hovering ? 1 : 0)
        reportVisual()
        needsDisplay = true
    }

    private func updatePending(_ event: NSEvent) {
        guard let total else { return }
        let spot = locate(event)
        pending = progressIndex(fraction: spot.fraction, total: total)
        needsDisplay = true
        onHover?(showsTip ? hoverText(spot.fraction) : nil, spot.x)
    }

    /// 与松开鼠标时相同的跳转（自测用）。
    func jump(toFraction fraction: Double) -> Bool {
        guard let total, let target = progressIndex(fraction: fraction, total: total) else { return false }
        onJump?(target)
        return true
    }
}

/// 左键、右键分别执行不同动作的按钮（翻页按钮：左键 1 张，右键 10 张）。
/// 指针移进移出、按下松开时通知（悬停高亮）。
final class PageButton: NSButton {
    var onRightClick: (() -> Void)?
    /// 指针移进（真）、移出（假）。设了才跟踪指针。
    var onHover: ((Bool) -> Void)? {
        didSet { updateTrackingAreas() }
    }
    /// 按下（真）、松开（假）。
    var onPress: ((Bool) -> Void)?
    private var hoverArea: NSTrackingArea?

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverArea {
            removeTrackingArea(hoverArea)
        }
        hoverArea = nil
        guard onHover != nil else { return }
        let area = NSTrackingArea(
            rect: .zero,
            options: [.mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(area)
        hoverArea = area
    }

    override func mouseEntered(with event: NSEvent) {
        guard let hoverArea, event.trackingArea === hoverArea else {
            super.mouseEntered(with: event)
            return
        }
        onHover?(true)
    }

    override func mouseExited(with event: NSEvent) {
        guard let hoverArea, event.trackingArea === hoverArea else {
            super.mouseExited(with: event)
            return
        }
        onHover?(false)
    }

    override func mouseDown(with event: NSEvent) {
        onPress?(true)
        // 按钮自己跟踪到松开为止（松开在按钮里才执行动作）。
        super.mouseDown(with: event)
        onPress?(false)
    }

    override func rightMouseDown(with event: NSEvent) {
        if let onRightClick {
            onRightClick()
        } else {
            super.rightMouseDown(with: event)
        }
    }
}

/// 悬停高亮的那块底：颜色是 labelColor（按所在位置的外观取，玻璃变亮变暗时跟着变），不挡鼠标。
private final class PlatterView: NSView {
    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        layer?.backgroundColor = NSColor.labelColor.withAlphaComponent(0.2).cgColor
        layer?.cornerCurve = .continuous
    }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// 悬停、按下的反馈，照 macOS 26 工具栏按钮的做法：指针移到按钮上时，图标下面淡入一块圆形
/// （方的按钮）或胶囊形（小图标、文字按钮）的亮底；在相邻的按钮之间移动时这块底滑过去，不先消失再出现；
/// 按下时更亮。按钮不可用、看不见时没有。亮底用约束对准按钮的中心：控制栏重新排列时跟着按钮走。
@MainActor
final class HoverPlatter {
    private let platter = PlatterView()
    private weak var host: NSView?
    private weak var target: NSView?
    private var placement: [NSLayoutConstraint] = []
    private var pressed = false
    private var hideSerial = 0
    /// 悬停、按下时亮底的不透明度（底本身是 labelColor 的 20%）。
    private let hoverAlpha: CGFloat = 0.6
    private let pressAlpha: CGFloat = 1

    /// host：亮底放在它里面、所有按钮的下面（玻璃的内容视图）；按钮都在它里面。
    init(in host: NSView) {
        self.host = host
        platter.wantsLayer = true
        platter.alphaValue = 0
        platter.translatesAutoresizingMaskIntoConstraints = false
        host.addSubview(platter, positioned: .below, relativeTo: nil)
    }

    /// 让这个按钮有悬停、按下反馈。
    func attach(_ button: PageButton) {
        button.onHover = { [weak self, weak button] inside in
            guard let self, let button else { return }
            if inside {
                self.show(button)
            } else {
                self.leave(button)
            }
        }
        button.onPress = { [weak self] down in
            self?.press(down)
        }
    }

    /// 亮底的大小：方的按钮（不小于 20 点）是同样大的圆，别的是比按钮大一圈、至少 26×24 的胶囊。
    private func size(for view: NSView) -> NSSize {
        let size = view.bounds.size
        if abs(size.width - size.height) < 2, size.width >= 20 {
            return size
        }
        return NSSize(width: max(size.width + 12, 26), height: max(size.height + 6, 24))
    }

    /// 把亮底对准这个按钮（中心对齐、按上面的大小）。
    private func pin(to view: NSView, size: NSSize) {
        NSLayoutConstraint.deactivate(placement)
        placement = [
            platter.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            platter.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            platter.widthAnchor.constraint(equalToConstant: size.width),
            platter.heightAnchor.constraint(equalToConstant: size.height),
        ]
        NSLayoutConstraint.activate(placement)
    }

    func show(_ view: NSView) {
        guard let host, !view.isHiddenOrHasHiddenAncestor,
              (view as? NSControl)?.isEnabled ?? true else { return }
        hideSerial += 1
        let size = size(for: view)
        let radius = min(size.width, size.height) / 2
        // 系统“减弱动态效果”打开时不滑，直接出现在新按钮下面。
        let sliding = target != nil && platter.alphaValue > 0.01
            && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        target = view
        pin(to: view, size: size)
        if sliding {
            // 从上一个按钮滑过来：位置、大小、圆角一起变（缓出，像液体停住）。
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.22
                context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0.9, 0.25, 1)
                context.allowsImplicitAnimation = true
                host.layoutSubtreeIfNeeded()
                platter.layer?.cornerRadius = radius
                platter.animator().alphaValue = pressed ? pressAlpha : hoverAlpha
            }
        } else {
            host.layoutSubtreeIfNeeded()
            platter.layer?.cornerRadius = radius
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.12
                platter.animator().alphaValue = hoverAlpha
            }
        }
    }

    /// 指针离开按钮：稍等一下再淡出。移到旁边的按钮时先收到这里的“移出”、再收到那边的“移进”，
    /// 亮底直接滑过去。
    private func leave(_ view: NSView) {
        guard target === view else { return }
        hideSerial += 1
        let serial = hideSerial
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.hideSerial == serial else { return }
                self.target = nil
                NSAnimationContext.runAnimationGroup { context in
                    context.duration = 0.18
                    self.platter.animator().alphaValue = 0
                }
            }
        }
    }

    private func press(_ down: Bool) {
        pressed = down
        guard target != nil else { return }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = down ? 0.06 : 0.2
            platter.animator().alphaValue = down ? pressAlpha : hoverAlpha
        }
    }

    /// 马上收起（底板隐藏时）。
    func reset() {
        hideSerial += 1
        target = nil
        pressed = false
        platter.alphaValue = 0
    }
}

/// 继承 NSObject：按钮的点击用 target-action 送到这里。
@MainActor
final class OverlayViews: NSObject {
    /// 控制栏的底板：液态玻璃或 HUD 深色磨砂（overlayBackdrop）。
    let bar: NSView
    let barFade: Fader
    let strip: ProgressStrip
    let left: PageButton
    let right: PageButton
    let leftFade: Fader
    let rightFade: Fader
    let info: NSStackView
    let infoLabel: NSTextField
    /// 进度条的悬停提示：底板（玻璃或磨砂）和里面的文字。
    let tip: NSView
    let tipLabel: NSTextField
    private let lockButton: PageButton
    private let openButton: PageButton
    /// 悬停高亮：控制栏里共用一块（没有液态玻璃的系统），动画信息控制里一块。
    private let barPlatter: HoverPlatter
    /// macOS 26 起控制栏的 SwiftUI 玻璃层（BarGlass）；更早的系统没有。存成 AnyObject：存储属性不能按系统版本声明。
    private var barGlassStorage: AnyObject?
    /// 锁定控制栏（“锁定”的小圆点是强调色；玻璃层画这个按钮的图标时要用）。
    private var locked = false
    private let animPlatter: HoverPlatter
    /// 有悬停高亮的按钮（自检按动作找）。
    private var hoverButtons: [PageButton] = []
    private let prev: PageButton
    private let next: PageButton
    private let infoClose: PageButton
    private let infoCopy: PageButton
    private var buttons: [(NSButton, String)] = []
    private var zone: HoverZone = .none
    private var pointerOnBar = false
    private var barHideSerial = 0
    private(set) var barHidePending = false
    private var arrowSerial = 0
    private let timing = overlayTiming()
    /// 执行动作。
    var onAction: ((String) -> Void)?
    private var infoText = ""
    /// 标题栏：与控制栏一起显示和隐藏；指针在窗口顶部时也显示。
    weak var titlebar: TitlebarControl?
    private var pointerNearTop = false
    private(set) var hasImage = false
    private var titleHideSerial = 0
    private(set) var titleHidePending = false
    /// 设置"不显示控制栏""不显示侧边箭头"：完全不显示，不同于自动隐藏。
    private(set) var barHidden = false
    private var arrowsHidden = false
    /// 图片信息的字号（设置"用小字体显示 EXIF/文件信息"）。
    private var infoFontSize: CGFloat = 15
    /// 动画信息控制：左下角的播放、暂停按钮和"第几帧 / 共几帧"。
    let anim: NSView
    private let animButton: PageButton
    let animLabel: NSTextField
    private var animBottom: NSLayoutConstraint?
    /// 设置"不显示动画信息控制"。
    private var animHidden = false

    /// 建立控件。文字和悬停提示由 applySettings 按语言填上。
    override init() {
        func symbol(_ name: String, size: CGFloat, weight: NSFont.Weight = .regular) -> NSImage? {
            NSImage(systemSymbolName: name, accessibilityDescription: nil)?
                .withSymbolConfiguration(NSImage.SymbolConfiguration(pointSize: size, weight: weight))
        }
        func button(_ name: String, size: CGFloat, round: CGFloat? = nil) -> PageButton {
            let button = PageButton(frame: .zero)
            button.image = symbol(name, size: size, weight: .medium)
            button.isBordered = false
            button.imagePosition = .imageOnly
            button.contentTintColor = .labelColor
            button.translatesAutoresizingMaskIntoConstraints = false
            if let round {
                // 大按钮：旧样式是圆形细边框，保持一致布局；玻璃上的按钮不再加框。
                button.wantsLayer = true
                button.layer?.cornerRadius = round / 2
                if !OverlayGlass.enabled {
                    button.layer?.borderWidth = 1
                    button.layer?.borderColor = NSColor(white: 1, alpha: 0.28).cgColor
                }
                NSLayoutConstraint.activate([
                    button.widthAnchor.constraint(equalToConstant: round),
                    button.heightAnchor.constraint(equalToConstant: round),
                ])
            }
            return button
        }
        func separator() -> NSView {
            let line = NSBox()
            line.boxType = .custom
            line.borderWidth = 0
            line.fillColor = .tertiaryLabelColor
            line.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                line.widthAnchor.constraint(equalToConstant: 1),
                line.heightAnchor.constraint(equalToConstant: 14),
            ])
            return line
        }
        func arrow(_ name: String) -> PageButton {
            let button = PageButton(frame: .zero)
            button.image = symbol(name, size: 34, weight: .semibold)
            button.isBordered = false
            button.imagePosition = .imageOnly
            button.contentTintColor = NSColor(white: 0.8, alpha: 0.9)
            button.translatesAutoresizingMaskIntoConstraints = false
            return button
        }

        // 控制栏：左边两个大按钮，中间进度条和一排小按钮，右边两个大按钮（MENUS"底部控制栏"）。
        let openButton = button("eject", size: 15, round: 40)
        let select = button("list.bullet", size: 15, round: 40)
        let zoomIn = button("plus", size: 12)
        let zoomOut = button("minus", size: 12)
        let rotateCCW = button("arrow.counterclockwise", size: 12)
        let rotateCW = button("arrow.clockwise", size: 12)
        let tools: [(PageButton, String)] = [
            (zoomIn, "zoom_in"), (zoomOut, "zoom_out"), (rotateCCW, "rotate_ccw"), (rotateCW, "rotate_cw"),
        ]
        let lockButton = PageButton(title: "", target: nil, action: nil)
        lockButton.isBordered = false
        lockButton.font = .systemFont(ofSize: 11)
        lockButton.contentTintColor = .labelColor
        lockButton.image = symbol("circle.fill", size: 8)
        lockButton.imagePosition = .imageTrailing
        let prev = button("chevron.left", size: 15, round: 40)
        let next = button("chevron.right", size: 15, round: 40)
        let strip = ProgressStrip()
        strip.translatesAutoresizingMaskIntoConstraints = false

        let toolRow = NSStackView(views: [zoomIn, zoomOut, separator(), rotateCCW, rotateCW, separator(), lockButton])
        toolRow.orientation = .horizontal
        toolRow.spacing = 8
        let middle = NSStackView(views: [strip, toolRow])
        middle.orientation = .vertical
        middle.alignment = .centerX
        middle.spacing = 4
        NSLayoutConstraint.activate([strip.widthAnchor.constraint(equalToConstant: 220)])
        let row = NSStackView(views: [openButton, select, middle, prev, next])
        row.orientation = .horizontal
        row.spacing = 8
        row.edgeInsets = NSEdgeInsets(top: 8, left: 12, bottom: 8, right: 12)
        // 胶囊形的底板（高 56）。macOS 26 起是 SwiftUI 的液态玻璃层（BarGlass）铺在按钮下面，悬停效果、
        // 进度条也由它画；更早的系统是 HUD 深色磨砂，按钮下面共用一块悬停高亮。
        let barPlatter = HoverPlatter(in: row)
        var barGlassStorage: AnyObject?
        var bar: NSView?
        #if compiler(>=6.2)
        if #available(macOS 26.0, *), OverlayGlass.enabled {
            let glass = BarGlass()
            let container = GlassBarContainer()
            container.translatesAutoresizingMaskIntoConstraints = false
            row.translatesAutoresizingMaskIntoConstraints = false
            container.addSubview(glass.view)
            container.addSubview(row)
            NSLayoutConstraint.activate([
                glass.view.leadingAnchor.constraint(equalTo: container.leadingAnchor),
                glass.view.trailingAnchor.constraint(equalTo: container.trailingAnchor),
                glass.view.topAnchor.constraint(equalTo: container.topAnchor),
                glass.view.bottomAnchor.constraint(equalTo: container.bottomAnchor),
                row.leadingAnchor.constraint(equalTo: container.leadingAnchor),
                row.trailingAnchor.constraint(equalTo: container.trailingAnchor),
                row.topAnchor.constraint(equalTo: container.topAnchor),
                row.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            ])
            bar = container
            barGlassStorage = glass
        }
        #endif
        if bar == nil {
            bar = overlayBackdrop(row, cornerRadius: 28, interactive: true)
        }

        // 左右切图箭头：浅灰细线 ‹ ›，贴近窗口边缘、垂直居中，没有底板（MENUS"边缘切图箭头"）。
        let left = arrow("chevron.left")
        let right = arrow("chevron.right")

        // 左上角图片信息：关闭、复制两个按钮，下面是白字深色描边的文字。
        // 液态玻璃时是两个圆形的玻璃按钮；旧样式是白色粗圆框。
        let infoClose = button("xmark", size: 12, round: 28)
        let infoCopy = button("doc.on.doc", size: 12, round: 28)
        for item in [infoClose, infoCopy] where !makeGlassButton(item) {
            item.layer?.borderWidth = 2
            item.layer?.borderColor = NSColor(white: 1, alpha: 0.9).cgColor
            item.contentTintColor = .white
        }
        let infoButtons = NSStackView(views: [infoClose, infoCopy])
        infoButtons.orientation = .horizontal
        infoButtons.spacing = 8
        let infoLabel = NSTextField(wrappingLabelWithString: "")
        infoLabel.preferredMaxLayoutWidth = 440
        infoLabel.maximumNumberOfLines = 0
        infoLabel.isSelectable = false
        let info = NSStackView(views: [infoButtons, infoLabel])
        info.orientation = .vertical
        info.alignment = .leading
        info.spacing = 10
        info.translatesAutoresizingMaskIntoConstraints = false
        info.isHidden = true

        // 动画信息控制：胶囊形底板（液态玻璃或深色磨砂）上一个播放、暂停按钮和帧序号。
        let animButton = button("pause.fill", size: 12)
        let animLabel = NSTextField(labelWithString: "")
        animLabel.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
        animLabel.textColor = .labelColor
        let animRow = NSStackView(views: [animButton, animLabel])
        animRow.orientation = .horizontal
        animRow.spacing = 6
        animRow.translatesAutoresizingMaskIntoConstraints = false
        let animContent = NSView()
        animContent.addSubview(animRow)
        NSLayoutConstraint.activate([
            animButton.widthAnchor.constraint(equalToConstant: 24),
            animButton.heightAnchor.constraint(equalToConstant: 24),
            animRow.leadingAnchor.constraint(equalTo: animContent.leadingAnchor, constant: 4),
            animRow.trailingAnchor.constraint(equalTo: animContent.trailingAnchor, constant: -12),
            animRow.topAnchor.constraint(equalTo: animContent.topAnchor, constant: 2),
            animRow.bottomAnchor.constraint(equalTo: animContent.bottomAnchor, constant: -2),
        ])
        let animPlatter = HoverPlatter(in: animContent)
        let anim = overlayBackdrop(animContent, cornerRadius: 14, interactive: true)
        anim.isHidden = true

        // 进度条的悬停提示（"序号/总数"），显示在控制栏上方：胶囊形底板上的一行字。按文字的大小放（showTip）。
        let tipLabel = NSTextField(labelWithString: "")
        tipLabel.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
        tipLabel.textColor = .labelColor
        tipLabel.translatesAutoresizingMaskIntoConstraints = false
        let tipContent = NSView()
        tipContent.addSubview(tipLabel)
        NSLayoutConstraint.activate([
            tipLabel.leadingAnchor.constraint(equalTo: tipContent.leadingAnchor, constant: 8),
            tipLabel.trailingAnchor.constraint(equalTo: tipContent.trailingAnchor, constant: -8),
            tipLabel.topAnchor.constraint(equalTo: tipContent.topAnchor, constant: 3),
            tipLabel.bottomAnchor.constraint(equalTo: tipContent.bottomAnchor, constant: -3),
        ])
        let tip = overlayBackdrop(tipContent, cornerRadius: 11, interactive: false)
        tip.translatesAutoresizingMaskIntoConstraints = true
        tip.isHidden = true

        self.bar = bar!
        barFade = Fader(bar!)
        self.barGlassStorage = barGlassStorage
        self.strip = strip
        self.left = left
        self.right = right
        leftFade = Fader(left)
        rightFade = Fader(right)
        self.info = info
        self.infoLabel = infoLabel
        self.tip = tip
        self.tipLabel = tipLabel
        self.lockButton = lockButton
        self.openButton = openButton
        self.barPlatter = barPlatter
        self.animPlatter = animPlatter
        self.prev = prev
        self.next = next
        self.infoClose = infoClose
        self.infoCopy = infoCopy
        self.anim = anim
        self.animButton = animButton
        self.animLabel = animLabel
        super.init()

        // 以下用到 self：按钮的目标都是本对象，点击时按标识执行对应动作。
        var enabledByCore: [(NSButton, String)] = [(select as NSButton, "select_image")]
        for (item, id) in tools {
            enabledByCore.append((item as NSButton, id))
        }
        buttons = enabledByCore
        var clickable: [(NSButton, String)] = enabledByCore
        clickable.append((openButton as NSButton, "open_file"))
        clickable.append((lockButton, "toggle_bar_lock"))
        clickable.append((prev as NSButton, "prev_image"))
        clickable.append((next as NSButton, "next_image"))
        clickable.append((left as NSButton, "prev_image"))
        clickable.append((right as NSButton, "next_image"))
        clickable.append((infoClose as NSButton, "toggle_info"))
        clickable.append((animButton as NSButton, "anim_toggle"))
        for (item, id) in clickable {
            item.target = self
            item.action = #selector(buttonPressed(_:))
            item.identifier = NSUserInterfaceItemIdentifier(id)
        }
        // 翻页按钮和边缘箭头：右键翻 10 张。
        let rightClicks: [(PageButton, String)] = [
            (prev, "back_10"), (next, "forward_10"), (left, "back_10"), (right, "forward_10"),
        ]
        for (item, id) in rightClicks {
            item.onRightClick = { [weak self] in
                self?.onAction?(id)
            }
        }
        infoCopy.target = self
        infoCopy.action = #selector(copyInfo(_:))

        // 悬停、按下的反馈：控制栏的按钮共用一块滑动的亮底，动画信息控制的按钮一块；
        // 两侧的切图箭头没有底板，指针移上去时变亮。
        let barButtons: [PageButton] = [openButton, select, zoomIn, zoomOut, rotateCCW, rotateCW, lockButton, prev, next]
        var attached = false
        #if compiler(>=6.2)
        if #available(macOS 26.0, *), let glass = barGlass {
            // 玻璃层画按钮图标时用的符号和字号（与上面建按钮时一致）。
            let symbols: [(PageButton, String, CGFloat)] = [
                (openButton, "eject", 15), (select, "list.bullet", 15), (zoomIn, "plus", 12), (zoomOut, "minus", 12),
                (rotateCCW, "arrow.counterclockwise", 12), (rotateCW, "arrow.clockwise", 12),
                (prev, "chevron.left", 15), (next, "chevron.right", 15),
            ]
            for (item, name, size) in symbols {
                glass.attach(item) { .symbol(name, size) }
            }
            glass.attach(lockButton) { [weak self, weak lockButton] in
                .text(lockButton?.title ?? "", accent: self?.locked ?? false)
            }
            strip.onVisual = { [weak self] visual in
                guard let self else { return }
                self.barGlass?.update(strip: visual, in: self.strip)
            }
            (bar as? GlassBarContainer)?.onLayout = { [weak self] in
                self?.strip.reportVisual()
                self?.barGlass?.refresh()
            }
            attached = true
        }
        #endif
        if !attached {
            for item in barButtons {
                barPlatter.attach(item)
            }
        }
        animPlatter.attach(animButton)
        hoverButtons = barButtons + [animButton]
        for item in [left, right] {
            item.onHover = { [weak item] inside in
                item?.contentTintColor = inside ? .white : OverlayViews.arrowTint
            }
        }
    }

    /// 箭头平时的颜色（指针在上面时白色）。
    private static let arrowTint = NSColor(white: 0.8, alpha: 0.9)

    /// 箭头的颜色按指针现在的位置定（内容视图坐标，y 向上；为空时指针不在窗口里），不只靠移进、移出：
    /// 箭头淡出时指针还在上面的话收不到“移出”，下次出来还是白的。
    private func tintArrows(at point: NSPoint?) {
        for item in [left, right] {
            let inside = point.map { !item.isHidden && item.frame.contains($0) } ?? false
            item.contentTintColor = inside ? .white : Self.arrowTint
        }
    }

    /// 收起所有悬停效果：窗口不在前面、最小化、关掉时（这时收不到“移出”，不收的话回来时还亮着）。
    func resetHover() {
        resetBarHover()
        animPlatter.reset()
        strip.resetHover()
        tintArrows(at: nil)
        tip.isHidden = true
    }

    #if compiler(>=6.2)
    /// 控制栏的 SwiftUI 玻璃层（macOS 26 起）。
    @available(macOS 26.0, *)
    private var barGlass: BarGlass? { barGlassStorage as? BarGlass }
    #endif

    /// 收起控制栏上的悬停效果（控制栏隐藏时）。
    private func resetBarHover() {
        barPlatter.reset()
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            barGlass?.reset()
        }
        #endif
    }

    /// 自检：像指针停在这个控件上（动作的标识；"strip" 是进度条，"none" 是都不停）。找不到时返回假。
    func simulateHover(_ id: String) -> Bool {
        switch id {
        case "strip":
            resetBarHover()
            strip.hoverForTest(true)
            return true
        case "none":
            resetBarHover()
            animPlatter.reset()
            strip.hoverForTest(false)
            return true
        default:
            guard let button = hoverButtons.first(where: { $0.identifier?.rawValue == id }) else { return false }
            strip.hoverForTest(false)
            button.onHover?(true)
            return true
        }
    }

    /// 放进内容视图：控制栏在底部居中，箭头贴左右边缘，图片信息在左上角。
    func install(in container: NSView) {
        for view in [bar, left, right, info, anim] as [NSView] {
            container.addSubview(view)
        }
        let animBottom = anim.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -16)
        self.animBottom = animBottom
        container.addSubview(tip)
        NSLayoutConstraint.activate([
            bar.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            bar.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -16),
            left.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 12),
            left.centerYAnchor.constraint(equalTo: container.centerYAnchor),
            right.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -12),
            right.centerYAnchor.constraint(equalTo: container.centerYAnchor),
            info.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 16),
            // 标题栏透明、内容铺到它下面：图片信息放在标题栏下面。
            info.topAnchor.constraint(equalTo: container.safeAreaLayoutGuide.topAnchor, constant: 12),
            anim.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 16),
            animBottom,
        ])
        strip.onHover = { [weak self, weak container] text, x in
            guard let self, let container else { return }
            self.showTip(text, x: x, in: container)
        }
    }

    @objc private func buttonPressed(_ sender: NSButton) {
        guard let id = sender.identifier?.rawValue else { return }
        onAction?(id)
    }

    @objc private func copyInfo(_ sender: Any?) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(infoText, forType: .string)
    }

    private func showTip(_ text: String?, x: CGFloat, in container: NSView) {
        guard let text, !bar.isHidden else {
            tip.isHidden = true
            return
        }
        tipLabel.stringValue = text
        // 底板按文字的大小：左右各 8 点、上下各 3 点（与 init 里的约束一致）。
        let label = tipLabel.fittingSize
        let size = NSSize(width: ceil(label.width) + 16, height: ceil(label.height) + 6)
        let point = container.convert(NSPoint(x: x, y: 0), from: strip)
        tip.frame = NSRect(
            x: min(max(point.x - size.width / 2, 4), container.bounds.width - size.width - 4),
            y: bar.frame.maxY + 6,
            width: size.width,
            height: size.height
        )
        tip.isHidden = false
    }

    /// 按设置和语言填上文字与悬停提示，隐藏设置要求不显示的控件（设置"浏览"页）。
    func applySettings(core: AppCore, lang: Lang, settings: UiSettings) {
        let showTips = !settings.hideTooltips
        func tip(_ text: String) -> String? {
            showTips ? text : nil
        }
        // 提示里的快捷键按键盘映射（ACTIONS 规则 7）。
        func actionTip(_ id: String) -> String? {
            tip(core.actionTooltip(id: id, lang: lang))
        }
        openButton.toolTip = actionTip("open_file")
        for (item, id) in buttons {
            item.toolTip = actionTip(id)
        }
        lockButton.title = uiText(key: .barLock, lang: lang)
        lockButton.toolTip = actionTip("toggle_bar_lock")
        prev.toolTip = tip(uiText(key: .barPrevTip, lang: lang))
        next.toolTip = tip(uiText(key: .barNextTip, lang: lang))
        infoClose.toolTip = tip(uiText(key: .infoClose, lang: lang))
        infoCopy.toolTip = tip(uiText(key: .infoCopy, lang: lang))
        strip.scanningText = uiText(key: .barScanning, lang: lang)
        // 进度条的"序号/总数"提示也是悬停提示。
        strip.showsTip = showTips
        infoFontSize = settings.smallInfoFont ? 12 : 15
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            barGlass?.setEffect(settings.barHoverEffect)
        }
        #endif
        infoLabel.attributedStringValue = outlined(infoText)
        barHidden = settings.hideControlBar
        arrowsHidden = settings.hideSideArrows
        animHidden = settings.hideAnimationInfo
        animButton.toolTip = actionTip("anim_toggle")
        if barHidden {
            barFade.hide()
            resetBarHover()
            // 上面有个同名的局部函数 tip(_:)，这里要写 self.tip。
            self.tip.isHidden = true
        }
        if arrowsHidden {
            leftFade.hide()
            rightFade.hide()
        }
    }

    /// 按核心的状态刷新：控制栏锁定、按钮可用、进度条、图片信息（MENUS 各节）。
    func refresh(core: AppCore, index: UInt32?, total: UInt32?, lang: Lang) {
        let locked = core.actionState(id: "toggle_bar_lock").checked
        lockButton.contentTintColor = locked ? .controlAccentColor : .labelColor
        self.locked = locked
        openButton.isEnabled = core.actionState(id: "open_file").enabled
        for (item, id) in buttons {
            item.isEnabled = core.actionState(id: id).enabled
        }
        let hasImage = core.actionState(id: "next_image").enabled
        // 刚打开图片：标题栏马上藏起来，不先显示再淡出（默认就是无边框的样子）。
        let appeared = hasImage && !self.hasImage
        self.hasImage = hasImage
        titlebar?.setHasImage(hasImage)
        prev.isEnabled = hasImage
        next.isEnabled = hasImage
        strip.index = index
        strip.total = total
        strip.needsDisplay = true

        let lines = core.infoLines(lang: lang)
        infoText = lines.joined(separator: "\n")
        infoLabel.attributedStringValue = outlined(infoText)
        info.isHidden = !(core.actionState(id: "toggle_info").checked && !lines.isEmpty)
        if !hasImage {
            leftFade.hide()
            rightFade.hide()
        }
        updateBar(locked: locked, immediately: appeared)
        refreshAnimation(core: core)
        // 玻璃层按按钮现在的图标（锁定）、能不能按、控制栏显示没有来画。
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            barGlass?.refresh()
        }
        #endif
    }

    /// 动画信息控制：正在显示动图时出现（设置"不显示动画信息控制"时不出现）；控制栏显示时让到它上面。
    func refreshAnimation(core: AppCore) {
        let info = core.animation()
        let hidden = info == nil || animHidden
        if hidden && !anim.isHidden {
            // 换到不是动图的图片时指针可能还停在按钮上：收不到“移出”，不收的话下次出来还亮着。
            animPlatter.reset()
        }
        anim.isHidden = hidden
        if let info {
            let count = info.count.map { String($0) } ?? "?"
            animLabel.stringValue = "\(info.index + 1) / \(count)"
            animButton.image = NSImage(systemSymbolName: info.playing ? "pause.fill" : "play.fill",
                                       accessibilityDescription: nil)?
                .withSymbolConfiguration(NSImage.SymbolConfiguration(pointSize: 12, weight: .medium))
        }
        animBottom?.constant = barFade.isShown ? -(bar.frame.height + 24) : -16
    }

    /// 动画信息控制的文字（自检用）；不显示时为空。
    var animationText: String? {
        anim.isHidden ? nil : animLabel.stringValue
    }

    /// 白字深色描边。
    private func outlined(_ text: String) -> NSAttributedString {
        let shadow = NSShadow()
        shadow.shadowColor = .black
        shadow.shadowBlurRadius = 2
        shadow.shadowOffset = NSSize(width: 1, height: -1)
        return NSAttributedString(string: text, attributes: [
            .font: NSFont.systemFont(ofSize: infoFontSize),
            .foregroundColor: NSColor.white,
            .strokeColor: NSColor.black,
            .strokeWidth: -2.0,
            .shadow: shadow,
        ])
    }

    /// 控制栏：在控制栏区域、指针在控制栏上或已锁定时显示；否则 1 秒后淡出。
    /// 标题栏跟着控制栏，另外在指针靠近窗口顶部或没有图片时也显示。
    private func updateBar(locked: Bool, immediately: Bool = false) {
        let wantBar = !barHidden && (locked || zone == .bar || pointerOnBar)
        updateTitlebar(shown: wantBar || pointerNearTop || !hasImage, immediately: immediately)
        if wantBar {
            barHideSerial += 1
            barHidePending = false
            barFade.show()
            return
        }
        guard barFade.isShown, !barHidePending else { return }
        barHideSerial += 1
        barHidePending = true
        let serial = barHideSerial
        let delay = Double(timing.barHideDelayMs) / 1000
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.barHideSerial == serial else { return }
                self.barHidePending = false
                self.barFade.hide()
                self.resetBarHover()
                self.tip.isHidden = true
            }
        }
    }

    /// 标题栏：要显示时马上淡入；不要时与控制栏一样等 1 秒再淡出（immediately 为真时马上隐藏）。
    private func updateTitlebar(shown: Bool, immediately: Bool = false) {
        // 设置"显示标题栏"打开时一直显示。
        guard let titlebar, !titlebar.alwaysShown else { return }
        if shown {
            titleHideSerial += 1
            titleHidePending = false
            titlebar.set(shown: true)
            return
        }
        if immediately {
            titleHideSerial += 1
            titleHidePending = false
            titlebar.set(shown: false, animated: false)
            return
        }
        guard titlebar.isShown, !titleHidePending else { return }
        titleHideSerial += 1
        titleHidePending = true
        let serial = titleHideSerial
        let delay = Double(timing.barHideDelayMs) / 1000
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.titleHideSerial == serial else { return }
                self.titleHidePending = false
                self.titlebar?.set(shown: false)
            }
        }
    }

    /// 指针移动（内容区的点坐标，原点在左上）：按核心的区域规则显示控制栏和箭头。
    func pointerMoved(x: Double, y: Double, width: Double, height: Double, core: AppCore) {
        zone = hoverZone(x: x, y: y, width: width, height: height)
        // 靠近顶部：标题栏的高度再加 24 点。
        pointerNearTop = y < Double(titlebar?.height ?? 28) + 24
        let hasImage = core.actionState(id: "next_image").enabled && !arrowsHidden
        switch zone {
        case .left where hasImage:
            leftFade.show()
            rightFade.hide()
        case .right where hasImage:
            rightFade.show()
            leftFade.hide()
        default:
            leftFade.hide()
            rightFade.hide()
        }
        // 静止 2 秒后箭头淡出；每次移动重新计时。
        arrowSerial += 1
        let serial = arrowSerial
        let idle = Double(timing.arrowIdleMs) / 1000
        DispatchQueue.main.asyncAfter(deadline: .now() + idle) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.arrowSerial == serial else { return }
                self.leftFade.hide()
                self.rightFade.hide()
                self.tintArrows(at: nil)
            }
        }
        let barRect = bar.isHidden ? NSRect.zero : bar.frame
        if let container = bar.superview {
            // 指针在控制栏上时不隐藏（坐标换回 y 向上）。
            let point = NSPoint(x: x, y: container.bounds.height - y)
            pointerOnBar = barRect.contains(point)
            tintArrows(at: point)
        }
        updateBar(locked: core.actionState(id: "toggle_bar_lock").checked)
    }

    func pointerExited(core: AppCore) {
        zone = .none
        pointerOnBar = false
        pointerNearTop = false
        leftFade.hide()
        rightFade.hide()
        tintArrows(at: nil)
        updateBar(locked: core.actionState(id: "toggle_bar_lock").checked)
    }

    /// 自检：当前可见的悬浮控件（内容视图坐标，y 向上），包括标题栏下面的渐变。
    var coveredRects: [NSRect] {
        var rects = ([bar, left, right, info, anim] as [NSView]).filter { !$0.isHidden }.map(\.frame)
        if let scrim = titlebar?.coveredRect {
            rects.append(scrim)
        }
        return rects
    }
}
