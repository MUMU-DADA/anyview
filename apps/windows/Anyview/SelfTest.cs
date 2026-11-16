// 自检模式（tools/windows「编译测试.cmd」使用）：写出 selftest.json 和截图后退出，返回码 0 表示通过。
// 步骤的写法与 macOS 的自检相同：
// - 不带 --open：空态自检。收到 Pong 后截图，核对背景色 #202020。
// - 带 --open：等第一张图（记下冷启动时间），再按 --steps 依次执行动作。切图的步骤等图像就绪，其他步骤等画面更新，
//   然后截图。每张截图都做"画面核对"：图片上 16 个点按核心算出的位置找到画布上的像素，与原图比较颜色，
//   验证平移、缩放、旋转、翻转确实按核心的计算画出来；透明处应为棋盘格或设置的纯色，半透明的点跳过。
//   每一步还核对悬浮界面：控制栏只在锁定（或模拟指针移到底部）时显示，图片信息与核心一致，进度条滑块与序号一致。
// - `key:按键`（如 key:Right、key:Ctrl+Up、key:Period）交给核心解析；`mouse:wheel_down|wheel_up|right|middle|back|forward`、
//   `mouse:left:x`、`mouse:double:x`（x 是画布宽度上的位置 0–1）模拟鼠标；`hover_bottom` 模拟指针移到窗口底部。
// - `wait:毫秒`、`idle:毫秒`（CPU 占用、内存）、`wait_exact`、`wait_frames:N`、`open:路径`、`window:宽x高`（有效像素）、
//   `bar_jump:比例`、`transition_hold:进度|off`。
// - 设置窗口：`settings_open`、`settings_page:页`、`settings_set:键=值`、`settings_button:键:按钮`、`settings_ok`、
//   `settings_ok_invalid`（应被拦下）、`settings_reset`、`settings_cancel`。选择图片窗口：`picker`、`picker_select:N`、
//   `picker_sort:动作`、`picker_hide:on|off`、`picker_ok`、`picker_cancel`。帮助窗口：`help:shortcuts|formats`。
// - 多个窗口、吸附边缘：`finder_open:路径`（模拟从资源管理器再打开）、`key_in:窗口序号:按键`、
//   `move_window:距离:snap|stay`（模拟拖标题栏把窗口放到离可用区域左上角这么远，松开后看吸附）、`drag_window:距离:snap`
//   （模拟在画面上拖动窗口）。文件夹变化：`add_file:源文件`、`remove_file:文件名`。
// - 拖动窗口基准测试：`move_bench`（程序移动窗口 60 次测步耗时）、`drag_real`（模拟真实鼠标拖动
//   真的拖几次，与普通 Win32 窗口对照，核对松开后窗口不跟着光标走，见 SelfTestDrag.cs；鼠标会自己动几秒）。
// - 到达首尾时的询问不弹对话框：记下来，由下一步（如 first_image）充当用户的选择。
// - 等新图像的步骤以新图像、到达首尾、复制或移动完成的提示、回到空态为完成；在动图上做反相这类只换帧的步骤还不支持。
// 截图只有画布（不含悬浮控件），按物理像素，与核心的坐标一致。
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Threading.Tasks;
using Anyview.Core;
using Microsoft.Graphics.Canvas;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.Foundation;

namespace Anyview;

internal sealed partial class SelfTest
{
    private const int ColorTolerance = 40;
    private readonly LaunchOptions _options;
    private readonly App _app;
    private readonly MainWindow _window;
    private readonly CoreBridge _bridge;
    private readonly string _dir;
    private readonly List<Dictionary<string, object?>> _records = new();
    private readonly List<string> _problems = new();
    private readonly List<Dictionary<string, object?>> _externals = new();
    private readonly List<string> _requests = new();
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private string? _failure;
    private bool _finished;
    private bool _waitingImage;
    private Dictionary<string, object?>? _pendingRecord;
    private double _stepStarted;
    private double? _previewMs;
    private int _framesSeen;
    private bool _hoverBar;
    private string? _boundary;
    private int _menuShown;
    private int _captureSerial;
    /// <summary>上一张和这一张图片中心的颜色（RGB；核对翻页效果的中间画面）。</summary>
    private int[]? _previousColor;
    private int[]? _currentColor;
    /// <summary>设置窗口最近一次"确定"的结果。</summary>
    private SettingsApplyResult.Applied? _lastApplied;
    /// <summary>最近一次复制、移动完成的提示。</summary>
    private string? _lastNotice;
    /// <summary>开着的询问对话框："按第几个按钮"的函数和按钮的个数（limit_open、limit_cancel 用）。</summary>
    private Action<int>? _dialogAnswer;
    private Prompt? _dialogPrompt;
    private int _dialogButtons;

    public SelfTest(LaunchOptions options, MainWindow window, CoreBridge bridge, App app)
    {
        _options = options;
        _app = app;
        _window = window;
        _bridge = bridge;
        _dir = options.SelfTestDir!;
        Directory.CreateDirectory(_dir);
        // 真的鼠标指针碰巧在窗口上时不影响控制栏、箭头（指针的影响由 hover_bottom 等步骤模拟）。
        window.IgnoreRealPointer = true;
        // 对照：关窗口时的收尾里哪几步不做（churn 那一步量差别）。
        CloseCleanup.LoadForSelfTest();
        window.OnImageShown = ImageShown;
        window.OnPreviewShown = () => _previewMs ??= Now - _stepStarted;
        window.OnAnimationFrame = () => _framesSeen++;
        window.OnProblem = Problem;
        // 复制、移动完成的提示：正在等的那一步（如 copy_to_folder1）就此完成。
        window.OnNotice = message =>
        {
            Note("notice", message);
            _lastNotice = message;
            if (_waitingImage)
            {
                _waitingImage = false;
                var record = _pendingRecord ?? new() { ["step"] = CurrentStep };
                _pendingRecord = null;
                record["notice"] = message;
                _records.Add(record);
                CaptureLater();
            }
        };
        // 删掉唯一一张图之类的步骤：回到空态就算完成。
        window.OnImageClosed = () =>
        {
            if (_waitingImage)
            {
                _waitingImage = false;
                var record = _pendingRecord ?? new() { ["step"] = CurrentStep };
                _pendingRecord = null;
                record["closed"] = true;
                _records.Add(record);
                CaptureLater();
            }
        };
        // 程序里记下的异常（事件处理出错等）都算问题：不会带着异常通过。
        Crash.Observer = text => _window.DispatcherQueue.TryEnqueue(() => _problems.Add($"异常：{text}"));
        window.OnExternal = (kind, value) =>
        {
            var external = new Dictionary<string, object?> { ["kind"] = kind, ["value"] = value };
            if (kind == "print")
            {
                // 打印只记下来、不弹对话框；对话框之前"取默认打印机的设置、按图片改纸张方向"那一步照做一遍（不弹窗口、
                // 不打印）：横图应改成横向（2），竖图竖向（1）。没有打印机时是 none。
                external["printDefaults"] = $"{Printing.ProbeDefaults(_window.Hwnd, true)} {Printing.ProbeDefaults(_window.Hwnd, false)}";
            }
            _externals.Add(external);
        };
        // 图片被安全上限拦下：询问的对话框真的弹出来了才算，记下这一步（blocked）和对话框的内容；对话框开着，
        // 下一步 limit_open、limit_cancel 充当用户按下的按钮。
        window.OnDialogOpened = (prompt, defaultButton, answer) =>
        {
            _dialogAnswer = answer;
            _dialogPrompt = prompt;
            _dialogButtons = prompt.Buttons.Length;
            if (_finished || !_waitingImage)
            {
                return;
            }
            _waitingImage = false;
            var record = _pendingRecord ?? new() { ["step"] = CurrentStep };
            _pendingRecord = null;
            record["name"] = _window.CurrentName;
            record["blocked"] = true;
            record["dialog"] = new Dictionary<string, object?>
            {
                ["message"] = prompt.Message,
                ["buttons"] = prompt.Buttons,
                ["default"] = defaultButton.ToString(),
            };
            _records.Add(record);
            // 这时画面上没有图片（等用户回答）：不截图核对，直接做下一步。
            _window.After(1, Advance);
        };
        window.OnBoundary = forward =>
        {
            _boundary = forward ? "last" : "first";
            // 到了首尾：这一步没有新图，记下后继续。
            if (_waitingImage)
            {
                _waitingImage = false;
                var record = _pendingRecord ?? new() { ["step"] = CurrentStep };
                _pendingRecord = null;
                record["boundary"] = _boundary;
                _records.Add(record);
                CaptureLater();
            }
            return true;
        };
        // 右键菜单真的弹出来（Opened）才算，随即关掉。
        window.OnContextMenu = menu => menu.Opened += (_, _) =>
        {
            _menuShown++;
            menu.Hide();
        };
    }

    private double Now => _clock.Elapsed.TotalMilliseconds;

    /// <summary>正在做的一步（记录里的第一条是打开）。</summary>
    private string CurrentStep => _records.Count == 0
        ? "open"
        : _records.Count - 1 < _options.Steps.Length ? _options.Steps[_records.Count - 1] : "finish";

    /// <summary>自检进行中窗口被关掉了（手动关的，或者出了问题）：算不通过，写出报告后结束。</summary>
    public void WindowClosed() => Fail($"{CurrentStep}：自检还没做完，窗口就被关掉了");

    /// <summary>是不是自检操作的那个窗口（其他窗口是 finder_open 开的）。</summary>
    public bool IsTestWindow(MainWindow window) => ReferenceEquals(window, _window);

    /// <summary>设置窗口的"确定"已经生效。</summary>
    public void SettingsApplied(SettingsApplyResult.Applied applied) => _lastApplied = applied;

    /// <summary>没接住的异常（自检时由应用交过来）：记下来，照样写出报告。</summary>
    public void Crashed(Exception error)
    {
        Crash.Write("SelfTest", error);
        Fail($"{CurrentStep} 出了异常：{error.GetType().Name}: {error.Message}");
    }

    public void Start()
    {
        // 万一卡住：90 秒后照样写报告退出。
        _window.After(90000, () => Fail("超时（90 秒没有完成）"));
        _stepStarted = 0;
        if (_options.OpenPath is null)
        {
            _window.OnPong = () =>
            {
                _window.OnPong = null;
                _records.Add(new() { ["step"] = "empty", ["pingMs"] = _bridge.LastPingMs });
                CaptureLater();
            };
            _bridge.Ping();
            return;
        }
        // 打开已经在启动时交给核心了，这里等第一张图。
        _waitingImage = true;
        _pendingRecord = new() { ["step"] = "open" };
        WatchStep(_pendingRecord, "open", 30000);
    }

    /// <summary>等新图像的一步：超过时间还没等到就算不通过，说明是哪一步（不用等到整个自检超时）。</summary>
    private void WatchStep(Dictionary<string, object?> record, string label, uint ms = 15000)
    {
        _window.After(ms, () =>
        {
            if (!_finished && _waitingImage && ReferenceEquals(_pendingRecord, record))
            {
                Fail($"{label}：{ms / 1000} 秒内没有等到新图像（这一步没有换图，或者读得太慢）");
            }
        });
    }

    /// <summary>程序要退出（Esc 等）：自检时不真的退出，记下这一步后结束自检。</summary>
    public void QuitRequested()
    {
        var record = _pendingRecord ?? new() { ["step"] = CurrentStep };
        if (!_records.Contains(record))
        {
            _records.Add(record);
        }
        record["quit"] = true;
        Finish();
    }

    public void UiRequested(UiRequest request) => _requests.Add(request.GetType().Name);

    /// <summary>
    /// 打不开、提示（如已到末尾）都算问题；正在等图像的步骤就此结束，记下原因、继续。
    /// HEIC 由 Windows 的扩展解码：这台电脑没装"HEIF 图像扩展"和"HEVC 视频扩展"时打不开，只记下来，不算不通过。
    /// AVIF 没有扩展时由核心自己解，打不开就算问题。
    /// </summary>
    private void Problem(string message)
    {
        var name = _window.CurrentName;
        var extension = Path.GetExtension(name).TrimStart('.').ToLowerInvariant();
        if (extension is "heic" or "heif" or "hif" or "heics")
        {
            Note("needsWindowsExtension", $"{name}: {message}");
        }
        else
        {
            _problems.Add($"{CurrentStep}: {message}");
        }
        if (_waitingImage)
        {
            _waitingImage = false;
            var record = _pendingRecord ?? new() { ["step"] = CurrentStep };
            _pendingRecord = null;
            record["failed"] = message;
            _records.Add(record);
            CaptureLater();
        }
    }

    private void Note(string kind, string value) => _externals.Add(new() { ["kind"] = kind, ["value"] = value });

    private void Fail(string message)
    {
        if (_finished)
        {
            return;
        }
        _failure ??= message;
        Finish();
    }

    // ---- 等图像 ----

    private void ImageShown(ImageHandle image)
    {
        if (!_waitingImage)
        {
            return;
        }
        // 大 JPEG 先出快速预览：截图等清晰图（ImageUpdated 也走这里）。
        if (_window.Canvas.ShowingPreview)
        {
            return;
        }
        _waitingImage = false;
        _previousColor = _currentColor;
        _currentColor = CenterColor(image);
        var record = _pendingRecord ?? new() { ["step"] = CurrentStep };
        _pendingRecord = null;
        record["imageMs"] = Now - _stepStarted;
        if (_previewMs is double p)
        {
            record["previewMs"] = p;
        }
        record["decodeMs"] = image.DecodeMs();
        if (_records.Count == 0)
        {
            // 冷启动：从进程创建算起。
            record["sinceProcessMs"] = LaunchClock.Now;
            record["marks"] = LaunchClock.Summary();
        }
        _records.Add(record);
        CaptureLater();
    }

    // ---- 截图与核对 ----

    /// <summary>等画面更新（布局跟上窗口的大小；需要精确重绘时等它盖上，最多 3 秒），再截图核对。</summary>
    private void CaptureLater()
    {
        var serial = ++_captureSerial;
        var started = Now;
        void Check()
        {
            if (_finished || serial != _captureSerial)
            {
                return;
            }
            var core = _window.Core;
            var waitExact = _window.Canvas.HasImage && core.NeedsExact() && !_window.Canvas.ExactShown;
            if ((waitExact || _window.Canvas.TransitionState is not null && _window.Canvas.HoldTransition is null) && Now - started < 3000)
            {
                _window.After(50, Check);
                return;
            }
            WhenSettled(() => _ = CaptureStep());
        }
        _window.After(120, Check);
    }

    /// <summary>
    /// 等画布跟上窗口的大小（窗口改了大小以后，画布在下一次布局时才变）再截图，最多等 2 秒。
    /// 不等屏幕画下一帧：画面不动时 WinUI 不画新的一帧，那样会一直等下去（要点一下窗口才继续）。
    /// 截图本身是另画一张（Snapshot），不依赖屏幕上的帧。
    /// </summary>
    private void WhenSettled(Action action)
    {
        var started = Now;
        void Check()
        {
            if (_finished)
            {
                return;
            }
            // 普通窗口：图片区（内容区）要等于按图片定的大小；最大化、全屏：画布铺满客户区（差不过 2 像素）。
            var content = _window.ContentSize;
            bool settled;
            if (_window.WindowMovable && _window.LastFitSize is Size fit)
            {
                settled = Math.Abs(content.Width - fit.Width) < 1 && Math.Abs(content.Height - fit.Height) < 1;
            }
            else
            {
                var client = _window.ClientSizeDips;
                settled = client.Width < 1 || client.Height < 1
                    || (Math.Abs(content.Width - client.Width) < 2 && Math.Abs(content.Height - client.Height) < 2);
            }
            if (settled || Now - started > 2000)
            {
                action();
                return;
            }
            _window.After(30, Check);
        }
        Check();
    }

    private async Task CaptureStep()
    {
        if (_finished)
        {
            return;
        }
        var record = _records[^1];
        var index = _records.Count - 1;
        try
        {
            record["name"] = _window.CurrentName;
            record["index"] = _window.CurrentIndex;
            record["total"] = _window.CurrentTotal;
            record["title"] = _window.Title;
            record["layout"] = LayoutInfo(_window.Core.ViewLayout());
            var client = _window.ClientSizeDips;
            var content = _window.ContentSize;
            record["window"] = new Dictionary<string, object?>
            {
                ["clientWidth"] = Math.Round(client.Width, 1),
                ["clientHeight"] = Math.Round(client.Height, 1),
                ["contentWidth"] = Math.Round(content.Width, 1),
                ["contentHeight"] = Math.Round(content.Height, 1),
                ["fitWidth"] = _window.LastFitSize?.Width,
                ["fitHeight"] = _window.LastFitSize?.Height,
                ["dpiScale"] = _window.DpiScale,
                ["maximized"] = _window.IsMaximized,
                ["fullScreen"] = _window.FullScreen,
                ["titleBar"] = _window.TitleBarShown,
            };
            record["notice"] = _window.NoticeText;
            record["zoomHint"] = _window.ZoomHintText;
            record["filename"] = _window.FilenameText;
            record["animation"] = _window.Overlays.AnimationText;
            record["sounds"] = _window.SoundLog.Count;
            var core = _window.Core;
            // 多个窗口、压缩包里不能删除、记住的压缩包位置、阻止屏保、背景图片。
            record["windowCount"] = core.WindowCount();
            record["windows"] = WindowsInfo();
            record["deleteEnabled"] = core.ActionState("delete_file").Enabled;
            record["archivePositions"] = core.ArchivePositionsCount();
            if (_lastNotice is not null)
            {
                record["lastNotice"] = _lastNotice;
            }
            record["preventingSleep"] = _app.KeepingDisplayOn;
            // 菜单栏显示着没有；最近打开的（"打开最近使用"）。
            record["menuBar"] = _window.MenuBarShown;
            record["recent"] = core.RecentFiles();
            record["background"] = _window.Canvas.HasBackgroundImage;
            // 显示着的图片：格式、像素格式、宽高（与 macOS 的报告相同，更多格式那一项按它核对）。
            if (_window.Canvas.CurrentHandle is { } handle && _window.Canvas.HasImage)
            {
                record["format"] = handle.FormatName();
                record["pixelFormat"] = handle.PixelFormat();
                record["width"] = handle.Width();
                record["height"] = handle.Height();
                record["hasProxy"] = handle.HasProxy();
            }
            // 系统的解码器（WIC）解不了这个文件时的原因（核心随后用自己的解码器，或报不支持）。
            if (core.CurrentPath() is string currentPath && WicDecoder.ErrorFor(currentPath) is string wicError)
            {
                record["wicError"] = wicError;
            }
            if (core.Animation() is AnimationInfo animation)
            {
                record["animationInfo"] = new Dictionary<string, object?>
                {
                    ["index"] = animation.Index,
                    ["count"] = animation.Count,
                    ["playing"] = animation.Playing,
                };
            }
            // 设置窗口、选择图片窗口开着时另截一张，并记下它的内容。
            if (_app.Settings is SettingsWindow settings)
            {
                record["settings"] = new Dictionary<string, object?>
                {
                    ["page"] = settings.PageSummary(),
                    ["dirty"] = core.SettingsDirty(),
                    ["screenshot"] = await settings.SnapshotAsync(Path.Combine(_dir, $"step-{index:00}-settings.png")),
                };
            }
            if (_window.Picker is { IsClosed: false } picker)
            {
                var info = PickerInfo(picker);
                info["screenshot"] = await picker.SnapshotAsync(Path.Combine(_dir, $"step-{index:00}-picker.png"));
                record["picker"] = info;
            }
            if (_finished)
            {
                return;
            }
            if (_boundary is not null)
            {
                record["boundary"] = _boundary;
                _boundary = null;
            }
            if (_window.Canvas.ExactInfo is { } exact)
            {
                record["exact"] = new Dictionary<string, object?> { ["x"] = exact.X, ["y"] = exact.Y, ["width"] = exact.W, ["height"] = exact.H, ["renderMs"] = exact.Ms };
            }
            if (_window.Canvas.TransitionState is { } turn)
            {
                record["transition"] = new Dictionary<string, object?> { ["effect"] = turn.Effect.ToString(), ["forward"] = turn.Forward, ["progress"] = turn.Progress };
            }
            var (overlay, overlayProblems) = OverlayState();
            record["overlay"] = overlay;
            foreach (var p in overlayProblems)
            {
                _problems.Add($"{record["step"]}: {p}");
            }
            // 窗口跟着图片：普通窗口的图片区（内容区）应是按图片定的大小。
            if (_window.WindowMovable && _window.LastFitSize is Size fit
                && (Math.Abs(fit.Width - content.Width) > 1 || Math.Abs(fit.Height - content.Height) > 1))
            {
                _problems.Add($"{record["step"]}: 图片区 {content.Width:0.#}x{content.Height:0.#}，应为 {fit.Width:0.#}x{fit.Height:0.#}");
            }
            using var shot = _window.Canvas.Snapshot();
            if (shot is null)
            {
                record["screenshot"] = null;
                _problems.Add($"{record["step"]}: 截不到画布");
            }
            else
            {
                // 先核对、后存图：存图要等，等的时候动图可能已经换了一帧。
                var bytes = shot.GetPixelBytes();
                var width = (int)shot.SizeInPixels.Width;
                var height = (int)shot.SizeInPixels.Height;
                record["size"] = new[] { width, height };
                // 窗口左上角附近的颜色：图片没盖住时就是背景色或背景图片。
                record["cornerColor"] = Hex(bytes, width, Math.Min(24, width - 1), Math.Min(24, height - 1));
                if (record["step"] is "empty")
                {
                    var center = Hex(bytes, width, width / 2, height / 2);
                    record["centerColor"] = center;
                    record["emptyShown"] = _window.EmptyShown;
                    if (center != "#202020")
                    {
                        _problems.Add($"空态背景色 {center}，应为 #202020");
                    }
                    if (!_window.EmptyShown)
                    {
                        _problems.Add("空态的提示没有显示");
                    }
                }
                else
                {
                    // transition_hold 之后换了图：翻页效果必须真的在、停在设定的进度上（没出现时不能拿平常的核对蒙混过去）。
                    if (_window.Canvas.HoldTransition is double hold && record.ContainsKey("imageMs")
                        && _window.Canvas.TransitionState is null)
                    {
                        _problems.Add($"{record["step"]}: 翻页效果没有出现（应停在 {hold}）");
                    }
                    // 翻页效果停在中间时（transition_hold）画面是两张叠在一起的：按效果核对几个点，不做平常的画面核对。
                    var samples = _window.Canvas.TransitionState is { } held
                        ? TransitionCheck(bytes, width, height, held)
                        : CheckSamples(bytes, width, height);
                    record["samples"] = samples.Info;
                    if (!samples.Skipped && (samples.Checked == 0 || samples.Mismatches > 0))
                    {
                        _problems.Add($"{record["step"]}: 画面核对不符（核对了 {samples.Checked} 个点，{samples.Mismatches} 个不符）");
                    }
                }
                var file = $"step-{index:00}.png";
                await shot.SaveAsync(Path.Combine(_dir, file), CanvasBitmapFileFormat.Png);
                record["screenshot"] = file;
            }
        }
        catch (Exception error)
        {
            _problems.Add($"{record["step"]}: 截图出错 {error.Message}");
        }
        Advance();
    }

    private static Dictionary<string, object?>? LayoutInfo(ViewLayout? l) => l is null ? null : new()
    {
        ["zoom"] = Math.Round(l.Zoom, 4),
        ["scale"] = Math.Round(l.Scale, 4),
        ["rotation"] = l.Rotation,
        ["flipH"] = l.FlipHorizontal,
        ["flipV"] = l.FlipVertical,
        ["center"] = new[] { Math.Round(l.CenterX, 1), Math.Round(l.CenterY, 1) },
        ["size"] = new[] { Math.Round(l.Width, 1), Math.Round(l.Height, 1) },
        ["smooth"] = l.Smooth,
    };

    private (Dictionary<string, object?> Info, List<string> Problems) OverlayState()
    {
        var overlays = _window.Overlays;
        var core = _window.Core;
        var locked = core.ActionState("toggle_bar_lock").Checked;
        var infoOn = core.ActionState("toggle_info").Checked;
        var lines = core.InfoLines(_window.Lang);
        var infoShown = overlays.Info.Visibility == Visibility.Visible;
        var problems = new List<string>();
        var hidden = _window.Settings.HideControlBar;
        var wanted = !hidden && (locked || _hoverBar);
        if (overlays.BarFade.IsShown != wanted && !(!wanted && overlays.BarHidePending))
        {
            problems.Add($"控制栏显示={overlays.BarFade.IsShown}，锁定={locked}，指针在底部={_hoverBar}，设置不显示={hidden}");
        }
        if (infoShown != (infoOn && lines.Length > 0))
        {
            problems.Add($"图片信息显示={infoShown}，开关={infoOn}");
        }
        if (infoShown && overlays.InfoText.Text != string.Join("\n", lines))
        {
            problems.Add("图片信息的文字与核心不同");
        }
        double? expectedKnob = _window.CurrentIndex is uint i && _window.CurrentTotal is uint t ? AnyviewCore.ProgressFraction(i, t) : null;
        if (overlays.Strip.KnobFraction != expectedKnob)
        {
            problems.Add($"进度条滑块 {overlays.Strip.KnobFraction}，应为 {expectedKnob}");
        }
        return (new()
        {
            ["barHidden"] = hidden,
            ["barShown"] = overlays.BarFade.IsShown,
            ["barHidePending"] = overlays.BarHidePending,
            ["barLocked"] = locked,
            ["knob"] = overlays.Strip.KnobFraction,
            ["infoShown"] = infoShown,
            ["infoText"] = infoShown ? lines : Array.Empty<string>(),
        }, problems);
    }

    private static string Hex(byte[] bgra, int width, int x, int y)
    {
        var i = (y * width + x) * 4;
        return $"#{bgra[i + 2]:X2}{bgra[i + 1]:X2}{bgra[i]:X2}";
    }

    /// <summary>
    /// 画面核对：图片上 4×4 个点按核心的映射找到画布上的位置，比较颜色（与 macOS 的自检相同）。这些点都不在窗口里时，
    /// 改在可见区域里均匀取点再换回图片坐标。放大显示时期望值是附近 2×2 个图片像素的颜色范围。
    /// </summary>
    private (int Checked, int Mismatches, bool Skipped, Dictionary<string, object?> Info) CheckSamples(byte[] shot, int sw, int sh)
    {
        var core = _window.Core;
        if (core.CurrentPath() is null || _window.Canvas.CurrentHandle is not ImageHandle image)
        {
            return (0, 0, true, new() { ["checked"] = 0, ["skipped"] = "空态" });
        }
        if (_options.SampleAt100 && core.ViewLayout() is ViewLayout l && Math.Abs(l.Zoom - 1) > 1e-9)
        {
            return (0, 0, true, new() { ["checked"] = 0, ["skipped"] = "只在 100% 时核对" });
        }
        if (image.IccProfile() is not null)
        {
            return (0, 0, true, new() { ["checked"] = 0, ["skipped"] = "图片带 ICC" });
        }
        if (_window.Canvas.ShowingPreview)
        {
            return (0, 0, true, new() { ["checked"] = 0, ["skipped"] = "快速预览" });
        }
        var magnified = (core.ViewLayout()?.Scale ?? 1) > 1 + 1e-6;
        var width = (double)image.Width();
        var height = (double)image.Height();
        var stride = (int)image.Stride();
        var basePtr = (nint)image.PixelAddress();
        const int margin = 8;
        bool OnScreen(double u, double v)
        {
            if (core.ViewImageToScreen(u, v) is not ScreenPoint p)
            {
                return false;
            }
            var x = (int)p.X;
            var y = (int)p.Y;
            return x >= margin && y >= margin && x < sw - margin && y < sh - margin;
        }
        static double PixelCenter(double t) => Math.Floor(t) + 0.5;
        var fractions = new[] { 0.125, 0.375, 0.625, 0.875 };
        var points = new List<(double U, double V)>();
        foreach (var fy in fractions)
        {
            foreach (var fx in fractions)
            {
                points.Add((PixelCenter(fx * width), PixelCenter(fy * height)));
            }
        }
        if (!points.Any(p => OnScreen(p.U, p.V)))
        {
            var corners = new[] { (0.0, 0.0), (width, 0.0), (0.0, height), (width, height) }
                .Select(c => core.ViewImageToScreen(c.Item1, c.Item2)).Where(c => c is not null).Select(c => c!).ToList();
            if (corners.Count > 0)
            {
                var left = Math.Max(corners.Min(c => c.X), 0);
                var right = Math.Min(corners.Max(c => c.X), sw);
                var top = Math.Max(corners.Min(c => c.Y), 0);
                var bottom = Math.Min(corners.Max(c => c.Y), sh);
                points.Clear();
                foreach (var fy in fractions)
                {
                    foreach (var fx in fractions)
                    {
                        if (core.ViewScreenToImage(left + fx * (right - left), top + fy * (bottom - top)) is ScreenPoint p
                            && p.X >= 0 && p.Y >= 0 && p.X < width && p.Y < height)
                        {
                            points.Add((PixelCenter(p.X), PixelCenter(p.Y)));
                        }
                    }
                }
            }
        }
        var fill = core.AlphaFill(image);
        var mismatches = new List<Dictionary<string, object?>>();
        var checkedCount = 0;
        byte[] Pixel(int i, int j)
        {
            var bytes = new byte[4];
            Marshal.Copy(basePtr + (nint)((long)j * stride + (long)i * 4), bytes, 0, 4);
            return bytes;
        }
        foreach (var (u, v) in points)
        {
            if (core.ViewImageToScreen(u, v) is not ScreenPoint point)
            {
                continue;
            }
            var x = (int)point.X;
            var y = (int)point.Y;
            if (x < margin || y < margin || x >= sw - margin || y >= sh - margin)
            {
                continue;
            }
            var around = new List<(int I, int J)> { ((int)u, (int)v) };
            if (magnified && core.ViewScreenToImage(x + 0.5, y + 0.5) is ScreenPoint q)
            {
                var i0 = Math.Clamp((int)Math.Floor(q.X - 0.5), 0, (int)width - 1);
                var j0 = Math.Clamp((int)Math.Floor(q.Y - 0.5), 0, (int)height - 1);
                var i1 = Math.Min(i0 + 1, (int)width - 1);
                var j1 = Math.Min(j0 + 1, (int)height - 1);
                around = new() { (i0, j0), (i1, j0), (i0, j1), (i1, j1) };
            }
            var pixels = around.Select(a => Pixel(a.I, a.J)).ToList();
            List<(int[] Low, int[] High)> ranges;
            if (pixels.All(p => p[3] == 255))
            {
                var low = new[] { 255, 255, 255 };
                var high = new[] { 0, 0, 0 };
                foreach (var p in pixels)
                {
                    for (var k = 0; k < 3; k++)
                    {
                        low[k] = Math.Min(low[k], p[k]);
                        high[k] = Math.Max(high[k], p[k]);
                    }
                }
                ranges = new() { (low, high) };
            }
            else if (pixels.All(p => p[3] == 0))
            {
                if (fill is AlphaFill.Solid solid)
                {
                    var c = new[] { (int)(solid.Rgb & 0xFF), (int)((solid.Rgb >> 8) & 0xFF), (int)((solid.Rgb >> 16) & 0xFF) };
                    ranges = new() { (c, c) };
                }
                else
                {
                    ranges = new() { (new[] { 255, 255, 255 }, new[] { 255, 255, 255 }), (new[] { 200, 200, 200 }, new[] { 200, 200, 200 }) };
                }
            }
            else
            {
                continue;
            }
            checkedCount++;
            var at = (y * sw + x) * 4;
            int Distance((int[] Low, int[] High) r)
            {
                var worst = 0;
                for (var k = 0; k < 3; k++)
                {
                    int value = shot[at + k];
                    worst = Math.Max(worst, Math.Max(r.Low[k] - value, value - r.High[k]));
                }
                return worst;
            }
            if (ranges.Min(Distance) > ColorTolerance)
            {
                var e = ranges[0].Low;
                mismatches.Add(new()
                {
                    ["image"] = new[] { u, v },
                    ["screen"] = new[] { x, y },
                    ["expected"] = $"#{e[2]:X2}{e[1]:X2}{e[0]:X2}",
                    ["got"] = Hex(shot, sw, x, y),
                });
            }
        }
        GC.KeepAlive(image);
        string? centerColor = null;
        if (core.ViewImageToScreen(width / 2, height / 2) is ScreenPoint c0 && c0.X >= 0 && c0.Y >= 0 && c0.X < sw && c0.Y < sh)
        {
            centerColor = Hex(shot, sw, (int)c0.X, (int)c0.Y);
        }
        return (checkedCount, mismatches.Count, false, new()
        {
            ["checked"] = checkedCount,
            ["mismatches"] = mismatches,
            ["centerColor"] = centerColor,
        });
    }

    /// <summary>图片中心的颜色（RGB）。</summary>
    private static int[]? CenterColor(ImageHandle image)
    {
        var address = (nint)image.PixelAddress();
        if (address == 0)
        {
            return null;
        }
        var offset = (long)(image.Height() / 2) * image.Stride() + (long)(image.Width() / 2) * 4;
        var p = new byte[4];
        Marshal.Copy(address + (nint)offset, p, 0, 4);
        GC.KeepAlive(image);
        return new[] { (int)p[2], (int)p[1], (int)p[0] };
    }

    /// <summary>
    /// 翻页效果停在中间时的画面核对（与 macOS 的自检相同）：两张图都是纯色（effects/），按效果看几个点的颜色。
    /// 淡出：中间是两种颜色按进度混合；滑动：往后翻左边是旧图、右边是新图（往前翻相反）；
    /// 翻页：往后翻旧的一页以左边为轴翻起（停住时是平面压扁的画法），右边露出新图、左边是变暗的旧图。
    /// </summary>
    private (int Checked, int Mismatches, bool Skipped, Dictionary<string, object?> Info) TransitionCheck(
        byte[] shot, int sw, int sh, (PageEffect Effect, bool Forward, double Progress) state)
    {
        if (_window.Core.ViewLayout() is not ViewLayout layout || _previousColor is not int[] old || _currentColor is not int[] neu)
        {
            return (0, 0, false, new() { ["checked"] = 0, ["error"] = "没有画面参数或新旧两张图的颜色" });
        }
        var t = state.Progress;
        var eased = t * t * (3 - 2 * t);
        static int[] Mix(int[] a, int[] b, double k) =>
            new[] { 0, 1, 2 }.Select(i => (int)Math.Round(a[i] * (1 - k) + b[i] * k)).ToArray();
        int[] Dark(int[] c) => Mix(c, new[] { 0, 0, 0 }, 0.35 * eased);
        var first = state.Forward ? old : neu;
        var second = state.Forward ? neu : old;
        var expected = state.Effect switch
        {
            PageEffect.Fade => new[] { (0.5, Mix(old, neu, eased)) },
            PageEffect.Slide => new[] { (0.25, first), (0.75, second) },
            PageEffect.Page => state.Forward ? new[] { (0.03, Dark(old)), (0.95, neu) } : new[] { (0.03, Dark(neu)), (0.95, old) },
            _ => Array.Empty<(double, int[])>(),
        };
        var left = layout.CenterX - layout.Width / 2;
        var y = (int)Math.Max(0, layout.CenterY);
        var points = new List<Dictionary<string, object?>>();
        var mismatches = 0;
        foreach (var (fx, want) in expected)
        {
            var x = (int)Math.Max(0, left + layout.Width * fx);
            if (x >= sw || y >= sh)
            {
                mismatches++;
                continue;
            }
            var at = (y * sw + x) * 4;
            var got = new[] { (int)shot[at + 2], (int)shot[at + 1], (int)shot[at] };
            var ok = got.Zip(want).All(pair => Math.Abs(pair.First - pair.Second) <= 30);
            if (!ok)
            {
                mismatches++;
            }
            points.Add(new() { ["x"] = fx, ["got"] = got, ["want"] = want, ["ok"] = ok });
        }
        return (expected.Length, mismatches, false, new()
        {
            ["checked"] = expected.Length,
            ["mismatches"] = mismatches,
            ["transition"] = new Dictionary<string, object?>
            {
                ["effect"] = state.Effect.ToString(),
                ["forward"] = state.Forward,
                ["progress"] = state.Progress,
                ["points"] = points,
            },
        });
    }

    // ---- 步骤 ----

    private void Advance()
    {
        _stepStarted = Now;
        _previewMs = null;
        _framesSeen = 0;
        var index = _records.Count - 1;
        if (index >= _options.Steps.Length)
        {
            Finish();
            return;
        }
        var id = _options.Steps[index];
        try
        {
            RunStep(id);
        }
        catch (Exception error)
        {
            Fail($"{id} 出错：{error}");
        }
    }

    private void Record(string id, Dictionary<string, object?>? extra = null)
    {
        var record = new Dictionary<string, object?> { ["step"] = id };
        if (extra is not null)
        {
            foreach (var (k, v) in extra)
            {
                record[k] = v;
            }
        }
        _records.Add(record);
        CaptureLater();
    }

    /// <summary>各线程到现在用了多少 CPU（线程号 → 时间）。</summary>
    private static Dictionary<int, TimeSpan> ThreadTimes(Process process)
    {
        var times = new Dictionary<int, TimeSpan>();
        process.Refresh();
        foreach (ProcessThread thread in process.Threads)
        {
            try
            {
                times[thread.Id] = thread.TotalProcessorTime;
            }
            catch (Exception)
            {
            }
        }
        return times;
    }

    /// <summary>这段时间里用 CPU 最多的几个线程：线程号、毫秒、线程从哪个模块开始（界面线程另标出）。</summary>
    private static List<string> BusyThreads(Process process, Dictionary<int, TimeSpan> before)
    {
        var modules = new List<(long Start, long End, string Name)>();
        try
        {
            foreach (ProcessModule module in process.Modules)
            {
                modules.Add((module.BaseAddress.ToInt64(), module.BaseAddress.ToInt64() + module.ModuleMemorySize, module.ModuleName));
            }
        }
        catch (Exception)
        {
        }
        var busy = new List<(double Ms, string Text)>();
        foreach (ProcessThread thread in process.Threads)
        {
            try
            {
                var ms = (thread.TotalProcessorTime - before.GetValueOrDefault(thread.Id)).TotalMilliseconds;
                if (ms < 1)
                {
                    continue;
                }
                var start = thread.StartAddress.ToInt64();
                var module = modules.FirstOrDefault(m => start >= m.Start && start < m.End).Name ?? "?";
                var ui = thread.Id == App.UiThreadId ? "，界面线程" : "";
                busy.Add((ms, $"{thread.Id}：{ms:0} ms（{module}{ui}）"));
            }
            catch (Exception)
            {
            }
        }
        return busy.OrderByDescending(b => b.Ms).Take(6).Select(b => b.Text).ToList();
    }

    private void RunStep(string id)
    {
        string? Arg(string prefix) => id.StartsWith(prefix, StringComparison.Ordinal) ? id[prefix.Length..] : null;
        if (id == "wait_exact")
        {
            var started = Now;
            void Check()
            {
                if (_window.Canvas.ExactShown || Now - started > 3000)
                {
                    Record(id, new() { ["exactShown"] = _window.Canvas.ExactShown, ["waitedMs"] = Now - started });
                    if (!_window.Canvas.ExactShown)
                    {
                        _problems.Add($"{id}: 精确重绘 3 秒内没有出现");
                    }
                    return;
                }
                _window.After(30, Check);
            }
            Check();
            return;
        }
        // about_licenses："关于"对话框：打开它、记下文字和按钮，按"开源许可"，核对开源许可窗口打开了（截图、记下文字长度）。
        if (id == "about_licenses")
        {
            _dialogAnswer = null;
            _dialogPrompt = null;
            _window.Invoke("about");
            var started = DateTime.UtcNow;
            void WaitForAbout()
            {
                if (_finished)
                {
                    return;
                }
                if (_dialogAnswer is { } answer && _dialogPrompt is { } prompt)
                {
                    var about = new Dictionary<string, object?> { ["message"] = prompt.Message, ["buttons"] = prompt.Buttons };
                    _dialogAnswer = null;
                    _dialogPrompt = null;
                    answer(0);
                    _window.After(400, () => _ = RecordHelp(id, about));
                    return;
                }
                if ((DateTime.UtcNow - started).TotalSeconds > 5)
                {
                    Fail("“关于”对话框没有打开");
                    return;
                }
                _window.After(30, WaitForAbout);
            }
            WaitForAbout();
            return;
        }
        // 被上限拦下后用户的选择：在开着的询问对话框上按"仍然打开"（第一个按钮，等图片显示出来）或"取消"（最后一个）。
        if (id is "limit_open" or "limit_cancel")
        {
            if (_dialogAnswer is not { } answer)
            {
                Fail($"{id}：没有开着的询问对话框");
                return;
            }
            _dialogAnswer = null;
            if (id == "limit_open")
            {
                _waitingImage = true;
                _pendingRecord = new() { ["step"] = id };
                WatchStep(_pendingRecord, id);
                answer(0);
            }
            else
            {
                answer(_dialogButtons - 1);
                Record(id);
            }
            return;
        }
        if (Arg("wait:") is string wait && uint.TryParse(wait, out var waitMs))
        {
            _window.After(waitMs, () => Record(id));
            return;
        }
        if (Arg("idle:") is string idle && uint.TryParse(idle, out var idleMs))
        {
            // 静止不操作时 CPU 占用应接近 0%（没有定时重绘）。另记下这段时间里各线程用了多少 CPU（按线程从哪个模块开始的
            // 分开：coreclr 是 .NET 自己的，anyview_ffi 是核心的），JIT 编译、垃圾回收、画布重画的次数，找出是谁在占用。
            var process = Process.GetCurrentProcess();
            var before = process.TotalProcessorTime;
            var threadsBefore = ThreadTimes(process);
            var jitBefore = System.Runtime.JitInfo.GetCompilationTime(false);
            var jitMethodsBefore = System.Runtime.JitInfo.GetCompiledMethodCount(false);
            var gcBefore = GC.CollectionCount(0);
            var drawsBefore = _window.Canvas.DrawCount;
            var diagBefore = (Diag.Drains, Diag.DelayTicks, Diag.PointerMoves, Diag.WindowChanges, Diag.AutomationQueries);
            // XAML 画了几帧：静止时应该一帧也不画（有一直在动的动画时会每秒画几十帧）。只数 Rendered，不订 Rendering
            // （订了 Rendering 会让它每帧都画）。
            var renderedFrames = 0;
            EventHandler<RenderedEventArgs> countFrame = (_, _) => renderedFrames++;
            CompositionTarget.Rendered += countFrame;
            _window.After(idleMs, async () =>
            {
                CompositionTarget.Rendered -= countFrame;
                process.Refresh();
                var percent = (process.TotalProcessorTime - before).TotalMilliseconds / idleMs * 100;
                var extra = new Dictionary<string, object?>
                {
                    ["idleCpuPercent"] = Math.Round(percent, 2),
                    ["workingSetMb"] = Math.Round(process.WorkingSet64 / 1048576.0, 1),
                    ["privateMb"] = Math.Round(process.PrivateMemorySize64 / 1048576.0, 1),
                    ["peakWorkingSetMb"] = Math.Round(process.PeakWorkingSet64 / 1048576.0, 1),
                    ["cacheMb"] = Math.Round(_window.Core.CacheBytes() / 1048576.0, 1),
                    ["idleThreads"] = BusyThreads(process, threadsBefore),
                    ["idleJitMs"] = Math.Round((System.Runtime.JitInfo.GetCompilationTime(false) - jitBefore).TotalMilliseconds, 1),
                    ["idleJitMethods"] = System.Runtime.JitInfo.GetCompiledMethodCount(false) - jitMethodsBefore,
                    ["idleGcCount"] = GC.CollectionCount(0) - gcBefore,
                    ["idleDraws"] = _window.Canvas.DrawCount - drawsBefore,
                    ["idleFrames"] = renderedFrames,
                    ["idleCallbacks"] = $"核心事件 {Diag.Drains - diagBefore.Drains} 次，延时 {Diag.DelayTicks - diagBefore.DelayTicks} 次，" +
                        $"指针移动 {Diag.PointerMoves - diagBefore.PointerMoves} 次，窗口变化 {Diag.WindowChanges - diagBefore.WindowChanges} 次，" +
                        $"UI 自动化来问主窗口 {Diag.AutomationQueries - diagBefore.AutomationQueries} 次" +
                        (Diag.AutomationClientsListening() ? "（系统里有程序在用 UI 自动化）" : "（系统里没有程序在用 UI 自动化）"),
                };
                // 量完以后再另外采样 2 秒，看界面线程忙在哪个模块（采样本身要用 CPU，不算进上面的占用）。
                try
                {
                    extra["idleUiSamples"] = await Sampler.SampleAsync(App.UiThreadId, 2000);
                }
                catch (Exception error)
                {
                    extra["idleUiSamples"] = new List<string> { $"采样出错：{error.Message}" };
                }
                if (!_finished)
                {
                    Record(id, extra);
                }
            });
            return;
        }
        if (Arg("open:") is string open)
        {
            _waitingImage = true;
            _pendingRecord = new() { ["step"] = id };
            WatchStep(_pendingRecord, id);
            _bridge.Open(open);
            return;
        }
        if (Arg("window:") is string size)
        {
            var parts = size.Split('x');
            if (parts.Length != 2 || !double.TryParse(parts[0], NumberStyles.Float, CultureInfo.InvariantCulture, out var w)
                || !double.TryParse(parts[1], NumberStyles.Float, CultureInfo.InvariantCulture, out var h))
            {
                Fail($"步骤写错了：{id}");
                return;
            }
            _window.SetClientSize(w, h);
            Record(id);
            return;
        }
        if (Arg("wait_frames:") is string frames && int.TryParse(frames, out var wanted))
        {
            var started = Now;
            void Check()
            {
                if (_framesSeen >= wanted || Now - started > 5000)
                {
                    Record(id, new() { ["frames"] = _framesSeen });
                    if (_framesSeen < wanted)
                    {
                        _problems.Add($"{id}: 5 秒里只换了 {_framesSeen} 帧");
                    }
                    return;
                }
                _window.After(30, Check);
            }
            Check();
            return;
        }
        if (Arg("transition_hold:") is string hold)
        {
            if (double.TryParse(hold, NumberStyles.Float, CultureInfo.InvariantCulture, out var progress))
            {
                _window.Canvas.HoldTransition = progress;
            }
            else
            {
                _window.Canvas.ReleaseHeldTransition();
            }
            Record(id);
            return;
        }
        if (Arg("bar_jump:") is string jump && double.TryParse(jump, NumberStyles.Float, CultureInfo.InvariantCulture, out var fraction))
        {
            _waitingImage = true;
            _pendingRecord = new() { ["step"] = id };
            WatchStep(_pendingRecord, id);
            if (!_window.Overlays.Strip.JumpTo(fraction))
            {
                Fail($"进度条还不能跳转：{id}");
            }
            return;
        }
        if (id == "hover_bottom")
        {
            _hoverBar = true;
            var w = _window.Content is FrameworkElement root ? root.ActualWidth : 0;
            var h = _window.Content is FrameworkElement r2 ? r2.ActualHeight : 0;
            _window.Overlays.PointerMoved(w / 2, h - 10, w, h, _window.Core);
            Record(id);
            return;
        }
        if (SettingsStep(id) || PickerStep(id) || WindowStep(id) || FileStep(id))
        {
            return;
        }
        // help:shortcuts、help:formats 打开帮助窗口，截一张帮助窗口的图、记下行数，然后关掉它。
        // generation:state 记下“生成信息”面板开着没有、显示的是什么（开着时截面板的图）；generation:close 关掉它。
        // 面板由 key:P（或菜单）打开。
        if (Arg("generation:") is string generation)
        {
            if (generation == "close")
            {
                _app.Generation?.Window.Close();
            }
            _window.After(400, () => _ = RecordGeneration(id));
            return;
        }
        if (Arg("help:") is string help)
        {
            _window.Invoke(help switch { "shortcuts" => "help_shortcuts", "licenses" => "help_licenses", _ => "help_formats" });
            _window.After(400, () => _ = RecordHelp(id));
            return;
        }
        if (Arg("key:") is string keySpec)
        {
            if (ParseKey(keySpec) is not var (key, mods))
            {
                Fail($"步骤写错了：{id}");
                return;
            }
            var action = _window.Core.ActionForKey(key, mods);
            if (action is null)
            {
                Record(id, new() { ["key"] = keySpec, ["action"] = null });
                return;
            }
            RunAction(action, id, new() { ["key"] = keySpec, ["action"] = action });
            return;
        }
        if (Arg("mouse:") is string mouse)
        {
            MouseStep(id, mouse);
            return;
        }
        RunAction(id, id, null);
    }

    // ---- 设置窗口 ----

    /// <summary>界面相关的设置（自检报告用）。</summary>
    private Dictionary<string, object?> UiSettingsInfo()
    {
        var settings = _window.Core.UiSettings();
        return new()
        {
            ["backgroundColor"] = $"#{settings.BackgroundColor:X6}",
            ["alphaBackground"] = settings.AlphaBackground.ToString(),
            ["hideControlBar"] = settings.HideControlBar,
            ["hideSideArrows"] = settings.HideSideArrows,
            ["hideZoomInfo"] = settings.HideZoomInfo,
            ["hideTooltips"] = settings.HideTooltips,
            ["smallInfoFont"] = settings.SmallInfoFont,
            ["cornerNotices"] = settings.CornerNotices,
            ["doubleClickMaximize"] = settings.DoubleClickMaximize,
            ["alwaysOnTop"] = settings.AlwaysOnTop,
            ["noRecent"] = settings.NoRecent,
            ["showTitleBar"] = settings.ShowTitleBar,
            ["snapToEdges"] = settings.SnapToEdges,
            ["reuseWindow"] = settings.ReuseWindow,
            ["windowFollowsImage"] = _window.Core.WindowFollowsImage(),
        };
    }

    /// <summary>设置窗口的步骤；不是这类步骤时返回 false。</summary>
    private bool SettingsStep(string id)
    {
        if (!id.StartsWith("settings_", StringComparison.Ordinal))
        {
            return false;
        }
        var core = _window.Core;
        if (id == "settings_open")
        {
            var outcome = core.InvokeAction("settings");
            if (outcome is not ActionOutcome.Ui { Request: UiRequest.ShowSettings })
            {
                Fail($"设置没有打开：{outcome}");
                return true;
            }
            _window.Apply(outcome);
            if (_app.Settings is null)
            {
                Fail("设置窗口没有出现");
                return true;
            }
            var pages = core.SettingsPages(_window.Lang).Select(p => $"{p.Id} {p.Title}").ToArray();
            Record(id, new() { ["settingsPages"] = pages, ["uiSettings"] = UiSettingsInfo(), ["lang"] = _window.Lang.ToString() });
            return true;
        }
        if (_app.Settings is not SettingsWindow window)
        {
            Fail($"{id}：设置窗口没有打开");
            return true;
        }
        string? Arg(string prefix) => id.StartsWith(prefix, StringComparison.Ordinal) ? id[prefix.Length..] : null;
        if (Arg("settings_page:") is string page)
        {
            window.SelectPage(page);
            if (window.CurrentPage != page)
            {
                Fail($"没有这一页：{page}");
                return true;
            }
            Record(id);
        }
        else if (Arg("settings_button:") is string spec)
        {
            // settings_button:键:按钮：按设置项里的按钮（如书签页的"删除"，；自检时不弹确认框）。
            var colon = spec.LastIndexOf(':');
            if (colon <= 0)
            {
                Fail($"步骤写错了：{id}");
                return true;
            }
            window.PressButton(spec[..colon], spec[(colon + 1)..]);
            Record(id, new() { ["archivePositions"] = core.ArchivePositionsCount() });
        }
        else if (Arg("settings_set:") is string assignment)
        {
            var equals = assignment.IndexOf('=');
            if (equals <= 0)
            {
                Fail($"步骤写错了：{id}");
                return true;
            }
            var key = assignment[..equals];
            var value = assignment[(equals + 1)..];
            if (!window.SetForTest(key, value))
            {
                Fail($"这一页没有 {key}，或它不能改成 {value}");
                return true;
            }
            Record(id, new() { ["dirty"] = core.SettingsDirty() });
        }
        else if (id is "settings_ok" or "settings_ok_invalid")
        {
            _lastApplied = null;
            var applied = window.Ok();
            if (applied != (id == "settings_ok"))
            {
                Fail($"点“确定”的结果不对：生效 {applied}，提示 {window.LastError}");
                return true;
            }
            var extra = new Dictionary<string, object?>
            {
                ["applied"] = applied,
                ["uiSettings"] = UiSettingsInfo(),
                ["lang"] = _window.Lang.ToString(),
            };
            if (!applied)
            {
                extra["error"] = window.LastError ?? "";
                extra["errorPage"] = window.CurrentPage;
            }
            if (_lastApplied is { } info)
            {
                extra["changed"] = info.Changed;
                extra["saveError"] = info.SaveError ?? "";
            }
            if (applied && core.SettingsPath() is string path && File.Exists(path))
            {
                extra["settingsFile"] = path;
                extra["settingsText"] = File.ReadAllText(path);
            }
            Record(id, extra);
        }
        else if (id == "settings_reset")
        {
            window.Reset();
            Record(id, new() { ["dirty"] = core.SettingsDirty() });
        }
        else if (id == "settings_cancel")
        {
            var dirty = core.SettingsDirty();
            window.Discard();
            Record(id, new() { ["dirtyBeforeCancel"] = dirty });
        }
        else
        {
            Fail($"步骤写错了：{id}");
        }
        return true;
    }

    // ---- 选择图片窗口 ----

    /// <summary>
    /// 选择图片窗口的步骤；不是这类步骤时返回 false。`picker` 打开窗口（与按 G 相同），
    /// `picker_select:N` 选中第 N 行（与单击、方向键相同），`picker_sort:动作` 在排序菜单里选一项，
    /// `picker_hide:on|off` 勾选"隐藏缩略图导航"，`picker_ok` 点"确定"（等新图片显示），`picker_cancel` 关窗口（不切换）。
    /// 窗口开着时每一步等看得见的缩略图都生成好（最多 5 秒）再截图。
    /// </summary>
    private bool PickerStep(string id)
    {
        if (id == "picker")
        {
            _window.Invoke("select_image");
            if (_window.Picker is null)
            {
                Fail("选择图片窗口没有打开");
                return true;
            }
            WaitPicker(id, Now);
            return true;
        }
        if (!id.StartsWith("picker_", StringComparison.Ordinal))
        {
            return false;
        }
        if (_window.Picker is not { IsClosed: false } picker)
        {
            Fail($"{id}：选择图片窗口没有打开");
            return true;
        }
        if (id.StartsWith("picker_select:", StringComparison.Ordinal))
        {
            if (!int.TryParse(id["picker_select:".Length..], out var n) || n < 0 || n >= picker.RowCount)
            {
                Fail($"步骤写错了：{id}");
                return true;
            }
            picker.Select(n);
            WaitPicker(id, Now);
        }
        else if (id.StartsWith("picker_sort:", StringComparison.Ordinal))
        {
            if (!picker.ChooseSort(id["picker_sort:".Length..]))
            {
                Fail($"排序菜单里没有：{id}");
                return true;
            }
            WaitPicker(id, Now);
        }
        else if (id.StartsWith("picker_hide:", StringComparison.Ordinal))
        {
            picker.SetHideThumbnails(id.EndsWith(":on", StringComparison.Ordinal));
            WaitPicker(id, Now);
        }
        else if (id == "picker_ok")
        {
            _waitingImage = true;
            _pendingRecord = new() { ["step"] = id };
            WatchStep(_pendingRecord, id);
            picker.Choose();
        }
        else if (id == "picker_cancel")
        {
            picker.Window.Close();
            Record(id);
        }
        else
        {
            Fail($"步骤写错了：{id}");
        }
        return true;
    }

    /// <summary>等选择图片窗口里看得见的缩略图都生成好（最多 5 秒），再记录、截图。</summary>
    private void WaitPicker(string label, double started)
    {
        if (_finished)
        {
            return;
        }
        var done = true;
        if (_window.Picker is { IsClosed: false } picker && picker.StripVisible)
        {
            var (ready, visible) = picker.ThumbnailsLoaded;
            done = visible > 0 && ready == visible;
        }
        if (done || Now - started > 5000)
        {
            Record(label, new() { ["pickerWaitMs"] = Now - started });
            return;
        }
        _window.After(50, () => WaitPicker(label, started));
    }

    private static Dictionary<string, object?> PickerInfo(PickerWindow picker)
    {
        var (ready, visible) = picker.ThumbnailsLoaded;
        var info = new Dictionary<string, object?>
        {
            ["rows"] = picker.RowCount,
            ["sortLabel"] = picker.SortLabel,
            ["stripVisible"] = picker.StripVisible,
            ["hideSetting"] = picker.HidesThumbnailsSetting,
            ["thumbnailsReady"] = ready,
            ["thumbnailsVisible"] = visible,
        };
        if (picker.SelectedIndex is int selected)
        {
            info["selected"] = selected;
            info["selectedName"] = picker.Row(selected)?.Name ?? "";
        }
        if (picker.Row(0) is PickerRow first)
        {
            info["firstName"] = first.Name;
            info["firstSize"] = first.Size;
            info["firstModified"] = first.Modified;
        }
        return info;
    }

    // ---- 帮助窗口 ----

    /// <summary>帮助窗口开着：截图、记下标题和行数，然后关掉它。</summary>
    private async Task RecordHelp(string id, Dictionary<string, object?>? about = null)
    {
        if (_finished)
        {
            return;
        }
        if (_app.HelpWindows.LastOrDefault(h => !h.IsClosed) is not HelpWindow help)
        {
            Fail($"帮助窗口没有打开：{id}");
            return;
        }
        var step = _records.Count;
        var headings = help.Rows.Count(r => r.Heading);
        var first = help.Rows.FirstOrDefault(r => !r.Heading) is HelpRow row ? $"{row.Left} {row.Right}" : "";
        var shot = await help.SnapshotAsync(Path.Combine(_dir, $"step-{step:00}-help.png"));
        if (_finished)
        {
            return;
        }
        var info = new Dictionary<string, object?>
        {
            ["title"] = help.Window.Title,
            ["headings"] = headings,
            ["rows"] = help.Rows.Length - headings,
            ["first"] = first,
            // 开源许可：文字有多长、有没有 Anyview 自己的 MIT 许可证。
            ["textLength"] = help.Text?.Length ?? 0,
            ["anyviewLicense"] = help.Text?.Contains("Copyright (c) 2026 Anyview contributors", StringComparison.Ordinal) ?? false,
            ["screenshot"] = shot,
        };
        help.Window.Close();
        Record(id, new() { ["help"] = info, ["about"] = about });
    }

    private async Task RecordGeneration(string id)
    {
        if (_finished)
        {
            return;
        }
        var step = _records.Count;
        var panel = _app.Generation;
        var info = new Dictionary<string, object?>
        {
            ["open"] = panel is not null,
            ["enabled"] = _window.Core.ActionState("show_generation_info").Enabled,
        };
        if (panel is not null)
        {
            info["tool"] = panel.Shown?.Tool;
            info["prompt"] = panel.Shown?.Prompt;
            info["screenshot"] = await panel.SnapshotAsync(Path.Combine(_dir, $"step-{step:00}-generation.png"));
            if (_finished)
            {
                return;
            }
        }
        Record(id, new() { ["generation"] = info });
    }

    // ---- 多个窗口、吸附边缘 ----

    /// <summary>所有主窗口的样子（有效像素）。</summary>
    private List<Dictionary<string, object?>> WindowsInfo() => _app.Windows.Select(w =>
    {
        var scale = w.DpiScale;
        var position = w.AppWindow.Position;
        var size = w.AppWindow.Size;
        var content = w.ContentSize;
        return new Dictionary<string, object?>
        {
            ["name"] = w.CurrentName,
            ["closed"] = w.IsClosed,
            ["visible"] = w.IsVisible,
            ["active"] = ReferenceEquals(w, _app.ActiveWindow),
            ["content"] = new[] { Math.Round(content.Width, 1), Math.Round(content.Height, 1) },
            ["frame"] = new[] { Math.Round(position.X / scale, 1), Math.Round(position.Y / scale, 1), Math.Round(size.Width / scale, 1), Math.Round(size.Height / scale, 1) },
            ["titleBar"] = w.TitleBarShown,
            ["background"] = $"#{w.Settings.BackgroundColor:X6}",
        };
    }).ToList();

    /// <summary>开一个空窗口、0.15 秒后关掉，再等 0.1 秒做 then（单独一个方法：窗口的引用不留在调用的地方）。</summary>
    private void OpenBare(List<WeakReference<Window>> refs, bool derived, Action then)
    {
        Window window = derived ? new BareWindow() : new Window();
        refs.Add(new WeakReference<Window>(window));
        window.Activate();
        _window.After(150, () =>
        {
            window.Close();
            _window.After(100, then);
        });
    }

    /// <summary>垃圾回收以后进程自己占的内存（MB）。</summary>
    private static double PrivateMb()
    {
        for (var round = 0; round < 3; round++)
        {
            GC.Collect();
            GC.WaitForPendingFinalizers();
        }
        using var process = Process.GetCurrentProcess();
        return Math.Round(process.PrivateMemorySize64 / 1048576.0, 1);
    }

    /// <summary>多个窗口、吸附边缘的步骤；不是这类步骤时返回 false。</summary>
    private bool WindowStep(string id)
    {
        string? Arg(string prefix) => id.StartsWith(prefix, StringComparison.Ordinal) ? id[prefix.Length..] : null;
        if (id == "minimize_restore")
        {
            // 最小化时不阻止显示器睡眠、取回后再阻止：最小化，等状态变过来记一笔，再取回；取回后照常截图核对
            // （那时的 preventingSleep 由平常的记录写上）。窗口状态变化的通知晚一点才到，所以各等 0.3 秒再看。
            var extra = new Dictionary<string, object?> { ["preventingSleepBefore"] = _app.KeepingDisplayOn };
            var started = Now;
            // 这期间系统通知了哪些窗口变化（标志：P 呈现方式、S 大小、M 位置、V 可见、Z 层次，A/D 激活/失去激活；冒号后是当时的状态）。
            var changes = new List<string>();
            _window.ChangeLog = changes;
            void WaitRestored()
            {
                if (_finished)
                {
                    return;
                }
                if (_window.IsMinimized && Now - started < 2000)
                {
                    _window.After(30, WaitRestored);
                    return;
                }
                extra["restored"] = !_window.IsMinimized;
                _window.After(300, () =>
                {
                    _window.ChangeLog = null;
                    extra["changes"] = string.Join(" ", changes);
                    Record(id, extra);
                });
            }
            void WaitMinimized()
            {
                if (_finished)
                {
                    return;
                }
                if (!_window.IsMinimized && Now - started < 2000)
                {
                    _window.After(30, WaitMinimized);
                    return;
                }
                _window.After(300, () =>
                {
                    extra["minimized"] = _window.IsMinimized;
                    extra["preventingSleepMinimized"] = _app.KeepingDisplayOn;
                    changes.Add("|");
                    started = Now;
                    _window.RestoreFromMinimized();
                    WaitRestored();
                });
            }
            _window.Minimize();
            WaitMinimized();
            return true;
        }
        if (id == "menu_bar_describe")
        {
            // 菜单栏现在的内容：顶层的菜单、各个菜单里的动作（标题、快捷键、可用）、"打开最近使用"里的路径。
            if (_window.MenuBar is not { } described || !_window.MenuBarShown)
            {
                Fail($"{id}：菜单栏没有显示");
                return true;
            }
            var (menuItems, menuRecent) = described.Describe();
            Record(id, new()
            {
                ["titles"] = described.Titles,
                ["items"] = menuItems.Select(i => new Dictionary<string, object?>
                {
                    ["menu"] = i.Menu, ["id"] = i.Id, ["title"] = i.Title, ["shortcut"] = i.Shortcut, ["enabled"] = i.Enabled,
                }).ToList(),
                ["recentInMenu"] = menuRecent,
            });
            return true;
        }
        if (Arg("menu_bar:") is string menuId)
        {
            // 像用户那样用一次菜单栏：按下那个菜单的按钮、等菜单真的弹出来、再按里面的那一项。
            // 翻页的几项等新图片出来；别的（开关图片信息等）过一会儿记录、截图。
            if (_window.MenuBar is not { } used || !_window.MenuBarShown)
            {
                Fail($"{id}：菜单栏没有显示");
                return true;
            }
            var menuRecord = new Dictionary<string, object?> { ["step"] = id, ["viaMenuBar"] = menuId };
            var changesImage = menuId is "next_image" or "prev_image" or "first_image" or "last_image";
            if (changesImage)
            {
                _waitingImage = true;
                _pendingRecord = menuRecord;
                WatchStep(menuRecord, id);
            }
            used.InvokeForTest(menuId, problem =>
            {
                if (problem is not null)
                {
                    Fail($"{id}：{problem}");
                }
                else if (!changesImage)
                {
                    _window.After(200, () =>
                    {
                        if (!_finished)
                        {
                            _records.Add(menuRecord);
                            CaptureLater();
                        }
                    });
                }
            });
            return true;
        }
        if (Arg("menu_recent:") is string recentText && int.TryParse(recentText, out var recentAt))
        {
            // "打开最近使用"里的第几个（从 0 起，最近的在最前）：等它显示出来。
            if (_window.MenuBar is not { } recents)
            {
                Fail($"{id}：菜单栏没有显示");
                return true;
            }
            _waitingImage = true;
            _pendingRecord = new() { ["step"] = id };
            WatchStep(_pendingRecord, id);
            if (!recents.OpenRecentForTest(recentAt))
            {
                Fail($"{id}：最近打开的里没有第 {recentAt} 个");
            }
            return true;
        }
        if (Arg("churn_bare:") is string bare)
        {
            // 诊断用（churn_bare:次数[:derived]）：开关几个什么都不放的窗口——WinUI 自己的 Window，或只是派生了一下的空窗口
            // （本程序的主窗口是派生的）——看关掉以后框架自己放不放它们。第一次不算。
            var bareParts = bare.Split(':');
            if (!int.TryParse(bareParts[0], out var bareRounds) || bareRounds < 1)
            {
                Fail($"步骤写错了：{id}");
                return true;
            }
            var derived = bareParts.Length > 1 && bareParts[1] == "derived";
            var bareRefs = new List<WeakReference<Window>>();
            var bareDone = -1;
            var bareBeforeMb = 0.0;
            void NextBare()
            {
                if (_finished)
                {
                    return;
                }
                bareDone++;
                if (bareDone == 1)
                {
                    bareBeforeMb = PrivateMb();
                }
                if (bareDone == bareRounds + 1)
                {
                    _window.After(500, () =>
                    {
                        var afterMb = PrivateMb();
                        Record(id, new()
                        {
                            ["rounds"] = bareRounds,
                            ["derived"] = derived,
                            ["leakedMb"] = Math.Round(afterMb - bareBeforeMb, 1),
                            ["closedWindows"] = bareRefs.Count,
                            ["aliveWindows"] = bareRefs.Count(r => r.TryGetTarget(out _)),
                        });
                    });
                    return;
                }
                OpenBare(bareRefs, derived, NextBare);
            }
            NextBare();
            return true;
        }
        if (Arg("churn:") is string churn)
        {
            // churn:次数:图片 —— 开一个新窗口显示这张图、显示出来就关掉，重复多次，看关窗口时该放的东西有没有放：
            // 关掉的窗口的画布控件回收了没有、还留没留着上传用的数组，这个进程占的内存、显存多了多少。
            // 第一次不算（头一回开窗口要编译代码、建一次性的东西），从它关掉以后量起。
            var churnParts = churn.Split(':', 2);
            if (churnParts.Length != 2 || !int.TryParse(churnParts[0], out var rounds) || rounds < 1)
            {
                Fail($"步骤写错了：{id}");
                return true;
            }
            var churnFile = churnParts[1];
            var baseline = _app.Windows.Count;
            var done = -1;
            var beforeMb = 0.0;
            var cacheBeforeMb = 0.0;
            void NextRound()
            {
                if (_finished)
                {
                    return;
                }
                done++;
                if (done == 1)
                {
                    beforeMb = PrivateMb();
                    cacheBeforeMb = Math.Round(_window.Core.CacheBytes() / 1048576.0, 1);
                }
                if (done == rounds + 1)
                {
                    // 等关窗口的收尾做完（放掉核心是排在后面做的；最后关的那个窗口的画布控件，界面框架还要过一会儿才松手）
                    // 再量：过 0.5 秒看一次，画布控件还没回收完就每 0.3 秒再看，最多 3 秒。剩 1 个不再等：显示过带透明的图片
                    // 以后总有 1 个留着，不随窗口数增加（见 test.ps1 里这一项的说明）。
                    var measureStarted = Now;
                    void Measure()
                    {
                        if (_finished)
                        {
                            return;
                        }
                        var afterMb = PrivateMb();
                        var alive = _app.ClosedWindows();
                        if (alive.Controls > 1 && CloseCleanup.Does("canvas-control") && Now - measureStarted < 3000)
                        {
                            _window.After(300, Measure);
                            return;
                        }
                        Record(id, new()
                        {
                            ["rounds"] = rounds,
                            ["privateMbBefore"] = beforeMb,
                            ["privateMbAfter"] = afterMb,
                            ["leakedMb"] = Math.Round(afterMb - beforeMb, 1),
                            ["cacheMbBefore"] = cacheBeforeMb,
                            ["cacheMbAfter"] = Math.Round(_window.Core.CacheBytes() / 1048576.0, 1),
                            ["closedWindows"] = alive.Closed,
                            ["aliveWindows"] = alive.Windows,
                            ["aliveControls"] = alive.Controls,
                            ["heldBuffers"] = alive.HeldBuffers,
                            ["skippedCleanup"] = CloseCleanup.Skipped,
                        });
                    }
                    _window.After(500, Measure);
                    return;
                }
                if (_app.FinderOpen(churnFile) is not var (opened, _) || ReferenceEquals(opened, _window))
                {
                    Fail($"{id}：第 {done + 1} 次没有开出新窗口");
                    return;
                }
                var openedAt = Now;
                void WaitClosed()
                {
                    if (_finished)
                    {
                        return;
                    }
                    if (_app.Windows.Count == baseline && _window.Core.WindowCount() == baseline)
                    {
                        NextRound();
                        return;
                    }
                    if (Now - openedAt > 8000)
                    {
                        Fail($"{id}：第 {done + 1} 个窗口没有关掉");
                        return;
                    }
                    _window.After(30, WaitClosed);
                }
                void WaitShown()
                {
                    if (_finished)
                    {
                        return;
                    }
                    if (opened.Canvas.HasImage && opened.IsVisible && !opened.Canvas.ShowingPreview)
                    {
                        opened.Invoke("close_window");
                        WaitClosed();
                        return;
                    }
                    if (Now - openedAt > 5000)
                    {
                        Fail($"{id}：第 {done + 1} 个窗口 5 秒内没有显示图片");
                        return;
                    }
                    _window.After(30, WaitShown);
                }
                WaitShown();
            }
            NextRound();
            return true;
        }
        if (Arg("finder_open:") is string path)
        {
            if (_app.FinderOpen(path) is not var (target, how))
            {
                Fail("没法模拟从资源管理器打开");
                return true;
            }
            if (ReferenceEquals(target, _window))
            {
                // 在自检看着的这个窗口里换图：与平常切图一样等图像就绪再截图核对。
                _waitingImage = true;
                _pendingRecord = new() { ["step"] = id, ["how"] = how };
                WatchStep(_pendingRecord, id);
                return true;
            }
            WaitForWindow(target, Path.GetFileName(path), id, how, Now);
            return true;
        }
        if (Arg("key_in:") is string keyIn)
        {
            var parts = keyIn.Split(':', 2);
            var windows = _app.Windows;
            if (parts.Length != 2 || !int.TryParse(parts[0], out var index) || index < 0 || index >= windows.Count
                || ParseKey(parts[1]) is not var (key, mods))
            {
                Fail($"步骤写错了，或没有这个窗口：{id}");
                return true;
            }
            var target = windows[index];
            var action = target.Core.ActionForKey(key, mods);
            var record = new Dictionary<string, object?> { ["step"] = id, ["key"] = parts[1], ["action"] = action, ["window"] = index };
            // 这是最后一个窗口时 Esc 会退出：记录由 QuitRequested 写上。
            _pendingRecord = record;
            if (action is not null)
            {
                target.Invoke(action);
            }
            if (_finished)
            {
                return true;
            }
            _pendingRecord = null;
            // 关掉的窗口的核心放掉以后，核心里的窗口数才变（最多等 2 秒）。
            var started = Now;
            void Check()
            {
                if (_finished)
                {
                    return;
                }
                if (_window.Core.WindowCount() == _app.Windows.Count || Now - started > 2000)
                {
                    _records.Add(record);
                    CaptureLater();
                    return;
                }
                _window.After(50, Check);
            }
            Check();
            return true;
        }
        if (id == "move_bench")
        {
            BenchMoves(id);
            return true;
        }
        if (id == "drag_real")
        {
            _ = RunRealDrags(id);
            return true;
        }
        if (Arg("move_window:") is string move)
        {
            MoveStep(id, move, dragging: false);
            return true;
        }
        if (Arg("drag_window:") is string drag)
        {
            MoveStep(id, drag, dragging: true);
            return true;
        }
        return false;
    }

    /// <summary>
    /// move_window:距离:snap|stay、drag_window:距离:snap：把看得见的窗口（去掉看不见的拖动边框）放到离可用区域左上角
    /// "距离"有效像素的地方。move 模拟拖标题栏（系统移动，松开后吸附），drag 模拟在画面上拖动（程序自己移动、边拖边吸附）。
    /// 核对结果：snap 应贴到左上角，stay 应不动。
    /// </summary>
    private void MoveStep(string id, string spec, bool dragging)
    {
        var parts = spec.Split(':');
        if (parts.Length != 2 || !double.TryParse(parts[0], NumberStyles.Float, CultureInfo.InvariantCulture, out var distance)
            || parts[1] is not ("snap" or "stay") || !_window.WindowMovable)
        {
            Fail($"步骤写错了，或窗口现在不能移动：{id}");
            return;
        }
        var scale = _window.DpiScale;
        var area = Win32.WorkAreaOfWindow(_window.Hwnd);
        var (left, top, _, _) = Win32.InvisibleBorders(_window.Hwnd);
        var target = new Windows.Graphics.PointInt32(
            area.X + (int)Math.Round(distance * scale) - left,
            area.Y + (int)Math.Round(distance * scale) - top);
        if (dragging)
        {
            var position = _window.AppWindow.Position;
            if (!_window.DragWindowForTest(target.X - position.X, target.Y - position.Y))
            {
                Fail($"{id}：吸附边缘关着，或现在拖动不是移动窗口");
                return;
            }
        }
        else
        {
            _window.AppWindow.Move(target);
            _window.FinishUserMove();
        }
        var now = _window.AppWindow.Position;
        var offsetLeft = Math.Round((now.X + left - area.X) / scale, 2);
        var offsetTop = Math.Round((now.Y + top - area.Y) / scale, 2);
        Record(id, new() { ["left"] = offsetLeft, ["top"] = offsetTop });
        var snapped = Math.Abs(offsetLeft) < 0.5 && Math.Abs(offsetTop) < 0.5;
        var stayed = Math.Abs(offsetLeft - distance) < 0.6 && Math.Abs(offsetTop - distance) < 0.6;
        if (parts[1] == "snap" ? !snapped : !stayed)
        {
            _problems.Add($"{id}: 窗口离可用区域左边 {offsetLeft}、上边 {offsetTop}，应{(parts[1] == "snap" ? "贴边" : "不动")}");
        }
    }

    /// <summary>
    /// move_bench：自动移动窗口 60 次（约每 16 ms 一次，左右来回 8 有效像素），量每一步 AppWindow.Move 用了多久、
    /// 计时器晚到多少，以及这段时间里窗口变化、XAML 根变化、画布改大小、UI 自动化来问、XAML 画帧、画布重画各几次。
    /// 先用现在的办法（Win32 去掉标题栏）量一遍，再换成 WinUI 自己的办法（ExtendsContentIntoTitleBar）量一遍，最后换回来。
    /// 测量窗口连续移动耗时以定位拖动卡顿来源。
    /// </summary>
    private void BenchMoves(string id)
    {
        var results = new Dictionary<string, object?>();
        void Pass(string name, Action next)
        {
            var durations = new List<double>();
            var late = new List<double>();
            var before = (Diag.WindowChanges, Diag.RootChanges, Diag.CanvasResizes, Diag.AutomationQueries);
            var drawsBefore = _window.Canvas.DrawCount;
            var renderedFrames = 0;
            EventHandler<RenderedEventArgs> countFrame = (_, _) => renderedFrames++;
            CompositionTarget.Rendered += countFrame;
            var origin = _window.AppWindow.Position;
            var step = (int)Math.Round(8 * _window.DpiScale);
            var count = 0;
            var expected = Now + 16;
            void Tick()
            {
                if (_finished)
                {
                    CompositionTarget.Rendered -= countFrame;
                    return;
                }
                late.Add(Math.Max(Now - expected, 0));
                var watch = Stopwatch.StartNew();
                _window.AppWindow.Move(new Windows.Graphics.PointInt32(origin.X + (count % 2 == 0 ? step : 0), origin.Y));
                durations.Add(watch.Elapsed.TotalMilliseconds);
                count++;
                if (count < 60)
                {
                    expected = Now + 16;
                    _window.After(16, Tick);
                    return;
                }
                CompositionTarget.Rendered -= countFrame;
                _window.AppWindow.Move(origin);
                durations.Sort();
                late.Sort();
                results[name] = new Dictionary<string, object?>
                {
                    ["moveMsMedian"] = Math.Round(durations[durations.Count / 2], 2),
                    ["moveMs95"] = Math.Round(durations[(int)(durations.Count * 0.95)], 2),
                    ["moveMsMax"] = Math.Round(durations[^1], 2),
                    ["lateMsMedian"] = Math.Round(late[late.Count / 2], 1),
                    ["lateMsMax"] = Math.Round(late[^1], 1),
                    ["windowChanges"] = Diag.WindowChanges - before.WindowChanges,
                    ["rootChanges"] = Diag.RootChanges - before.RootChanges,
                    ["canvasResizes"] = Diag.CanvasResizes - before.CanvasResizes,
                    ["automationQueries"] = Diag.AutomationQueries - before.AutomationQueries,
                    ["frames"] = renderedFrames,
                    ["draws"] = _window.Canvas.DrawCount - drawsBefore,
                };
                next();
            }
            _window.After(16, Tick);
        }
        Pass("win32", () =>
        {
            _window.UseWinUiTitleBarForTest(true);
            _window.After(400, () => Pass("winui", () =>
            {
                _window.UseWinUiTitleBarForTest(false);
                _window.After(400, () => Record(id, new() { ["moveBench"] = results }));
            }));
        });
    }

    /// <summary>drag_real（SelfTestDrag.cs）：出错时让这一项失败，不让异常漏到界面线程上。</summary>
    private async Task RunRealDrags(string id)
    {
        try
        {
            await RealDrags(id);
        }
        catch (Exception error)
        {
            Fail($"{id} 出错：{error}");
        }
    }

    /// <summary>等另一个窗口（从资源管理器打开时开的新窗口）出图：每 50 ms 看一次，最多 5 秒。</summary>
    private void WaitForWindow(MainWindow target, string name, string label, string how, double started)
    {
        if (_finished)
        {
            return;
        }
        if (target.CurrentName == name && target.Canvas.HasImage && target.IsVisible)
        {
            var mine = _window.AppWindow;
            var theirs = target.AppWindow;
            var scale = _window.DpiScale;
            var extra = new Dictionary<string, object?>
            {
                ["how"] = how,
                ["openedIn"] = name,
                ["msToImage"] = Now - started,
                // 新窗口的中心在原来窗口中心的右下方，大小按它的图片。
                ["centerOffset"] = new[]
                {
                    Math.Round((theirs.Position.X + theirs.Size.Width / 2.0 - mine.Position.X - mine.Size.Width / 2.0) / scale, 1),
                    Math.Round((theirs.Position.Y + theirs.Size.Height / 2.0 - mine.Position.Y - mine.Size.Height / 2.0) / scale, 1),
                },
            };
            Record(label, extra);
            if (target.LastFitSize is Size fit && target.WindowMovable
                && (Math.Abs(target.ContentSize.Width - fit.Width) > 1 || Math.Abs(target.ContentSize.Height - fit.Height) > 1))
            {
                _problems.Add($"{label}: 新窗口的图片区 {target.ContentSize.Width:0.#}x{target.ContentSize.Height:0.#}，应为 {fit.Width:0.#}x{fit.Height:0.#}");
            }
            return;
        }
        if (Now - started > 5000)
        {
            Fail($"新窗口 5 秒内没有显示 {name}");
            return;
        }
        _window.After(50, () => WaitForWindow(target, name, label, how, started));
    }

    // ---- 文件夹变化 ----

    /// <summary>
    /// add_file:源文件、remove_file:文件名：在当前图片的文件夹里加一个文件、删一个文件，等浏览集合的张数变了
    /// （文件夹变化自动刷新，最多 6 秒）。不是这类步骤时返回 false。
    /// </summary>
    private bool FileStep(string id)
    {
        var add = id.StartsWith("add_file:", StringComparison.Ordinal);
        if (!add && !id.StartsWith("remove_file:", StringComparison.Ordinal))
        {
            return false;
        }
        if (_window.Core.CurrentPath() is not string current || Path.GetDirectoryName(current) is not string folder)
        {
            Fail("没有当前文件夹");
            return true;
        }
        try
        {
            if (add)
            {
                var source = id["add_file:".Length..];
                File.Copy(source, Path.Combine(folder, Path.GetFileName(source)));
            }
            else
            {
                File.Delete(Path.Combine(folder, id["remove_file:".Length..]));
            }
        }
        catch (Exception error)
        {
            Fail($"{id} 失败：{error.Message}");
            return true;
        }
        var before = _window.CurrentTotal;
        var started = Now;
        void Check()
        {
            if (_finished)
            {
                return;
            }
            var total = _window.CurrentTotal;
            if (total != before || Now - started > 6000)
            {
                Record(id, new() { ["ms"] = Now - started });
                if (total == before)
                {
                    _problems.Add($"{id}: 6 秒内浏览集合没有变");
                }
                return;
            }
            _window.After(100, Check);
        }
        Check();
        return true;
    }

    /// <summary>"Ctrl+Shift+Up"、"Period"、"7"、"Ctrl++" → 核心的键名和修饰键。</summary>
    private static (string, KeyModifiers)? ParseKey(string spec)
    {
        bool shift = false, ctrl = false, alt = false, win = false;
        var rest = spec;
        while (true)
        {
            var plus = rest.IndexOf('+');
            if (plus <= 0 || plus == rest.Length - 1)
            {
                break;
            }
            switch (rest[..plus].ToLowerInvariant())
            {
                case "shift": shift = true; break;
                case "ctrl": case "control": ctrl = true; break;
                case "alt": alt = true; break;
                case "win": case "cmd": win = true; break;
                default: return null;
            }
            rest = rest[(plus + 1)..];
        }
        var key = rest switch
        {
            "Period" => ".",
            "Comma" => ",",
            "Plus" => "+",
            "Minus" => "-",
            "Slash" => "/",
            _ => rest.Length == 1 ? rest.ToLowerInvariant() : rest,
        };
        return (key, new KeyModifiers(shift, ctrl, alt, win));
    }

    private void MouseStep(string id, string spec)
    {
        var parts = spec.Split(':');
        var w = _window.Content is FrameworkElement root ? root.ActualWidth : 0;
        var h = _window.Content is FrameworkElement r2 ? r2.ActualHeight : 0;
        var input = parts[0] switch
        {
            "wheel_down" => MouseInput.WheelDown,
            "wheel_up" => MouseInput.WheelUp,
            "right" => MouseInput.Right,
            "middle" => MouseInput.Middle,
            "back" => MouseInput.Back,
            "forward" => MouseInput.Forward,
            _ => (MouseInput?)null,
        };
        string? action;
        if (input is MouseInput i)
        {
            action = _window.Core.MouseAction(i) switch
            {
                MouseResult.Action a => a.Id,
                MouseResult.Nothing => null,
                _ => i switch
                {
                    MouseInput.Back or MouseInput.WheelUp => "prev_image",
                    MouseInput.Forward or MouseInput.WheelDown => "next_image",
                    MouseInput.Right => "context_menu",
                    _ => null,
                },
            };
            if (action == "context_menu")
            {
                var before = _menuShown;
                _window.RunMouse(i, new Point(w / 2, h / 2));
                // 等菜单弹出、关掉。
                _window.After(500, () =>
                {
                    Record(id, new() { ["action"] = "context_menu", ["menuShown"] = _menuShown - before });
                    if (_menuShown - before != 1)
                    {
                        _problems.Add($"{id}: 右键菜单弹出了 {_menuShown - before} 次");
                    }
                });
                return;
            }
        }
        else if (parts.Length == 2 && double.TryParse(parts[1], NumberStyles.Float, CultureInfo.InvariantCulture, out var x))
        {
            action = parts[0] switch
            {
                "left" => _window.Core.LeftClickAction(x),
                "double" => _window.Core.LeftDoubleClickAction(x),
                _ => null,
            };
        }
        else
        {
            Fail($"步骤写错了：{id}");
            return;
        }
        if (action is null)
        {
            Record(id, new() { ["action"] = null });
            return;
        }
        RunAction(action, id, new() { ["action"] = action });
    }

    /// <summary>执行一个动作作为这一步，按结果等待图像或记录、截图。</summary>
    private void RunAction(string id, string label, Dictionary<string, object?>? extra)
    {
        var outcome = _window.Core.InvokeAction(id);
        var record = new Dictionary<string, object?> { ["step"] = label };
        if (extra is not null)
        {
            foreach (var (k, v) in extra)
            {
                record[k] = v;
            }
        }
        switch (outcome)
        {
            case ActionOutcome.Started when id == "close_image":
                _records.Add(record);
                CaptureLater();
                break;
            case ActionOutcome.Started:
                _waitingImage = true;
                _pendingRecord = record;
                WatchStep(record, label);
                break;
            case ActionOutcome.ViewChanged or ActionOutcome.OverlayChanged:
                _window.Apply(outcome);
                _window.FollowAfterAction(id);
                _records.Add(record);
                CaptureLater();
                break;
            case ActionOutcome.Ui ui:
                switch (ui.Request)
                {
                    case UiRequest.TrashFile trash:
                        record["deleted"] = trash.Path;
                        _waitingImage = true;
                        _pendingRecord = record;
                        WatchStep(record, label);
                        _window.Apply(outcome);
                        break;
                    case UiRequest.CloseOrQuit or UiRequest.Quit:
                        _pendingRecord = record;
                        _records.Add(record);
                        _window.Apply(outcome);
                        break;
                    case UiRequest.CopyImage copy:
                        // 等复制做完（在后台编码 PNG），核对放进剪贴板的内容（自检时不动真的剪贴板）。
                        _requests.Add(copy.GetType().Name);
                        _ = CopyStep(copy.Path, record, label);
                        break;
                    case UiRequest.OpenInEditor or UiRequest.SetWallpaper or UiRequest.Print:
                    {
                        // 编辑器、桌面图片、打印：自检时只记下交给外部的内容。
                        var before = _externals.Count;
                        _window.Apply(outcome);
                        record["request"] = ui.Request.GetType().Name;
                        record["external"] = _externals.Count > before ? _externals[^1] : null;
                        _records.Add(record);
                        CaptureLater();
                        break;
                    }
                    default:
                        _window.Apply(outcome);
                        record["request"] = ui.Request.GetType().Name;
                        _records.Add(record);
                        CaptureLater();
                        break;
                }
                break;
            default:
                Fail($"动作 {id} 没有执行：{outcome}");
                break;
        }
    }

    private async Task CopyStep(string path, Dictionary<string, object?> record, string label)
    {
        var formats = await _window.CopyImage(path);
        record["clipboard"] = formats;
        _records.Add(record);
        var hasImage = formats?.Contains("Bitmap") == true;
        var hasFile = formats?.Contains("Shell IDList Array") == true || formats?.Contains("StorageItems") == true;
        if (!hasImage || !hasFile)
        {
            _problems.Add($"{label}: 剪贴板内容不全（图像 {hasImage}，文件 {hasFile}）");
        }
        CaptureLater();
    }

    // ---- 结束 ----

    private void Finish()
    {
        if (_finished)
        {
            return;
        }
        _finished = true;
        // 真拖动（drag_real）进行到一半时超时退出：先松开自检按下的左键，不然系统一直当左键按着；之后不再按下。
        Inject.ReleaseIfDown(closing: true);
        var report = new Dictionary<string, object?>();
        try
        {
            // 右键菜单的实际内容（标题、快捷键、可用）。
            var menu = MenuBuilder.Flatten(_window.ContextMenu())
                .Select(m => new Dictionary<string, object?> { ["id"] = m.Id, ["title"] = m.Title, ["shortcut"] = m.Shortcut, ["enabled"] = m.Enabled })
                .ToList();
            report["contextMenu"] = menu;
            if (menu.Count == 0)
            {
                _problems.Add("右键菜单是空的");
            }
        }
        catch (Exception error)
        {
            _problems.Add($"右键菜单出错：{error.Message}");
        }
        var process = Process.GetCurrentProcess();
        process.Refresh();
        // 每一步都要有记录（打开算一条）：中途退出、卡住都算不通过。
        if (_failure is null && _records.Count != _options.Steps.Length + 1)
        {
            _failure = $"只做完了 {Math.Max(_records.Count - 1, 0)} / {_options.Steps.Length} 步";
        }
        if (_failure is null && _problems.Count > 0)
        {
            _failure = $"有 {_problems.Count} 个问题";
        }
        report["result"] = _failure ?? "ok";
        report["problems"] = _problems;
        report["steps"] = _records;
        report["externals"] = _externals;
        report["uiRequests"] = _requests;
        report["sounds"] = _window.SoundLog;
        report["marks"] = LaunchClock.Marks();
        report["workingSetMb"] = Math.Round(process.WorkingSet64 / 1048576.0, 1);
        report["privateMb"] = Math.Round(process.PrivateMemorySize64 / 1048576.0, 1);
        report["peakWorkingSetMb"] = Math.Round(process.PeakWorkingSet64 / 1048576.0, 1);
        report["coreVersion"] = AnyviewCore.CoreVersion();
        report["os"] = Environment.OSVersion.VersionString;
        report["dpiScale"] = _window.DpiScale;
        // 显卡（第 0 块是 Win2D 的共享设备用的那块）：冷启动时建设备的时间与它有关。
        report["adapters"] = Gpu.Adapters();
        // 显示器的配置文件：系统"颜色管理"里给窗口所在显示器设的那个，报给核心的有多少字节。
        report["displayProfile"] = new Dictionary<string, object?>
        {
            ["path"] = _window.DisplayProfilePath,
            ["bytes"] = _window.DisplayProfileBytes,
            ["system"] = _window.SystemProfilePath,
        };
        // 关掉过的窗口（finder_open 开的、Esc 关的）回收了没有（只记录）。
        try
        {
            var alive = _app.ClosedWindows();
            report["closedWindows"] = new Dictionary<string, object?>
            {
                ["closed"] = alive.Closed,
                ["alive"] = alive.Windows,
                ["aliveControls"] = alive.Controls,
                ["heldBuffers"] = alive.HeldBuffers,
            };
        }
        catch (Exception error)
        {
            report["closedWindows"] = error.Message;
        }
        try
        {
            File.WriteAllText(Path.Combine(_dir, "selftest.json"), ReportJson.Write(report), new UTF8Encoding(false));
        }
        catch (Exception error)
        {
            Crash.Write("SelfTest.Finish", error);
        }
        var code = _failure is null ? 0 : 1;
        try
        {
            _window.Core.Shutdown();
        }
        catch (Exception)
        {
        }
        Environment.Exit(code);
    }
}

/// <summary>诊断用的空窗口（只是从 Window 派生了一下）：见自检的 churn_bare 步骤。</summary>
internal sealed class BareWindow : Window
{
}

/// <summary>
/// 把自检报告写成 JSON。报告是字典、列表套着字符串、数、真假值；自己写出来，不用按类型自动序列化的库
/// （那种靠运行时反射，程序预先编译成本机代码以后用不了）。枚举写成数（和以前一样）；
/// 别的类型的对象写不了，直接报错——宁可自检失败，也不悄悄写出一份缺东西的报告。
/// </summary>
internal static class ReportJson
{
    public static string Write(object? value)
    {
        var text = new StringBuilder();
        Write(text, value, 0);
        text.Append('\n');
        return text.ToString();
    }

    private static void Write(StringBuilder text, object? value, int depth)
    {
        var culture = System.Globalization.CultureInfo.InvariantCulture;
        switch (value)
        {
            case null:
                text.Append("null");
                break;
            case string s:
                WriteString(text, s);
                break;
            case char c:
                WriteString(text, c.ToString());
                break;
            case bool b:
                text.Append(b ? "true" : "false");
                break;
            case double d:
                text.Append(double.IsFinite(d) ? d.ToString("R", culture) : "null");
                break;
            case float f:
                text.Append(float.IsFinite(f) ? f.ToString("R", culture) : "null");
                break;
            case decimal m:
                text.Append(m.ToString(culture));
                break;
            case sbyte or byte or short or ushort or int or uint or long or ulong or nint or nuint:
                text.Append(((IFormattable)value).ToString(null, culture));
                break;
            case Enum e:
                text.Append(Convert.ToInt64(e, culture).ToString(culture));
                break;
            case System.Collections.IDictionary map:
                WriteItems(text, depth, '{', '}', map.Count, each =>
                {
                    foreach (System.Collections.DictionaryEntry entry in map)
                    {
                        each(entry.Key.ToString() ?? "", entry.Value);
                    }
                });
                break;
            case System.Collections.IEnumerable list:
                var items = new List<object?>();
                foreach (var item in list)
                {
                    items.Add(item);
                }
                WriteItems(text, depth, '[', ']', items.Count, each =>
                {
                    foreach (var item in items)
                    {
                        each(null, item);
                    }
                });
                break;
            default:
                throw new NotSupportedException($"自检报告里有写不了的类型：{value.GetType().FullName}");
        }
    }

    /// <summary>一个对象或数组：每项一行，缩进两个空格。</summary>
    private static void WriteItems(StringBuilder text, int depth, char open, char close, int count, Action<Action<string?, object?>> enumerate)
    {
        text.Append(open);
        if (count == 0)
        {
            text.Append(close);
            return;
        }
        var first = true;
        enumerate((name, item) =>
        {
            text.Append(first ? "\n" : ",\n").Append(' ', (depth + 1) * 2);
            first = false;
            if (name is not null)
            {
                WriteString(text, name);
                text.Append(": ");
            }
            Write(text, item, depth + 1);
        });
        text.Append('\n').Append(' ', depth * 2).Append(close);
    }

    private static void WriteString(StringBuilder text, string value)
    {
        text.Append('"');
        foreach (var c in value)
        {
            switch (c)
            {
                case '"': text.Append("\\\""); break;
                case '\\': text.Append("\\\\"); break;
                case '\n': text.Append("\\n"); break;
                case '\r': text.Append("\\r"); break;
                case '\t': text.Append("\\t"); break;
                default:
                    if (c < ' ')
                    {
                        text.Append("\\u").Append(((int)c).ToString("x4", System.Globalization.CultureInfo.InvariantCulture));
                    }
                    else
                    {
                        text.Append(c);
                    }
                    break;
            }
        }
        text.Append('"');
    }
}
