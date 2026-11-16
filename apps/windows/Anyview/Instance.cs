// 一个程序多个窗口。Windows 上从资源管理器每打开一次图片都会启动一个新进程：新进程先看看有没有已经开着的
// Anyview（同一用户、同一登录会话、同一个设置文件）。有就把要打开的路径交给它、自己马上退出（还没建窗口，很快），
// 已运行的实例按多窗口设置开新窗口，或在空着的、前面的窗口里换图；没有就自己作为主实例。自检时不交也不接。
// 谁是"开着的那个"由一个命名互斥体决定（先启动的占着，进程退出时系统收回）；路径通过本机的命名管道交过去
// （只有同一用户能连）。开着的那个的界面线程真的开始处理了才回 "ok"；它卡住了（2.5 秒内没开始处理）就回 "busy"、
// 撤回这个请求，新进程自己开窗口（不会两边都打开）。
// 消息：4 字节长度（小端）加 UTF-8 的 "anyview-open 1\n" 和要打开的绝对路径（可以为空，只把窗口叫到前面）。
using System;
using System.Collections.Generic;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

namespace Anyview;

internal static partial class Instance
{
    private const string Header = "anyview-open 1\n";
    private const int MaxMessage = 64 * 1024;
    /// <summary>连接、读写的超时。</summary>
    private static readonly TimeSpan s_timeout = TimeSpan.FromSeconds(3);
    /// <summary>新进程等回话最多多久：比开着的那个等界面线程的时间长，免得那边刚开始处理、这边已经放弃。</summary>
    private static readonly TimeSpan s_replyTimeout = TimeSpan.FromSeconds(5);
    /// <summary>开着的那个等自己的界面线程开始处理最多多久。</summary>
    private static readonly TimeSpan s_uiTimeout = TimeSpan.FromSeconds(2.5);
    /// <summary>占着这个互斥体就是"开着的那个"（程序退出时系统收回，不用手动放）。</summary>
    private static Mutex? s_mutex;
    private static string? s_name;
    private static readonly List<Request> s_pending = new();
    private static Action<Request>? s_deliver;

    /// <summary>
    /// 别的进程交来的一个请求。界面线程开始处理时调 TryStart，接收线程等不及时撤回（TryCancel）：两者只有一个成功，
    /// 所以不会既让新进程自己开窗口、这边又打开一次。
    /// </summary>
    internal sealed class Request
    {
        private int _state; // 0 等着，1 已开始，2 已撤回
        private readonly ManualResetEventSlim _settled = new();

        public Request(string? path) => Path = path;

        /// <summary>要打开的绝对路径；为空时只把窗口叫到前面。</summary>
        public string? Path { get; }

        public bool Started => Volatile.Read(ref _state) == 1;

        public bool TryStart() => Settle(1);

        public bool TryCancel() => Settle(2);

        public void Wait(TimeSpan timeout) => _settled.Wait(timeout);

        private bool Settle(int state)
        {
            if (Interlocked.CompareExchange(ref _state, state, 0) != 0)
            {
                return false;
            }
            _settled.Set();
            return true;
        }
    }

    /// <summary>这个进程是"开着的那个"（接收别的进程交来的图片）。</summary>
    public static bool IsPrimary => s_mutex is not null;

    private enum Forward
    {
        Done,
        NoInstance,
        Failed,
    }

    /// <summary>
    /// 启动时调用（自检时不调用）：有开着的就把 path 交给它，返回 false（这个进程就此退出）；
    /// 否则开始接收别的进程交来的图片，返回 true（照常启动）。
    /// </summary>
    public static bool Start(string? settingsPath, string? path)
    {
        try
        {
            var name = Name(settingsPath);
            var result = TryForward(name, path);
            if (result == Forward.Done)
            {
                return false;
            }
            if (result == Forward.NoInstance)
            {
                if (TryBecomePrimary(name))
                {
                    return true;
                }
                // 几乎同时启动的另一个进程抢先占了：交给它。
                if (TryForward(name, path) == Forward.Done)
                {
                    return false;
                }
            }
        }
        catch (Exception error)
        {
            Crash.Write("Instance.Start", error);
        }
        // 开着的那个不回话：这个进程自己开窗口（不接收别的进程）。
        return true;
    }

    /// <summary>
    /// 应用准备好了：交来的请求交给 deliver（在接收线程上调用；deliver 自己转到界面线程，在那里先 TryStart，成功了才处理）。
    /// 之前收到的一起交出。
    /// </summary>
    public static void SetHandler(Action<Request> deliver)
    {
        List<Request> pending;
        lock (s_pending)
        {
            s_deliver = deliver;
            pending = new List<Request>(s_pending);
            s_pending.Clear();
        }
        foreach (var request in pending)
        {
            deliver(request);
        }
    }

    /// <summary>
    /// 管道和互斥体的名字：同一用户、同一登录会话、同一个设置文件的进程用同一个
    /// （用另一个设置文件启动的，如手动测试用的，自成一组）。
    /// </summary>
    private static string Name(string? settingsPath)
    {
        var key = settingsPath is null ? "default" : Path.GetFullPath(settingsPath).ToUpperInvariant();
        // FNV-1a（不加载加密库，省启动时间）。
        var hash = 14695981039346656037UL;
        foreach (var b in Encoding.UTF8.GetBytes(key))
        {
            hash = (hash ^ b) * 1099511628211UL;
        }
        return $"Anyview-{System.Diagnostics.Process.GetCurrentProcess().SessionId}-{hash:x16}";
    }

    private static Forward TryForward(string name, string? path)
    {
        if (!Mutex.TryOpenExisting(@"Local\" + name, out var existing))
        {
            return Forward.NoInstance;
        }
        existing.Dispose();
        try
        {
            using var client = new NamedPipeClientStream(".", name, PipeDirection.InOut,
                PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
            client.Connect((int)s_timeout.TotalMilliseconds);
            // 让开着的那个能把窗口放到前面（系统只允许前台进程这样做，这个进程是用户刚启动的）。
            if (GetNamedPipeServerProcessId(client.SafePipeHandle, out var pid))
            {
                AllowSetForegroundWindow(pid);
            }
            var body = Encoding.UTF8.GetBytes(Header + (path ?? ""));
            var message = new byte[4 + body.Length];
            BitConverter.TryWriteBytes(message.AsSpan(0, 4), body.Length);
            body.CopyTo(message, 4);
            using var cancel = new CancellationTokenSource(s_replyTimeout);
            client.WriteAsync(message, cancel.Token).AsTask().GetAwaiter().GetResult();
            client.Flush();
            var reply = new byte[8];
            var count = client.ReadAsync(reply, cancel.Token).AsTask().GetAwaiter().GetResult();
            return Encoding.ASCII.GetString(reply, 0, count).StartsWith("ok", StringComparison.Ordinal) ? Forward.Done : Forward.Failed;
        }
        catch (Exception)
        {
            return Forward.Failed;
        }
    }

    private static bool TryBecomePrimary(string name)
    {
        var mutex = new Mutex(true, @"Local\" + name, out var createdNew);
        if (!createdNew)
        {
            mutex.Dispose();
            return false;
        }
        s_mutex = mutex;
        s_name = name;
        new Thread(Serve) { IsBackground = true, Name = "anyview-instance" }.Start();
        return true;
    }

    /// <summary>接收线程：一次接一个连接，读完把请求交给界面线程，等它开始处理了回 "ok"，等不到就撤回、回 "busy"。</summary>
    private static void Serve()
    {
        while (true)
        {
            try
            {
                using var server = new NamedPipeServerStream(s_name!, PipeDirection.InOut,
                    NamedPipeServerStream.MaxAllowedServerInstances, PipeTransmissionMode.Byte,
                    PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
                server.WaitForConnection();
                using var cancel = new CancellationTokenSource(s_timeout);
                if (Read(server, cancel.Token) is not { } message || !message.StartsWith(Header, StringComparison.Ordinal))
                {
                    continue;
                }
                var path = message[Header.Length..];
                var request = new Request(path.Length == 0 ? null : path);
                Deliver(request);
                request.Wait(s_uiTimeout);
                request.TryCancel();
                var reply = request.Started ? "ok\n"u8.ToArray() : "busy\n"u8.ToArray();
                using var send = new CancellationTokenSource(s_timeout);
                server.WriteAsync(reply, send.Token).AsTask().GetAwaiter().GetResult();
                server.Flush();
            }
            catch (Exception)
            {
                // 对方中途断开、超时：等一下再接下一个（不让出错时空转）。
                Thread.Sleep(100);
            }
        }
    }

    private static string? Read(Stream stream, CancellationToken cancel)
    {
        var head = ReadExactly(stream, 4, cancel);
        var length = head is null ? -1 : BitConverter.ToInt32(head, 0);
        if (length < Header.Length || length > MaxMessage)
        {
            return null;
        }
        var body = ReadExactly(stream, length, cancel);
        return body is null ? null : Encoding.UTF8.GetString(body);
    }

    private static byte[]? ReadExactly(Stream stream, int count, CancellationToken cancel)
    {
        var buffer = new byte[count];
        var read = 0;
        while (read < count)
        {
            var n = stream.ReadAsync(buffer.AsMemory(read, count - read), cancel).AsTask().GetAwaiter().GetResult();
            if (n <= 0)
            {
                return null;
            }
            read += n;
        }
        return buffer;
    }

    private static void Deliver(Request request)
    {
        Action<Request>? deliver;
        lock (s_pending)
        {
            deliver = s_deliver;
            if (deliver is null)
            {
                s_pending.Add(request);
                return;
            }
        }
        deliver(request);
    }

    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool GetNamedPipeServerProcessId(SafePipeHandle pipe, out uint processId);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool AllowSetForegroundWindow(uint processId);
}
