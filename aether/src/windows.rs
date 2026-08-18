use std::ptr::NonNull;

use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, FreeConsole, GetStdHandle, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, GetPriorityClass, IDLE_PRIORITY_CLASS,
    NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
    SetPriorityClass, SetProcessInformation,
};

/// Whether an inherited priority class is one to climb out of.
///
/// A process starts at whatever priority its parent had, and only the two classes below normal are
/// worth correcting. Anything at normal or above is either what Windows gives a normal launch or
/// something the player chose, and Aether has no business raising itself past the shell that
/// started it.
fn is_below_normal_priority(priority_class: u32) -> bool {
    priority_class == IDLE_PRIORITY_CLASS || priority_class == BELOW_NORMAL_PRIORITY_CLASS
}

/// Undo the scheduling a background launcher hands down to the process it starts.
///
/// Reported by a player whose Fifine control deck launches Aether: from the deck it was slow, from
/// the same `aether.exe` by hand it was fine. Same binary, same machine, same graphics card, so
/// nothing about the rendering explains it. What differs is who started the process.
///
/// Two things are inherited from a parent, and a deck, tray helper or macro host is a background
/// process that has both. Priority class is inherited outright, so a helper running below normal
/// hands that down. Power throttling is the one that bites hardest on Windows 11: a process the
/// scheduler considers background is put in `EcoQoS`, which parks it on efficiency cores and holds
/// the clocks down. For something drawing frames that reads exactly like a slow computer.
///
/// Neither is raised above what a normal launch would give. Throttling is switched off outright,
/// because Aether is never a background task while it is running, and priority is only lifted as
/// far as normal, and only from below it.
pub(super) fn restore_foreground_scheduling() {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing and is always valid.
    let process = unsafe { GetCurrentProcess() };

    let throttling = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        // Name the mechanism as one being decided, then decide it off. Leaving it out of the mask
        // means "no opinion", which is not the same as "do not throttle".
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    // SAFETY: the struct matches ProcessPowerThrottling, and the size is its own. A failure here
    // is ignorable: the call is unsupported before Windows 10 2004, where there is no EcoQoS to
    // opt out of.
    unsafe {
        SetProcessInformation(
            process,
            ProcessPowerThrottling,
            (&raw const throttling).cast(),
            size_of_val(&throttling) as u32,
        );
    }

    // SAFETY: a process pseudo-handle is valid for both calls. GetPriorityClass returns 0 on
    // failure, which is not one of the classes tested for.
    unsafe {
        if is_below_normal_priority(GetPriorityClass(process)) {
            SetPriorityClass(process, NORMAL_PRIORITY_CLASS);
        }
    }
}

/// RAII guard for the attached parent console.
/// Frees the console on drop if it was successfully attached.
pub(super) struct Console {
    attached: bool,
}

impl Console {
    // When linked with the windows subsystem windows won't automatically attach
    // to the console of the parent process, so we do it explicitly. This fails
    // silently if the parent has no console.
    //
    // However, if stdout/stderr are already redirected (e.g., `ruffle.exe > file.txt`),
    // we should NOT attach to the console as that would bypass the redirection.
    // See: https://github.com/ruffle-rs/ruffle/issues/9145
    pub(super) fn attach() -> Self {
        // Check if stdout is already redirected to a file or pipe
        // SAFETY: STD_OUTPUT_HANDLE is a valid standard device constant.
        let stdout_handle = NonNull::new(unsafe { GetStdHandle(STD_OUTPUT_HANDLE) });

        let should_attach = stdout_handle.is_none_or(|mut handle| {
            // SAFETY: GetFileType accepts any handle value including INVALID_HANDLE_VALUE,
            // returning FILE_TYPE_UNKNOWN in that case.
            let file_type = unsafe { GetFileType(handle.as_mut()) };

            // If output is redirected to a file or pipe, don't attach to console
            // as that would bypass the redirection
            !matches!(file_type, FILE_TYPE_DISK | FILE_TYPE_PIPE)
        });

        let attached = if should_attach {
            // Otherwise, attach to parent console for interactive use
            // SAFETY: ATTACH_PARENT_PROCESS is a valid constant for AttachConsole.
            // This call fails silently if the parent has no console.
            (unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }) != 0
        } else {
            false
        };

        Self { attached }
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        if self.attached {
            // Without explicitly detaching, cmd won't redraw its prompt.
            // SAFETY: We only call FreeConsole if it was previously successfully attached.
            unsafe { FreeConsole() };
        }
    }
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;

    /// Only the two classes below normal are corrected. Raising anything else would mean a client
    /// that promotes itself past the shell that started it.
    #[test]
    fn only_the_classes_below_normal_are_corrected() {
        assert!(is_below_normal_priority(IDLE_PRIORITY_CLASS));
        assert!(is_below_normal_priority(BELOW_NORMAL_PRIORITY_CLASS));

        assert!(!is_below_normal_priority(NORMAL_PRIORITY_CLASS));
        assert!(!is_below_normal_priority(
            windows_sys::Win32::System::Threading::ABOVE_NORMAL_PRIORITY_CLASS
        ));
        assert!(!is_below_normal_priority(
            windows_sys::Win32::System::Threading::HIGH_PRIORITY_CLASS
        ));
        // GetPriorityClass reports 0 when it fails, which must not read as "too low".
        assert!(!is_below_normal_priority(0));
    }
}

/// How much memory the process is actually holding, in bytes.
///
/// The private working set is the number Task Manager shows in its Memory column, which is the one
/// every leak report arrives quoting. Reporting the same number the reporter is looking at is the
/// point: a census that measured something subtly different would have to be reconciled with their
/// screenshot before it could be believed.
pub fn working_set_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };

    // SAFETY: `counters` is a correctly sized, zeroed `PROCESS_MEMORY_COUNTERS`, and the pseudo
    // handle from `GetCurrentProcess` is always valid and needs no closing.
    unsafe {
        let mut counters = std::mem::zeroed::<PROCESS_MEMORY_COUNTERS>();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) == 0 {
            return None;
        }
        Some(counters.WorkingSetSize as u64)
    }
}

/// Bytes the process has committed, which is not the same as what it has resident.
///
/// The working set is what Windows currently keeps in physical memory and it can be trimmed
/// without the process releasing anything. Commit is what the process has actually asked for and
/// still owns, so a run where commit climbs while the Rust heap holds steady is memory the client
/// did not allocate -- the graphics driver, or the allocator holding freed pages back.
#[cfg_attr(not(feature = "metrics"), expect(dead_code))]
pub fn private_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };

    // SAFETY: `counters` is a correctly sized, zeroed `PROCESS_MEMORY_COUNTERS_EX`, and the API
    // takes the size so it knows the extended layout was passed. The pseudo handle from
    // `GetCurrentProcess` is always valid and needs no closing.
    unsafe {
        let mut counters = std::mem::zeroed::<PROCESS_MEMORY_COUNTERS_EX>();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            size,
        ) == 0
        {
            return None;
        }
        Some(counters.PrivateUsage as u64)
    }
}
