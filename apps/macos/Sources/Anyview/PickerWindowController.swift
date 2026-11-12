import AppKit
import AnyviewCore

/// 选择图片窗口：Esc 和关闭按钮一样，不切换。
private final class PickerWindow: NSWindow {
    var onCancel: (() -> Void)?

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }
}

/// 缩略图带：黑底，所选的一张在正中、放大、加橙色框，两边依次排开，排到边上为止。
/// 单击只选中，双击切换；滚轮一格移一张。缩略图由窗口控制器给出。
final class ThumbnailStripView: NSView {
    static let height: CGFloat = 112
    private static let small = NSSize(width: 64, height: 48)
    private static let big = NSSize(width: 96, height: 72)
    private static let gap: CGFloat = 8
    private static let frameWidth: CGFloat = 3
    private static let frameColor = NSColor(srgbRed: 1, green: 0.6, blue: 0.1, alpha: 1)

    var count = 0
    var selected: Int?
    /// 第 i 张的缩略图：.some(image) 已有，.some(nil) 生成失败，nil 还没有。
    var thumbnail: ((Int) -> CGImage??)?
    /// 点了第 i 张；第二个参数为真表示双击。
    var onPick: ((Int, Bool) -> Void)?
    var onStep: ((Int) -> Void)?
    /// 大小变了（窗口第一次排好、用户拖动窗口）：窗口控制器重新排格子并要还没有的缩略图。
    /// 窗口刚建好时缩略图带的宽度还是 0，只排得下正中那一格，排好后要补上两边的。
    var onResize: (() -> Void)?
    /// 现在画出来的格子：(序号, 左边 x, 宽度)。
    private(set) var cells: [(index: Int, x: CGFloat, width: CGFloat)] = []
    private var scrollSum: CGFloat = 0
    /// 上一次单击点到了格子：双击的第二下照样打开那一张（第二下可能落在重排后的别的格子或空白处）。
    private var pressedCell = false

    /// 按宽度重新排格子（选择变了、大小变了时）。
    func layoutCells() {
        cells = []
        guard let center = selected, center < count else { return }
        let width = bounds.width
        let bigWidth = Self.big.width + 2 * Self.frameWidth
        var x = (width - bigWidth) / 2
        cells.append((center, x, bigWidth))
        var i = center
        while i > 0 && x > 0 {
            i -= 1
            x -= Self.gap + Self.small.width
            cells.append((i, x, Self.small.width))
        }
        x = (width + bigWidth) / 2
        i = center
        while i + 1 < count && x < width {
            i += 1
            cells.append((i, x + Self.gap, Self.small.width))
            x += Self.gap + Self.small.width
        }
        cells.sort { $0.index < $1.index }
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        widthMayHaveChanged()
    }

    override func layout() {
        super.layout()
        widthMayHaveChanged()
    }

    private var laidOutWidth: CGFloat = -1

    private func widthMayHaveChanged() {
        guard bounds.width != laidOutWidth else { return }
        laidOutWidth = bounds.width
        layoutCells()
        onResize?()
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.black.setFill()
        bounds.fill()
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let mid = bounds.midY
        for cell in cells {
            let isSelected = cell.index == selected
            let box = isSelected ? Self.big : Self.small
            let boxX = isSelected ? cell.x + Self.frameWidth : cell.x
            let boxRect = NSRect(x: boxX, y: mid - box.height / 2, width: box.width, height: box.height)
            if isSelected {
                Self.frameColor.setFill()
                NSRect(x: cell.x, y: boxRect.minY - Self.frameWidth, width: cell.width,
                       height: box.height + 2 * Self.frameWidth).fill()
                NSColor.black.setFill()
                boxRect.fill()
            }
            switch thumbnail?(cell.index) {
            case let .some(.some(image)):
                // 按比例放进格子（很小的图也放大到格子大小）。
                let w = CGFloat(image.width)
                let h = CGFloat(image.height)
                let scale = min(box.width / max(w, 1), box.height / max(h, 1))
                let drawn = NSRect(x: boxRect.midX - w * scale / 2, y: boxRect.midY - h * scale / 2,
                                   width: w * scale, height: h * scale)
                context.interpolationQuality = .high
                context.draw(image, in: drawn)
            case .some(.none):
                NSColor(white: 0.12, alpha: 1).setFill()
                boxRect.fill()
            case .none:
                NSColor(white: 0.2, alpha: 1).setFill()
                boxRect.fill()
            }
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if event.clickCount >= 2 {
            // 双击：打开第一下选中的那一张（第一下以后格子重排了）。
            if pressedCell, let current = selected {
                onPick?(current, true)
            }
            return
        }
        guard let cell = cells.first(where: { point.x >= $0.x && point.x < $0.x + $0.width }) else {
            pressedCell = false
            return
        }
        pressedCell = true
        onPick?(cell.index, false)
    }

    override func scrollWheel(with event: NSEvent) {
        let delta = abs(event.scrollingDeltaY) >= abs(event.scrollingDeltaX) ? event.scrollingDeltaY : event.scrollingDeltaX
        if !event.hasPreciseScrollingDeltas {
            if delta != 0 {
                onStep?(delta > 0 ? -1 : 1)
            }
            return
        }
        // 触控板：累计够 24 点移一张。
        scrollSum += delta
        while abs(scrollSum) >= 24 {
            onStep?(scrollSum > 0 ? -1 : 1)
            scrollSum -= scrollSum > 0 ? 24 : -24
        }
    }
}

/// 选择图片窗口。
/// 上面是缩略图导航，下面是列表（名称、修改日期、大小），底部是排序按钮、"隐藏缩略图导航"和"确定"。
/// 单击、方向键只移动选择，主窗口不切换；双击、回车或"确定"切到所选图片并关掉窗口；Esc 或关闭按钮不切换。
/// 列表内容、文字和缩略图都由核心给出。
@MainActor
final class PickerWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate {
    private let core: AppCore
    private let lang: Lang
    let strip = ThumbnailStripView()
    private let table = NSTableView()
    private let sortButton = NSPopUpButton(frame: .zero, pullsDown: true)
    private let hideCheck: NSButton
    private var rows: [PickerRow] = []
    private var sortOptions: [PickerSortOption] = []
    /// 缩略图：路径 → 图像（nil 表示生成失败）。
    private var thumbs: [String: CGImage?] = [:]
    /// 程序自己在改选择（重新载入列表），这时不当作用户操作。
    private var updating = false
    var onClosed: (() -> Void)?

    init(core: AppCore, lang: Lang) {
        self.core = core
        self.lang = lang
        hideCheck = NSButton(checkboxWithTitle: uiText(key: .pickerHideThumbnails, lang: lang), target: nil, action: nil)
        let window = PickerWindow(
            contentRect: NSRect(x: 0, y: 0, width: 760, height: 560),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = uiText(key: .pickerTitle, lang: lang)
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 480, height: 320)
        super.init(window: window)
        window.delegate = self
        window.onCancel = { [weak self] in
            self?.window?.performClose(nil)
        }
        buildLayout(in: window)
        let hidden = core.pickerHidesThumbnails()
        hideCheck.state = hidden ? .on : .off
        strip.isHidden = hidden
        reload()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    private func buildLayout(in window: NSWindow) {
        guard let content = window.contentView else { return }
        strip.translatesAutoresizingMaskIntoConstraints = false
        strip.thumbnail = { [weak self] index in
            guard let self, index < self.rows.count else { return nil }
            return self.thumbs[self.rows[index].path]
        }
        // 单击选中（这一格移到正中），双击打开第一下选中的那一张（见缩略图带的 mouseDown）。
        strip.onPick = { [weak self] index, double in
            if double {
                self?.choose()
            } else {
                self?.select(index)
            }
        }
        strip.onResize = { [weak self] in
            self?.refreshStrip()
        }
        strip.onStep = { [weak self] step in
            guard let self, let current = self.selectedIndex else { return }
            self.select(max(0, min(self.rows.count - 1, current + step)))
        }

        let columns: [(String, TextKey, CGFloat)] = [
            ("name", .pickerColumnName, 420), ("modified", .pickerColumnModified, 160), ("size", .pickerColumnSize, 90),
        ]
        for (id, key, width) in columns {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(id))
            column.title = uiText(key: key, lang: lang)
            column.width = width
            if id == "size" {
                column.headerCell.alignment = .right
            }
            table.addTableColumn(column)
        }
        table.columnAutoresizingStyle = .firstColumnOnlyAutoresizingStyle
        table.usesAlternatingRowBackgroundColors = true
        table.allowsMultipleSelection = false
        table.allowsEmptySelection = false
        table.rowHeight = 20
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(doubleClicked(_:))
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.translatesAutoresizingMaskIntoConstraints = false

        // 底部：排序按钮（显示现在的排序）、"隐藏缩略图导航"、"确定"。
        sortButton.translatesAutoresizingMaskIntoConstraints = false
        hideCheck.target = self
        hideCheck.action = #selector(hideToggled(_:))
        hideCheck.translatesAutoresizingMaskIntoConstraints = false
        let ok = NSButton(title: uiText(key: .pickerOk, lang: lang), target: self, action: #selector(okPressed(_:)))
        ok.bezelStyle = .rounded
        ok.keyEquivalent = "\r"
        ok.translatesAutoresizingMaskIntoConstraints = false

        for view in [strip, scroll, sortButton, hideCheck, ok] as [NSView] {
            content.addSubview(view)
        }
        let stripHeight = strip.heightAnchor.constraint(equalToConstant: ThumbnailStripView.height)
        NSLayoutConstraint.activate([
            strip.topAnchor.constraint(equalTo: content.topAnchor, constant: 8),
            strip.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 8),
            strip.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -8),
            stripHeight,
            scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 8),
            scroll.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -8),
            scroll.bottomAnchor.constraint(equalTo: ok.topAnchor, constant: -10),
            sortButton.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 8),
            sortButton.widthAnchor.constraint(greaterThanOrEqualToConstant: 180),
            sortButton.centerYAnchor.constraint(equalTo: ok.centerYAnchor),
            hideCheck.leadingAnchor.constraint(equalTo: sortButton.trailingAnchor, constant: 16),
            hideCheck.centerYAnchor.constraint(equalTo: ok.centerYAnchor),
            ok.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -12),
            ok.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -10),
            ok.widthAnchor.constraint(greaterThanOrEqualToConstant: 96),
        ])
        // 缩略图带隐藏时列表从顶上开始。
        stripTop = scroll.topAnchor.constraint(equalTo: strip.bottomAnchor, constant: 8)
        listTop = scroll.topAnchor.constraint(equalTo: content.topAnchor, constant: 8)
        stripTop?.isActive = true
    }

    private var stripTop: NSLayoutConstraint?
    private var listTop: NSLayoutConstraint?

    // MARK: 内容

    /// 选中的行。
    var selectedIndex: Int? {
        let row = table.selectedRow
        return row >= 0 && row < rows.count ? row : nil
    }

    var rowCount: Int { rows.count }

    func row(_ index: Int) -> PickerRow? {
        index >= 0 && index < rows.count ? rows[index] : nil
    }

    var sortLabel: String {
        sortButton.numberOfItems > 0 ? sortButton.itemTitle(at: 0) : ""
    }

    var stripVisible: Bool { !strip.isHidden }

    /// 缩略图带上看得见的几张里，已经有缩略图的张数和总张数（自检用）。
    var thumbnailsLoaded: (ready: Int, visible: Int) {
        let ready = strip.cells.filter { cell in
            guard cell.index < rows.count, let thumb = thumbs[rows[cell.index].path] else { return false }
            return thumb != nil
        }.count
        return (ready, strip.cells.count)
    }

    /// 从核心重新取列表（打开时、排序后、文件夹变了）。原来选中的文件还在就继续选中它，否则选中当前图片。
    func reload() {
        guard let list = core.pickerList(lang: lang) else { return }
        let keep = selectedIndex.map { rows[$0].path }
        let target = keep.flatMap { path in list.rows.firstIndex { $0.path == path } }
            ?? list.current.map { Int($0) } ?? 0
        rows = list.rows
        // 文件夹变了：文件可能换过内容，画好的缩略图不留（核心按修改时间认，会给新的）。
        thumbs = thumbs.filter { $0.value == nil }
        sortOptions = list.sortOptions
        rebuildSortMenu(title: list.sortLabel)
        updating = true
        table.reloadData()
        updating = false
        select(target)
    }

    private func rebuildSortMenu(title: String) {
        sortButton.removeAllItems()
        // 下拉按钮的第一项是按钮上显示的文字。
        sortButton.addItem(withTitle: title)
        for (i, option) in sortOptions.enumerated() {
            if option.action == "sort_desc" {
                sortButton.menu?.addItem(NSMenuItem.separator())
            }
            let item = NSMenuItem(title: option.title, action: #selector(sortChosen(_:)), keyEquivalent: "")
            item.target = self
            item.tag = i
            item.state = option.checked ? .on : .off
            sortButton.menu?.addItem(item)
        }
    }

    /// 选中第 index 行并滚到能看见（单击、方向键的效果）。
    func select(_ index: Int) {
        guard index >= 0, index < rows.count else { return }
        if table.selectedRow != index {
            table.selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false)
        }
        table.scrollRowToVisible(index)
        refreshStrip()
    }

    /// 按缩略图带的宽度排好格子，向核心要还没有的缩略图，重画。
    private func refreshStrip() {
        strip.count = rows.count
        strip.selected = selectedIndex
        strip.layoutCells()
        // 只留看得见的那几张（不然在几千张的文件夹里一路翻过去，看过的全留在内存里）。核心另有一份有上限的
        // 缓存，翻回来时从那里取。生成失败的只是个记号，留着（核心不会再报一次）。
        let visible = Set(strip.cells.filter { $0.index < rows.count }.map { rows[$0.index].path })
        thumbs = thumbs.filter { $0.value == nil || visible.contains($0.key) }
        if !strip.isHidden {
            var missing: [String] = []
            for cell in strip.cells where cell.index < rows.count {
                let path = rows[cell.index].path
                if thumbs[path] != nil {
                    continue
                }
                if let handle = core.pickerThumbnail(path: path) {
                    thumbs[path] = .some(ImageCanvasView.makeCGImage(handle))
                } else {
                    missing.append(path)
                }
            }
            core.pickerWantThumbnails(paths: missing)
        }
        strip.needsDisplay = true
    }

    /// 缩略图好了（核心的 thumbnail 事件）。
    func thumbnailArrived(path: String, image: ImageHandle?) {
        thumbs[path] = .some(image.flatMap { ImageCanvasView.makeCGImage($0) })
        strip.needsDisplay = true
    }

    /// "确定"、双击、回车：切到所选图片，关掉窗口。
    func choose() {
        if let index = selectedIndex {
            core.pickerChoose(index: UInt32(index))
        }
        window?.performClose(nil)
    }

    func setHideThumbnails(_ hide: Bool) {
        hideCheck.state = hide ? .on : .off
        hideToggled(hideCheck)
    }

    /// 排序菜单里的一项（自检用）：与用户在菜单里点它相同。
    func chooseSort(_ action: String) -> Bool {
        guard let i = sortOptions.firstIndex(where: { $0.action == action }) else { return false }
        _ = core.invokeAction(id: sortOptions[i].action)
        reload()
        return true
    }

    @objc private func sortChosen(_ sender: NSMenuItem) {
        guard sender.tag >= 0, sender.tag < sortOptions.count else { return }
        _ = chooseSort(sortOptions[sender.tag].action)
    }

    @objc private func hideToggled(_ sender: NSButton) {
        let hide = sender.state == .on
        strip.isHidden = hide
        stripTop?.isActive = !hide
        listTop?.isActive = hide
        core.setPickerHidesThumbnails(hide: hide)
        refreshStrip()
    }

    @objc private func okPressed(_ sender: Any?) {
        choose()
    }

    @objc private func doubleClicked(_ sender: Any?) {
        let row = table.clickedRow
        guard row >= 0 else { return }
        select(row)
        choose()
    }

    // MARK: 列表

    func numberOfRows(in tableView: NSTableView) -> Int {
        rows.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let column = tableColumn, row < rows.count else { return nil }
        let id = column.identifier
        let cell: NSTableCellView
        if let reused = tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView {
            cell = reused
        } else {
            cell = NSTableCellView()
            cell.identifier = id
            let label = NSTextField(labelWithString: "")
            label.translatesAutoresizingMaskIntoConstraints = false
            label.lineBreakMode = id.rawValue == "name" ? .byTruncatingMiddle : .byClipping
            label.alignment = id.rawValue == "size" ? .right : .left
            cell.addSubview(label)
            cell.textField = label
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 4),
                label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
                label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        let item = rows[row]
        switch id.rawValue {
        case "name": cell.textField?.stringValue = item.name
        case "modified": cell.textField?.stringValue = item.modified
        default: cell.textField?.stringValue = item.size
        }
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        if !updating {
            refreshStrip()
        }
    }

    // MARK: 窗口

    /// 放在主窗口的正中。
    func place(over parent: NSWindow?) {
        guard let window else { return }
        if let parent {
            let frame = window.frame
            window.setFrameOrigin(NSPoint(x: parent.frame.midX - frame.width / 2, y: parent.frame.midY - frame.height / 2))
        } else {
            window.center()
        }
    }

    func windowWillClose(_ notification: Notification) {
        // 不再要缩略图了。
        core.pickerWantThumbnails(paths: [])
        onClosed?()
    }

    /// 把窗口的内容画成 PNG（自检报告用）。
    func snapshot(to url: URL) -> [String: Any] {
        guard let window else { return ["error": "没有选择图片窗口"] }
        return windowSnapshot(window, to: url)
    }
}

/// 把一个窗口的内容画成 PNG（自检报告用）。内容视图本身是透明的（窗口背景由系统画）：先铺上窗口背景色再画内容，
/// 深色外观下的白字才看得见。
@MainActor
func windowSnapshot(_ window: NSWindow, to url: URL) -> [String: Any] {
    guard let view = window.contentView else { return ["error": "没有内容视图"] }
    view.layoutSubtreeIfNeeded()
    guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
        return ["error": "无法创建位图"]
    }
    view.cacheDisplay(in: view.bounds, to: rep)
    guard let output = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: rep.pixelsWide,
        pixelsHigh: rep.pixelsHigh,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    ), let context = NSGraphicsContext(bitmapImageRep: output) else {
        return ["error": "无法创建位图"]
    }
    let rect = NSRect(x: 0, y: 0, width: rep.pixelsWide, height: rep.pixelsHigh)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    window.effectiveAppearance.performAsCurrentDrawingAppearance {
        NSColor.windowBackgroundColor.setFill()
        rect.fill()
    }
    _ = rep.draw(in: rect)
    NSGraphicsContext.restoreGraphicsState()
    guard let png = output.representation(using: .png, properties: [:]) else {
        return ["error": "PNG 编码失败"]
    }
    do {
        try png.write(to: url)
        return ["file": url.lastPathComponent, "pixelsWide": output.pixelsWide, "pixelsHigh": output.pixelsHigh]
    } catch {
        return ["error": "\(error)"]
    }
}
