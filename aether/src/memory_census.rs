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
//! It found one: five hours idle in Yulgar took `movies` from 81 to 5,479 without a single release,
//! and `rss` from 1.2 GB to 31.7 GB with it -- 5.7 MB per SWF, perfectly linear.
//!
//! Releasing them turned out not to be available. AQW loads its assets into a shared application
//! domain, where the definitions outlive the `Loader` by design and by Flash's own semantics, and
//! dropping them on unload stripped every avatar in the room to a black silhouette. So `distinct
//! urls` was added to ask the question that is still open: of the SWFs being retained, how many are
//! second and third copies of a URL already resident, which export nothing and are looked up never.
//!
//! **That question is now answered: 4,693 movies for 1,935 distinct urls, so 2.4 copies of every
//! file.** Since they cannot be released afterwards, they are no longer created -- a load of a URL
//! already resident shares the movie that is there (`Library::resident_movie`). `reused` counts how
//! often that path is taken, and `movies` should now track `distinct urls` rather than multiplying
//! it. What remains after that is the genuine first copy of each file, which is the cost of
//! shared-domain loading and is not a leak.
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

/// Bytes the process has committed, if the platform can say.
#[cfg(feature = "metrics")]
fn private_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        crate::windows::private_bytes()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

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
            "memory census: {} | gc {} ({:+}) MB over {} objects | movies {} ({:+}) | \
             characters {} ({:+}) | orphans {} ({:+}) | unloads {} | reused {} (skipped {} sprites) | weak dicts {} ({} object-keyed, pruned {} keys) | collections {} freeing {} MB worst {} ms | caches swept {} | distinct urls {} | \
             swf bytes {} MB ({} MB duplicated)",
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
            sample.core.unloads,
            sample.core.movie_reuses,
            sample.core.preload_sprites_already_defined,
            sample.core.weak_dictionaries,
            sample.core.weak_dictionaries_object_keyed,
            sample.core.dictionary_keys_pruned,
            sample.core.collections_run,
            sample.core.collection_reclaimed_bytes / (1024 * 1024),
            sample.core.collection_worst_micros / 1000,
            sample.core.bitmap_caches_swept,
            sample.core.distinct_urls,
            sample.core.movie_bytes / (1024 * 1024),
            sample.core.duplicate_movie_bytes / (1024 * 1024),
        );

        // What those characters actually are. `characters` climbing says retention is happening;
        // this says which kind of thing is being retained, and for bitmaps and binary data how many
        // bytes of Rust heap that comes to. Sounds are named separately because they are the one
        // kind no release path touches.
        let characters = sample.core.character_census;
        tracing::info!(
            "character census: bitmaps {} ({} MB source, {} uploaded) | graphics {} | \
             morph shapes {} | fonts {} | sounds {} | movie clips {} | \
             binary {} | other {}",
            characters.bitmaps,
            characters.bitmap_bytes / (1024 * 1024),
            characters.bitmaps_uploaded,
            characters.graphics,
            characters.morph_shapes,
            characters.fonts,
            characters.sounds,
            characters.movie_clips,
            characters.binary_data,
            characters.other,
        );

        // The line the other two cannot produce between them: what the client's own heap holds,
        // against what the process has committed. The difference is everything Rust did not
        // allocate -- driver, GPU staging, and pages the allocator has kept rather than returned.
        #[cfg(feature = "metrics")]
        {
            let mb = |bytes: u64| bytes / (1024 * 1024);
            let heap = crate::heap_census::live_bytes() as u64;
            let committed = private_bytes();
            let outside = committed.map(|committed| committed.saturating_sub(heap));
            tracing::info!(
                "heap census: rust heap {} MB live over {} allocations | committed {} | \
                 outside the rust heap {}",
                mb(heap),
                crate::heap_census::total_allocations(),
                committed.map_or("unavailable".to_string(), |b| format!("{} MB", mb(b))),
                outside.map_or("unavailable".to_string(), |b| format!("{} MB", mb(b))),
            );
        }

        // What the two summary lines above cannot say: which classes and which texture sizes. Both
        // tables are already maintained; only the totals were ever being printed.
        #[cfg(feature = "metrics")]
        {
            for line in ruffle_core::aether_object_census::object_census_report(20) {
                tracing::info!("{line}");
            }
            for line in ruffle_core::aether_object_census::event_type_census_report(20) {
                tracing::info!("{line}");
            }
            for line in ruffle_render_wgpu::aether_metrics::texture_census_report(20) {
                tracing::info!("{line}");
            }
        }

        if let Some(render) = sample.core.render {
            let first = first.core.render.unwrap_or_default();
            let mb = |bytes: u64| bytes / (1024 * 1024);
            tracing::info!(
                "gpu census: textures {} ({:+}) holding {} ({:+}) MB | buffers {} ({:+}) holding \
                 {} ({:+}) MB | driver allocations {} ({:+})",
                render.textures,
                delta64(render.textures, first.textures),
                mb(render.texture_bytes),
                delta64(mb(render.texture_bytes), mb(first.texture_bytes)),
                render.buffers,
                delta64(render.buffers, first.buffers),
                mb(render.buffer_bytes),
                delta64(mb(render.buffer_bytes), mb(first.buffer_bytes)),
                render.memory_allocations,
                delta64(render.memory_allocations, first.memory_allocations),
            );
        }
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

/// As [`delta`], for the renderer's counters.
fn delta64(current: u64, first: u64) -> i64 {
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
            megabytes(current) - megabytes(first),
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
