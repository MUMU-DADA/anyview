// 资源管理器的缩略图提供程序（/anyview-ffi/src/shell.rs）：不登记，直接向核心的 DLL
// 要类工厂、建对象，像资源管理器那样把文件当流交过去（IInitializeWithStream），再要一张位图。
// COM 接口按虚函数表直接调用，不靠 .NET 的 COM 互操作：这样在 Wine 里和真机上走的是同一条路。
using System;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using Anyview.Core;

namespace AnyviewBindingCheck;

internal static unsafe class ShellCheck
{
    [DllImport("anyview_ffi", ExactSpelling = true)]
    private static extern int DllGetClassObject(Guid* clsid, Guid* iid, void** result);

    [DllImport("anyview_ffi", ExactSpelling = true)]
    private static extern int DllCanUnloadNow();

    [DllImport("anyview_ffi", ExactSpelling = true)]
    private static extern uint anyview_shell_live_objects();

    [DllImport("shlwapi", ExactSpelling = true, CharSet = CharSet.Unicode)]
    private static extern int SHCreateStreamOnFileEx(string file, uint mode, uint attributes, int create, void* template, void** stream);

    [DllImport("gdi32", ExactSpelling = true)]
    private static extern int GetObjectW(nint handle, int size, void* output);

    [DllImport("gdi32", ExactSpelling = true)]
    private static extern int DeleteObject(nint handle);

    [DllImport("gdi32", ExactSpelling = true)]
    private static extern nint CreateCompatibleDC(nint dc);

    [DllImport("gdi32", ExactSpelling = true)]
    private static extern nint SelectObject(nint dc, nint handle);

    [DllImport("gdi32", ExactSpelling = true)]
    private static extern uint GetPixel(nint dc, int x, int y);

    [DllImport("gdi32", ExactSpelling = true)]
    private static extern int DeleteDC(nint dc);

    private static readonly Guid ClsidThumbnail = new("1dc1f4d7-6312-479a-9043-bca81ce0234b");
    private static readonly Guid IidUnknown = new("00000000-0000-0000-c000-000000000046");
    private static readonly Guid IidClassFactory = new("00000001-0000-0000-c000-000000000046");
    private static readonly Guid IidThumbnailProvider = new("e357fccd-a995-4576-b01f-234630154e96");
    private static readonly Guid IidInitializeWithStream = new("b824b49d-22ac-4161-ac8a-9916e8fa3f7f");
    private static readonly Guid IidInitializeWithFile = new("b7d14566-0509-4cce-a71f-0a554233bd9b");
    private static readonly Guid IidPersistFile = new("0000010b-0000-0000-c000-000000000046");

    private const int ClassNotAvailable = unchecked((int)0x80040111);
    private const int NoAggregation = unchecked((int)0x80040110);
    private const int NoInterface = unchecked((int)0x80004002);
    private const int AlreadyInitialized = unchecked((int)0x800704DF);
    private const uint AlphaRgb = 1;
    private const uint AlphaArgb = 2;

    private static void** Vtbl(void* p) => *(void***)p;

    private static int QueryInterface(void* p, Guid iid, out void* result)
    {
        void* r = null;
        int hr = ((delegate* unmanaged[Stdcall]<void*, Guid*, void**, int>)Vtbl(p)[0])(p, &iid, &r);
        result = r;
        return hr;
    }

    private static uint Release(void* p) => ((delegate* unmanaged[Stdcall]<void*, uint>)Vtbl(p)[2])(p);

    private static int CreateInstance(void* factory, void* outer, Guid iid, out void* result)
    {
        void* r = null;
        int hr = ((delegate* unmanaged[Stdcall]<void*, void*, Guid*, void**, int>)Vtbl(factory)[3])(factory, outer, &iid, &r);
        result = r;
        return hr;
    }

    private static int Initialize(void* initializer, void* argument) =>
        ((delegate* unmanaged[Stdcall]<void*, void*, uint, int>)Vtbl(initializer)[3])(initializer, argument, 0);

    private static int GetThumbnail(void* provider, uint side, out nint bitmap, out uint alpha)
    {
        nint b = 0;
        uint a = 0;
        int hr = ((delegate* unmanaged[Stdcall]<void*, uint, nint*, uint*, int>)Vtbl(provider)[3])(provider, side, &b, &a);
        bitmap = b;
        alpha = a;
        return hr;
    }

    /// <summary>
    /// 一张缩略图的结果：尺寸、每像素几位、内存里 (5,5) 处的像素（BGRA）、系统自己读出来的左上和左下的颜色
    /// （0x00BBGGRR；看系统把哪一行当最上面）、透明度的说法。
    /// </summary>
    private sealed record Shot(int Hr, int Width, int Height, int Bits, byte[] Corner, uint TopLeft, uint BottomLeft, uint Alpha, uint StreamLeft)
    {
        public override string ToString() =>
            $"hr=0x{Hr:X8} {Width}x{Height} {Bits}位 像素={string.Join(" ", Corner.Select(b => b.ToString("X2")))} 系统读的左上={TopLeft:X6} 左下={BottomLeft:X6} alpha={Alpha} 流剩余引用={StreamLeft}";
    }

    /// <summary>做一张缩略图。byStream：像资源管理器那样给流；否则直接给路径（IInitializeWithFile）。</summary>
    private static Shot Make(void* factory, string path, uint side, bool byStream)
    {
        if (CreateInstance(factory, null, IidThumbnailProvider, out void* provider) < 0)
        {
            return new Shot(-1, 0, 0, 0, new byte[4], 0, 0, 0, 0);
        }
        void* stream = null;
        int hr;
        if (byStream)
        {
            // STGM_READ | STGM_SHARE_DENY_NONE：资源管理器的流也是只读的文件流。
            hr = SHCreateStreamOnFileEx(path, 0x40, 0, 0, null, &stream);
            if (hr >= 0)
            {
                QueryInterface(provider, IidInitializeWithStream, out void* init);
                hr = Initialize(init, stream);
                Release(init);
            }
        }
        else
        {
            QueryInterface(provider, IidInitializeWithFile, out void* init);
            fixed (char* text = path)
            {
                hr = Initialize(init, text);
            }
            Release(init);
        }
        var shot = new Shot(hr, 0, 0, 0, new byte[4], 0, 0, 0, 0);
        if (hr >= 0)
        {
            hr = GetThumbnail(provider, side, out nint bitmap, out uint alpha);
            shot = shot with { Hr = hr, Alpha = alpha };
            if (bitmap != 0)
            {
                // BITMAP（x64 上 32 字节）：类型、宽、高、每行字节数、平面数、每像素位数、像素的地址。
                byte* info = stackalloc byte[32];
                int got = GetObjectW(bitmap, 32, info);
                int width = *(int*)(info + 4);
                int height = *(int*)(info + 8);
                int stride = *(int*)(info + 12);
                ushort bits = *(ushort*)(info + 18);
                byte* pixels = *(byte**)(info + 24);
                var corner = new byte[4];
                uint topLeft = 0, bottomLeft = 0;
                if (got == 32 && pixels != null && width > 5 && height > 11)
                {
                    byte* p = pixels + 5 * stride + 5 * 4;
                    corner = new[] { p[0], p[1], p[2], p[3] };
                    // 让系统自己读：位图选进内存 DC，取左上、左下的像素。
                    nint dc = CreateCompatibleDC(0);
                    nint old = SelectObject(dc, bitmap);
                    topLeft = GetPixel(dc, 5, 5);
                    bottomLeft = GetPixel(dc, 5, height - 6);
                    SelectObject(dc, old);
                    DeleteDC(dc);
                }
                shot = shot with { Width = width, Height = height, Bits = bits, Corner = corner, TopLeft = topLeft, BottomLeft = bottomLeft };
                DeleteObject(bitmap);
            }
        }
        Release(provider);
        // 对象放开后，流只剩我们自己的这一次引用。
        return stream == null ? shot : shot with { StreamLeft = Release(stream) };
    }

    private static bool IsRed(byte[] bgra, int tolerance) =>
        Math.Abs(bgra[2] - 0xDC) <= tolerance && Math.Abs(bgra[1] - 0x28) <= tolerance && bgra[3] == 255;

    /// <summary>系统读出来的颜色（0x00BBGGRR）是不是四色块左上的红色。</summary>
    private static bool IsRed(uint colorRef, int tolerance) =>
        Math.Abs((int)(colorRef & 0xFF) - 0xDC) <= tolerance && Math.Abs((int)((colorRef >> 8) & 0xFF) - 0x28) <= tolerance
            && Math.Abs((int)((colorRef >> 16) & 0xFF) - 0x28) <= tolerance;

    /// <summary>32 位位图，系统看到的左上是红色、左下不是：第一行确实在最上面。</summary>
    private static bool Upright(Shot shot, int tolerance) =>
        shot.Bits == 32 && IsRed(shot.Corner, tolerance) && IsRed(shot.TopLeft, tolerance) && !IsRed(shot.BottomLeft, 40);

    // ---- 登记到系统、撤销（Wine 里也有注册表，登记的内容在这里就能核对） ----

    private const string Slot = @"ShellEx\{e357fccd-a995-4576-b01f-234630154e96}";
    private const string Clsid = "{1DC1F4D7-6312-479A-9043-BCA81CE0234B}";

    private static string? Read(string key, string name = "") =>
        Microsoft.Win32.Registry.CurrentUser.OpenSubKey(key)?.GetValue(name) as string;

    private static bool Exists(string key) => Microsoft.Win32.Registry.CurrentUser.OpenSubKey(key) is not null;

    private static void WriteSettings(string dir, string text)
    {
        Directory.CreateDirectory(dir);
        File.WriteAllText(Path.Combine(dir, "settings.toml"), text, new System.Text.UTF8Encoding(false));
    }

    /// <summary>
    /// 登记、按设置重新登记、撤销：写进注册表的东西对不对，别的程序原来写的值有没有记下并恢复，撤销后留不留东西。
    /// 安装位置用这个测试程序自己的文件夹。系统按登记找到缩略图组件这一步 Wine 里查不了（Wine 不看当前用户那一半的
    /// 类登记），由真机上安装程序的检查来查。
    /// </summary>
    private static void CheckRegistration(string fixtures, string work, Action<string, bool, string> check)
    {
        using var classes = Microsoft.Win32.Registry.CurrentUser.CreateSubKey(@"Software\Classes");
        // 上次没跑完时留下的先撤掉；再摆两处“别的程序写的值”：PSD 的缩略图归别人，.tga 这种文件已经有主
        // （写在整台电脑的那一半里；这段检查只在 Wine 里跑）。
        AnyviewCore.ShellUnregister(false);
        using (var other = classes.CreateSubKey($@".psd\{Slot}"))
        {
            other.SetValue("", "{00000000-0000-0000-0000-00000000AAAA}");
        }
        using (var owner = Microsoft.Win32.Registry.LocalMachine.CreateSubKey(@"Software\Classes\.tga"))
        {
            owner.SetValue("", "SomeViewer.tga");
        }
        var settings = Path.Combine(work, "settings");
        var settingsFile = Path.Combine(settings, "settings.toml");
        WriteSettings(settings, "[context_menu]\nenabled = true\n\n[language]\nlanguage = \"zh-Hans\"\n");
        var dir = AppContext.BaseDirectory.TrimEnd('\\');
        check("登记：还没登记时没有安装位置", AnyviewCore.ShellInstalledDir() is null, AnyviewCore.ShellInstalledDir() ?? "");
        var error = AnyviewCore.ShellRegister(dir + "\\", "0.0.1", 1234, settingsFile, Lang.En);
        check("登记：成功，记下了安装位置", error is null && AnyviewCore.ShellInstalledDir() == dir, $"{error}；{AnyviewCore.ShellInstalledDir()}");

        var command = Read(@"Software\Classes\Anyview.Image\shell\open\command");
        check("登记：图片类型的打开命令是这个文件夹里的 Anyview.exe", command == $"\"{dir}\\Anyview.exe\" \"%1\"", command ?? "（没有）");
        check("登记：“默认应用”能找到 Anyview（RegisteredApplications、功能表）",
            Read(@"Software\RegisteredApplications", "Anyview") == @"Software\Anyview\Capabilities"
                && Read(@"Software\Anyview\Capabilities\FileAssociations", ".jpg") == "Anyview.Image"
                && Read(@"Software\Anyview\Capabilities\FileAssociations", ".zip") == "Anyview.Archive", "");
        var uninstall = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\Anyview");
        check("登记：“应用和功能”里有卸载项（名字、版本、大小、卸载命令）",
            uninstall?.GetValue("DisplayName") as string == "Anyview" && uninstall.GetValue("DisplayVersion") as string == "0.0.1"
                && uninstall.GetValue("EstimatedSize") is int size && size == 1234
                && uninstall.GetValue("UninstallString") as string == $"\"{dir}\\uninstall.exe\"", "");
        using (var jpg = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(@"Software\Classes\.jpg\OpenWithProgids"))
        {
            check("登记：“打开方式”里有 Anyview（.jpg）", jpg?.GetValueNames().Contains("Anyview.Image") == true, "");
        }
        // 没人认领的扩展名归 Anyview；系统里已经有主的不动。
        var qoi = Read(@"Software\Classes\.qoi");
        var tga = Read(@"Software\Classes\.tga");
        check("登记：没人认领的 .qoi 归 Anyview，已经有主的 .tga 不动", qoi == "Anyview.Image" && tga is null, $".qoi={qoi}，当前用户的 .tga={tga}");
        check("登记：缩略图登记在系统不认识的格式上（.qoi、.psd），常见格式不登记（.jpg）",
            Read($@"Software\Classes\.qoi\{Slot}") == Clsid && Read($@"Software\Classes\.psd\{Slot}") == Clsid && !Exists($@"Software\Classes\.jpg\{Slot}"),
            $".qoi={Read($@"Software\Classes\.qoi\{Slot}")}，.psd={Read($@"Software\Classes\.psd\{Slot}")}");
        var verb = @"Software\Classes\SystemFileAssociations\.jpg\shell\Anyview.Browse";
        check("登记：设置里开了右键菜单，图片上有“用 Anyview 浏览图片”，带图标",
            Read(verb) == "用 Anyview 浏览图片" && Read(verb, "Icon") == $"\"{dir}\\Anyview.exe\",0" && Read(verb + @"\command") == command
                && Exists(@"Software\Classes\SystemFileAssociations\.zip\shell\Anyview.Browse"),
            $"{Read(verb)}；{Read(verb, "Icon")}");

        check("登记：缩略图的类指向这个文件夹里的核心 DLL",
            Read($@"Software\Classes\CLSID\{Clsid}\InprocServer32") == $"{dir}\\anyview_ffi.dll"
                && Read($@"Software\Classes\CLSID\{Clsid}\InprocServer32", "ThreadingModel") == "Apartment", "");

        // 设置变了：关掉右键菜单和 PSD 的缩略图，重新登记。PSD 上别人原来的值要回来。
        WriteSettings(settings, "[context_menu]\nenabled = false\nthumb_psd = false\n");
        error = AnyviewCore.ShellRefresh(settingsFile, Lang.En);
        check("重新登记：关掉右键菜单后那一项没有了，空出来的项也删了",
            error is null && !Exists(verb) && !Exists(@"Software\Classes\SystemFileAssociations\.qoi"), $"{error}");
        check("重新登记：关掉 PSD 的缩略图后，别的程序原来登记的值回来了；别的格式照旧",
            Read($@"Software\Classes\.psd\{Slot}") == "{00000000-0000-0000-0000-00000000AAAA}" && Read($@"Software\Classes\.qoi\{Slot}") == Clsid,
            $".psd={Read($@"Software\Classes\.psd\{Slot}")}");

        // 登记以后别的程序（或用户）改过的值，撤销时不动它。
        using (var webp = classes.CreateSubKey($@".webp\{Slot}"))
        {
            webp.SetValue("", "{00000000-0000-0000-0000-00000000BBBB}");
        }
        AnyviewCore.ShellUnregister(false);
        check("撤销：Anyview 自己的项都没有了",
            AnyviewCore.ShellInstalledDir() is null && !Exists(@"Software\Anyview") && !Exists(@"Software\Classes\Anyview.Image")
                && !Exists(@"Software\Classes\Anyview.Archive") && !Exists($@"Software\Classes\CLSID\{Clsid}")
                && !Exists(@"Software\Classes\Applications\Anyview.exe") && !Exists(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\Anyview")
                && Read(@"Software\RegisteredApplications", "Anyview") is null, "");
        using (var jpg = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(@"Software\Classes\.jpg\OpenWithProgids"))
        {
            check("撤销：“打开方式”里没有 Anyview 了；认领的 .qoi 放开了，空出来的项也删了",
                jpg?.GetValueNames().Contains("Anyview.Image") != true && !Exists(@"Software\Classes\.qoi"),
                $".qoi 还在：{Exists(@"Software\Classes\.qoi")}");
        }
        check("撤销：别人的值原样留着（PSD 原来的、WebP 后来被改的）",
            Read($@"Software\Classes\.psd\{Slot}") == "{00000000-0000-0000-0000-00000000AAAA}"
                && Read($@"Software\Classes\.webp\{Slot}") == "{00000000-0000-0000-0000-00000000BBBB}",
            $".psd={Read($@"Software\Classes\.psd\{Slot}")}，.webp={Read($@"Software\Classes\.webp\{Slot}")}");
        // 收拾这里摆的三处。
        classes.DeleteSubKeyTree(".psd", false);
        classes.DeleteSubKeyTree(".webp", false);
        Microsoft.Win32.Registry.LocalMachine.DeleteSubKeyTree(@"Software\Classes\.tga", false);
        AnyviewCore.ShellUnregister(false);
        check("撤销：没登记时再撤销一次不出错", AnyviewCore.ShellInstalledDir() is null, "");
    }

    public static void Run(string fixtures, Action<string, bool, string> check)
    {
        var work = Path.Combine(Path.GetTempPath(), $"anyview-shell-check-{Environment.ProcessId}");
        Directory.CreateDirectory(work);
        try
        {
            RunChecks(fixtures, work, check);
        }
        finally
        {
            Directory.Delete(work, true);
        }
    }

    private static void RunChecks(string fixtures, string work, Action<string, bool, string> check)
    {
        var clsid = ClsidThumbnail;
        var iid = IidClassFactory;
        void* factory = null;
        int hr = DllGetClassObject(&clsid, &iid, &factory);
        check("缩略图组件：DLL 给出类工厂", hr == 0 && factory != null, $"0x{hr:X8}");
        if (factory == null)
        {
            return;
        }
        var other = IidThumbnailProvider;
        void* nothing = null;
        hr = DllGetClassObject(&other, &iid, &nothing);
        check("缩略图组件：别的类号不认", hr == ClassNotAvailable && nothing == null, $"0x{hr:X8}");
        check("缩略图组件：DLL 不让系统卸载", DllCanUnloadNow() == 1, "");

        hr = CreateInstance(factory, factory, IidThumbnailProvider, out void* aggregated);
        check("缩略图组件：不支持聚合", hr == NoAggregation && aggregated == null, $"0x{hr:X8}");
        hr = CreateInstance(factory, null, IidPersistFile, out void* unsupported);
        check("缩略图组件：不支持的接口要不到，也不留下对象", hr == NoInterface && unsupported == null && anyview_shell_live_objects() == 0,
            $"0x{hr:X8}，对象 {anyview_shell_live_objects()} 个");

        foreach (var file in new[] { "zthumbs/quadrants.qoi", "formats/psb_rgb8_rle.psb", "modern/clip_canvas.clip", "modern/jxr_still.jxr" })
        {
            var shot = Make(factory, Path.Combine(fixtures, file), 64, byStream: true);
            check($"缩略图组件：{file} 按流给，位图 64x48、左上是红色、不透明",
                shot.Hr == 0 && shot.Width == 64 && shot.Height == 48 && Upright(shot, 3) && shot.Alpha == AlphaRgb && shot.StreamLeft == 0,
                shot.ToString());
        }
        // SVG 只按扩展名认：流的名字里的扩展名带到了临时文件上才做得出来。
        var svg = Make(factory, Path.Combine(fixtures, "modern", "svg_quadrants.svg"), 64, byStream: true);
        check("缩略图组件：SVG 按流给也做得出来（扩展名取自流的名字）", svg.Hr == 0 && Math.Max(svg.Width, svg.Height) == 64 && svg.Bits == 32, svg.ToString());
        var transparent = Make(factory, Path.Combine(fixtures, "basic", "transparent_800x600.png"), 96, byStream: true);
        check("缩略图组件：有透明像素的图说明是带透明度的", transparent.Hr == 0 && transparent.Width == 96 && transparent.Height == 72 && transparent.Alpha == AlphaArgb,
            transparent.ToString());
        var direct = Make(factory, Path.Combine(fixtures, "zthumbs", "quadrants.qoi"), 32, byStream: false);
        check("缩略图组件：直接给路径也行，要多大给多大", direct.Hr == 0 && direct.Width == 32 && direct.Height == 24 && Upright(direct, 3), direct.ToString());
        var small = Make(factory, Path.Combine(fixtures, "basic", "small_64x48.png"), 256, byStream: true);
        check("缩略图组件：小图不放大", small.Hr == 0 && small.Width == 64 && small.Height == 48, small.ToString());
        var broken = Make(factory, Path.Combine(fixtures, "broken", "empty.png"), 64, byStream: true);
        check("缩略图组件：坏文件没有位图，报失败", broken.Hr < 0 && broken.Width == 0 && broken.StreamLeft == 0, broken.ToString());
        if (File.Exists(Path.Combine(AppContext.BaseDirectory, "heif.dll")))
        {
            var heic = Make(factory, Path.Combine(fixtures, "modern", "heic_still.heic"), 64, byStream: true);
            check("缩略图组件：HEIC（带着 heif.dll 时）", heic.Hr == 0 && heic.Width == 64 && heic.Height == 48 && Upright(heic, 4), heic.ToString());
        }

        // 没初始化就要缩略图；初始化两次。
        CreateInstance(factory, null, IidUnknown, out void* unknown);
        QueryInterface(unknown, IidThumbnailProvider, out void* provider);
        hr = GetThumbnail(provider, 64, out nint none, out _);
        check("缩略图组件：没给文件就要缩略图，报失败", hr < 0 && none == 0, $"0x{hr:X8}");
        QueryInterface(provider, IidInitializeWithFile, out void* init);
        int first, second;
        fixed (char* text = Path.Combine(fixtures, "zthumbs", "quadrants.qoi"))
        {
            first = Initialize(init, text);
            second = Initialize(init, text);
        }
        check("缩略图组件：同一个对象只收一次文件", first == 0 && second == AlreadyInitialized, $"0x{first:X8} 0x{second:X8}");
        Release(init);
        Release(provider);
        uint left = Release(unknown);
        check("缩略图组件：引用都放开后对象销毁", left == 0 && anyview_shell_live_objects() == 0, $"剩余引用 {left}，对象 {anyview_shell_live_objects()} 个");

        // 登记到注册表的检查只在 Wine 里做（ANYVIEW_CHECK_REGISTRY=1，check-binding.sh 设的）：在真的 Windows 上运行这个
        // 测试程序时不碰用户的注册表；真机上的登记由安装程序的检查来查（package.ps1 -Action test）。
        if (Environment.GetEnvironmentVariable("ANYVIEW_CHECK_REGISTRY") == "1")
        {
            CheckRegistration(fixtures, work, check);
        }
        else
        {
            Console.WriteLine("（没有设 ANYVIEW_CHECK_REGISTRY=1，跳过登记到注册表的检查）");
        }

        var temp = Path.Combine(Path.GetTempPath(), "Anyview-thumbnails");
        check("缩略图组件：临时文件用完就删，文件夹也不留", !Directory.Exists(temp),
            Directory.Exists(temp) ? string.Join("、", Directory.GetFiles(temp).Select(Path.GetFileName)) : "");
    }
}
