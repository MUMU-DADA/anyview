// 帮助窗口："键盘快捷键""支持的格式"：两列的表，分组标题加粗，内容由核心给出（快捷键是实际生效的按键，含键盘映射）。
// "开源许可"没有表，是一段文字（Anyview 和它包含的第三方软件的许可证原文，几百 KB）：放在只读的文字框里，
// 文字框自己滚动、只排出看得见的部分，打开不卡。只读，文字可以选中复制；Esc 和关闭按钮一样。
using System;
using System.Threading.Tasks;
using Anyview.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.System;

namespace Anyview;

internal sealed class HelpWindow
{
    public readonly Window Window;
    public readonly HelpKind Kind;
    public HelpRow[] Rows { get; private set; } = Array.Empty<HelpRow>();
    /// <summary>开源许可那样只有一段文字的帮助窗口：那段文字；表格式的为空。</summary>
    public string? Text => _text?.Text;
    private readonly TextBox? _text;
    private readonly Grid _table = new() { ColumnSpacing = 16, RowSpacing = 3, Margin = new Thickness(16, 12, 16, 12) };
    public Action? OnClosed;
    public bool IsClosed { get; private set; }

    public HelpWindow(HelpKind kind, AppCore core, Lang lang, MainWindow? owner)
    {
        Kind = kind;
        UIElement body;
        if (core.HelpText(kind, lang) is string text)
        {
            _text = new TextBox
            {
                IsReadOnly = true,
                AcceptsReturn = true,
                TextWrapping = TextWrapping.Wrap,
                BorderThickness = new Thickness(0),
                Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(Microsoft.UI.Colors.Transparent),
                Padding = new Thickness(16, 12, 16, 12),
                // 许可证原文按 80 列排版（缩进、对齐、手动折行）：等宽字体，窗口放得下 80 列。
                FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Consolas"),
                FontSize = 12,
            };
            // 先设成多行再给文字：单行的文字框收到文字时只留第一个换行之前的部分。
            _text.Text = text;
            ScrollViewer.SetVerticalScrollBarVisibility(_text, ScrollBarVisibility.Auto);
            body = _text;
        }
        else
        {
            _table.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(214) });
            _table.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            Fill(core, lang);
            body = new ScrollViewer { Content = _table, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled };
        }
        var host = new UserControl { Content = body, IsTabStop = false };
        host.PreviewKeyDown += (_, e) =>
        {
            if (e.Key == VirtualKey.Escape)
            {
                e.Handled = true;
                Window?.Close();
            }
        };
        Window = SubWindows.Create(core.HelpTitle(kind, lang), _text is not null ? 680 : 560, 600, owner, host, lang);
        Window.Closed += (_, _) =>
        {
            IsClosed = true;
            OnClosed?.Invoke();
        };
    }

    /// <summary>
    /// 设置生效以后（键盘映射、语言可能变了）重新取内容和标题：开着的帮助窗口不再显示旧的按键。
    /// </summary>
    public void Refresh(AppCore core, Lang lang)
    {
        if (IsClosed)
        {
            return;
        }
        if (_text is not null)
        {
            _text.Text = core.HelpText(Kind, lang) ?? "";
        }
        else
        {
            Fill(core, lang);
        }
        Window.Title = core.HelpTitle(Kind, lang);
        if (Window.Content is FrameworkElement root)
        {
            UiLanguage.Apply(root, lang);
        }
    }

    /// <summary>按核心给的行排出两列的表（分组标题加粗、占两列）。</summary>
    private void Fill(AppCore core, Lang lang)
    {
        Rows = core.HelpRows(Kind, lang);
        _table.Children.Clear();
        _table.RowDefinitions.Clear();
        for (var i = 0; i < Rows.Length; i++)
        {
            var row = Rows[i];
            _table.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            if (row.Heading)
            {
                var heading = new TextBlock
                {
                    Text = row.Left,
                    FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                    Margin = new Thickness(0, i == 0 ? 0 : 10, 0, 2),
                    IsTextSelectionEnabled = true,
                };
                Grid.SetRow(heading, i);
                Grid.SetColumnSpan(heading, 2);
                _table.Children.Add(heading);
                continue;
            }
            var left = new TextBlock { Text = row.Left, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
            var right = new TextBlock { Text = row.Right, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
            Grid.SetRow(left, i);
            Grid.SetRow(right, i);
            Grid.SetColumn(right, 1);
            _table.Children.Add(left);
            _table.Children.Add(right);
        }
    }

    public void Show() => Window.Activate();

    /// <summary>原来归 oldOwner 所有时改归 newOwner（oldOwner 要关掉了）。</summary>
    public void Reown(nint oldOwner, nint newOwner) => SubWindows.Reown(Window, oldOwner, newOwner);

    /// <summary>把窗口的内容画成 PNG（自检报告用）。</summary>
    public Task<string> SnapshotAsync(string path) => SubWindows.SnapshotAsync(_text is not null ? _text : _table, path);
}
