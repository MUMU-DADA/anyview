// 少量 Win32 调用：WinUI 没有对应接口的部分（去掉标题栏、鼠标按键的状态、按键对应的字符、回收站、桌面壁纸、阻止屏保、
// 翻页音效、界面语言）。
using System;
using System.Runtime.InteropServices;
using Anyview.Core;
using Windows.Graphics;

namespace Anyview;

internal static partial class Win32
{
    [LibraryImport("user32.dll")]
    public static partial uint GetDpiForWindow(nint hwnd);

    [LibraryImport("kernel32.dll")]
    public static partial uint GetCurrentThreadId();

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT
    {
        public int X;
        public int Y;
    }

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool GetCursorPos(out POINT point);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool SetForegroundWindow(nint hwnd);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool ShowWindow(nint hwnd, int command);

    /// <summary>最小化（不管现在用的是哪种呈现方式，全屏时也行）。</summary>
    public static void Minimize(nint hwnd)
    {
        const int SW_MINIMIZE = 6;
        ShowWindow(hwnd, SW_MINIMIZE);
    }

    /// <summary>把最小化的窗口取回（全屏的窗口用；普通窗口用呈现方式自己的 Restore）。</summary>
    public static void Restore(nint hwnd)
    {
        const int SW_RESTORE = 9;
        ShowWindow(hwnd, SW_RESTORE);
    }

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool IsIconic(nint hwnd);

    /// <summary>窗口是不是最小化着：直接问系统，普通窗口、全屏的窗口（最小化时不经过呈现方式）都一样。</summary>
    public static bool IsMinimized(nint hwnd) => IsIconic(hwnd);

    private static void OpenSettingsPage(string uri)
    {
        System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(uri) { UseShellExecute = true })?.Dispose();
    }

    /// <summary>“格式关联”：打开系统设置的“默认应用”。系统不让程序自己改默认程序，只能带用户去选。</summary>
    public static void OpenDefaultApps() => OpenSettingsPage("ms-settings:defaultapps");

    /// <summary>
    /// “经典方法”：逐个扩展名选。Windows 11 上这个地址直接打开“默认应用”里 Anyview 的那一页（Anyview 是登记时
    /// 写进 RegisteredApplications 的名字）；Windows 10 不认后面的参数，打开的是“默认应用”页（在部分 Windows 环境下
    /// Windows 10 上实测），要自己在页面最下面点“按应用设置默认值”。系统另有一个按程序打开那一页的接口
    /// （LaunchAdvancedAssociationUI），在这台 Windows 10 上调用后一直不返回，不用它。
    /// </summary>
    public static void OpenAssociationsByApp() => OpenSettingsPage("ms-settings:defaultapps?registeredAppUser=Anyview");

    [LibraryImport("user32.dll")]
    private static partial short GetAsyncKeyState(int key);

    [LibraryImport("user32.dll")]
    public static partial int GetSystemMetrics(int index);

    /// <summary>
    /// 鼠标左键（用户心目中的左键：左右键对调时是右边那个）现在是不是按着的。读的是此刻的状态，
    /// 不是界面线程正在处理的那条消息当时的状态。
    /// </summary>
    public static bool LeftButtonDown()
    {
        const int VK_LBUTTON = 0x01, VK_RBUTTON = 0x02, SM_SWAPBUTTON = 23;
        var key = GetSystemMetrics(SM_SWAPBUTTON) != 0 ? VK_RBUTTON : VK_LBUTTON;
        return (GetAsyncKeyState(key) & 0x8000) != 0;
    }

    // ---- 附属窗口（设置、选择图片、帮助） ----

    [LibraryImport("user32.dll", EntryPoint = "SetWindowLongPtrW")]
    private static partial nint SetWindowLongPtr(nint hwnd, int index, nint value);

    [LibraryImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
    private static partial nint GetWindowLongPtr(nint hwnd, int index);

    private const int GWLP_HWNDPARENT = -8;

    /// <summary>让 hwnd 归 owner 所有：总在 owner 上面，随 owner 最小化（与 macOS 的附属窗口相同）。</summary>
    public static void SetOwner(nint hwnd, nint owner) => SetWindowLongPtr(hwnd, GWLP_HWNDPARENT, owner);

    /// <summary>hwnd 归哪个窗口所有（没有时为 0）。</summary>
    public static nint GetOwner(nint hwnd) => GetWindowLongPtr(hwnd, GWLP_HWNDPARENT);

    // ---- 取色（设置里的颜色，系统的颜色对话框） ----

    [StructLayout(LayoutKind.Sequential)]
    private struct CHOOSECOLOR
    {
        public uint lStructSize;
        public nint hwndOwner;
        public nint hInstance;
        public uint rgbResult;
        public nint lpCustColors;
        public uint Flags;
        public nint lCustData;
        public nint lpfnHook;
        public nint lpTemplateName;
    }

    [LibraryImport("comdlg32.dll", EntryPoint = "ChooseColorW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool ChooseColor(ref CHOOSECOLOR cc);

    /// <summary>对话框里"自定义颜色"那一排（程序运行期间记着）。</summary>
    private static readonly uint[] s_customColors = new uint[16];

    /// <summary>系统的颜色对话框（0xRRGGBB）；取消时为空。对话框开着时这一句不返回（模态）。</summary>
    public static uint? PickColor(nint owner, uint rgb)
    {
        const uint CC_RGBINIT = 0x1, CC_FULLOPEN = 0x2;
        static uint Swap(uint c) => ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
        var pin = GCHandle.Alloc(s_customColors, GCHandleType.Pinned);
        try
        {
            var cc = new CHOOSECOLOR
            {
                lStructSize = (uint)Marshal.SizeOf<CHOOSECOLOR>(),
                hwndOwner = owner,
                rgbResult = Swap(rgb & 0xFFFFFF),
                lpCustColors = pin.AddrOfPinnedObject(),
                Flags = CC_RGBINIT | CC_FULLOPEN,
            };
            return ChooseColor(ref cc) ? Swap(cc.rgbResult & 0xFFFFFF) : null;
        }
        finally
        {
            pin.Free();
        }
    }

    // ---- 键盘 ----

    [LibraryImport("user32.dll")]
    public static partial short GetKeyState(int key);

    /// <summary>按下的键：0x8000 位是"现在按着"。</summary>
    public static bool IsDown(int key) => (GetKeyState(key) & 0x8000) != 0;

    public const int VK_SHIFT = 0x10;
    public const int VK_CONTROL = 0x11;
    public const int VK_MENU = 0x12;
    public const int VK_LWIN = 0x5B;
    public const int VK_RWIN = 0x5C;

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int ToUnicodeEx(uint virtualKey, uint scanCode, byte[] keyState,
        [Out, MarshalAs(UnmanagedType.LPArray)] char[] buffer, int bufferSize, uint flags, nint layout);

    [LibraryImport("user32.dll")]
    private static partial nint GetKeyboardLayout(uint thread);

    [LibraryImport("user32.dll")]
    private static partial uint MapVirtualKeyW(uint code, uint mapType);

    /// <summary>
    /// 按当前键盘布局，这个键（只算 Shift，不算 Ctrl、Alt）打出的字符；没有字符（功能键等）时为空。
    /// 与 Linux 的做法相同：Shift 已经体现在字符里（如 Shift+= 是 "+"），修饰键另外传给核心。
    /// </summary>
    public static string? KeyText(uint virtualKey, bool shift)
    {
        var state = new byte[256];
        if (shift)
        {
            state[VK_SHIFT] = 0x80;
        }
        var buffer = new char[8];
        var scan = MapVirtualKeyW(virtualKey, 0);
        // 标志 4：不改变键盘的状态（不会吞掉死键）。
        var n = ToUnicodeEx(virtualKey, scan, state, buffer, buffer.Length, 4, GetKeyboardLayout(0));
        if (n <= 0)
        {
            return null;
        }
        var text = new string(buffer, 0, n);
        return char.IsControl(text[0]) ? null : text;
    }

    /// <summary>输入法拦下的键（VirtualKey.ProcessKey）按扫描码找回原来的键。</summary>
    public static uint KeyFromScanCode(uint scanCode, bool extended)
    {
        const uint MAPVK_VSC_TO_VK_EX = 3;
        return MapVirtualKeyW(extended ? scanCode | 0xE000 : scanCode, MAPVK_VSC_TO_VK_EX);
    }

    // ---- 回收站 ----

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct SHFILEOPSTRUCT
    {
        public nint hwnd;
        public uint wFunc;
        [MarshalAs(UnmanagedType.LPWStr)] public string pFrom;
        [MarshalAs(UnmanagedType.LPWStr)] public string? pTo;
        public ushort fFlags;
        [MarshalAs(UnmanagedType.Bool)] public bool fAnyOperationsAborted;
        public nint hNameMappings;
        [MarshalAs(UnmanagedType.LPWStr)] public string? lpszProgressTitle;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    private static extern int SHFileOperationW(ref SHFILEOPSTRUCT op);

    /// <summary>删除的结果：移进了回收站、用户取消了、失败。</summary>
    public enum TrashResult
    {
        Done,
        Cancelled,
        Failed,
    }

    /// <summary>
    /// 移进回收站。平常不弹系统的确认框（确认由程序按设置自己问）；文件进不了回收站（网络上的文件夹、文件太大）、
    /// 会被直接删掉时，系统照常先警告，可以取消（FOF_WANTNUKEWARNING），不会不声不响地永久删除。
    /// </summary>
    public static (TrashResult Result, string? Error) MoveToRecycleBin(string path, nint owner)
    {
        const uint FO_DELETE = 3;
        const ushort FOF_SILENT = 0x0004, FOF_NOCONFIRMATION = 0x0010, FOF_ALLOWUNDO = 0x0040,
            FOF_NOERRORUI = 0x0400, FOF_WANTNUKEWARNING = 0x4000;
        var op = new SHFILEOPSTRUCT
        {
            hwnd = owner,
            wFunc = FO_DELETE,
            pFrom = path + "\0\0",
            fFlags = FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOF_SILENT | FOF_NOERRORUI,
        };
        var rc = SHFileOperationW(ref op);
        if (op.fAnyOperationsAborted)
        {
            return (TrashResult.Cancelled, null);
        }
        return rc == 0 ? (TrashResult.Done, null) : (TrashResult.Failed, $"0x{rc:X}");
    }

    // ---- 桌面背景 ----

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool SystemParametersInfoW(uint action, uint param, string value, uint winIni);

    public static bool SetWallpaper(string path)
    {
        const uint SPI_SETDESKWALLPAPER = 0x0014, SPIF_UPDATEINIFILE = 0x01, SPIF_SENDCHANGE = 0x02;
        return SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, path, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
    }

    // ---- 阻止屏保和显示器睡眠 ----

    [LibraryImport("kernel32.dll")]
    private static partial uint SetThreadExecutionState(uint flags);

    [LibraryImport("comctl32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static unsafe partial bool SetWindowSubclass(nint hwnd,
        delegate* unmanaged<nint, uint, nint, nint, nuint, nuint, nint> proc, nuint id, nuint refData);

    [LibraryImport("comctl32.dll")]
    private static partial nint DefSubclassProc(nint hwnd, uint msg, nint wParam, nint lParam);

    private static volatile bool s_blockScreenSaver;

    private static readonly System.Collections.Generic.HashSet<nint> s_captionless = new();

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool SetWindowPos(nint hwnd, nint after, int x, int y, int cx, int cy, uint flags);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool IsZoomed(nint hwnd);

    /// <summary>
    /// 去掉（或恢复）系统的标题栏（"显示标题栏"关着时）：WM_NCCALCSIZE 时让客户区盖住标题栏，左、右、下的边框照旧
    /// （拉边框能改大小，有阴影）。最大化时窗口比屏幕大出一圈边框，顶边也让出同样宽，内容不被屏幕切掉。
    /// 不用 WinUI 的 ExtendsContentIntoTitleBar：它要等 WinUI 自己的显示设备建好才返回，冷启动多等约 170 ms。
    /// 最小化、最大化、关闭按钮由程序自己画（CaptionButtons）。
    /// </summary>
    public static void SetCaptionless(nint hwnd, bool captionless)
    {
        const uint SWP_NOSIZE = 0x0001, SWP_NOMOVE = 0x0002, SWP_NOZORDER = 0x0004, SWP_NOACTIVATE = 0x0010, SWP_FRAMECHANGED = 0x0020;
        var changed = captionless ? s_captionless.Add(hwnd) : s_captionless.Remove(hwnd);
        if (changed)
        {
            SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    /// <summary>窗口关掉了：不再记着它（不改窗口框）。</summary>
    public static void ForgetCaptionless(nint hwnd) => s_captionless.Remove(hwnd);

    /// <summary>把窗口外框放到 (x, y)、大小 width x height（物理像素，屏幕坐标；顶边带子改大小时用）。</summary>
    public static void SetWindowRect(nint hwnd, int x, int y, int width, int height)
    {
        const uint SWP_NOZORDER = 0x0004, SWP_NOACTIVATE = 0x0010;
        SetWindowPos(hwnd, 0, x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
    }

    /// <summary>
    /// 在窗口过程前面加一层：要阻止屏保时，系统要启动屏保前发给前台窗口的 WM_SYSCOMMAND（SC_SCREENSAVE）直接吃掉。
    /// SetThreadExecutionState 只让显示器不睡，挡不住屏保（微软的文档如此）。
    /// </summary>
    public static unsafe void InstallScreenSaverGuard(nint hwnd)
    {
        SetWindowSubclass(hwnd, &SubclassProc, 1, 0);
    }

    /// <summary>用户用系统的方式（拖标题栏、拖边框）移动、改大小：开始时传 true，松开鼠标时传 false。按窗口登记。</summary>
    private static readonly System.Collections.Generic.Dictionary<nint, Action<bool>> s_sizeMove = new();

    /// <summary>登记（handler 为空时取消）窗口的"用户开始/结束移动、改大小"通知（吸附边缘）。只在界面线程调用。</summary>
    public static void OnSizeMove(nint hwnd, Action<bool>? handler)
    {
        if (handler is null)
        {
            s_sizeMove.Remove(hwnd);
        }
        else
        {
            s_sizeMove[hwnd] = handler;
        }
    }

    [UnmanagedCallersOnly]
    private static nint SubclassProc(nint hwnd, uint msg, nint wParam, nint lParam, nuint id, nuint refData)
    {
        const uint WM_SYSCOMMAND = 0x0112, WM_ENTERSIZEMOVE = 0x0231, WM_EXITSIZEMOVE = 0x0232, WM_NCCALCSIZE = 0x0083;
        // 去掉标题栏（SetCaptionless）：系统照常算客户区（有标题栏、边框），再把顶边挪回窗口顶上；最大化时让出一圈边框的宽度
        // （与左边框一样宽）。lParam 是 NCCALCSIZE_PARAMS，开头是客户区的 RECT（左、上、右、下）。
        if (msg == WM_NCCALCSIZE && wParam != 0 && s_captionless.Contains(hwnd))
        {
            var left = Marshal.ReadInt32(lParam, 0);
            var top = Marshal.ReadInt32(lParam, 4);
            var result = DefSubclassProc(hwnd, msg, wParam, lParam);
            var border = IsZoomed(hwnd) ? Math.Max(Marshal.ReadInt32(lParam, 0) - left, 0) : 0;
            Marshal.WriteInt32(lParam, 4, top + border);
            return result;
        }
        const long SC_SCREENSAVE = 0xF140;
        if (msg == WM_SYSCOMMAND && s_blockScreenSaver && ((long)wParam & 0xFFF0) == SC_SCREENSAVE)
        {
            return 0;
        }
        // 拖动记录（DragProbe）：移动窗口时（程序自己移动、系统的移动循环）每一步记下时间和位置。只有打开了记录时才有事做。
        const uint WM_WINDOWPOSCHANGED = 0x0047;
        if (msg == WM_WINDOWPOSCHANGED && DragProbe.Recording)
        {
            DragProbe.Moved(lParam);
        }
        if ((msg == WM_ENTERSIZEMOVE || msg == WM_EXITSIZEMOVE) && s_sizeMove.TryGetValue(hwnd, out var handler))
        {
            // 这里漏出去的异常会让程序直接崩溃：接住、记下来。拖动记录先结束再交给 handler：
            // 松开后吸附边缘的那一下移动不算拖动中的一步；记录出了错也不影响吸附。
            try
            {
                if (msg == WM_ENTERSIZEMOVE)
                {
                    DragProbe.Enter(hwnd);
                }
                else
                {
                    DragProbe.Exit();
                }
            }
            catch (Exception error)
            {
                Crash.Write("DragProbe", error);
            }
            try
            {
                handler(msg == WM_ENTERSIZEMOVE);
            }
            catch (Exception error)
            {
                Crash.Write("Win32.SizeMove", error);
            }
        }
        return DefSubclassProc(hwnd, msg, wParam, lParam);
    }

    /// <summary>
    /// 阻止屏保和显示器睡眠（运行时）：显示器不睡（SetThreadExecutionState，在界面线程调用，系统按线程记），
    /// 本程序的窗口在前台时屏保也不启动（InstallScreenSaverGuard）。
    /// </summary>
    public static void KeepDisplayOn(bool on)
    {
        const uint ES_CONTINUOUS = 0x80000000, ES_DISPLAY_REQUIRED = 0x00000002;
        s_blockScreenSaver = on;
        SetThreadExecutionState(on ? ES_CONTINUOUS | ES_DISPLAY_REQUIRED : ES_CONTINUOUS);
    }

    // ---- 屏幕的可用区域 ----

    [StructLayout(LayoutKind.Sequential)]
    private struct MONITORINFO
    {
        public uint cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
    }

    [LibraryImport("user32.dll")]
    private static partial nint MonitorFromWindow(nint hwnd, uint flags);

    [LibraryImport("user32.dll")]
    private static partial nint MonitorFromPoint(POINT point, uint flags);

    [LibraryImport("user32.dll", EntryPoint = "GetMonitorInfoW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool GetMonitorInfo(nint monitor, ref MONITORINFO info);

    private const uint MONITOR_DEFAULTTOPRIMARY = 1, MONITOR_DEFAULTTONEAREST = 2;

    /// <summary>
    /// 屏幕的可用区域（去掉任务栏），屏幕坐标、物理像素，与 AppWindow 的位置同一个坐标系。
    /// 不用 DisplayArea.WorkArea：文档说它的位置是相对于那块屏幕的，用在副屏上会算错。
    /// </summary>
    private static RectInt32 WorkAreaOf(nint monitor)
    {
        var info = new MONITORINFO { cbSize = (uint)Marshal.SizeOf<MONITORINFO>() };
        if (!GetMonitorInfo(monitor, ref info))
        {
            return new RectInt32(0, 0, 1280, 720);
        }
        var r = info.rcWork;
        return new RectInt32(r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top);
    }

    /// <summary>窗口所在（或离它最近）的那块屏幕的可用区域。</summary>
    public static RectInt32 WorkAreaOfWindow(nint hwnd) => WorkAreaOf(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST));

    /// <summary>某一点所在（或离它最近）的那块屏幕的可用区域。</summary>
    public static RectInt32 WorkAreaAt(int x, int y) => WorkAreaOf(MonitorFromPoint(new POINT { X = x, Y = y }, MONITOR_DEFAULTTONEAREST));

    /// <summary>主屏幕的可用区域。</summary>
    public static RectInt32 PrimaryWorkArea() => WorkAreaOf(MonitorFromPoint(new POINT(), MONITOR_DEFAULTTOPRIMARY));

    // ---- 显示器的 ICC 配置文件（"使用监视器的 ICC 设置"）----

    [StructLayout(LayoutKind.Sequential)]
    private unsafe struct MONITORINFOEX
    {
        public uint cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
        public fixed char szDevice[32];
    }

    // 结构里有定长的字符数组：按指针传，不经过封送（与 SelfTestDrag 的 PROCESSENTRY32W 相同）。
    [LibraryImport("user32.dll", EntryPoint = "GetMonitorInfoW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static unsafe partial bool GetMonitorInfoEx(nint monitor, MONITORINFOEX* info);

    [LibraryImport("gdi32.dll", EntryPoint = "CreateDCW", StringMarshalling = StringMarshalling.Utf16)]
    private static partial nint CreateDC(string driver, string device, nint output, nint initData);

    [LibraryImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool DeleteDC(nint hdc);

    [LibraryImport("gdi32.dll", EntryPoint = "GetICMProfileW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static unsafe partial bool GetICMProfile(nint hdc, ref uint size, char* name);

    /// <summary>窗口所在显示器（句柄，用来比较是不是换了显示器）。</summary>
    public static nint MonitorOfWindow(nint hwnd) => MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);

    /// <summary>
    /// 窗口所在显示器在系统"颜色管理"里设的 ICC 配置文件的路径（GetICMProfile 按显示器的设备上下文取）；
    /// 取不到时为空。
    /// </summary>
    public static unsafe string? MonitorProfilePath(nint hwnd)
    {
        var info = new MONITORINFOEX { cbSize = (uint)sizeof(MONITORINFOEX) };
        if (!GetMonitorInfoEx(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &info))
        {
            return null;
        }
        var device = new string(info.szDevice);
        var hdc = CreateDC("DISPLAY", device, 0, 0);
        if (hdc == 0)
        {
            return null;
        }
        try
        {
            const int Capacity = 520;
            var buffer = stackalloc char[Capacity];
            uint size = Capacity;
            if (!GetICMProfile(hdc, ref size, buffer))
            {
                return null;
            }
            var path = new string(buffer);
            return path.Length == 0 ? null : path;
        }
        finally
        {
            DeleteDC(hdc);
        }
    }

    /// <summary>屏幕坐标 (x, y) 在不在某块显示器上（记住的窗口位置还能不能用，：那块显示器可能已经拔掉了）。</summary>
    public static bool IsOnScreen(int x, int y)
    {
        const uint MONITOR_DEFAULTTONULL = 0;
        return MonitorFromPoint(new POINT { X = x, Y = y }, MONITOR_DEFAULTTONULL) != 0;
    }

    // ---- 双击间隔 ----

    [LibraryImport("user32.dll")]
    public static partial uint GetDoubleClickTime();

    // ---- 界面语言 ----

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern bool GetUserPreferredUILanguages(uint flags, out uint count,
        [Out, MarshalAs(UnmanagedType.LPArray)] char[]? buffer, ref uint size);

    /// <summary>用户的显示语言，按偏好的先后（"zh-CN"、"en-US" 这样的名字）；取不到时为空（用第一种程序有的）。</summary>
    public static string[] UserLanguages()
    {
        const uint MUI_LANGUAGE_NAME = 0x8;
        uint size = 0;
        if (!GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, out _, null, ref size) || size == 0)
        {
            return Array.Empty<string>();
        }
        var buffer = new char[size];
        if (!GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, out _, buffer, ref size))
        {
            return Array.Empty<string>();
        }
        // 一串以空字符分开的名字，最后两个空字符。
        return new string(buffer, 0, (int)Math.Min(size, (uint)buffer.Length))
            .Split('\0', StringSplitOptions.RemoveEmptyEntries);
    }

    // ---- 看不见的拖动边框（吸附边缘） ----

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [LibraryImport("dwmapi.dll")]
    private static partial int DwmGetWindowAttribute(nint hwnd, uint attribute, out RECT value, uint size);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool GetWindowRect(nint hwnd, out RECT rect);

    /// <summary>窗口外框比看得见的窗口多出的部分（Windows 10/11 四周看不见的拖动边框）：左、上、右、下，物理像素。</summary>
    public static (int Left, int Top, int Right, int Bottom) InvisibleBorders(nint hwnd)
    {
        const uint DWMWA_EXTENDED_FRAME_BOUNDS = 9;
        if (!GetWindowRect(hwnd, out var outer)
            || DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, out var visible, (uint)Marshal.SizeOf<RECT>()) != 0)
        {
            return (0, 0, 0, 0);
        }
        return (Math.Max(visible.Left - outer.Left, 0), Math.Max(visible.Top - outer.Top, 0),
            Math.Max(outer.Right - visible.Right, 0), Math.Max(outer.Bottom - visible.Bottom, 0));
    }
}

/// <summary>翻页音效：核心合成的 WAV，整个程序共用一份，固定在内存里（异步播放期间不能移动）。</summary>
internal static class PageSound
{
    private static byte[]? s_wav;
    private static GCHandle s_pin;

    [DllImport("winmm.dll", EntryPoint = "PlaySoundW")]
    private static extern bool PlaySound(nint sound, nint module, uint flags);

    public static int Length => Wav().Length;

    private static byte[] Wav()
    {
        if (s_wav is null)
        {
            s_wav = AnyviewCore.PageSoundWav();
            s_pin = GCHandle.Alloc(s_wav, GCHandleType.Pinned);
        }
        return s_wav;
    }

    /// <summary>异步播放（下一次播放会打断上一次）。</summary>
    public static bool Play()
    {
        const uint SND_ASYNC = 0x0001, SND_NODEFAULT = 0x0002, SND_MEMORY = 0x0004;
        Wav();
        return PlaySound(s_pin.AddrOfPinnedObject(), 0, SND_ASYNC | SND_NODEFAULT | SND_MEMORY);
    }
}
