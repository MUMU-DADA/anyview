import Foundation
import AnyviewCore

/// 连接 Rust 核心（docs/ARCHITECTURE.md"唤醒与线程"）：
/// 核心在后台线程调用 wake，这里转到主线程，再取出事件交给界面。
/// 核心在建窗口之前就建好：调用 startDelivering 之前取出的事件先存着，之后按顺序送出。
@MainActor
final class CoreBridge {
    let core: AppCore
    /// 界面处理事件的回调，总在主线程调用。
    var onEvent: ((Event) -> Void)?
    private var delivering = false
    private var held: [Event] = []
    private(set) var lastPingMilliseconds: Double?
    private var pingSentAt: [UInt64: UInt64] = [:]
    private var nextToken: UInt64 = 1

    /// `sibling` 不为空时是同一程序里的又一个窗口：与它共用设置、图片缓存和后台线程。
    init(sibling: AppCore? = nil) {
        let relay = WeakRelay()
        let listener = WakeListener {
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    if let bridge = relay.target {
                        bridge.drainEvents()
                    }
                }
            }
        }
        if let sibling {
            core = sibling.newWindow(listener: listener)
        } else {
            core = AppCore(listener: listener)
        }
        relay.target = self
    }

    /// 发一个 Ping，验证"命令、事件、唤醒"的完整往返。
    func ping() {
        let token = nextToken
        nextToken += 1
        pingSentAt[token] = DispatchTime.now().uptimeNanoseconds
        core.dispatch(command: .ping(token: token))
    }

    /// 打开图片文件或文件夹。
    func open(path: String) {
        core.dispatch(command: .`open`(path: path))
    }

    func drainEvents() {
        let events = core.drainEvents()
        guard delivering else {
            held.append(contentsOf: events)
            return
        }
        deliver(events)
    }

    /// 界面准备好了：先送出存着的事件，之后随到随送。
    func startDelivering() {
        guard !delivering else { return }
        delivering = true
        let events = held
        held = []
        deliver(events)
        drainEvents()
    }

    private func deliver(_ events: [Event]) {
        for event in events {
            if case .pong(let token) = event, let sentAt = pingSentAt.removeValue(forKey: token) {
                lastPingMilliseconds = Double(DispatchTime.now().uptimeNanoseconds - sentAt) / 1_000_000
            }
            onEvent?(event)
        }
    }
}

/// 弱引用中转：核心持有监听器，监听器不能强引用 CoreBridge，否则会循环引用。
private final class WeakRelay: @unchecked Sendable {
    weak var target: CoreBridge?
}

/// 核心可能在任意后台线程调用 wake。
private final class WakeListener: CoreListener, @unchecked Sendable {
    private let handler: @Sendable () -> Void

    init(handler: @escaping @Sendable () -> Void) {
        self.handler = handler
    }

    func wake() {
        handler()
    }
}
