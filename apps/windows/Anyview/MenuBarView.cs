// 窗口顶端的菜单栏：设置"不显示菜单栏"关掉时显示，内容是核心的菜单栏数据（与 macOS 的菜单栏同一棵树）。
// 不用 WinUI 的 MenuBar 控件：它的样式要 XamlControlsResources（整个程序的控件样式都跟着换，冷启动还要多装一大份
// 资源；读过 WinUI 的源代码，MenuBar.xaml 引用的主题资源只在那里面）。这里是一排按钮，每个按钮按下时现建一个菜单，
// 用的是右键菜单的同一个生成器：勾选、灰显、快捷键都是按下那一刻的状态，不用另外同步。
using System;
using System.Collections.Generic;
using System.Linq;
using Anyview.Core;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Automation.Provider;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace Anyview;

internal sealed class MenuBarView
{
    /// <summary>菜单栏的高度（有效像素）：与标题栏隐藏时顶端那一条一样高，右上角的三个按钮正好排在这一行里。</summary>
    public const double Height = 32;

    /// <summary>整行：左边是各个菜单，右边空着（标题栏隐藏时那里用来拖动窗口）。</summary>
    public readonly Grid Row = new();

    private readonly StackPanel _buttons = new()
    {
        Orientation = Orientation.Horizontal,
        HorizontalAlignment = HorizontalAlignment.Left,
    };
    private readonly AppCore _core;
    private readonly Action<string> _invoke;
    private readonly Action<string> _open;
    private readonly Action _closed;
    private readonly List<(Button Button, string Title, MenuEntry[] Entries)> _menus = new();
    private MenuFlyout? _openFlyout;
    private Button? _openButton;
    private Lang _lang = Lang.En;

    /// <summary>
    /// invoke 执行一个动作，open 打开"打开最近使用"里的一个路径，closed 在菜单关掉以后叫（把键盘焦点还给画面）。
    /// </summary>
    public MenuBarView(AppCore core, Action<string> invoke, Action<string> open, Action closed)
    {
        _core = core;
        _invoke = invoke;
        _open = open;
        _closed = closed;
        Row.Height = Height;
        Row.Background = new SolidColorBrush(Color.FromArgb(255, 0x2B, 0x2B, 0x2B));
        Row.Children.Add(_buttons);
    }

    /// <summary>各个菜单一共占了多宽（有效像素）：它右边的空白才是拖动窗口的地方。</summary>
    public double MenusWidth => _buttons.ActualWidth;

    /// <summary>顶层各个菜单的名字（自检用）。</summary>
    public List<string> Titles => _menus.Select(m => m.Title).ToList();

    /// <summary>按现在的语言、键盘映射重新排一遍顶层的菜单（设置生效、换语言以后）。</summary>
    public void Rebuild(Lang lang)
    {
        CloseOpen();
        _lang = lang;
        _buttons.Children.Clear();
        _menus.Clear();
        // 顶层的每个子菜单是一个按钮；它里面的项（到配对的结束为止）留着，按下时现建。
        var depth = 0;
        string? title = null;
        var inner = new List<MenuEntry>();
        foreach (var entry in _core.MenuEntries(MenuKind.MenuBar, lang))
        {
            switch (entry)
            {
                case MenuEntry.SubmenuStart start:
                    if (depth == 0)
                    {
                        title = start.Title;
                        inner = new List<MenuEntry>();
                    }
                    else
                    {
                        inner.Add(entry);
                    }
                    depth++;
                    break;
                case MenuEntry.SubmenuEnd:
                    depth--;
                    if (depth == 0 && title is not null)
                    {
                        Add(title, inner.ToArray());
                        title = null;
                    }
                    else if (depth > 0)
                    {
                        inner.Add(entry);
                    }
                    break;
                default:
                    if (depth > 0)
                    {
                        inner.Add(entry);
                    }
                    break;
            }
        }
    }

    private void Add(string title, MenuEntry[] entries)
    {
        var button = new Button
        {
            Content = title,
            Height = Height,
            MinWidth = 0,
            Padding = new Thickness(10, 0, 10, 0),
            Background = new SolidColorBrush(Colors.Transparent),
            BorderThickness = new Thickness(0),
            CornerRadius = new CornerRadius(0),
            FontSize = 12,
            // 不拿键盘焦点：按键总是交给画面（核心）。
            IsTabStop = false,
        };
        button.Click += (_, _) =>
        {
            if (ReferenceEquals(_openButton, button))
            {
                CloseOpen();
            }
            else
            {
                Show(button);
            }
        };
        // 一个菜单开着时指针移到别的菜单上：换成那一个（菜单栏的习惯）。
        button.PointerEntered += (_, _) =>
        {
            if (_openFlyout is not null && !ReferenceEquals(_openButton, button))
            {
                Show(button);
            }
        };
        _buttons.Children.Add(button);
        _menus.Add((button, title, entries));
    }

    private MenuFlyout Build(MenuEntry[] entries) => MenuBuilder.Build(_core, entries, _lang, _invoke, FillRecent);

    /// <summary>"打开最近使用"：最近打开的排在前面（最近的在最上），后面是核心给的"清除菜单"。</summary>
    private void FillRecent(IList<MenuFlyoutItemBase> items)
    {
        var paths = _core.RecentFiles();
        foreach (var path in paths)
        {
            var item = new MenuFlyoutItem { Text = Shorten(path), Tag = RecentTag + path };
            item.Click += (_, _) => _open(path);
            items.Add(item);
        }
        if (paths.Length > 0)
        {
            items.Add(new MenuFlyoutSeparator());
        }
    }

    private const string RecentTag = "recent:";

    /// <summary>路径太长时中间用省略号（菜单不至于宽得出格）；文件名那一段留着。</summary>
    private static string Shorten(string path)
    {
        const int Limit = 64;
        if (path.Length <= Limit)
        {
            return path;
        }
        var tail = Math.Min(path.Length - path.LastIndexOfAny(new[] { '\\', '/' }), Limit - 12);
        tail = Math.Max(tail, Limit / 2);
        return string.Concat(path.AsSpan(0, Limit - tail - 1), "…", path.AsSpan(path.Length - tail));
    }

    private void Show(Button button)
    {
        CloseOpen();
        var entries = _menus.First(m => ReferenceEquals(m.Button, button)).Entries;
        var flyout = Build(entries);
        flyout.Placement = FlyoutPlacementMode.BottomEdgeAlignedLeft;
        // 菜单开着时这一行照常收得到指针：移到别的菜单上能换过去，再按一下这个按钮能关掉。
        flyout.OverlayInputPassThroughElement = Row;
        flyout.Closed += (_, _) =>
        {
            if (ReferenceEquals(_openFlyout, flyout))
            {
                _openFlyout = null;
                _openButton = null;
                _closed();
            }
        };
        _openFlyout = flyout;
        _openButton = button;
        flyout.ShowAt(button);
    }

    /// <summary>关掉开着的菜单（没有开着的就什么也不做）。</summary>
    public void CloseOpen()
    {
        var flyout = _openFlyout;
        _openFlyout = null;
        _openButton = null;
        flyout?.Hide();
    }

    // ---- 自检 ----

    /// <summary>各个菜单里的动作项（标题、快捷键、可用），和"打开最近使用"里现在列着的路径。</summary>
    public (List<(string Menu, string Id, string Title, string Shortcut, bool Enabled)> Items, List<string> Recent) Describe()
    {
        var items = new List<(string, string, string, string, bool)>();
        var recent = new List<string>();
        foreach (var (_, title, entries) in _menus)
        {
            foreach (var (id, text, shortcut, enabled) in MenuBuilder.Flatten(Build(entries)))
            {
                if (id.StartsWith(RecentTag, StringComparison.Ordinal))
                {
                    recent.Add(id[RecentTag.Length..]);
                }
                else
                {
                    items.Add((title, id, text, shortcut, enabled));
                }
            }
        }
        return (items, recent);
    }

    /// <summary>
    /// 像用户那样用一次菜单：按下 id 所在的那个菜单的按钮，等菜单真的弹出来，再按里面的那一项（经 UI 自动化的
    /// "调用"，与点一下相同）。done 收到结果：null 是成功，否则是哪里不对。只找菜单的第一层（子菜单里的项不在这里试）。
    /// </summary>
    public void InvokeForTest(string id, Action<string?> done)
    {
        var menu = _menus.FirstOrDefault(m => m.Entries.Any(e => e is MenuEntry.Action a && a.Id == id));
        if (menu.Button is null)
        {
            done($"菜单栏里没有 {id}");
            return;
        }
        Show(menu.Button);
        var flyout = _openFlyout;
        if (flyout is null)
        {
            done("菜单没有弹出来");
            return;
        }
        var answered = false;
        flyout.Opened += (_, _) =>
        {
            if (answered)
            {
                return;
            }
            answered = true;
            var item = flyout.Items.OfType<MenuFlyoutItem>().FirstOrDefault(i => i.Tag is string tag && tag == id);
            if (item is null)
            {
                CloseOpen();
                done($"{menu.Title} 菜单的第一层里没有 {id}");
            }
            else if (!item.IsEnabled)
            {
                CloseOpen();
                done($"{id} 是灰的");
            }
            else
            {
                // 普通的项是"调用"，带勾的项（开关）是"切换"：都等于点一下。
                var peer = FrameworkElementAutomationPeer.CreatePeerForElement(item);
                if (peer is IInvokeProvider invoker)
                {
                    invoker.Invoke();
                    done(null);
                }
                else if (peer is IToggleProvider toggler)
                {
                    toggler.Toggle();
                    done(null);
                }
                else
                {
                    CloseOpen();
                    done($"{id} 没法调用");
                }
            }
        };
    }

    /// <summary>打开"打开最近使用"里的第 index 个（从 0 起），走的是点那一项时的同一个函数。没有这一个时返回 false。</summary>
    public bool OpenRecentForTest(int index)
    {
        var paths = _core.RecentFiles();
        if (index < 0 || index >= paths.Length)
        {
            return false;
        }
        _open(paths[index]);
        return true;
    }
}
