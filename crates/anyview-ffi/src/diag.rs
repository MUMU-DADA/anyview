//! Windows 版的自检诊断（只给 apps/windows 的自检用）：采样一个线程正在执行的指令地址。
//!
//! 用于排查静止时界面线程 CPU 占用的轻量级采样器：每隔约 1 ms 暂停目标线程采样调用栈，
//! 读它的指令地址（RIP），马上放开。停、读、放开都在这一个本机函数里做完：在 C# 里分三次调用时，调用之间要回到 .NET，
//! 避免采样过程中触发 .NET GC 导致与挂起线程死锁。
//! 也用于采集拖动窗口期间的界面线程热点。

use std::sync::atomic::AtomicU32;

/// 采样 `thread_id` 这个线程约 `duration_ms` 毫秒，把读到的指令地址写进 `out`（最多 `capacity` 个），返回个数。
/// 打不开线程时返回 0。只在 Windows 上有内容，其他系统返回 0。
///
/// # Safety
/// `out` 指向至少 `capacity` 个 `u64` 的可写内存（`capacity` 为 0 时可以为空）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anyview_diag_sample_thread(
    thread_id: u32,
    duration_ms: u32,
    out: *mut u64,
    capacity: usize,
) -> usize {
    // SAFETY: 同上；没有停止标志。
    unsafe {
        anyview_diag_sample_thread_until(thread_id, duration_ms, std::ptr::null(), out, capacity)
    }
}

/// 同 [`anyview_diag_sample_thread`]，另外 `stop` 指向的数变成非 0 时提前停（拖动窗口时从开始采到松开，
/// 拖多久事先不知道；`max_ms` 是最长的时间）。`stop` 可以为空。
///
/// # Safety
/// `out` 指向至少 `capacity` 个 `u64` 的可写内存；`stop` 为空，或指向 4 字节对齐、采样期间一直有效的 32 位整数
/// （别的线程可以随时改它）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anyview_diag_sample_thread_until(
    thread_id: u32,
    max_ms: u32,
    stop: *const u32,
    out: *mut u64,
    capacity: usize,
) -> usize {
    if out.is_null() || capacity == 0 {
        return 0;
    }
    // SAFETY: 调用方保证 stop 为空或指向对齐、有效的 32 位整数；AtomicU32 与 u32 的大小、对齐相同。
    let stop = unsafe { stop.cast::<AtomicU32>().as_ref() };
    #[cfg(windows)]
    {
        // SAFETY: 调用方保证 out 有 capacity 个元素。
        let out = unsafe { std::slice::from_raw_parts_mut(out, capacity) };
        windows::sample(thread_id, max_ms, stop, out)
    }
    #[cfg(not(windows))]
    {
        let _ = (thread_id, max_ms, stop);
        0
    }
}

#[cfg(windows)]
mod windows {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    type Handle = isize;

    const THREAD_SUSPEND_RESUME: u32 = 0x0002;
    const THREAD_GET_CONTEXT: u32 = 0x0008;
    const THREAD_QUERY_INFORMATION: u32 = 0x0040;
    /// CONTEXT_AMD64 | CONTEXT_CONTROL：只要控制寄存器（含 Rip）。
    const CONTEXT_CONTROL: u32 = 0x0010_0001;
    const CONTEXT_FLAGS_OFFSET: usize = 0x30;
    const RIP_OFFSET: usize = 0xF8;

    /// x64 的 CONTEXT：1232 字节，16 字节对齐。只按偏移读写需要的两个字段。
    #[repr(C, align(16))]
    struct Context([u8; 1232]);

    /// 高精度的可等待计时器（Windows 10 1803 起）：只让这个计时器准到 1 毫秒，不像 timeBeginPeriod 那样
    /// 改整个进程的计时器精度（拖动时采样，不该改变正在量的程序本身的计时）。
    const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x0000_0002;
    const TIMER_ALL_ACCESS: u32 = 0x001F_0003;
    const INFINITE: u32 = u32::MAX;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> Handle;
        fn SuspendThread(thread: Handle) -> u32;
        fn ResumeThread(thread: Handle) -> u32;
        fn GetThreadContext(thread: Handle, context: *mut Context) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn CreateWaitableTimerExW(
            attributes: *const std::ffi::c_void,
            name: *const u16,
            flags: u32,
            access: u32,
        ) -> Handle;
        fn SetWaitableTimer(
            timer: Handle,
            due: *const i64,
            period: i32,
            completion: *const std::ffi::c_void,
            argument: *const std::ffi::c_void,
            resume: i32,
        ) -> i32;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    }

    /// 等 1 毫秒：有高精度计时器时用它，没有时（旧的 Windows、Wine）忙等。
    ///
    /// # Safety
    /// `timer` 为 0，或是 CreateWaitableTimerExW 建的、还没关的计时器。
    unsafe fn wait_one_ms(timer: Handle) {
        if timer != 0 {
            // 相对时间，单位 100 纳秒，负数表示从现在算起。
            let due: i64 = -10_000;
            // SAFETY: timer 有效（调用方保证），due 是本地变量，其余参数为空。
            let set = unsafe {
                SetWaitableTimer(timer, &due, 0, std::ptr::null(), std::ptr::null(), 0) != 0
            };
            if set {
                // SAFETY: 同上。
                unsafe { WaitForSingleObject(timer, INFINITE) };
                return;
            }
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(1) {
            std::thread::yield_now();
        }
    }

    pub fn sample(
        thread_id: u32,
        duration_ms: u32,
        stop: Option<&AtomicU32>,
        out: &mut [u64],
    ) -> usize {
        // SAFETY: 只调系统函数；句柄在本函数里打开、关闭；context 是本地的、按 CONTEXT 的大小和对齐。
        unsafe {
            let thread = OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                thread_id,
            );
            if thread == 0 {
                return 0;
            }
            let timer = CreateWaitableTimerExW(
                std::ptr::null(),
                std::ptr::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS,
            );
            let mut context = Context([0; 1232]);
            let start = Instant::now();
            let limit = Duration::from_millis(u64::from(duration_ms));
            let mut count = 0;
            while start.elapsed() < limit
                && count < out.len()
                && stop.is_none_or(|flag| flag.load(Ordering::Relaxed) == 0)
            {
                context.0.fill(0);
                context.0[CONTEXT_FLAGS_OFFSET..CONTEXT_FLAGS_OFFSET + 4]
                    .copy_from_slice(&CONTEXT_CONTROL.to_le_bytes());
                if SuspendThread(thread) != u32::MAX {
                    let ok = GetThreadContext(thread, &mut context) != 0;
                    ResumeThread(thread);
                    if ok {
                        let mut rip = [0u8; 8];
                        rip.copy_from_slice(&context.0[RIP_OFFSET..RIP_OFFSET + 8]);
                        out[count] = u64::from_le_bytes(rip);
                        count += 1;
                    }
                }
                wait_one_ms(timer);
            }
            if timer != 0 {
                CloseHandle(timer);
            }
            CloseHandle(thread);
            count
        }
    }
}
