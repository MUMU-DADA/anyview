// 悬浮界面（docs/MENUS.md）：底部控制栏、左右切图箭头、左上角图片信息、动画信息控制。
// 显示规则（指针区域、时间）由核心给出，这里只负责 WinUI 控件和淡入淡出。
// 图标用系统的 Segoe Fluent Icons（Windows 11）或 Segoe MDL2 Assets（Windows 10），采用平台原生风格。
using System;
using System.Collections.Generic;
using Anyview.Core;
using Microsoft.UI;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Shapes;
using Windows.ApplicationModel.DataTransfer;
using Windows.Foundation;
using Windows.UI;

namespace Anyview;

/// <summary>淡入淡出。隐藏后设为 Collapsed，不再接收鼠标。</summary>
internal sealed class Fader
{
    private readonly UIElement _element;
    private readonly OverlayTiming _timing = AnyviewCore.OverlayTiming();
    private Storyboard? _story;
    public bool IsShown { get; private set; }
    /// <summary>显示状态变了（开始淡入或淡出时）。</summary>
    public Action? OnChanged;

    public Fader(UIElement element)
    {
        _element = element;
        element.Opacity = 0;
        element.Visibility = Visibility.Collapsed;
    }

    public void Show()
    {
        if (IsShown)
        {
            return;
        }
        IsShown = true;
        _element.Visibility = Visibility.Visible;
        Animate(1, _timing.FadeInMs, null);
        OnChanged?.Invoke();
    }

    public void Hide()
    {
        if (!IsShown)
        {
            return;
        }
        IsShown = false;
        Animate(0, _timing.FadeOutMs, () =>
        {
            if (!IsShown)
            {
                _element.Visibility = Visibility.Collapsed;
            }
        });
        OnChanged?.Invoke();
    }

    private void Animate(double to, uint ms, Action? done)
    {
        _story?.Stop();
        var animation = new DoubleAnimation { To = to, Duration = TimeSpan.FromMilliseconds(ms) };
        Storyboard.SetTarget(animation, _element);
        Storyboard.SetTargetProperty(animation, "Opacity");
        var story = new Storyboard();
        story.Children.Add(animation);
        story.Completed += (_, _) =>
        {
            _element.Opacity = to;
            done?.Invoke();
        };
        _story = story;
        story.Begin();
    }
}

/// <summary>控制栏上的进度条：悬停显示指针处的"序号/总数"；单击或拖动，松开后才跳转。</summary>
internal sealed partial class ProgressStrip : Grid
{
    private const double Margin8 = 8;
    private readonly Ellipse _knob;
    public uint? Index;
    public uint? Total;
    public uint? Pending { get; private set; }
    public Action<uint>? OnJump;
    /// <summary>悬停提示：文字和指针在本控件中的 x；文字为空表示隐藏。</summary>
    public Action<string?, double>? OnHover;
    public string ScanningText = "";
    public bool ShowsTip = true;
    private bool _pressed;

    public ProgressStrip()
    {
        Width = 220;
        Height = 16;
        // 几乎透明的底色：整条都能点到，不只是细细的轨道。
        Background = new SolidColorBrush(Color.FromArgb(1, 0, 0, 0));
        Children.Add(new Rectangle
        {
            Height = 4,
            RadiusX = 2,
            RadiusY = 2,
            Margin = new Thickness(Margin8, 0, Margin8, 0),
            VerticalAlignment = VerticalAlignment.Center,
            Fill = new SolidColorBrush(Color.FromArgb(64, 255, 255, 255)),
        });
        _knob = new Ellipse
        {
            Width = 12,
            Height = 12,
            Fill = new SolidColorBrush(Color.FromArgb(255, 242, 242, 242)),
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Center,
        };
        Children.Add(_knob);
        PointerMoved += (_, e) =>
        {
            var x = e.GetCurrentPoint(this).Position.X;
            if (_pressed)
            {
                UpdatePending(x);
            }
            OnHover?.Invoke(ShowsTip ? HoverText(Fraction(x)) : null, x);
        };
        PointerExited += (_, _) => OnHover?.Invoke(null, 0);
        PointerPressed += (_, e) =>
        {
            if (!e.GetCurrentPoint(this).Properties.IsLeftButtonPressed)
            {
                return;
            }
            _pressed = CapturePointer(e.Pointer);
            UpdatePending(e.GetCurrentPoint(this).Position.X);
            e.Handled = true;
        };
        PointerReleased += (_, e) =>
        {
            if (!_pressed)
            {
                return;
            }
            _pressed = false;
            ReleasePointerCapture(e.Pointer);
            if (Pending is uint target)
            {
                Pending = null;
                OnJump?.Invoke(target);
            }
            Refresh();
            e.Handled = true;
        };
        // 拖动中失去了指针（如窗口被切走）：不跳转，滑块回到原位。
        PointerCaptureLost += (_, _) =>
        {
            if (!_pressed)
            {
                return;
            }
            _pressed = false;
            Pending = null;
            Refresh();
        };
    }

    /// <summary>滑块现在的位置（0–1），自检用。</summary>
    public double? KnobFraction => Total is uint total && (Pending ?? Index) is uint index
        ? AnyviewCore.ProgressFraction(index, total)
        : null;

    public void Refresh()
    {
        if (KnobFraction is double f)
        {
            _knob.Visibility = Visibility.Visible;
            _knob.Margin = new Thickness(Margin8 + f * (Width - 2 * Margin8) - 6, 0, 0, 0);
        }
        else
        {
            _knob.Visibility = Visibility.Collapsed;
        }
    }

    private double Fraction(double x) => Math.Clamp((x - Margin8) / Math.Max(Width - 2 * Margin8, 1), 0, 1);

    private string HoverText(double fraction)
    {
        if (Total is not uint total || total == 0)
        {
            return ScanningText;
        }
        var index = Pending ?? AnyviewCore.ProgressIndex(fraction, total);
        return index is uint i ? AnyviewCore.PositionText(i, total) : ScanningText;
    }

    private void UpdatePending(double x)
    {
        if (Total is not uint total)
        {
            return;
        }
        Pending = AnyviewCore.ProgressIndex(Fraction(x), total);
        Refresh();
        OnHover?.Invoke(ShowsTip ? HoverText(Fraction(x)) : null, x);
    }

    /// <summary>与松开鼠标时相同的跳转（自检用）。</summary>
    public bool JumpTo(double fraction)
    {
        if (Total is not uint total || AnyviewCore.ProgressIndex(fraction, total) is not uint target)
        {
            return false;
        }
        OnJump?.Invoke(target);
        return true;
    }
}

/// <summary>白字深色描边的文字：深色的字在八个方向各偏移一点，上面盖白字。</summary>
internal sealed partial class OutlinedText : Grid
{
    private readonly List<TextBlock> _blocks = new();

    public OutlinedText()
    {
        foreach (var (dx, dy) in new (int, int)[] { (-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1), (0, 0) })
        {
            var block = new TextBlock
            {
                TextWrapping = TextWrapping.Wrap,
                MaxWidth = 440,
                Foreground = new SolidColorBrush(dx == 0 && dy == 0 ? Colors.White : Color.FromArgb(230, 0, 0, 0)),
                RenderTransform = new TranslateTransform { X = dx, Y = dy },
                IsTextSelectionEnabled = false,
            };
            _blocks.Add(block);
            Children.Add(block);
        }
        IsHitTestVisible = false;
    }

    public string Text
    {
        get => _blocks[^1].Text;
        set
        {
            foreach (var b in _blocks)
            {
                b.Text = value;
            }
        }
    }

    public double Size
    {
        set
        {
            foreach (var b in _blocks)
            {
                b.FontSize = value;
            }
        }
    }
}

internal sealed class Overlays
{
    public const string IconFont = "Segoe Fluent Icons,Segoe MDL2 Assets";
    public readonly Grid Root = new();
    public readonly Border Bar;
    public readonly Fader BarFade;
    public readonly ProgressStrip Strip = new();
    public readonly Button Left;
    public readonly Button Right;
    public readonly Fader LeftFade;
    public readonly Fader RightFade;
    public readonly StackPanel Info;
    public readonly OutlinedText InfoText = new();
    public readonly Border Tip;
    private readonly TextBlock _tipText = new() { FontSize = 12, Foreground = new SolidColorBrush(Colors.White) };
    private readonly Button _open;
    private readonly Button _prev;
    private readonly Button _next;
    private readonly Button _lock;
    private readonly Ellipse _lockDot;
    private readonly TextBlock _lockText = new() { FontSize = 11, VerticalAlignment = VerticalAlignment.Center };
    private readonly Button _infoClose;
    private readonly Button _infoCopy;
    public readonly Border Anim;
    private readonly Button _animButton;
    private readonly FontIcon _animIcon;
    public readonly TextBlock AnimLabel = new() { FontSize = 12, VerticalAlignment = VerticalAlignment.Center };
    private readonly List<(Button Button, string Id)> _buttons = new();
    private readonly OverlayTiming _timing = AnyviewCore.OverlayTiming();
    private readonly DispatcherQueue _queue;
    private HoverZone _zone = HoverZone.None;
    private bool _pointerOnBar;
    private int _barHideSerial;
    public bool BarHidePending { get; private set; }
    private bool _arrowTimerPending;
    private long _arrowIdleAt;
    public bool BarHidden { get; private set; }
    private bool _arrowsHidden;
    private bool _animHidden;
    /// <summary>执行动作。</summary>
    public Action<string>? OnAction;

    public Overlays(DispatcherQueue queue)
    {
        _queue = queue;
        Root.Background = null;
        // 悬浮控件上的按下不再传给下面的画面（不会拖动窗口、翻页、弹出右键菜单），与 macOS 相同。
        Root.PointerPressed += (_, e) => e.Handled = true;
        Root.ContextRequested += (_, e) => e.Handled = true;

        // 图标的颜色跟着按钮的前景色：按钮不可用时系统按主题把前景变暗。
        static FontIcon Icon(string glyph, double size, bool mirror = false) => new()
        {
            Glyph = glyph,
            FontSize = size,
            FontFamily = new FontFamily(IconFont),
            RenderTransformOrigin = new Point(0.5, 0.5),
            RenderTransform = mirror ? new ScaleTransform { ScaleX = -1 } : null,
        };
        static Button MakeButton(UIElement content, double? round = null)
        {
            var b = new Button
            {
                Content = content,
                Foreground = new SolidColorBrush(Color.FromArgb(255, 235, 235, 235)),
                Background = new SolidColorBrush(Colors.Transparent),
                BorderThickness = new Thickness(round is null ? 0 : 1),
                BorderBrush = new SolidColorBrush(Color.FromArgb(72, 255, 255, 255)),
                Padding = new Thickness(round is null ? 6 : 0),
                VerticalAlignment = VerticalAlignment.Center,
                // 键盘留给画面（按键交给核心）：点按钮不拿走焦点，空格、回车不会再按一次按钮。
                IsTabStop = false,
                AllowFocusOnInteraction = false,
            };
            if (round is double r)
            {
                b.Width = r;
                b.Height = r;
                b.CornerRadius = new CornerRadius(r / 2);
            }
            return b;
        }
        static Rectangle Separator() => new()
        {
            Width = 1,
            Height = 14,
            Fill = new SolidColorBrush(Color.FromArgb(56, 255, 255, 255)),
            VerticalAlignment = VerticalAlignment.Center,
        };
        // 右键执行另一个动作（翻页按钮、边缘箭头：右键翻 10 张）。按下时就执行，与 macOS 相同。
        void OnRightPress(Button button, string id)
        {
            button.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler((_, e) =>
            {
                if (e.GetCurrentPoint(button).Properties.PointerUpdateKind == PointerUpdateKind.RightButtonPressed)
                {
                    e.Handled = true;
                    OnAction?.Invoke(id);
                }
            }), true);
        }

        // 控制栏：左边两个大按钮，中间进度条和一排小按钮，右边两个大按钮（MENUS"底部控制栏"）。
        _open = MakeButton(Icon("\uE8E5", 15), 40);
        var select = MakeButton(Icon("\uE8FD", 15), 40);
        var zoomIn = MakeButton(Icon("\uE8A3", 12));
        var zoomOut = MakeButton(Icon("\uE71F", 12));
        var rotateCcw = MakeButton(Icon("\uE7AD", 12, mirror: true));
        var rotateCw = MakeButton(Icon("\uE7AD", 12));
        _lockDot = new Ellipse { Width = 8, Height = 8, Margin = new Thickness(4, 0, 0, 0), VerticalAlignment = VerticalAlignment.Center };
        var lockRow = new StackPanel { Orientation = Orientation.Horizontal };
        lockRow.Children.Add(_lockText);
        lockRow.Children.Add(_lockDot);
        _lock = MakeButton(lockRow);
        _prev = MakeButton(Icon("\uE76B", 15), 40);
        _next = MakeButton(Icon("\uE76C", 15), 40);
        var tools = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, HorizontalAlignment = HorizontalAlignment.Center };
        foreach (var item in new UIElement[] { zoomIn, zoomOut, Separator(), rotateCcw, rotateCw, Separator(), _lock })
        {
            tools.Children.Add(item);
        }
        var middle = new StackPanel { Orientation = Orientation.Vertical, Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        middle.Children.Add(Strip);
        middle.Children.Add(tools);
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Padding = new Thickness(12, 8, 12, 8) };
        foreach (var item in new UIElement[] { _open, select, middle, _prev, _next })
        {
            row.Children.Add(item);
        }
        Bar = new Border
        {
            Child = row,
            CornerRadius = new CornerRadius(28),
            Background = new SolidColorBrush(Color.FromArgb(200, 30, 30, 30)),
            HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Bottom,
            Margin = new Thickness(0, 0, 0, 16),
        };
        BarFade = new Fader(Bar);
        _buttons.Add((select, "select_image"));
        _buttons.Add((zoomIn, "zoom_in"));
        _buttons.Add((zoomOut, "zoom_out"));
        _buttons.Add((rotateCcw, "rotate_ccw"));
        _buttons.Add((rotateCw, "rotate_cw"));
        _open.Click += (_, _) => OnAction?.Invoke("open_file");
        foreach (var (button, id) in _buttons)
        {
            button.Click += (_, _) => OnAction?.Invoke(id);
        }
        _lock.Click += (_, _) => OnAction?.Invoke("toggle_bar_lock");
        // 翻页按钮：左键 1 张，右键 10 张。
        _prev.Click += (_, _) => OnAction?.Invoke("prev_image");
        _next.Click += (_, _) => OnAction?.Invoke("next_image");
        OnRightPress(_prev, "back_10");
        OnRightPress(_next, "forward_10");

        // 左右切图箭头：浅灰细线 ‹ ›，贴近窗口边缘、垂直居中，没有底板（MENUS"边缘切图箭头"）；右键翻 10 张。
        Button Arrow(string glyph, HorizontalAlignment side)
        {
            var b = MakeButton(Icon(glyph, 34));
            b.HorizontalAlignment = side;
            b.Margin = new Thickness(6);
            b.Foreground = new SolidColorBrush(Color.FromArgb(230, 204, 204, 204));
            return b;
        }
        Left = Arrow("\uE76B", HorizontalAlignment.Left);
        Right = Arrow("\uE76C", HorizontalAlignment.Right);
        Left.Click += (_, _) => OnAction?.Invoke("prev_image");
        Right.Click += (_, _) => OnAction?.Invoke("next_image");
        OnRightPress(Left, "back_10");
        OnRightPress(Right, "forward_10");
        LeftFade = new Fader(Left);
        RightFade = new Fader(Right);

        // 左上角图片信息：关闭、复制两个按钮，下面是白字深色描边的文字。
        _infoClose = MakeButton(Icon("\uE711", 12), 28);
        _infoCopy = MakeButton(Icon("\uE8C8", 12), 28);
        foreach (var b in new[] { _infoClose, _infoCopy })
        {
            b.BorderThickness = new Thickness(2);
            b.BorderBrush = new SolidColorBrush(Color.FromArgb(230, 255, 255, 255));
            b.Foreground = new SolidColorBrush(Colors.White);
        }
        _infoClose.Click += (_, _) => OnAction?.Invoke("toggle_info");
        // 复制按钮复制全部文字，与 macOS 相同，直接放到剪贴板。
        _infoCopy.Click += (_, _) => CopyInfo();
        var infoButtons = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        infoButtons.Children.Add(_infoClose);
        infoButtons.Children.Add(_infoCopy);
        Info = new StackPanel
        {
            Spacing = 10,
            Margin = new Thickness(16),
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Top,
            Visibility = Visibility.Collapsed,
        };
        Info.Children.Add(infoButtons);
        Info.Children.Add(InfoText);

        // 动画信息控制：左下角深色圆角底板上一个播放、暂停按钮和帧序号。
        _animIcon = Icon("\uE769", 12);
        _animButton = MakeButton(_animIcon);
        _animButton.Click += (_, _) => OnAction?.Invoke("anim_toggle");
        AnimLabel.Foreground = new SolidColorBrush(Color.FromArgb(255, 235, 235, 235));
        var animRow = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, Margin = new Thickness(4, 2, 12, 2) };
        animRow.Children.Add(_animButton);
        animRow.Children.Add(AnimLabel);
        Anim = new Border
        {
            Child = animRow,
            CornerRadius = new CornerRadius(14),
            Background = new SolidColorBrush(Color.FromArgb(184, 28, 28, 28)),
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Bottom,
            Margin = new Thickness(16, 0, 0, 16),
            Visibility = Visibility.Collapsed,
        };

        // 进度条的悬停提示（"序号/总数"），显示在控制栏上方。
        Tip = new Border
        {
            Child = _tipText,
            Background = new SolidColorBrush(Color.FromArgb(184, 0, 0, 0)),
            Padding = new Thickness(6, 2, 6, 2),
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Bottom,
            Visibility = Visibility.Collapsed,
            IsHitTestVisible = false,
        };
        Strip.OnHover = ShowTip;

        foreach (var element in new UIElement[] { Info, Anim, Left, Right, Bar, Tip })
        {
            Root.Children.Add(element);
        }
        // 控制栏显示、隐藏时，动画信息控制跟着让位。
        BarFade.OnChanged = PlaceAnim;
    }

    /// <summary>进度条的悬停提示：在指针上方、控制栏上面居中显示，不超出窗口。</summary>
    private void ShowTip(string? text, double x)
    {
        if (text is null || !BarFade.IsShown)
        {
            Tip.Visibility = Visibility.Collapsed;
            return;
        }
        _tipText.Text = text;
        Tip.Visibility = Visibility.Visible;
        Tip.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
        var width = Tip.DesiredSize.Width;
        var at = Strip.TransformToVisual(Root).TransformPoint(new Point(x, 0));
        var barTop = Bar.TransformToVisual(Root).TransformPoint(new Point(0, 0)).Y;
        var left = Math.Clamp(at.X - width / 2, 4, Math.Max(4, Root.ActualWidth - width - 4));
        Tip.Margin = new Thickness(left, 0, 0, Math.Max(0, Root.ActualHeight - barTop + 6));
    }

    private void CopyInfo()
    {
        var package = new DataPackage();
        package.SetText(InfoText.Text);
        Clipboard.SetContent(package);
        Clipboard.Flush();
    }

    /// <summary>标题栏隐藏、内容铺到顶端时，图片信息放到标题栏下面（top 为标题栏的高度，有效像素）。</summary>
    public void SetTopInset(double top)
    {
        Info.Margin = new Thickness(16, 16 + top, 16, 16);
    }

    /// <summary>设置生效：悬停提示、图片信息字号、隐藏的控件。</summary>
    public void ApplySettings(AppCore core, Lang lang, UiSettings settings)
    {
        var showTips = !settings.HideTooltips;
        void SetTip(FrameworkElement element, string text) => ToolTipService.SetToolTip(element, showTips ? UiLanguage.Tip(text, lang) : null);
        SetTip(_open, core.ActionTooltip("open_file", lang));
        foreach (var (button, id) in _buttons)
        {
            SetTip(button, core.ActionTooltip(id, lang));
        }
        _lockText.Text = AnyviewCore.UiText(TextKey.BarLock, lang);
        SetTip(_lock, core.ActionTooltip("toggle_bar_lock", lang));
        SetTip(_prev, AnyviewCore.UiText(TextKey.BarPrevTip, lang));
        SetTip(_next, AnyviewCore.UiText(TextKey.BarNextTip, lang));
        SetTip(_infoClose, AnyviewCore.UiText(TextKey.InfoClose, lang));
        SetTip(_infoCopy, AnyviewCore.UiText(TextKey.InfoCopy, lang));
        SetTip(_animButton, core.ActionTooltip("anim_toggle", lang));
        Strip.ScanningText = AnyviewCore.UiText(TextKey.BarScanning, lang);
        Strip.ShowsTip = showTips;
        InfoText.Size = settings.SmallInfoFont ? 12 : 15;
        BarHidden = settings.HideControlBar;
        _arrowsHidden = settings.HideSideArrows;
        _animHidden = settings.HideAnimationInfo;
        if (BarHidden)
        {
            BarFade.Hide();
            Tip.Visibility = Visibility.Collapsed;
        }
        if (_arrowsHidden)
        {
            LeftFade.Hide();
            RightFade.Hide();
        }
    }

    /// <summary>按核心的状态刷新：控制栏锁定、按钮可用、进度条、图片信息（MENUS 各节）。</summary>
    public void Refresh(AppCore core, uint? index, uint? total, Lang lang)
    {
        var locked = core.ActionState("toggle_bar_lock").Checked;
        _lockDot.Fill = new SolidColorBrush(locked ? Color.FromArgb(255, 0, 120, 215) : Color.FromArgb(255, 235, 235, 235));
        _open.IsEnabled = core.ActionState("open_file").Enabled;
        foreach (var (button, id) in _buttons)
        {
            button.IsEnabled = core.ActionState(id).Enabled;
        }
        var hasImage = core.ActionState("next_image").Enabled;
        _prev.IsEnabled = hasImage;
        _next.IsEnabled = hasImage;
        Strip.Index = index;
        Strip.Total = total;
        Strip.Refresh();
        var lines = core.InfoLines(lang);
        InfoText.Text = string.Join("\n", lines);
        Info.Visibility = core.ActionState("toggle_info").Checked && lines.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        if (!hasImage)
        {
            LeftFade.Hide();
            RightFade.Hide();
        }
        UpdateBar(locked);
        RefreshAnimation(core);
    }

    /// <summary>动画信息控制：正在显示动图时出现；控制栏显示时让到它上面。</summary>
    public void RefreshAnimation(AppCore core)
    {
        var info = core.Animation();
        Anim.Visibility = info is null || _animHidden ? Visibility.Collapsed : Visibility.Visible;
        if (info is not null)
        {
            AnimLabel.Text = $"{info.Index + 1} / {(info.Count is uint c ? c.ToString() : "?")}";
            _animIcon.Glyph = info.Playing ? "\uE769" : "\uE768";
        }
        PlaceAnim();
    }

    private void PlaceAnim()
    {
        var bar = Bar.ActualHeight > 0 ? Bar.ActualHeight : 72;
        Anim.Margin = new Thickness(16, 0, 0, BarFade.IsShown ? bar + 24 : 16);
    }

    /// <summary>动画信息控制的文字（自检用）；不显示时为空。</summary>
    public string? AnimationText => Anim.Visibility == Visibility.Visible ? AnimLabel.Text : null;

    /// <summary>控制栏：在控制栏区域、指针在控制栏上或已锁定时显示；否则 1 秒后淡出。</summary>
    private void UpdateBar(bool locked)
    {
        var want = !BarHidden && (locked || _zone == HoverZone.Bar || _pointerOnBar);
        if (want)
        {
            _barHideSerial++;
            BarHidePending = false;
            BarFade.Show();
            return;
        }
        if (!BarFade.IsShown || BarHidePending)
        {
            return;
        }
        var serial = ++_barHideSerial;
        BarHidePending = true;
        After(_timing.BarHideDelayMs, () =>
        {
            if (serial != _barHideSerial)
            {
                return;
            }
            BarHidePending = false;
            BarFade.Hide();
            Tip.Visibility = Visibility.Collapsed;
        });
    }

    /// <summary>指针移动（有效像素，原点在左上）：按核心的区域规则显示控制栏和箭头。</summary>
    public void PointerMoved(double x, double y, double width, double height, AppCore core)
    {
        _zone = AnyviewCore.HoverZone(x, y, width, height);
        // 指针在控制栏上时不隐藏（与 macOS 相同，按位置算）。
        _pointerOnBar = false;
        if (BarFade.IsShown && Bar.ActualWidth > 0)
        {
            var origin = Bar.TransformToVisual(Root).TransformPoint(new Point(0, 0));
            _pointerOnBar = x >= origin.X && y >= origin.Y && x < origin.X + Bar.ActualWidth && y < origin.Y + Bar.ActualHeight;
        }
        var arrows = core.ActionState("next_image").Enabled && !_arrowsHidden;
        if (_zone == HoverZone.Left && arrows)
        {
            LeftFade.Show();
            RightFade.Hide();
        }
        else if (_zone == HoverZone.Right && arrows)
        {
            RightFade.Show();
            LeftFade.Hide();
        }
        else
        {
            LeftFade.Hide();
            RightFade.Hide();
        }
        // 静止 2 秒后箭头淡出；每次移动重新计时。
        _arrowIdleAt = Environment.TickCount64 + _timing.ArrowIdleMs;
        ArmArrowTimer(_timing.ArrowIdleMs);
        UpdateBar(core.ActionState("toggle_bar_lock").Checked);
    }

    /// <summary>
    /// 箭头淡出的计时：同时只有一个计时器在等（以前指针每动一下新建一个，轮询率高的鼠标一秒几百上千个，
    /// 各自等满 2 秒再空跑一次）。到点时指针还在动过的 2 秒以内，就按剩下的时间再等。
    /// </summary>
    private void ArmArrowTimer(uint ms)
    {
        if (_arrowTimerPending)
        {
            return;
        }
        _arrowTimerPending = true;
        After(Math.Max(ms, 1), () =>
        {
            _arrowTimerPending = false;
            var left = _arrowIdleAt - Environment.TickCount64;
            if (left > 0)
            {
                ArmArrowTimer((uint)left);
                return;
            }
            LeftFade.Hide();
            RightFade.Hide();
        });
    }

    public void PointerExited(AppCore core)
    {
        _zone = HoverZone.None;
        _pointerOnBar = false;
        LeftFade.Hide();
        RightFade.Hide();
        UpdateBar(core.ActionState("toggle_bar_lock").Checked);
    }

    private bool _detached;

    /// <summary>窗口关掉了：之后到点的延时都不再做事。</summary>
    public void Detach() => _detached = true;

    private void After(uint ms, Action action)
    {
        Delay.After(_queue, ms, () =>
        {
            if (!_detached)
            {
                action();
            }
        });
    }
}
