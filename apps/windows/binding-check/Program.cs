// C# UniFFI 绑定集成测试：通过 uniffi-bindgen-cs 生成的绑定调用 Windows 核心 DLL，
// 检查同步函数、枚举和记录、后台线程唤醒、C# 实现的系统解码器，以及打开、切换、取像素的完整往返。
// 与 macOS 的 Swift 接口测试（apps/macos/Tests/BindingCheck）对应，按 Windows 的键位和措辞检查。
// 用法：BindingCheck.exe [测试图片目录 [无 JPEG 预览的 RAW 文件]]；全部通过时退出码为 0。可通过 tools/windows/check-binding.sh
// 编译并在 Wine 里运行。
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading;
using Anyview.Core;

namespace AnyviewBindingCheck;

internal static class Program
{
    private static int s_failures;
    internal static int MainThreadId;

    private static void Check(string name, bool passed, string detail = "")
    {
        if (passed)
        {
            Console.WriteLine("✓ " + name);
        }
        else
        {
            s_failures++;
            Console.WriteLine("✗ " + name + (detail.Length == 0 ? "" : "：" + detail));
        }
    }

    private static int Main(string[] args)
    {
        Console.OutputEncoding = new UTF8Encoding(false);
        MainThreadId = Environment.CurrentManagedThreadId;

        var version = AnyviewCore.CoreVersion();
        Check("核心版本号不为空", version.Length > 0, version);

        var hint = AnyviewCore.EmptyStateHint(Platform.Windows, Lang.ZhHans);
        Check("Windows 空态提示用 F2", hint == "将图片拖到窗口中，或按 F2 打开图片", hint);
        Check("macOS 空态提示用 ⌘O", AnyviewCore.EmptyStateHint(Platform.Macos, Lang.ZhHans).Contains("⌘O"));
        Check("zh-Hans-CN 对应简体中文", AnyviewCore.LangFromLocale("zh-Hans-CN") == Lang.ZhHans);
        Check("en-US 对应英文", AnyviewCore.LangFromLocale("en-US") == Lang.En);
        Check("偏好列表里第一种有的（瑞典语、德语 → 德语）",
            AnyviewCore.LangFromLocales(new[] { "sv-SE", "de-DE", "en-US" }) == Lang.De);
        Check("zh-HK 对应繁体中文", AnyviewCore.LangFromLocales(new[] { "zh-HK" }) == Lang.ZhHant);
        Check("界面文本带占位符", AnyviewCore.UiText(TextKey.CoreLinkOk, Lang.ZhHans).Contains("{ms}"));
        Check("应用名", AnyviewCore.UiText(TextKey.AppName, Lang.En) == "Anyview");
        Check("支持的扩展名含 jpg", AnyviewCore.SupportedExtensions().Contains("jpg"));
        var fit = AnyviewCore.FitRect(4000, 3000, 1800, 1200);
        Check("适合窗口的计算", fit == new FitRect(100, 0, 1600, 1200), fit.ToString());

        // 动作目录与菜单数据：快捷键写成 Ctrl+…。Windows 的菜单栏（设置里打开才显示）与 macOS 的
        // 是同一棵树：顶层 7 个菜单，没有 macOS 才有的"Anyview"菜单和隐藏那几项，设置、退出、关于各有一处。
        var bar = AnyviewCore.MenuEntries(MenuKind.MenuBar, Platform.Windows, Lang.ZhHans);
        var barDepth = 0;
        var barTop = new List<string>();
        var barIds = new List<string>();
        foreach (var entry in bar)
        {
            switch (entry)
            {
                case MenuEntry.SubmenuStart start:
                    if (barDepth == 0)
                    {
                        barTop.Add(start.Title);
                    }
                    barDepth++;
                    break;
                case MenuEntry.SubmenuEnd:
                    barDepth--;
                    break;
                case MenuEntry.Action action:
                    barIds.Add(action.Id);
                    break;
            }
        }
        Check("Windows 的菜单栏有 7 个菜单", barTop.Count == 7 && barDepth == 0, string.Join("、", barTop));
        Check("Windows 的菜单栏里设置、退出、关于各有一处，没有 macOS 才有的几项",
            barIds.Count(i => i == "settings") == 1 && barIds.Count(i => i == "quit") == 1 && barIds.Count(i => i == "about") == 1
            && !barIds.Contains("hide_app") && !barIds.Contains("hide_others") && !barIds.Contains("show_all"),
            barIds.Count.ToString());
        var context = AnyviewCore.MenuEntries(MenuKind.Context, Platform.Windows, Lang.ZhHans);
        var labels = new Dictionary<string, string>();
        foreach (var a in context.OfType<MenuEntry.Action>())
        {
            labels[a.Id] = a.Shortcut?.Label ?? "";
        }
        Check("右键菜单里打开图片的快捷键 F2", labels.GetValueOrDefault("open_file") == "F2", labels.GetValueOrDefault("open_file") ?? "（没有这一项）");
        Check(
            "菜单里没有 Mac 的按键符号",
            labels.Values.All(l => !l.Contains('⌘') && !l.Contains('⇧')),
            string.Join("、", labels.Values.Where(l => l.Length > 0).Take(8)));
        var contextTitles = context.OfType<MenuEntry.Action>().Select(a => a.Title).ToList();
        Check("右键菜单用 Windows 措辞（删除）", contextTitles.Contains("删除"), string.Join("、", contextTitles));
        Check("缩放提示文字", AnyviewCore.ZoomText(1.5) == "150%", AnyviewCore.ZoomText(1.5));

        var none = new KeyModifiers(false, false, false, false);
        var ctrl = new KeyModifiers(false, true, false, false);
        var actionCore = new AppCore(new TestListener(new WakeProbe()));
        var openDialog = new ActionOutcome.Ui(new UiRequest.OpenFileDialog());
        Check("打开图片的动作交给界面", actionCore.InvokeAction("open_file") == openDialog);
        Check("F2 解析为打开图片", actionCore.InvokeKey("F2", none) == openDialog);
        Check("Ctrl+= 是放大（没有图片时不可用）", actionCore.InvokeKey("=", ctrl) is ActionOutcome.Unavailable);
        Check("没有绑定的按键返回空", actionCore.InvokeKey("q", none) is null);
        Check(
            "回车全屏，Esc 关窗口或退出，G 选择图片",
            actionCore.ActionForKey("Enter", none) == "fullscreen"
                && actionCore.ActionForKey("Escape", none) == "close_or_quit"
                && actionCore.ActionForKey("g", none) == "select_image");
        Check("默认适合窗口", actionCore.ActionState("fit_window") == new ActionState(true, true));
        Check("没有图片时没有绘制参数", actionCore.ViewLayout() is null);

        // 悬浮界面的规则与文字。
        Check("左边 1/6 是箭头区", AnyviewCore.HoverZone(10, 300, 900, 600) == HoverZone.Left);
        Check("底部是控制栏区", AnyviewCore.HoverZone(450, 590, 900, 600) == HoverZone.Bar);
        Check("进度条一半处是第 5 张（共 9 张）", AnyviewCore.ProgressIndex(0.5, 9) == 4);
        Check("进度提示", AnyviewCore.PositionText(840, 1862) == "841/1862");
        var tooltip = AnyviewCore.ActionTooltip("open_file", Platform.Windows, Lang.ZhHans);
        Check("按钮提示带快捷键", tooltip == "打开图片 (F2)", tooltip);
        Check("淡入 150 ms", AnyviewCore.OverlayTiming().FadeInMs == 150);
        Check("图片信息开关", actionCore.InvokeAction("toggle_info") is ActionOutcome.OverlayChanged);

        // 到达首尾的询问与删除确认的文字。
        var boundary = AnyviewCore.BoundaryPrompt(true, false, Lang.ZhHans);
        Check(
            "末张询问的按钮",
            boundary.Buttons.SequenceEqual(new[] { "转到首张", "转到下一个文件夹", "取消" }),
            string.Join("、", boundary.Buttons));
        var copied = AnyviewCore.NoticeText(new NoticeKind.Copied("图片", "a (2).png"), Lang.ZhHans);
        Check("复制完成的提示", copied == "已复制到“图片”：a (2).png", copied);
        var trash = AnyviewCore.TrashPrompt(Platform.Windows, Lang.ZhHans, "a.png");
        Check(
            "删除确认用回收站",
            trash.Message.Contains("回收站") && trash.Message.Contains("a.png") && trash.Buttons.FirstOrDefault() == "删除",
            trash.Message);
        Check("没有图片时没有当前路径", actionCore.CurrentPath() is null);
        Check("精确重绘在静止 100 ms 后", AnyviewCore.ExactRenderDelayMs() == 100);
        Check("翻页音效是 WAV", AnyviewCore.PageSoundWav().Take(4).SequenceEqual(Encoding.ASCII.GetBytes("RIFF")));

        CheckSettings();
        // 要在登记测试用的系统解码器之前：RAW 完整解码会先问系统解码器。
        if (args.Length > 1)
        {
            CheckRawDevelop(args[1]);
        }
        CheckPlatformDecoder();
        CheckWake();
        CheckSampler();
        if (args.Length > 0)
        {
            CheckImages(args[0]);
            CheckFileThumbnails(args[0]);
            ShellCheck.Run(args[0], Check);
        }
        else
        {
            Console.WriteLine("（没有给测试图片目录，跳过打开与切换的检查）");
        }

        Console.WriteLine(s_failures == 0 ? "全部通过" : $"{s_failures} 项没通过");
        return s_failures == 0 ? 0 : 1;
    }

    /// <summary>
    /// 文件管理器的缩略图：以后 Windows 的缩略图提供程序调用的核心函数。QOI、PSB、CLIP、JPEG XR 各做一张
    /// 长边 64 的缩略图，左上那块是四色块的红色；坏文件没有缩略图。
    /// </summary>
    private static void CheckFileThumbnails(string fixtures)
    {
        foreach (var file in new[] { "zthumbs/quadrants.qoi", "formats/psb_rgb8_rle.psb", "modern/clip_canvas.clip", "modern/jxr_still.jxr" })
        {
            using var thumb = AnyviewCore.MakeFileThumbnail(Path.Combine(fixtures, file), 64);
            byte[] corner = new byte[4];
            if (thumb is not null)
            {
                unsafe
                {
                    var p = (byte*)checked((nint)thumb.PixelAddress()) + 5 * (int)thumb.Stride() + 5 * 4;
                    corner = new[] { p[0], p[1], p[2], p[3] };
                }
            }
            Check(
                $"缩略图：{file} 缩成 64x48，左上是红色",
                thumb is not null && thumb.Width() == 64 && thumb.Height() == 48
                    && Math.Abs(corner[2] - 0xDC) <= 3 && Math.Abs(corner[1] - 0x28) <= 3 && corner[3] == 255,
                thumb is null ? "没有缩略图" : $"{thumb.Width()}x{thumb.Height()} {string.Join(" ", corner.Select(b => b.ToString("X2")))}");
        }
        Check("缩略图：坏文件没有缩略图", AnyviewCore.MakeFileThumbnail(Path.Combine(fixtures, "broken", "empty.png"), 64) is null);
        // HEIC：核心旁边带着 heif.dll（libheif 加 libde265，tools/windows/build-heif.sh）时用它解；没带时不支持
        // （这里没有系统的解码器）。
        using var heic = AnyviewCore.MakeFileThumbnail(Path.Combine(fixtures, "modern", "heic_still.heic"), 64);
        if (File.Exists(Path.Combine(AppContext.BaseDirectory, "heif.dll")))
        {
            byte[] corner = new byte[4];
            if (heic is not null)
            {
                unsafe
                {
                    var p = (byte*)checked((nint)heic.PixelAddress()) + 5 * (int)heic.Stride() + 5 * 4;
                    corner = new[] { p[0], p[1], p[2], p[3] };
                }
            }
            Check(
                "HEIC：带着 heif.dll 时解得出来，缩成 64x48，左上是红色",
                heic is not null && heic.Width() == 64 && heic.Height() == 48
                    && Math.Abs(corner[2] - 0xDC) <= 4 && Math.Abs(corner[1] - 0x28) <= 4 && corner[3] == 255,
                heic is null ? "没有解出来" : $"{heic.Width()}x{heic.Height()} {string.Join(" ", corner.Select(b => b.ToString("X2")))}");
        }
        else
        {
            // 核心自己解不了，最后交给系统解码器：这里的"系统解码器"是上面登记的假解码器（什么文件都给一张 2x1 的 FAKE）。
            Check("HEIC：没带 heif.dll 时核心自己不解，交给系统解码器", heic is null || heic.FormatName() == "FAKE",
                heic is null ? "" : $"{heic.FormatName()} {heic.Width()}x{heic.Height()}");
        }
    }

    /// <summary>设置（docs/SETTINGS.md）：读文件、分类、草稿、校验、确定后生效并保存。</summary>
    private static void CheckSettings()
    {
        var dir = Path.Combine(Path.GetTempPath(), $"anyview-binding-settings-{Environment.ProcessId}");
        if (Directory.Exists(dir))
        {
            Directory.Delete(dir, true);
        }
        var file = Path.Combine(dir, "settings.toml");
        var core = new AppCore(new TestListener(new WakeProbe()));
        Check("设置文件还不存在时用默认值", core.LoadSettings(file) is SettingsLoad.Missing);
        var pages = core.SettingsPages(Lang.ZhHans);
        // 上下文菜单页仅在已安装 Windows 外壳扩展时显示，未安装时各平台均不显示。
        Check(
            "设置的分类（Windows 有格式关联页）",
            pages.FirstOrDefault()?.Title == "一般" && pages.Any(p => p.Title == "格式关联") && pages.All(p => p.Id != "context_menu"),
            string.Join("、", pages.Select(p => p.Title)));
        core.SettingsBegin();
        Check("改草稿", core.SettingsSet("image.cache_mb", new SettingsInput.Text("50")) && core.SettingsDirty());
        var invalid = core.SettingsApply(Lang.ZhHans) as SettingsApplyResult.Invalid;
        Check(
            "缓存大小填 50 时校验失败，停在图像处理页",
            invalid?.Key == "image.cache_mb" && invalid.Page == "image",
            invalid?.ToString() ?? "");
        core.SettingsSet("image.cache_mb", new SettingsInput.Text("512"));
        core.SettingsSet("browse.background_color", new SettingsInput.Color(0x336699));
        var applied = core.SettingsApply(Lang.ZhHans) as SettingsApplyResult.Applied;
        Check(
            "确定后生效并保存",
            applied is not null && applied.SaveError is null && applied.Changed.Length == 2,
            applied is null ? "" : string.Join("、", applied.Changed) + " " + applied.SaveError);
        Check("界面设置里的背景色", core.UiSettings().BackgroundColor == 0x336699);
        var saved = File.Exists(file) ? File.ReadAllText(file, Encoding.UTF8) : "";
        Check("设置文件已写出", saved.Contains("#336699"), saved);
        Check("PageDown 来自默认勾选的映射", core.ActionForKey("PageDown", new KeyModifiers(false, false, false, false)) == "next_container");
        Check("右键默认弹出菜单", core.MouseAction(MouseInput.Right) is MouseResult.Default);
        Directory.Delete(dir, true);
    }

    /// <summary>系统解码器：C# 实现的解码器由核心在后台线程调用（Windows 以后的 WIC 走这条路）。</summary>
    [System.Runtime.InteropServices.DllImport("kernel32.dll")]
    private static extern uint GetCurrentThreadId();

    [System.Runtime.InteropServices.DllImport("anyview_ffi")]
    private static extern unsafe nuint anyview_diag_sample_thread(uint threadId, uint durationMs, ulong* output, nuint capacity);

    [System.Runtime.InteropServices.DllImport("anyview_ffi")]
    private static extern unsafe nuint anyview_diag_sample_thread_until(uint threadId, uint maxMs, int* stop, ulong* output, nuint capacity);

    /// <summary>自检的诊断采样（Windows 版界面在静止时用它看界面线程忙在哪里）：对另一个在忙的线程采样得到地址，不卡住。</summary>
    private static unsafe void CheckSampler()
    {
        uint target = 0;
        var stop = false;
        var busy = new System.Threading.Thread(() =>
        {
            System.Threading.Volatile.Write(ref target, GetCurrentThreadId());
            var x = 0UL;
            while (!System.Threading.Volatile.Read(ref stop))
            {
                x = x * 6364136223846793005UL + 1442695040888963407UL;
            }
            GC.KeepAlive(x);
        });
        busy.Start();
        while (System.Threading.Volatile.Read(ref target) == 0)
        {
            System.Threading.Thread.Sleep(1);
        }
        var addresses = new ulong[256];
        nuint count;
        fixed (ulong* output = addresses)
        {
            count = anyview_diag_sample_thread(target, 200, output, (nuint)addresses.Length);
        }
        Check("诊断采样：对另一个线程采到地址", count > 10 && addresses.Take((int)count).All(a => a != 0), count.ToString());
        // 拖动窗口时用的：最长 5 秒，另一个线程 150 ms 后把停止标志改成 1，应马上停下。
        var flag = (int*)System.Runtime.InteropServices.Marshal.AllocHGlobal(sizeof(int));
        *flag = 0;
        var setter = new System.Threading.Thread(() =>
        {
            System.Threading.Thread.Sleep(150);
            System.Threading.Volatile.Write(ref *flag, 1);
        });
        var watch = System.Diagnostics.Stopwatch.StartNew();
        setter.Start();
        var many = new ulong[8192];
        fixed (ulong* output = many)
        {
            count = anyview_diag_sample_thread_until(target, 5000, flag, output, (nuint)many.Length);
        }
        var elapsed = watch.ElapsedMilliseconds;
        setter.Join();
        System.Runtime.InteropServices.Marshal.FreeHGlobal((nint)flag);
        System.Threading.Volatile.Write(ref stop, true);
        busy.Join();
        Check("诊断采样：停止标志改了就停", count > 10 && elapsed < 1500, $"{count} 次，{elapsed} ms");
    }

    private static void CheckPlatformDecoder()
    {
        Check("登记系统解码器", AnyviewCore.RegisterPlatformDecoder(new FakeDecoder(), new[] { "anyviewtest" }));
        Check("登记后扩展名可以打开", AnyviewCore.SupportedExtensions().Contains("anyviewtest"));
        var fake = Path.Combine(Path.GetTempPath(), $"anyview-binding-{Environment.ProcessId}.anyviewtest");
        File.WriteAllBytes(fake, new byte[] { (byte)'x' });
        var probe = new WakeProbe();
        var core = new AppCore(new TestListener(probe));
        core.Dispatch(new Command.Open(fake));
        var ready = WaitFor(core, probe, 10, e => e is Event.ImageReady) as Event.ImageReady;
        Check(
            "系统解码器的结果送达",
            ready is not null && ready.Image.FormatName() == "FAKE" && ready.Image.Width() == 2 && ready.Image.Height() == 1,
            ready?.ToString() ?? "超时");
        Check("解码器是在后台线程调用的", FakeDecoder.CalledOnMainThread == false);
        File.Delete(fake);
    }

    /// <summary>后台线程唤醒：单次往返，以及连发 200 个 Ping 不丢事件。</summary>
    private static void CheckWake()
    {
        var probe = new WakeProbe();
        var core = new AppCore(new TestListener(probe));
        core.Dispatch(new Command.Ping(42));
        var pong = WaitFor(core, probe, 5, e => e is Event.Pong p && p.Token == 42);
        Check("Ping 后 5 秒内收到 Pong", pong is not null);
        Check("唤醒来自后台线程", probe.WokeOnMainThread == false, $"WokeOnMainThread={probe.WokeOnMainThread}");

        var many = new WakeProbe();
        var busy = new AppCore(new TestListener(many));
        const ulong count = 200;
        for (ulong token = 0; token < count; token++)
        {
            busy.Dispatch(new Command.Ping(token));
        }
        var tokens = new HashSet<ulong>();
        var deadline = DateTime.UtcNow.AddSeconds(10);
        while (tokens.Count < (int)count && DateTime.UtcNow < deadline)
        {
            many.Signal.Wait(500);
            foreach (var e in busy.DrainEvents())
            {
                if (e is Event.Pong p)
                {
                    tokens.Add(p.Token);
                }
            }
        }
        Check("200 个 Ping 全部收到 Pong", tokens.Count == (int)count, $"收到 {tokens.Count} 个");
    }

    /// <summary>打开文件夹、取像素、查看计算、切换、快速预览（测试图片由 anyview-fixtures 生成）。</summary>
    private static void CheckImages(string fixtures)
    {
        var probe = new WakeProbe();
        var viewer = new AppCore(new TestListener(probe));
        viewer.Dispatch(new Command.Open(Path.Combine(fixtures, "sort")));
        var first = WaitFor(viewer, probe, 10, e => e is Event.ImageReady) as Event.ImageReady;
        if (first is null)
        {
            Check("打开文件夹后收到图像", false, "超时");
            return;
        }
        var image = first.Image;
        Check("打开文件夹后显示第一张 1.png", Path.GetFileName(first.Path) == "1.png", first.Path);
        Check("图像尺寸 32x32", image.Width() == 32 && image.Height() == 32);
        Check("每行字节数与数据长度", image.Stride() == 128 && image.ByteLen() == 4096);
        byte alpha;
        unsafe
        {
            alpha = ((byte*)checked((nint)image.PixelAddress()))[3];
        }
        Check("像素可直接读取（不透明）", alpha == 255, $"alpha={alpha}");
        var texture = image.Texture();
        Check(
            "普通图片的纹理就是它自己（没有代理）",
            !image.HasProxy() && texture.Width() == 32 && texture.PixelAddress() == image.PixelAddress());

        // 查看计算：32x32 的图放进 100x100 的视口，按原始大小居中；顺时针旋转后左上角到了右上。
        viewer.ViewSetViewport(100, 100);
        var layout = viewer.ViewShowImage(32, 32);
        Check("小图按原始大小居中", layout?.Scale == 1 && layout.CenterX == 50, layout?.ToString() ?? "");
        Check(
            "顺时针旋转后菜单勾选跟着变",
            viewer.InvokeAction("rotate_cw") == new ActionOutcome.ViewChanged(false) && viewer.ActionState("rotate_cw").Checked);
        var before = viewer.ViewSignature();
        viewer.ViewZoomBy(2, 50, 50);
        Check("缩放后画面签名改变", viewer.ViewSignature() != before);
        viewer.ViewZoomBy(0.5, 50, 50);
        var corner = viewer.ViewImageToScreen(0, 0);
        Check("旋转后图片左上角在右上", corner == new ScreenPoint(66, 34), corner?.ToString() ?? "");
        var info = viewer.InfoLines(Lang.ZhHans);
        Check("图片信息第一行带序号", info.FirstOrDefault()?.StartsWith("文件名: 1.png [1/") == true, string.Join(" | ", info));
        Check("图片信息有四行", info.Length == 4, info.Length.ToString());

        viewer.Dispatch(new Command.Navigate(new Navigation.Next()));
        var second = WaitFor(viewer, probe, 10, e => e is Event.ImageReady) as Event.ImageReady;
        Check("下一张是 2.png（自然排序）", second is not null && Path.GetFileName(second.Path) == "2.png", second?.Path ?? "超时");
        // 核心给界面的路径是普通写法（C:\…），不是 \\?\C:\…：WIC、剪贴板、回收站等系统接口不认后一种。
        var current = viewer.CurrentPath() ?? "";
        Check(@"当前图片的路径是普通写法（不以 \\?\ 开头）", current.Length > 0 && !current.StartsWith(@"\\?\", StringComparison.Ordinal), current);

        // 快速预览：600 万像素的 JPEG 先送 EXIF 缩略图，清晰图随后以 ImageUpdated 送达。
        viewer.Dispatch(new Command.Open(Path.Combine(fixtures, "preview", "thumbnail_3000x2000.jpg")));
        Event.ImagePreview? preview = null;
        var sharp = WaitFor(viewer, probe, 10, e =>
        {
            if (e is Event.ImagePreview p)
            {
                preview = p;
            }
            return e is Event.ImageUpdated;
        }) as Event.ImageUpdated;
        Check(
            "快速预览：缩略图 160x107，原图 3000x2000",
            preview is not null && preview.Image.Width() == 160 && preview.Image.Height() == 107
                && preview.Width == 3000 && preview.Height == 2000,
            preview is null ? "没有预览" : $"{preview.Image.Width()}x{preview.Image.Height()}，{preview.Width}x{preview.Height}");
        Check(
            "清晰图随后以 ImageUpdated 送达",
            sharp is not null && sharp.Image.Width() == 3000 && sharp.Image.Height() == 2000);

        // 扩展格式：JPEG 2000（纯 Rust）、Clip Studio Paint（内嵌 SQLite）在 Windows 版核心里也能解。
        // JPEG XR、AVIF：Windows 先交给 WIC（AVIF 要装"AV1 视频扩展"），解不了时用核心自己的解码器（jxrlib 与内置 AVIF 解码器）；
        // 这里没有登记 WIC，走的就是核心的。
        foreach (var (file, format) in new[] { ("jp2_still.jp2", "JP2"), ("j2k_still.j2k", "J2K"), ("clip_canvas.clip", "CLIP"), ("avif_still.avif", "AVIF"), ("jxr_still.jxr", "JXR") })
        {
            viewer.Dispatch(new Command.Open(Path.Combine(fixtures, "modern", file)));
            var decoded = WaitFor(viewer, probe, 10, e => e is Event.ImageReady || e is Event.ImageFailed);
            var ready = decoded as Event.ImageReady;
            byte[] center = new byte[4];
            if (ready is not null)
            {
                unsafe
                {
                    var p = (byte*)checked((nint)ready.Image.PixelAddress()) + 24 * (int)ready.Image.Stride() + 32 * 4;
                    center = new[] { p[0], p[1], p[2], p[3] };
                }
            }
            // 左上那块是 #DC2828（BGRA：28 28 DC FF）。
            Check(
                $"{format}：{file} 解出 128x96，左上那块的颜色对",
                ready is not null && ready.Image.Width() == 128 && ready.Image.Height() == 96
                    && center.SequenceEqual(new byte[] { 0x28, 0x28, 0xDC, 0xFF }),
                ready is null ? (decoded as Event.ImageFailed)?.Message ?? "超时" : string.Join(" ", center.Select(b => b.ToString("X2"))));
        }
        // JPEG XR 的高动态范围（RGBE）：左半边线性 0.5 → sRGB 约 188；右半边 (2, 0.25, 0.25) → 红截到 255，其余约 137。
        {
            viewer.Dispatch(new Command.Open(Path.Combine(fixtures, "modern", "jxr_hdr.jxr")));
            var decoded = WaitFor(viewer, probe, 10, e => e is Event.ImageReady || e is Event.ImageFailed);
            var ready = decoded as Event.ImageReady;
            byte[] left = new byte[4], right = new byte[4];
            if (ready is not null)
            {
                unsafe
                {
                    var row = (byte*)checked((nint)ready.Image.PixelAddress()) + 8 * (int)ready.Image.Stride();
                    left = new[] { row[4 * 4], row[4 * 4 + 1], row[4 * 4 + 2], row[4 * 4 + 3] };
                    right = new[] { row[28 * 4], row[28 * 4 + 1], row[28 * 4 + 2], row[28 * 4 + 3] };
                }
            }
            static bool Near(byte value, int expected) => Math.Abs(value - expected) <= 2;
            Check(
                "JXR：jxr_hdr.jxr（RGBE）解出 32x16，按 sRGB 换算、超出的截断",
                ready is not null && ready.Image.Width() == 32 && ready.Image.Height() == 16
                    && Near(left[0], 188) && Near(left[1], 188) && Near(left[2], 188)
                    && right[2] == 255 && Near(right[1], 137) && Near(right[0], 137),
                ready is null ? (decoded as Event.ImageFailed)?.Message ?? "超时"
                    : $"左 {string.Join(" ", left.Select(b => b.ToString("X2")))}，右 {string.Join(" ", right.Select(b => b.ToString("X2")))}");
        }
        // 使用监视器的 ICC 设置：设置关着时报来的配置文件只记下；打开后灰度 128 按线性 Gamma 的"显示器"
        // 配置文件显示成约 55；关掉后回到 128。
        {
            viewer.Dispatch(new Command.Open(Path.Combine(fixtures, "color", "gray_128.png")));
            var opened = WaitFor(viewer, probe, 10, e => e is Event.ImageReady) as Event.ImageReady;
            var profile = File.ReadAllBytes(Path.Combine(fixtures, "color", "linear_monitor.icc"));
            var changedWhileOff = viewer.SetDisplayProfile(profile);
            viewer.SettingsBegin();
            var accepted = viewer.SettingsSet("image.monitor_icc", new SettingsInput.Bool(true));
            var applied = viewer.SettingsApply(Lang.ZhHans) is SettingsApplyResult.Applied;
            var converted = WaitFor(viewer, probe, 10, e => e is Event.ImageUpdated) as Event.ImageUpdated;
            byte gray = 0;
            if (converted is not null)
            {
                unsafe
                {
                    gray = ((byte*)checked((nint)converted.Image.PixelAddress()))[0];
                }
            }
            Check(
                "监视器 ICC：设置打开后灰度 128 转成约 55（关着时只记下）",
                opened is not null && !changedWhileOff && accepted && applied && gray >= 52 && gray <= 58,
                $"关着时{(changedWhileOff ? "就改了" : "没改")}，设置{(accepted && applied ? "已生效" : "没生效")}，转换后 {gray}");
            viewer.SettingsBegin();
            viewer.SettingsSet("image.monitor_icc", new SettingsInput.Bool(false));
            viewer.SettingsApply(Lang.ZhHans);
            var restored = WaitFor(viewer, probe, 10, e => e is Event.ImageUpdated) as Event.ImageUpdated;
            byte back = 0;
            if (restored is not null)
            {
                unsafe
                {
                    back = ((byte*)checked((nint)restored.Image.PixelAddress()))[0];
                }
            }
            Check("监视器 ICC：关掉后回到 128", back == 128, back.ToString());
        }
    }

    /// <summary>没有 JPEG 预览的 RAW 由 LibRaw 完整解码：格式写扩展名本身，不带"/JPEG"。</summary>
    private static void CheckRawDevelop(string path)
    {
        var probe = new WakeProbe();
        var core = new AppCore(new TestListener(probe));
        core.Dispatch(new Command.Open(path));
        var ready = WaitFor(core, probe, 30, e => e is Event.ImageReady || e is Event.ImageFailed);
        var image = (ready as Event.ImageReady)?.Image;
        Check(
            "LibRaw 完整解码 RAW",
            image is not null && image.Width() > 500 && !image.FormatName().Contains("JPEG"),
            image is null ? ready?.ToString() ?? "超时" : $"{image.FormatName()} {image.Width()}x{image.Height()}");
    }

    /// <summary>取出事件直到出现满足条件的一个；超时返回 null。</summary>
    private static Event? WaitFor(AppCore core, WakeProbe probe, double seconds, Func<Event, bool> match)
    {
        var deadline = DateTime.UtcNow.AddSeconds(seconds);
        while (DateTime.UtcNow < deadline)
        {
            foreach (var e in core.DrainEvents())
            {
                if (match(e))
                {
                    return e;
                }
            }
            probe.Signal.Wait(200);
        }
        return null;
    }
}

/// <summary>记录唤醒情况：核心在后台线程调用 Wake。</summary>
internal sealed class WakeProbe
{
    public readonly SemaphoreSlim Signal = new(0);
    public volatile object? WokeOnMainThreadBox;
    public bool? WokeOnMainThread => WokeOnMainThreadBox as bool?;
}

internal sealed class TestListener : CoreListener
{
    private readonly WakeProbe _probe;

    public TestListener(WakeProbe probe)
    {
        _probe = probe;
    }

    public void Wake()
    {
        _probe.WokeOnMainThreadBox ??= Environment.CurrentManagedThreadId == Program.MainThreadId;
        _probe.Signal.Release();
    }
}

/// <summary>测试用的系统解码器：不管文件内容，返回 2x1 的图像。</summary>
internal sealed class FakeDecoder : PlatformDecoder
{
    public static volatile object? CalledOnMainThreadBox;
    public static bool? CalledOnMainThread => CalledOnMainThreadBox as bool?;

    public PlatformImage? Decode(string path, bool autoOrient, bool useIcc)
    {
        CalledOnMainThreadBox ??= Environment.CurrentManagedThreadId == Program.MainThreadId;
        return new PlatformImage(2, 1, new byte[] { 1, 2, 3, 255, 4, 5, 6, 255 }, "FAKE", "RGB24", false, null);
    }

    public uint FrameCount(string path) => 1;

    public PlatformFrame? DecodeFrame(string path, uint index) => null;
}
