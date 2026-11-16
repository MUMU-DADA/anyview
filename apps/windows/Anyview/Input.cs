// 键名与右键菜单：按键交给核心按 Windows 键位解析（docs/ACTIONS.md），菜单由核心的数据生成。
using System;
using System.Collections.Generic;
using Anyview.Core;
using Microsoft.UI.Xaml.Controls;
using Windows.System;

namespace Anyview;

internal static class KeyNames
{
    /// <summary>核心认识的键名：功能键用固定的名字，其余按当前键盘布局打出的字符（含 Shift，与 Linux 相同）。</summary>
    public static string? Name(VirtualKey key)
    {
        var named = key switch
        {
            VirtualKey.Left => "Left",
            VirtualKey.Right => "Right",
            VirtualKey.Up => "Up",
            VirtualKey.Down => "Down",
            VirtualKey.Home => "Home",
            VirtualKey.End => "End",
            VirtualKey.PageUp => "PageUp",
            VirtualKey.PageDown => "PageDown",
            VirtualKey.Enter => "Enter",
            VirtualKey.Tab => "Tab",
            VirtualKey.Escape => "Escape",
            VirtualKey.Delete => "Delete",
            VirtualKey.Back => "Backspace",
            VirtualKey.Insert => "Insert",
            VirtualKey.Space => "Space",
            >= VirtualKey.F1 and <= VirtualKey.F24 => "F" + (key - VirtualKey.F1 + 1),
            // 小键盘：数字锁定打开时数字和小数点按上面印的字符，运算符总是；数字锁定关闭时系统给的就是方向、翻页、
            // Delete 等键（与系统、Linux 相同）。
            >= VirtualKey.NumberPad0 and <= VirtualKey.NumberPad9 => ((char)('0' + (key - VirtualKey.NumberPad0))).ToString(),
            VirtualKey.Add => "+",
            VirtualKey.Subtract => "-",
            VirtualKey.Multiply => "*",
            VirtualKey.Divide => "/",
            VirtualKey.Decimal => ".",
            _ => null,
        };
        if (named is not null)
        {
            return named;
        }
        if (key is VirtualKey.Shift or VirtualKey.Control or VirtualKey.Menu or VirtualKey.LeftWindows
            or VirtualKey.RightWindows or VirtualKey.LeftShift or VirtualKey.RightShift or VirtualKey.LeftControl
            or VirtualKey.RightControl or VirtualKey.LeftMenu or VirtualKey.RightMenu or VirtualKey.CapitalLock)
        {
            return null;
        }
        var text = Win32.KeyText((uint)key, Win32.IsDown(Win32.VK_SHIFT));
        return text?.ToLowerInvariant();
    }

    public static KeyModifiers Modifiers() => new(
        Win32.IsDown(Win32.VK_SHIFT),
        Win32.IsDown(Win32.VK_CONTROL),
        Win32.IsDown(Win32.VK_MENU),
        Win32.IsDown(Win32.VK_LWIN) || Win32.IsDown(Win32.VK_RWIN));
}

internal static class MenuBuilder
{
    /// <summary>
    /// 按核心的菜单数据生成右键菜单：子菜单、分隔线、勾选、灰显都是当时的状态；
    /// 快捷键按键盘映射显示实际生效的按键（ACTIONS 规则 7）。
    /// </summary>
    public static MenuFlyout Build(AppCore core, MenuEntry[] entries, Lang lang, Action<string> invoke,
        Action<IList<MenuFlyoutItemBase>>? fillRecent = null)
    {
        var flyout = new MenuFlyout();
        var stack = new Stack<IList<MenuFlyoutItemBase>>();
        stack.Push(flyout.Items);
        foreach (var entry in entries)
        {
            var items = stack.Peek();
            switch (entry)
            {
                case MenuEntry.SubmenuStart start:
                    var sub = new MenuFlyoutSubItem { Text = start.Title };
                    items.Add(sub);
                    stack.Push(sub.Items);
                    // 菜单栏的"打开最近使用"：最近打开的由调用方先填进去，后面接核心给的"清除菜单"。
                    if (start.Role == MenuRole.Recent)
                    {
                        fillRecent?.Invoke(sub.Items);
                    }
                    break;
                case MenuEntry.SubmenuEnd:
                    if (stack.Count > 1)
                    {
                        stack.Pop();
                    }
                    break;
                case MenuEntry.Separator:
                    if (items.Count > 0 && items[^1] is not MenuFlyoutSeparator)
                    {
                        items.Add(new MenuFlyoutSeparator());
                    }
                    break;
                case MenuEntry.Action action:
                    var state = core.ActionState(action.Id);
                    MenuFlyoutItem item = action.Checkable
                        ? new ToggleMenuFlyoutItem { IsChecked = state.Checked }
                        : new MenuFlyoutItem();
                    item.Text = action.Title;
                    item.IsEnabled = state.Enabled;
                    item.Tag = action.Id;
                    if (action.Shortcut is { Label.Length: > 0 } shortcut)
                    {
                        item.KeyboardAcceleratorTextOverride = shortcut.Label;
                    }
                    var id = action.Id;
                    item.Click += (_, _) => invoke(id);
                    items.Add(item);
                    break;
            }
        }
        TrimSeparators(flyout.Items);
        UiLanguage.Apply(flyout.Items, UiLanguage.Code(lang));
        return flyout;
    }

    /// <summary>每一层末尾的分隔线去掉（核心按分组给出，组后面没有别的项时会多出一条）。</summary>
    private static void TrimSeparators(IList<MenuFlyoutItemBase> items)
    {
        while (items.Count > 0 && items[^1] is MenuFlyoutSeparator)
        {
            items.RemoveAt(items.Count - 1);
        }
        foreach (var item in items)
        {
            if (item is MenuFlyoutSubItem sub)
            {
                TrimSeparators(sub.Items);
            }
        }
    }

    /// <summary>菜单里所有动作项（自检核对用）：标识、标题、快捷键、可用。"打开最近使用"里的路径也在内（标识以 recent: 开头）。</summary>
    public static List<(string Id, string Title, string Shortcut, bool Enabled)> Flatten(MenuFlyout flyout)
    {
        var list = new List<(string, string, string, bool)>();
        void Walk(IList<MenuFlyoutItemBase> items)
        {
            foreach (var item in items)
            {
                if (item is MenuFlyoutSubItem sub)
                {
                    Walk(sub.Items);
                }
                else if (item is MenuFlyoutItem m && m.Tag is string id)
                {
                    list.Add((id, m.Text, m.KeyboardAcceleratorTextOverride ?? "", m.IsEnabled));
                }
            }
        }
        Walk(flyout.Items);
        return list;
    }
}
