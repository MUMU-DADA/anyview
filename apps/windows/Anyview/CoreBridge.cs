// 连接 Rust 核心（docs/ARCHITECTURE.md"唤醒与线程"）：核心在后台线程调用 Wake，这里转到界面线程，
// 再取出事件交给窗口。核心在建窗口之前就建好：StartDelivering 之前取出的事件先存着，之后按顺序送出。
using System;
using System.Collections.Generic;
using System.Threading;
using Anyview.Core;
using Microsoft.UI.Dispatching;

namespace Anyview;

internal sealed class CoreBridge
{
    public AppCore Core { get; }
    /// <summary>窗口处理事件的回调，总在界面线程调用。</summary>
    public Action<Event>? OnEvent;
    private readonly WakeListener _listener;
    private readonly List<Event> _held = new();
    private bool _delivering;

    /// <param name="sibling">不为空时是同一程序里的又一个窗口：共用设置、图片缓存和后台线程。</param>
    public CoreBridge(AppCore? sibling)
    {
        _listener = new WakeListener(this);
        Core = sibling is null ? new AppCore(_listener) : sibling.NewWindow(_listener);
    }

    /// <summary>界面线程的调度队列：界面建好之前为空，唤醒只记一下，建好后一起取。</summary>
    public DispatcherQueue? Queue
    {
        get => _listener.Queue;
        set => _listener.Queue = value;
    }

    public void Open(string path) => Core.Dispatch(new Command.Open(path));

    private readonly System.Diagnostics.Stopwatch _ping = new();
    private ulong _pingToken;

    /// <summary>界面到核心往返一次（空态的状态行显示用时）。</summary>
    public void Ping()
    {
        _pingToken++;
        _ping.Restart();
        Core.Dispatch(new Command.Ping(_pingToken));
    }

    /// <summary>最近一次 Ping 往返的毫秒数（收到 Pong 时读）。</summary>
    public double? LastPingMs => _ping.IsRunning ? _ping.Elapsed.TotalMilliseconds : null;

    /// <summary>窗口关掉了（Detach 以后）：不再取事件、不再送出。</summary>
    public bool IsDetached { get; private set; }

    /// <summary>窗口关掉了：不再送事件（之后核心的唤醒都不理）。核心先留着，Close 时才放掉。</summary>
    public void Detach()
    {
        IsDetached = true;
        OnEvent = null;
        foreach (var e in _held)
        {
            Release(e);
        }
        _held.Clear();
    }

    /// <summary>不用的事件：里面的图像句柄马上释放（背后是核心里的整张图，不等垃圾回收）。</summary>
    private static void Release(Event e)
    {
        switch (e)
        {
            case Event.ImageReady r: r.Image.Dispose(); break;
            case Event.ImagePreview p: p.Image.Dispose(); break;
            case Event.ImageUpdated u: u.Image.Dispose(); break;
            case Event.AnimationFrame f: f.Image.Dispose(); break;
            case Event.ExactReady x: x.Image.Dispose(); break;
            case Event.Thumbnail t: t.Image?.Dispose(); break;
        }
    }

    /// <summary>放掉核心（窗口关掉、设置窗口也不再用它以后）：连同它的图片、浏览集合一起放掉，不等垃圾回收。</summary>
    public void Close()
    {
        Detach();
        Core.Dispose();
    }

    public void Drain()
    {
        if (IsDetached)
        {
            return;
        }
        Diag.Drains++;
        var events = Core.DrainEvents();
        if (!_delivering)
        {
            _held.AddRange(events);
            return;
        }
        Deliver(events);
    }

    /// <summary>界面准备好了：先送出存着的事件，之后随到随送。</summary>
    public void StartDelivering()
    {
        if (_delivering)
        {
            return;
        }
        _delivering = true;
        var events = _held.ToArray();
        _held.Clear();
        Deliver(events);
        Drain();
    }

    private void Deliver(IEnumerable<Event> events)
    {
        foreach (var e in events)
        {
            try
            {
                if (OnEvent is { } handler)
                {
                    handler(e);
                }
                else
                {
                    // 窗口已经关掉（这一批事件处理到一半时关的）：事件里的图像马上释放。
                    Release(e);
                }
            }
            catch (Exception error)
            {
                Crash.Write("CoreBridge.Deliver", error);
            }
        }
    }

    /// <summary>核心可能在任意后台线程调用 Wake：转到界面线程取事件（合并多次唤醒）。</summary>
    private sealed class WakeListener : CoreListener
    {
        private readonly WeakReference<CoreBridge> _bridge;
        private int _pending;
        public volatile DispatcherQueue? Queue;

        public WakeListener(CoreBridge bridge)
        {
            _bridge = new WeakReference<CoreBridge>(bridge);
        }

        public void Wake()
        {
            var queue = Queue;
            if (queue is null || Interlocked.Exchange(ref _pending, 1) == 1)
            {
                return;
            }
            queue.TryEnqueue(() =>
            {
                Interlocked.Exchange(ref _pending, 0);
                try
                {
                    if (_bridge.TryGetTarget(out var bridge))
                    {
                        bridge.Drain();
                    }
                }
                catch (Exception error)
                {
                    Crash.Write("CoreBridge.Wake", error);
                }
            });
        }
    }
}

/// <summary>
/// 界面线程上的延时。DispatcherQueueTimer 要有人引用着：只有局部变量引用时，.NET 回收它的包装对象后计时器就不再触发
/// （Windows 上实测：自检卡在半路、等新图像的超时也不触发、精确重绘一直不来）。这里把还没到点的计时器存着，到点后放掉。
/// 只在界面线程调用。
/// </summary>
internal static class Delay
{
    private static readonly HashSet<DispatcherQueueTimer> s_pending = new();

    public static void After(DispatcherQueue queue, uint ms, Action action)
    {
        var timer = queue.CreateTimer();
        timer.Interval = TimeSpan.FromMilliseconds(ms);
        timer.IsRepeating = false;
        timer.Tick += (_, _) =>
        {
            timer.Stop();
            s_pending.Remove(timer);
            Diag.DelayTicks++;
            // 计时器回调里漏出去的异常会让程序直接崩溃（WinUI 不交给 UnhandledException）：在这里接住、记下来。
            try
            {
                action();
            }
            catch (Exception error)
            {
                Crash.Write("Delay.After", error);
            }
        };
        s_pending.Add(timer);
        timer.Start();
    }
}

/// <summary>
/// 关窗口时的几步收尾（WinUI 不回收关掉的窗口对象，窗口拿着的大东西要自己放掉，见 CanvasView.Release）。平常每一步都做；
/// 自检的对照用环境变量 ANYVIEW_TEST_LEAK 列出不做的步骤（逗号分隔，all 是都不做），好量出每一步管不管用。
/// </summary>
internal static class CloseCleanup
{
    private static string[] s_skip = Array.Empty<string>();

    /// <summary>自检开始时读一次环境变量（平常不读）。</summary>
    public static void LoadForSelfTest()
    {
        s_skip = (Environment.GetEnvironmentVariable("ANYVIEW_TEST_LEAK") ?? "")
            .Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
    }

    public static bool Does(string step) => Array.IndexOf(s_skip, step) < 0 && Array.IndexOf(s_skip, "all") < 0;

    public static string Skipped => string.Join(",", s_skip);
}

/// <summary>
/// 界面线程上的几种回调各来了几次（自检：静止时 CPU 占用不是 0，看是谁在动）。只在界面线程上加，不用加锁。
/// </summary>
internal static class Diag
{
    public static int Drains;
    public static int DelayTicks;
    public static int PointerMoves;
    public static int WindowChanges;
    /// <summary>UI 自动化（读屏、输入法、远程桌面、取词工具用的接口）来问主窗口内容的次数。</summary>
    public static int AutomationQueries;
    /// <summary>XAML 的根（XamlRoot）报告变化的次数、画布改大小的次数（自检：移动窗口时不该有）。</summary>
    public static int RootChanges;
    public static int CanvasResizes;
    /// <summary>所有窗口的画布一共重画了几次（拖动记录用）。</summary>
    public static int CanvasDraws;

    [System.Runtime.InteropServices.DllImport("UIAutomationCore.dll")]
    private static extern int UiaClientsAreListening();

    /// <summary>系统里有没有程序在用 UI 自动化（有的话它们会来问各窗口的内容，占界面线程的时间）。</summary>
    public static bool AutomationClientsListening()
    {
        try
        {
            return UiaClientsAreListening() != 0;
        }
        catch (Exception)
        {
            return false;
        }
    }
}

/// <summary>
/// 主窗口最外层的容器（拿键盘焦点的那个）。UI 自动化来问它时记一次数（自检：静止时界面线程的时间有一部分花在应答 UI 自动化上）。
/// </summary>
internal sealed partial class HostControl : Microsoft.UI.Xaml.Controls.UserControl
{
    protected override Microsoft.UI.Xaml.Automation.Peers.AutomationPeer OnCreateAutomationPeer() => new Peer(this);

    private sealed partial class Peer : Microsoft.UI.Xaml.Automation.Peers.FrameworkElementAutomationPeer
    {
        public Peer(HostControl owner) : base(owner)
        {
        }

        protected override string GetClassNameCore()
        {
            Diag.AutomationQueries++;
            return "AnyviewImage";
        }

        protected override string GetNameCore()
        {
            Diag.AutomationQueries++;
            return base.GetNameCore();
        }

        protected override Microsoft.UI.Xaml.Automation.Peers.AutomationControlType GetAutomationControlTypeCore()
        {
            Diag.AutomationQueries++;
            return Microsoft.UI.Xaml.Automation.Peers.AutomationControlType.Pane;
        }

        protected override System.Collections.Generic.IList<Microsoft.UI.Xaml.Automation.Peers.AutomationPeer>? GetChildrenCore()
        {
            Diag.AutomationQueries++;
            return base.GetChildrenCore();
        }

        protected override Windows.Foundation.Rect GetBoundingRectangleCore()
        {
            Diag.AutomationQueries++;
            return base.GetBoundingRectangleCore();
        }

        protected override bool HasKeyboardFocusCore()
        {
            Diag.AutomationQueries++;
            return base.HasKeyboardFocusCore();
        }
    }
}
