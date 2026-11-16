// Anyview Windows 版：告诉 WinUI 界面文字是哪种语言。
using System.Collections.Generic;
using Anyview.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Anyview;

/// <summary>
/// 界面文字的语言：WinUI 的默认字体（XamlAutoFontFamily）按元素的 Language 挑字体，日文用 Yu Gothic UI、
/// 繁体中文用 Microsoft JhengHei UI、韩文用 Malgun Gothic。不设时按系统的语言挑：中文的 Windows 上日文假名、
/// 繁体字都用微软雅黑显示，假名之间空隙不匀。语言标签由核心给（BCP 47）。
/// 窗口的根设一次，里面的元素都跟着；菜单、悬停提示、对话框是另外弹出来的，各自设上。
/// </summary>
internal static class UiLanguage
{
    public static string Code(Lang lang) => AnyviewCore.LangCode(lang);

    public static void Apply(FrameworkElement element, Lang lang) => element.Language = Code(lang);

    /// <summary>菜单里的每一项（连子菜单里的）。</summary>
    public static void Apply(IList<MenuFlyoutItemBase> items, string code)
    {
        foreach (var item in items)
        {
            item.Language = code;
            if (item is MenuFlyoutSubItem sub)
            {
                Apply(sub.Items, code);
            }
        }
    }

    /// <summary>带语言的悬停提示（ToolTipService.SetToolTip 直接给文字时，系统建的提示不带语言）；text 为空时没有提示。</summary>
    public static ToolTip? Tip(string? text, Lang lang) =>
        text is null ? null : new ToolTip { Content = text, Language = Code(lang) };
}
