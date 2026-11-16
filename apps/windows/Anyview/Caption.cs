// 标题栏隐藏时（"显示标题栏"关着）右上角的最小化、最大化（还原）、关闭按钮，以及顶边改窗口大小的细带子。
// 系统的标题栏由 Win32.SetCaptionless 去掉，这几个由程序自己画：样子与 Windows 自带的相同（透明底、白色图标，悬停变亮，
// 关闭悬停变红；窗口不在前面时图标变灰）。不用 WinUI 的 ExtendsContentIntoTitleBar 保留系统按钮的原因见 Win32.SetCaptionless。
// 按下去执行核心的动作（minimize、maximize、close_window），与菜单、按键相同。
using System;
using System.Threading;
using Anyview.Core;
using Microsoft.UI;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Automation.Provider;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Anyview;

/// <summary>右上角的三个按钮。</summary>
internal sealed class CaptionButtons
{
    public readonly StackPanel Root = new()
    {
        Orientation = Orientation.Horizontal,
        HorizontalAlignment = HorizontalAlignment.Right,
        VerticalAlignment = VerticalAlignment.Top,
    };
    private readonly CaptionButton _minimize;
    private readonly CaptionButton _maximize;
    private readonly CaptionButton _close;

    public CaptionButtons(Action<string> invoke)
    {
        _minimize = new CaptionButton("\uE921", false, () => invoke("minimize"));
        _maximize = new CaptionButton("\uE922", false, () => invoke("maximize"));
        _close = new CaptionButton("\uE8BB", true, () => invoke("close_window"));
        Root.Children.Add(_minimize);
        Root.Children.Add(_maximize);
        Root.Children.Add(_close);
    }

    /// <summary>窗口在前面时图标是白的，不在前面时变灰（与系统的标题栏按钮相同）。</summary>
    public void SetActive(bool active)
    {
        foreach (var button in new[] { _minimize, _maximize, _close })
        {
            button.SetActive(active);
        }
    }

    /// <summary>最大化时中间的按钮是"还原"。</summary>
    public void SetMaximized(bool maximized) => _maximize.SetGlyph(maximized ? "\uE923" : "\uE922");

    /// <summary>悬停提示（核心给的文字，带快捷键）。</summary>
    public void SetTips(AppCore core, Lang lang)
    {
        _minimize.SetTip(core.ActionTooltip("minimize", lang), lang);
        _maximize.SetTip(core.ActionTooltip("maximize", lang), lang);
        _close.SetTip(core.ActionTooltip("close_window", lang), lang);
    }
}

/// <summary>一个标题栏按钮：46x32 有效像素（Windows 10 的标题栏按钮大小），按下、松开都在按钮上才算点了。</summary>
internal sealed partial class CaptionButton : Grid
{
    private static readonly Color Normal = Colors.Transparent;
    private static readonly Color Hover = Color.FromArgb(64, 255, 255, 255);
    private static readonly Color Pressed = Color.FromArgb(96, 255, 255, 255);
    private static readonly Color CloseHover = Color.FromArgb(255, 0xE8, 0x11, 0x23);
    private static readonly Color ClosePressed = Color.FromArgb(255, 0xF1, 0x70, 0x7A);
    private static readonly Color ActiveGlyph = Color.FromArgb(255, 235, 235, 235);
    private static readonly Color InactiveGlyph = Color.FromArgb(255, 170, 170, 170);

    private readonly FontIcon _icon;
    private readonly bool _isClose;
    private readonly Action _click;
    private readonly SolidColorBrush _background = new(Colors.Transparent);
    private readonly SolidColorBrush _glyph = new(ActiveGlyph);
    private bool _over;
    private bool _pressed;
    private bool _active = true;

    public CaptionButton(string glyph, bool isClose, Action click)
    {
        _isClose = isClose;
        _click = click;
        Width = 46;
        Height = 32;
        Background = _background;
        IsTabStop = false;
        _icon = new FontIcon
        {
            Glyph = glyph,
            FontFamily = new FontFamily("Segoe MDL2 Assets"),
            FontSize = 10,
            Foreground = _glyph,
            HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center,
        };
        Children.Add(_icon);
        PointerEntered += (_, _) =>
        {
            _over = true;
            Refresh();
        };
        PointerExited += (_, _) =>
        {
            _over = false;
            Refresh();
        };
        PointerPressed += (_, e) =>
        {
            // 只认左键；画布、标题栏那一条都不再处理这次按下。
            e.Handled = true;
            if (!e.GetCurrentPoint(this).Properties.IsLeftButtonPressed)
            {
                return;
            }
            _pressed = CapturePointer(e.Pointer);
            Refresh();
        };
        PointerReleased += (_, e) =>
        {
            e.Handled = true;
            var wasPressed = _pressed;
            _pressed = false;
            ReleasePointerCapture(e.Pointer);
            var p = e.GetCurrentPoint(this).Position;
            var inside = p.X >= 0 && p.Y >= 0 && p.X < ActualWidth && p.Y < ActualHeight;
            _over = inside;
            Refresh();
            if (wasPressed && inside)
            {
                click();
            }
        };
        PointerCaptureLost += (_, _) =>
        {
            _pressed = false;
            Refresh();
        };
        // 双击、右键不交给下面的画布（不然双击按钮会最大化窗口、右键弹出看图的菜单）。
        DoubleTapped += (_, e) => e.Handled = true;
        RightTapped += (_, e) => e.Handled = true;
        ContextRequested += (_, e) => e.Handled = true;
    }

    public void SetGlyph(string glyph) => _icon.Glyph = glyph;

    /// <summary>读屏、UI 自动化看到的是"按钮"，能按（与系统的标题栏按钮相同）。</summary>
    protected override AutomationPeer OnCreateAutomationPeer() => new Peer(this);

    private sealed partial class Peer : FrameworkElementAutomationPeer, IInvokeProvider
    {
        private readonly CaptionButton _owner;

        public Peer(CaptionButton owner) : base(owner) => _owner = owner;

        protected override AutomationControlType GetAutomationControlTypeCore() => AutomationControlType.Button;

        protected override string GetClassNameCore() => "Button";

        protected override object? GetPatternCore(PatternInterface patternInterface) =>
            patternInterface == PatternInterface.Invoke ? this : base.GetPatternCore(patternInterface);

        public void Invoke() => _owner._click();
    }

    public void SetActive(bool active)
    {
        _active = active;
        Refresh();
    }

    public void SetTip(string tip, Lang lang)
    {
        ToolTipService.SetToolTip(this, UiLanguage.Tip(tip, lang));
        AutomationProperties.SetName(this, tip);
    }

    private void Refresh()
    {
        _background.Color = _pressed && _over ? (_isClose ? ClosePressed : Pressed)
            : _over || _pressed ? (_isClose ? CloseHover : Hover)
            : Normal;
        _glyph.Color = _isClose && (_over || _pressed) ? Colors.White : _active ? ActiveGlyph : InactiveGlyph;
    }
}

/// <summary>
/// 顶边改窗口大小的细带子：标题栏去掉以后窗口顶上没有系统的边框了。平常最上面那一条登记成了系统的上边框
/// （MainWindow.UpdateNonClientRegions），由系统改大小；登记没生效、按下交到这里时，由程序自己改大小：
/// 按下时记下指针和窗口外框，指针每动一下按顶边（左上角、右上角的小块连左边、右边）跟着改，松开为止，不小于最小尺寸。
/// 不交给系统（WM_NCLBUTTONDOWN）：WinUI 程序里那样开始的改大小收不到鼠标消息。
/// </summary>
internal sealed partial class ResizeEdge : Grid
{
    private const int HTTOPLEFT = 13, HTTOPRIGHT = 14;
    private (Win32.POINT Mouse, DragProbe.RECT Rect, uint Dpi)? _start;
    private static int s_resizing;

    /// <summary>有带子正在由程序改大小（自检等它开始了再动光标）。别的线程也可以读。</summary>
    public static bool Resizing => Volatile.Read(ref s_resizing) != 0;

    private void Begin((Win32.POINT, DragProbe.RECT, uint) start)
    {
        _start = start;
        Volatile.Write(ref s_resizing, 1);
    }

    private void End()
    {
        _start = null;
        Volatile.Write(ref s_resizing, 0);
    }

    /// <param name="hitTest">HTTOP 12、HTTOPLEFT 13、HTTOPRIGHT 14。</param>
    /// <param name="minimum">窗口外框的最小宽、高（物理像素）。</param>
    public ResizeEdge(InputSystemCursorShape cursor, int hitTest, Func<nint> hwnd, Func<(int Width, int Height)> minimum)
    {
        // 几乎透明的底色：没有底色时点不到。
        Background = new SolidColorBrush(Color.FromArgb(1, 0, 0, 0));
        ProtectedCursor = InputSystemCursor.Create(cursor);
        IsTabStop = false;
        PointerPressed += (_, e) =>
        {
            e.Handled = true;
            End();
            if (!e.GetCurrentPoint(this).Properties.IsLeftButtonPressed)
            {
                return;
            }
            // 鼠标的左键已经松开才轮到这里（界面线程忙）：不开始，不然窗口会一直跟着指针改大小。
            if (e.Pointer.PointerDeviceType == PointerDeviceType.Mouse && !Win32.LeftButtonDown())
            {
                return;
            }
            var window = hwnd();
            if (Win32.GetCursorPos(out var mouse) && DragProbe.GetWindowRect(window, out var rect) && CapturePointer(e.Pointer))
            {
                Begin((mouse, rect, Win32.GetDpiForWindow(window)));
            }
        };
        PointerMoved += (_, e) =>
        {
            if (_start is not { } start)
            {
                return;
            }
            e.Handled = true;
            // 左键已经松开却没收到松开的事件，或者窗口换到了缩放比例不同的屏幕上（系统按新的比例改了窗口大小，
            // 再按开始时的外框算就把它改回去了）：停下。
            if ((e.Pointer.PointerDeviceType == PointerDeviceType.Mouse && !e.GetCurrentPoint(this).Properties.IsLeftButtonPressed)
                || Win32.GetDpiForWindow(hwnd()) != start.Dpi)
            {
                End();
                ReleasePointerCapture(e.Pointer);
                return;
            }
            if (!Win32.GetCursorPos(out var mouse))
            {
                return;
            }
            var (minWidth, minHeight) = minimum();
            var r = start.Rect;
            var top = Math.Min(r.Top + mouse.Y - start.Mouse.Y, r.Bottom - minHeight);
            var left = r.Left;
            var right = r.Right;
            if (hitTest == HTTOPLEFT)
            {
                left = Math.Min(r.Left + mouse.X - start.Mouse.X, r.Right - minWidth);
            }
            else if (hitTest == HTTOPRIGHT)
            {
                right = Math.Max(r.Right + mouse.X - start.Mouse.X, r.Left + minWidth);
            }
            Win32.SetWindowRect(hwnd(), left, top, right - left, r.Bottom - top);
        };
        PointerReleased += (_, e) =>
        {
            e.Handled = true;
            if (_start is not null)
            {
                End();
                ReleasePointerCapture(e.Pointer);
            }
        };
        PointerCaptureLost += (_, _) => End();
        DoubleTapped += (_, e) => e.Handled = true;
    }
}
