// 应用：建主窗口，处理动作中需要应用层完成的部分（核心返回的 UiRequest：打开对话框、退出、全屏、文件操作、设置、帮助等）。
// 一个程序多个窗口：从资源管理器再打开图片时（别的进程交过来，见 Instance），有空着的窗口就用它；
// 勾了"不允许多个实例"就在前面的窗口里换图；否则开新窗口。各窗口共用设置、图片缓存和后台线程。
// 设置窗口整个程序一个，帮助窗口每种一个，选择图片窗口每个主窗口一个。关掉最后一个主窗口时退出程序（Windows 的习惯）。
using System;
using System.Collections.Generic;
using System.Linq;
using Anyview.Core;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Markup;
using Microsoft.UI.Xaml.XamlTypeInfo;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace Anyview;

/// <remarks>
/// 应用同时提供 XAML 的类型信息（IXamlMetadataProvider，交给 WinUI 控件库自带的 XamlControlsXamlMetaDataProvider）。
/// 平常由 XAML 编译器按 App.xaml 生成；这个程序只用代码、没有 XAML 文件，要自己提供。没有它时 WinUI 找不到控件库里的类型：
/// 文本框的默认样式里有一个控件库的 TextCommandBarFlyout（右键菜单），文本框一放进窗口就出"未指定的错误"
/// （Windows 上设置窗口里有输入框的几页都这样；WinUI 的源代码 MetadataAPI::TryGetMetadataProvider
/// 只问应用对象和 Microsoft.UI.Xaml.Phone.dll）。WinUI 的控件样式（XamlControlsResources）照旧不加，控件用自带的样式。
/// </remarks>
public sealed partial class App : Application, IXamlMetadataProvider
{
    /// <summary>开着的主窗口（按打开的先后）。</summary>
    private readonly List<MainWindow> _windows = new();
    /// <summary>最近在前面的主窗口：从资源管理器打开、设置窗口的提示交给它。</summary>
    private MainWindow? _lastActive;
    private SettingsWindow? _settings;
    private readonly List<HelpWindow> _help = new();
    /// <summary>“生成信息”面板：一个程序一个，跟着打开它的那个看图窗口。</summary>
    private GenerationWindow? _generation;
    private SelfTest? _selfTest;
    private DispatcherQueue? _queue;
    private bool _keepingDisplayOn;
    private bool _quitting;
    private bool _exiting;
    /// <summary>界面语言（设置里选的，或跟随系统）。</summary>
    private Lang _lang = Lang.En;

    /// <summary>控件库的类型信息：第一次用到时再建（冷启动不等它）。</summary>
    private XamlControlsXamlMetaDataProvider? _controlTypes;
    private XamlControlsXamlMetaDataProvider ControlTypes => _controlTypes ??= new XamlControlsXamlMetaDataProvider();

    public IXamlType GetXamlType(Type type) => ControlTypes.GetXamlType(type);

    public IXamlType GetXamlType(string fullName) => ControlTypes.GetXamlType(fullName);

    public XmlnsDefinition[] GetXmlnsDefinitions() => ControlTypes.GetXmlnsDefinitions();

    public App()
    {
        // 深色外观（与 macOS 的 darkAqua 相同）：控制栏的文字、按钮的悬停和不可用的样子、右键菜单、对话框都按深色。
        RequestedTheme = ApplicationTheme.Dark;
        UnhandledException += (_, e) =>
        {
            // e.Message 是 WinUI 自己的说明（常比异常本身具体，如找不到哪个资源、哪个类型）。
            Crash.Write("Application.UnhandledException", e.Exception, e.Message);
            // 平常接住、继续运行（已写进临时目录的 anyview-crash.txt）；自检时记进报告后结束。
            e.Handled = true;
            _selfTest?.Crashed(e.Exception);
        };
    }

    /// <summary>界面线程的系统线程号（自检：静止时各线程的 CPU 占用里标出它）。</summary>
    public static uint UiThreadId { get; private set; }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        LaunchClock.Mark("launched");
        UiThreadId = Win32.GetCurrentThreadId();
        _queue = DispatcherQueue.GetForCurrentThread();
        var bridge = Program.LaunchCore ?? new CoreBridge(null);
        _lang = bridge.Core.UiLang(Program.SystemLang);
        var window = MakeWindow(bridge);
        var options = Program.Options;
        // 直接启动（没有图片）时固定 900x600 有效像素、在主屏幕上居中；带着图片启动时按上次的位置，
        // 大小随后跟着图片。自检时每次都从 900x600 开始。
        if (options.SelfTestDir is null && options.OpenPath is not null)
        {
            window.RestorePlacement();
        }
        else
        {
            window.PlaceAtLaunchDefault();
        }
        LaunchClock.Mark("windowCreated");
        if (options.SelfTestDir is not null)
        {
            window.IsSelfTest = true;
            _selfTest = new SelfTest(options, window, bridge, this);
            window.ShowNow();
            // 先让自检开始等第一张图，再交出启动时存着的事件：图片可能在建窗口时就已经解码好了。
            _selfTest.Start();
            ShowSettingsNotice(window);
            bridge.StartDelivering();
            window.DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, window.FinishStartup);
            return;
        }
        if (options.OpenPath is not null)
        {
            // 窗口一出现就是图片的大小：等第一张图（最多 0.3 秒）再显示。
            window.ShowAfterFirstImage();
        }
        else
        {
            window.ShowNow();
        }
        // 设置文件坏了时提示一次。
        ShowSettingsNotice(window);
        bridge.StartDelivering();
        bridge.Ping();
        // 控制栏等悬浮界面、控件样式：第一张图交出去以后再放（不阻塞冷启动出图）。
        window.DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, window.FinishStartup);
        // 别的进程交来的图片（从资源管理器再打开）：转到界面线程处理。
        if (Instance.IsPrimary)
        {
            // 界面线程真的开始处理了才算交到（TryStart）；排不上队时撤回，新进程自己开窗口。
            Instance.SetHandler(request =>
            {
                var queued = _queue?.TryEnqueue(() =>
                {
                    if (request.TryStart())
                    {
                        Guard("Instance", () => FinderOpen(request.Path));
                    }
                }) == true;
                if (!queued)
                {
                    request.TryCancel();
                }
            });
        }
    }

    private void ShowSettingsNotice(MainWindow window)
    {
        if (window.Core.SettingsNotice(_lang) is string notice)
        {
            window.ShowNotice(notice);
        }
    }

    /// <summary>界面线程上排队执行的事：出了异常记下来，不让整个程序崩溃（排队的回调里漏出的异常会直接结束进程）。</summary>
    private static void Guard(string where, Action action)
    {
        try
        {
            action();
        }
        catch (Exception error)
        {
            Crash.Write(where, error);
        }
    }

    // ---- 主窗口 ----

    /// <summary>开着的主窗口（自检用）。</summary>
    internal IReadOnlyList<MainWindow> Windows => _windows;

    /// <summary>前面的主窗口：最近在前面的那个；它关掉了就用最后打开的。</summary>
    internal MainWindow? ActiveWindow =>
        _lastActive is { IsClosed: false } last && _windows.Contains(last) ? last : _windows.LastOrDefault();

    /// <summary>建一个主窗口（第一个用启动时的核心；之后的与它共用设置、缓存和后台线程）。</summary>
    private MainWindow MakeWindow(CoreBridge bridge)
    {
        var window = new MainWindow(bridge, _lang);
        if (_selfTest is not null)
        {
            // 自检中开的新窗口（从资源管理器打开）：与自检的窗口一样不受真的鼠标指针影响，不出声。
            window.IsSelfTest = true;
            window.IgnoreRealPointer = true;
        }
        window.OnUiRequest = request => Handle(request, window);
        window.OnDisplayChanged = RefreshKeepDisplayOn;
        window.OnBecameActive = () => _lastActive = window;
        window.OnSettingsReloaded = SettingsReloaded;
        window.OnClosedWindow = () => WindowClosed(window);
        _windows.Add(window);
        _lastActive = window;
        return window;
    }

    /// <summary>再开一个窗口：中心在 anchor 中心的右下方 24 有效像素，等第一张图出来再显示。</summary>
    private MainWindow NewWindow(MainWindow anchor)
    {
        var bridge = new CoreBridge(anchor.Core);
        var window = MakeWindow(bridge);
        window.PlaceNear(anchor);
        bridge.StartDelivering();
        window.DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, window.FinishStartup);
        return window;
    }

    /// <summary>
    /// 从资源管理器打开（别的进程交过来的；自检的 finder_open 也走这里）：前面的窗口空着就用它，再找别的空窗口；
    /// 勾了"不允许多个实例"就在前面的窗口里换图；否则开新窗口。path 为空时只把前面的窗口叫到前面。
    /// 返回用的窗口和用法（自检记录）。
    /// </summary>
    internal (MainWindow Window, string How)? FinderOpen(string? path)
    {
        if (ActiveWindow is not MainWindow active)
        {
            return null;
        }
        if (path is null)
        {
            active.BringToFront();
            return (active, "activated");
        }
        MainWindow target;
        string how;
        if (!active.HasImage)
        {
            target = active;
            how = "empty";
        }
        else if (_windows.FirstOrDefault(w => !w.HasImage) is MainWindow empty)
        {
            target = empty;
            how = "empty";
        }
        else if (active.Settings.ReuseWindow)
        {
            target = active;
            how = "reused";
        }
        else
        {
            var created = NewWindow(active);
            created.Bridge.Open(path);
            created.ShowAfterFirstImage();
            return (created, "new");
        }
        target.BringToFront();
        target.Bridge.Open(path);
        return (target, how);
    }

    /// <summary>
    /// 主窗口关掉了：记下位置（最后关的那个留下来）；还有别的窗口时放掉它的核心（连同图片）；
    /// 是最后一个时退出程序（Windows 的习惯）。
    /// </summary>
    private void WindowClosed(MainWindow window)
    {
        _windows.Remove(window);
        if (_selfTest is not null)
        {
            _closedForTest.Add((new WeakReference<MainWindow>(window), new WeakReference<CanvasView>(window.Canvas),
                window.Canvas.ReleasedControl ?? new WeakReference<object>(window.Canvas.Control)));
        }
        if (ReferenceEquals(_lastActive, window))
        {
            _lastActive = _windows.LastOrDefault();
        }
        if (_selfTest is null)
        {
            window.SavePlacement();
        }
        RefreshKeepDisplayOn();
        if (_windows.Count == 0)
        {
            ExitApp(window);
            return;
        }
        if (_selfTest?.IsTestWindow(window) == true)
        {
            _selfTest.WindowClosed();
        }
        // “生成信息”面板显示的是这个窗口的图片：跟着关掉（它的核心马上要放掉）。
        if (_generation is { IsClosed: false } generation && ReferenceEquals(generation.Source, window))
        {
            Guard("App.CloseGeneration", () => generation.Window.Close());
        }
        // 设置窗口、帮助窗口归这个窗口所有时改归前面的窗口（不然系统会随这个窗口一起关掉它们）；
        // 设置窗口用的是这个窗口的核心时改用前面那个窗口的（设置和草稿是共用的）。
        if (ActiveWindow is MainWindow next)
        {
            Guard("App.Reown", () =>
            {
                if (_settings is { IsClosed: false } settings)
                {
                    settings.Reown(window.Hwnd, next.Hwnd);
                    if (ReferenceEquals(settings.Core, window.Core))
                    {
                        settings.Rebind(next.Core);
                    }
                }
                foreach (var help in _help.Where(h => !h.IsClosed))
                {
                    help.Reown(window.Hwnd, next.Hwnd);
                }
                // “生成信息”面板换过看图窗口、但还归这个窗口所有时（一般 Show 时已经改过）。
                if (_generation is { IsClosed: false } panel)
                {
                    panel.Reown(window.Hwnd, next.Hwnd);
                }
            });
        }
        // 放掉这个窗口的核心（连同它的图片、浏览集合；动图的播放线程随之停下）。等这次事件处理完再放
        // （关窗口的过程中还在窗口自己的方法里）。
        var bridge = window.Bridge;
        bridge.Detach();
        if (_queue?.TryEnqueue(DispatcherQueuePriority.Low, () => Guard("App.CloseCore", bridge.Close)) != true)
        {
            Guard("App.CloseCore", bridge.Close);
        }
    }

    /// <summary>最后一个主窗口关掉了：记下查看状态、关掉压缩包（删掉解出的临时文件），关掉其他窗口，退出。</summary>
    private void ExitApp(MainWindow last)
    {
        if (_exiting)
        {
            return;
        }
        _exiting = true;
        Win32.KeepDisplayOn(false);
        if (_selfTest is not null)
        {
            // 自检进行中窗口被关掉了：记下来、写出报告后结束（不再对着关掉的窗口做下一步）。
            _selfTest.WindowClosed();
            return;
        }
        if (!_quitting)
        {
            Guard("App.Shutdown", last.Core.Shutdown);
        }
        _settings?.Discard();
        foreach (var help in _help.ToArray())
        {
            help.Window.Close();
        }
        Exit();
    }

    /// <summary>退出程序：按前面的窗口记下查看状态，关掉所有窗口（前面的最后关，它的位置留下来）。自检时交给自检记录。</summary>
    private void Quit()
    {
        if (_selfTest is not null)
        {
            _selfTest.QuitRequested();
            return;
        }
        if (_quitting)
        {
            return;
        }
        _quitting = true;
        var front = ActiveWindow;
        if (front is not null)
        {
            Guard("App.Shutdown", front.Core.Shutdown);
        }
        foreach (var window in _windows.ToArray())
        {
            if (!ReferenceEquals(window, front))
            {
                window.Close();
            }
        }
        front?.Close();
        // 万一窗口没有关掉（关窗口时出了问题）：照样退出。
        if (!_exiting)
        {
            _exiting = true;
            Exit();
        }
    }

    /// <summary>阻止屏保和显示器睡眠：设置打开、有没最小化的窗口在显示图片时。</summary>
    private void RefreshKeepDisplayOn()
    {
        var want = _windows.Count > 0 && _windows[0].Settings.NoScreensaver && _windows.Any(w => !w.IsClosed && w.HasImage && !w.IsMinimized);
        if (want != _keepingDisplayOn)
        {
            _keepingDisplayOn = want;
            Win32.KeepDisplayOn(want);
        }
    }

    /// <summary>现在是否在阻止显示器睡眠（自检用）。</summary>
    internal bool KeepingDisplayOn => _keepingDisplayOn;

    /// <summary>自检时关掉的窗口（只留弱引用）：看它们回收了没有（关掉的窗口不该一直占着内存）。</summary>
    private readonly List<(WeakReference<MainWindow> Window, WeakReference<CanvasView> Canvas, WeakReference<object> Control)>
        _closedForTest = new();

    /// <summary>
    /// 自检用：关掉过几个窗口；垃圾回收以后，窗口对象、Win2D 的画布控件各还有几个没回收，还有几个画布留着上传用的数组。
    /// 窗口对象 WinUI 自己不回收（空窗口也一样），画布控件和数组是本程序关窗口时该放掉的。
    /// </summary>
    internal (int Closed, int Windows, int Controls, int HeldBuffers) ClosedWindows()
    {
        if (_closedForTest.Count == 0)
        {
            return (0, 0, 0, 0);
        }
        for (var round = 0; round < 3; round++)
        {
            GC.Collect();
            GC.WaitForPendingFinalizers();
        }
        return (_closedForTest.Count,
            _closedForTest.Count(w => w.Window.TryGetTarget(out _)),
            _closedForTest.Count(w => w.Control.TryGetTarget(out _)),
            _closedForTest.Count(w => w.Canvas.TryGetTarget(out var canvas) && canvas.HoldsBuffers));
    }

    // ---- 设置、帮助 ----

    /// <summary>设置窗口（自检用）。</summary>
    internal SettingsWindow? Settings => _settings is { IsClosed: false } s ? s : null;

    /// <summary>开着的帮助窗口（自检用）。</summary>
    internal IReadOnlyList<HelpWindow> HelpWindows => _help;
    internal GenerationWindow? Generation => _generation is { IsClosed: false } panel ? panel : null;

    /// <summary>打开设置窗口（已经打开时放到最前面）。设置由各窗口共用。</summary>
    private void ShowSettings(MainWindow from)
    {
        if (_settings is { IsClosed: false } existing)
        {
            existing.Show();
            return;
        }
        var settings = new SettingsWindow(from.Core, _lang, from) { Quiet = _selfTest is not null };
        settings.OnApplied = SettingsApplied;
        settings.OnClosed = () =>
        {
            if (ReferenceEquals(_settings, settings))
            {
                _settings = null;
            }
        };
        _settings = settings;
        settings.Show();
    }

    /// <summary>设置点了"确定"、已经生效：刷新所有窗口（换了语言时一起换）；写文件失败时提示。</summary>
    private void SettingsApplied(SettingsApplyResult.Applied applied)
    {
        RefreshAll();
        if (applied.SaveError is string error)
        {
            ActiveWindow?.ShowNotice(AnyviewCore.UiText(TextKey.NoticeSettingsSaveFailed, _lang).Replace("{message}", error));
        }
        // 装好的程序：上下文菜单页的开关、界面语言变了，资源管理器里登记的右键菜单、缩略图跟着改。
        if (applied.Changed.Any(key => key.StartsWith("context_menu.", StringComparison.Ordinal) || key == "language.language"))
        {
            Program.RefreshShellRegistration(ActiveWindow?.Core.SettingsPath());
        }
        _selfTest?.SettingsApplied(applied);
    }

    /// <summary>设置文件被别的程序改了、核心已经重新读：像"确定"之后那样刷新所有窗口。</summary>
    private void SettingsReloaded() => RefreshAll();

    private void RefreshAll()
    {
        if (ActiveWindow is not MainWindow active)
        {
            return;
        }
        _lang = active.Core.UiLang(Program.SystemLang);
        foreach (var window in _windows)
        {
            window.ApplyUiSettings(_lang);
        }
        // 开着的帮助窗口：按键（键盘映射）、语言可能变了。
        foreach (var help in _help.ToArray())
        {
            help.Refresh(active.Core, _lang);
        }
        RefreshKeepDisplayOn();
    }

    /// <summary>
    /// "关于"：程序名、版本、许可证；"开源许可"按钮打开开源许可窗口，"确定"（回车、Esc）关掉。
    /// </summary>
    private async System.Threading.Tasks.Task ShowAbout(MainWindow window)
    {
        var lang = window.Lang;
        var text = $"{AnyviewCore.UiText(TextKey.AppName, lang)} {AnyviewCore.CoreVersion()}\n{AnyviewCore.LicenseLine(lang)}";
        var buttons = new[] { window.Core.HelpTitle(HelpKind.Licenses, lang), AnyviewCore.UiText(TextKey.SettingsOk, lang) };
        if (await window.AskAsync(new Prompt(text, buttons), Microsoft.UI.Xaml.Controls.ContentDialogButton.Close) == 0)
        {
            ShowHelp(HelpKind.Licenses, window);
        }
    }

    /// <summary>帮助窗口：同一种已经开着时放到最前面。</summary>
    private void ShowHelp(HelpKind kind, MainWindow from)
    {
        if (_help.FirstOrDefault(h => h.Kind == kind && !h.IsClosed) is HelpWindow existing)
        {
            existing.Show();
            return;
        }
        var help = new HelpWindow(kind, from.Core, _lang, from);
        help.OnClosed = () => _help.Remove(help);
        _help.Add(help);
        help.Show();
    }

    /// <summary>“生成信息”面板：已经开着时换成这个窗口的当前图片、放到最前面。</summary>
    private void ShowGenerationInfo(MainWindow from)
    {
        if (_generation is { IsClosed: false } existing)
        {
            existing.Show(from, _lang);
            return;
        }
        var panel = new GenerationWindow(from, _lang);
        panel.OnClosed = () =>
        {
            if (ReferenceEquals(_generation, panel))
            {
                _generation = null;
            }
        };
        _generation = panel;
        panel.Show(from, _lang);
    }

    // ---- 动作 ----

    /// <summary>动作中需要应用层完成的部分。window 是执行动作的窗口。</summary>
    private void Handle(UiRequest request, MainWindow window)
    {
        _selfTest?.UiRequested(request);
        switch (request)
        {
            case UiRequest.OpenFileDialog:
                OpenDialog(window, false);
                break;
            case UiRequest.ClearRecent:
                // 菜单栏"打开最近使用"里的"清除菜单"。
                window.Core.ClearRecent();
                break;
            case UiRequest.OpenFolderDialog:
                OpenDialog(window, true);
                break;
            case UiRequest.Quit:
                Quit();
                break;
            case UiRequest.CloseOrQuit:
            case UiRequest.CloseWindow:
                // Esc、关闭窗口：开着别的窗口时只关这个窗口；这是最后一个窗口时退出程序。
                if (_windows.Any(w => !ReferenceEquals(w, window)))
                {
                    window.Close();
                }
                else
                {
                    Quit();
                }
                break;
            case UiRequest.ToggleFullscreen:
                window.ToggleFullScreen();
                break;
            case UiRequest.Minimize:
            case UiRequest.HideApp:
                window.Minimize();
                break;
            case UiRequest.ToggleMaximize:
                window.ToggleMaximize();
                break;
            case UiRequest.ShowAbout:
                _ = ShowAbout(window);
                break;
            case UiRequest.ShowContextMenu:
                window.ShowContextMenu();
                break;
            case UiRequest.TrashFile t:
                window.Trash(t.Path, t.Name, t.Confirm);
                break;
            case UiRequest.RevealFile r:
                window.Reveal(r.Path);
                break;
            case UiRequest.CopyImage c:
                _ = window.CopyImage(c.Path);
                break;
            case UiRequest.OpenInEditor e:
                window.OpenInEditor(e.Path, e.Editor);
                break;
            case UiRequest.SetWallpaper w:
                window.SetWallpaper(w.Path);
                break;
            case UiRequest.Print p:
                window.PrintImage(p.Name);
                break;
            case UiRequest.ShowSettings:
                ShowSettings(window);
                break;
            case UiRequest.ShowPicker:
                window.ShowPicker();
                break;
            case UiRequest.ShowHelp h:
                ShowHelp(h.Kind, window);
                break;
            case UiRequest.ShowGenerationInfo:
                ShowGenerationInfo(window);
                break;
            case UiRequest.ApplySettings:
                // 界面相关的设置（如"总在最前"）是各窗口共用的。
                RefreshAll();
                break;
            default:
                // 清除最近使用（Windows 版还没有最近使用的列表）、隐藏其他、全部显示（macOS 的菜单）：不用做。
                break;
        }
    }

    /// <summary>打开文件或文件夹（系统的选择对话框），在这个窗口里打开。</summary>
    private async void OpenDialog(MainWindow window, bool folder)
    {
        try
        {
            string? path;
            if (folder)
            {
                var picker = new FolderPicker { SuggestedStartLocation = PickerLocationId.PicturesLibrary };
                picker.FileTypeFilter.Add("*");
                InitializeWithWindow.Initialize(picker, window.Hwnd);
                path = (await picker.PickSingleFolderAsync())?.Path;
            }
            else
            {
                var picker = new FileOpenPicker { SuggestedStartLocation = PickerLocationId.PicturesLibrary, ViewMode = PickerViewMode.Thumbnail };
                foreach (var ext in AnyviewCore.SupportedExtensions().Concat(AnyviewCore.ArchiveExtensions()).Distinct())
                {
                    picker.FileTypeFilter.Add("." + ext);
                }
                InitializeWithWindow.Initialize(picker, window.Hwnd);
                path = (await picker.PickSingleFileAsync())?.Path;
            }
            if (!string.IsNullOrEmpty(path) && !window.IsClosed)
            {
                window.Bridge.Open(path);
            }
        }
        catch (Exception error)
        {
            if (!window.IsClosed)
            {
                window.ShowNotice(AnyviewCore.UiText(TextKey.NoticeOpenFailed, window.Lang).Replace("{message}", error.Message));
            }
        }
    }
}
