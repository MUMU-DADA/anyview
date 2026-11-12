import AppKit
import UniformTypeIdentifiers
import AnyviewCore

/// "确定"生效后的结果。
struct SettingsAppliedInfo {
    let changed: [String]
    let saveError: String?
}

/// 0xRRGGBB → sRGB 颜色。
func colorFromRGB(_ rgb: UInt32) -> NSColor {
    NSColor(
        srgbRed: CGFloat((rgb >> 16) & 0xFF) / 255,
        green: CGFloat((rgb >> 8) & 0xFF) / 255,
        blue: CGFloat(rgb & 0xFF) / 255,
        alpha: 1
    )
}

/// 圆形色块（带细边，浅色在浅色背景上也看得出）；是当前颜色时外面画一圈强调色。
/// 画的时候才取颜色，跟着窗口的外观走。
private func swatchImage(_ rgb: UInt32, selected: Bool) -> NSImage {
    NSImage(size: NSSize(width: 22, height: 22), flipped: false) { rect in
        let circle = NSBezierPath(ovalIn: rect.insetBy(dx: 4, dy: 4))
        colorFromRGB(rgb).setFill()
        circle.fill()
        NSColor(white: 0.5, alpha: 0.45).setStroke()
        circle.lineWidth = 1
        circle.stroke()
        if selected {
            let ring = NSBezierPath(ovalIn: rect.insetBy(dx: 1, dy: 1))
            ring.lineWidth = 2
            NSColor.controlAccentColor.setStroke()
            ring.stroke()
        }
        return true
    }
}

/// 原点在左上的视图：放进滚动视图后内容从顶部排起。
private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

/// 能换行的标签（中文按字换行）：换行的宽度跟着实际分到的宽度走，宽度变了高度随之重算。
private final class WrappingLabel: NSTextField {
    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        if newSize.width > 0, abs(preferredMaxLayoutWidth - newSize.width) > 0.5 {
            preferredMaxLayoutWidth = newSize.width
            invalidateIntrinsicContentSize()
        }
    }
}

/// 设置项的卡片：圆角，底色和边框从文字颜色调出来，浅色、深色外观都能用。
private final class CardView: NSView {
    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.cornerRadius = 10
        layer?.borderWidth = 1
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = NSColor.labelColor.withAlphaComponent(0.045).cgColor
            layer?.borderColor = NSColor.labelColor.withAlphaComponent(0.08).cgColor
        }
    }
}

/// 一像素的分隔线（卡片里两行之间、侧栏右边、底部按钮上面）。
private final class HairlineView: NSView {
    init(vertical: Bool) {
        super.init(frame: .zero)
        wantsLayer = true
        translatesAutoresizingMaskIntoConstraints = false
        if vertical {
            widthAnchor.constraint(equalToConstant: 1).isActive = true
        } else {
            heightAnchor.constraint(equalToConstant: 1).isActive = true
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = NSColor.labelColor.withAlphaComponent(0.09).cgColor
        }
    }
}

/// 设置窗口：Esc 和关闭按钮一样（有没确定的修改就先询问）。
private final class SettingsWindow: NSWindow {
    var onCancel: (() -> Void)?

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }

    /// 正在录快捷键时按键先交给按键框：回车不能被默认按钮"确定"接走，⌘W 等也不关窗口。
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if let recorder = firstResponder as? KeyRecorderButton, recorder.capturing {
            return recorder.performKeyEquivalent(with: event)
        }
        return super.performKeyEquivalent(with: event)
    }
}

/// 快捷键的按键框：点一下后按下组合键即录入，⌘ 组合、Esc、Tab 也能录入。
private final class KeyRecorderButton: NSButton {
    var onRecord: ((String, KeyModifiers) -> Void)?
    var promptText = ""
    var keyLabel = "" {
        didSet {
            if !capturing {
                title = keyLabel
            }
        }
    }
    private(set) var capturing = false

    override var acceptsFirstResponder: Bool { isEnabled }

    override func mouseDown(with event: NSEvent) {
        startCapture()
    }

    /// 等着按键（点了按键框，或刚添加了这一行）。
    func startCapture() {
        guard isEnabled else { return }
        capturing = true
        title = promptText
        window?.makeFirstResponder(self)
    }

    override func keyDown(with event: NSEvent) {
        guard capturing else {
            super.keyDown(with: event)
            return
        }
        record(event)
    }

    /// ⌘ 组合先作为快捷键送给窗口里的各个视图：正在录入时由这里接住，不交给菜单栏。
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard capturing, window?.firstResponder === self, event.type == .keyDown else {
            return super.performKeyEquivalent(with: event)
        }
        record(event)
        return true
    }

    private func record(_ event: NSEvent) {
        // 只按了修饰键等认不出的键：继续等。
        guard let name = KeyNames.name(of: event) else { return }
        capturing = false
        title = keyLabel
        onRecord?(name, KeyNames.modifiers(of: event))
    }

    override func resignFirstResponder() -> Bool {
        if capturing {
            capturing = false
            title = keyLabel
        }
        return super.resignFirstResponder()
    }
}

/// 自己设的快捷键的一行：按键框、旁边的小按钮（列出不方便按出的键）、动作下拉框和提示、开关、删除按钮。
@MainActor
private final class KeyRowControls {
    /// 映射表里的行号。
    let row: Int
    let container: NSStackView
    let toggle: NSSwitch
    let key: KeyRecorderButton
    let more: NSPopUpButton
    let popup: NSPopUpButton
    let note: NSTextField

    init(row: Int, container: NSStackView, toggle: NSSwitch, key: KeyRecorderButton, more: NSPopUpButton,
         popup: NSPopUpButton, note: NSTextField) {
        self.row = row
        self.container = container
        self.toggle = toggle
        self.key = key
        self.more = more
        self.popup = popup
        self.note = note
    }
}

/// 一项在窗口里的控件。
@MainActor
private final class SettingsRow {
    let key: String
    let view: NSView
    /// 开关是翻过来说的（只给自检把存的值换成开关的状态）。
    let inverted: Bool
    /// 灰显时一起灰显的控件。
    var controls: [NSControl] = []
    /// 左边的文字（灰显时变淡）。
    var texts: [NSView] = []
    var toggle: NSSwitch?
    var popup: NSPopUpButton?
    var optionIDs: [String] = []
    var field: NSTextField?
    /// 背景颜色：各个色块和它的颜色、当前颜色。
    var swatches: [(NSButton, UInt32)] = []
    var isColor = false
    var color: UInt32 = 0
    /// 鼠标映射的下拉框。
    var mappingPopup: NSPopUpButton?
    /// 自己设的快捷键：列出来的各行，"添加快捷键"用的空行。
    var keyRows: [KeyRowControls] = []
    var addRow: Int?

    init(key: String, view: NSView, inverted: Bool) {
        self.key = key
        self.view = view
        self.inverted = inverted
    }

    func setEnabled(_ enabled: Bool) {
        for control in controls {
            control.isEnabled = enabled
        }
        for text in texts {
            text.alphaValue = enabled ? 1 : 0.45
        }
    }
}

/// 设置窗口（docs/SETTINGS.md"设置窗口"）。
///  重新设计：左边是带图标的分类列表（系统的侧栏材质）；右边是页的大标题，下面按核心给的分组
/// 排成几张圆角卡片，每行左边是名字（后面可以跟着灰色的快捷键，下面可以有一行灰色说明），右边是开关、下拉框等；
/// 底部左边"恢复本页默认"，右边"取消""确定"。所有修改只改核心里的草稿，点"确定"才校验、生效、保存；
/// 关闭窗口（×、Esc、⌘W）时有没确定的修改就先询问，"取消"直接放弃。哪些项能改、改了以后哪些项
/// 跟着灰显、开关是不是翻过来说的，都由核心决定（每次修改后重新取这一页的数据）。
@MainActor
final class SettingsWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource,
    NSTableViewDelegate, NSTextFieldDelegate {
    private static let keyMappingsKey = "keyboard.mappings"
    /// 页面内容左右的空白。
    private static let pageInset: CGFloat = 32
    /// 左边分类栏的宽度。
    private static let sidebarWidth: CGFloat = 196

    private let core: AppCore
    private let lang: Lang
    private let pages: [SettingsPageInfo]
    private let sidebar = NSTableView()
    private let pageStack = NSStackView()
    private let pageScroll = NSScrollView()
    private(set) var currentPage = ""
    private var rows: [SettingsRow] = []
    /// 程序自己在改控件（不是用户操作），这时不把变化写进草稿。
    private var updating = false
    /// 已经确定或放弃，窗口正在关闭：不再询问。
    private var closing = false
    /// 取色器正在为哪一项选颜色。
    private var colorKey: String?
    /// "确定"生效后通知应用（刷新界面、换语言）。
    var onApplied: ((SettingsAppliedInfo) -> Void)?
    var onClosed: (() -> Void)?
    /// 自检时为真：校验失败不弹提示框（只记下来）。
    var quiet = false
    /// 最近一次"确定"校验失败的提示。
    private(set) var lastError: String?
    /// 按键框旁小按钮列出的按键（首项为"清除"）。
    private lazy var specialKeys: [SettingsOption] = core.settingsSpecialKeys(lang: lang)
    /// 已经安排了重建这一页（快捷键多了或少了一行）。
    private var rebuildPending = false
    /// 刚"添加"的快捷键行：重建这一页以后让它的按键框等着按键。
    private var captureAfterRebuild: Int?

    init(core: AppCore, lang: Lang) {
        self.core = core
        self.lang = lang
        core.settingsBegin()
        pages = core.settingsPages(lang: lang)
        let window = SettingsWindow(
            contentRect: NSRect(x: 0, y: 0, width: 860, height: 680),
            styleMask: [.titled, .closable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        // 侧栏伸到标题栏下面（与系统设置相同），标题文字不显示（窗口菜单里照常有）。
        window.title = uiText(key: .settingsTitle, lang: lang)
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.delegate = self
        window.onCancel = { [weak self] in
            self?.window?.performClose(nil)
        }
        buildLayout(in: window)
        window.center()
        if let first = pages.first {
            selectPage(first.id)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    private func text(_ key: TextKey) -> String {
        uiText(key: key, lang: lang)
    }

    // MARK: 布局

    private func buildLayout(in window: NSWindow) {
        // 左边：系统的侧边栏（分栏控制器的侧栏项）：macOS 26 起是浮在窗口里的液态玻璃侧栏，
        // 更早的系统是侧栏材质，都由系统画（以前是自己铺的侧栏材质加一条分隔线）。里面是带图标的分类列表。
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("page"))
        column.resizingMask = .autoresizingMask
        sidebar.addTableColumn(column)
        sidebar.headerView = nil
        sidebar.style = .sourceList
        sidebar.backgroundColor = .clear
        // 不设成自定义，源列表的行高按系统的大小走，下面的 30 不起作用。
        sidebar.rowSizeStyle = .custom
        sidebar.rowHeight = 30
        sidebar.dataSource = self
        sidebar.delegate = self
        sidebar.reloadData()
        let sidebarScroll = NSScrollView()
        sidebarScroll.documentView = sidebar
        sidebarScroll.hasVerticalScroller = false
        sidebarScroll.drawsBackground = false
        sidebarScroll.borderType = .noBorder
        sidebarScroll.automaticallyAdjustsContentInsets = false
        sidebarScroll.translatesAutoresizingMaskIntoConstraints = false
        let sidebarView = NSView()
        sidebarView.addSubview(sidebarScroll)
        let sidebarController = NSViewController()
        sidebarController.view = sidebarView

        // 右边：可以上下滚动的页，下面是按钮。
        pageStack.orientation = .vertical
        pageStack.alignment = .leading
        pageStack.spacing = 0
        pageStack.edgeInsets = NSEdgeInsets(top: 6, left: Self.pageInset, bottom: 24, right: Self.pageInset)
        pageStack.translatesAutoresizingMaskIntoConstraints = false
        let document = FlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(pageStack)
        pageScroll.documentView = document
        pageScroll.hasVerticalScroller = true
        pageScroll.autohidesScrollers = true
        pageScroll.drawsBackground = false
        pageScroll.borderType = .noBorder
        pageScroll.automaticallyAdjustsContentInsets = false
        pageScroll.translatesAutoresizingMaskIntoConstraints = false

        let footer = NSView()
        footer.translatesAutoresizingMaskIntoConstraints = false
        let footerLine = HairlineView(vertical: false)
        let reset = NSButton(title: text(.settingsReset), target: self, action: #selector(resetPressed(_:)))
        reset.isBordered = false
        reset.attributedTitle = NSAttributedString(
            string: text(.settingsReset),
            attributes: [.foregroundColor: NSColor.linkColor, .font: NSFont.systemFont(ofSize: 13)]
        )
        let cancel = NSButton(title: text(.buttonCancel), target: self, action: #selector(cancelPressed(_:)))
        let ok = NSButton(title: text(.settingsOk), target: self, action: #selector(okPressed(_:)))
        // 回车是"确定"；Esc 不给"取消"：Esc 和关闭按钮一样，有没确定的修改时先问。
        ok.keyEquivalent = "\r"
        for button in [cancel, ok] {
            button.bezelStyle = .rounded
            button.translatesAutoresizingMaskIntoConstraints = false
            button.widthAnchor.constraint(greaterThanOrEqualToConstant: 84).isActive = true
        }
        reset.translatesAutoresizingMaskIntoConstraints = false
        for view in [footerLine, reset, cancel, ok] as [NSView] {
            footer.addSubview(view)
        }

        let mainView = NSView()
        for view in [pageScroll, footer] as [NSView] {
            mainView.addSubview(view)
        }
        let mainController = NSViewController()
        mainController.view = mainView

        // 侧栏宽度固定、不能收起；窗口大小不变（860×680）。
        let split = NSSplitViewController()
        split.splitView.dividerStyle = .thin
        let sidebarItem = NSSplitViewItem(sidebarWithViewController: sidebarController)
        sidebarItem.canCollapse = false
        sidebarItem.minimumThickness = Self.sidebarWidth
        sidebarItem.maximumThickness = Self.sidebarWidth
        split.addSplitViewItem(sidebarItem)
        split.addSplitViewItem(NSSplitViewItem(viewController: mainController))
        let size = window.contentRect(forFrameRect: window.frame).size
        split.view.frame = NSRect(origin: .zero, size: size)
        window.contentViewController = split
        window.setContentSize(size)

        NSLayoutConstraint.activate([
            // 分类列表从窗口的三个按钮下面开始；页从标题栏下面开始。
            sidebarScroll.leadingAnchor.constraint(equalTo: sidebarView.leadingAnchor, constant: 10),
            sidebarScroll.trailingAnchor.constraint(equalTo: sidebarView.trailingAnchor, constant: -10),
            sidebarScroll.topAnchor.constraint(equalTo: sidebarView.safeAreaLayoutGuide.topAnchor, constant: 14),
            sidebarScroll.bottomAnchor.constraint(equalTo: sidebarView.bottomAnchor, constant: -12),
            pageScroll.leadingAnchor.constraint(equalTo: mainView.leadingAnchor),
            pageScroll.trailingAnchor.constraint(equalTo: mainView.trailingAnchor),
            pageScroll.topAnchor.constraint(equalTo: mainView.safeAreaLayoutGuide.topAnchor),
            pageScroll.bottomAnchor.constraint(equalTo: footer.topAnchor),
            footer.leadingAnchor.constraint(equalTo: mainView.leadingAnchor),
            footer.trailingAnchor.constraint(equalTo: mainView.trailingAnchor),
            footer.bottomAnchor.constraint(equalTo: mainView.bottomAnchor),
            footer.heightAnchor.constraint(equalToConstant: 56),
            footerLine.leadingAnchor.constraint(equalTo: footer.leadingAnchor),
            footerLine.trailingAnchor.constraint(equalTo: footer.trailingAnchor),
            footerLine.topAnchor.constraint(equalTo: footer.topAnchor),
            reset.leadingAnchor.constraint(equalTo: footer.leadingAnchor, constant: 26),
            reset.centerYAnchor.constraint(equalTo: footer.centerYAnchor),
            ok.trailingAnchor.constraint(equalTo: footer.trailingAnchor, constant: -20),
            ok.centerYAnchor.constraint(equalTo: footer.centerYAnchor),
            cancel.trailingAnchor.constraint(equalTo: ok.leadingAnchor, constant: -10),
            cancel.centerYAnchor.constraint(equalTo: footer.centerYAnchor),
            // 滚动内容：宽度跟着可见区域，高度由各项撑开，从顶部排起。
            document.leadingAnchor.constraint(equalTo: pageScroll.contentView.leadingAnchor),
            document.topAnchor.constraint(equalTo: pageScroll.contentView.topAnchor),
            document.widthAnchor.constraint(equalTo: pageScroll.contentView.widthAnchor),
            pageStack.leadingAnchor.constraint(equalTo: document.leadingAnchor),
            pageStack.trailingAnchor.constraint(equalTo: document.trailingAnchor),
            pageStack.topAnchor.constraint(equalTo: document.topAnchor),
            pageStack.bottomAnchor.constraint(equalTo: document.bottomAnchor),
        ])
    }

    // MARK: 左侧列表

    /// 左侧列表里各页的图标（SF Symbols；系统没有某个符号时用后面的）。
    private static func pageIcon(_ id: String) -> NSImage? {
        let names: [String]
        switch id {
        case "general": names = ["gearshape"]
        case "browse": names = ["eye"]
        case "image": names = ["photo"]
        case "formats": names = ["doc.badge.gearshape", "doc"]
        case "keyboard": names = ["keyboard"]
        case "mouse": names = ["computermouse", "cursorarrow"]
        case "bookmarks": names = ["bookmark"]
        case "context_menu": names = ["contextualmenu.and.cursorarrow", "list.bullet"]
        case "misc": names = ["square.grid.2x2"]
        case "language": names = ["globe"]
        default: names = ["gearshape"]
        }
        for name in names {
            if let image = NSImage(systemSymbolName: name, accessibilityDescription: nil) {
                return image
            }
        }
        return nil
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        pages.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = NSTableCellView()
        let icon = NSImageView()
        icon.image = Self.pageIcon(pages[row].id)
        icon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 14, weight: .regular)
        icon.translatesAutoresizingMaskIntoConstraints = false
        let label = NSTextField(labelWithString: pages[row].title)
        label.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(icon)
        cell.addSubview(label)
        cell.imageView = icon
        cell.textField = label
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
            icon.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 20),
            label.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 8),
            label.trailingAnchor.constraint(lessThanOrEqualTo: cell.trailingAnchor, constant: -4),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        let row = sidebar.selectedRow
        guard row >= 0, row < pages.count, pages[row].id != currentPage else { return }
        showPage(pages[row].id)
    }

    /// 切到一页（左侧列表跟着选中；选中的变化由 tableViewSelectionDidChange 换页）。
    func selectPage(_ id: String) {
        guard let index = pages.firstIndex(where: { $0.id == id }) else { return }
        if sidebar.selectedRow != index {
            sidebar.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        }
        if currentPage != id {
            showPage(id)
        }
    }

    // MARK: 一页的内容

    /// 一段固定高度的空白（页里的各段之间）。
    private func spacer(_ height: CGFloat) -> NSView {
        let view = NSView()
        view.translatesAutoresizingMaskIntoConstraints = false
        view.heightAnchor.constraint(equalToConstant: height).isActive = true
        view.widthAnchor.constraint(equalToConstant: 1).isActive = true
        return view
    }

    /// 加进页里，宽度占满（减去左右的空白）。
    private func addFullWidth(_ view: NSView) {
        view.translatesAutoresizingMaskIntoConstraints = false
        pageStack.addArrangedSubview(view)
        view.widthAnchor.constraint(equalTo: pageStack.widthAnchor, constant: -2 * Self.pageInset).isActive = true
    }

    /// 能换行的标签。
    private func wrappingLabel(_ string: String, size: CGFloat, color: NSColor) -> WrappingLabel {
        let label = WrappingLabel(wrappingLabelWithString: string)
        label.font = .systemFont(ofSize: size)
        label.textColor = color
        label.isSelectable = false
        label.preferredMaxLayoutWidth = 360
        label.translatesAutoresizingMaskIntoConstraints = false
        label.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        label.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        return label
    }

    /// 卡片里两行之间的分隔线，左边缩进到文字开始的地方。
    private func separator(inset: CGFloat) -> NSView {
        let holder = NSView()
        holder.translatesAutoresizingMaskIntoConstraints = false
        let line = HairlineView(vertical: false)
        holder.addSubview(line)
        NSLayoutConstraint.activate([
            line.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: inset),
            line.trailingAnchor.constraint(equalTo: holder.trailingAnchor),
            line.topAnchor.constraint(equalTo: holder.topAnchor),
            line.bottomAnchor.constraint(equalTo: holder.bottomAnchor),
        ])
        return holder
    }

    /// 一张卡片，里面是竖着排的各行。
    private func makeCard() -> (CardView, NSStackView) {
        let card = CardView(frame: .zero)
        card.translatesAutoresizingMaskIntoConstraints = false
        let inner = NSStackView()
        inner.orientation = .vertical
        inner.alignment = .leading
        inner.spacing = 0
        inner.translatesAutoresizingMaskIntoConstraints = false
        card.addSubview(inner)
        NSLayoutConstraint.activate([
            inner.leadingAnchor.constraint(equalTo: card.leadingAnchor),
            inner.trailingAnchor.constraint(equalTo: card.trailingAnchor),
            inner.topAnchor.constraint(equalTo: card.topAnchor),
            inner.bottomAnchor.constraint(equalTo: card.bottomAnchor),
        ])
        return (card, inner)
    }

    /// 卡片里加一行，宽度占满卡片。
    private func addCardRow(_ view: NSView, to inner: NSStackView) {
        view.translatesAutoresizingMaskIntoConstraints = false
        inner.addArrangedSubview(view)
        view.widthAnchor.constraint(equalTo: inner.widthAnchor).isActive = true
    }

    private func showPage(_ id: String) {
        guard let page = core.settingsPage(page: id, lang: lang) else { return }
        let samePage = currentPage == id
        currentPage = id
        for view in pageStack.arrangedSubviews {
            pageStack.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        updating = true
        rows = []
        // 大标题（就是左边的名称），键盘、鼠标页下面有一段说明。
        let title = NSTextField(labelWithString: page.title)
        title.font = .systemFont(ofSize: 20, weight: .semibold)
        pageStack.addArrangedSubview(title)
        if !page.lead.isEmpty {
            pageStack.addArrangedSubview(spacer(8))
            addFullWidth(wrappingLabel(page.lead, size: 12, color: .secondaryLabelColor))
        }
        var first = true
        for (index, caption) in page.sections.enumerated() {
            let members = page.items.filter { Int($0.section) == index }
            if members.isEmpty {
                continue
            }
            pageStack.addArrangedSubview(spacer(first ? 14 : 20))
            first = false
            if !caption.isEmpty {
                // 组名：卡片上面的灰色小字，比卡片稍微往里一点。
                let label = NSTextField(labelWithString: caption)
                label.font = .systemFont(ofSize: 11, weight: .semibold)
                label.textColor = .secondaryLabelColor
                label.translatesAutoresizingMaskIntoConstraints = false
                let holder = NSView()
                holder.translatesAutoresizingMaskIntoConstraints = false
                holder.addSubview(label)
                NSLayoutConstraint.activate([
                    label.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: 4),
                    label.trailingAnchor.constraint(lessThanOrEqualTo: holder.trailingAnchor),
                    label.topAnchor.constraint(equalTo: holder.topAnchor),
                    label.bottomAnchor.constraint(equalTo: holder.bottomAnchor),
                ])
                addFullWidth(holder)
                pageStack.addArrangedSubview(spacer(7))
            }
            // 自己设的快捷键另外排（下面还有"添加快捷键"）。
            if members.count == 1, case let .keyMappings(mappingRows, options, addRow) = members[0].control {
                let row = buildKeyMappings(members[0], mappingRows: mappingRows, options: options, addRow: addRow)
                addFullWidth(row.view)
                rows.append(row)
                continue
            }
            let (card, inner) = makeCard()
            for (position, item) in members.enumerated() {
                if position > 0 {
                    addCardRow(separator(inset: 14 + CGFloat(item.indent) * 20), to: inner)
                }
                let row = buildRow(item)
                addCardRow(row.view, to: inner)
                rows.append(row)
            }
            addFullWidth(card)
        }
        updating = false
        if !samePage {
            pageScroll.contentView.scroll(to: .zero)
            pageScroll.reflectScrolledClipView(pageScroll.contentView)
        }
        refreshStates(page)
        if let row = captureAfterRebuild {
            captureAfterRebuild = nil
            startCapture(row: row)
        }
    }

    /// 按核心的页面数据更新各项的可用状态（修改后，依赖它的项可能跟着灰显），以及快捷键各行的按键和提示。
    private func refreshStates(_ given: SettingsPage? = nil) {
        guard let page = given ?? core.settingsPage(page: currentPage, lang: lang) else { return }
        for row in rows {
            guard let item = page.items.first(where: { $0.key == row.key }) else { continue }
            row.setEnabled(item.enabled)
            guard case let .keyMappings(mappingRows, _, addRow) = item.control else { continue }
            // 多了或少了一行：整页重建（等这次事件处理完）。
            guard row.keyRows.map(\.row) == mappingRows.map({ Int($0.row) }) else {
                scheduleRebuild()
                continue
            }
            row.addRow = addRow.map { Int($0) }
            for (controls, mapping) in zip(row.keyRows, mappingRows) {
                controls.toggle.state = mapping.enabled ? .on : .off
                controls.key.keyLabel = mapping.keyLabel.isEmpty ? text(.settingsKeyUnset) : mapping.keyLabel
                controls.note.stringValue = mapping.note
                controls.note.textColor = .secondaryLabelColor
                controls.note.isHidden = mapping.note.isEmpty
                if let index = row.optionIDs.firstIndex(of: mapping.action),
                   controls.popup.indexOfSelectedItem != index {
                    controls.popup.selectItem(at: index)
                }
                let usable = item.enabled && mapping.enabled
                controls.key.isEnabled = usable
                controls.more.isEnabled = usable
                controls.popup.isEnabled = usable
                controls.container.layer?.backgroundColor = nil
            }
        }
    }

    private func scheduleRebuild() {
        guard !rebuildPending else { return }
        rebuildPending = true
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.rebuildPending = false
                self.showPage(self.currentPage)
            }
        }
    }

    /// 把一次修改写进草稿，再刷新各项的可用状态。返回核心是否接受。
    @discardableResult
    private func set(_ key: String, _ input: SettingsInput) -> Bool {
        guard !updating else { return false }
        let accepted = core.settingsSet(key: key, input: input)
        refreshStates()
        return accepted
    }

    private func keyRowControls(_ index: Int) -> KeyRowControls? {
        rows.first(where: { $0.key == Self.keyMappingsKey })?.keyRows.first(where: { $0.row == index })
    }

    /// 快捷键第 row 行录入了一个按键（按下的组合键，或小按钮里选的键；名称为空表示清除）。
    /// 系统占用的按键不接受，在这一行提示（ACTIONS 规则 6）。
    private func recordKey(row index: Int, name: String, modifiers: KeyModifiers) {
        let accepted = set(Self.keyMappingsKey, .keyRowKey(row: UInt32(index), key: name, modifiers: modifiers))
        if !accepted, let controls = keyRowControls(index) {
            controls.note.stringValue = text(.settingsKeyReserved)
            controls.note.isHidden = false
        }
    }

    /// 让第 row 行的按键框等着按键（刚添加了这一行）。
    private func startCapture(row index: Int) {
        keyRowControls(index)?.key.startCapture()
    }

    private func popup(_ options: [SettingsOption], selected: String, width: CGFloat, action: Selector) -> NSPopUpButton {
        let popup = NSPopUpButton(frame: .zero, pullsDown: false)
        // 逐项加入（addItems(withTitles:) 会去掉同名的项）。
        for option in options {
            popup.menu?.addItem(NSMenuItem(title: option.title, action: nil, keyEquivalent: ""))
        }
        if let index = options.firstIndex(where: { $0.id == selected }) {
            popup.selectItem(at: index)
        }
        popup.target = self
        popup.action = action
        popup.translatesAutoresizingMaskIntoConstraints = false
        popup.widthAnchor.constraint(equalToConstant: width).isActive = true
        return popup
    }

    /// 控件不被拉宽（多出来的宽度给左边的文字）。
    private func keepSize(_ view: NSView) {
        view.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        view.setContentCompressionResistancePriority(.defaultHigh, for: .horizontal)
    }

    /// 一行左边的文字：名字（后面跟着灰色的快捷键），下面一行灰色说明。
    private func textColumn(_ item: SettingsItem, row: SettingsRow) -> NSView {
        let column = NSStackView()
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 2
        column.translatesAutoresizingMaskIntoConstraints = false
        let title = wrappingLabel("", size: 13, color: .labelColor)
        let string = NSMutableAttributedString(
            string: item.label,
            attributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: NSColor.labelColor]
        )
        if !item.shortcut.isEmpty {
            string.append(NSAttributedString(
                string: "  " + item.shortcut,
                attributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.secondaryLabelColor]
            ))
        }
        title.attributedStringValue = string
        column.addArrangedSubview(title)
        title.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
        if !item.detail.isEmpty {
            let detail = wrappingLabel(item.detail, size: 11, color: .secondaryLabelColor)
            column.addArrangedSubview(detail)
            detail.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
        }
        column.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        column.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        row.texts.append(column)
        return column
    }

    /// 设置项里的一个按钮（路径旁的"选取…"、格式关联页的按钮等）。
    private func itemButton(key: String, option: SettingsOption) -> NSButton {
        let button = NSButton(title: option.title, target: self, action: #selector(buttonPressed(_:)))
        button.bezelStyle = .rounded
        button.identifier = NSUserInterfaceItemIdentifier("\(key) \(option.id)")
        keepSize(button)
        return button
    }

    private func buildRow(_ item: SettingsItem) -> SettingsRow {
        let stack = NSStackView()
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.distribution = .fill
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 9, left: 14 + CGFloat(item.indent) * 20, bottom: 9, right: 14)
        let row = SettingsRow(key: item.key, view: stack, inverted: item.inverted)
        let identifier = NSUserInterfaceItemIdentifier(item.key)
        switch item.control {
        case let .toggle(on):
            stack.addArrangedSubview(textColumn(item, row: row))
            let toggle = NSSwitch(frame: .zero)
            toggle.state = on ? .on : .off
            toggle.target = self
            toggle.action = #selector(toggleChanged(_:))
            toggle.identifier = identifier
            keepSize(toggle)
            stack.addArrangedSubview(toggle)
            row.toggle = toggle
            row.controls = [toggle]
        case let .choice(options, selected):
            stack.addArrangedSubview(textColumn(item, row: row))
            let menu = popup(options, selected: selected, width: 220, action: #selector(choiceChanged(_:)))
            menu.identifier = identifier
            keepSize(menu)
            stack.addArrangedSubview(menu)
            row.popup = menu
            row.optionIDs = options.map(\.id)
            row.controls = [menu]
        case let .number(text, unit):
            stack.addArrangedSubview(textColumn(item, row: row))
            let field = NSTextField(string: text)
            field.identifier = identifier
            field.delegate = self
            field.alignment = .right
            field.translatesAutoresizingMaskIntoConstraints = false
            field.widthAnchor.constraint(equalToConstant: 72).isActive = true
            keepSize(field)
            let unitLabel = NSTextField(labelWithString: unit)
            unitLabel.textColor = .secondaryLabelColor
            keepSize(unitLabel)
            stack.addArrangedSubview(field)
            stack.addArrangedSubview(unitLabel)
            row.field = field
            row.controls = [field]
            row.texts.append(unitLabel)
        case let .color(value, presets, choose):
            // 名字一行，下面一行是圆形色块（当前颜色外面一圈）和"选取…"（打开系统的取色器）：
            // 15 个色块和名字排在一行时，英文和大多数语言的名字只剩几十点宽，会被拆成几段。
            let column = NSStackView()
            column.orientation = .vertical
            column.alignment = .leading
            column.spacing = 8
            let text = textColumn(item, row: row)
            column.addArrangedSubview(text)
            text.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
            let line = NSStackView()
            line.orientation = .horizontal
            line.alignment = .centerY
            line.spacing = 12
            column.addArrangedSubview(line)
            column.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
            stack.addArrangedSubview(column)
            let strip = NSStackView()
            strip.orientation = .horizontal
            strip.spacing = 1
            var controls: [NSControl] = []
            for preset in presets {
                let chip = NSButton(image: swatchImage(preset, selected: preset == value),
                                    target: self, action: #selector(presetPressed(_:)))
                chip.isBordered = false
                chip.imageScaling = .scaleNone
                chip.tag = Int(preset)
                chip.identifier = identifier
                chip.toolTip = String(format: "#%06X", preset)
                strip.addArrangedSubview(chip)
                row.swatches.append((chip, preset))
                controls.append(chip)
            }
            keepSize(strip)
            line.addArrangedSubview(strip)
            let button = NSButton(title: choose, target: self, action: #selector(choosePressed(_:)))
            button.bezelStyle = .rounded
            button.identifier = identifier
            keepSize(button)
            line.addArrangedSubview(button)
            controls.append(button)
            row.isColor = true
            row.color = value
            row.controls = controls
        case let .path(text, buttons):
            if !item.label.isEmpty {
                let label = NSTextField(labelWithString: item.label)
                label.translatesAutoresizingMaskIntoConstraints = false
                label.widthAnchor.constraint(equalToConstant: 76).isActive = true
                keepSize(label)
                stack.addArrangedSubview(label)
                row.texts.append(label)
            }
            let field = NSTextField(string: text)
            field.identifier = identifier
            field.delegate = self
            field.lineBreakMode = .byTruncatingMiddle
            field.translatesAutoresizingMaskIntoConstraints = false
            field.widthAnchor.constraint(greaterThanOrEqualToConstant: 160).isActive = true
            field.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
            field.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
            stack.addArrangedSubview(field)
            var controls: [NSControl] = [field]
            for option in buttons {
                let button = itemButton(key: item.key, option: option)
                stack.addArrangedSubview(button)
                controls.append(button)
            }
            row.field = field
            row.controls = controls
        case let .buttons(buttons):
            if !item.label.isEmpty {
                stack.addArrangedSubview(textColumn(item, row: row))
            }
            var controls: [NSControl] = []
            for option in buttons {
                let button = itemButton(key: item.key, option: option)
                stack.addArrangedSubview(button)
                controls.append(button)
            }
            if item.label.isEmpty {
                // 没有名字时按钮靠左，后面的空白给一个占位。
                let filler = NSView()
                filler.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
                stack.addArrangedSubview(filler)
            }
            row.controls = controls
        case .note:
            let label = wrappingLabel(item.label, size: 12, color: .secondaryLabelColor)
            stack.addArrangedSubview(label)
            row.texts.append(label)
        case let .mapping(selected, options):
            stack.addArrangedSubview(textColumn(item, row: row))
            let menu = popup(options, selected: selected, width: 220, action: #selector(mappingChanged(_:)))
            menu.identifier = identifier
            keepSize(menu)
            stack.addArrangedSubview(menu)
            row.mappingPopup = menu
            row.optionIDs = options.map(\.id)
            row.controls = [menu]
        case .keyMappings:
            // 自己设的快捷键在 buildKeyMappings 里排。
            break
        }
        return row
    }

    /// 自己设的快捷键：一张卡片列出用过的行，下面一个"添加快捷键"。
    private func buildKeyMappings(_ item: SettingsItem, mappingRows: [KeyMappingRow], options: [SettingsOption],
                                  addRow: UInt32?) -> SettingsRow {
        let block = NSStackView()
        block.orientation = .vertical
        block.alignment = .leading
        block.spacing = 12
        let row = SettingsRow(key: item.key, view: block, inverted: false)
        row.optionIDs = options.map(\.id)
        row.addRow = addRow.map { Int($0) }
        if !mappingRows.isEmpty {
            let (card, inner) = makeCard()
            for (position, mapping) in mappingRows.enumerated() {
                if position > 0 {
                    addCardRow(separator(inset: 14), to: inner)
                }
                let controls = buildKeyRow(mapping, options: options)
                addCardRow(controls.container, to: inner)
                row.keyRows.append(controls)
                row.controls.append(contentsOf: [controls.toggle, controls.key, controls.more, controls.popup])
            }
            block.addArrangedSubview(card)
            card.widthAnchor.constraint(equalTo: block.widthAnchor).isActive = true
        }
        // "添加快捷键"：打开映射表里的一行空行，它就出现在列表里，按键框等着按键。
        let add = NSButton(title: text(.settingsAddShortcut), target: self, action: #selector(addShortcutPressed(_:)))
        add.bezelStyle = .rounded
        if let plus = NSImage(systemSymbolName: "plus", accessibilityDescription: nil) {
            add.image = plus
            add.imagePosition = .imageLeading
        }
        add.isEnabled = addRow != nil
        block.addArrangedSubview(add)
        return row
    }

    private func buildKeyRow(_ mapping: KeyMappingRow, options: [SettingsOption]) -> KeyRowControls {
        let index = Int(mapping.row)
        let container = NSStackView()
        container.orientation = .horizontal
        container.alignment = .centerY
        container.distribution = .fill
        container.spacing = 10
        container.edgeInsets = NSEdgeInsets(top: 8, left: 14, bottom: 8, right: 10)
        container.wantsLayer = true
        container.layer?.cornerRadius = 8
        // 按键框：点一下后按下组合键即录入。
        let key = KeyRecorderButton(title: "", target: nil, action: nil)
        key.bezelStyle = .rounded
        key.font = .boldSystemFont(ofSize: 13)
        key.promptText = text(.settingsPressKey)
        key.keyLabel = mapping.keyLabel.isEmpty ? text(.settingsKeyUnset) : mapping.keyLabel
        key.translatesAutoresizingMaskIntoConstraints = false
        key.widthAnchor.constraint(equalToConstant: 116).isActive = true
        key.onRecord = { [weak self] name, modifiers in
            self?.recordKey(row: index, name: name, modifiers: modifiers)
        }
        keepSize(key)
        // 小按钮：下拉列出不方便直接按出的键，首项是清除。
        let more = NSPopUpButton(frame: .zero, pullsDown: true)
        more.menu?.addItem(NSMenuItem(title: "…", action: nil, keyEquivalent: ""))
        for option in specialKeys {
            more.menu?.addItem(NSMenuItem(title: option.title, action: nil, keyEquivalent: ""))
        }
        more.tag = index
        more.target = self
        more.action = #selector(specialKeyChosen(_:))
        more.translatesAutoresizingMaskIntoConstraints = false
        more.widthAnchor.constraint(equalToConstant: 44).isActive = true
        keepSize(more)
        let arrow = NSTextField(labelWithString: "→")
        arrow.textColor = .secondaryLabelColor
        keepSize(arrow)
        // 动作下拉框，下面是这一行的提示（将替代某个默认快捷键、和别的行重复）。
        let column = NSStackView()
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 3
        let menu = popup(options, selected: mapping.action, width: 236, action: #selector(keyActionChanged(_:)))
        menu.tag = index
        let note = NSTextField(labelWithString: mapping.note)
        note.font = .systemFont(ofSize: 11)
        note.textColor = .secondaryLabelColor
        note.isHidden = mapping.note.isEmpty
        column.addArrangedSubview(menu)
        column.addArrangedSubview(note)
        column.setContentHuggingPriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        column.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(1), for: .horizontal)
        let toggle = NSSwitch(frame: .zero)
        toggle.state = mapping.enabled ? .on : .off
        toggle.tag = index
        toggle.target = self
        toggle.action = #selector(keyRowToggled(_:))
        keepSize(toggle)
        let remove: NSButton
        if let trash = NSImage(systemSymbolName: "trash", accessibilityDescription: text(.settingsRemoveShortcut)) {
            remove = NSButton(image: trash, target: self, action: #selector(removeKeyRowPressed(_:)))
        } else {
            remove = NSButton(title: "✕", target: self, action: #selector(removeKeyRowPressed(_:)))
        }
        remove.isBordered = false
        remove.tag = index
        remove.toolTip = text(.settingsRemoveShortcut)
        keepSize(remove)
        for view in [key, more, arrow, column, toggle, remove] as [NSView] {
            container.addArrangedSubview(view)
        }
        return KeyRowControls(row: index, container: container, toggle: toggle, key: key, more: more,
                              popup: menu, note: note)
    }

    // MARK: 控件的操作

    private func row(for sender: NSView) -> SettingsRow? {
        guard let key = sender.identifier?.rawValue else { return nil }
        let itemKey = key.split(separator: " ").first.map(String.init) ?? key
        return rows.first { $0.key == itemKey }
    }

    @objc private func toggleChanged(_ sender: NSSwitch) {
        guard let row = row(for: sender) else { return }
        set(row.key, .bool(value: sender.state == .on))
    }

    @objc private func choiceChanged(_ sender: NSPopUpButton) {
        guard let row = row(for: sender), row.optionIDs.indices.contains(sender.indexOfSelectedItem) else { return }
        set(row.key, .choice(id: row.optionIDs[sender.indexOfSelectedItem]))
    }

    /// 鼠标映射的下拉框："默认"、"无操作"或动作（核心按 ID 决定勾不勾选）。
    @objc private func mappingChanged(_ sender: NSPopUpButton) {
        guard let row = row(for: sender), row.optionIDs.indices.contains(sender.indexOfSelectedItem) else { return }
        set(row.key, .mappingChoice(id: row.optionIDs[sender.indexOfSelectedItem]))
    }

    @objc private func keyRowToggled(_ sender: NSSwitch) {
        set(Self.keyMappingsKey, .keyRowEnabled(row: UInt32(sender.tag), value: sender.state == .on))
    }

    @objc private func keyActionChanged(_ sender: NSPopUpButton) {
        guard let row = rows.first(where: { $0.key == Self.keyMappingsKey }),
              row.optionIDs.indices.contains(sender.indexOfSelectedItem)
        else { return }
        set(Self.keyMappingsKey, .keyRowAction(row: UInt32(sender.tag), id: row.optionIDs[sender.indexOfSelectedItem]))
    }

    @objc private func specialKeyChosen(_ sender: NSPopUpButton) {
        // 下拉的第一项是按钮自己的标题"…"。
        let index = sender.indexOfSelectedItem - 1
        guard specialKeys.indices.contains(index) else { return }
        let none = KeyModifiers(shift: false, control: false, alt: false, command: false)
        recordKey(row: sender.tag, name: specialKeys[index].id, modifiers: none)
    }

    /// 删掉一行快捷键（变回空行，列表里少一行）。
    @objc private func removeKeyRowPressed(_ sender: NSButton) {
        set(Self.keyMappingsKey, .keyRowRemove(row: UInt32(sender.tag)))
    }

    /// "添加快捷键"：打开第一行空行，重建这一页后它的按键框等着按键。
    @objc private func addShortcutPressed(_ sender: NSButton) {
        guard let addRow = rows.first(where: { $0.key == Self.keyMappingsKey })?.addRow,
              set(Self.keyMappingsKey, .keyRowEnabled(row: UInt32(addRow), value: true))
        else { return }
        captureAfterRebuild = addRow
        scheduleRebuild()
    }

    /// 数字、路径输入框：每改一个字都写进草稿（不合法的数字到"确定"时再拦下）。
    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField, let row = row(for: field) else { return }
        field.layer?.borderWidth = 0
        set(row.key, .text(text: field.stringValue))
    }

    @objc private func presetPressed(_ sender: NSButton) {
        guard let row = row(for: sender) else { return }
        setColor(row, UInt32(truncatingIfNeeded: sender.tag))
    }

    /// 选了一个颜色（预设色块或取色器）：写进草稿，色块上的选中标记跟着换。
    private func setColor(_ row: SettingsRow, _ color: UInt32) {
        row.color = color
        for (chip, preset) in row.swatches {
            chip.image = swatchImage(preset, selected: preset == color)
        }
        set(row.key, .color(value: color))
    }

    @objc private func choosePressed(_ sender: NSButton) {
        guard let row = row(for: sender) else { return }
        run(core.settingsButton(key: row.key, button: "choose"))
    }

    @objc private func buttonPressed(_ sender: NSButton) {
        guard let identifier = sender.identifier?.rawValue else { return }
        let parts = identifier.split(separator: " ").map(String.init)
        guard parts.count == 2 else { return }
        run(core.settingsButton(key: parts[0], button: parts[1]))
    }

    /// 设置项里的按钮：核心说做什么就做什么（点击即执行，不属于草稿）。
    private func run(_ command: SettingsCommand?) {
        guard let command else { return }
        switch command {
        case .setDefaultViewer:
            setDefaultViewer()
        case let .openFolder(path):
            _ = NSWorkspace.shared.open(URL(fileURLWithPath: path, isDirectory: true))
        case let .chooseFolder(key):
            choosePath(key: key, folder: true)
        case let .chooseFile(key):
            choosePath(key: key, folder: false)
        case let .chooseApp(key):
            chooseApp(key: key)
        case .refresh:
            // 核心改了草稿（如图像编辑器的"用推荐的"）：重新显示这一页。
            showPage(currentPage)
        case .confirmClearArchivePositions:
            confirmClearPositions()
        case .restoreDefaultViewer, .registerThumbnailer, .unregisterThumbnailer, .openDefaultApps, .openAssociationsByApp:
            // 只在 Linux、Windows 的设置窗口里有。
            break
        case let .chooseColor(key):
            guard let row = rows.first(where: { $0.key == key }) else { return }
            colorKey = key
            let panel = NSColorPanel.shared
            panel.showsAlpha = false
            panel.color = colorFromRGB(row.color)
            panel.setTarget(self)
            panel.setAction(#selector(colorPanelChanged(_:)))
            panel.orderFront(nil)
        }
    }

    @objc private func colorPanelChanged(_ sender: NSColorPanel) {
        guard let key = colorKey, let row = rows.first(where: { $0.key == key }),
              let color = sender.color.usingColorSpace(.sRGB)
        else { return }
        func channel(_ value: CGFloat) -> UInt32 {
            UInt32(max(0, min(255, (value * 255).rounded())))
        }
        setColor(row, channel(color.redComponent) << 16 | channel(color.greenComponent) << 8 | channel(color.blueComponent))
    }

    /// 选图像编辑器：在"应用程序"文件夹里选一个程序。
    private func chooseApp(key: String) {
        guard let window else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.allowedContentTypes = [.application]
        panel.directoryURL = URL(fileURLWithPath: "/Applications", isDirectory: true)
        panel.beginSheetModal(for: window) { [weak self] response in
            MainActor.assumeIsolated {
                guard response == .OK, let url = panel.url, let self,
                      let row = self.rows.first(where: { $0.key == key }), let field = row.field
                else { return }
                field.stringValue = url.path(percentEncoded: false)
                self.set(key, .text(text: field.stringValue))
            }
        }
    }

    /// 书签页的"清除"：先确认，再忘掉所有压缩包内的位置。自检时直接清。
    private func confirmClearPositions() {
        if quiet {
            core.clearArchivePositions()
            return
        }
        guard let window else { return }
        let alert = NSAlert()
        alert.messageText = uiText(key: .promptClearPositions, lang: lang)
        alert.addButton(withTitle: uiText(key: .buttonDelete, lang: lang))
        alert.addButton(withTitle: uiText(key: .buttonCancel, lang: lang))
        alert.buttons.last?.keyEquivalent = String(UnicodeScalar(UInt8(27)))
        alert.beginSheetModal(for: window) { [weak self] response in
            MainActor.assumeIsolated {
                if response == .alertFirstButtonReturn {
                    self?.core.clearArchivePositions()
                }
            }
        }
    }

    /// 自检：按设置项里的一个按钮（与点它相同）。
    func pressButtonForTest(key: String, button: String) {
        run(core.settingsButton(key: key, button: button))
    }

    private func choosePath(key: String, folder: Bool) {
        guard let window else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = !folder
        panel.canChooseDirectories = folder
        panel.allowsMultipleSelection = false
        panel.beginSheetModal(for: window) { [weak self] response in
            MainActor.assumeIsolated {
                guard response == .OK, let url = panel.url, let self,
                      let row = self.rows.first(where: { $0.key == key }), let field = row.field
                else { return }
                field.stringValue = url.path(percentEncoded: false)
                self.set(key, .text(text: field.stringValue))
            }
        }
    }

    /// "设为默认看图软件"还在等系统回复的类型数，以及第一个错误。
    private var defaultViewerPending = 0
    private var defaultViewerError: String?

    /// "设为默认"：把本程序设为它支持的每种图片类型的默认程序。
    private func setDefaultViewer() {
        let app = Bundle.main.bundleURL
        let types = Array(Set(supportedExtensions().compactMap { UTType(filenameExtension: $0) }))
        guard !types.isEmpty, defaultViewerPending == 0 else { return }
        defaultViewerPending = types.count
        defaultViewerError = nil
        for type in types {
            NSWorkspace.shared.setDefaultApplication(at: app, toOpen: type) { [weak self] error in
                let message = error?.localizedDescription
                DispatchQueue.main.async { [weak self] in
                    MainActor.assumeIsolated {
                        self?.defaultViewerAnswered(message)
                    }
                }
            }
        }
    }

    private func defaultViewerAnswered(_ error: String?) {
        defaultViewerPending -= 1
        if defaultViewerError == nil, let error {
            defaultViewerError = error
        }
        if defaultViewerPending == 0 {
            showDefaultViewerResult(defaultViewerError)
        }
    }

    private func showDefaultViewerResult(_ error: String?) {
        let message = error.map {
            text(.settingsDefaultViewerFailed).replacingOccurrences(of: "{message}", with: $0)
        } ?? text(.settingsDefaultViewerDone)
        guard let window, !quiet else { return }
        let alert = NSAlert()
        alert.messageText = message
        alert.beginSheetModal(for: window, completionHandler: nil)
    }

    // MARK: 恢复本页默认、取消、确定、关闭

    @objc private func resetPressed(_ sender: Any?) {
        reset()
    }

    /// "取消"：直接放弃修改、关窗口，不问。
    @objc private func cancelPressed(_ sender: Any?) {
        discard()
    }

    @objc private func okPressed(_ sender: Any?) {
        _ = ok()
    }

    /// "恢复本页默认"：这一页恢复默认值（还要点"确定"）。
    func reset() {
        core.settingsResetPage(page: currentPage)
        showPage(currentPage)
    }

    /// "确定"：校验通过就生效、关窗口；不通过就切到出错的页、标出那一项（快捷键标出那一行）并说明原因。
    /// 返回是否已生效。
    func ok() -> Bool {
        switch core.settingsApply(lang: lang) {
        case let .applied(changed, saveError):
            lastError = nil
            closing = true
            window?.close()
            onApplied?(SettingsAppliedInfo(changed: changed, saveError: saveError))
            return true
        case let .invalid(page, key, badRow, message):
            lastError = message
            selectPage(page)
            let target = rows.first(where: { $0.key == key })
            if let field = target?.field {
                window?.makeFirstResponder(field)
                field.wantsLayer = true
                field.layer?.borderColor = NSColor.systemRed.cgColor
                field.layer?.borderWidth = 2
            }
            // 快捷键：在出错的那一行用红字说明，整行标红。
            if let badRow, let controls = target?.keyRows.first(where: { $0.row == Int(badRow) }) {
                controls.note.stringValue = message
                controls.note.textColor = .systemRed
                controls.note.isHidden = false
                controls.container.layer?.backgroundColor = NSColor.systemRed.withAlphaComponent(0.16).cgColor
            }
            if !quiet, let window {
                let alert = NSAlert()
                alert.messageText = message
                alert.beginSheetModal(for: window, completionHandler: nil)
            }
            return false
        }
    }

    /// 放弃修改并关窗口（"取消"、询问后选了"放弃修改"；自检也用它）。
    func discard() {
        closing = true
        window?.close()
    }

    /// 关闭窗口（×、Esc、⌘W）：有没确定的修改就先询问"放弃修改？"。
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if closing || !core.settingsDirty() {
            return true
        }
        let alert = NSAlert()
        alert.messageText = text(.settingsDiscardMessage)
        alert.addButton(withTitle: text(.settingsKeepEditing))
        alert.addButton(withTitle: text(.settingsDiscard))
        alert.beginSheetModal(for: sender) { [weak self] response in
            MainActor.assumeIsolated {
                if response == .alertSecondButtonReturn {
                    self?.discard()
                }
            }
        }
        return false
    }

    func windowWillClose(_ notification: Notification) {
        // 没点"确定"就关掉：丢掉草稿（"确定"时草稿已经用掉了）。
        core.settingsCancel()
        if colorKey != nil {
            NSColorPanel.shared.setTarget(nil)
            NSColorPanel.shared.orderOut(nil)
        }
        onClosed?()
    }

    // MARK: 自检

    /// 像用户那样改一项（操作对应的控件）。value 按设置文件里存的值写：开关为 true/false（翻过来说的开关在这里
    /// 换成开关的状态），下拉框为选项 ID，数字和路径为文字，颜色为 #RRGGBB。返回控件是否找到并且可以改。
    func setForTest(key: String, value: String) -> Bool {
        let parts = value.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
        // 快捷键的一行：`keyboard.mappings#行=on|按键|动作`（按键写法同 `Ctrl+K`，可省略后两段）或 `=off`。
        // 映射表里这一行还没用过时，先像"添加快捷键"那样把它加进列表。
        if let hash = key.firstIndex(of: "#") {
            let base = String(key[..<hash])
            guard let index = Int(key[key.index(after: hash)...]) else { return false }
            if keyRowControls(index) == nil {
                guard set(base, .keyRowEnabled(row: UInt32(index), value: true)) else { return false }
                showPage(currentPage)
            }
            guard let row = rows.first(where: { $0.key == base }),
                  let controls = row.keyRows.first(where: { $0.row == index }),
                  parts[0] == "on" || parts[0] == "off"
            else { return false }
            controls.toggle.state = parts[0] == "on" ? .on : .off
            keyRowToggled(controls.toggle)
            if parts.count > 1 {
                guard let parsed = Self.parseKey(parts[1]) else { return false }
                recordKey(row: index, name: parsed.0, modifiers: parsed.1)
            }
            if parts.count > 2 {
                guard let position = row.optionIDs.firstIndex(of: parts[2]) else { return false }
                controls.popup.selectItem(at: position)
                keyActionChanged(controls.popup)
            }
            return true
        }
        guard let row = rows.first(where: { $0.key == key }), row.controls.first?.isEnabled == true else {
            return false
        }
        // 鼠标映射：`on|动作`（动作为空时是"无操作"）或 `off`（下拉框的"默认"）。
        if let menu = row.mappingPopup {
            let id: String
            switch parts[0] {
            case "off":
                id = "default"
            case "on":
                id = parts.count > 1 ? parts[1] : ""
            default:
                return false
            }
            guard let position = row.optionIDs.firstIndex(of: id) else { return false }
            menu.selectItem(at: position)
            mappingChanged(menu)
            return true
        }
        if let toggle = row.toggle {
            guard value == "true" || value == "false" else { return false }
            toggle.state = (value == "true") != row.inverted ? .on : .off
            toggleChanged(toggle)
        } else if let menu = row.popup {
            guard let index = row.optionIDs.firstIndex(of: value) else { return false }
            menu.selectItem(at: index)
            choiceChanged(menu)
        } else if let field = row.field {
            field.stringValue = value
            set(row.key, .text(text: value))
        } else if row.isColor {
            guard value.hasPrefix("#"), value.count == 7, let color = UInt32(value.dropFirst(), radix: 16) else {
                return false
            }
            setColor(row, color)
        } else {
            return false
        }
        return true
    }

    /// 按键的写法（同动作目录，如 `Ctrl+K`、`Cmd+Shift+O`、`Q`）→ 按键名称和修饰键。
    static func parseKey(_ spec: String) -> (String, KeyModifiers)? {
        var rest = Substring(spec)
        var shift = false, control = false, alt = false, command = false
        while true {
            if rest.hasPrefix("Shift+"), rest.count > 6 {
                shift = true
                rest = rest.dropFirst(6)
            } else if rest.hasPrefix("Ctrl+"), rest.count > 5 {
                control = true
                rest = rest.dropFirst(5)
            } else if rest.hasPrefix("Alt+"), rest.count > 4 {
                alt = true
                rest = rest.dropFirst(4)
            } else if rest.hasPrefix("Cmd+"), rest.count > 4 {
                command = true
                rest = rest.dropFirst(4)
            } else {
                break
            }
        }
        guard !rest.isEmpty else { return nil }
        return (String(rest), KeyModifiers(shift: shift, control: control, alt: alt, command: command))
    }

    /// 这一页的标题、各组名，每一项的键、文字、所在的卡片、是否可用和值（自检报告用）。
    func pageSummary() -> [String: Any] {
        guard let page = core.settingsPage(page: currentPage, lang: lang) else { return [:] }
        let items: [[String: Any]] = page.items.map { item in
            var value: Any = NSNull()
            switch item.control {
            case let .toggle(on): value = on
            case let .choice(_, selected): value = selected
            case let .number(text, _): value = text
            case let .path(text, _): value = text
            case let .color(color, _, _): value = String(format: "#%06X", color)
            case let .mapping(selected, _): value = selected
            // 快捷键：列出来的行（用过的）写成"on/off 按键 动作 提示"。
            case let .keyMappings(mappingRows, _, _):
                value = mappingRows.map {
                    "\($0.enabled ? "on" : "off") \($0.keyLabel) \($0.action) \($0.note)"
                        .trimmingCharacters(in: .whitespaces)
                }
            default: break
            }
            return ["key": item.key, "label": item.label, "section": Int(item.section), "enabled": item.enabled,
                    "value": value]
        }
        return ["id": page.id, "title": page.title, "sections": page.sections, "items": items]
    }

    /// 把设置窗口的内容画成 PNG（自检报告用）。
    func snapshot(to url: URL) -> [String: Any] {
        guard let window else { return ["error": "没有设置窗口"] }
        return windowSnapshot(window, to: url)
    }
}
