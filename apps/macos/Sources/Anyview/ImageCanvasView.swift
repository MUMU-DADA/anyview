import AppKit
import AnyviewCore

/// 鼠标按键要做的事（设置"鼠标"页）。
enum MouseTarget: Equatable {
    case action(String)
    case contextMenu
    case nothing
}

/// 图片画布（ARCHITECTURE"画布与渲染"的交互路径）：图片作为 CALayer 的内容由 GPU 合成。
/// 位置、倍率、旋转、翻转都由核心计算（物理像素），这里只换算成点并设到图层上。
/// 鼠标与触控板：左键拖动平移、双击缩放窗口、右键菜单、滚轮翻页（按住 ⌘ 缩放）、
/// 触控板双指平移、捏合缩放、双指轻点两下切换倍率、旋转手势按 90° 步进。
final class ImageCanvasView: NSView {
    /// 背景图片：按"填满并裁切"铺满视图，在最下面，翻页时也不动。
    private let backgroundImageLayer = CALayer()
    /// 平常的画面（棋盘格、图片、精确重绘）都放在这一层里，大小与视图相同：翻页效果时整层移动、翻转。
    private let contentLayer = CALayer()
    /// 正在做的翻页效果。
    private var turn: PageTurn?
    /// 自检用：下一次翻页效果停在这个进度（0–1），不随时间走；停住时用平面的画法（截图画不了三维变换）。
    var holdTransition: Double?
    private let imageLayer = CALayer()
    /// 透明图片下面的棋盘格，与图片图层的位置和变换相同。
    private let checkerLayer = CALayer()
    /// 精确重绘的覆盖层：与屏幕物理像素一一对应，签名与当前画面相同时才显示。
    private let exactLayer = CALayer()
    private var exactSignature: UInt64?
    private var exactInfo: (x: Int64, y: Int64, width: Int, height: Int, renderMs: Double)?
    private var scheduledSignature: UInt64 = 0
    private var exactSerial = 0
    private var hasImage = false
    /// 当前图片（完整的显示缓冲区；超大图的纹理是其中的代理）。快速预览时是预览。
    private(set) var currentHandle: ImageHandle?
    /// 正在显示快速预览。
    private(set) var showingPreview = false
    /// 核心（查看计算是同步的）。
    var core: AppCore?
    /// 键盘事件先交给窗口控制器处理；返回 true 表示已处理。
    var onKey: ((NSEvent) -> Bool)?
    var onOpenURL: ((URL) -> Void)?
    /// 执行动作（双击、滚轮翻页、旋转手势）。
    var onAction: ((String) -> Void)?
    /// 手势改变了倍率，显示倍率提示。
    var onZoomChanged: (() -> Void)?
    /// 核心已经执行了动作（双指轻扫翻页），由窗口控制器处理结果。
    var onOutcome: ((ActionOutcome) -> Void)?
    /// 右键菜单，每次弹出时重新生成。
    var makeContextMenu: (() -> NSMenu?)?
    /// 窗口现在能不能移动（最大化、全屏时不能），由窗口控制器给出。
    var windowMovable: (() -> Bool)?
    /// 吸附边缘：给出窗口应在的原点，设置关掉时为空；由窗口控制器给出。
    var snapOrigin: ((NSRect) -> NSPoint?)?
    /// 自己移动窗口（打开"拖动窗口时吸附边缘"时，拖动中就要贴边）：按下时窗口的原点和指针的屏幕位置。
    private var moveStart: (origin: NSPoint, mouse: NSPoint)?
    /// 等着判断是不是单击的左键按下（映射了左键或打开了"单击屏幕两侧时翻页"，规则由核心给出）。
    private var pendingClick: NSEvent?
    /// 中键按下的位置：松开时没怎么动就算中键单击。
    private var middleDown: NSPoint?
    private var dragLast: NSPoint?
    private var rotationSum: CGFloat = 0

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = EmptyStateView.defaultBackground.cgColor
        imageLayer.contentsGravity = .resize
        imageLayer.minificationFilter = .trilinear
        imageLayer.magnificationFilter = .linear
        imageLayer.isHidden = true
        checkerLayer.isHidden = true
        checkerLayer.backgroundColor = Self.checkerColor
        exactLayer.isHidden = true
        exactLayer.contentsGravity = .resize
        exactLayer.magnificationFilter = .nearest
        exactLayer.minificationFilter = .nearest
        contentLayer.addSublayer(checkerLayer)
        contentLayer.addSublayer(imageLayer)
        contentLayer.addSublayer(exactLayer)
        backgroundImageLayer.contentsGravity = .resizeAspectFill
        backgroundImageLayer.masksToBounds = true
        backgroundImageLayer.isHidden = true
        layer?.addSublayer(backgroundImageLayer)
        layer?.addSublayer(contentLayer)
        placeContentLayer()
        registerForDraggedTypes([.fileURL])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override var acceptsFirstResponder: Bool { true }

    /// 一个点对应的物理像素数。
    private var pixelScale: CGFloat {
        window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
    }

    /// 显示一张新图片；传 nil 清空。旋转、倍率等由核心按切图与锁定规则处理。
    func show(_ handle: ImageHandle?) {
        if let handle {
            display(handle, width: handle.width(), height: handle.height(), preview: false)
        } else {
            clear()
        }
    }

    /// 快速预览：小缩略图拉伸到原图尺寸显示，查看状态按新图片处理；清晰图随后由 replaceImage 换上。
    func showPreview(_ handle: ImageHandle, width: UInt32, height: UInt32) {
        display(handle, width: width, height: height, preview: true)
    }

    /// 图层的内容是交给 GPU 的纹理（超大图是缩小的代理），按原图尺寸 width x height 摆放、拉伸。
    private func display(_ handle: ImageHandle, width: UInt32, height: UInt32, preview: Bool) {
        guard let cgImage = Self.makeCGImage(handle.texture()) else {
            clear()
            return
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        imageLayer.contents = cgImage
        currentHandle = handle
        showingPreview = preview
        imageLayer.isHidden = false
        checkerLayer.isHidden = !handle.hasAlpha()
        refreshAlphaFill()
        hideExact()
        scheduledSignature = 0
        hasImage = true
        updateViewport()
        apply(core?.viewShowImage(width: width, height: height))
        CATransaction.commit()
    }

    private func clear() {
        endTransition()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        imageLayer.contents = nil
        currentHandle = nil
        showingPreview = false
        imageLayer.isHidden = true
        checkerLayer.isHidden = true
        hideExact()
        hasImage = false
        core?.viewClear()
        CATransaction.commit()
    }

    /// 同一张图片换了显示缓冲区（反相、Gamma，或清晰图替换快速预览）：只换图像，查看状态不变。
    func replaceImage(_ handle: ImageHandle) {
        guard hasImage, let cgImage = Self.makeCGImage(handle.texture()) else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        imageLayer.contents = cgImage
        currentHandle = handle
        showingPreview = false
        checkerLayer.isHidden = !handle.hasAlpha()
        refreshAlphaFill()
        hideExact()
        CATransaction.commit()
        scheduledSignature = 0
        scheduleExact()
    }

    /// 盖上精确重绘的结果（物理像素 (x, y) 处）；画面已经变了（签名不同）就不用。
    func showExact(_ handle: ImageHandle, x: Int64, y: Int64, signature: UInt64, renderMs: Double) {
        guard hasImage, let core, signature == core.viewSignature(), let cgImage = Self.makeCGImage(handle) else { return }
        let scale = Double(pixelScale)
        let width = Double(handle.width())
        let height = Double(handle.height())
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        exactLayer.contents = cgImage
        exactLayer.contentsScale = pixelScale
        // 视图坐标 y 向上：从顶部算起的 y 换成从底部算起。
        exactLayer.frame = CGRect(
            x: Double(x) / scale,
            y: Double(bounds.height) - (Double(y) + height) / scale,
            width: width / scale,
            height: height / scale
        )
        exactLayer.isHidden = false
        CATransaction.commit()
        exactSignature = signature
        exactInfo = (x, y, Int(width), Int(height), renderMs)
    }

    /// 精确重绘的结果正盖在画面上（自检用）。
    var exactShown: (x: Int64, y: Int64, width: Int, height: Int, renderMs: Double)? {
        guard let core, let signature = exactSignature, signature == core.viewSignature() else { return nil }
        return exactInfo
    }

    private func hideExact() {
        guard exactSignature != nil || !exactLayer.isHidden else { return }
        exactLayer.isHidden = true
        exactLayer.contents = nil
        exactSignature = nil
        exactInfo = nil
    }

    /// 画面变了：撤下覆盖层；静止 100 ms 后请求精确重绘（不需要时核心什么也不做）。
    private func scheduleExact() {
        guard hasImage, let core else { return }
        let signature = core.viewSignature()
        if exactSignature == signature {
            return
        }
        hideExact()
        guard scheduledSignature != signature else { return }
        scheduledSignature = signature
        exactSerial += 1
        let serial = exactSerial
        let delay = Double(exactRenderDelayMs()) / 1000
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.exactSerial == serial, let core = self.core,
                      core.viewSignature() == signature else { return }
                core.requestExact()
            }
        }
    }

    /// 核心的查看参数变了（菜单、按键、手势），重新摆放图片。
    func refreshLayout() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        apply(core?.viewLayout())
        CATransaction.commit()
    }

    override func layout() {
        super.layout()
        relayoutForSize()
    }

    /// 窗口缩放时视图靠自动调整尺寸改变大小，不一定会触发 layout，这里也重新摆放图片。
    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        relayoutForSize()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        relayoutForSize()
    }

    private func relayoutForSize() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        placeContentLayer()
        apply(updateViewport())
        CATransaction.commit()
    }

    /// 内容层与视图一样大（用 bounds 和 position：翻页时它带着变换，不能设 frame）。
    private func placeContentLayer() {
        contentLayer.bounds = CGRect(origin: .zero, size: bounds.size)
        contentLayer.position = CGPoint(x: bounds.midX, y: bounds.midY)
        backgroundImageLayer.frame = bounds
    }

    /// 背景图片：传 nil 时用背景色。
    func setBackgroundImage(_ handle: ImageHandle?) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        backgroundImageLayer.contents = handle.flatMap { Self.makeCGImage($0) }
        backgroundImageLayer.contentsScale = pixelScale
        backgroundImageLayer.isHidden = backgroundImageLayer.contents == nil
        CATransaction.commit()
    }

    /// 现在有没有背景图片（自检用）。
    var hasBackgroundImage: Bool {
        !backgroundImageLayer.isHidden
    }

    // MARK: 翻页效果
    // 开始时把现在的画面（棋盘格、图片、精确重绘）复制成一个旧画面层，然后照常换上新图片；
    // 之后每一帧按核心算出的参数摆放新旧两层，做完拿掉旧画面层。

    /// 开始翻页效果：在换上新图片之前调用。画布上没有图片（空态）时不做。
    func beginTransition(effect: PageEffect, forward: Bool, durationMs: UInt32) {
        endTransition()
        guard effect != PageEffect.none, hasImage, window != nil, let root = layer else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        let old = CALayer()
        old.bounds = CGRect(origin: .zero, size: bounds.size)
        old.position = CGPoint(x: bounds.midX, y: bounds.midY)
        for source in [checkerLayer, imageLayer, exactLayer] where !source.isHidden {
            let copy = CALayer()
            copy.contents = source.contents
            copy.backgroundColor = source.backgroundColor
            copy.contentsGravity = source.contentsGravity
            copy.contentsScale = source.contentsScale
            copy.magnificationFilter = source.magnificationFilter
            copy.minificationFilter = source.minificationFilter
            copy.bounds = source.bounds
            copy.position = source.position
            copy.setAffineTransform(source.affineTransform())
            old.addSublayer(copy)
        }
        let oldShade = Self.shadeLayer(frame: imageLayer.frame)
        old.addSublayer(oldShade)
        let newShade = Self.shadeLayer(frame: imageLayer.frame)
        contentLayer.addSublayer(newShade)
        let first = AnyviewCore.transitionFrame(effect: effect, forward: forward, progress: 0)
        if first.oldOnTop {
            root.insertSublayer(old, above: contentLayer)
        } else {
            root.insertSublayer(old, below: contentLayer)
        }
        turn = PageTurn(old: old, oldShade: oldShade, newShade: newShade, oldSize: bounds.size, effect: effect,
                        forward: forward, duration: Double(durationMs) / 1000, hold: holdTransition)
        CATransaction.commit()
        applyTurn()
        guard holdTransition == nil else { return }
        // 每秒 60 次按时间摆放（拖动窗口时也走）。
        let timer = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.applyTurn()
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        turn?.timer = timer
    }

    /// 正在做的翻页效果（自检用）：效果、方向和现在的进度。
    var transitionState: (effect: PageEffect, forward: Bool, progress: Double)? {
        guard let turn else { return nil }
        return (turn.effect, turn.forward, turn.progress)
    }

    /// 自检用：取消停住，并结束停着的效果。
    func releaseHeldTransition() {
        holdTransition = nil
        if turn?.hold != nil {
            endTransition()
        }
    }

    private func applyTurn() {
        guard let turn else { return }
        let progress = turn.progress
        let frame = AnyviewCore.transitionFrame(effect: turn.effect, forward: turn.forward, progress: progress)
        let flat = turn.hold != nil
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        // 旧画面按它原来的大小居中（窗口跟着新图片变了大小时，屏幕上的位置不变）。
        turn.old.position = CGPoint(x: bounds.midX, y: bounds.midY)
        turn.newShade.frame = imageLayer.frame
        place(turn.old, with: frame.old, shade: turn.oldShade, flat: flat)
        place(contentLayer, with: frame.new, shade: turn.newShade, flat: flat)
        CATransaction.commit()
        if progress >= 1, turn.hold == nil {
            endTransition()
        }
    }

    /// 按一层的参数摆放：平移、以画布左边为轴翻转（透视；自检停住时用水平压扁）、不透明度、变暗。
    private func place(_ target: CALayer, with params: LayerParams, shade: CALayer, flat: Bool) {
        let width = bounds.width
        target.opacity = Float(params.opacity)
        var transform = CATransform3DMakeTranslation(CGFloat(params.shift) * width, 0, 0)
        if params.angle != 0 {
            let theta = CGFloat(params.angle) * .pi / 180
            // 转轴是画布左边：相对于这一层的中心在 -width/2 处。
            let toHinge = CATransform3DMakeTranslation(width / 2, 0, 0)
            let back = CATransform3DMakeTranslation(-width / 2, 0, 0)
            if flat {
                let squeeze = CATransform3DMakeScale(max(0, cos(theta)), 1, 1)
                transform = CATransform3DConcat(CATransform3DConcat(toHinge, squeeze), back)
            } else {
                var perspective = CATransform3DIdentity
                perspective.m34 = -1 / (CGFloat(AnyviewCore.transitionPerspective()) * max(width, 1))
                let rotate = CATransform3DMakeRotation(theta, 0, 1, 0)
                transform = CATransform3DConcat(
                    CATransform3DConcat(CATransform3DConcat(toHinge, rotate), back), perspective)
            }
        }
        target.transform = transform
        shade.opacity = Float(params.shade)
    }

    private func endTransition() {
        guard let turn else { return }
        turn.timer?.invalidate()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        turn.old.removeFromSuperlayer()
        turn.newShade.removeFromSuperlayer()
        contentLayer.transform = CATransform3DIdentity
        contentLayer.opacity = 1
        CATransaction.commit()
        self.turn = nil
    }

    /// 翻起的一页变暗：盖在图片上的一层黑色（不透明度随效果变）。
    private static func shadeLayer(frame: CGRect) -> CALayer {
        let shade = CALayer()
        shade.backgroundColor = NSColor.black.cgColor
        shade.opacity = 0
        shade.frame = frame
        return shade
    }

    @discardableResult
    private func updateViewport() -> ViewLayout? {
        let scale = Double(pixelScale)
        // 100% 是一个图片像素一个点：先告诉核心这块屏幕一个点几个物理像素。
        _ = core?.viewSetPixelsPerPoint(pixelsPerPoint: scale)
        return core?.viewSetViewport(width: Double(bounds.width) * scale, height: Double(bounds.height) * scale)
    }

    /// 把核心的绘制参数（物理像素，原点在左上、y 向下）换算到图层（点，原点在左下、y 向上）。
    /// 图层先顺时针旋转（y 向上的坐标里角度取负），再按屏幕方向翻转。
    private func apply(_ layout: ViewLayout?) {
        guard hasImage, let layout else { return }
        let scale = Double(pixelScale)
        imageLayer.setAffineTransform(.identity)
        imageLayer.bounds = CGRect(x: 0, y: 0, width: layout.width / scale, height: layout.height / scale)
        imageLayer.position = CGPoint(x: layout.centerX / scale, y: Double(bounds.height) - layout.centerY / scale)
        let rotation = CGAffineTransform(rotationAngle: -CGFloat(layout.rotation) * .pi / 180)
        let flip = CGAffineTransform(scaleX: layout.flipHorizontal ? -1 : 1, y: layout.flipVertical ? -1 : 1)
        imageLayer.setAffineTransform(rotation.concatenating(flip))
        imageLayer.magnificationFilter = layout.smooth ? .linear : .nearest
        imageLayer.minificationFilter = layout.smooth ? .trilinear : .nearest
        imageLayer.contentsScale = pixelScale
        // 棋盘格跟着图片走：方格按点计算，不随缩放变化。
        checkerLayer.setAffineTransform(.identity)
        checkerLayer.bounds = imageLayer.bounds
        checkerLayer.position = imageLayer.position
        checkerLayer.setAffineTransform(imageLayer.affineTransform())
        scheduleExact()
    }

    /// 背景色（设置"背景颜色"）。
    func setBackground(_ color: NSColor) {
        layer?.backgroundColor = color.cgColor
    }

    /// 透明处的背景：按当前图片由核心决定（GIF 与 32 位图片各有设置）：棋盘格或纯色。
    /// 显示图片时、设置生效后调用。
    func refreshAlphaFill() {
        guard let core, let handle = currentHandle else {
            checkerLayer.backgroundColor = Self.checkerColor
            return
        }
        switch core.alphaFill(image: handle) {
        case .checker:
            checkerLayer.backgroundColor = Self.checkerColor
        case let .solid(rgb):
            checkerLayer.backgroundColor = NSColor(
                srgbRed: CGFloat((rgb >> 16) & 0xFF) / 255,
                green: CGFloat((rgb >> 8) & 0xFF) / 255,
                blue: CGFloat(rgb & 0xFF) / 255,
                alpha: 1
            ).cgColor
        }
    }

    /// 棋盘格图案：#FFFFFF 与 #C8C8C8，每格 8 点。
    private static let checkerColor: CGColor = {
        let image = NSImage(size: NSSize(width: 16, height: 16), flipped: false) { rect in
            NSColor.white.setFill()
            rect.fill()
            NSColor(srgbRed: 200.0 / 255.0, green: 200.0 / 255.0, blue: 200.0 / 255.0, alpha: 1).setFill()
            NSRect(x: 8, y: 0, width: 8, height: 8).fill()
            NSRect(x: 0, y: 8, width: 8, height: 8).fill()
            return true
        }
        return NSColor(patternImage: image).cgColor
    }()

    /// 事件位置 → 画布内的物理像素（原点在左上）。
    private func pixelPoint(_ event: NSEvent) -> (x: Double, y: Double) {
        let point = convert(event.locationInWindow, from: nil)
        let scale = Double(pixelScale)
        return (Double(point.x) * scale, Double(bounds.height - point.y) * scale)
    }

    private func viewChanged(_ layout: ViewLayout?, zoom: Bool) {
        guard layout != nil else { return }
        refreshLayout()
        if zoom {
            onZoomChanged?()
        }
    }

    // MARK: 键盘

    override func keyDown(with event: NSEvent) {
        if onKey?(event) == true {
            return
        }
        super.keyDown(with: event)
    }

    // MARK: 鼠标与触控板

    /// 窗口不在前台时，右键也直接弹出菜单（与访达一致）；左键也直接接收：拖图片就能移动窗口，
    /// 和拖标题栏一样，只是单击时窗口不动、只被激活。
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool {
        event?.type == .rightMouseDown || event?.type == .leftMouseDown
    }

    override func mouseDown(with event: NSEvent) {
        // ⌃ + 点按是 macOS 的辅助点按，与右键相同。
        if event.modifierFlags.contains(.control) {
            showContextMenu(with: event)
            return
        }
        guard let core else { return }
        moveStart = nil
        let fraction = xFraction(event)
        // 双击：默认最大化、还原；映射了左键或两侧单击翻页时由核心决定（可能什么也不做）。
        if event.clickCount >= 2, let action = core.leftDoubleClickAction(xFraction: fraction) {
            dragLast = nil
            pendingClick = nil
            onAction?(action)
            return
        }
        // 单击要等松开才能判断：先不拖动，移动超过几个点才算拖动（设置"鼠标"页）。
        if core.leftClickMatters() {
            dragLast = nil
            pendingClick = event
            return
        }
        startLeftDrag(event)
    }

    /// 左键拖动："拖着图片走"。图片完整显示、窗口能移动时用系统的窗口拖动（真的移动了才挪窗口，
    /// 屏幕边缘的贴靠照常）；图片放大到超出窗口，或最大化、全屏时平移图片。
    /// 打开"拖动窗口时吸附边缘"时由程序自己移动窗口，拖动中靠近屏幕边缘就贴上去。
    private func startLeftDrag(_ event: NSEvent) {
        if let core, let window, core.leftDragMovesWindow(windowMovable: windowMovable?() ?? false) {
            dragLast = nil
            if core.uiSettings().snapToEdges {
                moveStart = (window.frame.origin, window.convertPoint(toScreen: event.locationInWindow))
                return
            }
            window.performDrag(with: event)
            return
        }
        dragLast = event.locationInWindow
    }

    /// 自己移动窗口（吸附边缘）：窗口跟着指针走，离屏幕边缘够近时贴上去。
    private func moveWindow(with event: NSEvent) {
        guard let start = moveStart, let window else { return }
        let mouse = window.convertPoint(toScreen: event.locationInWindow)
        var frame = window.frame
        frame.origin = NSPoint(x: start.origin.x + mouse.x - start.mouse.x, y: start.origin.y + mouse.y - start.mouse.y)
        if let snapped = snapOrigin?(frame) {
            frame.origin = snapped
        }
        window.setFrameOrigin(frame.origin)
    }

    /// 点在画布宽度上的位置（0–1），左键单击、双击的规则用。
    private func xFraction(_ event: NSEvent) -> Double {
        let point = convert(event.locationInWindow, from: nil)
        return Double(point.x / max(bounds.width, 1))
    }

    override func mouseDragged(with event: NSEvent) {
        if let down = pendingClick {
            let a = down.locationInWindow
            let b = event.locationInWindow
            // 移动超过 4 点才算拖动，之后与平常一样移动窗口（用按下时的事件交给系统）或平移图片。
            guard hypot(b.x - a.x, b.y - a.y) >= 4 else { return }
            pendingClick = nil
            if let core, let window, core.leftDragMovesWindow(windowMovable: windowMovable?() ?? false) {
                if core.uiSettings().snapToEdges {
                    moveStart = (window.frame.origin, window.convertPoint(toScreen: a))
                    moveWindow(with: event)
                    return
                }
                window.performDrag(with: down)
                return
            }
            dragLast = a
        }
        if moveStart != nil {
            moveWindow(with: event)
            return
        }
        guard let last = dragLast, let core else { return }
        let point = event.locationInWindow
        dragLast = point
        let scale = Double(pixelScale)
        // 窗口坐标 y 向上，核心的坐标 y 向下。
        viewChanged(core.viewPanBy(dx: Double(point.x - last.x) * scale, dy: -Double(point.y - last.y) * scale), zoom: false)
    }

    override func mouseUp(with event: NSEvent) {
        dragLast = nil
        moveStart = nil
        // 没有拖动就松开：单击（映射了左键时执行映射的动作；两侧单击翻页）。
        if pendingClick != nil {
            pendingClick = nil
            if let action = core?.leftClickAction(xFraction: xFraction(event)) {
                onAction?(action)
            }
        }
    }

    /// 鼠标按键要做的事：设置里映射了就按映射，没有映射时按默认（ACTIONS"鼠标与触控板的默认动作"：
    /// 右键弹出菜单，侧键和滚轮翻页，中键单击不做事）。
    func mouseTarget(_ input: MouseInput) -> MouseTarget {
        guard let core else { return .nothing }
        switch core.mouseAction(input: input) {
        case let .action(id):
            return .action(id)
        case .nothing:
            return .nothing
        case .default:
            switch input {
            case .right: return .contextMenu
            case .back, .wheelUp: return .action("prev_image")
            case .forward, .wheelDown: return .action("next_image")
            case .left, .middle: return .nothing
            }
        }
    }

    private func runMouse(_ input: MouseInput, event: NSEvent) {
        switch mouseTarget(input) {
        case let .action(id): onAction?(id)
        case .contextMenu: showContextMenu(with: event)
        case .nothing: break
        }
    }

    override func scrollWheel(with event: NSEvent) {
        guard let core else { return }
        let anchor = pixelPoint(event)
        // 按住 ⌘ 或 ⌃ 滚动都是缩放：Windows 键盘上的 Ctrl 在 Mac 上是 ⌃。
        let zoom = event.modifierFlags.contains(.command) || event.modifierFlags.contains(.control)
        // 去掉"自然滚动"的反转，得到滚轮实际转动的方向：正数为向上。
        let up = event.isDirectionInvertedFromDevice ? -event.scrollingDeltaY : event.scrollingDeltaY
        if event.hasPreciseScrollingDeltas {
            // 触控板、妙控鼠标：画面跟着手指走；按住 ⌘ 时连续缩放。
            if !zoom {
                trackSwipe(event)
            }
            if zoom {
                viewChanged(core.viewZoomBy(factor: exp(Double(up) * 0.01), x: anchor.x, y: anchor.y), zoom: true)
            } else {
                let scale = Double(pixelScale)
                viewChanged(
                    core.viewPanBy(dx: Double(event.scrollingDeltaX) * scale, dy: Double(event.scrollingDeltaY) * scale),
                    zoom: false
                )
            }
            return
        }
        // 普通滚轮：默认向上为上一张，向下为下一张，设置里可以映射；按住 ⌘ 时以指针为中心按档位缩放。
        guard up != 0 else { return }
        if zoom {
            viewChanged(core.viewZoomStep(direction: up > 0 ? 1 : -1, x: anchor.x, y: anchor.y), zoom: true)
        } else {
            runMouse(up > 0 ? .wheelUp : .wheelDown, event: event)
        }
    }

    /// 双指左右轻扫翻页：把手势的开始、位移、结束交给核心，由核心判断是否翻页。
    /// 惯性滚动（手指离开后）不计入。
    private func trackSwipe(_ event: NSEvent) {
        guard let core else { return }
        let scale = Double(pixelScale)
        let dx = Double(event.scrollingDeltaX) * scale
        let dy = Double(event.scrollingDeltaY) * scale
        if event.phase.contains(.began) {
            _ = core.viewSwipe(phase: .began, dx: 0, dy: 0)
        }
        if event.phase.contains(.changed) {
            _ = core.viewSwipe(phase: .changed, dx: dx, dy: dy)
        }
        if event.phase.contains(.ended) {
            if let outcome = core.viewSwipe(phase: .ended, dx: 0, dy: 0) {
                onOutcome?(outcome)
            }
        } else if event.phase.contains(.cancelled) {
            _ = core.viewSwipe(phase: .cancelled, dx: 0, dy: 0)
        }
    }

    /// 鼠标侧键：默认后退键为上一张，前进键为下一张，设置里可以映射（"X 键""Y 键"）。
    override func otherMouseDown(with event: NSEvent) {
        switch event.buttonNumber {
        // 中键拖动：任何时候都平移图片；单击中键默认不做事，可以映射。
        case 2:
            moveStart = nil
            dragLast = event.locationInWindow
            middleDown = event.locationInWindow
        case 3: runMouse(.back, event: event)
        case 4: runMouse(.forward, event: event)
        default: super.otherMouseDown(with: event)
        }
    }

    override func otherMouseDragged(with event: NSEvent) {
        guard event.buttonNumber == 2 else {
            super.otherMouseDragged(with: event)
            return
        }
        mouseDragged(with: event)
    }

    override func otherMouseUp(with event: NSEvent) {
        guard event.buttonNumber == 2 else {
            super.otherMouseUp(with: event)
            return
        }
        dragLast = nil
        if let down = middleDown {
            middleDown = nil
            let up = event.locationInWindow
            if hypot(up.x - down.x, up.y - down.y) < 4 {
                runMouse(.middle, event: event)
            }
        }
    }

    override func magnify(with event: NSEvent) {
        guard let core else { return }
        let anchor = pixelPoint(event)
        viewChanged(core.viewZoomBy(factor: 1 + Double(event.magnification), x: anchor.x, y: anchor.y), zoom: true)
    }

    override func smartMagnify(with event: NSEvent) {
        guard let core else { return }
        let anchor = pixelPoint(event)
        viewChanged(core.viewToggleFitActual(x: anchor.x, y: anchor.y), zoom: true)
    }

    /// 旋转手势：累计转过 45° 就按 90° 旋转一次。rotation 为逆时针的度数。
    override func rotate(with event: NSEvent) {
        if event.phase == .began {
            rotationSum = 0
        }
        rotationSum += CGFloat(event.rotation)
        if rotationSum >= 45 {
            rotationSum -= 90
            onAction?("rotate_ccw")
        } else if rotationSum <= -45 {
            rotationSum += 90
            onAction?("rotate_cw")
        }
    }

    /// 右键弹出右键菜单。自己处理，不依赖系统默认的右键处理：菜单每次按核心的数据重新生成，
    /// 勾选和灰显都是当时的状态。
    override func rightMouseDown(with event: NSEvent) {
        // 默认弹出右键菜单；设置里映射了右键就执行映射的动作（⌃ 点按仍然弹出菜单）。
        runMouse(.right, event: event)
    }

    private func showContextMenu(with event: NSEvent) {
        dragLast = nil
        guard let menu = makeContextMenu?() else { return }
        NSMenu.popUpContextMenu(menu, with: event, for: self)
    }

    // MARK: 拖放

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        Self.fileURL(from: sender) == nil ? [] : .copy
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard let url = Self.fileURL(from: sender) else { return false }
        onOpenURL?(url)
        return true
    }

    private static func fileURL(from sender: NSDraggingInfo) -> URL? {
        let objects = sender.draggingPasteboard.readObjects(
            forClasses: [NSURL.self],
            options: [.urlReadingFileURLsOnly: true]
        )
        return objects?.first as? URL
    }

    /// 让 CGImage 直接引用 Rust 持有的像素（BGRA8 预乘，不复制）；CGImage 释放时再释放这份引用。
    static func makeCGImage(_ handle: ImageHandle) -> CGImage? {
        guard let base = UnsafeRawPointer(bitPattern: UInt(handle.pixelAddress())) else { return nil }
        let retained = Unmanaged.passRetained(handle)
        guard let provider = CGDataProvider(
            dataInfo: retained.toOpaque(),
            data: base,
            size: Int(handle.byteLen()),
            releaseData: { info, _, _ in
                if let info {
                    Unmanaged<ImageHandle>.fromOpaque(info).release()
                }
            }
        ) else {
            retained.release()
            return nil
        }
        let bitmapInfo = CGBitmapInfo(
            rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        )
        // 色彩空间：图片带 ICC 时按它解释，由 ColorSync 匹配显示器（保留广色域）；否则为 sRGB。
        // 只认 RGB 的：灰度、CMYK 的配置文件配不上 BGRA 像素（图像建不出来），那些由核心转成 sRGB。
        let space = handle.iccProfile().flatMap { CGColorSpace(iccData: $0 as CFData) }
            .flatMap { $0.model == .rgb ? $0 : nil }
            ?? CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
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

/// 正在做的翻页效果：旧画面层、两层的变暗层、开始的时刻。
@MainActor
private final class PageTurn {
    let old: CALayer
    let oldShade: CALayer
    let newShade: CALayer
    let oldSize: CGSize
    let effect: PageEffect
    let forward: Bool
    let duration: Double
    let hold: Double?
    let started = CACurrentMediaTime()
    var timer: Timer?

    init(old: CALayer, oldShade: CALayer, newShade: CALayer, oldSize: CGSize, effect: PageEffect, forward: Bool,
         duration: Double, hold: Double?) {
        self.old = old
        self.oldShade = oldShade
        self.newShade = newShade
        self.oldSize = oldSize
        self.effect = effect
        self.forward = forward
        self.duration = duration
        self.hold = hold
    }

    /// 进度 0–1。
    var progress: Double {
        if let hold {
            return hold
        }
        return min(1, max(0, (CACurrentMediaTime() - started) / max(duration, 0.001)))
    }
}
