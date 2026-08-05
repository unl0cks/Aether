//! Tracks fatal GPU faults so Aether can report and shut down cleanly instead of
//! panicking from inside wgpu's default error handler.
//!
//! wgpu reports device trouble through three separate channels, and each needs
//! different handling:
//!
//! * `Device::set_device_lost_callback` — the authoritative signal for a driver
//!   reset or GPU removal. Without a callback installed, wgpu drops these on the
//!   floor (`ErrorType::DeviceLost => return`), so nothing else ever learns the
//!   device died.
//! * `Device::on_uncaptured_error` — validation, out-of-memory and internal
//!   errors. Without a handler installed these reach `default_error_handler`,
//!   which panics.
//! * `handle_error_fatal` — used by a handful of entry points such as
//!   `Buffer::get_mapped_range`. This panics unconditionally and *cannot* be
//!   intercepted, so those calls must be avoided once the device is faulted.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// What kind of fault took the device down.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceFaultKind {
    /// The device was lost: driver reset, TDR, or GPU removal.
    Lost,
    /// wgpu reported a validation error that nothing captured.
    Validation,
    /// The device ran out of memory.
    OutOfMemory,
    /// wgpu reported an internal error.
    Internal,
}

impl DeviceFaultKind {
    /// A short, user-facing description of what went wrong.
    pub fn summary(self) -> &'static str {
        match self {
            DeviceFaultKind::Lost => "The graphics device was lost",
            DeviceFaultKind::Validation => "The graphics device reported a validation error",
            DeviceFaultKind::OutOfMemory => "The graphics device ran out of memory",
            DeviceFaultKind::Internal => "The graphics driver reported an internal error",
        }
    }
}

/// The first fatal fault observed on a device, with whatever detail wgpu gave us.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceFault {
    pub kind: DeviceFaultKind,
    pub detail: String,
}

impl DeviceFault {
    pub fn new(kind: DeviceFaultKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

/// Shared, cheaply-cloneable record of whether the GPU device is still usable.
///
/// The `faulted` flag is a lock-free read so render hot paths can check it every
/// frame without contending on the mutex that carries the detail string.
#[derive(Clone, Debug)]
pub struct DeviceStatus {
    inner: Arc<DeviceStatusInner>,
}

#[derive(Debug)]
struct DeviceStatusInner {
    faulted: AtomicBool,
    fault: Mutex<Option<DeviceFault>>,
}

impl Default for DeviceStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceStatus {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DeviceStatusInner {
                faulted: AtomicBool::new(false),
                fault: Mutex::new(None),
            }),
        }
    }

    /// Whether the device has suffered a fault and must no longer be used.
    pub fn is_faulted(&self) -> bool {
        self.inner.faulted.load(Ordering::Acquire)
    }

    /// Record a fault. Only the first one sticks — later faults are almost always
    /// cascading consequences of the first, and the first is the useful one to report.
    ///
    /// Returns `true` if this call was the one that recorded the fault.
    pub fn record(&self, fault: DeviceFault) -> bool {
        let mut slot = self
            .inner
            .fault
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return false;
        }
        *slot = Some(fault);
        self.inner.faulted.store(true, Ordering::Release);
        true
    }

    /// The first fault recorded, if any.
    pub fn fault(&self) -> Option<DeviceFault> {
        self.inner
            .fault
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// Classify an uncaptured wgpu error into the fault we should record.
pub fn fault_from_uncaptured_error(error: &wgpu::Error) -> DeviceFault {
    match error {
        wgpu::Error::OutOfMemory { .. } => {
            DeviceFault::new(DeviceFaultKind::OutOfMemory, error.to_string())
        }
        wgpu::Error::Validation { description, .. } => {
            DeviceFault::new(DeviceFaultKind::Validation, description.clone())
        }
        wgpu::Error::Internal { description, .. } => {
            DeviceFault::new(DeviceFaultKind::Internal, description.clone())
        }
    }
}

/// Classify a device-lost notification.
///
/// `DeviceLostReason::Destroyed` is the normal consequence of us calling
/// `Device::destroy` during shutdown, so it is not a fault.
pub fn fault_from_device_lost(reason: wgpu::DeviceLostReason, detail: &str) -> Option<DeviceFault> {
    match reason {
        wgpu::DeviceLostReason::Destroyed => None,
        wgpu::DeviceLostReason::Unknown => {
            Some(DeviceFault::new(DeviceFaultKind::Lost, detail.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct TestSource;

    impl fmt::Display for TestSource {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("test source")
        }
    }

    impl std::error::Error for TestSource {}

    #[test]
    fn starts_unfaulted() {
        let status = DeviceStatus::new();
        assert!(!status.is_faulted());
        assert_eq!(status.fault(), None);
    }

    #[test]
    fn records_a_fault() {
        let status = DeviceStatus::new();
        assert!(status.record(DeviceFault::new(DeviceFaultKind::Lost, "gone")));
        assert!(status.is_faulted());
        assert_eq!(
            status.fault(),
            Some(DeviceFault::new(DeviceFaultKind::Lost, "gone"))
        );
    }

    #[test]
    fn keeps_the_first_fault_and_rejects_later_ones() {
        let status = DeviceStatus::new();
        assert!(status.record(DeviceFault::new(DeviceFaultKind::Lost, "first")));
        assert!(!status.record(DeviceFault::new(DeviceFaultKind::Validation, "second")));
        assert_eq!(
            status.fault(),
            Some(DeviceFault::new(DeviceFaultKind::Lost, "first"))
        );
    }

    #[test]
    fn clones_share_state() {
        let status = DeviceStatus::new();
        let clone = status.clone();
        clone.record(DeviceFault::new(DeviceFaultKind::Internal, "boom"));
        assert!(status.is_faulted());
    }

    #[test]
    fn classifies_validation_errors() {
        let error = wgpu::Error::Validation {
            source: Box::new(TestSource),
            description: "Texture with '' label is invalid".to_owned(),
        };
        assert_eq!(
            fault_from_uncaptured_error(&error),
            DeviceFault::new(
                DeviceFaultKind::Validation,
                "Texture with '' label is invalid"
            )
        );
    }

    #[test]
    fn classifies_out_of_memory_errors() {
        let error = wgpu::Error::OutOfMemory {
            source: Box::new(TestSource),
        };
        assert_eq!(
            fault_from_uncaptured_error(&error).kind,
            DeviceFaultKind::OutOfMemory
        );
    }

    #[test]
    fn classifies_internal_errors() {
        let error = wgpu::Error::Internal {
            source: Box::new(TestSource),
            description: "driver said no".to_owned(),
        };
        assert_eq!(
            fault_from_uncaptured_error(&error),
            DeviceFault::new(DeviceFaultKind::Internal, "driver said no")
        );
    }

    #[test]
    fn device_destroyed_is_not_a_fault() {
        assert_eq!(
            fault_from_device_lost(wgpu::DeviceLostReason::Destroyed, "shutting down"),
            None
        );
    }

    #[test]
    fn unknown_device_loss_is_a_fault() {
        assert_eq!(
            fault_from_device_lost(wgpu::DeviceLostReason::Unknown, "driver reset"),
            Some(DeviceFault::new(DeviceFaultKind::Lost, "driver reset"))
        );
    }
}
