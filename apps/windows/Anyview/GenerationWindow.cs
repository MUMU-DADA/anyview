// “生成信息”面板：AI 生成的图片里记的提示词、反向提示词、参数，每段都能复制，最后可以复制文件里的原文。
// 内容由核心读出；开着时跟着打开它的那个看图窗口的当前图片换内容（悬浮界面刷新时调 Refresh）：换到还没读好的
// 图片时先不换（等读好了），界面语言变了也重新填。只读，文字可以选中；Esc 和关闭按钮一样。
using System;
using System.Threading.Tasks;
using Anyview.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.ApplicationModel.DataTransfer;
using Windows.System;

namespace Anyview;

internal sealed class GenerationWindow
{
    /// <summary>开着的面板（看图窗口刷新悬浮界面时找它）。</summary>
    public static GenerationWindow? Current { get; private set; }

    public readonly Window Window;
    public MainWindow Source { get; private set; }
    public bool IsClosed { get; private set; }
    public Action? OnClosed;
    /// <summary>现在显示的内容；还没填过时 Filled 为假。</summary>
    public GenerationInfo? Shown { get; private set; }
    private bool _filled;
    private Lang _lang;
    private readonly StackPanel _content = new() { Spacing = 8, Margin = new Thickness(16, 14, 16, 14) };

    public GenerationWindow(MainWindow source, Lang lang)
    {
        Source = source;
        _lang = lang;
        var scroller = new ScrollViewer { Content = _content, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled };
        var host = new UserControl { Content = scroller, IsTabStop = false };
        host.PreviewKeyDown += (_, e) =>
        {
            if (e.Key == VirtualKey.Escape)
            {
                e.Handled = true;
                Window?.Close();
            }
        };
        var texts = source.Core.GenerationTexts(lang);
        Window = SubWindows.Create(texts.Title, 520, 560, source, host, lang);
        Window.Closed += (_, _) =>
        {
            IsClosed = true;
            if (Current == this)
            {
                Current = null;
            }
            OnClosed?.Invoke();
        };
        Current = this;
        Refresh();
    }

    /// <summary>显示面板（已经开着时放到最前面），内容按这个看图窗口的当前图片。</summary>
    public void Show(MainWindow source, Lang lang)
    {
        if (!ReferenceEquals(Source, source))
        {
            // 改归这个看图窗口所有：不然原来那个窗口关掉、最小化时系统连这个面板一起关掉、藏起来。
            Reown(Source.Hwnd, source.Hwnd);
            _filled = false;
        }
        if (_lang != lang)
        {
            _filled = false;
        }
        Source = source;
        Refresh();
        Window.Activate();
    }

    /// <summary>按当前图片重新填内容；内容没变时不动（免得选中的文字被清掉），当前图片还在读时也不动。</summary>
    public void Refresh()
    {
        if (IsClosed || Source.Core.GenerationPending())
        {
            return;
        }
        var info = Source.Core.GenerationInfo();
        // 界面语言跟着看图窗口（设置里改了语言时它先换）。
        var lang = Source.Lang;
        if (_filled && _lang == lang && Same(Shown, info))
        {
            return;
        }
        _filled = true;
        if (_lang != lang && Window.Content is FrameworkElement root)
        {
            UiLanguage.Apply(root, lang);
        }
        _lang = lang;
        Shown = info;
        var texts = Source.Core.GenerationTexts(_lang);
        Window.Title = texts.Title;
        _content.Children.Clear();
        if (info is null)
        {
            _content.Children.Add(Text(texts.Empty, secondary: true));
            return;
        }
        _content.Children.Add(Text(texts.Tool.Replace("{tool}", info.Tool), secondary: true));
        Section(texts.Prompt, info.Prompt, texts);
        if (info.Negative.Length > 0)
        {
            Section(texts.Negative, info.Negative, texts);
        }
        if (info.Params.Length > 0)
        {
            _content.Children.Add(Heading(texts.Params, null));
            var grid = new Grid { ColumnSpacing = 16, RowSpacing = 4 };
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            for (var i = 0; i < info.Params.Length; i++)
            {
                grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
                var name = Text(info.Params[i].Name, secondary: true);
                var value = Text(info.Params[i].Value, secondary: false);
                Grid.SetRow(name, i);
                Grid.SetRow(value, i);
                Grid.SetColumn(value, 1);
                grid.Children.Add(name);
                grid.Children.Add(value);
            }
            _content.Children.Add(grid);
        }
        var raw = CopyButton(texts.CopyRaw, info.Raw, texts);
        raw.HorizontalAlignment = HorizontalAlignment.Right;
        raw.Margin = new Thickness(0, 10, 0, 0);
        _content.Children.Add(raw);
    }

    /// <summary>两次的内容一样不一样（按生成工具、提示词和原文认）。</summary>
    private static bool Same(GenerationInfo? a, GenerationInfo? b) =>
        a is null ? b is null : b is not null && a.Tool == b.Tool && a.Prompt == b.Prompt && a.Raw == b.Raw;

    /// <summary>一段文字：标题、右边的“复制”，下面是可以选中的文字。</summary>
    private void Section(string title, string text, GenerationTexts texts)
    {
        var heading = Heading(title, CopyButton(texts.Copy, text, texts));
        heading.Margin = new Thickness(0, 6, 0, 0);
        _content.Children.Add(heading);
        _content.Children.Add(Text(text, secondary: false));
    }

    private static Grid Heading(string title, Button? button)
    {
        var row = new Grid();
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var label = new TextBlock
        {
            Text = title,
            FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
            VerticalAlignment = VerticalAlignment.Center,
            IsTextSelectionEnabled = true,
        };
        row.Children.Add(label);
        if (button is not null)
        {
            Grid.SetColumn(button, 1);
            row.Children.Add(button);
        }
        return row;
    }

    private static TextBlock Text(string text, bool secondary)
    {
        var block = new TextBlock { Text = text, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = true };
        if (secondary)
        {
            block.Opacity = 0.7;
        }
        return block;
    }

    /// <summary>“复制”按钮：按下后把文字放进剪贴板，按钮上的字短暂换成“已复制”。</summary>
    private Button CopyButton(string label, string text, GenerationTexts texts)
    {
        var button = new Button { Content = label };
        button.Click += (_, _) =>
        {
            var package = new DataPackage();
            package.SetText(text);
            Clipboard.SetContent(package);
            Clipboard.Flush();
            button.Content = texts.Copied;
            Delay.After(Window.DispatcherQueue, 1200, () => button.Content = label);
        };
        return button;
    }

    /// <summary>原来归 oldOwner 所有时改归 newOwner（oldOwner 要关掉了）。</summary>
    public void Reown(nint oldOwner, nint newOwner) => SubWindows.Reown(Window, oldOwner, newOwner);

    /// <summary>把窗口的内容画成 PNG（自检报告用）。</summary>
    public Task<string> SnapshotAsync(string path) => SubWindows.SnapshotAsync(_content, path);
}
