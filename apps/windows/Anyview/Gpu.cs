// 显卡信息（自检报告用）：DXGI 列出的显卡，按系统给的顺序。Win2D 的共享设备建在第 0 块上（D3D11CreateDevice 不指定显卡时用它）。
// 冷启动阶段创建 D3D11/Win2D 显卡设备可能耗时 200 ms 以上（多显卡或虚拟显示适配器环境下尤为明显），
// 报告里记下第 0 块是哪一块，才能判断慢在哪里。只读信息，不建设备。
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace Anyview;

internal static unsafe class Gpu
{
    [DllImport("dxgi.dll")]
    private static extern int CreateDXGIFactory1(in Guid riid, out nint factory);

    private static readonly Guid IID_IDXGIFactory1 = new("770aae78-f26f-4dba-a829-253c83d1b387");

    [StructLayout(LayoutKind.Sequential)]
    private struct DXGI_ADAPTER_DESC1
    {
        public fixed char Description[128];
        public uint VendorId;
        public uint DeviceId;
        public uint SubSysId;
        public uint Revision;
        public nuint DedicatedVideoMemory;
        public nuint DedicatedSystemMemory;
        public nuint SharedSystemMemory;
        public uint LuidLow;
        public int LuidHigh;
        public uint Flags;
    }

    /// <summary>"序号: 名称（厂商、显存、是否软件）"，按 DXGI 的顺序；读不出来时为空表。</summary>
    public static List<string> Adapters()
    {
        var list = new List<string>();
        try
        {
            if (CreateDXGIFactory1(IID_IDXGIFactory1, out var factory) < 0 || factory == 0)
            {
                return list;
            }
            try
            {
                // 虚表：IUnknown 3 个、IDXGIObject 4 个、IDXGIFactory 5 个，之后是 IDXGIFactory1::EnumAdapters1。
                var enumAdapters1 = (delegate* unmanaged[Stdcall]<nint, uint, nint*, int>)(*(nint**)factory)[12];
                for (uint i = 0; i < 16; i++)
                {
                    nint adapter = 0;
                    if (enumAdapters1(factory, i, &adapter) < 0 || adapter == 0)
                    {
                        break;
                    }
                    try
                    {
                        // IUnknown 3 个、IDXGIObject 4 个、IDXGIAdapter 3 个，之后是 IDXGIAdapter1::GetDesc1。
                        var getDesc1 = (delegate* unmanaged[Stdcall]<nint, DXGI_ADAPTER_DESC1*, int>)(*(nint**)adapter)[10];
                        DXGI_ADAPTER_DESC1 desc;
                        if (getDesc1(adapter, &desc) >= 0)
                        {
                            var name = new string(desc.Description).TrimEnd('\0').Trim();
                            var software = (desc.Flags & 2) != 0 ? "，软件" : "";
                            list.Add($"{i}: {name}（厂商 0x{desc.VendorId:X4}，显存 {(ulong)desc.DedicatedVideoMemory / 1048576} MB{software}）");
                        }
                    }
                    finally
                    {
                        Release(adapter);
                    }
                }
            }
            finally
            {
                Release(factory);
            }
        }
        catch (Exception error)
        {
            list.Add($"读不出显卡：{error.Message}");
        }
        return list;
    }

    private static void Release(nint unknown) =>
        ((delegate* unmanaged[Stdcall]<nint, uint>)(*(nint**)unknown)[2])(unknown);
}
