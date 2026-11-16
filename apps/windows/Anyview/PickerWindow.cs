// 选择图片窗口。
// 上面是缩略图导航，下面是列表（名称、修改日期、大小），底部是排序按钮、"隐藏缩略图导航"和"确定"。
// 单击、方向键只移动选择，主窗口不切换；双击、回车或"确定"切到所选图片并关掉窗口；Esc 或关闭按钮不切换。
// 列表内容、文字和缩略图都由核心给出。列表很长时（一个文件夹几千张）只建看得见的行。
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Threading.Tasks;
using Anyview.Core;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Markup;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Foundation;
using Windows.System;
using Windows.UI;

namespace Anyview;

internal sealed class PickerWindow
{
    public readonly Window Window;
    private readonly AppCore _core;
    private readonly Lang _lang;
    private PickerRow[] _rows = Array.Empty<PickerRow>();
    private PickerSortOption[] _sortOptions = Array.Empty<PickerSortOption>();
    /// <summary>缩略图：路径 → 图像（null 表示生成失败）。</summary>
    private readonly Dictionary<string, ImageSource?> _thumbs = new();
    private readonly Canvas _strip = new() { Height = StripHeight, Background = new SolidColorBrush(Colors.Black) };
    private readonly ListView _list = new() { SelectionMode = ListViewSelectionMode.Single, IsItemClickEnabled = false };
    private readonly Button _sort = new() { MinWidth = 180 };
    private readonly CheckBox _hide = new();
    private readonly Grid _root = new() { Padding = new Thickness(8), RowSpacing = 8 };
    /// <summary>程序自己在改选择（重新载入列表），这时不当作用户操作。</summary>
    private bool _updating;
    /// <summary>缩略图带上现在画着的格子：序号。</summary>
    private readonly List<int> _cells = new();
    /// <summary>现在画出来的格子：(序号, 左边 x, 宽度)。点哪一格按位置找（格子在点了以后会重排）。</summary>
    private readonly List<(int Index, double X, double Width)> _spans = new();
    private DateTime _lastPress = DateTime.MinValue;
    private Point _lastPressPoint;
    /// <summary>上一次单击点到了格子：双击的第二下照样打开那一张（第二下可能落在重排后的别的格子或空白处）。</summary>
    private bool _pressedCell;
    private double _scrollSum;
    public Action? OnClosed;
    public bool IsClosed { get; private set; }

    private const double StripHeight = 112;
    private static readonly Size Small = new(64, 48);
    private static readonly Size Big = new(96, 72);
    private const double Gap = 8;
    private const double FrameWidth = 3;

    public PickerWindow(AppCore core, Lang lang, MainWindow? owner)
    {
        _core = core;
        _lang = lang;
        _root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        _root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        _root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        _root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });

        // 缩略图带：黑底，所选的一张在正中、放大、加橙色框，两边依次排开；单击选中，双击切换，滚轮一格移一张。
        _strip.SizeChanged += (_, _) => RefreshStrip();
        // 单击选中（这一格移到正中），双击打开：在缩略图带上判断（与 macOS 按 clickCount 相同）。不用各格子的
        // Tapped/DoubleTapped：第一下就把格子重排了，被点的那一格已经不在，第二下的双击收不到。
        _strip.PointerPressed += (_, e) =>
        {
            var point = e.GetCurrentPoint(_strip);
            if (!point.Properties.IsLeftButtonPressed)
            {
                return;
            }
            e.Handled = true;
            var p = point.Position;
            var now = DateTime.UtcNow;
            var doubleClick = (now - _lastPress).TotalMilliseconds <= Win32.GetDoubleClickTime()
                && Math.Abs(p.X - _lastPressPoint.X) < 4 && Math.Abs(p.Y - _lastPressPoint.Y) < 4;
            _lastPress = doubleClick ? DateTime.MinValue : now;
            _lastPressPoint = p;
            if (doubleClick)
            {
                // 双击打开第一下选中的那一张（第一下以后格子重排了，第二下落在别的格子或空白处）。
                if (_pressedCell)
                {
                    Choose();
                }
                return;
            }
            _pressedCell = false;
            foreach (var (index, x, width) in _spans)
            {
                if (p.X >= x && p.X < x + width)
                {
                    _pressedCell = true;
                    Select(index);
                    return;
                }
            }
        };
        _strip.PointerWheelChanged += (_, e) =>
        {
            var delta = e.GetCurrentPoint(_strip).Properties.MouseWheelDelta;
            _scrollSum += delta;
            while (Math.Abs(_scrollSum) >= 120)
            {
                Step(_scrollSum > 0 ? -1 : 1);
                _scrollSum -= _scrollSum > 0 ? 120 : -120;
            }
            e.Handled = true;
        };
        _root.Children.Add(_strip);

        // 表头：名称、修改日期、大小。
        var header = new Grid { Padding = new Thickness(12, 0, 16, 0) };
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(160) });
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(90) });
        var titles = new[] { TextKey.PickerColumnName, TextKey.PickerColumnModified, TextKey.PickerColumnSize };
        for (var i = 0; i < titles.Length; i++)
        {
            var title = new TextBlock
            {
                Text = AnyviewCore.UiText(titles[i], lang),
                FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                TextAlignment = i == 2 ? TextAlignment.Right : TextAlignment.Left,
            };
            Grid.SetColumn(title, i);
            header.Children.Add(title);
        }
        Grid.SetRow(header, 1);
        _root.Children.Add(header);

        // 列表：一行三列；只建看得见的行（ContainerContentChanging 时填字）。
        // XamlReader 读出来的东西运行时才知道是什么类型。程序预先编译成本机代码以后不能直接强转成 DataTemplate：
        // 取出它在系统那边的指针，再明说“这是 DataTemplate”。
        var template = XamlReader.Load(
            "<DataTemplate xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\">" +
            "<Grid Height=\"22\"><Grid.ColumnDefinitions><ColumnDefinition Width=\"*\"/><ColumnDefinition Width=\"160\"/>" +
            "<ColumnDefinition Width=\"90\"/></Grid.ColumnDefinitions>" +
            "<TextBlock VerticalAlignment=\"Center\" TextTrimming=\"CharacterEllipsis\"/>" +
            "<TextBlock Grid.Column=\"1\" VerticalAlignment=\"Center\"/>" +
            "<TextBlock Grid.Column=\"2\" VerticalAlignment=\"Center\" TextAlignment=\"Right\"/>" +
            "</Grid></DataTemplate>");
        var templatePointer = WinRT.MarshalInspectable<object>.FromManaged(template);
        try
        {
            _list.ItemTemplate = DataTemplate.FromAbi(templatePointer);
        }
        finally
        {
            System.Runtime.InteropServices.Marshal.Release(templatePointer);
        }
        _list.ContainerContentChanging += (_, args) =>
        {
            if (args.ItemContainer.ContentTemplateRoot is Grid grid && args.ItemIndex >= 0 && args.ItemIndex < _rows.Length
                && grid.Children.Count == 3)
            {
                var row = _rows[args.ItemIndex];
                ((TextBlock)grid.Children[0]).Text = row.Name;
                ((TextBlock)grid.Children[1]).Text = row.Modified;
                ((TextBlock)grid.Children[2]).Text = row.Size;
            }
        };
        _list.SelectionChanged += (_, _) =>
        {
            if (!_updating)
            {
                RefreshStrip();
            }
        };
        _list.DoubleTapped += (_, _) => Choose();
        Grid.SetRow(_list, 2);
        _root.Children.Add(_list);

        // 底部：排序按钮（显示现在的排序）、"隐藏缩略图导航"、"确定"。
        var bottom = new Grid { ColumnSpacing = 16 };
        bottom.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        bottom.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        bottom.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        _hide.Content = AnyviewCore.UiText(TextKey.PickerHideThumbnails, lang);
        _hide.IsChecked = core.PickerHidesThumbnails();
        _hide.Checked += (_, _) => HideChanged(true);
        _hide.Unchecked += (_, _) => HideChanged(false);
        var ok = new Button { Content = AnyviewCore.UiText(TextKey.PickerOk, lang), MinWidth = 96 };
        ok.Click += (_, _) => Choose();
        bottom.Children.Add(_sort);
        Grid.SetColumn(_hide, 1);
        bottom.Children.Add(_hide);
        Grid.SetColumn(ok, 2);
        bottom.Children.Add(ok);
        Grid.SetRow(bottom, 3);
        _root.Children.Add(bottom);
        _strip.Visibility = _hide.IsChecked == true ? Visibility.Collapsed : Visibility.Visible;

        var host = new UserControl { Content = _root, IsTabStop = false };
        // 回车切换，Esc 不切换。
        host.PreviewKeyDown += (_, e) =>
        {
            if (e.Key == VirtualKey.Enter)
            {
                e.Handled = true;
                Choose();
            }
            else if (e.Key == VirtualKey.Escape)
            {
                e.Handled = true;
                Window?.Close();
            }
        };
        Window = SubWindows.Create(AnyviewCore.UiText(TextKey.PickerTitle, lang), 760, 560, owner, host, lang);
        Window.Closed += (_, _) =>
        {
            IsClosed = true;
            // 不再要缩略图了。
            _core.PickerWantThumbnails(Array.Empty<string>());
            OnClosed?.Invoke();
        };
        Reload();
    }

    public void Show()
    {
        Window.Activate();
        _list.Focus(FocusState.Programmatic);
    }

    // ---- 内容 ----

    /// <summary>选中的行。</summary>
    public int? SelectedIndex => _list.SelectedIndex >= 0 && _list.SelectedIndex < _rows.Length ? _list.SelectedIndex : null;

    public int RowCount => _rows.Length;

    public PickerRow? Row(int index) => index >= 0 && index < _rows.Length ? _rows[index] : null;

    public string SortLabel => _sort.Content as string ?? "";

    public bool StripVisible => _strip.Visibility == Visibility.Visible;

    /// <summary>"隐藏缩略图导航"记在设置里的值（自检用）。</summary>
    public bool HidesThumbnailsSetting => _core.PickerHidesThumbnails();

    /// <summary>缩略图带上看得见的几张里，已经有缩略图的张数和总张数（自检用）。</summary>
    public (int Ready, int Visible) ThumbnailsLoaded =>
        (_cells.Count(i => i < _rows.Length && _thumbs.TryGetValue(_rows[i].Path, out var t) && t is not null), _cells.Count);

    /// <summary>从核心重新取列表（打开时、排序后、文件夹变了）。原来选中的文件还在就继续选中它，否则选中当前图片。</summary>
    public void Reload()
    {
        if (_core.PickerList(_lang) is not PickerList list)
        {
            return;
        }
        var keep = SelectedIndex is int s ? _rows[s].Path : null;
        var target = keep is not null ? Array.FindIndex(list.Rows, r => r.Path == keep) : -1;
        if (target < 0)
        {
            target = list.Current is uint current ? (int)current : 0;
        }
        _rows = list.Rows;
        _sortOptions = list.SortOptions;
        RebuildSortMenu(list.SortLabel);
        _updating = true;
        _list.ItemsSource = _rows.ToList();
        _updating = false;
        Select(target);
    }

    private void RebuildSortMenu(string title)
    {
        _sort.Content = title;
        var menu = new MenuFlyout();
        foreach (var option in _sortOptions)
        {
            if (option.Action == "sort_desc" && menu.Items.Count > 0)
            {
                menu.Items.Add(new MenuFlyoutSeparator());
            }
            var item = new ToggleMenuFlyoutItem { Text = option.Title, IsChecked = option.Checked };
            var action = option.Action;
            item.Click += (_, _) => ChooseSort(action);
            menu.Items.Add(item);
        }
        UiLanguage.Apply(menu.Items, UiLanguage.Code(_lang));
        _sort.Flyout = menu;
    }

    /// <summary>选中第 index 行并滚到能看见（单击、方向键的效果）。</summary>
    public void Select(int index)
    {
        if (index < 0 || index >= _rows.Length)
        {
            return;
        }
        if (_list.SelectedIndex != index)
        {
            _updating = true;
            _list.SelectedIndex = index;
            _updating = false;
        }
        _list.ScrollIntoView(_list.Items[index]);
        RefreshStrip();
    }

    private void Step(int step)
    {
        if (SelectedIndex is int current)
        {
            Select(Math.Max(0, Math.Min(_rows.Length - 1, current + step)));
        }
    }

    /// <summary>按缩略图带的宽度排好格子，向核心要还没有的缩略图，重画。</summary>
    private void RefreshStrip()
    {
        _strip.Children.Clear();
        _cells.Clear();
        _spans.Clear();
        if (_strip.Visibility != Visibility.Visible || SelectedIndex is not int center)
        {
            return;
        }
        var width = _strip.ActualWidth;
        var bigWidth = Big.Width + 2 * FrameWidth;
        var placed = new List<(int Index, double X, double Width)>();
        var x = (width - bigWidth) / 2;
        placed.Add((center, x, bigWidth));
        for (var i = center; i > 0 && x > 0;)
        {
            i--;
            x -= Gap + Small.Width;
            placed.Add((i, x, Small.Width));
        }
        x = (width + bigWidth) / 2;
        for (var i = center; i + 1 < _rows.Length && x < width;)
        {
            i++;
            placed.Add((i, x + Gap, Small.Width));
            x += Gap + Small.Width;
        }
        var missing = new List<string>();
        foreach (var (index, left, cellWidth) in placed.OrderBy(p => p.Index))
        {
            _cells.Add(index);
            _spans.Add((index, left, cellWidth));
            var selected = index == center;
            var box = selected ? Big : Small;
            var path = _rows[index].Path;
            if (!_thumbs.ContainsKey(path))
            {
                if (_core.PickerThumbnail(path) is ImageHandle handle)
                {
                    using (handle)
                    {
                        _thumbs[path] = ToImageSource(handle);
                    }
                }
                else
                {
                    missing.Add(path);
                }
            }
            _thumbs.TryGetValue(path, out var source);
            var image = new Image { Source = source, Stretch = Stretch.Uniform, Width = box.Width, Height = box.Height };
            var cell = new Border
            {
                Child = image,
                Width = box.Width + (selected ? 2 * FrameWidth : 0),
                Height = box.Height + (selected ? 2 * FrameWidth : 0),
                BorderThickness = new Thickness(selected ? FrameWidth : 0),
                BorderBrush = new SolidColorBrush(Color.FromArgb(255, 255, 153, 26)),
                Background = new SolidColorBrush(source is null && _thumbs.ContainsKey(path)
                    ? Color.FromArgb(255, 31, 31, 31) : Color.FromArgb(255, 51, 51, 51)),
            };
            Canvas.SetLeft(cell, left);
            Canvas.SetTop(cell, (StripHeight - cell.Height) / 2);
            _strip.Children.Add(cell);
        }
        _core.PickerWantThumbnails(missing.ToArray());
    }

    /// <summary>缩略图好了（核心的 Thumbnail 事件；句柄在这里释放）。</summary>
    public void ThumbnailArrived(string path, ImageHandle? image)
    {
        if (image is null)
        {
            _thumbs[path] = null;
        }
        else
        {
            using (image)
            {
                _thumbs[path] = ToImageSource(image);
            }
        }
        if (_cells.Any(i => i < _rows.Length && _rows[i].Path == path))
        {
            RefreshStrip();
        }
    }

    /// <summary>核心的图像（BGRA8 预乘）→ XAML 的位图。</summary>
    private static ImageSource? ToImageSource(ImageHandle handle)
    {
        var bytes = CanvasView.CopyPixels(handle, out var width, out var height);
        if (bytes is null)
        {
            return null;
        }
        var bitmap = new WriteableBitmap(width, height);
        using (var stream = bitmap.PixelBuffer.AsStream())
        {
            stream.Write(bytes, 0, bytes.Length);
        }
        bitmap.Invalidate();
        return bitmap;
    }

    /// <summary>
    /// "确定"、双击、回车：切到所选图片，关掉窗口。按所选文件的路径在核心现在的列表里找序号（列表可能刚变过，
    /// 如文件夹里删了一张而这里还没重新载入：按旧序号会切到别的图）；找不到时重新载入、不关窗口。
    /// </summary>
    public void Choose()
    {
        if (SelectedIndex is int index)
        {
            var path = _rows[index].Path;
            var current = _core.PickerList(_lang)?.Rows ?? Array.Empty<PickerRow>();
            var actual = Array.FindIndex(current, r => r.Path == path);
            if (actual < 0)
            {
                Reload();
                return;
            }
            _core.PickerChoose((uint)actual);
        }
        Window.Close();
    }

    public void SetHideThumbnails(bool hide) => _hide.IsChecked = hide;

    private void HideChanged(bool hide)
    {
        _strip.Visibility = hide ? Visibility.Collapsed : Visibility.Visible;
        _core.SetPickerHidesThumbnails(hide);
        RefreshStrip();
    }

    /// <summary>排序菜单里的一项（自检也用）：与用户在菜单里点它相同。</summary>
    public bool ChooseSort(string action)
    {
        if (!_sortOptions.Any(o => o.Action == action))
        {
            return false;
        }
        _core.InvokeAction(action);
        Reload();
        return true;
    }

    /// <summary>把窗口的内容画成 PNG（自检报告用）。</summary>
    public Task<string> SnapshotAsync(string path) => SubWindows.SnapshotAsync(_root, path);
}
