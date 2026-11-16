// 打印：系统的打印对话框（PrintDlgEx），选好后用 GDI 把图片按纸张的可打印区域等比放到最大、居中打印。
// 横图默认用横向纸（对话框里还可以改）。透明处按白纸处理（打印机不认透明）。对话框在界面线程上（模态），
// 把像素交给打印机在后台线程做，界面不卡。
using System;
using System.Runtime.InteropServices;
using System.Threading.Tasks;

namespace Anyview;

internal static partial class Printing
{
    public enum Result
    {
        Printed,
        Cancelled,
        Failed,
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct PRINTDLGEX
    {
        public uint lStructSize;
        public nint hwndOwner;
        public nint hDevMode;
        public nint hDevNames;
        public nint hDC;
        public uint Flags;
        public uint Flags2;
        public uint ExclusionFlags;
        public uint nPageRanges;
        public uint nMaxPageRanges;
        public nint lpPageRanges;
        public uint nMinPage;
        public uint nMaxPage;
        public uint nCopies;
        public nint hInstance;
        public nint lpPrintTemplateName;
        public nint lpCallback;
        public uint nPropertyPages;
        public nint lphPropertyPages;
        public uint nStartPage;
        public uint dwResultAction;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct DOCINFO
    {
        public int cbSize;
        public nint lpszDocName;
        public nint lpszOutput;
        public nint lpszDatatype;
        public uint fwType;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BITMAPINFOHEADER
    {
        public uint biSize;
        public int biWidth;
        public int biHeight;
        public ushort biPlanes;
        public ushort biBitCount;
        public uint biCompression;
        public uint biSizeImage;
        public int biXPelsPerMeter;
        public int biYPelsPerMeter;
        public uint biClrUsed;
        public uint biClrImportant;
    }

    [LibraryImport("comdlg32.dll", EntryPoint = "PrintDlgExW")]
    private static partial int PrintDlgEx(ref PRINTDLGEX dialog);

    [LibraryImport("gdi32.dll", EntryPoint = "StartDocW")]
    private static partial int StartDoc(nint hdc, ref DOCINFO info);

    [LibraryImport("gdi32.dll")]
    private static partial int StartPage(nint hdc);

    [LibraryImport("gdi32.dll")]
    private static partial int EndPage(nint hdc);

    [LibraryImport("gdi32.dll")]
    private static partial int EndDoc(nint hdc);

    [LibraryImport("gdi32.dll")]
    private static partial int AbortDoc(nint hdc);

    [LibraryImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool DeleteDC(nint hdc);

    [LibraryImport("gdi32.dll")]
    private static partial int GetDeviceCaps(nint hdc, int index);

    [LibraryImport("gdi32.dll")]
    private static partial int SetStretchBltMode(nint hdc, int mode);

    [LibraryImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool SetBrushOrgEx(nint hdc, int x, int y, nint previous);

    [LibraryImport("gdi32.dll")]
    private static unsafe partial int StretchDIBits(nint hdc, int x, int y, int width, int height, int srcX, int srcY,
        int srcWidth, int srcHeight, byte* bits, ref BITMAPINFOHEADER info, uint usage, uint rop);

    [LibraryImport("kernel32.dll")]
    private static partial nint GlobalLock(nint memory);

    [LibraryImport("kernel32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool GlobalUnlock(nint memory);

    [LibraryImport("kernel32.dll")]
    private static partial nint GlobalFree(nint memory);

    private const uint PD_NOSELECTION = 0x4, PD_NOPAGENUMS = 0x8, PD_RETURNDC = 0x100, PD_RETURNDEFAULT = 0x400,
        PD_USEDEVMODECOPIESANDCOLLATE = 0x40000, PD_HIDEPRINTTOFILE = 0x100000, PD_NOCURRENTPAGE = 0x800000;
    private const uint START_PAGE_GENERAL = 0xFFFFFFFF, PD_RESULT_PRINT = 1;

    /// <summary>
    /// 打印一张图（BGRA、预乘透明度，第一行是最上面一行）：先弹系统的打印对话框（模态，在界面线程调用），
    /// 选好后在单独的线程上交给打印机。这个线程不是后台线程：窗口都关了、程序要退出时，也等它把作业交完
    /// （交给慢的网络打印机可能要好一会儿）。name 是打印队列里显示的名字。
    /// </summary>
    public static Task<(Result Result, string? Error)> PrintAsync(nint owner, string name, byte[] pixels, int width, int height)
    {
        if (ShowDialog(owner, width > height) is not nint hdc)
        {
            return Task.FromResult<(Result, string?)>((Result.Cancelled, null));
        }
        var done = new TaskCompletionSource<(Result, string?)>(TaskCreationOptions.RunContinuationsAsynchronously);
        var thread = new System.Threading.Thread(() =>
        {
            try
            {
                done.SetResult(Draw(hdc, name, pixels, width, height));
            }
            catch (Exception error)
            {
                done.SetResult((Result.Failed, error.Message));
            }
            finally
            {
                DeleteDC(hdc);
            }
        })
        { IsBackground = false, Name = "anyview-print" };
        thread.Start();
        return done.Task;
    }

    /// <summary>系统的打印对话框；按了"打印"时返回打印机的设备上下文（用完 DeleteDC），取消时为空。</summary>
    private static nint? ShowDialog(nint owner, bool landscape)
    {
        var dialog = new PRINTDLGEX
        {
            lStructSize = (uint)Marshal.SizeOf<PRINTDLGEX>(),
            hwndOwner = owner,
            nStartPage = START_PAGE_GENERAL,
        };
        try
        {
            // 先取默认打印机的设置，把纸张方向按图片改好（横图用横向纸），再弹对话框。没有打印机时直接弹（对话框里能添加）。
            LoadDefaults(ref dialog, landscape);
            // 不提供"打印到文件"（要另外问文件名、把 "FILE:" 交给打印作业；打成 PDF 用系统的"Microsoft Print to PDF"打印机）。
            dialog.Flags = PD_RETURNDC | PD_NOPAGENUMS | PD_NOSELECTION | PD_NOCURRENTPAGE | PD_USEDEVMODECOPIESANDCOLLATE
                | PD_HIDEPRINTTOFILE;
            dialog.nCopies = 1;
            dialog.hDC = 0;
            if (PrintDlgEx(ref dialog) != 0 || dialog.dwResultAction != PD_RESULT_PRINT || dialog.hDC == 0)
            {
                if (dialog.hDC != 0)
                {
                    DeleteDC(dialog.hDC);
                }
                return null;
            }
            return dialog.hDC;
        }
        finally
        {
            if (dialog.hDevMode != 0)
            {
                GlobalFree(dialog.hDevMode);
            }
            if (dialog.hDevNames != 0)
            {
                GlobalFree(dialog.hDevNames);
            }
        }
    }

    /// <summary>
    /// 取默认打印机的设置（不弹对话框），把纸张方向按图片改好；返回 PrintDlgEx 的结果（0 是成功）。没有打印机时
    /// 成功但 hDevMode 为 0。PD_RETURNDEFAULT 要和 PD_NOPAGENUMS 一起给：没给页码范围的缓冲区又不带它时，
    /// PrintDlgEx 直接返回 E_INVALIDARG——以前只给 PD_RETURNDEFAULT，
    /// 这一步一直失败，横图也照打印机原来的方向（一般是竖向）。
    /// </summary>
    private static int LoadDefaults(ref PRINTDLGEX dialog, bool landscape)
    {
        dialog.Flags = PD_RETURNDEFAULT | PD_NOPAGENUMS;
        var result = PrintDlgEx(ref dialog);
        if (result == 0 && dialog.hDevMode != 0)
        {
            SetOrientation(dialog.hDevMode, landscape);
        }
        return result;
    }

    /// <summary>
    /// 自检用：只做"取默认打印机的设置、改纸张方向"这一步（不弹对话框、不打印），返回 "ok:2"（改成了横向）、
    /// "ok:1"（竖向）、"none"（没有默认打印机）或 "error:0x…"。
    /// </summary>
    public static string ProbeDefaults(nint owner, bool landscape)
    {
        var dialog = new PRINTDLGEX
        {
            lStructSize = (uint)Marshal.SizeOf<PRINTDLGEX>(),
            hwndOwner = owner,
            nStartPage = START_PAGE_GENERAL,
        };
        try
        {
            var result = LoadDefaults(ref dialog, landscape);
            if (result != 0)
            {
                return $"error:0x{result:X8}";
            }
            if (dialog.hDevMode == 0)
            {
                return "none";
            }
            var p = GlobalLock(dialog.hDevMode);
            if (p == 0)
            {
                return "error:lock";
            }
            try
            {
                return $"ok:{Marshal.ReadInt16(p, 76)}";
            }
            finally
            {
                GlobalUnlock(dialog.hDevMode);
            }
        }
        catch (Exception error)
        {
            return $"error:{error.GetType().Name}";
        }
        finally
        {
            if (dialog.hDevMode != 0)
            {
                GlobalFree(dialog.hDevMode);
            }
            if (dialog.hDevNames != 0)
            {
                GlobalFree(dialog.hDevNames);
            }
        }
    }

    /// <summary>改 DEVMODE 里的纸张方向（dmFields 在第 72 字节，dmOrientation 在第 76 字节）。</summary>
    private static void SetOrientation(nint devMode, bool landscape)
    {
        const int DM_ORIENTATION = 0x1, DMORIENT_PORTRAIT = 1, DMORIENT_LANDSCAPE = 2;
        var p = GlobalLock(devMode);
        if (p == 0)
        {
            return;
        }
        try
        {
            Marshal.WriteInt32(p, 72, Marshal.ReadInt32(p, 72) | DM_ORIENTATION);
            Marshal.WriteInt16(p, 76, (short)(landscape ? DMORIENT_LANDSCAPE : DMORIENT_PORTRAIT));
        }
        finally
        {
            GlobalUnlock(devMode);
        }
    }

    /// <summary>在打印机上画一页：透明处按白纸，按可打印区域等比放到最大、居中。</summary>
    private static unsafe (Result, string?) Draw(nint hdc, string name, byte[] pixels, int width, int height)
    {
        const int HORZRES = 8, VERTRES = 10, LOGPIXELSX = 88, LOGPIXELSY = 90, HALFTONE = 4;
        const uint DIB_RGB_COLORS = 0, SRCCOPY = 0x00CC0020;
        // 预乘透明度的像素叠到白底上：c + (255 - a)。
        for (var i = 0; i + 3 < pixels.Length; i += 4)
        {
            var white = (byte)(255 - pixels[i + 3]);
            pixels[i] = (byte)(pixels[i] + white);
            pixels[i + 1] = (byte)(pixels[i + 1] + white);
            pixels[i + 2] = (byte)(pixels[i + 2] + white);
            pixels[i + 3] = 255;
        }
        var title = Marshal.StringToHGlobalUni(name);
        try
        {
            var doc = new DOCINFO { cbSize = Marshal.SizeOf<DOCINFO>(), lpszDocName = title };
            if (StartDoc(hdc, ref doc) <= 0)
            {
                return (Result.Failed, "StartDoc");
            }
            if (StartPage(hdc) <= 0)
            {
                AbortDoc(hdc);
                return (Result.Failed, "StartPage");
            }
            var pageWidth = GetDeviceCaps(hdc, HORZRES);
            var pageHeight = GetDeviceCaps(hdc, VERTRES);
            // 打印机横竖的分辨率可能不同（如 600x1200 dpi）：按英寸算等比放大，再分别换回两个方向的点数。
            var dpiX = Math.Max(1, GetDeviceCaps(hdc, LOGPIXELSX));
            var dpiY = Math.Max(1, GetDeviceCaps(hdc, LOGPIXELSY));
            var scale = Math.Min((double)pageWidth / dpiX / width, (double)pageHeight / dpiY / height);
            var w = Math.Max(1, Math.Min(pageWidth, (int)Math.Round(width * scale * dpiX)));
            var h = Math.Max(1, Math.Min(pageHeight, (int)Math.Round(height * scale * dpiY)));
            SetStretchBltMode(hdc, HALFTONE);
            SetBrushOrgEx(hdc, 0, 0, 0);
            var header = new BITMAPINFOHEADER
            {
                biSize = (uint)Marshal.SizeOf<BITMAPINFOHEADER>(),
                biWidth = width,
                // 负数：第一行是最上面一行。
                biHeight = -height,
                biPlanes = 1,
                biBitCount = 32,
            };
            int lines;
            fixed (byte* bits = pixels)
            {
                lines = StretchDIBits(hdc, (pageWidth - w) / 2, (pageHeight - h) / 2, w, h, 0, 0, width, height,
                    bits, ref header, DIB_RGB_COLORS, SRCCOPY);
            }
            if (lines == 0)
            {
                AbortDoc(hdc);
                return (Result.Failed, "StretchDIBits");
            }
            if (EndPage(hdc) <= 0)
            {
                AbortDoc(hdc);
                return (Result.Failed, "EndPage");
            }
            return EndDoc(hdc) <= 0 ? (Result.Failed, "EndDoc") : (Result.Printed, null);
        }
        finally
        {
            Marshal.FreeHGlobal(title);
        }
    }
}
