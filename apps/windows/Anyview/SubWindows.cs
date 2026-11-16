// 附属窗口（设置、选择图片、帮助）的共用部分：建窗口、放在主窗口正中、归主窗口所有、深色标题栏、自检截图。
using System;
using System.IO;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Threading.Tasks;
using Anyview.Core;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Graphics;
using Windows.Graphics.Imaging;
using Windows.Storage;
using Windows.Storage.Streams;
using WinRT.Interop;

namespace Anyview;

internal static class SubWindows
{
    /// <summary>
    /// 窗口的图标（标题栏、任务栏、Alt+Tab）：输出目录里的 Assets/Anyview.ico。没有这个文件或设不上时照旧用系统的。
    /// </summary>
    public static void ApplyIcon(Microsoft.UI.Windowing.AppWindow appWindow)
    {
        try
        {
            var icon = System.IO.Path.Combine(AppContext.BaseDirectory, "Assets", "Anyview.ico");
            if (System.IO.File.Exists(icon))
            {
                appWindow.SetIcon(icon);
            }
        }
        catch (Exception error)
        {
            Crash.Write("SubWindows.ApplyIcon", error);
        }
    }

    /// <summary>
    /// 建一个附属窗口：内容区 width x height 有效像素，放在 owner 的正中（屏幕放不下时挪进可用区域），归 owner 所有
    /// （总在它上面，随它最小化）。还没显示，调用方 Activate。
    /// </summary>
    public static Window Create(string title, double width, double height, MainWindow? owner, FrameworkElement content, Lang lang)
    {
        UiLanguage.Apply(content, lang);
        var window = new Window { Title = title, Content = content };
        var hwnd = WindowNative.GetWindowHandle(window);
        window.AppWindow.TitleBar.PreferredTheme = TitleBarTheme.Dark;
        ApplyIcon(window.AppWindow);
        RectInt32 area;
        double cx, cy;
        if (owner is not null)
        {
            Win32.SetOwner(hwnd, owner.Hwnd);
            var position = owner.AppWindow.Position;
            var ownerSize = owner.AppWindow.Size;
            cx = position.X + ownerSize.Width / 2.0;
            cy = position.Y + ownerSize.Height / 2.0;
            area = Win32.WorkAreaOfWindow(owner.Hwnd);
        }
        else
        {
            area = Win32.PrimaryWorkArea();
            cx = area.X + area.Width / 2.0;
            cy = area.Y + area.Height / 2.0;
        }
        // 先挪到要放的那块屏幕上，再按这个窗口自己的 DPI 定大小。反过来（按所有者的 DPI 定好大小再挪过去）时，
        // 两块屏幕的缩放不同，WinUI 收到 DPI 变化会再缩放一次，窗口差不多大了一倍。
        var initial = window.AppWindow.Size;
        window.AppWindow.Move(new PointInt32((int)Math.Round(cx - initial.Width / 2.0), (int)Math.Round(cy - initial.Height / 2.0)));
        var dpi = Win32.GetDpiForWindow(hwnd);
        var scale = dpi == 0 ? 1.0 : dpi / 96.0;
        window.AppWindow.ResizeClient(new SizeInt32((int)Math.Round(width * scale), (int)Math.Round(height * scale)));
        var size = window.AppWindow.Size;
        var x = (int)Math.Round(cx - size.Width / 2.0);
        var y = (int)Math.Round(cy - size.Height / 2.0);
        x = Math.Max(area.X, Math.Min(x, area.X + area.Width - size.Width));
        y = Math.Max(area.Y, Math.Min(y, area.Y + area.Height - size.Height));
        window.AppWindow.Move(new PointInt32(x, y));
        return window;
    }

    /// <summary>
    /// 原来归 oldOwner 所有的附属窗口改归 newOwner（oldOwner 要关掉了：不改的话系统会随它一起关掉这个窗口）。
    /// </summary>
    public static void Reown(Window window, nint oldOwner, nint newOwner)
    {
        var hwnd = WindowNative.GetWindowHandle(window);
        if (Win32.GetOwner(hwnd) == oldOwner)
        {
            Win32.SetOwner(hwnd, newOwner);
        }
    }

    /// <summary>把一个界面元素画成 PNG（自检报告用）。返回文件名；画不了时返回原因。</summary>
    public static async Task<string> SnapshotAsync(UIElement element, string path)
    {
        try
        {
            var bitmap = new RenderTargetBitmap();
            await bitmap.RenderAsync(element);
            var pixels = (await bitmap.GetPixelsAsync()).ToArray();
            // 文件还不存在：要说明新建（只给 FileAccessMode 的那个重载要求文件已经在）。
            using var stream = await FileRandomAccessStream.OpenAsync(path, FileAccessMode.ReadWrite,
                StorageOpenOptions.None, FileOpenDisposition.CreateAlways);
            var encoder = await BitmapEncoder.CreateAsync(BitmapEncoder.PngEncoderId, stream);
            encoder.SetPixelData(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied,
                (uint)bitmap.PixelWidth, (uint)bitmap.PixelHeight, 96, 96, pixels);
            await encoder.FlushAsync();
            return Path.GetFileName(path);
        }
        catch (Exception error)
        {
            return $"截不了图：{error.Message}";
        }
    }
}
