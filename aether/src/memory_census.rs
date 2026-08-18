//! A periodic note of how much memory the process is holding and what is holding it.
//!
//! Written for a report that could not be chased any other way: a player idle in Yulgar for three
//! hours came back to 24 GB resident and 4 fps. Yulgar is the worst case for a reason -- it loads a
//! fresh set of armour, hair, cape and weapon SWFs for every player who walks through -- but a leak
//! that takes hours to show cannot be reproduced from a desk, and reading the code for it has
//! already retired two candidates that turned out to be sound.
//!
//! So this measures rather than guesses. Each line is one sample, and the shape of the growth over
//! a long session is what names the culprit:
//!
//! * `movies` and `characters` climbing -- loaded assets are never released.
//! * `orphans` climbing while movies hold steady -- detached avatar pieces are being kept alive.
//! * `gc` climbing -- ActionScript objects are being kept alive, not collected.
//! * all of them flat while `rss` still climbs -- neither the library nor the collector is holding
//!   it, which leaves the renderer.
//!
//! Always compiled, not gated behind the metrics build. The people who can reproduce this are
//! running ordinary releases, and a diagnostic they have to be handed a special binary for is a
//! diagnostic that does not get run.

use std::time::{Duration, Instant};

use ruffle_core::CoreCensus;

/// How often a sample is taken.
///
/// Long enough that the cost is irrelevant, short enough that an hour of play is still sixty
/// points of curve.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(60);

/// Bytes the process is holding, if the platform can say.
fn resident_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        crate::windows::working_set_bytes()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Tracks when the next sample is due.
pub struct MemoryCensus {
    last_sample: Instant,
    first: Option<Sample>,
}

#[derive(Clone, Copy)]
struct Sample {
    resident: Option<u64>,
    core: CoreCensus,
}

impl MemoryCensus {
    pub fn new(now: Instant) -> Self {
        Self {
            // Nothing is loaded at startup, so the first sample is taken an interval in rather than
            // immediately -- a baseline of zero movies would make every later line look like a leak.
            last_sample: now,
            first: None,
        }
    }

    /// Whether a sample is due, claiming the slot if so.
    pub fn sample_due(&mut self, now: Instant) -> bool {
        if now.duration_since(self.last_sample) < SAMPLE_INTERVAL {
            return false;
        }
        self.last_sample = now;
        true
    }

    /// Record and log one sample.
    pub fn record(&mut self, core: CoreCensus) {
        let sample = Sample {
            resident: resident_bytes(),
            core,
        };
        let first = *self.first.get_or_insert(sample);

        tracing::info!(
            "memory census: {} | gc {} ({:+}) MB over {} objects | movies {} ({:+}) |              characters {} ({:+}) | orphans {} ({:+})",
            describe_resident(sample.resident, first.resident),
            sample.core.gc_bytes / (1024 * 1024),
            delta(sample.core.gc_bytes, first.core.gc_bytes) / (1024 * 1024),
            sample.core.gc_objects,
            sample.core.movies,
            delta(sample.core.movies, first.core.movies),
            sample.core.characters,
            delta(sample.core.characters, first.core.characters),
            sample.core.orphans,
            delta(sample.core.orphans, first.core.orphans),
        );
    }
}

/// Change since the first sample, as a signed count.
///
/// Both sides are counts that can move either way, and `usize` subtraction on the wrong order
/// panics in debug and wraps to something enormous in release, which is exactly the sort of number
/// that gets mistaken for the leak it is meant to be measuring.
fn delta(current: usize, first: usize) -> i64 {
    current as i64 - first as i64
}

/// The resident figure and how far it has moved, or a note that the platform will not say.
fn describe_resident(current: Option<u64>, first: Option<u64>) -> String {
    let Some(current) = current else {
        return "rss unavailable".to_string();
    };
    let megabytes = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
    match first {
        Some(first) => format!(
            "rss {:.0} MB ({:+.0} MB)",
            megabytes(current),
            megabytes(current) as f64 - megabytes(first),
        ),
        None => format!("rss {:.0} MB", megabytes(current)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_sample_waits_for_a_full_interval() {
        // Sampling at startup would record an empty library as the baseline, so every later line
        // would report the game's normal working set as growth.
        let start = Instant::now();
        let mut census = MemoryCensus::new(start);
        assert!(!census.sample_due(start));
        assert!(!census.sample_due(start + SAMPLE_INTERVAL - Duration::from_millis(1)));
        assert!(census.sample_due(start + SAMPLE_INTERVAL));
    }

    #[test]
    fn taking_a_sample_starts_the_next_interval() {
        let start = Instant::now();
        let mut census = MemoryCensus::new(start);
        assert!(census.sample_due(start + SAMPLE_INTERVAL));
        assert!(!census.sample_due(start + SAMPLE_INTERVAL));
        assert!(census.sample_due(start + SAMPLE_INTERVAL * 2));
    }

    #[test]
    fn a_count_that_falls_reports_a_negative_delta() {
        // Assets being released is the outcome being hoped for, and it has to be reportable.
        assert_eq!(delta(3, 10), -7);
        assert_eq!(delta(10, 3), 7);
        assert_eq!(delta(0, 0), 0);
    }

    #[test]
    fn resident_is_described_against_the_first_sample() {
        let mb = 1024 * 1024;
        assert_eq!(
            describe_resident(Some(600 * mb), Some(400 * mb)),
            "rss 600 MB (+200 MB)"
        );
        assert_eq!(describe_resident(Some(400 * mb), None), "rss 400 MB");
        assert_eq!(describe_resident(None, None), "rss unavailable");
    }
}
