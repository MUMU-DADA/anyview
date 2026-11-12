import AppKit
import AnyviewCore

// 菜单由核心提供的数据生成（docs/PLATFORMS.md、docs/MENUS.md）。
// 菜单上的快捷键主要用来显示：不带 ⌘、⌃ 的按键（方向键、数字、字母）不由菜单执行，
// 而是交给主窗口按核心的键位处理，以免在其他窗口（如打开面板的搜索框）里打字时误触发菜单项。

/// 菜单项的目标：点击时交给核心执行；显示前向核心取可用与勾选状态（MENUS"启用规则"）。
@MainActor
final class MenuActions: NSObject, NSMenuItemValidation {
    /// 动作交给哪个窗口的核心（多窗口）：菜单栏是前面的窗口，右键菜单是弹出菜单的那个窗口。
    private let core: () -> AppCore?
    /// 执行动作，由那个窗口的控制器处理结果。
    var onAction: ((String) -> Void)?

    init(core: @escaping () -> AppCore?) {
        self.core = core
        super.init()
    }

    @objc func performAction(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        onAction?(id)
    }

    func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
        guard let id = menuItem.representedObject as? String else { return true }
        guard let core = core() else { return false }
        let state = core.actionState(id: id)
        menuItem.state = state.checked ? .on : .off
        return state.enabled
    }
}

/// 只让菜单执行带 ⌘ 或 ⌃ 的快捷键，其余按键继续交给窗口（见文件开头的说明）。
final class CoreMenu: NSMenu {
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.intersection([.command, .control]).isEmpty {
            return false
        }
        return super.performKeyEquivalent(with: event)
    }
}

@MainActor
enum MenuBuilder {
    /// 把扁平的菜单数据建成 NSMenu。mainMenu 为真时，把服务、窗口、帮助菜单交给系统管理，
    /// "打开最近使用"交给 recent 填充。
    static func build(
        _ entries: [MenuEntry],
        actions: MenuActions,
        mainMenu: Bool,
        recent: RecentMenuController? = nil
    ) -> NSMenu {
        let root = CoreMenu(title: "")
        var stack: [NSMenu] = [root]
        var roles: [MenuRole] = [.normal]
        for entry in entries {
            guard let current = stack.last else { break }
            switch entry {
            case let .action(id, title, shortcut, _):
                current.addItem(item(id: id, title: title, shortcut: shortcut, actions: actions))
            case .separator:
                current.addItem(.separator())
            case let .submenuStart(title, role):
                let submenu = CoreMenu(title: title)
                let holder = NSMenuItem(title: title, action: nil, keyEquivalent: "")
                holder.submenu = submenu
                current.addItem(holder)
                stack.append(submenu)
                roles.append(role)
                if mainMenu {
                    switch role {
                    case .services: NSApp.servicesMenu = submenu
                    case .window: NSApp.windowsMenu = submenu
                    case .help: NSApp.helpMenu = submenu
                    case .normal, .app, .edit, .recent: break
                    }
                }
            case .submenuEnd:
                if stack.count > 1 {
                    let finished = stack.removeLast()
                    if roles.removeLast() == .recent, mainMenu {
                        recent?.attach(to: finished)
                    }
                }
            }
        }
        return root
    }

    private static func item(id: String, title: String, shortcut: Shortcut?, actions: MenuActions) -> NSMenuItem {
        let item: NSMenuItem
        if id == "fullscreen" {
            // 全屏交给窗口：系统会在全屏时自动改成"退出全屏幕"，也不会再自动加一个全屏菜单项。
            item = NSMenuItem(title: title, action: #selector(NSWindow.toggleFullScreen(_:)), keyEquivalent: "")
        } else {
            item = NSMenuItem(title: title, action: #selector(MenuActions.performAction(_:)), keyEquivalent: "")
            item.target = actions
        }
        item.representedObject = id
        if let shortcut, let key = keyEquivalent(for: shortcut.key) {
            item.keyEquivalent = key
            item.keyEquivalentModifierMask = modifierFlags(shortcut.modifiers)
        }
        return item
    }

    static func modifierFlags(_ modifiers: KeyModifiers) -> NSEvent.ModifierFlags {
        var flags: NSEvent.ModifierFlags = []
        if modifiers.command { flags.insert(.command) }
        if modifiers.control { flags.insert(.control) }
        if modifiers.alt { flags.insert(.option) }
        if modifiers.shift { flags.insert(.shift) }
        return flags
    }

    /// 核心的按键名称 → NSMenuItem 的 keyEquivalent。
    static func keyEquivalent(for key: String) -> String? {
        let functionKeys: [String: Int] = [
            "Left": NSLeftArrowFunctionKey,
            "Right": NSRightArrowFunctionKey,
            "Up": NSUpArrowFunctionKey,
            "Down": NSDownArrowFunctionKey,
            "Home": NSHomeFunctionKey,
            "End": NSEndFunctionKey,
            "PageUp": NSPageUpFunctionKey,
            "PageDown": NSPageDownFunctionKey,
            "Delete": NSDeleteFunctionKey,
            "Insert": NSInsertFunctionKey,
        ]
        if let code = functionKeys[key] {
            return character(code)
        }
        if key.count > 1, key.hasPrefix("F"), let number = Int(key.dropFirst()), (1...35).contains(number) {
            return character(NSF1FunctionKey + number - 1)
        }
        switch key {
        case "Enter": return character(0x0D)
        case "Tab": return character(0x09)
        case "Escape": return character(0x1B)
        case "Backspace": return character(0x08)
        case "Space": return " "
        default: return key.count == 1 ? key.lowercased() : nil
        }
    }

    static func character(_ code: Int) -> String {
        String(utf16CodeUnits: [unichar(code)], count: 1)
    }

    /// 自检报告用：把菜单写成文字，每项一行"标题  快捷键"，子菜单缩进，灰显和勾选都标出。
    static func describe(_ menu: NSMenu, depth: Int = 0) -> [String] {
        // 让每一项按当前状态校验一次，得到可用与勾选状态。
        menu.update()
        let indent = String(repeating: "  ", count: depth)
        var lines: [String] = []
        for item in menu.items {
            if item.isSeparatorItem {
                lines.append(indent + "——")
            } else if let submenu = item.submenu {
                lines.append(indent + item.title + " ›")
                lines += describe(submenu, depth: depth + 1)
            } else {
                let mark = item.state == .on ? "✓ " : ""
                let key = shortcutText(item)
                let grey = item.isEnabled ? "" : "（灰显）"
                lines.append("\(indent)\(mark)\(item.title)\(key.isEmpty ? "" : "  \(key)")\(grey)")
            }
        }
        return lines
    }

    /// 菜单项上实际设置的快捷键（⌃⌥⇧⌘ 加按键）。
    static func shortcutText(_ item: NSMenuItem) -> String {
        guard !item.keyEquivalent.isEmpty else { return "" }
        let flags = item.keyEquivalentModifierMask
        var text = ""
        if flags.contains(.control) { text += "⌃" }
        if flags.contains(.option) { text += "⌥" }
        if flags.contains(.shift) { text += "⇧" }
        if flags.contains(.command) { text += "⌘" }
        let glyphs: [Int: String] = [
            NSLeftArrowFunctionKey: "←",
            NSRightArrowFunctionKey: "→",
            NSUpArrowFunctionKey: "↑",
            NSDownArrowFunctionKey: "↓",
            NSHomeFunctionKey: "↖",
            NSEndFunctionKey: "↘",
            NSPageUpFunctionKey: "⇞",
            NSPageDownFunctionKey: "⇟",
            NSDeleteFunctionKey: "⌦",
            0x0D: "↩",
            0x09: "⇥",
            0x1B: "⎋",
            0x08: "⌫",
        ]
        let unit = Int(item.keyEquivalent.utf16.first ?? 0)
        return text + (glyphs[unit] ?? item.keyEquivalent.uppercased())
    }
}

/// "打开最近使用"子菜单：每次打开时按系统记录的最近文档重建（最多数量由系统设置决定），
/// 下面保留核心给出的项目（"清除菜单"）。
@MainActor
final class RecentMenuController: NSObject, NSMenuDelegate {
    /// 打开选中的最近文件或文件夹。
    var onOpen: ((URL) -> Void)?
    private var fixedItems: [NSMenuItem] = []

    func attach(to menu: NSMenu) {
        fixedItems = menu.items
        menu.delegate = self
    }

    @objc func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let urls = NSDocumentController.shared.recentDocumentURLs
        for url in urls {
            let item = NSMenuItem(
                title: FileManager.default.displayName(atPath: url.path(percentEncoded: false)),
                action: #selector(openRecent(_:)),
                keyEquivalent: ""
            )
            item.target = self
            item.representedObject = url
            let icon = NSWorkspace.shared.icon(forFile: url.path(percentEncoded: false))
            icon.size = NSSize(width: 16, height: 16)
            item.image = icon
            menu.addItem(item)
        }
        if !urls.isEmpty {
            menu.addItem(.separator())
        }
        for item in fixedItems {
            menu.addItem(item)
        }
    }

    @objc private func openRecent(_ sender: NSMenuItem) {
        if let url = sender.representedObject as? URL {
            onOpen?(url)
        }
    }
}

/// 按键事件 → 核心的按键名称和修饰键（docs/ACTIONS.md）。
@MainActor
enum KeyNames {
    /// 有名字的按键用名字；其余按键取含 ⇧ 效果、不含其他修饰键的字符（如 ⇧= 得到 +）。
    static func name(of event: NSEvent) -> String? {
        if let special = event.specialKey {
            switch special {
            case .leftArrow: return "Left"
            case .rightArrow: return "Right"
            case .upArrow: return "Up"
            case .downArrow: return "Down"
            case .home: return "Home"
            case .end: return "End"
            case .pageUp: return "PageUp"
            case .pageDown: return "PageDown"
            case .carriageReturn, .enter, .newline: return "Enter"
            case .tab, .backTab: return "Tab"
            case .delete, .backspace: return "Backspace"
            case .deleteForward: return "Delete"
            case .insert: return "Insert"
            case .f1: return "F1"
            case .f2: return "F2"
            case .f3: return "F3"
            case .f4: return "F4"
            case .f5: return "F5"
            case .f6: return "F6"
            case .f7: return "F7"
            case .f8: return "F8"
            case .f9: return "F9"
            case .f10: return "F10"
            case .f11: return "F11"
            case .f12: return "F12"
            default: break
            }
        }
        // Esc 键（虚拟键码 53）。
        if event.keyCode == 53 {
            return "Escape"
        }
        guard let characters = event.charactersIgnoringModifiers, characters.count == 1 else {
            return nil
        }
        return characters == " " ? "Space" : characters
    }

    static func modifiers(of event: NSEvent) -> KeyModifiers {
        let flags = event.modifierFlags
        return KeyModifiers(
            shift: flags.contains(.shift),
            control: flags.contains(.control),
            alt: flags.contains(.option),
            command: flags.contains(.command)
        )
    }
}
