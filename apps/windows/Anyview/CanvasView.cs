// 图片画布（ARCHITECTURE"画布与渲染"）：Win2D 的 CanvasControl，图片作为 GPU 纹理绘制。
// 位置、倍率、旋转、翻转都由核心计算（物理像素，原点在左上），这里只套上变换矩阵。
// 静止后由核心精确重绘可见区域，结果按物理像素一一对应盖在上面；翻页效果用离屏画面加 3D 变换。
// 图像句柄（核心的像素）和 GPU 纹理都用完就释放，不等垃圾回收：一张 1200 万像素的图就是 48 MB。
using System;
using System.Numerics;
using System.Runtime.InteropServices;
using Anyview.Core;
using Microsoft.Graphics.Canvas;
using Microsoft.Graphics.Canvas.Brushes;
using Microsoft.Graphics.Canvas.Effects;
using Microsoft.Graphics.Canvas.UI.Xaml;
using Microsoft.UI.Dispatching;
using Windows.Foundation;
using Windows.Graphics.DirectX;
using Windows.UI;

namespace Anyview;

internal sealed class CanvasView
{
    /// <summary>Win2D 的画布控件。关窗口以后换成一个没用过的空控件（见 Release）。</summary>
    public CanvasControl Control { get; private set; } = new();

    /// <summary>自检用：关窗口时放掉的那个画布控件，看它回收了没有。</summary>
    public WeakReference<object>? ReleasedControl { get; private set; }
    private readonly AppCore _core;
    private readonly DispatcherQueue _queue;

    // 图片：核心的图像句柄（画布持有，换图时释放旧的），纹理按需上传（设备丢失后重新上传）。
    private ImageHandle? _handle;
    private CanvasBitmap? _bitmap;
    /// <summary>上传纹理时复制像素用的数组（见 Upload）。</summary>
    private byte[]? _uploadScratch;
    public bool HasImage { get; private set; }
    public bool ShowingPreview { get; private set; }
    /// <summary>正在显示的图像（画布持有：只能在当时用，不能留着）。</summary>
    public ImageHandle? CurrentHandle => _handle;
    private ViewLayout? _layout;

    // 精确重绘。
    private ImageHandle? _exactHandle;
    private CanvasBitmap? _exactBitmap;
    private ulong? _exactSignature;
    private (long X, long Y) _exactAt;
    private ulong _scheduledSignature;
    private int _exactSerial;
    public (long X, long Y, uint W, uint H, double Ms)? ExactInfo { get; private set; }

    // 背景色、背景图片、透明处的底色。
    private Color _background = Color.FromArgb(255, 0x20, 0x20, 0x20);
    private ImageHandle? _backgroundHandle;
    private CanvasBitmap? _backgroundBitmap;
    private AlphaFill _alphaFill = new AlphaFill.Checker();
    private CanvasBitmap? _checker;

    // 翻页效果。
    private CanvasRenderTarget? _turnOld;
    private PageEffect _turnEffect = PageEffect.None;
    private bool _turnForward;
    private double _turnDuration;
    private DateTime _turnStart;
    private DispatcherQueueTimer? _turnTimer;
    /// <summary>自检用：下一次翻页效果停在这个进度（0–1）。</summary>
    public double? HoldTransition;
    public (PageEffect Effect, bool Forward, double Progress)? TransitionState =>
        _turnOld is null ? null : (_turnEffect, _turnForward, TurnProgress);

    private double _watchedScale;
    private bool _watchingRoot;
    private bool _drawnMarked;
    private bool _previewDrawnMarked;

    /// <summary>每个有效像素对应的物理像素数（DPI 缩放）。</summary>
    public double Scale => Control.XamlRoot?.RasterizationScale ?? 1.0;

    public CanvasView(AppCore core, DispatcherQueue queue)
    {
        _core = core;
        _queue = queue;
        // 用程序启动时在后台先建好的共享设备（Program 里 CanvasDevice.GetSharedDevice）。CanvasControl 默认自己另建一个设备
        // （界面线程上、放进窗口时才建），那样后台先建的设备白建了，冷启动还要再等一次建设备。各窗口的画布也共用这一个设备。
        Control.UseSharedDevice = true;
        Control.ClearColor = _background;
        Control.Draw += OnDraw;
        // 设备新建（或丢失后重建）：旧设备上的纹理、离屏画面都不能再用，画的时候重新上传。
        Control.CreateResources += (_, _) =>
        {
            // 冷启动：画布拿到显卡设备的时刻。
            LaunchClock.Mark("canvasDevice");
            EndTransition();
            Drop(ref _bitmap);
            Drop(ref _exactBitmap);
            Drop(ref _backgroundBitmap);
            Drop(ref _checker);
        };
        Control.SizeChanged += (_, _) =>
        {
            Diag.CanvasResizes++;
            RelayoutForSize();
        };
        // 窗口拖到缩放比例不同的屏幕上：有效像素的大小不变，SizeChanged 不一定来，按缩放比例的变化重新摆放。
        Control.Loaded += (_, _) =>
        {
            if (_watchingRoot || Control.XamlRoot is not { } root)
            {
                return;
            }
            _watchingRoot = true;
            _watchedScale = root.RasterizationScale;
            root.Changed += (_, _) =>
            {
                Diag.RootChanges++;
                if (root.RasterizationScale != _watchedScale)
                {
                    _watchedScale = root.RasterizationScale;
                    RelayoutForSize();
                }
            };
        };
    }

    private static void Drop<T>(ref T? field) where T : class, IDisposable
    {
        field?.Dispose();
        field = null;
    }

    /// <summary>换上新的图像句柄：旧的（不是同一个时）释放，纹理等画的时候重新上传。</summary>
    private void SetHandle(ImageHandle? handle)
    {
        if (!ReferenceEquals(_handle, handle))
        {
            _handle?.Dispose();
        }
        _handle = handle;
        Drop(ref _bitmap);
    }

    // ---- 图片 ----

    /// <summary>显示一张新图片（画布接管这个句柄）；传 null 清空。</summary>
    public void Show(ImageHandle? handle)
    {
        if (handle is null)
        {
            Clear();
            return;
        }
        Display(handle, handle.Width(), handle.Height(), false);
    }

    /// <summary>快速预览：小缩略图拉伸到原图尺寸显示；清晰图随后由 Replace 换上。</summary>
    public void ShowPreview(ImageHandle handle, uint width, uint height) => Display(handle, width, height, true);

    private void Display(ImageHandle handle, uint width, uint height, bool preview)
    {
        SetHandle(handle);
        ShowingPreview = preview;
        HasImage = true;
        RefreshAlphaFill();
        HideExact();
        _scheduledSignature = 0;
        UpdateViewport();
        _layout = _core.ViewShowImage(width, height);
        Control.Invalidate();
        ScheduleExact();
    }

    private bool _released;

    /// <summary>
    /// 窗口关掉了：放掉图像句柄、纹理和离屏画面（不等垃圾回收），之后不再画、不再找核心（核心随后也会放掉）。
    /// </summary>
    public void Release()
    {
        _released = true;
        _turnTimer?.Stop();
        _turnTimer = null;
        Drop(ref _turnOld);
        SetHandle(null);
        Drop(ref _exactHandle);
        Drop(ref _exactBitmap);
        Drop(ref _backgroundHandle);
        Drop(ref _backgroundBitmap);
        Drop(ref _checker);
        HasImage = false;
        _layout = null;
        // 关掉的窗口，WinUI 不回收它的对象，
        // 所以窗口拿着的东西要自己放掉，不能等垃圾回收。下面三步都是量出来管用的（一键测试的"开关窗口"两项：
        // 少做哪一步，那一步管的东西就漏；另外试过退订画布和 AppWindow 的事件、清空窗口的内容，量不出差别，没加）。
        //
        // 1. 上传纹理时留着接着用的数组（16 MB 以内的才留）。不放的话每关一个窗口留一份：1868x1848 的图片每个窗口
        //    多占约 11 MB 内存。
        if (CloseCleanup.Does("scratch"))
        {
            _uploadScratch = null;
        }
        try
        {
            ReleasedControl = new WeakReference<object>(Control);
            // 2. 把画布控件从界面树上摘下来（Win2D 文档"Avoiding memory leaks"的第一步；只是从面板的子元素里去掉它）。
            if (CloseCleanup.Does("canvas-tree"))
            {
                Control.RemoveFromVisualTree();
            }
            // 3. 把对画布控件的引用换掉（文档的第二步"把引用清掉"）。2、3 少一步，控件就一直在，连同它那张和窗口一样大的
            //    画面：1856x1392 的窗口每关一个漏约 27 MB 显存，30 个窗口 800 MB；两步都做，显存不再随窗口数涨。
            //    换成一个没用过的空控件而不是空引用：晚到的回调里还有地方会碰它（Invalidate 之类），碰空控件没有害处。
            if (CloseCleanup.Does("canvas-control"))
            {
                Control = new CanvasControl();
            }
        }
        catch (Exception error)
        {
            Crash.Write("CanvasView.Release", error);
        }
    }

    /// <summary>自检用：还留着上传用的数组吗（关掉的窗口不该留）。</summary>
    public bool HoldsBuffers => _uploadScratch is not null;

    private void Clear()
    {
        EndTransition();
        SetHandle(null);
        HasImage = false;
        ShowingPreview = false;
        _layout = null;
        HideExact();
        _core.ViewClear();
        Control.Invalidate();
    }

    /// <summary>同一张图片换了显示缓冲区（反相、Gamma、动图的下一帧、清晰图替换预览）：查看状态不变。</summary>
    public void Replace(ImageHandle handle)
    {
        if (!HasImage)
        {
            handle.Dispose();
            return;
        }
        SetHandle(handle);
        ShowingPreview = false;
        RefreshAlphaFill();
        HideExact();
        _scheduledSignature = 0;
        Control.Invalidate();
        ScheduleExact();
    }

    /// <summary>核心的查看参数变了（菜单、按键、手势），重新摆放图片。</summary>
    public void RefreshLayout()
    {
        _layout = _core.ViewLayout();
        Control.Invalidate();
        ScheduleExact();
    }

    private void RelayoutForSize()
    {
        if (_released)
        {
            return;
        }
        var layout = UpdateViewport();
        if (HasImage)
        {
            _layout = layout ?? _core.ViewLayout();
            ScheduleExact();
        }
        Control.Invalidate();
    }

    /// <summary>100% 是一个图片像素一个有效像素：先告诉核心一个有效像素几个物理像素，再给视口大小。</summary>
    private ViewLayout? UpdateViewport()
    {
        var scale = Scale;
        _core.ViewSetPixelsPerPoint(scale);
        return _core.ViewSetViewport(Math.Max(1, Control.ActualWidth) * scale, Math.Max(1, Control.ActualHeight) * scale);
    }

    // ---- 精确重绘 ----

    /// <summary>盖上精确重绘的结果（画布接管这个句柄）；画面已经变了（签名不同）就不用。</summary>
    public void ShowExact(ImageHandle handle, long x, long y, ulong signature, double renderMs)
    {
        if (!HasImage || signature != _core.ViewSignature())
        {
            handle.Dispose();
            return;
        }
        HideExact();
        _exactHandle = handle;
        _exactSignature = signature;
        _exactAt = (x, y);
        ExactInfo = (x, y, handle.Width(), handle.Height(), renderMs);
        Control.Invalidate();
    }

    /// <summary>精确重绘的结果正盖在画面上（自检用）。</summary>
    public bool ExactShown => _exactSignature is ulong s && s == _core.ViewSignature();

    private void HideExact()
    {
        Drop(ref _exactHandle);
        Drop(ref _exactBitmap);
        _exactSignature = null;
        ExactInfo = null;
    }

    /// <summary>画面变了：撤下覆盖层；静止 100 ms 后请求精确重绘（不需要时核心什么也不做）。</summary>
    private void ScheduleExact()
    {
        if (!HasImage)
        {
            return;
        }
        var signature = _core.ViewSignature();
        if (_exactSignature == signature)
        {
            return;
        }
        HideExact();
        if (_scheduledSignature == signature)
        {
            return;
        }
        _scheduledSignature = signature;
        var serial = ++_exactSerial;
        After(AnyviewCore.ExactRenderDelayMs(), () =>
        {
            if (serial == _exactSerial && _core.ViewSignature() == signature)
            {
                _core.RequestExact();
            }
        });
    }

    // ---- 背景 ----

    public void SetBackground(uint rgb)
    {
        _background = Color.FromArgb(255, (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb);
        Control.ClearColor = _background;
        Control.Invalidate();
    }

    /// <summary>背景图片（画布接管这个句柄）；传 null 时只用背景色。</summary>
    public void SetBackgroundImage(ImageHandle? handle)
    {
        if (!ReferenceEquals(_backgroundHandle, handle))
        {
            _backgroundHandle?.Dispose();
        }
        _backgroundHandle = handle;
        Drop(ref _backgroundBitmap);
        Control.Invalidate();
    }

    public bool HasBackgroundImage => _backgroundHandle is not null;

    /// <summary>透明处的背景由核心按当前图片决定（GIF 与 32 位图片各有设置）：棋盘格或纯色。</summary>
    public void RefreshAlphaFill()
    {
        _alphaFill = _handle is null ? new AlphaFill.Checker() : _core.AlphaFill(_handle);
        Control.Invalidate();
    }

    // ---- 翻页效果 ----

    /// <summary>开始翻页效果：在换上新图片之前调用，先把现在的画面存成旧画面。空态时不做。</summary>
    public void BeginTransition(PageEffect effect, bool forward, uint durationMs)
    {
        EndTransition();
        if (effect == PageEffect.None || !HasImage || Control.Device is null)
        {
            return;
        }
        var (w, h) = PixelSize();
        if (w < 1 || h < 1)
        {
            return;
        }
        // 这里不在 Win2D 的绘制回调里：显卡出问题（设备丢失）时自己接住，不做翻页效果，新图照常换上。
        CanvasRenderTarget? old = null;
        try
        {
            old = new CanvasRenderTarget(Control, w, h, 96);
            using var ds = old.CreateDrawingSession();
            ds.Clear(Color.FromArgb(0, 0, 0, 0));
            DrawContent(ds, Matrix3x2.Identity);
        }
        catch (Exception error)
        {
            old?.Dispose();
            if (Control.Device is { } device && device.IsDeviceLost(error.HResult))
            {
                device.RaiseDeviceLost();
            }
            return;
        }
        _turnOld = old;
        _turnEffect = effect;
        _turnForward = forward;
        _turnDuration = Math.Max(durationMs, 1) / 1000.0;
        _turnStart = DateTime.UtcNow;
        if (HoldTransition is null)
        {
            _turnTimer = _queue.CreateTimer();
            _turnTimer.Interval = TimeSpan.FromMilliseconds(16);
            _turnTimer.Tick += (_, _) =>
            {
                Control.Invalidate();
                if (TurnProgress >= 1)
                {
                    EndTransition();
                }
            };
            _turnTimer.Start();
        }
        Control.Invalidate();
    }

    private double TurnProgress => HoldTransition
        ?? Math.Clamp((DateTime.UtcNow - _turnStart).TotalSeconds / _turnDuration, 0, 1);

    /// <summary>自检用：取消停住，并结束停着的效果。</summary>
    public void ReleaseHeldTransition()
    {
        HoldTransition = null;
        EndTransition();
    }

    private void EndTransition()
    {
        _turnTimer?.Stop();
        _turnTimer = null;
        Drop(ref _turnOld);
        Control.Invalidate();
    }

    // ---- 绘制 ----

    private (float W, float H) PixelSize() =>
        ((float)Math.Round(Control.ActualWidth * Scale), (float)Math.Round(Control.ActualHeight * Scale));

    /// <summary>画了几次（自检：静止时不该一直重画）。</summary>
    public int DrawCount { get; private set; }

    private void OnDraw(CanvasControl sender, CanvasDrawEventArgs args)
    {
        if (_released)
        {
            return;
        }
        DrawCount++;
        Diag.CanvasDraws++;
        try
        {
            var ds = args.DrawingSession;
            // 以下都按物理像素画：一个位图像素对应一个物理像素。
            var basis = Matrix3x2.CreateScale((float)(1 / Scale));
            ds.Transform = basis;
            ds.Clear(_background);
            DrawBackgroundImage(ds);
            if (_turnOld is not null)
            {
                DrawTransition(ds, basis);
            }
            else
            {
                DrawContent(ds, basis);
            }
        }
        catch (Exception error) when (Control.Device is not null && Control.Device.IsDeviceLost(error.HResult))
        {
            Control.Device.RaiseDeviceLost();
        }
    }

    private void DrawBackgroundImage(CanvasDrawingSession ds)
    {
        if (_backgroundHandle is null)
        {
            return;
        }
        _backgroundBitmap ??= Upload(Control, _backgroundHandle);
        if (_backgroundBitmap is null)
        {
            return;
        }
        // 填满并裁切，居中。
        var (w, h) = PixelSize();
        var size = _backgroundBitmap.SizeInPixels;
        var k = Math.Max(w / size.Width, h / size.Height);
        float dw = size.Width * k, dh = size.Height * k;
        ds.DrawImage(_backgroundBitmap, new Rect((w - dw) / 2, (h - dh) / 2, dw, dh),
            new Rect(0, 0, size.Width, size.Height), 1, CanvasImageInterpolation.HighQualityCubic);
    }

    /// <summary>棋盘格、图片、精确重绘（翻页时整组移动、翻转）。</summary>
    private void DrawContent(CanvasDrawingSession ds, Matrix3x2 basis)
    {
        if (!HasImage || _handle is null || _layout is null)
        {
            return;
        }
        if (_bitmap is null)
        {
            // 超大图的纹理是缩小的代理，按原图尺寸拉伸。
            using var texture = _handle.Texture();
            _bitmap = Upload(Control, texture, ref _uploadScratch);
        }
        if (_bitmap is null)
        {
            return;
        }
        var l = _layout;
        float w = (float)l.Width, h = (float)l.Height;
        // 先旋转（顺时针为正，y 向下），再按屏幕方向翻转，最后移到中心（与 macOS 的图层变换相同）。
        var m = Matrix3x2.CreateTranslation(-w / 2, -h / 2)
            * Matrix3x2.CreateRotation((float)(l.Rotation * Math.PI / 180))
            * Matrix3x2.CreateScale(l.FlipHorizontal ? -1 : 1, l.FlipVertical ? -1 : 1)
            * Matrix3x2.CreateTranslation((float)l.CenterX, (float)l.CenterY);
        ds.Transform = m * basis;
        var dest = new Rect(0, 0, w, h);
        if (_handle.HasAlpha())
        {
            FillTransparent(ds, dest);
        }
        var src = _bitmap.SizeInPixels;
        // 平滑显示：放大用双线性；缩小（一个纹理像素不到一个物理像素）用高质量的预缩小，相当于其他两端的三线性（核心 Layout.smooth）。
        var interpolation = !l.Smooth
            ? CanvasImageInterpolation.NearestNeighbor
            : w < src.Width - 0.5 ? CanvasImageInterpolation.HighQualityCubic : CanvasImageInterpolation.Linear;
        ds.DrawImage(_bitmap, dest, new Rect(0, 0, src.Width, src.Height), 1, interpolation);
        // 冷启动：第一次真的画出预览、清晰图的时刻（交给界面之后还要上传纹理、画一帧）。
        if (!_drawnMarked && !ShowingPreview)
        {
            _drawnMarked = true;
            LaunchClock.Mark("imageDrawn");
        }
        if (!_previewDrawnMarked && ShowingPreview)
        {
            _previewDrawnMarked = true;
            LaunchClock.Mark("previewDrawn");
        }
        ds.Transform = basis;
        if (_exactHandle is not null && ExactShown)
        {
            _exactBitmap ??= Upload(Control, _exactHandle);
            if (_exactBitmap is not null)
            {
                var size = _exactBitmap.SizeInPixels;
                ds.DrawImage(_exactBitmap, new Rect(_exactAt.X, _exactAt.Y, size.Width, size.Height),
                    new Rect(0, 0, size.Width, size.Height), 1, CanvasImageInterpolation.NearestNeighbor);
            }
        }
    }

    /// <summary>透明处：棋盘格（#FFFFFF 与 #C8C8C8，每格 8 个有效像素）或纯色。</summary>
    private void FillTransparent(CanvasDrawingSession ds, Rect dest)
    {
        if (_alphaFill is AlphaFill.Solid solid)
        {
            var rgb = solid.Rgb;
            ds.FillRectangle(dest, Color.FromArgb(255, (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb));
            return;
        }
        _checker ??= MakeChecker();
        using var brush = new CanvasImageBrush(Control, _checker)
        {
            ExtendX = CanvasEdgeBehavior.Wrap,
            ExtendY = CanvasEdgeBehavior.Wrap,
            Interpolation = CanvasImageInterpolation.NearestNeighbor,
            Transform = Matrix3x2.CreateScale((float)Scale),
        };
        ds.FillRectangle(dest, brush);
    }

    private CanvasBitmap MakeChecker()
    {
        var bytes = new byte[16 * 16 * 4];
        for (var y = 0; y < 16; y++)
        {
            for (var x = 0; x < 16; x++)
            {
                var v = ((x < 8) ^ (y < 8)) ? (byte)200 : (byte)255;
                var i = (y * 16 + x) * 4;
                bytes[i] = v;
                bytes[i + 1] = v;
                bytes[i + 2] = v;
                bytes[i + 3] = 255;
            }
        }
        return CanvasBitmap.CreateFromBytes(Control, bytes, 16, 16, DirectXPixelFormat.B8G8R8A8UIntNormalized, 96,
            CanvasAlphaMode.Premultiplied);
    }

    private void DrawTransition(CanvasDrawingSession ds, Matrix3x2 basis)
    {
        var (w, h) = PixelSize();
        if (w < 1 || h < 1 || _turnOld is null)
        {
            DrawContent(ds, basis);
            return;
        }
        using var current = new CanvasRenderTarget(Control, w, h, 96);
        using (var cds = current.CreateDrawingSession())
        {
            cds.Clear(Color.FromArgb(0, 0, 0, 0));
            DrawContent(cds, Matrix3x2.Identity);
        }
        var frame = AnyviewCore.TransitionFrame(_turnEffect, _turnForward, TurnProgress);
        var flat = HoldTransition is not null;
        // 旧画面按它原来的大小居中（窗口跟着新图片变了大小时，屏幕上的位置不变）。
        var old = _turnOld.SizeInPixels;
        var offset = new Vector2((w - old.Width) / 2f, (h - old.Height) / 2f);
        ds.Transform = basis;
        if (frame.OldOnTop)
        {
            DrawLayer(ds, current, Vector2.Zero, frame.New, w, h, flat);
            DrawLayer(ds, _turnOld, offset, frame.Old, w, h, flat);
        }
        else
        {
            DrawLayer(ds, _turnOld, offset, frame.Old, w, h, flat);
            DrawLayer(ds, current, Vector2.Zero, frame.New, w, h, flat);
        }
    }

    /// <summary>
    /// 按一层的参数画：平移、以画布左边为轴翻转（透视；自检停住时水平压扁）、不透明度、变暗。
    /// offset 是这一层在画布上的位置（物理像素）。
    /// </summary>
    private static void DrawLayer(CanvasDrawingSession ds, CanvasRenderTarget layer, Vector2 offset, LayerParams p,
        float w, float h, bool flat)
    {
        if (p.Opacity <= 0)
        {
            return;
        }
        var size = layer.SizeInPixels;
        // 变暗：颜色乘以 (1 - shade)，透明处仍是透明（与 macOS 盖一层黑色相同）。
        using var shade = p.Shade > 0
            ? new ColorMatrixEffect
            {
                Source = layer,
                ColorMatrix = new Matrix5x4 { M11 = (float)(1 - p.Shade), M22 = (float)(1 - p.Shade), M33 = (float)(1 - p.Shade), M44 = 1 },
            }
            : null;
        ICanvasImage image = shade is null ? layer : shade;
        var shift = (float)(p.Shift * w);
        if (p.Angle == 0)
        {
            ds.DrawImage(image, offset.X + shift, offset.Y, new Rect(0, 0, size.Width, size.Height), (float)p.Opacity);
            return;
        }
        var theta = (float)(p.Angle * Math.PI / 180);
        Matrix4x4 m;
        if (flat)
        {
            m = Matrix4x4.CreateScale(Math.Max(0, MathF.Cos(theta)), 1, 1);
        }
        else
        {
            var perspective = Matrix4x4.Identity;
            perspective.M34 = -1f / (float)(AnyviewCore.TransitionPerspective() * Math.Max(w, 1));
            // 转轴是画布左边（x = 0），透视以画布中心为准。
            m = Matrix4x4.CreateRotationY(theta)
                * Matrix4x4.CreateTranslation(-w / 2, -h / 2, 0)
                * perspective
                * Matrix4x4.CreateTranslation(w / 2, h / 2, 0);
        }
        using var effect = new Transform3DEffect
        {
            Source = image,
            TransformMatrix = Matrix4x4.CreateTranslation(offset.X, offset.Y, 0) * m * Matrix4x4.CreateTranslation(shift, 0, 0),
            InterpolationMode = CanvasImageInterpolation.Linear,
        };
        // 变换后的画面可能伸到画布外：取画布四周一圈作源区域。DrawImage 把源区域的左上角放在给定的点上，
        // 所以给的点也是 (-w, -h)，画面才在原位。
        ds.DrawImage(effect, -w, -h, new Rect(-w, -h, 3 * w, 3 * h), (float)p.Opacity);
    }

    /// <summary>把核心的图像（BGRA8 预乘）上传成 GPU 纹理。</summary>
    internal static CanvasBitmap? Upload(ICanvasResourceCreator device, ImageHandle handle)
    {
        byte[]? scratch = null;
        return Upload(device, handle, ref scratch);
    }

    /// <summary>
    /// 同上；`scratch` 是上一次用过的数组，大小一样就接着用（动图每一帧都上传一次：不用每帧新分配、清零几 MB）。
    /// 纹理建好后数组里的内容就没用了。只留小的（16 MB 以内），大图的不占着内存。
    /// 试过把大图改成分条上传（先建一张空的纹理，每条最多 8 MB 地填，不在 .NET 里放整张的像素）， 在部分 Windows 环境下
    /// 电脑上量过又改回来了：进程内存没有少（看过几张大图以后还是停在高出两三百 MB 的地方），显存反而多占——那样建的
    /// 纹理是"可以往上画"的那一种，开关 30 个大窗口以后显存多 440 MB，这里的做法是 256 MB。
    /// </summary>
    private static CanvasBitmap? Upload(ICanvasResourceCreator device, ImageHandle handle, ref byte[]? scratch)
    {
        var bytes = CopyPixels(handle, out var width, out var height, scratch);
        scratch = bytes is { Length: <= 16 << 20 } ? bytes : null;
        return bytes is null
            ? null
            : CanvasBitmap.CreateFromBytes(device, bytes, width, height, DirectXPixelFormat.B8G8R8A8UIntNormalized, 96,
                CanvasAlphaMode.Premultiplied);
    }

    /// <summary>把核心的像素（BGRA8 预乘，按行可能有填充）复制成紧凑排列的数组。</summary>
    internal static byte[]? CopyPixels(ImageHandle handle, out int width, out int height, byte[]? reuse = null)
    {
        width = (int)handle.Width();
        height = (int)handle.Height();
        var stride = (int)handle.Stride();
        var address = (nint)handle.PixelAddress();
        if (width <= 0 || height <= 0 || address == 0)
        {
            return null;
        }
        var row = width * 4;
        var length = (long)row * height;
        // 每个字节下面都会写到，不用先清零。
        var bytes = reuse is not null && reuse.LongLength == length
            ? reuse
            : GC.AllocateUninitializedArray<byte>(checked((int)length));
        if (stride == row)
        {
            Marshal.Copy(address, bytes, 0, bytes.Length);
        }
        else
        {
            for (var y = 0; y < height; y++)
            {
                Marshal.Copy(address + (nint)((long)y * stride), bytes, y * row, row);
            }
        }
        GC.KeepAlive(handle);
        return bytes;
    }

    /// <summary>把现在的画面（不含悬浮控件）画到一张位图上（自检截图用）。</summary>
    public CanvasRenderTarget? Snapshot()
    {
        if (Control.Device is null)
        {
            return null;
        }
        var (w, h) = PixelSize();
        if (w < 1 || h < 1)
        {
            return null;
        }
        CanvasRenderTarget? target = null;
        try
        {
            target = new CanvasRenderTarget(Control, w, h, 96);
            using (var ds = target.CreateDrawingSession())
            {
                ds.Clear(_background);
                DrawBackgroundImage(ds);
                if (_turnOld is not null)
                {
                    DrawTransition(ds, Matrix3x2.Identity);
                }
                else
                {
                    DrawContent(ds, Matrix3x2.Identity);
                }
            }
            return target;
        }
        catch (Exception error)
        {
            target?.Dispose();
            if (Control.Device is { } device && device.IsDeviceLost(error.HResult))
            {
                device.RaiseDeviceLost();
            }
            return null;
        }
    }

    private void After(uint ms, Action action)
    {
        Delay.After(_queue, ms, () =>
        {
            if (!_released)
            {
                action();
            }
        });
    }
}
