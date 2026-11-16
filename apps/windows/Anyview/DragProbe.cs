// 窗口拖动性能与卡顿采样工具，用于定位首拖延迟与拖动掉帧问题。
// 移动窗口时（拖图片是程序自己移动；拖顶端那一条、标题栏，拉边框是系统的移动循环）每移动一步记下时间和位置，算出每步的间隔、
// 卡住了几次；这段时间里界面线程忙在哪个模块（采样）、UI 自动化来问几次、垃圾回收和 JIT 各几次、XAML 画了几帧。
// 只在自检（--self-test）和手动测试打开了记录（--drag-log 目录，打开Anyview.cmd 传）时记；平常什么也不做。
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using Microsoft.UI.Xaml.Media;

namespace Anyview;

/// <summary>一次拖动（程序自己移动或系统的移动循环，从开始移动到松开）的结果。</summary>
internal sealed class MoveResult
{
    /// <summary>从哪里拖的（图片、顶端那一条、系统的标题栏……）。</summary>
    public string Source = "";
    public double SinceLaunchS;
    /// <summary>按下以后多久才轮到程序处理（按下在消息队列里等的时间）：按指针事件的时间戳、按消息时间各算一次，读不到时为空。</summary>
    public double? PressWaitMs;
    public double? PressWaitTickMs;
    /// <summary>程序处理按下用了多久（到开始移动为止）。</summary>
    public double? HandlerMs;
    public double DurationMs;
    public int Moves;
    /// <summary>开始移动以后多久窗口第一次动（没动过时为空）。</summary>
    public double? FirstMoveMs;
    public double GapMedianMs;
    public double Gap95Ms;
    public double GapMaxMs;
    /// <summary>卡住的次数：停了 50 ms 以上、接着一下子跳 8 像素以上。</summary>
    public int Stalls;
    public double MaxJumpPx;
    public int Automation;
    public int Gc;
    public int JitMethods;
    public double JitMs;
    public int Frames;
    public int Draws;
    public List<string> Samples = new();
    /// <summary>开始移动、松开的时刻（Stopwatch.GetTimestamp）。</summary>
    public long EnterTicks;
    public long ExitTicks;
    public int StartX;
    public int StartY;
    public int EndX;
    public int EndY;

    private static string Ms(double? value) => value is double v ? $"{v:0.#} ms" : "（读不到）";

    /// <summary>一行文字（拖动记录、自检摘要用）。</summary>
    public string Text()
    {
        var text = new StringBuilder();
        text.Append($"从{Source}拖");
        if (HandlerMs is not null || PressWaitMs is not null || PressWaitTickMs is not null)
        {
            text.Append($"：按下以后 {Ms(PressWaitMs)}（按消息时间 {Ms(PressWaitTickMs)}）轮到程序处理，处理用了 {Ms(HandlerMs)} 开始移动");
        }
        text.Append($"；移动 {Moves} 步，用时 {DurationMs:0} ms");
        if (Moves > 0)
        {
            text.Append($"，第一步在开始移动后 {Ms(FirstMoveMs)}；每步间隔 中位 {GapMedianMs:0.#} ms、95% {Gap95Ms:0.#} ms、最长 {GapMaxMs:0.#} ms，" +
                $"卡住 {Stalls} 次（停了 50 ms 以上、接着一下子跳 8 像素以上），最大一跳 {MaxJumpPx:0} 像素");
        }
        text.Append($"；UI 自动化来问 {Automation} 次，垃圾回收 {Gc} 次，JIT {JitMethods} 个方法（{JitMs:0.#} ms），" +
            $"XAML 画了 {Frames} 帧，画布重画 {Draws} 次");
        return text.ToString();
    }

    public Dictionary<string, object?> ToRecord() => new()
    {
        ["source"] = Source,
        ["pressWaitMs"] = Round(PressWaitMs),
        ["pressWaitTickMs"] = Round(PressWaitTickMs),
        ["handlerMs"] = Round(HandlerMs),
        ["durationMs"] = Math.Round(DurationMs, 1),
        ["moves"] = Moves,
        ["firstMoveMs"] = Round(FirstMoveMs),
        ["gapMsMedian"] = Math.Round(GapMedianMs, 2),
        ["gapMs95"] = Math.Round(Gap95Ms, 2),
        ["gapMsMax"] = Math.Round(GapMaxMs, 2),
        ["stalls"] = Stalls,
        ["maxJumpPx"] = Math.Round(MaxJumpPx, 1),
        ["automationQueries"] = Automation,
        ["gcCount"] = Gc,
        ["jitMethods"] = JitMethods,
        ["jitMs"] = Math.Round(JitMs, 1),
        ["frames"] = Frames,
        ["draws"] = Draws,
        ["uiSamples"] = Samples,
        ["text"] = Text(),
    };

    private static double? Round(double? value) => value is double v ? Math.Round(v, 2) : null;
}

/// <summary>
/// 记录一个线程上的移动循环（主窗口的在界面线程上；自检的对照窗口在它自己的线程上）。Enter、Moved、Exit 都在
/// 窗口所属的线程上调用；结果（带采样）在线程池上补齐，之后 Completed 加一、Last 换成新的结果。
/// </summary>
internal sealed class MoveRecorder
{
    private const int Capacity = 16384;
    private readonly long[] _times = new long[Capacity];
    private readonly int[] _xs = new int[Capacity];
    private readonly int[] _ys = new int[Capacity];
    private readonly Func<int> _automation;
    private readonly Func<int> _draws;
    private int _count;
    private long _enter;
    private int _startX;
    private int _startY;
    private Sampler.Run? _sampling;
    private (int Automation, int Gc, long Jit, TimeSpan JitTime, int Draws) _before;
    private int _completed;
    private volatile bool _active;
    private volatile MoveResult? _last;

    public MoveRecorder(Func<int> automation, Func<int> draws)
    {
        _automation = automation;
        _draws = draws;
    }

    /// <summary>正在系统的移动循环里（从开始移动到松开）。别的线程也可以读。</summary>
    public bool Active => _active;
    /// <summary>补齐了的结果有几个（别的线程等结果用）。</summary>
    public int Completed => Volatile.Read(ref _completed);
    public MoveResult? Last => _last;
    /// <summary>结果补齐以后（在线程池上）调用。</summary>
    public Action<MoveResult>? Done;

    public void Enter(nint hwnd, uint threadId)
    {
        _count = 0;
        _enter = Stopwatch.GetTimestamp();
        if (DragProbe.GetWindowRect(hwnd, out var rect))
        {
            _startX = rect.Left;
            _startY = rect.Top;
        }
        _before = (_automation(), GcCount(), System.Runtime.JitInfo.GetCompiledMethodCount(false),
            System.Runtime.JitInfo.GetCompilationTime(false), _draws());
        _sampling = new Sampler.Run(threadId, 30000);
        _active = true;
    }

    /// <summary>窗口移到了 (x, y)（外框左上角，物理像素）。</summary>
    public void Moved(int x, int y)
    {
        if (!_active || _count >= Capacity)
        {
            return;
        }
        _times[_count] = Stopwatch.GetTimestamp();
        _xs[_count] = x;
        _ys[_count] = y;
        _count++;
    }

    /// <summary>松开了：算出结果（不带采样）交给 fill 补上按下的信息，采样完成后补上采样、发布。</summary>
    public void Exit(int frames, Action<MoveResult>? fill = null)
    {
        if (!_active)
        {
            return;
        }
        _active = false;
        var exit = Stopwatch.GetTimestamp();
        var result = new MoveResult
        {
            EnterTicks = _enter,
            ExitTicks = exit,
            DurationMs = Ms(_enter, exit),
            Moves = _count,
            StartX = _startX,
            StartY = _startY,
            EndX = _count > 0 ? _xs[_count - 1] : _startX,
            EndY = _count > 0 ? _ys[_count - 1] : _startY,
            Automation = _automation() - _before.Automation,
            Gc = GcCount() - _before.Gc,
            JitMethods = (int)(System.Runtime.JitInfo.GetCompiledMethodCount(false) - _before.Jit),
            JitMs = (System.Runtime.JitInfo.GetCompilationTime(false) - _before.JitTime).TotalMilliseconds,
            Frames = frames,
            Draws = _draws() - _before.Draws,
            SinceLaunchS = Math.Round(LaunchClock.Now / 1000, 1),
        };
        if (_count > 0)
        {
            result.FirstMoveMs = Ms(_enter, _times[0]);
            var gaps = new List<double>();
            int previousX = _startX, previousY = _startY;
            for (var i = 0; i < _count; i++)
            {
                var jump = Math.Sqrt(Math.Pow(_xs[i] - previousX, 2) + Math.Pow(_ys[i] - previousY, 2));
                result.MaxJumpPx = Math.Max(result.MaxJumpPx, jump);
                if (i > 0)
                {
                    var gap = Ms(_times[i - 1], _times[i]);
                    gaps.Add(gap);
                    if (gap >= 50 && jump >= 8)
                    {
                        result.Stalls++;
                    }
                }
                previousX = _xs[i];
                previousY = _ys[i];
            }
            if (gaps.Count > 0)
            {
                gaps.Sort();
                result.GapMedianMs = gaps[gaps.Count / 2];
                result.Gap95Ms = gaps[Math.Min((int)(gaps.Count * 0.95), gaps.Count - 1)];
                result.GapMaxMs = gaps[^1];
            }
        }
        fill?.Invoke(result);
        var sampling = _sampling;
        _sampling = null;
        sampling?.Stop();
        void Publish(List<string>? samples)
        {
            if (samples is not null)
            {
                result.Samples = samples;
            }
            _last = result;
            Interlocked.Increment(ref _completed);
            try
            {
                Done?.Invoke(result);
            }
            catch (Exception error)
            {
                Crash.Write("DragProbe.Done", error);
            }
        }
        if (sampling is null)
        {
            ThreadPool.QueueUserWorkItem(_ => Publish(null));
            return;
        }
        sampling.Result.ContinueWith(task => Publish(task.IsCompletedSuccessfully ? task.Result : null),
            System.Threading.Tasks.TaskScheduler.Default);
    }

    /// <summary>垃圾回收的次数（每次回收都包括第 0 代，所以第 0 代的次数就是总次数）。</summary>
    private static int GcCount() => GC.CollectionCount(0);

    public static double Ms(long from, long to) => (to - from) * 1000.0 / Stopwatch.Frequency;
}

/// <summary>主窗口的拖动记录（界面线程）。见文件开头。</summary>
internal static partial class DragProbe
{
    /// <summary>打开了记录（自检，或手动测试传了 --drag-log）。启动时设好，之后不变。</summary>
    public static bool Enabled { get; private set; }
    /// <summary>手动测试的拖动采样日志输出目录（如 reports/windows/drags）；为空时不写文件。</summary>
    public static string? LogDirectory { get; private set; }
    public static readonly MoveRecorder Main = new(() => Diag.AutomationQueries, () => Diag.CanvasDraws);
    /// <summary>正在移动窗口（程序自己移动，或系统的移动循环里），要记每一步（窗口过程里先看它，平常是 false）。</summary>
    public static bool Recording => Main.Active;

    private static long s_pressTicks;
    private static double? s_pressWaitMs;
    private static double? s_pressWaitTickMs;
    /// <summary>正在进行的程序自己移动：从哪里拖的、什么时候开始的，以及属于它的那次按下（没有时 PressTicks 为 0）。</summary>
    private static bool s_manual;
    private static string? s_manualSource;
    private static long s_manualTicks;
    private static long s_manualPressTicks;
    private static double? s_manualPressWaitMs;
    private static double? s_manualPressWaitTickMs;
    private static int s_frames;
    private static readonly EventHandler<RenderedEventArgs> s_countFrame = (_, _) => s_frames++;
    private static int s_drags;
    private static string? s_logFile;
    private static readonly object s_logLock = new();
    private static int s_notStarted;

    private static readonly object s_anchorLock = new();
    private static Win32.POINT s_anchor;
    private static long s_anchorTicks;

    /// <summary>
    /// 最近一次程序自己开始移动窗口时光标在哪里（窗口按光标离这一点多远跟着走），在 since（Stopwatch 的时刻）之后开始的才算。
    /// 自检算窗口落后光标多少用：按下以后程序晚处理了，光标已经走了一段，从那里算起。
    /// </summary>
    public static Win32.POINT? AnchorSince(long since)
    {
        lock (s_anchorLock)
        {
            return s_anchorTicks >= since && s_anchorTicks != 0 ? s_anchor : null;
        }
    }

    /// <summary>"左键已经松开、没有开始移动"发生了几次，以及最近一次的说明（自检核对用）。</summary>
    public static int NotStarted => Volatile.Read(ref s_notStarted);
    public static string? LastNotStarted { get; private set; }

    public static void Enable(string? logDirectory)
    {
        Enabled = true;
        LogDirectory = string.IsNullOrWhiteSpace(logDirectory) ? null : logDirectory;
        Main.Done = Write;
    }

    [LibraryImport("user32.dll")]
    private static partial int GetMessageTime();

    [LibraryImport("kernel32.dll")]
    private static partial uint GetTickCount();

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool GetWindowRect(nint hwnd, out RECT rect);

    /// <summary>
    /// 主窗口上按下了左键（指针事件的时间戳，微秒）：记下这一刻，以及按下在消息队列里等了多久才轮到程序处理
    /// （时间戳和 QueryPerformanceCounter 是同一个时钟时才对得上；另按消息时间算一次）。
    /// </summary>
    public static void Pressed(ulong timestampUs)
    {
        if (!Enabled)
        {
            return;
        }
        s_pressTicks = Stopwatch.GetTimestamp();
        var nowUs = s_pressTicks * 1_000_000.0 / Stopwatch.Frequency;
        var wait = (nowUs - timestampUs) / 1000;
        s_pressWaitMs = wait is >= -5 and <= 60000 ? Math.Max(wait, 0) : null;
        var tickWait = (int)(GetTickCount() - (uint)GetMessageTime());
        s_pressWaitTickMs = tickWait is >= 0 and <= 60000 ? tickWait : null;
    }

    /// <summary>取走属于这一刻的那次按下（2 秒以内的；用过就清掉，免得算到下一次头上）。</summary>
    private static (long Ticks, double? Wait, double? WaitTick) TakePress(long now)
    {
        var pressed = Recent(s_pressTicks, now);
        var press = pressed ? (s_pressTicks, s_pressWaitMs, s_pressWaitTickMs) : (0L, (double?)null, (double?)null);
        s_pressTicks = 0;
        return press;
    }

    /// <summary>
    /// 程序自己开始移动窗口（拖图片；顶端那一条、顶边在系统接不住时）：source 是从哪里拖的，anchor 是开始时光标的位置
    /// （窗口按光标离这一点多远跟着走）。界面线程。
    /// </summary>
    public static void ManualStart(nint hwnd, string source, Win32.POINT anchor)
    {
        if (!Enabled || Main.Active)
        {
            return;
        }
        s_manualTicks = Stopwatch.GetTimestamp();
        (s_manualPressTicks, s_manualPressWaitMs, s_manualPressWaitTickMs) = TakePress(s_manualTicks);
        s_manualSource = source;
        s_manual = true;
        lock (s_anchorLock)
        {
            s_anchor = anchor;
            s_anchorTicks = s_manualTicks;
        }
        s_frames = 0;
        CompositionTarget.Rendered += s_countFrame;
        Main.Enter(hwnd, App.UiThreadId);
    }

    /// <summary>程序自己移动窗口结束了（松开、指针被取消）。界面线程。</summary>
    public static void ManualEnd()
    {
        if (!Enabled || !s_manual)
        {
            return;
        }
        s_manual = false;
        CompositionTarget.Rendered -= s_countFrame;
        var source = s_manualSource ?? "";
        var started = s_manualTicks;
        var pressTicks = s_manualPressTicks;
        var pressWait = s_manualPressWaitMs;
        var pressWaitTick = s_manualPressWaitTickMs;
        Main.Exit(s_frames, result =>
        {
            result.Source = source;
            if (pressTicks != 0)
            {
                result.HandlerMs = MoveRecorder.Ms(pressTicks, started);
                result.PressWaitMs = pressWait;
                result.PressWaitTickMs = pressWaitTick;
            }
        });
    }

    /// <summary>
    /// 鼠标的左键已经松开才轮到程序处理这次按下：不开始移动（不然窗口会一直跟着指针走）。记下来（自检核对、手动测试的记录）。
    /// </summary>
    public static void NotStartedMove(string source)
    {
        if (!Enabled)
        {
            return;
        }
        var now = Stopwatch.GetTimestamp();
        var press = TakePress(now);
        var text = $"从{source}拖：按下以后 {Text(press.Wait)} 才轮到程序处理，" +
            $"处理用了 {Text(press.Ticks != 0 ? MoveRecorder.Ms(press.Ticks, now) : null)}，那时左键已经松开，没有开始移动（不然窗口会一直跟着指针走）";
        LastNotStarted = text;
        Interlocked.Increment(ref s_notStarted);
        var since = LaunchClock.Now / 1000;
        ThreadPool.QueueUserWorkItem(_ => Append($"[{DateTime.Now:HH:mm:ss.fff}] 启动后 {since:0.0} 秒，{text}\r\n"));
    }

    /// <summary>主窗口进入系统的移动循环（WM_ENTERSIZEMOVE：拖标题栏、顶端那一条，拉边框，界面线程）。</summary>
    public static void Enter(nint hwnd)
    {
        if (!Enabled || Main.Active)
        {
            return;
        }
        s_frames = 0;
        CompositionTarget.Rendered += s_countFrame;
        Main.Enter(hwnd, App.UiThreadId);
    }

    /// <summary>主窗口移动了一步（WM_WINDOWPOSCHANGED 的 lParam 是 WINDOWPOS）。</summary>
    public static unsafe void Moved(nint lParam)
    {
        const uint SWP_NOMOVE = 0x0002;
        // WINDOWPOS（x64）：hwnd、hwndInsertAfter 各 8 字节，之后是 x、y、cx、cy、flags。
        var flags = (uint)Marshal.ReadInt32(lParam, 32);
        if ((flags & SWP_NOMOVE) != 0)
        {
            return;
        }
        Main.Moved(Marshal.ReadInt32(lParam, 16), Marshal.ReadInt32(lParam, 20));
    }

    /// <summary>主窗口离开系统的移动循环（WM_EXITSIZEMOVE，界面线程）。程序自己移动时不算。</summary>
    public static void Exit()
    {
        if (!Enabled || !Main.Active || s_manual)
        {
            return;
        }
        CompositionTarget.Rendered -= s_countFrame;
        Main.Exit(s_frames, result => result.Source = "系统的移动（标题栏、边框）");
    }

    /// <summary>from 在 to 之前 2 秒以内（同一次按下）。</summary>
    private static bool Recent(long from, long to) => from != 0 && from <= to && MoveRecorder.Ms(from, to) < 2000;

    private static string Text(double? ms) => ms is double v ? $"{v:0.#} ms" : "（读不到）";

    /// <summary>一次拖动的结果写进记录文件（线程池上）。</summary>
    private static void Write(MoveResult result)
    {
        var number = Interlocked.Increment(ref s_drags);
        var text = new StringBuilder();
        text.Append($"[{DateTime.Now:HH:mm:ss.fff}] 第 {number} 次，启动后 {result.SinceLaunchS:0.0} 秒，{result.Text()}\r\n");
        if (result.Samples.Count > 0)
        {
            text.Append($"    界面线程在哪个模块（采样）：{string.Join("；", result.Samples)}\r\n");
        }
        Append(text.ToString());
    }

    private static void Append(string text)
    {
        if (LogDirectory is not string dir)
        {
            return;
        }
        lock (s_logLock)
        {
            try
            {
                if (s_logFile is null)
                {
                    Directory.CreateDirectory(dir);
                    var mode = Program.WinUiTitleBar ? "winui" : "win32";
                    s_logFile = Path.Combine(dir, $"{DateTime.Now:yyyy-MM-dd-HHmmss}-{mode}.txt");
                    File.AppendAllText(s_logFile,
                        $"Anyview 拖动记录 {DateTime.Now:yyyy-MM-dd HH:mm:ss}；标题栏 {mode}；{SystemInfo()}\r\n", Encoding.UTF8);
                }
                File.AppendAllText(s_logFile, text, Encoding.UTF8);
            }
            catch (Exception)
            {
                // 日志目录不可写时静默忽略，不影响正常看图。
            }
        }
    }

    [LibraryImport("user32.dll")]
    private static partial nint GetDC(nint hwnd);

    [LibraryImport("user32.dll")]
    private static partial int ReleaseDC(nint hwnd, nint hdc);

    [LibraryImport("gdi32.dll")]
    private static partial int GetDeviceCaps(nint hdc, int index);

    [LibraryImport("user32.dll", EntryPoint = "SystemParametersInfoW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static unsafe partial bool SystemParametersInfo(uint action, uint param, void* value, uint winIni);

    /// <summary>与拖动有关的系统情况：主屏幕的刷新率、"拖动时显示窗口内容"、有没有程序在用 UI 自动化。</summary>
    public static unsafe string SystemInfo()
    {
        const int VREFRESH = 116;
        const uint SPI_GETDRAGFULLWINDOWS = 0x0026;
        var hdc = GetDC(0);
        var hz = hdc != 0 ? GetDeviceCaps(hdc, VREFRESH) : 0;
        if (hdc != 0)
        {
            ReleaseDC(0, hdc);
        }
        int full = 0;
        var fullKnown = SystemParametersInfo(SPI_GETDRAGFULLWINDOWS, 0, &full, 0);
        return $"主屏幕刷新率 {hz} Hz；拖动时显示窗口内容：{(fullKnown ? (full != 0 ? "开" : "关（拖动时只显示框，松开才移动）") : "读不到")}；" +
            $"系统里{(Diag.AutomationClientsListening() ? "有" : "没有")}程序在用 UI 自动化";
    }
}
