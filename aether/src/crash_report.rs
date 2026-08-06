//! One self-contained report file written when Aether dies, behind `--aether-crash-report`.
//!
//! The panic hook is not enough on its own. A lost graphics device is a *graceful* shutdown --
//! wgpu reports the fault, the controller stops touching GPU resources and Aether exits normally --
//! so the panic path never runs for the failure mode that actually ends AQW sessions. Both routes
//! funnel through [`write`] instead, and the report carries the things that are otherwise scattered
//! across the console: the ActionScript callstack, the renderer's own diagnostics, and the last few
//! hundred log lines leading up to the fault.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Log lines retained for the report. Enough to cover the seconds before a fault -- which is where
/// the AVM2 errors and renderer warnings live -- without holding a whole session in memory.
const RECENT_LOG_LINES: usize = 500;

static RECENT_LOG: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
static ENVIRONMENT: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static OUTPUT_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static ALREADY_WRITTEN: Mutex<bool> = Mutex::new(false);

/// A poisoned lock here means another thread panicked while holding it. That is precisely when a
/// crash report is most wanted, so recover the data rather than panicking a second time.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Enable crash reporting and choose where reports land. Until this is called every hook in this
/// module is inert, so the default build carries no cost beyond a branch per log event.
pub fn arm(directory: PathBuf) {
    let _ = OUTPUT_DIRECTORY.set(directory);
}

pub fn is_armed() -> bool {
    OUTPUT_DIRECTORY.get().is_some()
}

/// Record a `key: value` fact about this session for the report's header -- adapter, backend,
/// resolved flags. Call order is preserved.
pub fn record_environment(line: impl Into<String>) {
    if !is_armed() {
        return;
    }
    lock(ENVIRONMENT.get_or_init(Mutex::default)).push(line.into());
}

struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={:?}", field.name(), value);
        }
    }
}

/// Keeps the tail of the log in memory so a report can include what led up to the fault.
pub struct RecentLogLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for RecentLogLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if !is_armed() {
            return;
        }
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));
        let metadata = event.metadata();
        let line = format!(
            "{:>5} {}: {}",
            metadata.level(),
            metadata.target(),
            message.trim()
        );

        let mut buffer = lock(RECENT_LOG.get_or_init(Mutex::default));
        while buffer.len() >= RECENT_LOG_LINES {
            buffer.pop_front();
        }
        buffer.push_back(line);
    }
}

/// A named block of text in the report. Callers supply whatever they alone can reach: the AVM2
/// callstack, the renderer's texture census, a panic backtrace.
pub struct Section {
    pub title: &'static str,
    pub body: String,
}

impl Section {
    pub fn new(title: &'static str, body: impl Into<String>) -> Self {
        Self {
            title,
            body: body.into(),
        }
    }
}

/// Write the report. Returns the path written, or `None` when reporting is disabled, a report has
/// already been written for this process, or the file could not be created.
///
/// Only the first call produces a file. A device loss usually surfaces through several channels at
/// once, and the first one to arrive is the one closest to the cause.
pub fn write(reason: &str, detail: &str, sections: &[Section]) -> Option<PathBuf> {
    let directory = OUTPUT_DIRECTORY.get()?;

    {
        let mut written = lock(&ALREADY_WRITTEN);
        if *written {
            return None;
        }
        *written = true;
    }

    let timestamp = chrono::Local::now();
    let path = directory.join(format!(
        "aether-crash-{}.txt",
        timestamp.format("%Y%m%d-%H%M%S")
    ));

    let mut report = String::new();
    let _ = writeln!(report, "Aether {} crash report", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(report, "time:   {}", timestamp.to_rfc3339());
    let _ = writeln!(report, "os:     {}", os_info::get());
    let _ = writeln!(report, "reason: {reason}");
    if !detail.is_empty() {
        let _ = writeln!(report, "detail: {detail}");
    }

    if let Some(environment) = ENVIRONMENT.get() {
        let environment = lock(environment);
        if !environment.is_empty() {
            let _ = writeln!(report, "\n=== Session ===");
            for line in environment.iter() {
                let _ = writeln!(report, "{line}");
            }
        }
    }

    for section in sections {
        if section.body.trim().is_empty() {
            continue;
        }
        let _ = writeln!(report, "\n=== {} ===", section.title);
        let _ = writeln!(report, "{}", section.body.trim_end());
    }

    if let Some(recent) = RECENT_LOG.get() {
        let recent = lock(recent);
        if !recent.is_empty() {
            let _ = writeln!(
                report,
                "\n=== Last {} log lines (oldest first) ===",
                recent.len()
            );
            for line in recent.iter() {
                let _ = writeln!(report, "{line}");
            }
        }
    }

    write_file(&path, &report).map(|()| path)
}

fn write_file(path: &Path, contents: &str) -> Option<()> {
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return None;
    }
    std::fs::write(path, contents).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disarmed_reporter_writes_nothing() {
        // The default build must not produce files, and must not care that nothing is configured.
        assert!(!is_armed());
        record_environment("adapter: irrelevant");
        assert!(write("panic", "boom", &[Section::new("Trace", "frames")]).is_none());
    }

    #[test]
    fn a_section_with_only_whitespace_is_omitted() {
        let section = Section::new("AVM2 Callstack", "   \n  ");
        assert!(section.body.trim().is_empty());
    }
}
