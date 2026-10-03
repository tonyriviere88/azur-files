//! What this process is costing: the private working set, and the GDI and user handles.
//!
//! The Windows half of [`super::process_memory`]. The Rust heap is only part of a process that
//! also holds shell, COM and GDI allocations, so a leak test that reads only the allocator is
//! measuring the wrong number.

#[cfg(windows)]
pub(crate) fn process_memory() -> (usize, u32, u32) {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS, GR_USEROBJECTS,
    };

    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct is told its own size, and the handle is this process's
    // pseudo-handle, which needs no closing.
    unsafe {
        let me = GetCurrentProcess();
        let _ = GetProcessMemoryInfo(
            me,
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        );
        (
            counters.PrivateUsage,
            GetGuiResources(me, GR_GDIOBJECTS),
            GetGuiResources(me, GR_USEROBJECTS),
        )
    }
}
