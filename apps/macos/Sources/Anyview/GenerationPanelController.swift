import AppKit
import AnyviewCore

/// 生成信息面板：Esc 和关闭按钮一样。
private final class GenerationWindow: NSWindow {
    var onCancel: (() -> Void)?

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }
}

/// 滚动视图里的内容从上往下排。
private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

/// “复制”按钮：按下后把文字放进剪贴板，按钮上的字短暂换成“已复制”。
private final class CopyButton: NSButton {
    private let text: String
    private let normal: String
    private let copied: String

    init(title: String, text: String, copied: String) {
        self.text = text
        self.normal = title
        self.copied = copied
        super.init(frame: .zero)
        self.title = title
        bezelStyle = .push
        controlSize = .small
        target = self
        action = #selector(copyText(_:))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    @objc private func copyText(_ sender: Any?) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        title = copied
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.title = self.normal
            }
        }
    }
}

/// “生成信息”面板：AI 生成的图片里记的提示词、反向提示词、参数，每段都能复制，最后可以复制文件里的原文。
/// 内容由核心读出；开着时跟着打开它的那个看图窗口的当前图片换内容（悬浮界面刷新时调 refresh）：换到还没读好的
/// 图片时先不换（等读好了），界面语言变了也重新填。那个看图窗口关掉时面板跟着关。只读，文字可以选中。
@MainActor
final class GenerationPanelController: NSWindowController, NSWindowDelegate {
    /// 开着的面板（看图窗口刷新悬浮界面时找它）。
    static weak var current: GenerationPanelController?

    private let stack = NSStackView()
    private let inset: CGFloat = 16
    private(set) weak var source: MainWindowController?
    private var lang: Lang
    /// 现在显示的内容；还没填过时为空（填了“没有生成信息”是 .some(nil)）。
    private(set) var shown: GenerationInfo??
    var onClosed: (() -> Void)?

    init(source: MainWindowController, lang: Lang) {
        self.source = source
        self.lang = lang
        let window = GenerationWindow(
            contentRect: NSRect(x: 0, y: 0, width: 520, height: 560),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 360, height: 240)
        super.init(window: window)
        window.delegate = self
        window.onCancel = { [weak self] in
            self?.window?.performClose(nil)
        }
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 14, left: inset, bottom: 14, right: inset)
        stack.translatesAutoresizingMaskIntoConstraints = false
        let document = FlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(stack)
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.documentView = document
        window.contentView = scroll
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: document.topAnchor),
            stack.leadingAnchor.constraint(equalTo: document.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: document.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: document.bottomAnchor),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
        ])
        Self.current = self
        refresh()
        window.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    /// 显示面板（已经开着时放到最前面），内容按这个看图窗口的当前图片。
    func show(from source: MainWindowController, lang: Lang) {
        if self.source !== source || self.lang != lang {
            shown = nil
        }
        self.source = source
        self.lang = lang
        // 看图窗口设了“总在最前面”时面板也一样，不然被它挡在后面：看不见，按键却都到了面板这里。
        window?.level = source.window?.level ?? .normal
        refresh()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    /// 按当前图片重新填内容；内容没变时不动（免得选中的文字被清掉），当前图片还在读时也不动。
    func refresh() {
        guard let source, !source.isClosed else { return }
        let core = source.appCore
        if let level = source.window?.level, window?.level != level {
            window?.level = level
        }
        if core.generationPending() {
            return
        }
        let info = core.generationInfo()
        // 界面语言跟着看图窗口（设置里改了语言时它先换）。
        if source.lang != lang {
            lang = source.lang
            shown = nil
        }
        if let shown, Self.same(shown, info) {
            return
        }
        shown = .some(info)
        let texts = core.generationTexts(lang: lang)
        window?.title = texts.title
        for view in stack.arrangedSubviews {
            stack.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        guard let info else {
            add(label(texts.empty, secondary: true))
            return
        }
        add(label(texts.tool.replacingOccurrences(of: "{tool}", with: info.tool), secondary: true))
        section(texts.prompt, info.prompt, texts)
        if !info.negative.isEmpty {
            section(texts.negative, info.negative, texts)
        }
        if !info.params.isEmpty {
            add(heading(texts.params, button: nil))
            let rows = info.params.map { param -> [NSView] in
                let value = label(param.value, secondary: false)
                value.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
                return [label(param.name, secondary: true), value]
            }
            let grid = NSGridView(views: rows)
            grid.columnSpacing = 16
            grid.rowSpacing = 4
            grid.column(at: 0).xPlacement = .leading
            grid.rowAlignment = .firstBaseline
            add(grid)
        }
        let raw = CopyButton(title: texts.copyRaw, text: info.raw, copied: texts.copied)
        let row = NSStackView(views: [spacer(), raw])
        row.orientation = .horizontal
        add(row, spacingBefore: 10)
    }

    /// 两次的内容一样不一样（按生成工具、提示词和原文认）。
    private static func same(_ a: GenerationInfo?, _ b: GenerationInfo?) -> Bool {
        switch (a, b) {
        case (nil, nil):
            return true
        case let (a?, b?):
            return a.tool == b.tool && a.prompt == b.prompt && a.raw == b.raw
        default:
            return false
        }
    }

    /// 一段文字：标题、右边的“复制”，下面是可以选中的文字。
    private func section(_ title: String, _ text: String, _ texts: GenerationTexts) {
        add(heading(title, button: CopyButton(title: texts.copy, text: text, copied: texts.copied)), spacingBefore: 6)
        add(label(text, secondary: false))
    }

    private func heading(_ title: String, button: NSButton?) -> NSView {
        let label = NSTextField(labelWithString: title)
        label.font = NSFont.boldSystemFont(ofSize: 13)
        var views: [NSView] = [label, spacer()]
        if let button {
            views.append(button)
        }
        let row = NSStackView(views: views)
        row.orientation = .horizontal
        return row
    }

    private func label(_ text: String, secondary: Bool) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text)
        label.isSelectable = true
        label.font = NSFont.systemFont(ofSize: 13)
        label.textColor = secondary ? .secondaryLabelColor : .labelColor
        return label
    }

    private func spacer() -> NSView {
        let view = NSView()
        view.setContentHuggingPriority(.init(1), for: .horizontal)
        return view
    }

    /// 加一行，宽度和面板的内容一样（文字按这个宽度折行）。
    private func add(_ view: NSView, spacingBefore: CGFloat? = nil) {
        if let spacingBefore, let last = stack.arrangedSubviews.last {
            stack.setCustomSpacing(spacingBefore + stack.spacing, after: last)
        }
        stack.addArrangedSubview(view)
        view.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -2 * inset).isActive = true
    }

    func windowWillClose(_ notification: Notification) {
        if Self.current === self {
            Self.current = nil
        }
        onClosed?()
    }

    /// 自检记录：开着没有、显示的生成工具和提示词，开着时截一张面板的图。
    func report(screenshot url: URL) -> [String: Any] {
        var result: [String: Any] = ["open": window?.isVisible ?? false]
        if let info = shown ?? nil {
            result["tool"] = info.tool
            result["prompt"] = info.prompt
        }
        if let window, window.isVisible {
            result["screenshot"] = windowSnapshot(window, to: url)
        }
        return result
    }
}
