import AnyviewCore
import AppKit

// 控制栏的液态玻璃层（macOS 26+）：控制栏的底、指针移到按钮上时的效果（液面鼓起、玻璃水珠、
// 淡色亮底，设置 › 浏览 › 悬停效果）、进度条的轨道和滑块，都由这里的 SwiftUI 玻璃层画，铺在控制栏的按钮下面；
// 按钮、右键翻 10 张、提示、自检仍是 AppKit 的控件（Overlays.swift），它们把悬停、按下、进度条的状态告诉这里。
// 用苹果的 SwiftUI 玻璃接口（glassEffect、GlassEffectContainer、glassEffectID）：鼓包要和控制栏的玻璃融成一体，
// 控制栏本身也得是 SwiftUI 的玻璃。
// 状态放在 @Observable 的模型里，不用 @State：macOS 27 的开发包里 @State 是宏，宏的实现插件只随完整的 Xcode
// 提供，只装命令行工具的 Mac 上编不了。

/// 控制栏的容器（玻璃层在下、按钮在上）：排好以后通知一声，好让玻璃层按按钮、进度条的新位置画。
final class GlassBarContainer: NSView {
    var onLayout: (() -> Void)?

    override func layout() {
        super.layout()
        onLayout?()
    }
}

#if compiler(>=6.2)
import SwiftUI

/// 玻璃层要画的东西。坐标都是玻璃层自己的（原点在左上）。
@available(macOS 26.0, *)
@MainActor
@Observable
final class BarGlassModel {
    /// 按钮的图标：SF Symbols 的名字和字号；“锁定”是文字加小圆点（锁定时是强调色）。
    enum Glyph: Equatable {
        case symbol(String, CGFloat)
        case text(String, accent: Bool)
    }

    /// 指针下的按钮：位置、是不是两边的大按钮、图标。
    struct Hover: Equatable {
        var frame: CGRect
        var big: Bool
        var glyph: Glyph
    }

    /// 进度条：轨道（一条线的位置和宽度）、滑块的位置（0–1）、指针在上面、按住拖动、拖得快时的拉长。
    struct Strip: Equatable {
        var track: CGRect
        var fraction: Double?
        var hovering: Bool
        var dragging: Bool
        var stretch: CGFloat
    }

    var effect: BarHoverEffect = .bulge
    var hover: Hover?
    var pressed = false
    var strip: Strip?
}

@available(macOS 26.0, *)
struct BarGlassView: View {
    let model: BarGlassModel
    @Namespace private var space
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    /// 窗口在前台（是活动窗口）。不在前台时系统不把同一容器里的玻璃融在一起，鼓包会成一个带深色边的圆，
    /// 这时只让图标浮起，不画鼓包。
    @Environment(\.appearsActive) private var appearsActive

    /// 液面鼓起效果参数（鼓包直径 55 pt、图标浮起 2 pt），按 56 pt 控制栏高度换算偏移：
    /// 直径、位置、浮起高度都按控制栏的高度等比换算，样子和演示程序里一样——大按钮的圆，上沿在控制栏上沿以内
    /// 1.5 点（不冒出来，贴着上沿和控制栏融在一起）；下面一排的鼓包，下沿正好贴着控制栏的下沿。
    private let demoBarHeight: CGFloat = 62
    private let bulgeDiameter: CGFloat = 55
    private let bulgeLift: CGFloat = 2
    private let bulgeTopInset: CGFloat = 1.5
    private let bulgeBottomOverhang: CGFloat = 0.15

    var body: some View {
        GeometryReader { geometry in
            ZStack {
                background(height: geometry.size.height)
                stripLayer
                if model.effect == .droplet {
                    lens
                }
                liftedGlyph(height: geometry.size.height)
            }
            .frame(width: geometry.size.width, height: geometry.size.height)
        }
        .allowsHitTesting(false)
        // 只是画出来的样子：按钮、进度条本身是下面 AppKit 的控件，旁白不要在这里再读到一遍。
        .accessibilityHidden(true)
    }

    // MARK: 控制栏的玻璃和悬停效果

    private var barGlass: some View {
        Capsule()
            .fill(.clear)
            .glassEffect(.regular, in: Capsule())
    }

    /// 控制栏的玻璃；液面鼓起时在同一个玻璃容器里加一个鼓包（和控制栏融在一起）；淡色亮底时加一块淡色的底。
    @ViewBuilder
    private func background(height: CGFloat) -> some View {
        switch model.effect {
        case .bulge:
            GlassEffectContainer(spacing: 18) {
                ZStack {
                    barGlass
                    if let hover = model.hover, appearsActive {
                        let size = bulgeSize(hover, barHeight: height)
                        Capsule()
                            .fill(.clear)
                            .frame(width: size.width, height: size.height)
                            .glassEffect(.regular, in: Capsule())
                            .glassEffectID("bulge", in: space)
                            .scaleEffect(model.pressed ? 0.94 : 1)
                            .position(x: hover.frame.midX, y: bulgeCenter(hover, size: size, barHeight: height))
                    }
                }
            }
        case .droplet:
            barGlass
        case .highlight:
            ZStack {
                barGlass
                if let hover = model.hover {
                    let size = platterSize(hover)
                    Capsule()
                        .fill(Color.primary.opacity(model.pressed ? 0.24 : 0.13))
                        .frame(width: size.width, height: size.height)
                        .position(x: hover.frame.midX, y: hover.frame.midY)
                        .transition(.opacity)
                }
            }
        }
    }

    /// 玻璃水珠：盖在按钮上面的一颗透明玻璃（自己一个容器，不和控制栏融在一起），图标放大了画在水珠里。
    private var lens: some View {
        GlassEffectContainer {
            ZStack {
                if let hover = model.hover {
                    let size = lensSize(hover)
                    glyph(hover.glyph)
                        .scaleEffect(1.3)
                        .frame(width: size.width, height: size.height)
                        .glassEffect(.clear, in: Capsule())
                        .glassEffectID("lens", in: space)
                        .scaleEffect(model.pressed ? 0.88 : 1)
                        .position(x: hover.frame.midX, y: hover.frame.midY)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    /// 液面鼓起：图标跟着鼓包浮起一点、略放大（按钮自己的图标这时藏起来）。
    @ViewBuilder
    private func liftedGlyph(height: CGFloat) -> some View {
        if model.effect == .bulge, let hover = model.hover {
            glyph(hover.glyph)
                .scaleEffect(model.pressed ? 0.97 : 1.08)
                .position(x: hover.frame.midX, y: hover.frame.midY + lift(hover, barHeight: height))
        }
    }

    @ViewBuilder
    private func glyph(_ glyph: BarGlassModel.Glyph) -> some View {
        switch glyph {
        case let .symbol(name, size):
            Image(systemName: name)
                .font(.system(size: size, weight: .medium))
                .foregroundStyle(.primary)
        case let .text(title, accent):
            HStack(spacing: 2) {
                Text(title)
                    .font(.system(size: 11))
                Image(systemName: "circle.fill")
                    .font(.system(size: 8))
            }
            .foregroundStyle(accent ? Color.accentColor : Color.primary)
        }
    }

    /// 按控制栏的高度把演示程序里的数换算过来。
    private func scale(_ barHeight: CGFloat) -> CGFloat {
        barHeight > 0 ? barHeight / demoBarHeight : 1
    }

    /// 图标浮起多少：大按钮往上，下面一排往下（0.7 倍）；减弱动态效果时不浮起。
    private func lift(_ hover: BarGlassModel.Hover, barHeight: CGFloat) -> CGFloat {
        guard !reduceMotion else { return 0 }
        let lift = bulgeLift * scale(barHeight)
        return hover.big ? -lift : 0.7 * lift
    }

    /// 鼓包中心的高度（玻璃层的坐标，原点在上）：大按钮贴着上沿，下面一排贴着下沿。
    private func bulgeCenter(_ hover: BarGlassModel.Hover, size: CGSize, barHeight: CGFloat) -> CGFloat {
        let s = scale(barHeight)
        return hover.big
            ? bulgeTopInset * s + size.height / 2
            : barHeight + bulgeBottomOverhang * s - size.height / 2
    }

    /// 鼓包：大按钮是圆；下面一排是同样比例的扁胶囊（演示程序里直径 46 时 32×28）。
    private func bulgeSize(_ hover: BarGlassModel.Hover, barHeight: CGFloat) -> CGSize {
        let diameter = bulgeDiameter * scale(barHeight)
        if hover.big {
            return CGSize(width: diameter, height: diameter)
        }
        let ratio = diameter / 46
        return CGSize(width: max(hover.frame.width + 14, 32) * ratio, height: 28 * ratio)
    }

    private func lensSize(_ hover: BarGlassModel.Hover) -> CGSize {
        hover.big ? CGSize(width: 50, height: 50) : CGSize(width: max(hover.frame.width + 16, 34), height: 30)
    }

    /// 淡色亮底：大按钮是比按钮大一圈的圆（和以前一样大），别的是大一圈的胶囊。
    private func platterSize(_ hover: BarGlassModel.Hover) -> CGSize {
        hover.big
            ? CGSize(width: hover.frame.width + 12, height: hover.frame.width + 12)
            : CGSize(width: max(hover.frame.width + 12, 26), height: max(hover.frame.height + 6, 24))
    }

    // MARK: 进度条

    /// 轨道和滑块。液面鼓起、玻璃水珠时，指针移上去、按住拖动时滑块变成一颗透明的玻璃，拖得快会被拉长。
    /// 指针移上去、按下、松开时整个有动画；拉长只给滑块的大小加动画（写在 position 前面），位置不加：拖动时滑块
    /// 跟着指针走，不弹。
    @ViewBuilder
    private var stripLayer: some View {
        if let strip = model.strip, strip.track.width > 0 {
            let active = strip.hovering || strip.dragging
            ZStack {
                Capsule()
                    .fill(Color.primary.opacity(0.25))
                    .frame(width: strip.track.width, height: active ? 5 : 4)
                    .position(x: strip.track.midX, y: strip.track.midY)
                if let fraction = strip.fraction {
                    let x = strip.track.minX + strip.track.width * CGFloat(fraction)
                    if model.effect != .highlight && active {
                        Capsule()
                            .fill(.clear)
                            .frame(width: (strip.dragging ? 34 : 24) + strip.stretch, height: strip.dragging ? 22 : 18)
                            .glassEffect(.clear, in: Capsule())
                            .animation(stripSpring, value: strip.stretch)
                            .position(x: x, y: strip.track.midY)
                            .transition(.scale(scale: 0.5).combined(with: .opacity))
                    } else {
                        Circle()
                            .fill(Color.primary)
                            .frame(width: active ? 15 : 12, height: active ? 15 : 12)
                            .position(x: x, y: strip.track.midY)
                            .transition(.scale(scale: 0.6).combined(with: .opacity))
                    }
                }
            }
            .animation(stripSpring, value: strip.hovering)
            .animation(stripSpring, value: strip.dragging)
        }
    }

    private var stripSpring: Animation {
        .spring(response: 0.25, dampingFraction: 0.65)
    }
}

/// 不接鼠标的 NSHostingView：鼠标交给上面的按钮。
@available(macOS 26.0, *)
final class PassthroughHostingView<Content: View>: NSHostingView<Content> {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// 控制栏的玻璃层和按钮之间的连接：按钮的悬停、按下 → 模型（用 SwiftUI 的动画过渡）；进度条要画的 → 模型。
/// 玻璃水珠、液面鼓起时，指针下按钮自己的图标藏起来，由玻璃层画（水珠里放大、鼓包上浮起）。
@available(macOS 26.0, *)
@MainActor
final class BarGlass {
    let model = BarGlassModel()
    let view: NSView
    private var glyphs: [ObjectIdentifier: () -> BarGlassModel.Glyph] = [:]
    private weak var current: PageButton?
    /// 指针停在上面的按钮（不管能不能按）：它变得能按时补上效果。
    private weak var pointer: PageButton?
    private var hideSerial = 0

    init() {
        let hosting = PassthroughHostingView(rootView: BarGlassView(model: model))
        hosting.sizingOptions = []
        hosting.translatesAutoresizingMaskIntoConstraints = false
        view = hosting
    }

    /// 在按钮之间移动用的弹簧：稍微冲过头再停住，像液体；减弱动态效果时只淡入淡出。
    private var motion: Animation {
        NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            ? .easeOut(duration: 0.15)
            : .spring(response: 0.38, dampingFraction: 0.68)
    }

    /// 让这个按钮有悬停、按下反馈；glyph 给出它现在的图标。
    func attach(_ button: PageButton, glyph: @escaping () -> BarGlassModel.Glyph) {
        glyphs[ObjectIdentifier(button)] = glyph
        button.onHover = { [weak self, weak button] inside in
            guard let self, let button else { return }
            if inside {
                self.pointer = button
                self.show(button)
            } else {
                if self.pointer === button {
                    self.pointer = nil
                }
                self.leave(button)
            }
        }
        button.onPress = { [weak self] down in
            self?.press(down)
        }
    }

    func setEffect(_ effect: BarHoverEffect) {
        guard effect != model.effect else { return }
        reset()
        model.effect = effect
    }

    func show(_ button: PageButton) {
        guard usable(button), let hover = hover(for: button) else { return }
        hideSerial += 1
        let previous = current
        current = button
        withAnimation(motion) {
            model.hover = hover
        }
        if previous !== button {
            setIconHidden(previous, false)
        }
        setIconHidden(button, model.effect != .highlight)
    }

    /// 指针离开按钮：稍等一下再收。移到旁边的按钮时先收到这里的“移出”、再收到那边的“移进”，直接流过去。
    private func leave(_ button: PageButton) {
        guard current === button else { return }
        hideSerial += 1
        let serial = hideSerial
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.hideSerial == serial else { return }
                self.fadeOut()
            }
        }
    }

    /// 收起效果（淡出）。
    private func fadeOut() {
        hideSerial += 1
        let button = current
        current = nil
        withAnimation(.easeOut(duration: 0.22)) {
            model.hover = nil
            model.pressed = false
        }
        setIconHidden(button, false)
    }

    private func press(_ down: Bool) {
        guard current != nil else { return }
        withAnimation(down ? .spring(response: 0.18, dampingFraction: 0.6) : motion) {
            model.pressed = down
        }
    }

    /// 马上收起（控制栏隐藏、换效果时）。指针要重新移进按钮才再有效果。
    func reset() {
        hideSerial += 1
        setIconHidden(current, false)
        current = nil
        pointer = nil
        model.hover = nil
        model.pressed = false
    }

    /// 控制栏重新排列后、按钮的图标或能不能按变了以后（锁定、图片打开了、没有下一张了）：按新的位置和图标画，
    /// 不加动画。指针一直停着时不会再有移进、移出：它下面的按钮变得不能按就收起，变得能按就补上。
    func refresh() {
        if let current, !usable(current) {
            fadeOut()
        }
        guard let current else {
            // 指针还在不在它上面要现看：窗口关掉、最小化、控制栏藏起来以后指针早走了，收不到“移出”。
            if let pointer {
                if !pointerInside(pointer) {
                    self.pointer = nil
                } else if usable(pointer) {
                    show(pointer)
                }
            }
            return
        }
        guard let hover = hover(for: current) else { return }
        if model.hover != hover {
            model.hover = hover
        }
    }

    private func usable(_ button: PageButton) -> Bool {
        button.isEnabled && !button.isHiddenOrHasHiddenAncestor
    }

    /// 指针现在在不在这个按钮上（程序不在前台、窗口看不见时算不在：按钮的悬停也只在程序在前台时有）。
    private func pointerInside(_ button: PageButton) -> Bool {
        guard let window = button.window, window.isVisible, !window.isMiniaturized, NSApp.isActive else {
            return false
        }
        let point = button.convert(window.mouseLocationOutsideOfEventStream, from: nil)
        return button.bounds.contains(point)
    }

    /// 进度条要画的（坐标换成玻璃层的）。动画由玻璃层按哪一项变了自己加（[`BarGlassView`] 的 stripLayer）：
    /// 指针移上去、按下、松开、拉长有动画；拖动时滑块跟着指针走，不加动画。
    func update(strip visual: ProgressStrip.Visual, in stripView: NSView) {
        let next = BarGlassModel.Strip(
            track: view.convert(visual.track, from: stripView),
            fraction: visual.fraction,
            hovering: visual.hovering,
            dragging: visual.dragging,
            stretch: visual.stretch
        )
        if next != model.strip {
            model.strip = next
        }
    }

    /// 按钮在玻璃层里的位置：用对齐框（看到的那块，40×40 的圆按钮），不用 frame（下面多出对齐留白，是 40×46）。
    private func hover(for button: PageButton) -> BarGlassModel.Hover? {
        guard let glyph = glyphs[ObjectIdentifier(button)] else { return nil }
        let frame: CGRect
        if let superview = button.superview {
            frame = view.convert(button.alignmentRect(forFrame: button.frame), from: superview)
        } else {
            frame = view.convert(button.bounds, from: button)
        }
        let big = abs(frame.width - frame.height) < 2 && frame.width >= 30
        return BarGlassModel.Hover(frame: frame, big: big, glyph: glyph())
    }

    /// 玻璃水珠、液面鼓起时，指针下按钮自己的图标藏起来（玻璃层画一个）；按钮照样接鼠标。
    private func setIconHidden(_ button: PageButton?, _ hidden: Bool) {
        guard let button else { return }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.12
            button.animator().alphaValue = hidden ? 0 : 1
        }
    }
}
#endif
