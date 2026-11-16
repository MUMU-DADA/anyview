// Anyview 的 Windows 版入口。
// 用法：Anyview.exe [图片或文件夹] [--settings 设置文件] [--drag-log 拖动记录目录] [--self-test 报告目录 [--steps 步骤] [--sample-at-100]]
// 核心在建窗口之前就建好、开始读图（与 macOS、Linux 相同）。已经开着 Anyview 时把图片交给它后退出（Instance）。
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text;
using System.Threading;
using Anyview.Core;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace Anyview;

/// <summary>命令行参数。</summary>
internal sealed class LaunchOptions
{
    public string? OpenPath { get; private set; }
    public string? SettingsPath { get; private set; }
    /// <summary>安装、卸载程序要做的事（--register、--unregister、--stop-hosts）；平常启动时为空。</summary>
    public string? Shell { get; private set; }
    /// <summary>--register 时安装程序给的版本号和装好后的大小（显示在系统的“应用和功能”里）。</summary>
    public string Version { get; private set; } = "";
    public uint SizeKb { get; private set; }
    /// <summary>自检的报告目录（--self-test）。</summary>
    public string? SelfTestDir { get; private set; }
    /// <summary>自检的步骤（--steps，逗号分隔，与 macOS 的自检相同的写法）。</summary>
    public string[] Steps { get; private set; } = Array.Empty<string>();
    /// <summary>只在 100% 时核对画面（真实照片缩小显示时，滤波后的颜色本来就与单个像素不同）。</summary>
    public bool SampleAt100 { get; private set; }
    /// <summary>手动测试时的窗口拖动采样日志目录（通过 --drag-log 指定）。</summary>
    public string? DragLog { get; private set; }

    public static LaunchOptions Parse(string[] args)
    {
        var o = new LaunchOptions();
        for (var i = 0; i < args.Length; i++)
        {
            var value = i + 1 < args.Length ? args[i + 1] : null;
            switch (args[i])
            {
                case "--settings": o.SettingsPath = value; i++; break;
                case "--self-test": o.SelfTestDir = value; i++; break;
                case "--open": o.OpenPath = value; i++; break;
                case "--steps": o.Steps = (value ?? "").Split(',', StringSplitOptions.RemoveEmptyEntries); i++; break;
                case "--sample-at-100": o.SampleAt100 = true; break;
                case "--drag-log": o.DragLog = value; i++; break;
                // 安装、卸载程序调用：登记到系统、撤销登记、结束占着核心 DLL 的缩略图进程。不开窗口。
                case "--register": o.Shell = "register"; break;
                case "--unregister": o.Shell = "unregister"; break;
                case "--stop-hosts": o.Shell = "stop-hosts"; break;
                case "--version": o.Version = value ?? ""; i++; break;
                case "--size-kb": _ = uint.TryParse(value, out var kb); o.SizeKb = kb; i++; break;
                default:
                    // 资源管理器"打开方式"直接把文件路径放在参数里。
                    if (!args[i].StartsWith("--", StringComparison.Ordinal))
                    {
                        o.OpenPath = args[i];
                    }
                    break;
            }
        }
        // 自检用报告目录里的设置文件，不碰用户自己的设置。
        if (o.SelfTestDir is not null && o.SettingsPath is null)
        {
            o.SettingsPath = Path.Combine(o.SelfTestDir, "settings.toml");
        }
        // 免安装版：程序旁边有 portable.txt 时，设置、最近打开的等都放在旁边的 data 文件夹里，不写进系统。
        if (o.SettingsPath is null)
        {
            o.SettingsPath = AnyviewCore.WindowsPortableSettingsPath(AppContext.BaseDirectory.TrimEnd('\\', '/'));
        }
        return o;
    }
}

/// <summary>启动的各个时刻：从进程创建（系统记的时间）算起的毫秒数（冷启动耗时分解）。</summary>
internal static class LaunchClock
{
    private static readonly DateTime s_start = Process.GetCurrentProcess().StartTime.ToUniversalTime();
    private static readonly List<(string Name, double Ms)> s_marks = new();

    public static double Now => (DateTime.UtcNow - s_start).TotalMilliseconds;

    public static void Mark(string name)
    {
        lock (s_marks)
        {
            if (!s_marks.Exists(m => m.Name == name))
            {
                s_marks.Add((name, Now));
            }
        }
    }

    public static double? Get(string name)
    {
        lock (s_marks)
        {
            foreach (var (n, ms) in s_marks)
            {
                if (n == name)
                {
                    return ms;
                }
            }
        }
        return null;
    }

    /// <summary>所有时刻（自检报告用）。</summary>
    public static List<Dictionary<string, object>> Marks()
    {
        lock (s_marks)
        {
            return s_marks.ConvertAll(m => new Dictionary<string, object> { ["name"] = m.Name, ["ms"] = Math.Round(m.Ms, 1) });
        }
    }

    public static string Summary()
    {
        var sb = new StringBuilder();
        lock (s_marks)
        {
            foreach (var (n, ms) in s_marks)
            {
                if (sb.Length > 0) sb.Append(" → ");
                sb.Append(n).Append(' ').Append(ms.ToString("0", CultureInfo.InvariantCulture));
            }
        }
        return sb.ToString();
    }
}

/// <summary>没接住的异常写进日志（自检时写进报告目录的 crash.txt，否则写进临时目录）。</summary>
internal static class Crash
{
    public static string? Directory { get; set; }
    /// <summary>自检用：每记一次异常就通知（自检把它算作问题，不会带着异常通过）。</summary>
    public static Action<string>? Observer;

    /// <summary>
    /// 自检时记下最近几次抛出的 .NET 异常（带调用栈）。WinUI 交来的异常常常只剩"未指定的错误"、没有调用栈，
    /// 真正出错的地方要从这里找。
    /// </summary>
    private static readonly Queue<string> s_recent = new();
    private static readonly object s_recentLock = new();
    [ThreadStatic]
    private static bool t_recording;

    public static void WatchFirstChance()
    {
        AppDomain.CurrentDomain.FirstChanceException += (_, e) =>
        {
            // 这里自己再出异常会又进来：同一个线程上不重入。
            if (t_recording)
            {
                return;
            }
            t_recording = true;
            try
            {
                var error = e.Exception;
                var text = $"[{DateTime.Now:HH:mm:ss.fff}] {error.GetType().FullName} 0x{error.HResult:X8}: {error.Message}\n{new System.Diagnostics.StackTrace(1, false)}";
                lock (s_recentLock)
                {
                    s_recent.Enqueue(text);
                    while (s_recent.Count > 12)
                    {
                        s_recent.Dequeue();
                    }
                }
            }
            catch (Exception)
            {
            }
            finally
            {
                t_recording = false;
            }
        };
    }

    /// <summary>记一次异常。detail：另外的说明（如 WinUI 的 UnhandledExceptionEventArgs.Message）。</summary>
    public static void Write(string where, object? error, string? detail = null)
    {
        Observer?.Invoke($"{where}: {(error as Exception)?.Message ?? error?.ToString()}");
        try
        {
            var text = new StringBuilder();
            text.Append('[').Append(DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture)).Append("] ").Append(where).Append('\n');
            if (error is Exception exception)
            {
                text.Append($"0x{exception.HResult:X8} ");
            }
            text.Append(error).Append('\n');
            if (!string.IsNullOrWhiteSpace(detail) && detail != (error as Exception)?.Message)
            {
                text.Append("说明：").Append(detail).Append('\n');
            }
            string[] recent;
            lock (s_recentLock)
            {
                recent = s_recent.ToArray();
            }
            if (recent.Length > 0)
            {
                text.Append("之前抛出的异常（最近的在最后）：\n");
                foreach (var line in recent)
                {
                    text.Append(line).Append('\n');
                }
            }
            text.Append('\n');
            var dir = Directory ?? Path.GetTempPath();
            System.IO.Directory.CreateDirectory(dir);
            File.AppendAllText(Path.Combine(dir, Directory is null ? "anyview-crash.txt" : "crash.txt"), text.ToString(), new UTF8Encoding(false));
        }
        catch (IOException)
        {
        }
        catch (UnauthorizedAccessException)
        {
        }
    }
}

public static class Program
{
    internal static LaunchOptions Options { get; private set; } = new();

    /// <summary>标题栏用 WinUI 自己的办法（环境变量 ANYVIEW_TITLEBAR=winui）：只用来对比拖动窗口是否卡顿，见 MainWindow.WinUiTitleBar。</summary>
    internal static bool WinUiTitleBar { get; } =
        string.Equals(Environment.GetEnvironmentVariable("ANYVIEW_TITLEBAR"), "winui", StringComparison.OrdinalIgnoreCase);
    /// <summary>启动时就建好的核心（第一个窗口用它）。</summary>
    internal static CoreBridge? LaunchCore { get; private set; }
    internal static Lang SystemLang { get; private set; } = Lang.En;

    /// <summary>
    /// 系统的显示语言（"跟随系统"）：偏好列表里第一种程序有的，都没有用英文。程序用固定区域性
    /// （InvariantGlobalization，省启动时间和体积），CultureInfo 读不到用户的语言，直接问 Windows。
    /// </summary>
    private static Lang SystemLanguage() => AnyviewCore.LangFromLocales(Win32.UserLanguages());

    /// <summary>在后台建好画布用的显卡设备（Win2D 的共享设备）。</summary>
    public static void WarmUpGpu()
    {
        ThreadPool.QueueUserWorkItem(_ =>
        {
            try
            {
                LaunchClock.Mark("gpuStart");
                Microsoft.Graphics.Canvas.CanvasDevice.GetSharedDevice();
                LaunchClock.Mark("gpuReady");
            }
            catch (Exception error)
            {
                Crash.Write("CanvasDevice.GetSharedDevice", error);
            }
        });
    }

    /// <summary>程序所在的文件夹（末尾不带反斜杠）。</summary>
    private static string ProgramDir => AppContext.BaseDirectory.TrimEnd('\\', '/');

    /// <summary>
    /// 安装、卸载程序调用的三件事，做完就退出，不开窗口、不碰已经开着的 Anyview。写什么、怎么撤销都在核心里：
    /// --register 把这个文件夹里的 Anyview 登记到系统（按设置文件里的开关），--unregister 撤销，--stop-hosts 结束占着
    /// 核心 DLL 的缩略图进程（升级前文件才换得掉）。返回码 0 表示成功。
    /// </summary>
    private static int RunShellCommand(LaunchOptions options)
    {
        try
        {
            switch (options.Shell)
            {
                case "register":
                    var error = AnyviewCore.ShellRegister(ProgramDir, options.Version, options.SizeKb, options.SettingsPath,
                        SystemLanguage());
                    if (error is not null)
                    {
                        Crash.Write("--register", error);
                        return 1;
                    }
                    return 0;
                case "unregister":
                    AnyviewCore.ShellUnregister(true);
                    return 0;
                default:
                    AnyviewCore.ShellStopHosts();
                    return 0;
            }
        }
        catch (Exception error)
        {
            Crash.Write($"--{options.Shell}", error);
            return 1;
        }
    }

    /// <summary>这个程序是不是用安装程序装的：登记在系统里的安装位置就是它所在的文件夹。</summary>
    private static bool IsInstalled()
    {
        try
        {
            return AnyviewCore.ShellInstalledDir() is string dir
                && string.Equals(dir.TrimEnd('\\', '/'), ProgramDir, StringComparison.OrdinalIgnoreCase);
        }
        catch (Exception)
        {
            return false;
        }
    }

    /// <summary>设置变了以后（设置窗口点了“确定”）：装好的程序按新的开关重新登记右键菜单、缩略图。</summary>
    public static void RefreshShellRegistration(string? settingsPath)
    {
        if (Options.SelfTestDir is not null || !IsInstalled())
        {
            return;
        }
        try
        {
            if (AnyviewCore.ShellRefresh(settingsPath, SystemLang) is string error)
            {
                Crash.Write("ShellRefresh", error);
            }
        }
        catch (Exception error)
        {
            Crash.Write("ShellRefresh", error);
        }
    }

    [STAThread]
    private static int Main(string[] args)
    {
        LaunchClock.Mark("main");
        Options = LaunchOptions.Parse(args);
        if (Options.Shell is not null)
        {
            return RunShellCommand(Options);
        }
        Crash.Directory = Options.SelfTestDir;
        if (Options.SelfTestDir is not null)
        {
            Crash.WatchFirstChance();
        }
        // 拖动窗口的记录（DragProbe）：自检、手动测试传了 --drag-log 时才记。
        if (Options.SelfTestDir is not null || Options.DragLog is not null)
        {
            DragProbe.Enable(Options.DragLog);
        }
        AppDomain.CurrentDomain.UnhandledException += (_, e) => Crash.Write("AppDomain.UnhandledException", e.ExceptionObject);
        // 已经开着 Anyview 时把图片交给它、这个进程马上退出。自检时不交也不接（每次自检单独一个进程）。
        if (Options.SelfTestDir is null)
        {
            var path = Options.OpenPath is string p ? Path.GetFullPath(p) : null;
            if (!Instance.Start(Options.SettingsPath, path))
            {
                return 0;
            }
            LaunchClock.Mark("instanceChecked");
        }
        WinRT.ComWrappersSupport.InitializeComWrappers();
        // 画布用的显卡设备（Win2D 的共享设备，画布设了 UseSharedDevice）在后台先建好，与建核心、WinUI 启动、建窗口同时进行；
        // 尽早在后台线程预创建图形设备以缩短冷启动时间；若已转交单实例则跳过。
        WarmUpGpu();
        // HEIC、AVIF、JPEG XR 交给 Windows 自带的 WIC；要在打开任何图片之前登记。
        AnyviewCore.RegisterPlatformDecoder(new WicDecoder(), WicDecoder.Extensions);
        // 程序是不是用安装程序装的：设置窗口按它决定上下文菜单页显不显示、格式关联的按钮能不能用。
        // 自检可以用环境变量假装装了或没装（没装的测试版也能检查那几页）。
        AnyviewCore.SetWindowsInstalled(Environment.GetEnvironmentVariable("ANYVIEW_TEST_INSTALLED") switch
        {
            "1" when Options.SelfTestDir is not null => true,
            "0" when Options.SelfTestDir is not null => false,
            _ => IsInstalled(),
        });
        var bridge = new CoreBridge(null);
        // 先读设置：EXIF 方向、ICC、快速预览等影响第一张图的解码。
        bridge.Core.LoadSettings(Options.SettingsPath);
        SystemLang = SystemLanguage();
        LaunchCore = bridge;
        // 命令行给了图片时马上开始读，与启动、建窗口同时进行；自检也一样，冷启动时间才可比。
        if (Options.OpenPath is not null)
        {
            bridge.Open(Path.GetFullPath(Options.OpenPath));
        }
        LaunchClock.Mark("coreReady");
        Application.Start(_ =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            new App();
        });
        return 0;
    }
}
