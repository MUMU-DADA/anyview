// 主窗口（与 macOS 的 MainWindowController 对应）：画布、空态、通知、倍率提示、悬浮界面。
// 处理核心事件；按键、菜单、鼠标手势都交给核心执行动作，这里只把结果落到界面上。
// 窗口跟着图片变大小，大小由核心算（有效像素），这里按 DPI 换成物理像素交给 AppWindow。
using System;
using System.Diagnostics;
using System.IO;
using System.Threading.Tasks;
using Anyview.Core;
using Microsoft.UI;
using Microsoft.UI.Input;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.ApplicationModel.DataTransfer;
using Windows.Foundation;
using Windows.Graphics;
using Windows.Graphics.Imaging;
using Windows.Storage;
using Windows.Storage.Streams;
using Windows.System;
using Windows.UI;
using WinRT.Interop;

namespace Anyview;

internal sealed class MainWindow : Window
{
    /// <summary>直接启动（没有图片）时的窗口大小（有效像素）。</summary>
    public const double LaunchWidth = 900;
    public const double LaunchHeight = 600;

    private readonly CoreBridge _bridge;
    public readonly CanvasView Canvas;
    public readonly Overlays Overlays;
    private readonly Grid _grid = new();
    /// <summary>窗口的内容：上面一行是菜单栏（平常没有），下面是图片区 _grid。</summary>
    private readonly Grid _root = new();
    /// <summary>菜单栏：设置"不显示菜单栏"关掉以后才建（默认不显示，冷启动不碰它）。</summary>
    private MenuBarView? _menu;
    private readonly HostControl _host;
    private readonly Grid _empty = new();
    private readonly TextBlock _emptyHint = new() { FontSize = 15, Foreground = new SolidColorBrush(Color.FromArgb(255, 158, 158, 158)), HorizontalAlignment = HorizontalAlignment.Center, TextAlignment = TextAlignment.Center };
    private readonly TextBlock _emptyStatus = new() { FontSize = 12, Foreground = new SolidColorBrush(Color.FromArgb(255, 110, 110, 110)), HorizontalAlignment = HorizontalAlignment.Center };
    private readonly Border _notice;
    private readonly TextBlock _noticeText = new() { FontSize = 13, Foreground = new SolidColorBrush(Color.FromArgb(255, 242, 242, 242)), TextWrapping = TextWrapping.Wrap, MaxWidth = 520 };
    private readonly Border _zoom;
    private readonly TextBlock _zoomText = new() { FontSize = 20, FontWeight = Microsoft.UI.Text.FontWeights.Medium, Foreground = new SolidColorBrush(Color.FromArgb(255, 242, 242, 242)) };
    private readonly OutlinedText _filename = new();
    private int _noticeSerial;
    private int _zoomSerial;
    public readonly nint Hwnd;
    /// <summary>窗口自带的普通呈现方式（建窗口时系统给的，不另建、不切换，省启动时间）；全屏回来时再套回它。</summary>
    private readonly OverlappedPresenter _presenter;
    private bool _wasMaximized;
    /// <summary>这个窗口显示过图片（打不开时要不要显示空态的提示）。</summary>
    private bool _everShown;
    private double _minimumScale;

    public AppCore Core => _bridge.Core;
    public CoreBridge Bridge => _bridge;
    public Lang Lang { get; private set; }
    public UiSettings Settings { get; private set; }
    public ulong Generation { get; private set; }
    public string CurrentName { get; private set; } = "";
    public uint? CurrentIndex { get; private set; }
    public uint? CurrentTotal { get; private set; }
    public bool FullScreen { get; private set; }
    public bool TitleBarShown { get; private set; } = true;
    private bool _titleBarApplied;
    /// <summary>最近一次按图片定的窗口内容区大小（有效像素，自检核对用）。</summary>
    public Size? LastFitSize { get; private set; }
    public bool IsClosed { get; private set; }
    private (ulong Generation, Transition Turn)? _pendingTurn;
    public readonly System.Collections.Generic.List<string> SoundLog = new();
    /// <summary>带着图片启动时等第一张图出来、窗口按图片定好大小再显示；读得慢时 0.3 秒后先显示。</summary>
    private bool _showWhenImageReady;
    private bool _activated;

    // 自检用的钩子。
    public bool IsSelfTest;
    /// <summary>自检时为真：真的鼠标指针在窗口上移动不影响控制栏和箭头（自检按步骤模拟，结果才可比）。</summary>
    public bool IgnoreRealPointer;
    public Func<bool, bool>? OnBoundary;
    /// <summary>
    /// 自检用：询问的对话框真的弹出来以后叫它，交给它对话框的内容、默认按钮和一个"按第几个按钮"的函数
    /// （自检的下一步用这个函数回答）。现在只有安全上限的询问会走到这里：到达首尾的询问自检另接，删除在自检里不问。
    /// </summary>
    public Action<Prompt, ContentDialogButton, Action<int>>? OnDialogOpened;
    public Action? OnPong;
    public Action<ImageHandle>? OnImageShown;
    public Action? OnPreviewShown;
    public Action? OnAnimationFrame;
    public Action<string>? OnNotice;
    public Action<string, string>? OnExternal;
    public Action<string>? OnProblem;
    public Action? OnDisplayChanged;
    /// <summary>图片关掉了、回到空态（自检用：删掉唯一一张图之类的步骤以此完成）。</summary>
    public Action? OnImageClosed;
    public Action<UiRequest>? OnUiRequest;
    public Action? OnClosedWindow;
    /// <summary>窗口来到前面（应用记下"前面的窗口"）。</summary>
    public Action? OnBecameActive;
    /// <summary>设置文件被别的程序改了、核心已经重新读：由应用刷新所有窗口。</summary>
    public Action? OnSettingsReloaded;
    /// <summary>这个窗口的选择图片窗口（开着时）。</summary>
    public PickerWindow? Picker { get; private set; }
    /// <summary>普通状态（不是最大化、最小化、全屏）下最近的外框（物理像素）：记住窗口位置用。</summary>
    private RectInt32? _restoredRect;
    /// <summary>上次关掉时是最大化的：显示窗口时再最大化。</summary>
    private bool _restoreMaximized;
    /// <summary>用户用系统的方式开始移动、改大小时的外框大小（松开时没变大小才算移动，吸附边缘）。</summary>
    private SizeInt32? _sizeMoveStart;
    /// <summary>右键菜单建好、弹出之前（自检用）。</summary>
    public Action<MenuFlyout>? OnContextMenu;
    /// <summary>标题栏隐藏时右上角自己画的三个按钮。</summary>
    private readonly CaptionButtons _caption;
    /// <summary>标题栏隐藏时顶边改窗口大小的细带子（顶边、左上角、右上角）。</summary>
    private readonly ResizeEdge[] _resizeEdges;

    public MainWindow(CoreBridge bridge, Lang lang)
    {
        _bridge = bridge;
        Lang = lang;
        Settings = bridge.Core.UiSettings();
        Hwnd = WindowNative.GetWindowHandle(this);
        Win32.InstallScreenSaverGuard(Hwnd);
        // 拖标题栏移动窗口（系统负责移动）：松开鼠标后吸附边缘。
        Win32.OnSizeMove(Hwnd, entering =>
        {
            if (entering)
            {
                _sizeMoveStart = AppWindow.Size;
                return;
            }
            // 拖动中不记位置（每移动一步都会来 AppWindow.Changed），松开时记一次。
            if (WindowMovable)
            {
                var position = AppWindow.Position;
                var size = AppWindow.Size;
                _restoredRect = new RectInt32(position.X, position.Y, size.Width, size.Height);
            }
            var moved = _sizeMoveStart is SizeInt32 start && start.Width == AppWindow.Size.Width && start.Height == AppWindow.Size.Height;
            _sizeMoveStart = null;
            if (moved)
            {
                FinishUserMove();
            }
            // 拖到别的显示器上：松开后按那块显示器的配置文件。
            if (_startupFinished)
            {
                ReportDisplayProfile(force: false);
            }
        });
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            _presenter = presenter;
        }
        else
        {
            _presenter = OverlappedPresenter.Create();
            AppWindow.SetPresenter(_presenter);
        }
        ApplyMinimumSize();
        // 深色外观（与 macOS 的 darkAqua 相同）：显示标题栏时标题栏也是深色。
        AppWindow.TitleBar.PreferredTheme = TitleBarTheme.Dark;
        Title = AnyviewCore.UiText(TextKey.AppName, lang);
        bridge.Queue = DispatcherQueue;

        LaunchClock.Mark("windowBase");
        Canvas = new CanvasView(bridge.Core, DispatcherQueue);
        LaunchClock.Mark("canvasCreated");
        Overlays = new Overlays(DispatcherQueue);
        LaunchClock.Mark("overlaysBuilt");

        // 空态：深灰背景加居中提示，不接收鼠标。
        var emptyStack = new StackPanel { Spacing = 8, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center };
        emptyStack.Children.Add(_emptyHint);
        emptyStack.Children.Add(_emptyStatus);
        _empty.Children.Add(emptyStack);
        _empty.IsHitTestVisible = false;
        _emptyHint.Text = AnyviewCore.EmptyStateHint(Platform.Windows, lang);
        _emptyStatus.Text = $"{AnyviewCore.UiText(TextKey.AppName, lang)} {AnyviewCore.CoreVersion()}";

        _notice = new Border
        {
            Child = _noticeText,
            Background = new SolidColorBrush(Color.FromArgb(158, 0, 0, 0)),
            Padding = new Thickness(10, 4, 10, 4),
            Visibility = Visibility.Collapsed,
            IsHitTestVisible = false,
        };
        _zoom = new Border
        {
            Child = _zoomText,
            Background = new SolidColorBrush(Color.FromArgb(140, 0, 0, 0)),
            Padding = new Thickness(12, 4, 12, 4),
            HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center,
            Visibility = Visibility.Collapsed,
            IsHitTestVisible = false,
        };
        // 标题栏隐藏时显示的文件名：白字深色描边，顶端居中，左右让开系统的最小化、最大化、关闭按钮。
        _filename.Size = 13;
        _filename.HorizontalAlignment = HorizontalAlignment.Center;
        _filename.VerticalAlignment = VerticalAlignment.Top;
        _filename.Margin = new Thickness(150, 7, 150, 0);
        _filename.Visibility = Visibility.Collapsed;

        LaunchClock.Mark("emptyBuilt");
        // 悬浮界面（控制栏、箭头、图片信息）等第一张图交出去以后再放进窗口（FinishStartup），冷启动不等它们。
        foreach (var element in new UIElement[] { Canvas.Control, _empty, _filename, _notice, _zoom })
        {
            _grid.Children.Add(element);
        }
        // 带着图片启动时先不显示空态的提示（马上就有图；打不开时再显示）。
        if (Program.Options.OpenPath is not null)
        {
            _empty.Visibility = Visibility.Collapsed;
        }
        // 标题栏隐藏时的标题栏按钮、顶边的改大小带子（显示与否见 RefreshChrome）：盖在画布、悬浮界面上面。
        // 它们放在整个内容（_root）的顶端，不放在图片区里：有菜单栏时正好排在菜单栏那一行的右边。
        _root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        _root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        Grid.SetRow(_grid, 1);
        _root.Children.Add(_grid);
        _caption = new CaptionButtons(Invoke);
        Microsoft.UI.Xaml.Controls.Canvas.SetZIndex(_caption.Root, 10);
        Grid.SetRowSpan(_caption.Root, 2);
        _root.Children.Add(_caption.Root);
        _resizeEdges = new[]
        {
            new ResizeEdge(InputSystemCursorShape.SizeNorthSouth, 12, () => Hwnd, MinimumOuterSize)
            {
                Height = 4, VerticalAlignment = VerticalAlignment.Top, Margin = new Thickness(8, 0, 8, 0),
            },
            new ResizeEdge(InputSystemCursorShape.SizeNorthwestSoutheast, 13, () => Hwnd, MinimumOuterSize)
            {
                Width = 8, Height = 4, HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Top,
            },
            new ResizeEdge(InputSystemCursorShape.SizeNortheastSouthwest, 14, () => Hwnd, MinimumOuterSize)
            {
                Width = 8, Height = 4, HorizontalAlignment = HorizontalAlignment.Right, VerticalAlignment = VerticalAlignment.Top,
            },
        };
        foreach (var edge in _resizeEdges)
        {
            Microsoft.UI.Xaml.Controls.Canvas.SetZIndex(edge, 11);
            edge.Visibility = Visibility.Collapsed;
            Grid.SetRowSpan(edge, 2);
            _root.Children.Add(edge);
        }
        _grid.Background = new SolidColorBrush(Color.FromArgb(255, 0x20, 0x20, 0x20));
        _grid.AllowDrop = true;
        _host = new HostControl
        {
            Content = _root,
            IsTabStop = true,
            UseSystemFocusVisuals = false,
            RequestedTheme = ElementTheme.Dark,
        };
        UiLanguage.Apply(_host, lang);
        LaunchClock.Mark("hostBuilt");
        Content = _host;
        LaunchClock.Mark("contentSet");
        _host.PreviewKeyDown += OnKeyDown;
        _host.ContextRequested += OnContextRequested;
        _grid.PointerPressed += OnPointerPressed;
        // 指针在控制栏的按钮上移动时事件可能已被按钮处理，也要收到（控制栏、箭头的显示）。
        _grid.AddHandler(UIElement.PointerMovedEvent, new PointerEventHandler(OnPointerMoved), true);
        _grid.PointerReleased += OnPointerReleased;
        // 以下几个事件会从子控件冒泡上来：只管画布自己的。
        _grid.PointerCanceled += (_, e) =>
        {
            if (ReferenceEquals(e.OriginalSource, _grid))
            {
                EndPointer();
            }
        };
        _grid.PointerCaptureLost += (_, e) =>
        {
            if (ReferenceEquals(e.OriginalSource, _grid))
            {
                EndPointer();
            }
        };
        _grid.PointerExited += OnPointerExited;
        // 内容区和系统给的客户区差多少（Windows 10 上标题栏隐藏时顶端留 1 像素给窗口边线）：窗口大小按它补上。
        _grid.SizeChanged += (_, _) => UpdateContentInset();
        _grid.PointerWheelChanged += OnWheel;
        _grid.DragOver += OnDragOver;
        _grid.Drop += OnDrop;

        Overlays.OnAction = Invoke;
        Overlays.Strip.OnJump = index => Core.Dispatch(new Command.Navigate(new Navigation.Jump(index)));
        bridge.OnEvent = Handle;
        Activated += (_, e) =>
        {
            if (IsClosed)
            {
                return;
            }
            var active = e.WindowActivationState != WindowActivationState.Deactivated;
            _caption.SetActive(active);
            ChangeLog?.Add((active ? "A" : "D") + (IsMinimized ? ":min" : ":shown"));
            CheckMinimized();
            if (active)
            {
                _host.Focus(FocusState.Programmatic);
                OnBecameActive?.Invoke();
                // 显示器的配置文件可能在系统"颜色管理"里换过：回到窗口时重新取一次。
                if (_startupFinished)
                {
                    ReportDisplayProfile(force: true);
                }
            }
        };
        AppWindow.Changed += (_, args) =>
        {
            Diag.WindowChanges++;
            if (IsClosed)
            {
                return;
            }
            // 拖到缩放比例不同的屏幕上：最小尺寸按新的比例换算。
            if (args.DidSizeChange || args.DidPositionChange)
            {
                ApplyMinimumSize();
                if (_startupFinished && _sizeMoveStart is null)
                {
                    ReportDisplayProfile(force: false);
                }
                // 用户正在拖动、拉边框（系统的移动循环里）时每一步都会来：只做必要的，位置等松开时再记。
                if (WindowMovable && _sizeMoveStart is null)
                {
                    var position = AppWindow.Position;
                    var size = AppWindow.Size;
                    _restoredRect = new RectInt32(position.X, position.Y, size.Width, size.Height);
                }
            }
            // 最大化、还原以后：中间的标题栏按钮换图标，顶边的改大小带子只在普通状态时有。
            if (args.DidPresenterChange || args.DidSizeChange)
            {
                RefreshChrome();
            }
            // 最小化、取回：阻止显示器睡眠只在窗口看得见时生效。
            ChangeLog?.Add((args.DidPresenterChange ? "P" : "") + (args.DidSizeChange ? "S" : "") + (args.DidPositionChange ? "M" : "")
                + (args.DidVisibilityChange ? "V" : "") + (args.DidZOrderChange ? "Z" : "") + (IsMinimized ? ":min" : ":shown"));
            CheckMinimized();
            // 最大化按钮、双击标题栏也走这里：从最大化还原时回到当前图片的大小。
            // 最小化不算还原（最大化的窗口最小化后再点出来，仍是最大化）。
            if (!(args.DidPresenterChange || args.DidSizeChange) || FullScreen
                || _presenter.State == OverlappedPresenterState.Minimized)
            {
                return;
            }
            var maximized = IsMaximized;
            if (_wasMaximized && !maximized)
            {
                _wasMaximized = false;
                FollowCurrentImage();
            }
            _wasMaximized = maximized;
        };
        LaunchClock.Mark("eventsAttached");
        Closed += (_, _) =>
        {
            IsClosed = true;
            Win32.OnSizeMove(Hwnd, null);
            Win32.ForgetCaptionless(Hwnd);
            // 选择图片窗口随主窗口关掉；画布放掉图片（不等垃圾回收），之后到点的延时都不再做事。
            Picker?.Window.Close();
            Canvas.Release();
            Overlays.Detach();
            OnClosedWindow?.Invoke();
        };
        ApplyUiSettings();
        LaunchClock.Mark("windowConstructed");
    }

    private string Text(TextKey key) => AnyviewCore.UiText(key, Lang);

    private bool _startupFinished;

    /// <summary>
    /// 第一张图（或空态）交出去以后再做的事，冷启动不等它们：悬浮界面放进窗口。由应用在交出启动时的事件后以低优先级排上。
    /// 不在这时加 WinUI 的控件样式（XamlControlsResources）：窗口建好以后再加，程序直接崩溃（Windows 上实测，0xC000027B）。
    /// </summary>
    public void FinishStartup()
    {
        if (_startupFinished || IsClosed)
        {
            return;
        }
        _startupFinished = true;
        try
        {
            Overlays.ApplySettings(Core, Lang, Settings);
            _grid.Children.Add(Overlays.Root);
            RefreshOverlays();
            LaunchClock.Mark("overlaysReady");
            // 顶端那一条、上边框交给系统（UpdateNonClientRegions）：也等到这时，不碰冷启动。
            _nonClientReady = true;
            UpdateNonClientRegions();
            After(800, WarmUpWindowMove);
            // 显示器的 ICC 配置文件：启动收尾时报一次，之后换了显示器再报。
            ReportDisplayProfile(force: true);
            // 窗口图标：读文件，放到第一张图之后，不碰冷启动。
            SubWindows.ApplyIcon(AppWindow);
        }
        catch (Exception error)
        {
            Crash.Write("MainWindow.FinishStartup", error);
        }
    }

    /// <summary>
    /// 用户拖动窗口边缘时的最小尺寸：480x360 有效像素（与 macOS 相同）。系统按物理像素算，
    /// 所以按窗口所在屏幕的缩放比例换算，换了屏幕再算一次。
    /// </summary>
    /// <summary>窗口外框的最小宽、高（物理像素，与 ApplyMinimumSize 给系统的相同；程序自己改大小时用）。</summary>
    private (int Width, int Height) MinimumOuterSize()
    {
        var scale = DpiScale;
        return ((int)Math.Round(480 * scale), (int)Math.Round(360 * scale));
    }

    private void ApplyMinimumSize()
    {
        var scale = DpiScale;
        if (scale == _minimumScale)
        {
            return;
        }
        _minimumScale = scale;
        _presenter.PreferredMinimumWidth = (int)Math.Round(480 * scale);
        _presenter.PreferredMinimumHeight = (int)Math.Round(360 * scale);
    }

    // ---- 显示窗口 ----

    /// <summary>显示窗口并放到前面。</summary>
    public void ShowNow()
    {
        _showWhenImageReady = false;
        if (!_activated)
        {
            _activated = true;
            Activate();
            LaunchClock.Mark("windowShown");
            if (_restoreMaximized)
            {
                _restoreMaximized = false;
                _presenter.Maximize();
            }
        }
        else
        {
            AppWindow.Show();
            Win32.SetForegroundWindow(Hwnd);
        }
    }

    /// <summary>等第一张图出来、窗口按图片定好大小再显示；读得慢时 0.3 秒后先显示。</summary>
    public void ShowAfterFirstImage()
    {
        _showWhenImageReady = true;
        After(300, ShowIfWaiting);
    }

    private void ShowIfWaiting()
    {
        if (_showWhenImageReady)
        {
            ShowNow();
        }
    }

    // ---- 设置 ----

    /// <summary>让界面相关的设置生效（启动时、设置"确定"后）：背景色、隐藏的控件、悬停提示、字号、通知位置、总在最前、标题栏、语言。</summary>
    public void ApplyUiSettings(Lang? newLang = null)
    {
        var settings = Core.UiSettings();
        Settings = settings;
        _caption.SetTips(Core, newLang ?? Lang);
        // 冷启动的分解：这一段在上一次报告里要 190 毫秒左右，分开记下各步的时刻（只记第一次）。
        LaunchClock.Mark("uiSettingsRead");
        if (newLang is Lang l && l != Lang)
        {
            Lang = l;
            UiLanguage.Apply(_host, l);
            _emptyHint.Text = AnyviewCore.EmptyStateHint(Platform.Windows, l);
            UpdateTitle();
        }
        Canvas.SetBackground(settings.BackgroundColor);
        Canvas.RefreshAlphaFill();
        RefreshBackgroundImage();
        LaunchClock.Mark("backgroundApplied");
        if (settings.HideZoomInfo)
        {
            _zoom.Visibility = Visibility.Collapsed;
        }
        if (_presenter.IsAlwaysOnTop != settings.AlwaysOnTop)
        {
            _presenter.IsAlwaysOnTop = settings.AlwaysOnTop;
        }
        LaunchClock.Mark("topmostApplied");
        ApplyTitleBar(settings.ShowTitleBar);
        LaunchClock.Mark("titleBarApplied");
        ApplyMenuBar(rebuild: true);
        PlaceTopControls();
        RefreshFilename();
        // 悬浮界面放进窗口以前（冷启动）不用：FinishStartup 放进去时再按设置摆好。
        if (_startupFinished)
        {
            Overlays.ApplySettings(Core, Lang, settings);
            RefreshOverlays();
        }
    }

    private static Color ToColor(uint rgb) => Color.FromArgb(255, (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb);

    /// <summary>背景图片：有时空态透明，露出画布上的背景图片。</summary>
    private void RefreshBackgroundImage()
    {
        var image = Core.BackgroundImage();
        var hasImage = image is not null;
        Canvas.SetBackgroundImage(image);
        _empty.Background = hasImage ? null : new SolidColorBrush(ToColor(Settings.BackgroundColor));
        _grid.Background = new SolidColorBrush(ToColor(Settings.BackgroundColor));
    }

    /// <summary>
    /// 设置"显示标题栏"：打开时是普通的带标题栏的窗口；关掉时去掉系统的标题栏（Win32.SetCaptionless），图片铺满
    /// 整个窗口，右上角是程序自己画的最小化、最大化、关闭按钮，顶端一条（标题栏的高度）可以拖动窗口、双击最大化，
    /// 顶边有一条细带子可以改大小。改了以后窗口按当前图片重新定大小。
    /// </summary>
    private void ApplyTitleBar(bool shown)
    {
        if (shown == TitleBarShown && _titleBarApplied)
        {
            return;
        }
        _titleBarApplied = true;
        TitleBarShown = shown;
        ApplyTitleBarMethod();
        LaunchClock.Mark("titleBarExtended");
        RefreshChrome();
        FollowCurrentImage();
    }

    /// <summary>
    /// 标题栏隐藏时用 WinUI 自己的办法（ExtendsContentIntoTitleBar，系统的按钮）：早期实现中冷启动要多等约 170 ms。
    /// 只用来对比移动窗口的耗时（环境变量 ANYVIEW_TITLEBAR=winui；自检的 move_bench 两种都量）。
    /// </summary>
    public bool WinUiTitleBar { get; private set; } = Program.WinUiTitleBar;

    /// <summary>按 TitleBarShown 和 WinUiTitleBar 去掉或恢复系统的标题栏。</summary>
    private void ApplyTitleBarMethod()
    {
        var hidden = !TitleBarShown;
        if (WinUiTitleBar)
        {
            Win32.SetCaptionless(Hwnd, false);
            ExtendsContentIntoTitleBar = hidden;
            var bar = AppWindow.TitleBar;
            bar.ButtonBackgroundColor = hidden ? Colors.Transparent : null;
            bar.ButtonInactiveBackgroundColor = hidden ? Colors.Transparent : null;
            bar.ButtonForegroundColor = hidden ? Color.FromArgb(255, 235, 235, 235) : null;
            bar.ButtonInactiveForegroundColor = hidden ? Color.FromArgb(255, 170, 170, 170) : null;
            bar.ButtonHoverBackgroundColor = hidden ? Color.FromArgb(64, 255, 255, 255) : null;
        }
        else
        {
            if (ExtendsContentIntoTitleBar)
            {
                ExtendsContentIntoTitleBar = false;
            }
            Win32.SetCaptionless(Hwnd, hidden);
        }
    }

    /// <summary>自检：换一种去掉标题栏的办法（move_bench 对比拖动窗口的耗时）。</summary>
    public void UseWinUiTitleBarForTest(bool winui)
    {
        if (WinUiTitleBar == winui)
        {
            return;
        }
        WinUiTitleBar = winui;
        ApplyTitleBarMethod();
        RefreshChrome();
        FollowCurrentImage();
    }

    /// <summary>自己画的标题栏按钮、顶边的改大小带子：标题栏隐藏、不是全屏时显示；带子只在普通状态（不是最大化）时有。</summary>
    private void RefreshChrome()
    {
        var chrome = !TitleBarShown && !FullScreen && !WinUiTitleBar;
        _caption.Root.Visibility = chrome ? Visibility.Visible : Visibility.Collapsed;
        _caption.SetMaximized(IsMaximized);
        var edges = chrome && _presenter.State == OverlappedPresenterState.Restored ? Visibility.Visible : Visibility.Collapsed;
        foreach (var edge in _resizeEdges)
        {
            edge.Visibility = edges;
        }
        UpdateNonClientRegions();
    }

    /// <summary>标题栏的高度（有效像素）：标题栏隐藏时，内容顶端这么高的一条当标题栏用。</summary>
    private const double CaptionHeight = 32;

    /// <summary>
    /// 内容顶端被"标题栏"占着的高度（有效像素）：标题栏隐藏、内容铺到顶端时，那一条是拖动窗口的地方，右上角是三个按钮，
    /// 图片信息、通知要放在它下面（与 macOS 放在标题栏下面相同）。全屏时也照样让开。
    /// </summary>
    private double TopInset => TitleBarShown || MenuBarShown ? 0 : CaptionHeight;

    /// <summary>指针在顶端那一条（标题栏隐藏时当标题栏用）上。有菜单栏时那一条是菜单栏那一行，不在图片区里。</summary>
    private bool InCaptionStrip(Point p) =>
        !TitleBarShown && !FullScreen && !WinUiTitleBar && !MenuBarShown && p.Y >= 0 && p.Y < CaptionHeight;

    /// <summary>菜单栏现在显示着。</summary>
    public bool MenuBarShown => _menu is not null && _menu.Row.Visibility == Visibility.Visible;

    /// <summary>菜单栏（自检用；没建过时为空）。</summary>
    public MenuBarView? MenuBar => _menu;

    /// <summary>
    /// 菜单栏（设置"不显示菜单栏"）：关掉这一项时在窗口顶端显示一行菜单，内容与 macOS 的菜单栏相同；全屏时不显示
    /// （右键菜单、快捷键照常）。图片区在它下面，窗口跟着图片时多出这一行的高度。标题栏隐藏时这一行就是标题栏：
    /// 菜单右边的空白拖动窗口，右上角是三个按钮。rebuild 为真时按现在的语言、键盘映射重排菜单。
    /// </summary>
    private void ApplyMenuBar(bool rebuild)
    {
        var show = !Settings.HideMenuBar && !FullScreen;
        if (show && _menu is null)
        {
            _menu = new MenuBarView(Core, Invoke, path => Core.Dispatch(new Command.Open(path)),
                () => _host.Focus(FocusState.Programmatic));
            _menu.Row.Visibility = Visibility.Collapsed;
            _root.Children.Insert(0, _menu.Row);
            // 这一行排好了（高度、各个菜单占的宽度定下来了）：窗口大小、拖动窗口的范围按它重新算。
            _menu.Row.SizeChanged += (_, _) =>
            {
                UpdateContentInset();
                UpdateNonClientRegions();
            };
            // 菜单右边的空白平常登记成了标题栏、由系统处理（UpdateNonClientRegions）；没登记上时按下交到这里。
            _menu.Row.PointerPressed += (sender, e) =>
            {
                if (ReferenceEquals(e.OriginalSource, sender) && !TitleBarShown && !WinUiTitleBar
                    && e.GetCurrentPoint(_menu.Row).Properties.PointerUpdateKind == PointerUpdateKind.LeftButtonPressed)
                {
                    e.Handled = true;
                    CaptionStripDown(e);
                }
            };
            rebuild = true;
        }
        if (_menu is null)
        {
            return;
        }
        if (show && rebuild)
        {
            _menu.Rebuild(Lang);
        }
        var visibility = show ? Visibility.Visible : Visibility.Collapsed;
        if (_menu.Row.Visibility == visibility)
        {
            return;
        }
        if (!show)
        {
            _menu.CloseOpen();
        }
        _menu.Row.Visibility = visibility;
        PlaceTopControls();
        UpdateNonClientRegions();
    }

    private DateTime _lastStripDown = DateTime.MinValue;
    private Win32.POINT _lastStripPoint;

    /// <summary>
    /// 在顶端那一条上按下左键，而且按下交到了程序这里（平常那一条登记成了标题栏，由系统处理，见 UpdateNonClientRegions；
    /// 登记没生效时才会到这里）：双击最大化、还原；拖动时程序自己移动窗口（不交给系统，原因见 StartWindowMove）。
    /// 最大化时不拖。双击按屏幕上的位置判断，真的拖动过就不算（拖完马上再按一下不会最大化）。
    /// </summary>
    private void CaptionStripDown(PointerRoutedEventArgs e)
    {
        // 左键已经松开才交到这里：是系统处理完（没拖动，当作单击）才转过来的（
        // 按下马上就拖的会这样）。不算单击、双击（双击由系统处理，这里再算一次会把刚最大化的窗口又还原），也不移动。
        if (e.Pointer.PointerDeviceType == PointerDeviceType.Mouse && !Win32.LeftButtonDown())
        {
            DragProbe.NotStartedMove("顶端那一条（程序移动）");
            return;
        }
        Win32.GetCursorPos(out var screen);
        var now = DateTime.UtcNow;
        var doubleClick = (now - _lastStripDown).TotalMilliseconds <= Win32.GetDoubleClickTime()
            && Math.Abs(screen.X - _lastStripPoint.X) < 4 && Math.Abs(screen.Y - _lastStripPoint.Y) < 4;
        _lastStripDown = doubleClick ? DateTime.MinValue : now;
        _lastStripPoint = screen;
        _moveStart = null;
        _dragLast = null;
        _pendingClick = null;
        if (doubleClick)
        {
            Invoke("maximize");
            return;
        }
        if (!WindowMovable)
        {
            return;
        }
        _captured = _grid.CapturePointer(e.Pointer);
        if (!StartWindowMove("顶端那一条（程序移动）", e.Pointer.PointerDeviceType))
        {
            _grid.ReleasePointerCapture(e.Pointer);
            _captured = false;
        }
    }

    // ---- 标题栏、顶边交给系统（非客户区） ----

    private InputNonClientPointerSource? _nonClient;
    private bool _nonClientReady;
    private (RectInt32 Caption, RectInt32 Top)? _nonClientApplied;
    /// <summary>顶端那一条、上边框登记的情况（自检报告里写出来：登记出错时按下交到程序这里，由程序自己移动）。</summary>
    public string NonClientState { get; private set; } = "还没登记";

    /// <summary>
    /// 标题栏隐藏时：顶端那一条（除去右上角三个按钮）登记成标题栏、最上面 4 个有效像素登记成上边框
    /// （Windows App SDK 的 InputNonClientPointerSource，WinUI 的 SetTitleBar 也是这样做的）。在那里按下由系统处理：
    /// 拖动移动窗口（有贴靠，最大化时拖下来还原）、双击最大化、右键是系统的窗口菜单，上边框拉了改大小。
    /// 不能自己发 WM_NCLBUTTONDOWN 交给系统：WinUI 让整个程序的鼠标输入都变成指针消息（EnableMouseInPointer），
    /// 系统按老办法开始的移动收不到鼠标消息，窗口不动、松开了也停不下来。
    /// 显示第一张图（或空态）以后才登记（不碰冷启动），之后窗口大小、最大化、全屏、"显示标题栏"变了都重新登记。
    /// </summary>
    private void UpdateNonClientRegions()
    {
        if (!_nonClientReady || IsClosed)
        {
            return;
        }
        try
        {
            _nonClient ??= InputNonClientPointerSource.GetForWindowId(AppWindow.Id);
            var chrome = !TitleBarShown && !FullScreen && !WinUiTitleBar && _presenter.State != OverlappedPresenterState.Minimized;
            if (!chrome)
            {
                if (_nonClientApplied is not null)
                {
                    _nonClient.ClearRegionRects(NonClientRegionKind.Caption);
                    _nonClient.ClearRegionRects(NonClientRegionKind.TopBorder);
                    _nonClientApplied = null;
                }
                NonClientState = "没有登记（显示着标题栏、全屏或最小化）";
                return;
            }
            var scale = DpiScale;
            var width = AppWindow.ClientSize.Width;
            var strip = (int)Math.Round(CaptionHeight * scale);
            var band = IsMaximized ? 0 : (int)Math.Round(4 * scale);
            var buttons = (int)Math.Round(3 * 46 * scale);
            // 有菜单栏时那一条就是菜单栏那一行（一样高），各个菜单占的那一段不算：只有它右边的空白拖动窗口。
            var left = MenuBarShown ? Math.Min((int)Math.Ceiling(_menu!.MenusWidth * scale), Math.Max(width - buttons, 0)) : 0;
            var caption = new RectInt32(left, band, Math.Max(width - buttons - left, 0), Math.Max(strip - band, 0));
            var top = new RectInt32(0, 0, band > 0 ? width : 0, band);
            if (_nonClientApplied is var (appliedCaption, appliedTop) && Same(appliedCaption, caption) && Same(appliedTop, top))
            {
                return;
            }
            _nonClient.SetRegionRects(NonClientRegionKind.Caption, new[] { caption });
            if (band > 0)
            {
                _nonClient.SetRegionRects(NonClientRegionKind.TopBorder, new[] { top });
            }
            else
            {
                _nonClient.ClearRegionRects(NonClientRegionKind.TopBorder);
            }
            _nonClientApplied = (caption, top);
            NonClientState = $"已登记（标题栏 {caption.Width}x{caption.Height}，上边框 {top.Width}x{top.Height}，物理像素）";
        }
        catch (Exception error)
        {
            // 登记不了时，按下会交到程序这里，由程序自己移动、改大小（CaptionStripDown、ResizeEdge）。
            // 不当作程序出错（自检里每个异常都算不通过）：记下来，自检的拖动那一项写出来。
            _nonClientApplied = null;
            NonClientState = $"登记出错（由程序自己移动）：0x{error.HResult:X8} {error.Message}";
        }
        static bool Same(RectInt32 a, RectInt32 b) => a.X == b.X && a.Y == b.Y && a.Width == b.Width && a.Height == b.Height;
    }

    /// <summary>图片信息、右上角的通知按标题栏让出位置。</summary>
    private void PlaceTopControls()
    {
        var inset = TopInset;
        Overlays.SetTopInset(inset);
        if (Settings.CornerNotices)
        {
            _notice.HorizontalAlignment = HorizontalAlignment.Right;
            _notice.VerticalAlignment = VerticalAlignment.Top;
            _notice.Margin = new Thickness(0, inset + 12, 16, 0);
        }
        else
        {
            _notice.HorizontalAlignment = HorizontalAlignment.Center;
            _notice.VerticalAlignment = VerticalAlignment.Center;
            _notice.Margin = new Thickness(0);
        }
    }

    /// <summary>标题栏隐藏时在顶端显示文件名。</summary>
    private void RefreshFilename()
    {
        var show = Settings.TitleFilename && CurrentName.Length > 0 && (!TitleBarShown || FullScreen);
        _filename.Text = CurrentName;
        _filename.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
    }

    public void RefreshOverlays()
    {
        Overlays.Refresh(Core, CurrentIndex, CurrentTotal, Lang);
        // 开着的“生成信息”面板跟着这个窗口的当前图片换内容。
        if (GenerationWindow.Current is { } panel && ReferenceEquals(panel.Source, this))
        {
            panel.Refresh();
        }
    }

    // ---- 核心事件 ----

    /// <summary>
    /// 处理核心事件。事件里的图像句柄交给画布（画布换图时释放旧的）；过期（代次不对）的事件不用，马上释放，
    /// 不等垃圾回收（句柄背后是核心里的整张图）。
    /// </summary>
    private void Handle(Event e)
    {
        switch (e)
        {
            case Event.Pong:
                if (_bridge.LastPingMs is double ms)
                {
                    var link = Text(TextKey.CoreLinkOk).Replace("{ms}", ms.ToString("0.00", System.Globalization.CultureInfo.InvariantCulture));
                    _emptyStatus.Text = $"{Text(TextKey.AppName)} {AnyviewCore.CoreVersion()} · {link}";
                }
                OnPong?.Invoke();
                break;
            case Event.Current c:
                Generation = c.Generation;
                // 翻页效果和音效：音效马上放，效果等新图像送达时做。
                if (Core.TransitionFor(c.Generation) is Transition turn)
                {
                    _pendingTurn = (c.Generation, turn);
                    if (turn.Sound)
                    {
                        PlayPageSound();
                    }
                }
                else
                {
                    _pendingTurn = null;
                }
                CurrentName = c.Name;
                CurrentIndex = c.Index;
                CurrentTotal = c.Total;
                UpdateTitle();
                RefreshOverlays();
                break;
            case Event.Collection c:
                if (c.Generation == Generation)
                {
                    CurrentIndex = c.Index;
                    CurrentTotal = c.Total;
                    UpdateTitle();
                    RefreshOverlays();
                }
                // 排序、文件夹变了：选择图片窗口重新载入列表。
                Picker?.Reload();
                break;
            case Event.BackgroundChanged:
                RefreshBackgroundImage();
                break;
            case Event.SettingsReloaded:
                if (OnSettingsReloaded is { } reloaded)
                {
                    reloaded();
                }
                else
                {
                    ApplyUiSettings(Core.UiLang(Program.SystemLang));
                    OnDisplayChanged?.Invoke();
                }
                break;
            case Event.ImageReady r:
                if (r.Generation != Generation)
                {
                    r.Image.Dispose();
                    break;
                }
                StartTurn(r.Generation);
                FollowImage(r.Image.Width(), r.Image.Height());
                _everShown = true;
                Canvas.Show(r.Image);
                LaunchClock.Mark("imageSet");
                _empty.Visibility = Visibility.Collapsed;
                RefreshOverlays();
                ShowIfWaiting();
                OnDisplayChanged?.Invoke();
                OnImageShown?.Invoke(r.Image);
                break;
            case Event.ImagePreview p:
                if (p.Generation != Generation)
                {
                    p.Image.Dispose();
                    break;
                }
                StartTurn(p.Generation);
                FollowImage(p.Width, p.Height);
                _everShown = true;
                Canvas.ShowPreview(p.Image, p.Width, p.Height);
                LaunchClock.Mark("previewSet");
                _empty.Visibility = Visibility.Collapsed;
                RefreshOverlays();
                ShowIfWaiting();
                // 先出预览的图片之后来的是 ImageUpdated（不是 ImageReady）：显示内容在这里就变了（阻止屏保）。
                OnDisplayChanged?.Invoke();
                OnPreviewShown?.Invoke();
                break;
            case Event.ImageUpdated u:
                // 画布上没有图（如这张已经打不开、清空了）时不换，也不通知别人（句柄这就释放了）。
                if (u.Generation != Generation || !Canvas.HasImage)
                {
                    u.Image.Dispose();
                    break;
                }
                Canvas.Replace(u.Image);
                RefreshOverlays();
                OnImageShown?.Invoke(u.Image);
                break;
            case Event.AnimationFrame f:
                if (f.Generation != Generation || !Canvas.HasImage)
                {
                    f.Image.Dispose();
                    break;
                }
                Canvas.Replace(f.Image);
                Overlays.RefreshAnimation(Core);
                OnAnimationFrame?.Invoke();
                break;
            case Event.ExactReady x:
                if (x.Generation != Generation)
                {
                    x.Image.Dispose();
                    break;
                }
                Canvas.ShowExact(x.Image, x.X, x.Y, x.Signature, x.RenderMs);
                break;
            case Event.Thumbnail t:
                // 选择图片窗口要的缩略图（句柄在那里释放）；窗口已经关了就直接释放。
                if (Picker is { IsClosed: false } picker)
                {
                    picker.ThumbnailArrived(t.Path, t.Image);
                }
                else
                {
                    t.Image?.Dispose();
                }
                break;
            case Event.ImageFailed f:
                if (f.Generation == Generation)
                {
                    Canvas.Show(null);
                    if (!_everShown)
                    {
                        _empty.Visibility = Visibility.Visible;
                    }
                    RefreshOverlays();
                    ShowIfWaiting();
                    ShowNotice(Text(TextKey.NoticeImageFailed).Replace("{message}", f.Message));
                    OnProblem?.Invoke(f.Message);
                }
                break;
            case Event.ImageBlocked b:
                if (b.Generation == Generation)
                {
                    Canvas.Show(null);
                    if (!_everShown)
                    {
                        _empty.Visibility = Visibility.Visible;
                    }
                    RefreshOverlays();
                    ShowIfWaiting();
                    AskLimit(b.Path, b.Generation);
                }
                break;
            case Event.OpenFailed f:
                if (!_everShown)
                {
                    _empty.Visibility = Visibility.Visible;
                }
                ShowIfWaiting();
                ShowNotice(Text(TextKey.NoticeOpenFailed).Replace("{message}", f.Message));
                OnProblem?.Invoke(f.Message);
                break;
            case Event.Notice n:
            {
                // 提示的文字由核心给出。复制、移动完成是正常的结果；其余在自检里算问题。
                var message = AnyviewCore.NoticeText(n.Kind, Lang);
                ShowNotice(message);
                if (n.Kind is NoticeKind.Copied or NoticeKind.Moved)
                {
                    OnNotice?.Invoke(message);
                }
                else
                {
                    OnProblem?.Invoke(n.Kind.GetType().Name);
                }
                break;
            }
            case Event.Boundary b:
                AskBoundary(b.Forward, b.Archive);
                break;
            case Event.Closed c:
                Generation = c.Generation;
                Canvas.Show(null);
                OnDisplayChanged?.Invoke();
                OnImageClosed?.Invoke();
                _empty.Visibility = Visibility.Visible;
                CurrentName = "";
                CurrentIndex = null;
                CurrentTotal = null;
                UpdateTitle();
                RefreshOverlays();
                break;
        }
    }

    /// <summary>空态正显示着（自检用）。</summary>
    public bool EmptyShown => _empty.Visibility == Visibility.Visible && _emptyHint.Text.Length > 0;

    private void UpdateTitle()
    {
        RefreshFilename();
        if (CurrentName.Length == 0)
        {
            Title = Text(TextKey.AppName);
        }
        else if (CurrentIndex is uint i && CurrentTotal is uint t)
        {
            Title = $"{CurrentName} [{i + 1}/{t}]";
        }
        else
        {
            Title = CurrentName;
        }
    }

    private void StartTurn(ulong generation)
    {
        if (_pendingTurn is not { } pending)
        {
            return;
        }
        _pendingTurn = null;
        if (pending.Generation == generation)
        {
            Canvas.BeginTransition(pending.Turn.Effect, pending.Turn.Forward, pending.Turn.DurationMs);
        }
    }

    /// <summary>翻页音效：核心合成的 WAV。自检时只记下来，不出声。</summary>
    private void PlayPageSound()
    {
        if (IsSelfTest)
        {
            SoundLog.Add($"PlaySound（自检不出声，WAV {PageSound.Length} 字节）");
            return;
        }
        SoundLog.Add(PageSound.Play() ? "PlaySound" : "PlaySound 失败");
    }

    // ---- 窗口跟着图片 ----
    // 默认情况下窗口就是图片本身：每切到一张图片窗口都变成图片的大小，屏幕放不下时按比例缩到可用区域以内
    // （大小由核心算）。手动放大缩小、翻转不改变窗口；旋转后窗口跟着转。最大化、全屏时窗口不变，退出后回到当前图片的大小。

    public bool IsMaximized => _presenter.State == OverlappedPresenterState.Maximized && !FullScreen;

    /// <summary>最小化了（看不见）：这时不阻止屏保和显示器睡眠。</summary>
    public bool IsMinimized => Win32.IsMinimized(Hwnd);

    private bool _wasMinimized;

    /// <summary>自检用：不为空时记下每次窗口变化带了哪些标志、当时是不是最小化（看系统到底怎么通知的）。</summary>
    public System.Collections.Generic.List<string>? ChangeLog;

    /// <summary>
    /// 最小化、取回（阻止显示器睡眠只在窗口看得见时生效）：窗口有变化、被激活时都看一眼是不是最小化，变了才通知。
    /// 不能只在 AppWindow.Changed 的 DidPresenterChange 时看：Windows 10 上最小化时有这个标志，
    /// 取回时没有（只有大小、位置、层次的变化和一次激活），原来取回窗口后就不再阻止睡眠了。
    /// </summary>
    private void CheckMinimized()
    {
        var minimized = IsMinimized;
        if (minimized != _wasMinimized)
        {
            _wasMinimized = minimized;
            OnDisplayChanged?.Invoke();
        }
    }

    /// <summary>窗口现在能不能移动、改大小：普通窗口才能；最大化、最小化、全屏时不能。</summary>
    public bool WindowMovable => !FullScreen && _presenter.State == OverlappedPresenterState.Restored;

    /// <summary>每个有效像素几个物理像素（窗口所在的屏幕）。</summary>
    public double DpiScale
    {
        get
        {
            var dpi = Win32.GetDpiForWindow(Hwnd);
            return dpi == 0 ? 1.0 : dpi / 96.0;
        }
    }

    /// <summary>窗口所在屏幕的可用区域（屏幕坐标、物理像素）。</summary>
    private RectInt32 WorkArea => Win32.WorkAreaOfWindow(Hwnd);

    /// <summary>内容区最大能有多大（有效像素）：可用区域去掉标题栏和边框。</summary>
    private (double W, double H) Available()
    {
        var area = WorkArea;
        var outer = AppWindow.Size;
        var client = AppWindow.ClientSize;
        var scale = DpiScale;
        return ((area.Width - Math.Max(outer.Width - client.Width, 0)) / scale - _contentInset.W,
            (area.Height - Math.Max(outer.Height - client.Height, 0)) / scale - _contentInset.H);
    }

    /// <summary>
    /// 系统给的客户区比 XAML 内容区大多少（有效像素）。Windows 10 上标题栏隐藏、内容铺到顶端时，WinUI 在顶端留 1 像素
    /// 给窗口的边线，图片区就比客户区矮 1 像素：100% 的图片放不下、被缩到 99.75%（发虚）。按实测的差补上。
    /// </summary>
    private (double W, double H) _contentInset;

    /// <summary>内容区（画布）现在的大小（有效像素）。</summary>
    public Size ContentSize => new(_grid.ActualWidth, _grid.ActualHeight);

    private void UpdateContentInset()
    {
        if (!WindowMovable)
        {
            return;
        }
        var client = AppWindow.ClientSize;
        if (client.Width <= 0 || client.Height <= 0)
        {
            return;
        }
        var scale = DpiScale;
        // 菜单栏占的那一行也算在里面：图片区比客户区矮这么多。它刚显示、还没排好（高度是 0）时先不算，
        // 排好以后会再来一次。
        var menu = MenuBarShown ? _menu!.Row.ActualHeight : 0;
        var w = Math.Round(client.Width - _grid.ActualWidth * scale);
        var h = Math.Round(client.Height - (_grid.ActualHeight + menu) * scale);
        // 只认很小的差（边线）；窗口刚改了大小、布局还没跟上时的中间值不算。
        if (w < 0 || h < 0 || w > 8 || h > 8)
        {
            return;
        }
        var inset = (w / scale, h / scale + menu);
        if (inset == _contentInset)
        {
            return;
        }
        _contentInset = inset;
        // 差变了（第一次量到，或标题栏改了）：正跟着图片时按图片重新定一次大小。
        if (LastFitSize is Size fit)
        {
            SetClientSize(fit.Width, fit.Height);
        }
    }

    private void FollowImage(uint width, uint height)
    {
        if (!WindowMovable)
        {
            return;
        }
        var (w, h) = Available();
        if (Core.WindowSizeForImage(width, height, w, h) is WindowSize size)
        {
            SetClientSize(size.Width, size.Height);
        }
    }

    /// <summary>窗口回到当前图片（按现在的方向）的大小。</summary>
    private void FollowCurrentImage()
    {
        if (!WindowMovable)
        {
            return;
        }
        var (w, h) = Available();
        if (Core.WindowSizeForView(w, h) is WindowSize size)
        {
            SetClientSize(size.Width, size.Height);
        }
    }

    /// <summary>动作执行后：核心说要变（旋转）时窗口跟着变。</summary>
    public void FollowAfterAction(string id)
    {
        if (IsClosed || !WindowMovable)
        {
            return;
        }
        var (w, h) = Available();
        if (Core.WindowSizeAfterAction(id, w, h) is WindowSize size)
        {
            SetClientSize(size.Width, size.Height);
        }
    }

    /// <summary>
    /// 设置内容区（图片区）大小（有效像素）：窗口中心尽量不动，整个窗口留在可用区域以内。不做动画（翻页要快）。
    /// 一次移动带改大小（按现在窗口外框比客户区多出的部分算）；客户区没对上时（标题栏样式刚改过等）再修正一次。
    /// </summary>
    public void SetClientSize(double width, double height)
    {
        var scale = DpiScale;
        LastFitSize = new Size(width, height);
        var target = new SizeInt32((int)Math.Round((width + _contentInset.W) * scale), (int)Math.Round((height + _contentInset.H) * scale));
        var position = AppWindow.Position;
        var before = AppWindow.Size;
        var client = AppWindow.ClientSize;
        var cx = position.X + before.Width / 2.0;
        var cy = position.Y + before.Height / 2.0;
        var size = new SizeInt32(target.Width + Math.Max(before.Width - client.Width, 0), target.Height + Math.Max(before.Height - client.Height, 0));
        AppWindow.MoveAndResize(Placed(cx, cy, size));
        client = AppWindow.ClientSize;
        if (client.Width > 0 && client.Height > 0 && (client.Width != target.Width || client.Height != target.Height))
        {
            var outer = AppWindow.Size;
            size = new SizeInt32(outer.Width + target.Width - client.Width, outer.Height + target.Height - client.Height);
            AppWindow.MoveAndResize(Placed(cx, cy, size));
        }
    }

    /// <summary>外框为 size、中心尽量在 (cx, cy) 的窗口位置，留在可用区域以内（看不见的拖动边框可以伸出去）。</summary>
    private RectInt32 Placed(double cx, double cy, SizeInt32 size)
    {
        var area = WorkArea;
        var (left, top, right, bottom) = Win32.InvisibleBorders(Hwnd);
        var x = Clamp((int)Math.Round(cx - size.Width / 2.0), area.X - left, area.X + area.Width + right - size.Width);
        var y = Clamp((int)Math.Round(cy - size.Height / 2.0), area.Y - top, area.Y + area.Height + bottom - size.Height);
        return new RectInt32(x, y, size.Width, size.Height);
    }

    private static int Clamp(int value, int low, int high) => high < low ? low : Math.Clamp(value, low, high);

    /// <summary>
    /// 把内容区设成 target（物理像素）。标题栏隐藏、内容铺到标题栏下面时，AppWindow.ResizeClient 仍按普通窗口
    /// 扣掉标题栏的高度，内容区会多出一个标题栏高（Windows 上实测多 31 像素）：按实际的内容区再修正一次。
    /// </summary>
    private void ResizeContent(SizeInt32 target)
    {
        AppWindow.ResizeClient(target);
        var client = AppWindow.ClientSize;
        if (client.Width > 0 && client.Height > 0 && (client.Width != target.Width || client.Height != target.Height))
        {
            var outer = AppWindow.Size;
            AppWindow.Resize(new SizeInt32(outer.Width + target.Width - client.Width, outer.Height + target.Height - client.Height));
        }
    }

    /// <summary>直接启动时的窗口：内容区 900x600 有效像素（屏幕放不下时缩小），在主屏幕的可用区域里居中。</summary>
    public void PlaceAtLaunchDefault()
    {
        var area = Win32.PrimaryWorkArea();
        var scale = DpiScale;
        ResizeContent(new SizeInt32(
            (int)Math.Round(Math.Min(LaunchWidth * scale, area.Width)),
            (int)Math.Round(Math.Min(LaunchHeight * scale, area.Height))));
        var size = AppWindow.Size;
        AppWindow.Move(new PointInt32(area.X + (area.Width - size.Width) / 2, area.Y + (area.Height - size.Height) / 2));
    }

    /// <summary>
    /// 新开的窗口：先按直接启动的大小，中心放在 anchor 中心的右下方 24 有效像素；随后按图片定大小时中心不动。
    /// </summary>
    public void PlaceNear(MainWindow anchor)
    {
        var scale = anchor.DpiScale;
        var position = anchor.AppWindow.Position;
        var size = anchor.AppWindow.Size;
        var cx = position.X + size.Width / 2.0 + 24 * scale;
        var cy = position.Y + size.Height / 2.0 + 24 * scale;
        var area = Win32.WorkAreaAt((int)cx, (int)cy);
        ResizeContent(new SizeInt32(
            (int)Math.Round(Math.Min(LaunchWidth * scale, area.Width)),
            (int)Math.Round(Math.Min(LaunchHeight * scale, area.Height))));
        var outer = AppWindow.Size;
        AppWindow.Move(new PointInt32((int)Math.Round(cx - outer.Width / 2.0), (int)Math.Round(cy - outer.Height / 2.0)));
    }

    /// <summary>记住窗口位置的文件：设置文件旁边的 window.json。</summary>
    private string? PlacementFile => Core.SettingsPath() is string settings && Path.GetDirectoryName(settings) is string dir
        ? Path.Combine(dir, "window.json")
        : null;

    /// <summary>
    /// 带着图片启动时的窗口：放回上次关掉时的位置（外框，物理像素；那块显示器已经不在时不用），上次是最大化的显示后
    /// 再最大化；大小随后跟着图片。还没有记录时约占主屏幕可用区域的 70%，居中。
    /// </summary>
    public void RestorePlacement()
    {
        try
        {
            if (PlacementFile is string file && File.Exists(file))
            {
                using var json = System.Text.Json.JsonDocument.Parse(File.ReadAllText(file));
                var root = json.RootElement;
                var rect = new RectInt32(root.GetProperty("x").GetInt32(), root.GetProperty("y").GetInt32(),
                    root.GetProperty("width").GetInt32(), root.GetProperty("height").GetInt32());
                if (rect.Width >= 100 && rect.Height >= 100 && rect.Width <= 32768 && rect.Height <= 32768
                    && Win32.IsOnScreen(rect.X + rect.Width / 2, rect.Y + rect.Height / 2))
                {
                    AppWindow.MoveAndResize(rect);
                    _restoreMaximized = root.TryGetProperty("maximized", out var maximized) && maximized.ValueKind == System.Text.Json.JsonValueKind.True;
                    return;
                }
            }
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or System.Text.Json.JsonException
            or InvalidOperationException or System.Collections.Generic.KeyNotFoundException or FormatException)
        {
            // 记录坏了：按第一次启动处理。
        }
        var area = Win32.PrimaryWorkArea();
        var width = Math.Max((int)(area.Width * 0.7), 480);
        var height = Math.Max((int)(area.Height * 0.7), 360);
        AppWindow.MoveAndResize(new RectInt32(area.X + (area.Width - width) / 2, area.Y + (area.Height - height) / 2, width, height));
    }

    /// <summary>记下窗口位置：关窗口时调用，记普通状态下的外框和是否最大化。写不进去时不提示，下次按第一次启动处理。</summary>
    public void SavePlacement()
    {
        if (WindowMovable)
        {
            var position = AppWindow.Position;
            var size = AppWindow.Size;
            _restoredRect = new RectInt32(position.X, position.Y, size.Width, size.Height);
        }
        if (_restoredRect is not RectInt32 rect || PlacementFile is not string file)
        {
            return;
        }
        // 最小化的窗口按最小化以前的样子记（最大化后最小化、再从任务栏关掉，下次仍是最大化）。
        var maximized = _presenter.State == OverlappedPresenterState.Minimized ? _wasMaximized : IsMaximized;
        var text = $"{{\"x\": {rect.X}, \"y\": {rect.Y}, \"width\": {rect.Width}, \"height\": {rect.Height}, " +
            $"\"maximized\": {(maximized ? "true" : "false")}}}\n";
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            File.WriteAllText(file, text);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
        }
    }

    /// <summary>把窗口放到最前面（从资源管理器打开时）：最小化了先还原。</summary>
    public void BringToFront()
    {
        if (!FullScreen && _presenter.State == OverlappedPresenterState.Minimized)
        {
            _presenter.Restore();
        }
        ShowNow();
        Activate();
    }

    /// <summary>窗口显示着（自检用）。</summary>
    public bool IsVisible => AppWindow.IsVisible;

    // ---- 选择图片、打印 ----

    /// <summary>打开选择图片窗口；已经打开时放到最前面。</summary>
    public void ShowPicker()
    {
        if (Picker is { IsClosed: false } existing)
        {
            existing.Show();
            return;
        }
        var picker = new PickerWindow(Core, Lang, this);
        picker.OnClosed = () =>
        {
            if (ReferenceEquals(Picker, picker))
            {
                Picker = null;
            }
            if (!IsClosed)
            {
                _host.Focus(FocusState.Programmatic);
            }
        };
        Picker = picker;
        picker.Show();
    }

    private bool _printing;

    /// <summary>
    /// 打印当前图片：原图（按 EXIF 方向，不含反相等效果；双页时是合成的这一组）按纸张等比放到最大、居中，
    /// 横图用横向纸，用系统的打印对话框。自检时只记下来。
    /// </summary>
    public async void PrintImage(string name)
    {
        if (IsSelfTest)
        {
            OnExternal?.Invoke("print", name);
            return;
        }
        if (_printing)
        {
            return;
        }
        byte[]? pixels = null;
        int width = 0, height = 0;
        using (var original = Core.OriginalImage())
        {
            if (original is not null)
            {
                pixels = CanvasView.CopyPixels(original, out width, out height);
            }
        }
        if (pixels is null && Canvas.CurrentHandle is ImageHandle current)
        {
            pixels = CanvasView.CopyPixels(current, out width, out height);
        }
        if (pixels is null)
        {
            return;
        }
        _printing = true;
        try
        {
            var (result, error) = await Printing.PrintAsync(Hwnd, name, pixels, width, height);
            if (result == Printing.Result.Failed && !IsClosed)
            {
                ShowNotice(Text(TextKey.NoticeFileFailed).Replace("{message}", error ?? ""));
            }
        }
        catch (Exception error)
        {
            Crash.Write("MainWindow.PrintImage", error);
        }
        finally
        {
            _printing = false;
        }
    }

    /// <summary>最大化、还原：还原时回到当前图片的大小。</summary>
    public void ToggleMaximize()
    {
        if (FullScreen)
        {
            return;
        }
        if (IsMaximized)
        {
            _presenter.Restore();
        }
        else
        {
            _presenter.Maximize();
        }
    }

    /// <summary>即时全屏：没有动画，窗口去掉标题栏、盖满当前屏幕。窗口不跟着图片变。</summary>
    public void ToggleFullScreen()
    {
        if (FullScreen)
        {
            FullScreen = false;
            AppWindow.SetPresenter(_presenter);
            FollowCurrentImage();
        }
        else
        {
            FullScreen = true;
            AppWindow.SetPresenter(FullScreenPresenter.Create());
        }
        ApplyMenuBar(rebuild: false);
        RefreshChrome();
        PlaceTopControls();
        RefreshFilename();
        RefreshOverlays();
        _host.Focus(FocusState.Programmatic);
    }

    /// <summary>最小化了就取回（自检用）。全屏的窗口最小化时没经过普通窗口的呈现方式，取回也直接让窗口还原。</summary>
    public void RestoreFromMinimized()
    {
        if (!IsMinimized)
        {
            return;
        }
        if (FullScreen)
        {
            Win32.Restore(Hwnd);
        }
        else
        {
            _presenter.Restore();
        }
    }

    /// <summary>最小化：全屏时用的不是普通窗口的呈现方式，直接让窗口最小化（点回来仍是全屏）。</summary>
    public void Minimize()
    {
        if (FullScreen)
        {
            Win32.Minimize(Hwnd);
        }
        else
        {
            _presenter.Minimize();
        }
    }

    // ---- 动作 ----

    /// <summary>执行动作（菜单、按键、手势、控件、自检都走这里）。旋转后窗口跟着图片转。</summary>
    public void Invoke(string id)
    {
        if (IsClosed)
        {
            return;
        }
        Apply(Core.InvokeAction(id));
        // Esc、关闭窗口：Apply 里窗口已经关掉了，不再去量它的大小。
        if (IsClosed)
        {
            return;
        }
        FollowAfterAction(id);
    }

    public void Apply(ActionOutcome outcome)
    {
        switch (outcome)
        {
            case ActionOutcome.ViewChanged v:
                Canvas.RefreshLayout();
                if (v.ShowZoom)
                {
                    ShowZoomHint();
                }
                break;
            case ActionOutcome.OverlayChanged:
                RefreshOverlays();
                break;
            case ActionOutcome.Ui ui:
                OnUiRequest?.Invoke(ui.Request);
                break;
        }
    }

    private void ViewChanged(ViewLayout? layout, bool zoom)
    {
        if (layout is null)
        {
            return;
        }
        Canvas.RefreshLayout();
        if (zoom)
        {
            ShowZoomHint();
        }
    }

    // ---- 通知、倍率提示 ----

    /// <summary>正在显示的通知文字（自检用）。</summary>
    public string? NoticeText => _notice.Visibility == Visibility.Visible ? _noticeText.Text : null;
    public string? ZoomHintText => _zoom.Visibility == Visibility.Visible ? _zoomText.Text : null;
    public string? FilenameText => _filename.Visibility == Visibility.Visible ? _filename.Text : null;

    public void ShowNotice(string message)
    {
        var serial = ++_noticeSerial;
        _noticeText.Text = message;
        _notice.Visibility = Visibility.Visible;
        After(2000, () =>
        {
            if (serial == _noticeSerial)
            {
                _notice.Visibility = Visibility.Collapsed;
            }
        });
    }

    /// <summary>缩放倍率提示：在画面中央显示约 1 秒；设置"不显示缩放信息控制"时不显示。</summary>
    private void ShowZoomHint()
    {
        if (Settings.HideZoomInfo || Core.ViewLayout() is not ViewLayout layout)
        {
            return;
        }
        var serial = ++_zoomSerial;
        _zoomText.Text = AnyviewCore.ZoomText(layout.Zoom);
        _zoom.Visibility = Visibility.Visible;
        After(1000, () =>
        {
            if (serial == _zoomSerial)
            {
                _zoom.Visibility = Visibility.Collapsed;
            }
        });
    }

    // ---- 键盘 ----

    /// <summary>VK_PROCESSKEY（229）：VirtualKey 里没有这个名字。</summary>
    private const VirtualKey VkProcessKey = (VirtualKey)0xE5;

    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        var key = e.Key;
        // 输入法开着时按键可能被它拦下（ProcessKey）：按扫描码找回原来的键。
        if (key == VkProcessKey)
        {
            key = (VirtualKey)Win32.KeyFromScanCode(e.KeyStatus.ScanCode, e.KeyStatus.IsExtendedKey);
        }
        if (HandleKey(key))
        {
            e.Handled = true;
        }
    }

    /// <summary>
    /// 按键交给核心按 Windows 键位解析（docs/ACTIONS.md）；没有绑定的按键交给系统
    /// （菜单键、Shift+F10 由系统转成弹出右键菜单，见 OnContextRequested）。
    /// </summary>
    public bool HandleKey(VirtualKey key)
    {
        if (KeyNames.Name(key) is not string name)
        {
            return false;
        }
        return HandleKeyName(name, KeyNames.Modifiers());
    }

    /// <summary>按键名和修饰键交给核心（自检也用它）。返回是否有对应的动作。</summary>
    public bool HandleKeyName(string name, KeyModifiers modifiers)
    {
        if (Core.ActionForKey(name, modifiers) is not string id)
        {
            return false;
        }
        Invoke(id);
        return true;
    }

    // ---- 鼠标 ----

    private Point? _pendingClick;
    private Point? _dragLast;
    private Point? _middleDown;
    private DateTime _lastLeftDown = DateTime.MinValue;
    private Point _lastLeftPoint;
    private bool _captured;
    /// <summary>自己移动窗口（吸附边缘）：按下时的指针和窗口位置（物理像素，屏幕坐标）。</summary>
    private (Win32.POINT Mouse, PointInt32 Origin)? _moveStart;
    private int _wheelAccumulated;

    private double XFraction(Point p) => p.X / Math.Max(_grid.ActualWidth, 1);

    private void OnPointerPressed(object sender, PointerRoutedEventArgs e)
    {
        var point = e.GetCurrentPoint(_grid);
        var p = point.Position;
        // 拖动记录：按下的这一刻（在设焦点之前记，设焦点慢的话算在程序处理的时间里）。
        if (point.Properties.PointerUpdateKind == PointerUpdateKind.LeftButtonPressed)
        {
            DragProbe.Pressed(point.Timestamp);
        }
        // 已经有焦点时不再设：焦点一变，在用 UI 自动化的程序（远程桌面、输入法、取词工具）会来问一大串，
        // 避免拖动开始瞬间界面线程阻塞导致首拖卡顿。
        if (_host.FocusState == FocusState.Unfocused)
        {
            _host.Focus(FocusState.Pointer);
        }
        switch (point.Properties.PointerUpdateKind)
        {
            case PointerUpdateKind.LeftButtonPressed:
                e.Handled = true;
                if (InCaptionStrip(p))
                {
                    CaptionStripDown(e);
                    break;
                }
                LeftDown(p, e);
                break;
            case PointerUpdateKind.RightButtonPressed:
                // 右键：设置里映射了动作时按下就执行；默认的右键菜单按 Windows 的习惯在松开时弹出
                // （OnContextRequested）。这里不标记已处理，系统才会在松开时发出弹出菜单的请求。
                if (Core.MouseAction(MouseInput.Right) is MouseResult.Action mapped)
                {
                    e.Handled = true;
                    Invoke(mapped.Id);
                }
                break;
            case PointerUpdateKind.MiddleButtonPressed:
                // 中键拖动：任何时候都平移图片；单击中键默认不做事，可以映射。
                e.Handled = true;
                _moveStart = null;
                _dragLast = p;
                _middleDown = p;
                _captured = _grid.CapturePointer(e.Pointer);
                break;
            case PointerUpdateKind.XButton1Pressed:
                e.Handled = true;
                RunMouse(MouseInput.Back, p);
                break;
            case PointerUpdateKind.XButton2Pressed:
                e.Handled = true;
                RunMouse(MouseInput.Forward, p);
                break;
        }
    }

    private void LeftDown(Point p, PointerRoutedEventArgs e)
    {
        _moveStart = null;
        var now = DateTime.UtcNow;
        var doubleClick = (now - _lastLeftDown).TotalMilliseconds <= Win32.GetDoubleClickTime()
            && Math.Abs(p.X - _lastLeftPoint.X) < 4 && Math.Abs(p.Y - _lastLeftPoint.Y) < 4;
        _lastLeftDown = doubleClick ? DateTime.MinValue : now;
        _lastLeftPoint = p;
        // 双击：默认最大化、还原；映射了左键或两侧单击翻页时由核心决定。
        if (doubleClick && Core.LeftDoubleClickAction(XFraction(p)) is string action)
        {
            _dragLast = null;
            _pendingClick = null;
            Invoke(action);
            return;
        }
        _captured = _grid.CapturePointer(e.Pointer);
        // 单击要等松开才能判断：移动超过 4 个有效像素才算拖动。
        if (Core.LeftClickMatters())
        {
            _dragLast = null;
            _pendingClick = p;
            return;
        }
        StartLeftDrag(p, e);
    }

    /// <summary>
    /// 左键拖动："拖着图片走"。图片完整显示、窗口能移动时移动窗口（程序自己移动，见 StartWindowMove；打开"拖动窗口时
    /// 吸附边缘"时边拖边贴）；图片放大到超出窗口，或最大化、全屏时平移图片。
    /// </summary>
    private void StartLeftDrag(Point p, PointerRoutedEventArgs e)
    {
        if (Core.LeftDragMovesWindow(WindowMovable))
        {
            _dragLast = null;
            StartWindowMove("图片", e.Pointer.PointerDeviceType);
            return;
        }
        _dragLast = p;
    }

    private Windows.Graphics.PointInt32? _lastMoveTarget;

    /// <summary>
    /// 程序自己移动窗口：记下按下时的指针和窗口位置，之后指针每动一下窗口跟着动（OnPointerMoved → MoveWindow），松开为止。
    /// 不交给系统（WM_NCLBUTTONDOWN）：WinUI 程序里那样开始的移动收不到鼠标消息。代价是拖图片时没有 Windows 的贴靠。
    /// 鼠标的左键已经松开（界面线程忙、轮到处理这次按下时用户已经松手）就不开始，不然窗口会一直跟着指针走。返回是否开始了。
    /// </summary>
    private bool StartWindowMove(string source, PointerDeviceType device)
    {
        if (device == PointerDeviceType.Mouse && !Win32.LeftButtonDown())
        {
            DragProbe.NotStartedMove(source);
            return false;
        }
        if (!Win32.GetCursorPos(out var mouse))
        {
            return false;
        }
        var origin = AppWindow.Position;
        _moveStart = (mouse, origin);
        _lastMoveTarget = origin;
        DragProbe.ManualStart(Hwnd, source, mouse);
        return true;
    }

    /// <summary>上一次报给核心的显示器（换了显示器才重新取配置文件）。</summary>
    private nint _profileMonitor;

    /// <summary>
    /// "使用监视器的 ICC 设置"：把窗口所在显示器在系统"颜色管理"里设的配置文件报给核心；
    /// 设置关着时核心只记下，打开时按它转换颜色。没有配置文件时报空。只有换了显示器（或 force）时才去取。
    /// </summary>
    public void ReportDisplayProfile(bool force)
    {
        if (IsClosed)
        {
            return;
        }
        var monitor = Win32.MonitorOfWindow(Hwnd);
        if (!force && monitor == _profileMonitor)
        {
            return;
        }
        _profileMonitor = monitor;
        byte[]? icc = null;
        string? path = null;
        try
        {
            path = Win32.MonitorProfilePath(Hwnd);
            SystemProfilePath = path;
            // 自检：环境变量 ANYVIEW_TEST_MONITOR_ICC 指向一个配置文件时用它（与 Linux 的自测相同：给一个线性 Gamma 的
            // "显示器"，核对打开设置后颜色真的按它转换）。系统说的那个照样取、记下。
            if (IsSelfTest && Environment.GetEnvironmentVariable("ANYVIEW_TEST_MONITOR_ICC") is { Length: > 0 } testProfile)
            {
                path = testProfile;
            }
            if (path is not null && File.Exists(path))
            {
                icc = File.ReadAllBytes(path);
            }
        }
        catch (Exception error)
        {
            Crash.Write("MainWindow.ReportDisplayProfile", error);
            // 读失败了：下次移动窗口时再试。
            _profileMonitor = 0;
        }
        DisplayProfileBytes = icc?.Length ?? 0;
        DisplayProfilePath = path;
        Core.SetDisplayProfile(icc);
    }

    /// <summary>报给核心的显示器配置文件有多少字节（自检记录；没有时为 0）。</summary>
    public int DisplayProfileBytes { get; private set; }

    /// <summary>报给核心的那个配置文件的路径（自检记录；自检时可能是环境变量给的）。</summary>
    public string? DisplayProfilePath { get; private set; }

    /// <summary>系统"颜色管理"里给这块显示器设的配置文件的路径（自检记录；取不到时为空）。</summary>
    public string? SystemProfilePath { get; private set; }

    /// <summary>
    /// 拖动窗口要用到的代码先跑一遍（JIT 编译、第一次调用系统接口），窗口不动（移到原处）：免得启动后第一次拖动开头停一下。
    /// 启动后空闲时做。
    /// </summary>
    private void WarmUpWindowMove()
    {
        if (IsClosed || !WindowMovable || _moveStart is not null)
        {
            return;
        }
        try
        {
            var position = AppWindow.Position;
            _ = SnappedPosition(position);
            _ = Win32.LeftButtonDown();
            Win32.GetCursorPos(out _);
            AppWindow.Move(position);
        }
        catch (Exception)
        {
            // 只是预热，出错了不影响拖动。
        }
    }

    /// <summary>自检：鼠标的左键松开以后才轮到程序开始移动窗口时，应不开始（返回是否开始了；万一开始了马上结束）。</summary>
    public bool TryStartMoveForTest()
    {
        var started = StartWindowMove("自检：松开以后才开始", PointerDeviceType.Mouse);
        if (started)
        {
            EndPointer();
        }
        return started;
    }

    private void OnPointerMoved(object sender, PointerRoutedEventArgs e)
    {
        Diag.PointerMoves++;
        var point = e.GetCurrentPoint(_grid);
        var p = point.Position;
        if (!IgnoreRealPointer)
        {
            Overlays.PointerMoved(p.X, p.Y, _grid.ActualWidth, _grid.ActualHeight, Core);
        }
        if (_pendingClick is Point down)
        {
            if (Math.Abs(p.X - down.X) < 4 && Math.Abs(p.Y - down.Y) < 4)
            {
                return;
            }
            _pendingClick = null;
            StartLeftDrag(down, e);
        }
        if (_moveStart is not null)
        {
            // 左键已经松开、却没收到松开的事件：停下，不让窗口一直跟着指针走。
            if (e.Pointer.PointerDeviceType == PointerDeviceType.Mouse && !point.Properties.IsLeftButtonPressed)
            {
                if (_captured)
                {
                    _grid.ReleasePointerCapture(e.Pointer);
                }
                EndPointer();
                return;
            }
            MoveWindow();
            return;
        }
        if (_dragLast is not Point last)
        {
            return;
        }
        _dragLast = p;
        var scale = Canvas.Scale;
        ViewChanged(Core.ViewPanBy((p.X - last.X) * scale, (p.Y - last.Y) * scale), false);
    }

    /// <summary>指针离开窗口：子控件的 PointerExited 也会冒泡上来，指针还在画布范围里时不算。</summary>
    private void OnPointerExited(object sender, PointerRoutedEventArgs e)
    {
        if (IgnoreRealPointer)
        {
            return;
        }
        var p = e.GetCurrentPoint(_grid).Position;
        if (p.X >= 0 && p.Y >= 0 && p.X < _grid.ActualWidth && p.Y < _grid.ActualHeight)
        {
            return;
        }
        Overlays.PointerExited(Core);
    }

    /// <summary>自己移动窗口（吸附边缘）：窗口跟着指针走，离屏幕边缘够近时贴上去。</summary>
    private void MoveWindow()
    {
        if (Win32.GetCursorPos(out var mouse))
        {
            MoveWindowWith(mouse);
        }
    }

    /// <summary>指针（屏幕坐标、物理像素）到了 mouse：窗口跟着走，离屏幕边缘够近时贴上去。</summary>
    private void MoveWindowWith(Win32.POINT mouse)
    {
        if (_moveStart is not { } start)
        {
            return;
        }
        var x = start.Origin.X + mouse.X - start.Mouse.X;
        var y = start.Origin.Y + mouse.Y - start.Mouse.Y;
        var target = SnappedPosition(new PointInt32(x, y)) ?? new PointInt32(x, y);
        // 位置没变（指针在窗口里动了一下又回来、窗口移动后系统补发的指针事件）时不再移动。
        if (_lastMoveTarget is PointInt32 last && last.X == target.X && last.Y == target.Y)
        {
            return;
        }
        _lastMoveTarget = target;
        AppWindow.Move(target);
    }

    /// <summary>用户用系统的方式（拖标题栏）移动完窗口：离屏幕边缘够近就贴上去（设置"拖动窗口时吸附边缘"）。自检也用它。</summary>
    public void FinishUserMove()
    {
        if (IsClosed || !WindowMovable)
        {
            return;
        }
        var position = AppWindow.Position;
        if (SnappedPosition(position) is PointInt32 snapped && (snapped.X != position.X || snapped.Y != position.Y))
        {
            AppWindow.Move(snapped);
        }
    }

    /// <summary>
    /// 自检：模拟按住画面中央拖动窗口 (dx, dy)（物理像素），经过与真的拖动相同的移动代码（程序自己移动，吸附边缘打开时边拖边贴）。
    /// 返回是否移动了（现在拖动不是移动窗口时为 false）。
    /// </summary>
    public bool DragWindowForTest(int dx, int dy)
    {
        if (!Core.LeftDragMovesWindow(WindowMovable))
        {
            return false;
        }
        var position = AppWindow.Position;
        var size = AppWindow.Size;
        var start = new Win32.POINT { X = position.X + size.Width / 2, Y = position.Y + size.Height / 2 };
        _moveStart = (start, position);
        _lastMoveTarget = position;
        const int steps = 8;
        for (var i = 1; i <= steps; i++)
        {
            MoveWindowWith(new Win32.POINT { X = start.X + dx * i / steps, Y = start.Y + dy * i / steps });
        }
        _moveStart = null;
        _lastMoveTarget = null;
        return true;
    }

    /// <summary>
    /// 窗口外框放在 position 时按"吸附边缘"应在的位置；设置关掉时为空。按看得见的窗口（去掉看不见的拖动边框）
    /// 和窗口中心所在屏幕的可用区域算，单位换成有效像素交给核心。
    /// </summary>
    public PointInt32? SnappedPosition(PointInt32 position)
    {
        var size = AppWindow.Size;
        var (left, top, right, bottom) = Win32.InvisibleBorders(Hwnd);
        var visible = new RectInt32(position.X + left, position.Y + top, size.Width - left - right, size.Height - top - bottom);
        var center = new PointInt32(visible.X + visible.Width / 2, visible.Y + visible.Height / 2);
        var area = Win32.WorkAreaAt(center.X, center.Y);
        var s = DpiScale;
        var point = Core.SnapWindow(
            new WindowFrame(visible.X / s, visible.Y / s, visible.Width / s, visible.Height / s),
            new WindowFrame(area.X / s, area.Y / s, area.Width / s, area.Height / s));
        if (point is null)
        {
            return null;
        }
        return new PointInt32((int)Math.Round(point.X * s) - left, (int)Math.Round(point.Y * s) - top);
    }

    private void OnPointerReleased(object sender, PointerRoutedEventArgs e)
    {
        var point = e.GetCurrentPoint(_grid);
        var p = point.Position;
        var kind = point.Properties.PointerUpdateKind;
        if (kind == PointerUpdateKind.LeftButtonReleased && _pendingClick is not null)
        {
            // 没有拖动就松开：单击（映射了左键时执行映射的动作；两侧单击翻页）。
            _pendingClick = null;
            if (Core.LeftClickAction(XFraction(p)) is string action)
            {
                Invoke(action);
            }
        }
        if (kind == PointerUpdateKind.MiddleButtonReleased && _middleDown is Point down)
        {
            _middleDown = null;
            if (Math.Abs(p.X - down.X) < 4 && Math.Abs(p.Y - down.Y) < 4)
            {
                RunMouse(MouseInput.Middle, p);
            }
        }
        if (_captured)
        {
            _grid.ReleasePointerCapture(e.Pointer);
        }
        EndPointer();
    }

    private void EndPointer()
    {
        if (_moveStart is { } start)
        {
            DragProbe.ManualEnd();
            // 真的拖动过：紧接着再按顶端那一条不算双击。
            if (_lastMoveTarget is PointInt32 last && (last.X != start.Origin.X || last.Y != start.Origin.Y))
            {
                _lastStripDown = DateTime.MinValue;
            }
        }
        _captured = false;
        _dragLast = null;
        _moveStart = null;
        _lastMoveTarget = null;
        _pendingClick = null;
    }

    /// <summary>鼠标按键要做的事：设置里映射了就按映射，没有映射时按默认（右键弹出菜单，侧键和滚轮翻页，中键不做事）。</summary>
    public void RunMouse(MouseInput input, Point at)
    {
        switch (Core.MouseAction(input))
        {
            case MouseResult.Action a:
                Invoke(a.Id);
                break;
            case MouseResult.Nothing:
                break;
            default:
                switch (input)
                {
                    case MouseInput.Right:
                        ShowContextMenu(at);
                        break;
                    case MouseInput.Back:
                    case MouseInput.WheelUp:
                        Invoke("prev_image");
                        break;
                    case MouseInput.Forward:
                    case MouseInput.WheelDown:
                        Invoke("next_image");
                        break;
                }
                break;
        }
    }

    /// <summary>滚轮：默认向上为上一张，向下为下一张；按住 Ctrl 时以指针为中心按档位缩放。
    /// 精确式触控板的小步滚动累计满一格（120）才算一次。</summary>
    private void OnWheel(object sender, PointerRoutedEventArgs e)
    {
        var point = e.GetCurrentPoint(_grid);
        if (point.Properties.IsHorizontalMouseWheel)
        {
            return;
        }
        e.Handled = true;
        _wheelAccumulated += point.Properties.MouseWheelDelta;
        var steps = _wheelAccumulated / 120;
        if (steps == 0)
        {
            return;
        }
        _wheelAccumulated -= steps * 120;
        var zoom = (e.KeyModifiers & VirtualKeyModifiers.Control) != 0;
        var scale = Canvas.Scale;
        var p = point.Position;
        for (var i = 0; i < Math.Abs(steps); i++)
        {
            var up = steps > 0;
            if (zoom)
            {
                ViewChanged(Core.ViewZoomStep(up ? 1 : -1, p.X * scale, p.Y * scale), true);
            }
            else
            {
                RunMouse(up ? MouseInput.WheelUp : MouseInput.WheelDown, p);
            }
        }
    }

    // ---- 右键菜单 ----

    /// <summary>右键菜单，由核心的数据生成，每次弹出时重新生成。</summary>
    public MenuFlyout ContextMenu() => MenuBuilder.Build(Core, Core.MenuEntries(MenuKind.Context, Lang), Lang, Invoke);

    public void ShowContextMenu(Point? at = null)
    {
        _pendingClick = null;
        _dragLast = null;
        var menu = ContextMenu();
        OnContextMenu?.Invoke(menu);
        var position = at ?? new Point(_grid.ActualWidth / 2, _grid.ActualHeight / 2);
        menu.ShowAt(_grid, new FlyoutShowOptions { Position = position, ShowMode = FlyoutShowMode.Standard });
    }

    /// <summary>
    /// 系统的"弹出菜单"请求：鼠标右键松开、触摸长按（有位置），键盘的菜单键、Shift+F10（没有位置，在画面中央弹出）。
    /// 设置里把右键映射成了动作时，按下时已经执行过，这里不再弹菜单。悬浮控件上右键不弹（与 macOS 相同）。
    /// </summary>
    private void OnContextRequested(UIElement sender, ContextRequestedEventArgs e)
    {
        e.Handled = true;
        if (e.TryGetPosition(_grid, out var point))
        {
            if (Core.MouseAction(MouseInput.Right) is MouseResult.Default)
            {
                ShowContextMenu(point);
            }
            return;
        }
        ShowContextMenu();
    }

    // ---- 拖放 ----

    private void OnDragOver(object sender, DragEventArgs e)
    {
        if (e.DataView.Contains(StandardDataFormats.StorageItems))
        {
            e.AcceptedOperation = DataPackageOperation.Copy;
        }
    }

    private async void OnDrop(object sender, DragEventArgs e)
    {
        if (!e.DataView.Contains(StandardDataFormats.StorageItems))
        {
            return;
        }
        try
        {
            var items = await e.DataView.GetStorageItemsAsync();
            if (items.Count > 0 && !string.IsNullOrEmpty(items[0].Path))
            {
                _bridge.Open(items[0].Path);
            }
        }
        catch (Exception error)
        {
            ShowNotice(Text(TextKey.NoticeOpenFailed).Replace("{message}", error.Message));
        }
    }

    // ---- 对话框 ----

    private bool _dialogOpen;

    /// <summary>
    /// 核心给出的询问：第一个按钮是默认按钮（回车），最后一个是取消（Esc）。
    /// 返回按下的按钮序号；对话框已经开着时返回 null（不叠加）。
    /// </summary>
    public Task<int?> AskAsync(Prompt prompt) => AskAsync(prompt, ContentDialogButton.Primary);

    /// <summary>同上，指定默认按钮（回车）：Close 是最后那个"取消"。</summary>
    public async Task<int?> AskAsync(Prompt prompt, ContentDialogButton defaultButton)
    {
        if (_dialogOpen || Content.XamlRoot is null || prompt.Buttons.Length == 0)
        {
            return null;
        }
        _dialogOpen = true;
        try
        {
            var buttons = prompt.Buttons;
            var dialog = new ContentDialog
            {
                XamlRoot = Content.XamlRoot,
                Content = new TextBlock { Text = prompt.Message, TextWrapping = TextWrapping.Wrap },
                PrimaryButtonText = buttons[0],
                CloseButtonText = buttons.Length > 1 ? buttons[^1] : "",
                SecondaryButtonText = buttons.Length > 2 ? buttons[1] : "",
                DefaultButton = defaultButton,
                RequestedTheme = ElementTheme.Dark,
                Language = UiLanguage.Code(Lang),
            };
            int? chosen = null;
            if (OnDialogOpened is { } opened)
            {
                dialog.Opened += (_, _) => opened(prompt, defaultButton, index =>
                {
                    chosen = index;
                    dialog.Hide();
                });
            }
            var result = await dialog.ShowAsync();
            if (chosen is int index)
            {
                return index;
            }
            return result switch
            {
                ContentDialogResult.Primary => 0,
                ContentDialogResult.Secondary => 1,
                _ => buttons.Length - 1,
            };
        }
        finally
        {
            _dialogOpen = false;
            _host.Focus(FocusState.Programmatic);
        }
    }

    /// <summary>窗口的内容装进界面（有了 XamlRoot，对话框才弹得出来）以后完成；已经装好就马上完成。</summary>
    private Task WhenContentLoadedAsync()
    {
        if (Content is not FrameworkElement root || root.XamlRoot is not null)
        {
            return Task.CompletedTask;
        }
        var loaded = new TaskCompletionSource();
        void OnLoaded(object sender, RoutedEventArgs e)
        {
            root.Loaded -= OnLoaded;
            loaded.TrySetResult();
        }
        root.Loaded += OnLoaded;
        return loaded.Task;
    }

    /// <summary>到达首尾时询问：转到首张（末张）、转到下一个（上一个）文件夹或压缩包，或取消。</summary>
    private async void AskBoundary(bool forward, bool archive)
    {
        if (OnBoundary?.Invoke(forward) == true)
        {
            return;
        }
        var prompt = AnyviewCore.BoundaryPrompt(forward, archive, Lang);
        var answer = await AskAsync(prompt);
        if (answer == 0)
        {
            Invoke(forward ? "first_image" : "last_image");
        }
        else if (answer == 1 && prompt.Buttons.Length > 2)
        {
            Invoke(forward ? "next_container" : "prev_container");
        }
    }

    /// <summary>
    /// 图片被安全上限拦下了：问用户要不要仍然打开。自检看着的窗口照样弹对话框，由自检的下一步回答
    /// （OnDialogOpened）；自检中开的别的窗口不问，当作取消。
    /// </summary>
    private async void AskLimit(string path, ulong generation)
    {
        try
        {
            if (IsSelfTest && OnDialogOpened is null)
            {
                Core.KeepLimits(path);
                return;
            }
            // 启动时打开的就是这张图时，窗口的内容还没装进界面，对话框弹不出来：等装好了再问。 在部分 Windows 环境下
            // Windows 10 上用真的对话框核对时查出：直接打开超限的文件只看到空窗口，没有询问（当成了"取消"）。
            await WhenContentLoadedAsync();
            if (IsClosed || generation != Generation)
            {
                return;
            }
            // 默认按钮是"取消"：回车、Esc 都不打开。
            var answer = await AskAsync(AnyviewCore.LimitPrompt(Lang), ContentDialogButton.Close);
            if (answer == 0)
            {
                Core.OpenWithoutLimits(path);
            }
            else if (answer is not null)
            {
                Core.KeepLimits(path);
            }
            // 没问成（已经有别的对话框开着）：不算取消，下次翻到这张图时再问。
        }
        catch (Exception error)
        {
            Crash.Write("AskLimit", error);
        }
    }

    // ---- 文件操作 ----

    /// <summary>
    /// 移到回收站：先确认（设置"删除文件时确认"），成功后告诉核心，由核心显示下一张。
    /// 文件进不了回收站、会被直接删掉时系统先警告（见 Win32.MoveToRecycleBin），取消了就什么也不做。
    /// 自检时不进回收站，直接删掉（自检用的是临时目录里的副本）。
    /// </summary>
    public async void Trash(string path, string name, bool confirm)
    {
        if (!IsSelfTest && confirm)
        {
            var answer = await AskAsync(AnyviewCore.TrashPrompt(Platform.Windows, Lang, name));
            if (answer != 0)
            {
                return;
            }
        }
        Win32.TrashResult result;
        string? error = null;
        try
        {
            if (IsSelfTest)
            {
                File.Delete(path);
                result = Win32.TrashResult.Done;
            }
            else
            {
                (result, error) = Win32.MoveToRecycleBin(path, Hwnd);
            }
        }
        catch (Exception e)
        {
            result = Win32.TrashResult.Failed;
            error = e.Message;
        }
        if (result == Win32.TrashResult.Cancelled)
        {
            return;
        }
        if (result == Win32.TrashResult.Done && !File.Exists(path))
        {
            Core.Dispatch(new Command.Removed(path));
        }
        else
        {
            ShowNotice(Text(TextKey.NoticeTrashFailed).Replace("{message}", error ?? path));
        }
    }

    /// <summary>
    /// 复制到剪贴板：原图（按 EXIF 方向，不含反相等效果）加文件；粘贴到资源管理器得到文件，粘贴到编辑器得到图像。
    /// 图像编成 PNG（在后台编码，不受显卡纹理大小的限制）。原图还没解码好时不复制。
    /// 返回放进剪贴板的格式（自检核对；自检时不动剪贴板），没复制成时为空。
    /// </summary>
    public async Task<System.Collections.Generic.IReadOnlyList<string>?> CopyImage(string path)
    {
        // 像素先复制出来，核心给的句柄马上释放。
        byte[]? pixels;
        int width, height;
        using (var original = Core.OriginalImage())
        {
            if (original is null)
            {
                return null;
            }
            pixels = CanvasView.CopyPixels(original, out width, out height);
        }
        if (pixels is null)
        {
            return null;
        }
        try
        {
            var package = new DataPackage { RequestedOperation = DataPackageOperation.Copy };
            var stream = new InMemoryRandomAccessStream();
            var encoder = await BitmapEncoder.CreateAsync(BitmapEncoder.PngEncoderId, stream);
            encoder.SetPixelData(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied, (uint)width, (uint)height, 96, 96, pixels);
            await encoder.FlushAsync();
            stream.Seek(0);
            package.SetBitmap(RandomAccessStreamReference.CreateFromStream(stream));
            var file = await StorageFile.GetFileFromPathAsync(path);
            package.SetStorageItems(new IStorageItem[] { file });
            var formats = new System.Collections.Generic.List<string>(package.GetView().AvailableFormats);
            if (!IsSelfTest)
            {
                Clipboard.SetContent(package);
                Clipboard.Flush();
            }
            ShowNotice(Text(TextKey.NoticeCopied));
            return formats;
        }
        catch (Exception error)
        {
            ShowNotice(Text(TextKey.NoticeFileFailed).Replace("{message}", error.Message));
            return null;
        }
    }

    /// <summary>用图像编辑器打开：用设置里的程序打开这个文件；没有设置时提示去设置。</summary>
    public void OpenInEditor(string path, string editor)
    {
        if (editor.Length == 0)
        {
            ShowNotice(Text(TextKey.NoticeEditorMissing));
            OnExternal?.Invoke("editor", "");
            return;
        }
        if (IsSelfTest)
        {
            OnExternal?.Invoke("editor", $"{editor} {path}");
            return;
        }
        try
        {
            var start = new ProcessStartInfo(editor) { UseShellExecute = false };
            start.ArgumentList.Add(path);
            Process.Start(start)?.Dispose();
        }
        catch (Exception error)
        {
            ShowNotice(Text(TextKey.NoticeEditorFailed).Replace("{message}", error.Message));
        }
    }

    /// <summary>设为桌面背景。</summary>
    public void SetWallpaper(string path)
    {
        if (IsSelfTest)
        {
            OnExternal?.Invoke("wallpaper", path);
            return;
        }
        ShowNotice(Win32.SetWallpaper(path)
            ? Text(TextKey.NoticeWallpaperSet)
            : Text(TextKey.NoticeFileFailed).Replace("{message}", path));
    }

    /// <summary>在资源管理器中显示：打开所在文件夹并选中文件。</summary>
    public void Reveal(string path)
    {
        if (IsSelfTest)
        {
            OnExternal?.Invoke("reveal", path);
            return;
        }
        try
        {
            // 资源管理器只认 /select,"路径" 这种写法（整个参数加引号时认不出）。
            Process.Start(new ProcessStartInfo("explorer.exe", $"/select,\"{path}\"") { UseShellExecute = false })?.Dispose();
        }
        catch (Exception error)
        {
            ShowNotice(Text(TextKey.NoticeFileFailed).Replace("{message}", error.Message));
        }
    }

    // ---- 工具 ----

    public void After(uint ms, Action action)
    {
        Delay.After(DispatcherQueue, ms, action);
    }

    /// <summary>窗口内容区的实际大小（有效像素，自检用）。</summary>
    public Size ClientSizeDips
    {
        get
        {
            var s = AppWindow.ClientSize;
            var scale = DpiScale;
            return new Size(s.Width / scale, s.Height / scale);
        }
    }

    public bool HasImage => Core.CurrentPath() is not null;
}
