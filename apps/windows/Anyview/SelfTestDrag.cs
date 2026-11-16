// 自检真实鼠标拖动测试（drag_real），用于回归验证拖动流畅度、首拖延迟与松开跟手问题：
// 用 SendInput 移动鼠标、按下、拖一圈、松开（与手拖一样经过系统的输入），每毫秒动 1 像素：拖图片（程序自己移动窗口）、
// 拖顶端那一条（登记成了标题栏，由系统移动）、拖一个普通的 Win32 窗口的标题栏（对照：同一台电脑上系统移动窗口有多顺）、
// 再拖一次图片（第一次有没有特别慢）、拉顶边改大小。每次记下：按下到开始移动多久、窗口落后光标多少、每步间隔、卡住几次、
// 松开后多久停下（停不下来就是"跟着光标走"）、这段时间界面线程忙在哪里（DragProbe）。
// 最后核对：左键松开以后才轮到程序开始移动时不开始（不会跟着光标走）；列出加载了 UI 自动化的程序（谁在一直问窗口）。
// 实测对比：系统原生标题栏拖动按下约 6 ms 即响应，而直接发送 WM_NCLBUTTONDOWN 的
// 0.5–0.7 秒后才开始、窗口一步也没动、松开后停不下来——WinUI 程序里那样开始的移动收不到鼠标消息，所以改成上面的办法。
// 自检期间鼠标会自己动几秒，不要碰鼠标。
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;

namespace Anyview;

internal sealed partial class SelfTest
{
    /// <summary>一次注入的拖动的结果（注入的线程上量的部分）。</summary>
    private sealed class InjectedDrag
    {
        public string Name = "";
        public string? Note;
        /// <summary>一定要开始移动（不然算不通过）。模拟的鼠标拖顶端那一条时系统可能当作单击，那一项只记录。</summary>
        public bool Required = true;
        /// <summary>移动开始了（程序自己移动，或系统的移动循环）。</summary>
        public bool Entered;
        public double? DownToLoopMs;
        /// <summary>窗口落后光标多少像素（光标每毫秒走 1 像素，也就是落后多少毫秒）。</summary>
        public double? LagMedian;
        public double? Lag95;
        public double? LagMax;
        /// <summary>松开时、停下以后窗口离起点多远（光标回到了起点，窗口也应回到原处）。</summary>
        public int? EndErrorPx;
        /// <summary>开始移动时光标已经离按下的地方多远（按下以后程序晚处理了多少，换成光标走过的像素）。</summary>
        public int? SlipPx;
        public double? ReleaseToStopMs;
        /// <summary>松开 1.5 秒以后还在移动循环里（窗口跟着光标走），由自检取消掉了。</summary>
        public bool StillFollowing;
        public MoveResult? Result;

        public Dictionary<string, object?> ToRecord()
        {
            var record = new Dictionary<string, object?>
            {
                ["name"] = Name,
                ["note"] = Note,
                ["required"] = Required,
                ["entered"] = Entered,
                ["downToLoopMs"] = Round(DownToLoopMs),
                ["lagMsMedian"] = Round(LagMedian),
                ["lagMs95"] = Round(Lag95),
                ["lagMsMax"] = Round(LagMax),
                ["endErrorPx"] = EndErrorPx,
                ["slipPx"] = SlipPx,
                ["releaseToStopMs"] = Round(ReleaseToStopMs),
                ["stillFollowing"] = StillFollowing,
            };
            if (Result is not null)
            {
                record["loop"] = Result.ToRecord();
            }
            record["text"] = Text();
            return record;
        }

        public string Text()
        {
            if (Note is not null && !Entered)
            {
                return $"{Name}：{Note}";
            }
            var text = $"{Name}：按下到开始移动 {Ms(DownToLoopMs)}（那时光标已经走了 {SlipPx?.ToString() ?? "？"} 像素）；" +
                $"之后窗口落后光标 中位 {Ms(LagMedian)}、95% {Ms(Lag95)}、最多 {Ms(LagMax)}；" +
                $"松开后 {Ms(ReleaseToStopMs)} 停下，离原处 {EndErrorPx?.ToString() ?? "？"} 像素" +
                (StillFollowing ? "；松开 1.5 秒后还在跟着光标走（自检取消了）" : "");
            if (Result is not null)
            {
                text += $"；{Result.Text()}";
            }
            if (Note is not null)
            {
                text += $"；{Note}";
            }
            return text;
        }

        private static string Ms(double? value) => value is double v ? $"{v:0.#} ms" : "（没量到）";
        private static double? Round(double? value) => value is double v ? Math.Round(v, 1) : null;
    }

    /// <summary>drag_real：见文件开头。</summary>
    private async Task RealDrags(string id)
    {
        var drags = new List<InjectedDrag>();
        var scale = _window.DpiScale;
        if (!_window.WindowMovable)
        {
            Fail($"{id}：窗口现在不能移动（最大化或全屏）");
            return;
        }
        async Task Front()
        {
            _window.Activate();
            Win32.SetForegroundWindow(_window.Hwnd);
            await Task.Delay(300);
        }
        Win32.POINT Center()
        {
            DragProbe.GetWindowRect(_window.Hwnd, out var r);
            return new Win32.POINT { X = (r.Left + r.Right) / 2, Y = (r.Top + r.Bottom) / 2 };
        }
        Win32.POINT Strip()
        {
            // 顶端那一条（高 32 有效像素，最上面 4 个是改大小的带子），离右上角的按钮远一点。
            DragProbe.GetWindowRect(_window.Hwnd, out var r);
            return new Win32.POINT { X = r.Left + (r.Right - r.Left) * 3 / 10, Y = r.Top + (int)Math.Round(16 * scale) };
        }
        var hwnd = _window.Hwnd;
        // 量完把窗口放回原处、原来的大小（按图片定的大小，；不然之后核对图片区大小会不对）。
        DragProbe.GetWindowRect(hwnd, out var saved);
        // 先把窗口改成 640x480 有效像素，放到屏幕可用区域的中间偏左上（离顶边至少 120 像素）：光标走的一圈（向右、向下各
        // 150 像素）、拉顶边（向上 60 像素）都碰不到屏幕边缘，不会触发贴靠（测试用的大图让窗口几乎和屏幕一样高）。
        var area = Win32.WorkAreaOfWindow(hwnd);
        var width = (int)Math.Round(640 * scale);
        var height = (int)Math.Round(480 * scale);
        _window.AppWindow.MoveAndResize(new Windows.Graphics.RectInt32(
            area.X + Math.Max((area.Width - width) / 2 - 75, 0),
            area.Y + Math.Max((area.Height - height) / 2 - 75, 120),
            width, height));
        await Task.Delay(500);
        await Front();
        drags.Add(await Task.Run(() => InjectDrag("图片（第一次）", hwnd, Center(), DragProbe.Main)));
        await Task.Delay(400);
        await Front();
        // 顶端那一条（登记成了标题栏，系统处理）：问窗口那一点、最上沿是什么（命中测试，应是标题栏、上边框），再像人手一样
        // 从旁边滑过去、按下后停 0.1 秒再拖。模拟的鼠标在这里系统认不出是拖动，松开后才把按下当作单击转给程序，
        // 所以这一项只记录，不算不通过。
        var stripPoint = Strip();
        var hitStrip = Inject.HitTest(hwnd, stripPoint);
        var hitTop = Inject.HitTest(hwnd, new Win32.POINT { X = stripPoint.X, Y = stripPoint.Y - (int)Math.Round(14 * scale) });
        var strip = await Task.Run(() => InjectDrag("顶端那一条（模拟的鼠标，按下后停 0.1 秒再拖）", hwnd, stripPoint, DragProbe.Main, holdMs: 100, glide: true));
        strip.Required = false;
        drags.Add(strip);
        await Task.Delay(400);
        // 对照：普通的 Win32 窗口（自己的线程，不经过 WinUI），放在主窗口的位置，拖它的标题栏。
        DragProbe.GetWindowRect(hwnd, out var main);
        var plain = await Task.Run(() => PlainWindow.Open(main.Left, main.Top, Math.Max(main.Right - main.Left, 400), Math.Max(main.Bottom - main.Top, 300)));
        if (plain is null)
        {
            drags.Add(new InjectedDrag { Name = "对照：普通 Win32 窗口", Note = "建不了对照窗口" });
        }
        else
        {
            await Task.Delay(400);
            drags.Add(await Task.Run(() =>
            {
                var start = plain.CaptionPoint();
                return start is Win32.POINT p
                    ? InjectDrag("对照：普通 Win32 窗口", plain.Hwnd, p, plain.Recorder)
                    : new InjectedDrag { Name = "对照：普通 Win32 窗口", Note = "找不到对照窗口的标题栏" };
            }));
            await Task.Run(plain.Close);
        }
        await Front();
        drags.Add(await Task.Run(() => InjectDrag("图片（再一次）", hwnd, Center(), DragProbe.Main)));
        await Task.Delay(400);
        // 拉顶边改大小（登记成了系统的上边框；没生效时程序自己改）。
        await Front();
        var resize = await Task.Run(() => InjectResize(hwnd, scale));
        await Task.Delay(300);

        // 左键已经松开以后才轮到程序开始移动：不该开始（不然窗口一直跟着光标走）。
        var guard = GuardCheck();
        _window.AppWindow.MoveAndResize(new Windows.Graphics.RectInt32(saved.Left, saved.Top, saved.Right - saved.Left, saved.Bottom - saved.Top));
        await Task.Delay(500);
        var uia = await Task.Run(UiaProcesses);
        if (_finished)
        {
            return;
        }
        foreach (var drag in drags.Where(d => d.StillFollowing))
        {
            _problems.Add($"{id}: {drag.Name}：松开以后窗口还跟着光标走");
        }
        if (guard.Problem is string problem)
        {
            _problems.Add($"{id}: {problem}");
        }
        if (resize.Problem is string resizeProblem)
        {
            _problems.Add($"{id}: {resizeProblem}");
        }
        Record(id, new()
        {
            ["realDrags"] = drags.Select(d => d.ToRecord()).ToList(),
            ["resize"] = resize.Text,
            ["resizeOk"] = resize.Problem is null,
            ["nonClient"] = _window.NonClientState,
            ["hitTest"] = $"窗口说顶端那一条是「{Inject.HitTestName(hitStrip)}」、最上沿是「{Inject.HitTestName(hitTop)}」（应是标题栏、上边框）",
            ["hitTestOk"] = hitStrip == 2 && hitTop == 12,
            ["guard"] = guard.Text,
            ["uiaProcesses"] = uia,
            ["dragEnvironment"] = DragProbe.SystemInfo(),
        });
    }

    /// <summary>
    /// 在注入的线程上拖一次：光标移到 start，按下，每毫秒动 1 像素走一圈（右、下、左、上各 150 像素，回到起点），松开。
    /// 每一步之前看窗口在哪里（不经过界面线程），算窗口落后光标多少。recorder 是那个窗口的移动循环记录。
    /// </summary>
    /// <param name="holdMs">按下以后停多久再动（像人手一样；0 是按下马上就动）。</param>
    /// <param name="glide">光标从左边 40 像素外一点一点滑到起点（像人手一样），不是一下跳过去。</param>
    private static InjectedDrag InjectDrag(string name, nint hwnd, Win32.POINT start, MoveRecorder recorder, int holdMs = 0, bool glide = false)
    {
        var drag = new InjectedDrag { Name = name };
        if (!DragProbe.GetWindowRect(hwnd, out var origin))
        {
            drag.Note = "读不到窗口位置";
            return drag;
        }
        if (glide)
        {
            for (var i = 40; i > 0; i -= 2)
            {
                Inject.MoveTo(start.X - i, start.Y);
                Thread.Sleep(2);
            }
        }
        if (!Inject.MoveTo(start.X, start.Y))
        {
            drag.Note = "移动鼠标失败（SendInput 被拦下，屏幕锁着？）";
            return drag;
        }
        Thread.Sleep(250);
        Win32.GetCursorPos(out var actual);
        if (Math.Abs(actual.X - start.X) > 3 || Math.Abs(actual.Y - start.Y) > 3)
        {
            drag.Note = $"鼠标没到起点（在 {actual.X},{actual.Y}，应在 {start.X},{start.Y}；自检时碰了鼠标？）";
            return drag;
        }
        if (Inject.RootWindowAt(actual) != hwnd)
        {
            drag.Note = "起点不在要拖的窗口上（被别的窗口挡住了？）";
            return drag;
        }
        var completed = recorder.Completed;
        var notStarted = DragProbe.NotStarted;
        var frequency = Stopwatch.Frequency;
        var lags = new List<double>();
        long down, up;
        // 系统从哪里算起窗口跟着光标走了多少：程序交给系统时光标所在的地方（按下以后晚处理了，光标已经走了一段）；
        // 系统自己处理的标题栏从按下的地方算起。移动开始以后才知道。
        Win32.POINT? anchor = null;
        try
        {
            down = Stopwatch.GetTimestamp();
            Inject.Button(true);
            var moving = down + holdMs * frequency / 1000;
            int x = actual.X, y = actual.Y;
            const int steps = 600;
            for (var i = 1; i <= steps; i++)
            {
                Inject.WaitUntil(moving + i * frequency / 1000);
                if (anchor is null && recorder.Active)
                {
                    anchor = DragProbe.AnchorSince(down) ?? actual;
                }
                // 开头 30 毫秒是移动循环开始的时间，不算落后。
                if (i > 30 && anchor is Win32.POINT a && DragProbe.GetWindowRect(hwnd, out var now))
                {
                    var expectedX = origin.Left + x - a.X;
                    var expectedY = origin.Top + y - a.Y;
                    lags.Add(Math.Max(Math.Abs(now.Left - expectedX), Math.Abs(now.Top - expectedY)));
                }
                (x, y) = i switch
                {
                    <= 150 => (actual.X + i, actual.Y),
                    <= 300 => (actual.X + 150, actual.Y + i - 150),
                    <= 450 => (actual.X + 150 - (i - 300), actual.Y + 150),
                    _ => (actual.X, actual.Y + 150 - (i - 450)),
                };
                Inject.MoveTo(x, y);
            }
            Inject.WaitUntil(Stopwatch.GetTimestamp() + frequency * 40 / 1000);
            up = Stopwatch.GetTimestamp();
            Inject.Button(false);
        }
        finally
        {
            // 出了异常也不让左键一直按着。
            Inject.ReleaseIfDown();
        }
        // 等它停下、结果补齐（采样在线程池上收尾），最多 3 秒。松开 1.5 秒以后还在移动循环里（包括很晚才开始的），
        // 就是"窗口跟着光标走"了：取消掉。
        var followLimit = up + frequency * 3 / 2;
        var deadline = up + frequency * 3;
        while (Stopwatch.GetTimestamp() < deadline)
        {
            if (recorder.Active)
            {
                if (Stopwatch.GetTimestamp() > followLimit)
                {
                    drag.StillFollowing = true;
                    Rescue(hwnd, recorder);
                }
            }
            else if (recorder.Completed != completed || DragProbe.NotStarted != notStarted)
            {
                break;
            }
            Thread.Sleep(5);
        }
        if (recorder.Active)
        {
            drag.StillFollowing = true;
            Rescue(hwnd, recorder);
        }
        if (DragProbe.GetWindowRect(hwnd, out var end))
        {
            drag.EndErrorPx = Math.Max(Math.Abs(end.Left - origin.Left), Math.Abs(end.Top - origin.Top));
        }
        if (lags.Count > 0)
        {
            lags.Sort();
            drag.LagMedian = lags[lags.Count / 2];
            drag.Lag95 = lags[Math.Min((int)(lags.Count * 0.95), lags.Count - 1)];
            drag.LagMax = lags[^1];
        }
        if (anchor is Win32.POINT used)
        {
            drag.SlipPx = Math.Max(Math.Abs(used.X - actual.X), Math.Abs(used.Y - actual.Y));
        }
        if (DragProbe.NotStarted != notStarted)
        {
            drag.Note = $"没有开始移动：{DragProbe.LastNotStarted}";
        }
        if (recorder.Completed != completed && recorder.Last is MoveResult result && result.EnterTicks >= down)
        {
            drag.Entered = true;
            drag.Result = result;
            drag.DownToLoopMs = MoveRecorder.Ms(down, result.EnterTicks);
            drag.ReleaseToStopMs = MoveRecorder.Ms(up, result.ExitTicks);
        }
        else
        {
            drag.Note ??= "系统的移动没有开始（按下以后窗口没有进入移动）";
        }
        return drag;
    }

    /// <summary>窗口还在系统的移动里（跟着光标走）：先取消，还停不下来就点一下（这种移动方式点一下就停）。</summary>
    private static void Rescue(nint hwnd, MoveRecorder recorder)
    {
        Inject.CancelMode(hwnd);
        Thread.Sleep(300);
        if (recorder.Active)
        {
            Inject.Button(true);
            Inject.Button(false);
            Thread.Sleep(300);
        }
    }

    /// <summary>左键松开以后才轮到程序开始移动窗口：应不开始（TryStartMoveForTest 返回 false，记一次"没有开始移动"）。</summary>
    private (string Text, string? Problem) GuardCheck()
    {
        if (Win32.LeftButtonDown())
        {
            var held = "左键按着（自检时碰了鼠标？），没有核对";
            return (held, held);
        }
        var notStarted = DragProbe.NotStarted;
        var started = _window.TryStartMoveForTest();
        if (!started && DragProbe.NotStarted != notStarted)
        {
            return ("左键松开以后才开始拖动：没有开始移动（窗口不会跟着光标走）", null);
        }
        var text = started ? "左键松开以后才开始拖动：开始了移动（会一直跟着光标走）" : "左键松开以后才开始拖动：没有记下来";
        return (text, text);
    }

    /// <summary>
    /// 拉顶边改大小：光标移到顶边（离窗口顶 2 个有效像素）按下，每毫秒向上 1 像素走 60 像素，停 100 毫秒，再走回来，松开。
    /// 核对：走到最上面时窗口高了 60 像素左右，回来后高度复原；松开后再动光标，窗口不再跟着变（不跟着光标走）。
    /// </summary>
    private static (string Text, string? Problem) InjectResize(nint hwnd, double scale)
    {
        if (!DragProbe.GetWindowRect(hwnd, out var origin))
        {
            return ("拉顶边改大小：读不到窗口位置", "拉顶边改大小：读不到窗口位置");
        }
        var start = new Win32.POINT { X = origin.Left + (origin.Right - origin.Left) * 2 / 5, Y = origin.Top + Math.Max((int)Math.Round(2 * scale), 1) };
        if (!Inject.MoveTo(start.X, start.Y))
        {
            return ("拉顶边改大小：移动鼠标失败", "拉顶边改大小：移动鼠标失败");
        }
        Thread.Sleep(250);
        if (Inject.RootWindowAt(start) != hwnd)
        {
            var blocked = "拉顶边改大小：起点不在窗口上（被别的窗口挡住了？）";
            return (blocked, blocked);
        }
        var height = origin.Bottom - origin.Top;
        var frequency = Stopwatch.Frequency;
        int peak, back, after;
        double? startMs = null;
        try
        {
            var pressed = Stopwatch.GetTimestamp();
            Inject.Button(true);
            // 等改大小真的开始了（系统的改大小循环，或程序接住了按下）再动光标：程序晚处理了按下的话，它从那时光标所在的
            // 地方算起，先动的部分就不算了。最多等 300 毫秒（系统要等光标动了才开始时也不会卡在这里）。
            while (!DragProbe.Main.Active && !ResizeEdge.Resizing && MoveRecorder.Ms(pressed, Stopwatch.GetTimestamp()) < 300)
            {
                Thread.Sleep(1);
            }
            if (DragProbe.Main.Active || ResizeEdge.Resizing)
            {
                startMs = MoveRecorder.Ms(pressed, Stopwatch.GetTimestamp());
            }
            var down = Stopwatch.GetTimestamp();
            for (var i = 1; i <= 60; i++)
            {
                Inject.WaitUntil(down + i * frequency / 1000);
                Inject.MoveTo(start.X, start.Y - i);
            }
            Thread.Sleep(100);
            peak = DragProbe.GetWindowRect(hwnd, out var r1) ? r1.Bottom - r1.Top : -1;
            var turn = Stopwatch.GetTimestamp();
            for (var i = 1; i <= 60; i++)
            {
                Inject.WaitUntil(turn + i * frequency / 1000);
                Inject.MoveTo(start.X, start.Y - 60 + i);
            }
            Thread.Sleep(100);
            back = DragProbe.GetWindowRect(hwnd, out var r2) ? r2.Bottom - r2.Top : -1;
            Inject.Button(false);
        }
        finally
        {
            Inject.ReleaseIfDown();
        }
        // 松开以后光标再往下走 40 像素：窗口不该再变。先量一次松开后的高度再动光标：系统的改大小循环在松开时会把
        // 最后一步补上，拿松开前的高度比会把这 1 像素当成"还跟着光标走"。
        Thread.Sleep(200);
        var released = DragProbe.GetWindowRect(hwnd, out var r2b) ? r2b.Bottom - r2b.Top : -1;
        for (var i = 1; i <= 40; i++)
        {
            Inject.MoveTo(start.X, start.Y + i);
            Thread.Sleep(1);
        }
        Thread.Sleep(200);
        after = DragProbe.GetWindowRect(hwnd, out var r3) ? r3.Bottom - r3.Top : -1;
        var following = after != released;
        if (following)
        {
            // 还跟着光标在改：点一下停下（系统的改大小这样就停），再取消。
            Inject.Button(true);
            Inject.Button(false);
            Inject.CancelMode(hwnd);
            Thread.Sleep(300);
        }
        var text = $"拉顶边改大小：按下后{(startMs is double ms ? $" {ms:0.#} ms 开始改大小" : " 300 ms 内没看到开始（系统要等光标动了才开始）")}，" +
            $"往上拉 60 像素时窗口高了 {peak - height} 像素，拉回来后差 {back - height} 像素，" +
            $"松开后光标再动窗口{(following ? $"还在变（{after - released} 像素，跟着光标走）" : "不再变")}";
        var ok = Math.Abs(peak - height - 60) <= 4 && Math.Abs(back - height) <= 4 && !following;
        return (text, ok ? null : text);
    }

    // ---- 谁在用 UI 自动化 ----

    [StructLayout(LayoutKind.Sequential)]
    private unsafe struct PROCESSENTRY32W
    {
        public uint dwSize;
        public uint cntUsage;
        public uint th32ProcessID;
        public nint th32DefaultHeapID;
        public uint th32ModuleID;
        public uint cntThreads;
        public uint th32ParentProcessID;
        public int pcPriClassBase;
        public uint dwFlags;
        public fixed char szExeFile[260];
    }

    [StructLayout(LayoutKind.Sequential)]
    private unsafe struct MODULEENTRY32W
    {
        public uint dwSize;
        public uint th32ModuleID;
        public uint th32ProcessID;
        public uint GlblcntUsage;
        public uint ProccntUsage;
        public nint modBaseAddr;
        public uint modBaseSize;
        public nint hModule;
        public fixed char szModule[256];
        public fixed char szExePath[260];
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern nint CreateToolhelp32Snapshot(uint flags, uint processId);

    [DllImport("kernel32.dll", EntryPoint = "Process32FirstW", SetLastError = true)]
    private static extern unsafe int Process32First(nint snapshot, PROCESSENTRY32W* entry);

    [DllImport("kernel32.dll", EntryPoint = "Process32NextW", SetLastError = true)]
    private static extern unsafe int Process32Next(nint snapshot, PROCESSENTRY32W* entry);

    [DllImport("kernel32.dll", EntryPoint = "Module32FirstW", SetLastError = true)]
    private static extern unsafe int Module32First(nint snapshot, MODULEENTRY32W* entry);

    [DllImport("kernel32.dll", EntryPoint = "Module32NextW", SetLastError = true)]
    private static extern unsafe int Module32Next(nint snapshot, MODULEENTRY32W* entry);

    [DllImport("kernel32.dll")]
    private static extern int CloseHandle(nint handle);

    /// <summary>
    /// 加载了 UIAutomationCore.dll 的程序（UI 自动化的客户端都要加载它；系统自带的也有几个）。用来找静止时、拖动时
    /// 一直来问窗口内容的是哪个程序。最后一项是看不了的程序个数（权限不够，比如以管理员身份运行的）。
    /// </summary>
    private static unsafe List<string> UiaProcesses()
    {
        const uint TH32CS_SNAPPROCESS = 0x2, TH32CS_SNAPMODULE = 0x8, TH32CS_SNAPMODULE32 = 0x10;
        var found = new List<string>();
        var blocked = 0;
        var own = (uint)Environment.ProcessId;
        var processes = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if (processes == -1)
        {
            return new List<string> { "列不出进程" };
        }
        try
        {
            var entry = new PROCESSENTRY32W { dwSize = (uint)sizeof(PROCESSENTRY32W) };
            for (var ok = Process32First(processes, &entry); ok != 0; ok = Process32Next(processes, &entry))
            {
                var pid = entry.th32ProcessID;
                if (pid == 0 || pid == 4 || pid == own)
                {
                    continue;
                }
                var name = new string(entry.szExeFile);
                nint modules = -1;
                // 进程正在加载模块时会暂时失败（ERROR_BAD_LENGTH），再试几次。
                for (var attempt = 0; attempt < 3 && modules == -1; attempt++)
                {
                    modules = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
                    if (modules == -1 && Marshal.GetLastPInvokeError() != 24)
                    {
                        break;
                    }
                }
                if (modules == -1)
                {
                    blocked++;
                    continue;
                }
                try
                {
                    var module = new MODULEENTRY32W { dwSize = (uint)sizeof(MODULEENTRY32W) };
                    for (var more = Module32First(modules, &module); more != 0; more = Module32Next(modules, &module))
                    {
                        if (string.Equals(new string(module.szModule), "UIAutomationCore.dll", StringComparison.OrdinalIgnoreCase))
                        {
                            found.Add($"{name}（{pid}）");
                            break;
                        }
                    }
                }
                finally
                {
                    CloseHandle(modules);
                }
            }
        }
        finally
        {
            CloseHandle(processes);
        }
        found.Sort(StringComparer.OrdinalIgnoreCase);
        found.Add($"另有 {blocked} 个程序看不了（权限不够）");
        return found;
    }
}

/// <summary>自检用：用 SendInput 模拟鼠标（与真的鼠标一样经过系统的输入）。</summary>
internal static unsafe partial class Inject
{
    [StructLayout(LayoutKind.Sequential)]
    private struct MOUSEINPUT
    {
        public int Dx;
        public int Dy;
        public uint MouseData;
        public uint Flags;
        public uint Time;
        public nint ExtraInfo;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct INPUT
    {
        public uint Type;
        public MOUSEINPUT Mouse;
    }

    [LibraryImport("user32.dll")]
    private static partial uint SendInput(uint count, INPUT* inputs, int size);

    [LibraryImport("user32.dll")]
    private static partial nint WindowFromPoint(Win32.POINT point);

    [LibraryImport("user32.dll")]
    private static partial nint GetAncestor(nint hwnd, uint flags);

    [LibraryImport("user32.dll", EntryPoint = "SendMessageTimeoutW")]
    private static partial nint SendMessageTimeout(nint hwnd, uint msg, nint wParam, nint lParam, uint flags, uint timeout, out nint result);

    private const uint MOUSEEVENTF_MOVE = 0x0001, MOUSEEVENTF_LEFTDOWN = 0x0002, MOUSEEVENTF_LEFTUP = 0x0004,
        MOUSEEVENTF_RIGHTDOWN = 0x0008, MOUSEEVENTF_RIGHTUP = 0x0010, MOUSEEVENTF_VIRTUALDESK = 0x4000, MOUSEEVENTF_ABSOLUTE = 0x8000;

    private static bool Send(uint flags, int dx = 0, int dy = 0)
    {
        var input = new INPUT { Type = 0, Mouse = new MOUSEINPUT { Dx = dx, Dy = dy, Flags = flags } };
        return SendInput(1, &input, sizeof(INPUT)) == 1;
    }

    /// <summary>光标移到屏幕上的 (x, y)（物理像素）：绝对坐标按整个虚拟桌面换算成 0–65535。</summary>
    public static bool MoveTo(int x, int y)
    {
        const int SM_XVIRTUALSCREEN = 76, SM_YVIRTUALSCREEN = 77, SM_CXVIRTUALSCREEN = 78, SM_CYVIRTUALSCREEN = 79;
        var left = Win32.GetSystemMetrics(SM_XVIRTUALSCREEN);
        var top = Win32.GetSystemMetrics(SM_YVIRTUALSCREEN);
        var width = Math.Max(Win32.GetSystemMetrics(SM_CXVIRTUALSCREEN), 1);
        var height = Math.Max(Win32.GetSystemMetrics(SM_CYVIRTUALSCREEN), 1);
        return Send(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            (int)((x - left) * 65536L / width + 1), (int)((y - top) * 65536L / height + 1));
    }

    /// <summary>按下、松开和退出前的收尾排队进行：按下的状态与真发出去的一致，收尾以后不再按下。</summary>
    private static readonly object s_buttonLock = new();
    /// <summary>自检按下了左键还没松开（退出前要松开，不然系统一直当左键按着）。</summary>
    private static bool s_down;
    /// <summary>自检要退出了：不再按下。</summary>
    private static bool s_closed;

    /// <summary>按下、松开左键（用户心目中的左键：左右键对调时发右键）。退出前的收尾以后不再按下，返回 false。</summary>
    public static bool Button(bool down)
    {
        lock (s_buttonLock)
        {
            if (down && s_closed)
            {
                return false;
            }
            const int SM_SWAPBUTTON = 23;
            var swapped = Win32.GetSystemMetrics(SM_SWAPBUTTON) != 0;
            var sent = Send(swapped ? (down ? MOUSEEVENTF_RIGHTDOWN : MOUSEEVENTF_RIGHTUP) : (down ? MOUSEEVENTF_LEFTDOWN : MOUSEEVENTF_LEFTUP));
            // 按下时不管发没发成都当作按着（收尾时多发一个松开没有坏处）；松开发成了才算松开。
            if (down || sent)
            {
                s_down = down;
            }
            return sent;
        }
    }

    /// <summary>
    /// 自检按下的左键还没松开时松开。closing（自检退出前）为 true 时，之后不再按下（还在拖的线程再按也不发）。
    /// </summary>
    public static void ReleaseIfDown(bool closing = false)
    {
        lock (s_buttonLock)
        {
            if (closing)
            {
                s_closed = true;
            }
            if (s_down)
            {
                Button(false);
            }
        }
    }

    /// <summary>屏幕上这一点是哪个顶层窗口。</summary>
    public static nint RootWindowAt(Win32.POINT point)
    {
        const uint GA_ROOT = 2;
        var hwnd = WindowFromPoint(point);
        return hwnd == 0 ? 0 : GetAncestor(hwnd, GA_ROOT);
    }

    /// <summary>问窗口屏幕上这一点是什么（WM_NCHITTEST：1 内容区，2 标题栏，12 上边框……；问不到时为 null）。</summary>
    public static int? HitTest(nint hwnd, Win32.POINT point)
    {
        const uint WM_NCHITTEST = 0x0084, SMTO_ABORTIFHUNG = 0x0002;
        var lParam = (nint)((point.Y << 16) | (point.X & 0xFFFF));
        return SendMessageTimeout(hwnd, WM_NCHITTEST, 0, lParam, SMTO_ABORTIFHUNG, 1000, out var result) == 0 ? null : (int)(short)result;
    }

    public static string HitTestName(int? code) => code switch
    {
        null => "问不到",
        -1 => "透明（交给下面的窗口）",
        0 => "哪里也不是",
        1 => "内容区",
        2 => "标题栏",
        12 => "上边框",
        13 => "左上角",
        14 => "右上角",
        _ => $"其他（{code}）",
    };

    /// <summary>结束窗口的系统移动（窗口跟着光标走时，自检用它停下来）。</summary>
    public static void CancelMode(nint hwnd)
    {
        const uint WM_CANCELMODE = 0x001F, SMTO_ABORTIFHUNG = 0x0002;
        SendMessageTimeout(hwnd, WM_CANCELMODE, 0, 0, SMTO_ABORTIFHUNG, 2000, out _);
    }

    /// <summary>等到 Stopwatch 的时刻 ticks（忙等，精确到微秒级；只在自检注入时用，最多一秒左右）。</summary>
    public static void WaitUntil(long ticks)
    {
        while (Stopwatch.GetTimestamp() < ticks)
        {
            Thread.SpinWait(20);
        }
    }
}

/// <summary>
/// 自检的对照窗口：普通的 Win32 窗口（不经过 WinUI），在自己的线程上，只有系统的标题栏和灰色的底。
/// 拖它的标题栏，看同一台电脑上系统的移动循环本来有多顺。
/// </summary>
internal sealed unsafe partial class PlainWindow
{
    private static PlainWindow? s_current;
    public readonly MoveRecorder Recorder = new(() => 0, () => 0);
    public nint Hwnd { get; private set; }
    private Thread? _thread;
    /// <summary>0 正在建，1 建好了，2 等不及、不要了（建好以后马上关掉）。</summary>
    private int _state;

    [StructLayout(LayoutKind.Sequential)]
    private struct WNDCLASSEXW
    {
        public uint cbSize;
        public uint style;
        public delegate* unmanaged<nint, uint, nint, nint, nint> lpfnWndProc;
        public int cbClsExtra;
        public int cbWndExtra;
        public nint hInstance;
        public nint hIcon;
        public nint hCursor;
        public nint hbrBackground;
        public char* lpszMenuName;
        public char* lpszClassName;
        public nint hIconSm;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MSG
    {
        public nint hwnd;
        public uint message;
        public nint wParam;
        public nint lParam;
        public uint time;
        public int x;
        public int y;
    }

    [LibraryImport("user32.dll", EntryPoint = "RegisterClassExW")]
    private static partial ushort RegisterClassEx(WNDCLASSEXW* wc);

    [LibraryImport("user32.dll", EntryPoint = "CreateWindowExW")]
    private static partial nint CreateWindowEx(uint exStyle, char* className, char* title, uint style, int x, int y, int width, int height,
        nint parent, nint menu, nint instance, nint param);

    [LibraryImport("user32.dll", EntryPoint = "DefWindowProcW")]
    private static partial nint DefWindowProc(nint hwnd, uint msg, nint wParam, nint lParam);

    [LibraryImport("user32.dll", EntryPoint = "GetMessageW")]
    private static partial int GetMessage(MSG* msg, nint hwnd, uint min, uint max);

    [LibraryImport("user32.dll")]
    private static partial int TranslateMessage(MSG* msg);

    [LibraryImport("user32.dll", EntryPoint = "DispatchMessageW")]
    private static partial nint DispatchMessage(MSG* msg);

    [LibraryImport("user32.dll")]
    private static partial void PostQuitMessage(int code);

    [LibraryImport("user32.dll")]
    private static partial int DestroyWindow(nint hwnd);

    [LibraryImport("user32.dll", EntryPoint = "PostMessageW")]
    private static partial int PostMessage(nint hwnd, uint msg, nint wParam, nint lParam);

    [LibraryImport("user32.dll", EntryPoint = "SendMessageW")]
    private static partial nint SendMessage(nint hwnd, uint msg, nint wParam, nint lParam);

    [LibraryImport("user32.dll", EntryPoint = "LoadCursorW")]
    private static partial nint LoadCursor(nint instance, nint name);

    [LibraryImport("gdi32.dll")]
    private static partial nint GetStockObject(int index);

    [LibraryImport("kernel32.dll", EntryPoint = "GetModuleHandleW")]
    private static partial nint GetModuleHandle(char* name);

    private const uint WM_DESTROY = 0x0002, WM_CLOSE = 0x0010, WM_WINDOWPOSCHANGED = 0x0047, WM_NCHITTEST = 0x0084,
        WM_ENTERSIZEMOVE = 0x0231, WM_EXITSIZEMOVE = 0x0232;

    /// <summary>在 (x, y) 开一个 width x height 的对照窗口（物理像素）；3 秒内没开出来时为空。</summary>
    public static PlainWindow? Open(int x, int y, int width, int height)
    {
        var window = new PlainWindow();
        s_current = window;
        // 不用 using：超时返回以后线程还可能 Set 它。
        var ready = new ManualResetEventSlim();
        window._thread = new Thread(() => window.Run(x, y, width, height, ready)) { IsBackground = true, Name = "对照窗口" };
        window._thread.SetApartmentState(ApartmentState.STA);
        window._thread.Start();
        ready.Wait(3000);
        // 3 秒还没建好就不要了（建好以后那边自己关掉）；正好在这时建好的照常用。
        if (Interlocked.CompareExchange(ref window._state, 2, 0) != 1 || window.Hwnd == 0)
        {
            return null;
        }
        return window;
    }

    private void Run(int x, int y, int width, int height, ManualResetEventSlim ready)
    {
        // 这个线程上漏出去的异常会让程序直接退出：接住、记下来。
        try
        {
            RunLoop(x, y, width, height, ready);
        }
        catch (Exception error)
        {
            Crash.Write("PlainWindow.Run", error);
            ready.Set();
        }
    }

    private void RunLoop(int x, int y, int width, int height, ManualResetEventSlim ready)
    {
        const uint WS_OVERLAPPEDWINDOW = 0x00CF0000, WS_VISIBLE = 0x10000000;
        const int DKGRAY_BRUSH = 3, IDC_ARROW = 32512;
        try
        {
            var instance = GetModuleHandle(null);
            fixed (char* className = "AnyviewDragControl")
            fixed (char* title = "Anyview 自检的对照窗口（马上关掉）")
            {
                var wc = new WNDCLASSEXW
                {
                    cbSize = (uint)sizeof(WNDCLASSEXW),
                    lpfnWndProc = &WndProc,
                    hInstance = instance,
                    hCursor = LoadCursor(0, IDC_ARROW),
                    hbrBackground = GetStockObject(DKGRAY_BRUSH),
                    lpszClassName = className,
                };
                // 第二次登记会失败（类已经有了），照样能用。
                RegisterClassEx(&wc);
                Hwnd = CreateWindowEx(0, className, title, WS_OVERLAPPEDWINDOW | WS_VISIBLE, x, y, width, height, 0, 0, instance, 0);
            }
            // 先登记"建好了"再通知等的那边；那边已经不要了（等了 3 秒）就马上关掉，不留一个没人管的窗口挡着后面的拖动
            // （关掉以后下面的消息循环收到退出消息就结束）。
            if (Hwnd != 0 && Interlocked.CompareExchange(ref _state, 1, 0) == 2)
            {
                DestroyWindow(Hwnd);
            }
        }
        finally
        {
            ready.Set();
        }
        if (Hwnd == 0)
        {
            return;
        }
        MSG msg;
        while (GetMessage(&msg, 0, 0, 0) > 0)
        {
            TranslateMessage(&msg);
            DispatchMessage(&msg);
        }
    }

    [UnmanagedCallersOnly]
    private static nint WndProc(nint hwnd, uint msg, nint wParam, nint lParam)
    {
        try
        {
            var self = s_current;
            switch (msg)
            {
                case WM_ENTERSIZEMOVE:
                    self?.Recorder.Enter(hwnd, Win32.GetCurrentThreadId());
                    break;
                case WM_WINDOWPOSCHANGED:
                    if (self is not null && self.Recorder.Active && (Marshal.ReadInt32(lParam, 32) & 0x0002) == 0)
                    {
                        self.Recorder.Moved(Marshal.ReadInt32(lParam, 16), Marshal.ReadInt32(lParam, 20));
                    }
                    break;
                case WM_EXITSIZEMOVE:
                    if (self is not null)
                    {
                        self.Recorder.Exit(0, result => result.Source = "对照窗口的标题栏");
                    }
                    break;
                case WM_DESTROY:
                    PostQuitMessage(0);
                    return 0;
            }
        }
        catch (Exception error)
        {
            Crash.Write("PlainWindow.WndProc", error);
        }
        return DefWindowProc(hwnd, msg, wParam, lParam);
    }

    /// <summary>标题栏中间的一点（按窗口自己说的：WM_NCHITTEST 是 HTCAPTION 的那一段的中间）。</summary>
    public Win32.POINT? CaptionPoint()
    {
        const int HTCAPTION = 2;
        if (!DragProbe.GetWindowRect(Hwnd, out var rect))
        {
            return null;
        }
        var x = (rect.Left + rect.Right) / 2;
        var hits = new List<int>();
        for (var y = rect.Top; y < rect.Top + 120 && y < rect.Bottom; y++)
        {
            if (SendMessage(Hwnd, WM_NCHITTEST, 0, (nint)((y << 16) | (x & 0xFFFF))) == HTCAPTION)
            {
                hits.Add(y);
            }
        }
        return hits.Count == 0 ? null : new Win32.POINT { X = x, Y = hits[hits.Count / 2] };
    }

    public void Close()
    {
        PostMessage(Hwnd, WM_CLOSE, 0, 0);
        _thread?.Join(3000);
        s_current = null;
    }
}
