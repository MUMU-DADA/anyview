import AppKit
import AnyviewCore

/// 帮助窗口：Esc 和关闭按钮一样。
private final class HelpWindow: NSWindow {
    var onCancel: (() -> Void)?

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }
}

/// 帮助窗口："键盘快捷键""支持的格式"（"帮助"菜单）：两列的表，分组标题加粗，内容由核心给出
/// （快捷键是实际生效的按键，含键盘映射）。"开源许可"没有表，是核心给的一段文字（许可证原文）。
/// 只读，文字可以选中复制。
@MainActor
final class HelpWindowController: NSWindowController, NSWindowDelegate {
    let kind: HelpKind
    private(set) var rows: [HelpRow] = []
    /// 开源许可那样只有一段文字的帮助窗口：那段文字；表格式的为空。
    private(set) var text: String?
    var onClosed: (() -> Void)?

    init(kind: HelpKind, core: AppCore, lang: Lang) {
        self.kind = kind
        let text = core.helpText(kind: kind, lang: lang)
        // 许可证原文按 80 列排版：等宽字体，窗口放得下 80 列。
        let window = HelpWindow(
            contentRect: NSRect(x: 0, y: 0, width: text == nil ? 560 : 680, height: 600),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = core.helpTitle(kind: kind, lang: lang)
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 360, height: 240)
        super.init(window: window)
        window.delegate = self
        window.onCancel = { [weak self] in
            self?.window?.performClose(nil)
        }
        rows = core.helpRows(kind: kind, lang: lang)
        self.text = text
        let scroll = NSTextView.scrollableTextView()
        scroll.borderType = .noBorder
        if let textView = scroll.documentView as? NSTextView {
            textView.isEditable = false
            textView.isSelectable = true
            textView.drawsBackground = false
            textView.textContainerInset = NSSize(width: 16, height: 12)
            if let text {
                textView.textStorage?.setAttributedString(NSAttributedString(string: text, attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular),
                    .foregroundColor: NSColor.labelColor,
                ]))
            } else {
                textView.textStorage?.setAttributedString(Self.text(rows))
            }
        }
        window.contentView = scroll
        window.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    /// 表的文字：右列从制表位开始，折行时也对齐到右列。
    private static func text(_ rows: [HelpRow]) -> NSAttributedString {
        let column: CGFloat = 230
        let result = NSMutableAttributedString()
        let body = NSMutableParagraphStyle()
        body.tabStops = [NSTextTab(textAlignment: .left, location: column)]
        body.headIndent = column
        body.lineSpacing = 3
        let heading = NSMutableParagraphStyle()
        heading.paragraphSpacingBefore = 10
        heading.paragraphSpacing = 2
        for row in rows {
            if row.heading {
                result.append(NSAttributedString(string: row.left + "\n", attributes: [
                    .font: NSFont.boldSystemFont(ofSize: 13),
                    .foregroundColor: NSColor.labelColor,
                    .paragraphStyle: heading,
                ]))
            } else {
                result.append(NSAttributedString(string: "\(row.left)\t\(row.right)\n", attributes: [
                    .font: NSFont.systemFont(ofSize: 13),
                    .foregroundColor: NSColor.labelColor,
                    .paragraphStyle: body,
                ]))
            }
        }
        return result
    }

    func windowWillClose(_ notification: Notification) {
        onClosed?()
    }

    /// 把窗口的内容画成 PNG（自检报告用）。
    func snapshot(to url: URL) -> [String: Any] {
        guard let window else { return ["error": "没有帮助窗口"] }
        return windowSnapshot(window, to: url)
    }
}
