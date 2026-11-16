// 设置窗口（docs/SETTINGS.md"设置窗口"）。
//  重新设计：左边是带图标的分类列表；右边是页的大标题，下面按核心给的分组排成几张圆角卡片，
// 每行左边是名字（后面可以跟着灰色的快捷键，下面可以有一行灰色说明），右边是开关、下拉框等；底部左边"恢复本页默认"，
// 右边"确定""取消"（Windows 的顺序）。所有修改只改核心里的草稿，点"确定"才校验、生效、保存；关闭窗口时有没确定的
// 修改就先询问。哪些项能改、改了以后哪些项跟着灰显、开关是不是翻过来说的，都由核心决定（每次修改后重新取这一页）。
// 只用 WinUI 自带样式的控件（开关、下拉框、文本框、按钮、列表）：控件库的样式（XamlControlsResources）在运行中加不了，
// 卡片的底色、边线这些颜色自己给（整个程序是深色外观）。颜色用系统的颜色对话框。
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Threading.Tasks;
using Anyview.Core;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.Storage.Pickers;
using Windows.System;
using Windows.UI;
using Windows.UI.ViewManagement;
using WinRT.Interop;

namespace Anyview;

internal sealed class SettingsWindow
{
    public readonly Window Window;
    private AppCore _core;
    private readonly Lang _lang;
    private readonly nint _hwnd;
    private readonly SettingsPageInfo[] _pages;
    private readonly ListView _sidebar = new() { SelectionMode = ListViewSelectionMode.Single };
    private readonly StackPanel _pageBox = new() { Margin = new Thickness(32, 22, 32, 18) };
    private readonly ScrollViewer _scroller;
    private readonly Grid _root = new();
    private string _current = "";
    private readonly List<Row> _rows = new();
    /// <summary>程序自己在改控件的值（不是用户操作），这时不把变化写进草稿。</summary>
    private bool _updating;
    /// <summary>已经确定或放弃，窗口正在关闭：不再询问。</summary>
    private bool _closing;
    private bool _rebuildPending;
    private bool _dialogOpen;
    /// <summary>正在等按键的那一行快捷键（点了按键框以后）：快捷键列表和映射表里的行号。</summary>
    private (Row Table, uint Index)? _capturing;
    /// <summary>刚"添加"的快捷键行（映射表的行号）：重建这一页以后让它的按键框等着按键。</summary>
    private uint? _captureAfterRebuild;
    private SettingsOption[]? _specialKeys;

    /// <summary>自检时为真：校验失败不弹提示框（只记下来），清除记住的位置不先问。</summary>
    public bool Quiet;
    /// <summary>最近一次"确定"校验失败的提示。</summary>
    public string? LastError { get; private set; }
    /// <summary>"确定"生效以后（应用刷新所有窗口、换语言）。</summary>
    public Action<SettingsApplyResult.Applied>? OnApplied;
    public Action? OnClosed;
    public bool IsClosed { get; private set; }
    /// <summary>用哪个主窗口的核心改设置（设置和草稿是各窗口共用的，用哪个都一样）。</summary>
    public AppCore Core => _core;

    /// <summary>原来用的那个主窗口要关掉了：改用另一个主窗口的核心（草稿在共用的设置里，不受影响）。</summary>
    public void Rebind(AppCore core) => _core = core;

    /// <summary>原来归 oldOwner 所有时改归 newOwner（oldOwner 要关掉了）。</summary>
    public void Reown(nint oldOwner, nint newOwner) => SubWindows.Reown(Window, oldOwner, newOwner);

    // ---- 颜色（深色外观配色） ----

    private static readonly Color WindowColor = Color.FromArgb(255, 0x20, 0x20, 0x20);
    private static readonly Color SidebarColor = Color.FromArgb(255, 0x27, 0x27, 0x27);
    private static readonly Color FooterColor = Color.FromArgb(255, 0x1C, 0x1C, 0x1C);
    private static readonly Color CardColor = Color.FromArgb(255, 0x2B, 0x2B, 0x2B);
    private static readonly Color CardEdgeColor = Color.FromArgb(255, 0x35, 0x35, 0x35);
    private static readonly Color LineColor = Color.FromArgb(255, 0x38, 0x38, 0x38);
    private static readonly Color DimColor = Color.FromArgb(255, 0x9E, 0x9E, 0xA4);
    private static readonly Color CaptionColor = Color.FromArgb(255, 0xA8, 0xA8, 0xAE);
    private static readonly Color ErrorColor = Color.FromArgb(255, 232, 72, 72);

    private static SolidColorBrush Brush(Color color) => new(color);

    private enum Kind
    {
        Toggle,
        Choice,
        Number,
        Color,
        Path,
        Mapping,
        KeyMappings,
        Other,
    }

    /// <summary>一项在窗口里的控件（卡片里的一行）。</summary>
    private sealed class Row
    {
        public required string Key;
        /// <summary>整行：灰显时整行一起（ContentControl 的 IsEnabled 管到里面所有控件）。</summary>
        public required ContentControl Container;
        /// <summary>这一行的外框：校验失败时标红。</summary>
        public required Border Frame;
        /// <summary>左边的文字（灰显时变淡：文字本身没有灰显的样子）。</summary>
        public FrameworkElement? Text;
        /// <summary>开关是翻过来说的（只给自检把存的值换成开关的状态）。</summary>
        public bool Inverted;
        public Kind Kind = Kind.Other;
        public ToggleSwitch? Switch;
        public ComboBox? Combo;
        public List<string> Ids = new();
        public TextBox? Field;
        public uint Color;
        /// <summary>颜色：各个预设色块和它外面那一圈（是当前颜色时显示）。</summary>
        public List<(uint Preset, Ellipse Ring)> Swatches = new();
        public List<KeyRow> KeyRows = new();
    }

    /// <summary>
    /// 自己设的快捷键的一行：按键框（点一下后按下组合键录入）、旁边的小按钮（列出不方便按出的键）、动作下拉框和提示、
    /// 开关、删除按钮。
    /// </summary>
    private sealed class KeyRow
    {
        /// <summary>在映射表里是第几行。</summary>
        public required uint Index;
        public required Border Frame;
        public required ToggleSwitch Switch;
        public required Button Key;
        public required Button More;
        public required ComboBox Combo;
        public required TextBlock Note;
    }

    private readonly MainWindow? _owner;

    public SettingsWindow(AppCore core, Lang lang, MainWindow? owner)
    {
        _core = core;
        _lang = lang;
        _owner = owner;
        core.SettingsBegin();
        _pages = core.SettingsPages(lang);

        // 左边：带图标的分类列表。
        foreach (var page in _pages)
        {
            var line = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12, Padding = new Thickness(2, 6, 2, 6) };
            line.Children.Add(new FontIcon
            {
                FontFamily = new FontFamily(Overlays.IconFont),
                Glyph = PageGlyph(page.Id),
                FontSize = 16,
                VerticalAlignment = VerticalAlignment.Center,
            });
            line.Children.Add(new TextBlock { Text = page.Title, FontSize = 14, VerticalAlignment = VerticalAlignment.Center });
            _sidebar.Items.Add(line);
        }
        _sidebar.SelectionChanged += (_, _) =>
        {
            if (_updating)
            {
                return;
            }
            var index = _sidebar.SelectedIndex;
            if (index >= 0 && index < _pages.Length && _pages[index].Id != _current)
            {
                ShowPage(_pages[index].Id);
            }
        };
        var left = new Border
        {
            Width = 196,
            Padding = new Thickness(6, 12, 6, 12),
            Background = Brush(SidebarColor),
            BorderBrush = Brush(LineColor),
            BorderThickness = new Thickness(0, 0, 1, 0),
            Child = _sidebar,
        };

        // 右边：可以滚动的页，下面一条按钮。
        _scroller = new ScrollViewer
        {
            Content = _pageBox,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
        };
        var reset = FlatButton(Text(TextKey.SettingsReset));
        reset.Click += (_, _) => Reset();
        var ok = AccentButton(Text(TextKey.SettingsOk));
        ok.Click += (_, _) => Ok();
        var cancel = new Button { Content = Text(TextKey.ButtonCancel), MinWidth = 80 };
        // "取消"：直接放弃修改、关窗口，不问。
        cancel.Click += (_, _) => Discard();
        var footer = new Grid { ColumnSpacing = 8 };
        footer.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        footer.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        footer.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        footer.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        footer.Children.Add(reset);
        Grid.SetColumn(ok, 2);
        footer.Children.Add(ok);
        Grid.SetColumn(cancel, 3);
        footer.Children.Add(cancel);
        var footerFrame = new Border
        {
            Padding = new Thickness(20, 12, 20, 12),
            Background = Brush(FooterColor),
            BorderBrush = Brush(LineColor),
            BorderThickness = new Thickness(0, 1, 0, 0),
            Child = footer,
        };
        var right = new Grid();
        right.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        right.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        right.Children.Add(_scroller);
        Grid.SetRow(footerFrame, 1);
        right.Children.Add(footerFrame);

        _root.Background = Brush(WindowColor);
        _root.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        _root.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        _root.Children.Add(left);
        Grid.SetColumn(right, 1);
        _root.Children.Add(right);
        var host = new UserControl { Content = _root, IsTabStop = false };
        host.PreviewKeyDown += OnKeyDown;
        host.PreviewKeyUp += (_, e) =>
        {
            if (_swallowKeyUp)
            {
                _swallowKeyUp = false;
                e.Handled = true;
            }
        };

        Window = SubWindows.Create(Text(TextKey.SettingsTitle), 860, 680, owner, host, _lang);
        _hwnd = WindowNative.GetWindowHandle(Window);
        // 用户点关闭按钮：有没确定的修改就先询问。
        Window.AppWindow.Closing += (_, args) =>
        {
            if (!_closing && _core.SettingsDirty())
            {
                args.Cancel = true;
                RequestClose();
            }
        };
        Window.Closed += (_, _) =>
        {
            IsClosed = true;
            // 没点"确定"就关掉：丢掉草稿（"确定"时草稿已经用掉了）。
            _core.SettingsCancel();
            OnClosed?.Invoke();
        };
        if (_pages.Length > 0)
        {
            SelectPage(_pages[0].Id);
        }
    }

    private string Text(TextKey key) => AnyviewCore.UiText(key, _lang);

    /// <summary>左边列表里各页的图标（Segoe Fluent Icons / Segoe MDL2 Assets 里两边都有的字形）。</summary>
    private static string PageGlyph(string id) => id switch
    {
        "general" => "\uE713",
        "browse" => "\uE890",
        "image" => "\uE91B",
        "formats" => "\uE7AC",
        "keyboard" => "\uE765",
        "mouse" => "\uE962",
        "bookmarks" => "\uE8A4",
        "context_menu" => "\uE8FD",
        "misc" => "\uE71D",
        "language" => "\uE774",
        _ => "\uE713",
    };

    /// <summary>不带底色的按钮（"恢复本页默认"、色块、删除）：指针移上去时照常变亮。</summary>
    private static Button FlatButton(object content) => new()
    {
        Content = content,
        Background = new SolidColorBrush(Colors.Transparent),
        BorderThickness = new Thickness(0),
        MinWidth = 0,
        VerticalAlignment = VerticalAlignment.Center,
    };

    /// <summary>
    /// "确定"：系统的强调色（取不到时用 Windows 默认的蓝）。控件库的 AccentButtonStyle 在 XamlControlsResources 里，
    /// 这里没有：按钮自己的资源里换掉指针移上去、按下时的颜色（WinUI 的轻量样式）。
    /// </summary>
    private static Button AccentButton(string text)
    {
        Color accent;
        try
        {
            accent = new UISettings().GetColorValue(UIColorType.Accent);
        }
        catch (Exception)
        {
            accent = Color.FromArgb(255, 0x00, 0x78, 0xD4);
        }
        static Color Mix(Color c, double toward, byte target) => Color.FromArgb(255,
            (byte)Math.Round(c.R + (target - c.R) * toward),
            (byte)Math.Round(c.G + (target - c.G) * toward),
            (byte)Math.Round(c.B + (target - c.B) * toward));
        var white = Brush(Color.FromArgb(255, 255, 255, 255));
        var button = new Button
        {
            Content = text,
            MinWidth = 80,
            Background = Brush(accent),
            Foreground = white,
            BorderThickness = new Thickness(0),
        };
        button.Resources["ButtonBackground"] = Brush(accent);
        button.Resources["ButtonBackgroundPointerOver"] = Brush(Mix(accent, 0.15, 255));
        button.Resources["ButtonBackgroundPressed"] = Brush(Mix(accent, 0.2, 0));
        button.Resources["ButtonForeground"] = white;
        button.Resources["ButtonForegroundPointerOver"] = white;
        button.Resources["ButtonForegroundPressed"] = Brush(Color.FromArgb(255, 230, 230, 230));
        button.Resources["ButtonBorderBrushPointerOver"] = new SolidColorBrush(Colors.Transparent);
        button.Resources["ButtonBorderBrushPressed"] = new SolidColorBrush(Colors.Transparent);
        return button;
    }

    public void Show()
    {
        Window.Activate();
    }

    // ---- 页面 ----

    /// <summary>现在显示的页。</summary>
    public string CurrentPage => _current;

    /// <summary>切到一页（左侧列表跟着选中）。</summary>
    public void SelectPage(string id)
    {
        var index = Array.FindIndex(_pages, p => p.Id == id);
        if (index >= 0 && _sidebar.SelectedIndex != index)
        {
            _updating = true;
            _sidebar.SelectedIndex = index;
            _updating = false;
        }
        ShowPage(id);
    }

    private void ShowPage(string id)
    {
        if (_core.SettingsPage(id, _lang) is not SettingsPage page)
        {
            return;
        }
        var samePage = _current == id;
        _current = id;
        _capturing = null;
        _pageBox.Children.Clear();
        _rows.Clear();
        _updating = true;
        _pageBox.Children.Add(new TextBlock
        {
            Text = page.Title,
            FontSize = 24,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
            Margin = new Thickness(0, 0, 0, 14),
        });
        if (page.Lead.Length > 0)
        {
            _pageBox.Children.Add(new TextBlock
            {
                Text = page.Lead,
                TextWrapping = TextWrapping.Wrap,
                Foreground = Brush(DimColor),
                Margin = new Thickness(0, -6, 0, 8),
            });
        }
        for (var section = 0; section < page.Sections.Length; section++)
        {
            var members = page.Items.Where(i => i.Section == section).ToArray();
            if (members.Length == 0)
            {
                continue;
            }
            var caption = page.Sections[section];
            if (caption.Length > 0)
            {
                _pageBox.Children.Add(new TextBlock
                {
                    Text = caption,
                    FontSize = 12,
                    FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                    Foreground = Brush(CaptionColor),
                    Margin = new Thickness(4, 18, 0, 7),
                });
            }
            else
            {
                _pageBox.Children.Add(new Border { Height = 10 });
            }
            // 快捷键列表另外排（下面还有"添加快捷键"）。
            if (members.Length == 1 && members[0].Control is SettingsControl.KeyMappings mappings)
            {
                var table = BuildKeyMappings(members[0], mappings);
                _pageBox.Children.Add(table.Frame);
                _rows.Add(table);
                continue;
            }
            var card = new StackPanel();
            for (var i = 0; i < members.Length; i++)
            {
                if (i > 0)
                {
                    card.Children.Add(Separator(members[i].Indent));
                }
                var row = BuildRow(members[i]);
                card.Children.Add(row.Frame);
                _rows.Add(row);
            }
            _pageBox.Children.Add(Card(card));
        }
        _updating = false;
        if (!samePage)
        {
            _scroller.ChangeView(null, 0, null, true);
        }
        RefreshStates();
        if (_captureAfterRebuild is uint pending)
        {
            _captureAfterRebuild = null;
            StartCapture(pending);
        }
    }

    /// <summary>一张圆角卡片。</summary>
    private static Border Card(UIElement content) => new()
    {
        CornerRadius = new CornerRadius(8),
        Background = Brush(CardColor),
        BorderBrush = Brush(CardEdgeColor),
        BorderThickness = new Thickness(1),
        Child = content,
    };

    /// <summary>卡片里两行之间的细线（从左边留一点开始，跟着缩进）。</summary>
    private static Rectangle Separator(byte indent) => new()
    {
        Height = 1,
        Fill = Brush(LineColor),
        Margin = new Thickness(14 + indent * 20, 0, 0, 0),
    };

    /// <summary>按核心的页面数据更新各项的可用状态（修改后，依赖它的项可能跟着灰显），以及快捷键各行的按键和提示。</summary>
    private void RefreshStates()
    {
        if (_core.SettingsPage(_current, _lang) is not SettingsPage page)
        {
            return;
        }
        _updating = true;
        foreach (var row in _rows)
        {
            var item = page.Items.FirstOrDefault(i => i.Key == row.Key);
            if (item is null)
            {
                continue;
            }
            row.Container.IsEnabled = item.Enabled;
            if (row.Text is not null)
            {
                row.Text.Opacity = item.Enabled ? 1 : 0.45;
            }
            if (item.Control is SettingsControl.KeyMappings mappings)
            {
                // 多了或少了一行：整页重建（等这次事件处理完）。
                if (row.KeyRows.Count != mappings.Rows.Length
                    || row.KeyRows.Zip(mappings.Rows).Any(pair => pair.First.Index != pair.Second.Row))
                {
                    ScheduleRebuild();
                    continue;
                }
                for (var i = 0; i < mappings.Rows.Length; i++)
                {
                    UpdateKeyRow(row, row.KeyRows[i], mappings.Rows[i]);
                }
            }
        }
        _updating = false;
    }

    /// <summary>快捷键一行跟着草稿更新：开关、按键、动作、提示，以及关掉时按键和动作灰显。</summary>
    private void UpdateKeyRow(Row table, KeyRow w, KeyMappingRow r)
    {
        w.Switch.IsOn = r.Enabled;
        if (_capturing is not { } c || !ReferenceEquals(c.Table, table) || c.Index != w.Index)
        {
            w.Key.Content = r.KeyLabel.Length > 0 ? r.KeyLabel : Text(TextKey.SettingsKeyUnset);
        }
        w.Note.Text = r.Note;
        w.Note.Foreground = Brush(DimColor);
        w.Note.Visibility = r.Note.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        var index = table.Ids.IndexOf(r.Action);
        if (index >= 0 && w.Combo.SelectedIndex != index)
        {
            w.Combo.SelectedIndex = index;
        }
        w.Key.IsEnabled = r.Enabled;
        w.More.IsEnabled = r.Enabled;
        w.Combo.IsEnabled = r.Enabled;
        w.Frame.Background = null;
    }

    private void ScheduleRebuild()
    {
        if (_rebuildPending)
        {
            return;
        }
        _rebuildPending = true;
        Window.DispatcherQueue.TryEnqueue(() =>
        {
            _rebuildPending = false;
            // 排队的回调里漏出去的异常会让程序直接崩溃：接住、记下来。
            try
            {
                if (!IsClosed)
                {
                    ShowPage(_current);
                }
            }
            catch (Exception error)
            {
                Crash.Write("SettingsWindow.Rebuild", error);
            }
        });
    }

    /// <summary>把一次修改写进草稿，再刷新各项的可用状态。返回核心是否接受。</summary>
    private bool Set(string key, SettingsInput input)
    {
        if (_updating)
        {
            return false;
        }
        var accepted = _core.SettingsSet(key, input);
        RefreshStates();
        return accepted;
    }

    // ---- 生成控件 ----

    private static ComboBox Combo(SettingsOption[] options, string selected, double width)
    {
        var combo = new ComboBox { Width = width, VerticalAlignment = VerticalAlignment.Center };
        foreach (var option in options)
        {
            combo.Items.Add(option.Title);
        }
        combo.SelectedIndex = Array.FindIndex(options, o => o.Id == selected);
        return combo;
    }

    private static ToggleSwitch Switch(bool on)
    {
        var toggle = new ToggleSwitch
        {
            IsOn = on,
            OnContent = "",
            OffContent = "",
            MinWidth = 0,
            HorizontalAlignment = HorizontalAlignment.Right,
            VerticalAlignment = VerticalAlignment.Center,
            // 模板里开关右边还有一格 12 的空当（隔开“开/关”文字用的），挪掉它，开关的右沿和下拉框对齐。
            Margin = new Thickness(0, 0, -12, 0),
        };
        // Windows App SDK 1.8 的开关模板在里面那层 Grid 上写着最小宽度 154（给“开/关”文字留的地方，开关画在左边），
        // 开关自己的 MinWidth、在开关的资源里改 ToggleSwitchThemeMinWidth 都管不到它，开关离右沿空出一大截。
        // 套上模板以后把那一层的最小宽度去掉。
        toggle.Loaded += (_, _) =>
        {
            if (VisualTreeHelper.GetChildrenCount(toggle) > 0 && VisualTreeHelper.GetChild(toggle, 0) is Grid root)
            {
                foreach (var child in root.Children)
                {
                    if (child is FrameworkElement part && part.MinWidth > 0)
                    {
                        part.MinWidth = 0;
                    }
                }
            }
        };
        return toggle;
    }

    private static Color ToColor(uint rgb) => Color.FromArgb(255, (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb);

    /// <summary>一行左边的文字：名字（后面可以跟着灰色的快捷键），下面一行灰色说明。</summary>
    private static StackPanel TextColumn(SettingsItem item)
    {
        var column = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        var title = new TextBlock { TextWrapping = TextWrapping.Wrap, FontSize = 14 };
        title.Inlines.Add(new Run { Text = item.Label });
        if (item.Shortcut.Length > 0)
        {
            title.Inlines.Add(new Run { Text = "   " + item.Shortcut, Foreground = Brush(DimColor) });
        }
        column.Children.Add(title);
        if (item.Detail.Length > 0)
        {
            column.Children.Add(new TextBlock
            {
                Text = item.Detail,
                TextWrapping = TextWrapping.Wrap,
                FontSize = 12,
                Foreground = Brush(DimColor),
            });
        }
        return column;
    }

    /// <summary>右边的一排控件。</summary>
    private static StackPanel Controls() => new()
    {
        Orientation = Orientation.Horizontal,
        Spacing = 8,
        VerticalAlignment = VerticalAlignment.Center,
        HorizontalAlignment = HorizontalAlignment.Right,
    };

    private Row BuildRow(SettingsItem item)
    {
        // 一行：左边文字（占满剩下的宽度），右边控件。
        var grid = new Grid { ColumnSpacing = 16 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var container = new ContentControl
        {
            Content = grid,
            IsTabStop = false,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            VerticalContentAlignment = VerticalAlignment.Center,
        };
        var frame = new Border
        {
            Child = container,
            Padding = new Thickness(14 + item.Indent * 20, 8, 14, 8),
            MinHeight = 44,
            CornerRadius = new CornerRadius(8),
            BorderThickness = new Thickness(0),
        };
        var row = new Row { Key = item.Key, Container = container, Frame = frame, Inverted = item.Inverted };
        var key = item.Key;
        void AddText()
        {
            var text = TextColumn(item);
            grid.Children.Add(text);
            row.Text = text;
        }
        void AddControls(UIElement controls)
        {
            Grid.SetColumn((FrameworkElement)controls, 1);
            grid.Children.Add(controls);
        }
        switch (item.Control)
        {
            case SettingsControl.Toggle toggle:
            {
                AddText();
                var control = Switch(toggle.On);
                control.Toggled += (_, _) => Set(key, new SettingsInput.Bool(control.IsOn));
                AddControls(control);
                row.Kind = Kind.Toggle;
                row.Switch = control;
                break;
            }
            case SettingsControl.Choice choice:
            {
                AddText();
                var combo = Combo(choice.Options, choice.Selected, 220);
                var ids = choice.Options.Select(o => o.Id).ToList();
                combo.SelectionChanged += (_, _) =>
                {
                    if (combo.SelectedIndex >= 0 && combo.SelectedIndex < ids.Count)
                    {
                        Set(key, new SettingsInput.Choice(ids[combo.SelectedIndex]));
                    }
                };
                AddControls(combo);
                row.Kind = Kind.Choice;
                row.Combo = combo;
                row.Ids = ids;
                break;
            }
            case SettingsControl.Number number:
            {
                AddText();
                var controls = Controls();
                var field = new TextBox { Text = number.Text, Width = 90, TextAlignment = TextAlignment.Right, VerticalAlignment = VerticalAlignment.Center };
                field.TextChanged += (_, _) => Set(key, new SettingsInput.Text(field.Text));
                controls.Children.Add(field);
                controls.Children.Add(new TextBlock { Text = number.Unit, Foreground = Brush(DimColor), VerticalAlignment = VerticalAlignment.Center });
                AddControls(controls);
                row.Kind = Kind.Number;
                row.Field = field;
                break;
            }
            case SettingsControl.Color color:
            {
                // 名字一行，下面一行是圆形色块（当前颜色的外面有一圈）和"选取…"（打开系统的颜色对话框）：
                // 15 个色块和名字排在一行时，英文和大多数语言的名字只剩几十像素宽，会被拆成几段。
                grid.ColumnDefinitions.Clear();
                grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
                grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
                grid.RowSpacing = 8;
                AddText();
                var controls = Controls();
                controls.Spacing = 2;
                controls.HorizontalAlignment = HorizontalAlignment.Left;
                foreach (var preset in color.Presets)
                {
                    var ring = new Ellipse
                    {
                        Width = 22,
                        Height = 22,
                        Stroke = Brush(Color.FromArgb(230, 255, 255, 255)),
                        StrokeThickness = 2,
                        Visibility = preset == color.Value ? Visibility.Visible : Visibility.Collapsed,
                    };
                    var dot = new Ellipse
                    {
                        Width = 15,
                        Height = 15,
                        Fill = Brush(ToColor(preset)),
                        Stroke = Brush(Color.FromArgb(110, 128, 128, 128)),
                        StrokeThickness = 1,
                    };
                    var face = new Grid { Width = 22, Height = 22 };
                    face.Children.Add(ring);
                    face.Children.Add(dot);
                    var chip = FlatButton(face);
                    chip.Padding = new Thickness(1);
                    chip.MinHeight = 0;
                    chip.CornerRadius = new CornerRadius(12);
                    ToolTipService.SetToolTip(chip, $"#{preset:X6}");
                    var value = preset;
                    chip.Click += (_, _) => SetColor(key, value);
                    controls.Children.Add(chip);
                    row.Swatches.Add((preset, ring));
                }
                var choose = new Button { Content = color.Choose, Margin = new Thickness(10, 0, 0, 0), VerticalAlignment = VerticalAlignment.Center };
                choose.Click += (_, _) => PressButton(key, "choose");
                controls.Children.Add(choose);
                Grid.SetRow(controls, 1);
                grid.Children.Add(controls);
                row.Kind = Kind.Color;
                row.Color = color.Value;
                break;
            }
            case SettingsControl.Path path:
            {
                // 名字（有的话）、输入框占剩下的宽度、按钮按文字宽度排在后面
                // （横排的面板给多少宽度都会全给，按钮会被挤出窗口，所以用表格）。
                grid.ColumnDefinitions.Clear();
                grid.ColumnSpacing = 8;
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star), MinWidth = 120 });
                if (item.Label.Length > 0)
                {
                    var label = new TextBlock { Text = item.Label, Width = 76, VerticalAlignment = VerticalAlignment.Center };
                    grid.Children.Add(label);
                    row.Text = label;
                }
                var field = new TextBox { Text = path.Text, VerticalAlignment = VerticalAlignment.Center };
                field.TextChanged += (_, _) => Set(key, new SettingsInput.Text(field.Text));
                Grid.SetColumn(field, 1);
                grid.Children.Add(field);
                var column = 2;
                foreach (var button in path.ButtonsValue)
                {
                    grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
                    var b = new Button { Content = button.Title, VerticalAlignment = VerticalAlignment.Center };
                    var id = button.Id;
                    b.Click += (_, _) => PressButton(key, id);
                    Grid.SetColumn(b, column++);
                    grid.Children.Add(b);
                }
                row.Kind = Kind.Path;
                row.Field = field;
                break;
            }
            case SettingsControl.Buttons buttons:
            {
                var controls = Controls();
                foreach (var button in buttons.ButtonsValue)
                {
                    var b = new Button { Content = button.Title, VerticalAlignment = VerticalAlignment.Center };
                    var id = button.Id;
                    b.Click += (_, _) => PressButton(key, id);
                    controls.Children.Add(b);
                }
                if (item.Label.Length > 0)
                {
                    AddText();
                    AddControls(controls);
                }
                else
                {
                    controls.HorizontalAlignment = HorizontalAlignment.Left;
                    grid.Children.Add(controls);
                }
                break;
            }
            case SettingsControl.Note:
            {
                var note = new TextBlock
                {
                    Text = item.Label,
                    TextWrapping = TextWrapping.Wrap,
                    Foreground = Brush(Color.FromArgb(255, 0xC0, 0xC0, 0xC4)),
                    VerticalAlignment = VerticalAlignment.Center,
                };
                Grid.SetColumnSpan(note, 2);
                grid.Children.Add(note);
                row.Text = note;
                break;
            }
            case SettingsControl.Mapping mapping:
            {
                // 鼠标：一个下拉框，首项"默认"（照常用原来的功能），其次"无操作"，然后是动作。
                AddText();
                var combo = Combo(mapping.Options, mapping.Selected, 220);
                var ids = mapping.Options.Select(o => o.Id).ToList();
                combo.SelectionChanged += (_, _) =>
                {
                    if (combo.SelectedIndex >= 0 && combo.SelectedIndex < ids.Count)
                    {
                        Set(key, new SettingsInput.MappingChoice(ids[combo.SelectedIndex]));
                    }
                };
                AddControls(combo);
                row.Kind = Kind.Mapping;
                row.Combo = combo;
                row.Ids = ids;
                break;
            }
        }
        return row;
    }

    /// <summary>自己设的快捷键：一张卡片列出用过的行，下面一个"添加快捷键"。</summary>
    private Row BuildKeyMappings(SettingsItem item, SettingsControl.KeyMappings mappings)
    {
        var outer = new StackPanel { Spacing = 12 };
        var container = new ContentControl
        {
            Content = outer,
            IsTabStop = false,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
        };
        var frame = new Border { Child = container };
        var table = new Row
        {
            Key = item.Key,
            Container = container,
            Frame = frame,
            Kind = Kind.KeyMappings,
            Ids = mappings.Options.Select(o => o.Id).ToList(),
        };
        _specialKeys ??= _core.SettingsSpecialKeys(_lang);
        if (mappings.Rows.Length > 0)
        {
            var card = new StackPanel();
            for (var i = 0; i < mappings.Rows.Length; i++)
            {
                if (i > 0)
                {
                    card.Children.Add(Separator(0));
                }
                var w = BuildKeyRow(table, mappings.Rows[i], mappings.Options);
                table.KeyRows.Add(w);
                UpdateKeyRow(table, w, mappings.Rows[i]);
                card.Children.Add(w.Frame);
            }
            outer.Children.Add(Card(card));
        }
        // "添加快捷键"：打开映射表里的一行空行，它就出现在列表里，按键框等着按键。
        var content = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        content.Children.Add(new FontIcon { FontFamily = new FontFamily(Overlays.IconFont), Glyph = "\uE710", FontSize = 12 });
        content.Children.Add(new TextBlock { Text = Text(TextKey.SettingsAddShortcut) });
        var add = new Button { Content = content, HorizontalAlignment = HorizontalAlignment.Left, IsEnabled = mappings.AddRow is not null };
        var addRow = mappings.AddRow;
        add.Click += (_, _) =>
        {
            if (addRow is uint free && Set(table.Key, new SettingsInput.KeyRowEnabled(free, true)))
            {
                _captureAfterRebuild = free;
                ScheduleRebuild();
            }
        };
        outer.Children.Add(add);
        return table;
    }

    private KeyRow BuildKeyRow(Row table, KeyMappingRow mapping, SettingsOption[] options)
    {
        var index = mapping.Row;
        var key = table.Key;
        var grid = new Grid { ColumnSpacing = 10 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        // 按键框：点一下后按下组合键即录入（Esc、Tab 也能录入）。
        var keyButton = new Button
        {
            MinWidth = 104,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
            VerticalAlignment = VerticalAlignment.Center,
        };
        keyButton.Click += (_, _) => StartCapture(index);
        keyButton.LostFocus += (_, _) =>
        {
            if (_capturing is { } c && ReferenceEquals(c.Table, table) && c.Index == index)
            {
                _capturing = null;
                RefreshStates();
            }
        };
        // 小按钮：列出不方便直接按出的键，首项是清除。
        var more = new Button { Content = "…", MinWidth = 0, VerticalAlignment = VerticalAlignment.Center };
        var menu = new MenuFlyout();
        foreach (var special in _specialKeys ?? Array.Empty<SettingsOption>())
        {
            var entry = new MenuFlyoutItem { Text = special.Title };
            var name = special.Id;
            entry.Click += (_, _) => RecordKey(table, index, name, new KeyModifiers(false, false, false, false));
            menu.Items.Add(entry);
        }
        UiLanguage.Apply(menu.Items, UiLanguage.Code(_lang));
        more.Flyout = menu;
        var cell = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        cell.Children.Add(keyButton);
        cell.Children.Add(more);
        grid.Children.Add(cell);
        var arrow = new TextBlock { Text = "→", Foreground = Brush(DimColor), VerticalAlignment = VerticalAlignment.Center };
        Grid.SetColumn(arrow, 1);
        grid.Children.Add(arrow);
        // 动作下拉框，下面是这一行的提示（将替代某个默认快捷键、和别的行重复）。
        var column = new StackPanel { Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        var ids = table.Ids;
        var combo = Combo(options, mapping.Action, 236);
        combo.HorizontalAlignment = HorizontalAlignment.Left;
        combo.SelectionChanged += (_, _) =>
        {
            if (combo.SelectedIndex >= 0 && combo.SelectedIndex < ids.Count)
            {
                Set(key, new SettingsInput.KeyRowAction(index, ids[combo.SelectedIndex]));
            }
        };
        column.Children.Add(combo);
        var note = new TextBlock { FontSize = 12, TextWrapping = TextWrapping.Wrap, Foreground = Brush(DimColor) };
        column.Children.Add(note);
        Grid.SetColumn(column, 2);
        grid.Children.Add(column);
        var toggle = Switch(mapping.Enabled);
        toggle.Toggled += (_, _) => Set(key, new SettingsInput.KeyRowEnabled(index, toggle.IsOn));
        Grid.SetColumn(toggle, 3);
        grid.Children.Add(toggle);
        var remove = FlatButton(new FontIcon { FontFamily = new FontFamily(Overlays.IconFont), Glyph = "\uE74D", FontSize = 14 });
        ToolTipService.SetToolTip(remove, UiLanguage.Tip(Text(TextKey.SettingsRemoveShortcut), _lang));
        remove.Click += (_, _) => Set(key, new SettingsInput.KeyRowRemove(index));
        Grid.SetColumn(remove, 4);
        grid.Children.Add(remove);
        var frame = new Border
        {
            Child = grid,
            Padding = new Thickness(14, 8, 10, 8),
            CornerRadius = new CornerRadius(8),
        };
        return new KeyRow { Index = index, Frame = frame, Switch = toggle, Key = keyButton, More = more, Combo = combo, Note = note };
    }

    /// <summary>快捷键列表里映射表第 index 行的控件；不在列表里时为空。</summary>
    private (Row Table, KeyRow Row)? FindKeyRow(uint index)
    {
        foreach (var row in _rows.Where(r => r.Kind == Kind.KeyMappings))
        {
            if (row.KeyRows.FirstOrDefault(w => w.Index == index) is KeyRow w)
            {
                return (row, w);
            }
        }
        return null;
    }

    /// <summary>让第 index 行的按键框等着按键（点了按键框，或刚添加了这一行）。</summary>
    private void StartCapture(uint index)
    {
        if (FindKeyRow(index) is not { } found)
        {
            return;
        }
        _capturing = (found.Table, index);
        var button = found.Row.Key;
        button.Content = Text(TextKey.SettingsPressKey);
        // 刚重建的一页还没放进窗口：等按键框放好了再给它焦点（没有焦点时按键到不了这个窗口）。
        if (button.IsLoaded)
        {
            button.Focus(FocusState.Programmatic);
        }
        else
        {
            void Focus(object sender, RoutedEventArgs e)
            {
                button.Loaded -= Focus;
                button.Focus(FocusState.Programmatic);
            }
            button.Loaded += Focus;
        }
    }

    /// <summary>
    /// 快捷键一行录入了一个按键（按下的组合键，或小按钮里选的键；名称为空表示清除）。
    /// 系统占用的按键不接受，在这一行提示（ACTIONS 规则 6）。
    /// </summary>
    private void RecordKey(Row table, uint index, string name, KeyModifiers modifiers)
    {
        _capturing = null;
        var accepted = Set(table.Key, new SettingsInput.KeyRowKey(index, name, modifiers));
        if (!accepted && FindKeyRow(index) is { } found)
        {
            found.Row.Note.Text = Text(TextKey.SettingsKeyReserved);
            found.Row.Note.Visibility = Visibility.Visible;
        }
    }

    /// <summary>录下一个键以后，同一个键松开时不再交给按键框（空格松开时按钮会再"点"一次，又开始等按键）。</summary>
    private bool _swallowKeyUp;

    /// <summary>按键：正在录快捷键时交给那一行；Esc 关闭设置窗口，不退出程序。</summary>
    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (_capturing is { } capture)
        {
            var key = e.Key;
            // 输入法开着时按键可能被它拦下（ProcessKey）：按扫描码找回原来的键（与主窗口相同）。
            if (key == (VirtualKey)0xE5)
            {
                key = (VirtualKey)Win32.KeyFromScanCode(e.KeyStatus.ScanCode, e.KeyStatus.IsExtendedKey);
            }
            // 只按了修饰键：继续等。
            if (KeyNames.Name(key) is not string name)
            {
                return;
            }
            e.Handled = true;
            _swallowKeyUp = true;
            RecordKey(capture.Table, capture.Index, name, KeyNames.Modifiers());
            return;
        }
        if (e.Key == VirtualKey.Escape)
        {
            e.Handled = true;
            RequestClose();
        }
    }

    /// <summary>选了一个颜色（预设色块或颜色对话框）：写进草稿，色块外面那一圈跟着换。</summary>
    private void SetColor(string key, uint color)
    {
        foreach (var row in _rows.Where(r => r.Key == key && r.Kind == Kind.Color))
        {
            row.Color = color;
            foreach (var (preset, ring) in row.Swatches)
            {
                ring.Visibility = preset == color ? Visibility.Visible : Visibility.Collapsed;
            }
        }
        Set(key, new SettingsInput.Color(color));
    }

    // ---- 按钮 ----

    /// <summary>设置项里的按钮：问核心要做什么，再落到 Windows 上。</summary>
    public void PressButton(string key, string button)
    {
        switch (_core.SettingsButton(key, button))
        {
            case SettingsCommand.ChooseColor choose:
            {
                var initial = _rows.FirstOrDefault(r => r.Key == choose.Key && r.Kind == Kind.Color)?.Color ?? 0x202020;
                if (Win32.PickColor(_hwnd, initial) is uint color)
                {
                    SetColor(choose.Key, color);
                }
                break;
            }
            case SettingsCommand.ChooseFolder folder:
                _ = ChoosePath(folder.Key, folder: true, executable: false);
                break;
            case SettingsCommand.ChooseFile file:
                _ = ChoosePath(file.Key, folder: false, executable: false);
                break;
            // 图像编辑器：选一个程序。
            case SettingsCommand.ChooseApp app:
                _ = ChoosePath(app.Key, folder: false, executable: true);
                break;
            // 核心改了草稿（如编辑器的"用推荐的"）：重新显示这一页。
            case SettingsCommand.Refresh:
                ScheduleRebuild();
                break;
            // 书签页的"清除"：先确认。
            case SettingsCommand.ConfirmClearArchivePositions:
                _ = ConfirmClearPositions();
                break;
            case SettingsCommand.OpenFolder open:
                try
                {
                    Process.Start(new ProcessStartInfo("explorer.exe", $"\"{open.Path}\"") { UseShellExecute = false })?.Dispose();
                }
                catch (Exception)
                {
                }
                break;
            // 设为默认看图软件：macOS、Linux 的按钮，Windows 不会收到。
            case SettingsCommand.SetDefaultViewer:
                break;
            // 格式关联页的两个按钮：系统不让程序自己改默认程序，带用户到系统设置里选。
            case SettingsCommand.OpenDefaultApps:
                OpenSystemPage("default-apps", Win32.OpenDefaultApps);
                break;
            case SettingsCommand.OpenAssociationsByApp:
                OpenSystemPage("associations-by-app", Win32.OpenAssociationsByApp);
                break;
        }
    }

    /// <summary>打开系统设置里的一页；自检时只记下来，不真的打开。</summary>
    private void OpenSystemPage(string kind, Action open)
    {
        if (_owner is { IsSelfTest: true })
        {
            _owner.OnExternal?.Invoke(kind, "");
            return;
        }
        try
        {
            open();
        }
        catch (Exception)
        {
        }
    }

    /// <summary>选文件或文件夹，选好后填进这一项的输入框（输入框的变化会写进草稿）。</summary>
    private async Task ChoosePath(string key, bool folder, bool executable)
    {
        try
        {
            string? path;
            if (folder)
            {
                var picker = new FolderPicker();
                picker.FileTypeFilter.Add("*");
                InitializeWithWindow.Initialize(picker, _hwnd);
                path = (await picker.PickSingleFolderAsync())?.Path;
            }
            else
            {
                var picker = new FileOpenPicker();
                picker.FileTypeFilter.Add(executable ? ".exe" : "*");
                InitializeWithWindow.Initialize(picker, _hwnd);
                path = (await picker.PickSingleFileAsync())?.Path;
            }
            if (string.IsNullOrEmpty(path))
            {
                return;
            }
            foreach (var row in _rows.Where(r => r.Key == key && r.Field is not null))
            {
                row.Field!.Text = path;
            }
        }
        catch (Exception error)
        {
            Crash.Write("SettingsWindow.ChoosePath", error);
        }
    }

    /// <summary>忘掉所有压缩包内的位置之前先问一下。自检时直接清。</summary>
    private async Task ConfirmClearPositions()
    {
        if (Quiet)
        {
            _core.ClearArchivePositions();
            return;
        }
        var answer = await AskAsync(Text(TextKey.PromptClearPositions), cancelIsDefault: true, Text(TextKey.ButtonDelete), Text(TextKey.ButtonCancel));
        if (answer == 0)
        {
            _core.ClearArchivePositions();
        }
    }

    /// <summary>
    /// 这个窗口里的询问：返回按下的按钮序号（最后一个按钮是取消，Esc 也算取消）；已经有对话框开着时为空。
    /// cancelIsDefault：回车按的是取消（删除、放弃修改这类问题，按错了也不丢东西）。
    /// </summary>
    private async Task<int?> AskAsync(string message, bool cancelIsDefault, params string[] buttons)
    {
        if (_dialogOpen || Window.Content?.XamlRoot is null)
        {
            return null;
        }
        _dialogOpen = true;
        try
        {
            var dialog = new ContentDialog
            {
                XamlRoot = Window.Content.XamlRoot,
                Content = new TextBlock { Text = message, TextWrapping = TextWrapping.Wrap },
                PrimaryButtonText = buttons[0],
                CloseButtonText = buttons.Length > 1 ? buttons[^1] : "",
                SecondaryButtonText = buttons.Length > 2 ? buttons[1] : "",
                DefaultButton = cancelIsDefault && buttons.Length > 1 ? ContentDialogButton.Close : ContentDialogButton.Primary,
                Language = UiLanguage.Code(_lang),
            };
            return await dialog.ShowAsync() switch
            {
                ContentDialogResult.Primary => 0,
                ContentDialogResult.Secondary => 1,
                _ => buttons.Length - 1,
            };
        }
        finally
        {
            _dialogOpen = false;
        }
    }

    // ---- 恢复本页默认、确定、取消、关闭 ----

    /// <summary>"恢复本页默认"：这一页恢复默认值（还要点"确定"）。</summary>
    public void Reset()
    {
        _core.SettingsResetPage(_current);
        ShowPage(_current);
    }

    /// <summary>
    /// "确定"：校验通过就生效、关窗口；不通过就切到出错的页、标出那一项（快捷键标出那一行）并说明原因。
    /// 返回是否已生效。
    /// </summary>
    public bool Ok()
    {
        switch (_core.SettingsApply(_lang))
        {
            case SettingsApplyResult.Applied applied:
                LastError = null;
                _closing = true;
                Window.Close();
                OnApplied?.Invoke(applied);
                return true;
            case SettingsApplyResult.Invalid invalid:
                LastError = invalid.Message;
                SelectPage(invalid.Page);
                if (_rows.FirstOrDefault(r => r.Key == invalid.Key) is Row row)
                {
                    // 快捷键：在出错的那一行底下标红，用红字说明。
                    if (row.Kind == Kind.KeyMappings)
                    {
                        if (invalid.Row is uint bad && row.KeyRows.FirstOrDefault(w => w.Index == bad) is KeyRow w)
                        {
                            w.Frame.Background = Brush(Color.FromArgb(40, 232, 72, 72));
                            w.Note.Text = invalid.Message;
                            w.Note.Foreground = Brush(ErrorColor);
                            w.Note.Visibility = Visibility.Visible;
                        }
                    }
                    else
                    {
                        row.Frame.BorderBrush = Brush(ErrorColor);
                        row.Frame.BorderThickness = new Thickness(2);
                        row.Field?.Focus(FocusState.Programmatic);
                    }
                }
                if (!Quiet)
                {
                    _ = AskAsync(invalid.Message, cancelIsDefault: false, Text(TextKey.SettingsOk));
                }
                return false;
        }
        return false;
    }

    /// <summary>关闭（×、Esc）：有没确定的修改就先询问"放弃修改？"。</summary>
    public async void RequestClose()
    {
        if (_closing || !_core.SettingsDirty())
        {
            _closing = true;
            Window.Close();
            return;
        }
        // "放弃修改"在前、"继续编辑"是取消：回车、Esc 都是继续编辑，不会一按就丢了改动。
        var answer = await AskAsync(Text(TextKey.SettingsDiscardMessage), cancelIsDefault: true,
            Text(TextKey.SettingsDiscard), Text(TextKey.SettingsKeepEditing));
        if (answer == 0)
        {
            Discard();
        }
    }

    /// <summary>放弃修改并关窗口（"取消"、询问后选了"放弃修改"；自检也用它）。</summary>
    public void Discard()
    {
        _closing = true;
        Window.Close();
    }

    // ---- 自检 ----

    /// <summary>
    /// 像用户那样改一项（操作对应的控件，走同样的事件）。value 按设置文件里存的值写：开关为 true/false（翻过来说的开关
    /// 在这里换成开关的状态），下拉框为选项 ID，数字和路径为文字，颜色为 #RRGGBB；鼠标映射为 on|动作（动作为空时是
    /// "无操作"）或 off（下拉框的"默认"）；快捷键的一行写成 key#行=on|按键|动作（按键写法如 Ctrl+K，可省略后两段）
    /// 或 =off，映射表里这一行还没用过时先像"添加快捷键"那样把它加进列表。返回控件是否找到并且可以改。
    /// </summary>
    public bool SetForTest(string key, string value)
    {
        var parts = value.Split('|');
        var hash = key.IndexOf('#');
        if (hash > 0)
        {
            var baseKey = key[..hash];
            if (!uint.TryParse(key[(hash + 1)..], out var index) || parts[0] is not ("on" or "off"))
            {
                return false;
            }
            if (FindKeyRow(index) is null)
            {
                if (!Set(baseKey, new SettingsInput.KeyRowEnabled(index, true)))
                {
                    return false;
                }
                ShowPage(_current);
            }
            if (FindKeyRow(index) is not { } found || !found.Table.Container.IsEnabled)
            {
                return false;
            }
            found.Row.Switch.IsOn = parts[0] == "on";
            if (parts.Length > 1)
            {
                if (ParseKey(parts[1]) is not var (name, modifiers))
                {
                    return false;
                }
                RecordKey(found.Table, index, name, modifiers);
            }
            if (parts.Length > 2)
            {
                var position = found.Table.Ids.IndexOf(parts[2]);
                if (position < 0)
                {
                    return false;
                }
                // 录了按键以后可能整页重建过：重新取这一行的下拉框。
                (FindKeyRow(index)?.Row ?? found.Row).Combo.SelectedIndex = position;
            }
            return true;
        }
        if (_rows.FirstOrDefault(r => r.Key == key) is not Row row || !row.Container.IsEnabled)
        {
            return false;
        }
        switch (row.Kind)
        {
            case Kind.Mapping:
            {
                var id = parts[0] switch
                {
                    "off" => "default",
                    "on" => parts.Length > 1 ? parts[1] : "",
                    _ => null,
                };
                var position = id is null ? -1 : row.Ids.IndexOf(id);
                if (position < 0)
                {
                    return false;
                }
                row.Combo!.SelectedIndex = position;
                return true;
            }
            case Kind.Toggle:
                if (value is not ("true" or "false"))
                {
                    return false;
                }
                row.Switch!.IsOn = (value == "true") != row.Inverted;
                return true;
            case Kind.Choice:
            {
                var position = row.Ids.IndexOf(value);
                if (position < 0)
                {
                    return false;
                }
                row.Combo!.SelectedIndex = position;
                return true;
            }
            case Kind.Number:
            case Kind.Path:
                row.Field!.Text = value;
                // 文本框的 TextChanged 是异步的（之后才到）：这里马上写进草稿，下一步（如"确定"）不会赶在它前面。
                // 之后到的 TextChanged 写进同样的值，不算改动。
                Set(key, new SettingsInput.Text(value));
                return true;
            case Kind.Color:
                if (value.Length != 7 || value[0] != '#'
                    || !uint.TryParse(value[1..], System.Globalization.NumberStyles.HexNumber, null, out var color))
                {
                    return false;
                }
                SetColor(key, color);
                return true;
        }
        return false;
    }

    /// <summary>按键的写法（同动作目录，如 Ctrl+K、Shift+Tab、Q）→ 按键名称和修饰键。</summary>
    public static (string, KeyModifiers)? ParseKey(string spec)
    {
        bool shift = false, control = false, alt = false, command = false;
        var rest = spec;
        while (true)
        {
            if (rest.StartsWith("Shift+", StringComparison.Ordinal) && rest.Length > 6)
            {
                shift = true;
                rest = rest[6..];
            }
            else if (rest.StartsWith("Ctrl+", StringComparison.Ordinal) && rest.Length > 5)
            {
                control = true;
                rest = rest[5..];
            }
            else if (rest.StartsWith("Alt+", StringComparison.Ordinal) && rest.Length > 4)
            {
                alt = true;
                rest = rest[4..];
            }
            else if (rest.StartsWith("Win+", StringComparison.Ordinal) && rest.Length > 4)
            {
                command = true;
                rest = rest[4..];
            }
            else
            {
                break;
            }
        }
        // 步骤用逗号分隔：逗号、句号等写成名字。
        rest = rest switch
        {
            "Comma" => ",",
            "Period" => ".",
            "Plus" => "+",
            "Minus" => "-",
            "Slash" => "/",
            _ => rest,
        };
        return rest.Length == 0 ? null : (rest, new KeyModifiers(shift, control, alt, command));
    }

    /// <summary>这一页的标题、各张卡片的组名，每一项的键、文字、在哪张卡片、是否可用和值（自检报告用）。</summary>
    public Dictionary<string, object?> PageSummary()
    {
        if (_core.SettingsPage(_current, _lang) is not SettingsPage page)
        {
            return new();
        }
        var items = page.Items.Select(item => new Dictionary<string, object?>
        {
            ["key"] = item.Key,
            ["label"] = item.Label,
            ["section"] = item.Section,
            ["enabled"] = item.Enabled,
            ["value"] = item.Control switch
            {
                SettingsControl.Toggle t => t.On,
                SettingsControl.Choice c => c.Selected,
                SettingsControl.Number n => n.Text,
                SettingsControl.Path p => p.Text,
                SettingsControl.Color c => $"#{c.Value:X6}",
                SettingsControl.Mapping m => m.Selected,
                // 快捷键：列出来的行（用过的）写成"on/off 按键 动作 提示"。
                SettingsControl.KeyMappings k => k.Rows
                    .Select(r => $"{(r.Enabled ? "on" : "off")} {r.KeyLabel} {r.Action} {r.Note}".Trim()).ToArray(),
                _ => null,
            },
        }).ToList();
        return new() { ["id"] = page.Id, ["title"] = page.Title, ["sections"] = page.Sections, ["items"] = items };
    }

    /// <summary>把设置窗口的内容画成 PNG（自检报告用）。</summary>
    public Task<string> SnapshotAsync(string path) => SubWindows.SnapshotAsync(_root, path);
}
