// 自检用的简易采样（找静止时界面线程的 CPU 用在哪里）：每隔约 1 毫秒把界面线程停一下、读它正在执行的指令地址（RIP），
// 马上放开，按地址所在的模块计数。界面线程在等消息时停在 win32u.dll（或 ntdll.dll）里，其他模块里的次数就是它在忙的地方。
// 停、读、放开在核心 DLL 的本机函数里一次做完（anyview_diag_sample_thread）：分三次从 C# 调用时，中间回到 .NET 赶上
// 垃圾回收会卡住（垃圾回收在等被停住的界面线程）。拖动窗口时用 Run：从开始移动采到松开（DragProbe）。
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;

namespace Anyview;

internal static unsafe partial class Sampler
{
    /// <summary>核心 DLL 里的采样函数（crates/anyview-ffi/src/diag.rs）：停、读地址、放开都在本机代码里做完。</summary>
    [LibraryImport("anyview_ffi")]
    private static partial nuint anyview_diag_sample_thread(uint threadId, uint durationMs, ulong* output, nuint capacity);

    /// <summary>同上，stop 指向的数变成非 0 时提前停。</summary>
    [LibraryImport("anyview_ffi")]
    private static partial nuint anyview_diag_sample_thread_until(uint threadId, uint maxMs, int* stop, ulong* output, nuint capacity);

    /// <summary>一次进行中的采样（拖动窗口时从开始移动采到松开）：Stop 以后 Result 很快完成。</summary>
    public sealed class Run
    {
        /// <summary>停止标志：放在不会被垃圾回收挪动的数组里，本机函数一直读它。</summary>
        private readonly int[] _flag = GC.AllocateArray<int>(1, pinned: true);
        public Task<List<string>> Result { get; }

        public Run(uint threadId, int maxMs)
        {
            var flag = _flag;
            Result = Task.Run(() =>
            {
                try
                {
                    // 大约每毫秒一次。
                    var addresses = new ulong[maxMs + 64];
                    int count;
                    fixed (int* stop = flag)
                    fixed (ulong* output = addresses)
                    {
                        count = (int)anyview_diag_sample_thread_until(threadId, (uint)maxMs, stop, output, (nuint)addresses.Length);
                    }
                    return Histogram(addresses, count);
                }
                catch (Exception error)
                {
                    return new List<string> { $"采样出错：{error.Message}" };
                }
            });
        }

        public void Stop() => Volatile.Write(ref _flag[0], 1);
    }

    /// <summary>
    /// 采样 threadId 这个线程 ms 毫秒：返回按模块统计的次数（多的在前，带百分比），以及非等待模块里最常见的几个地址
    /// （模块+偏移，以后可以对着符号查是哪个函数）。
    /// </summary>
    public static Task<List<string>> SampleAsync(uint threadId, int ms) => Task.Run(() =>
    {
        try
        {
            return Sample(threadId, ms);
        }
        catch (Exception error)
        {
            return new List<string> { $"采样出错：{error.Message}" };
        }
    });

    private static List<string> Sample(uint threadId, int ms)
    {
        var addresses = new ulong[Math.Max(ms * 2, 64)];
        int count;
        fixed (ulong* output = addresses)
        {
            count = (int)anyview_diag_sample_thread(threadId, (uint)ms, output, (nuint)addresses.Length);
        }
        return Histogram(addresses, count);
    }

    /// <summary>按地址所在的模块统计（多的在前，带百分比），再列出非等待模块里最常见的几个地址。</summary>
    private static List<string> Histogram(ulong[] addresses, int count)
    {
        if (count == 0)
        {
            return new List<string> { "没有采到（打不开线程）" };
        }
        // 模块的地址范围。
        var modules = new List<(ulong Start, ulong End, string Name)>();
        foreach (ProcessModule module in Process.GetCurrentProcess().Modules)
        {
            var start = (ulong)module.BaseAddress.ToInt64();
            modules.Add((start, start + (ulong)module.ModuleMemorySize, module.ModuleName));
        }
        // 按模块统计。
        var byModule = new Dictionary<string, int>();
        var byAddress = new Dictionary<string, int>();
        for (var i = 0; i < count; i++)
        {
            var rip = addresses[i];
            var module = modules.FirstOrDefault(m => rip >= m.Start && rip < m.End);
            var name = module.Name ?? "?";
            byModule[name] = byModule.GetValueOrDefault(name) + 1;
            if (name is not ("win32u.dll" or "ntdll.dll"))
            {
                var key = module.Name is null ? $"0x{rip:X}" : $"{name}+0x{rip - module.Start:X}";
                byAddress[key] = byAddress.GetValueOrDefault(key) + 1;
            }
        }
        var result = new List<string> { $"共 {count} 次" };
        result.AddRange(byModule.OrderByDescending(p => p.Value).Take(8)
            .Select(p => $"{p.Key} {p.Value}（{100.0 * p.Value / count:0.#}%）"));
        result.AddRange(byAddress.OrderByDescending(p => p.Value).Take(6).Select(p => $"  {p.Key} {p.Value}"));
        return result;
    }
}
