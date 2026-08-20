//! A rolling record of what the GPU was holding, kept so a crash report can show the run-up to a
//! device loss instead of the wreckage afterwards.
//!
//! The census that shipped in 0.4.8 sampled inside the device-loss handler, which turned out to be
//! about fifty milliseconds too late: wgpu has torn the device down by then, and the report came
//! back claiming zero textures and half a megabyte of GPU memory during a live AQW session. That
//! is not a measurement, it is a post mortem, and three fixes shipped against it.
//!
//! So this samples while the renderer is still healthy and keeps the last few minutes. When the
//! device dies, the interesting question is not what was live at the instant of death; it is
//! whether something was climbing, how fast, and what ran out first.
//!
//! Two properties matter more than completeness:
//!
//! **It is always on.** Not behind the `metrics` feature, not behind a flag. Public builds are
//! deliberately built without `metrics`, which is exactly why every AMD report so far arrived with
//! no resource data in it. A sample is fourteen atomic loads once a second; a fix that only works
//! in a build nobody runs is not a fix.
//!
//! **It records rates, not just levels.** A count of 4,000 textures means nothing on its own. Four
//! thousand textures that were two hundred a minute ago is a leak, and four thousand that have sat
//! there since load is a working set.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How often to take a sample.
///
/// The AMD failure reproduces in about forty seconds, so a second's resolution is plenty to see a
/// climb, and it keeps the cost to something not worth measuring.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// How many samples to keep.
///
/// Ten minutes at one a second. Long enough to cover a session that dies early and to show that a
/// slower one was climbing steadily, without letting the buffer grow without bound.
const MAX_SAMPLES: usize = 600;

/// Everything worth knowing about one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuSample {
    /// Milliseconds since the recorder started, so samples line up with the log.
    pub at_ms: u64,
    pub textures: u64,
    pub texture_views: u64,
    pub buffers: u64,
    pub bind_groups: u64,
    pub bind_group_layouts: u64,
    pub samplers: u64,
    pub render_pipelines: u64,
    pub compute_pipelines: u64,
    pub pipeline_layouts: u64,
    pub shader_modules: u64,
    pub command_encoders: u64,
    pub query_sets: u64,
    pub fences: u64,
    /// The count that a Vulkan driver caps, separately from how many bytes are in use. AMD reports
    /// out of device memory when this limit is reached even with the card nearly empty, so it is
    /// worth watching on its own rather than folding into a byte total.
    pub memory_allocations: u64,
    pub texture_memory_bytes: u64,
    pub buffer_memory_bytes: u64,
    /// How many blocks the allocator has reserved from the driver, and how much they hold in
    /// total. Textures are suballocated out of these rather than allocated individually.
    pub allocator_blocks: u64,
    pub allocator_reserved_bytes: u64,
    /// The largest contiguous free run in any block, which is the real ceiling on the next
    /// allocation that can be served without asking the driver for another block. Free bytes far
    /// above this is fragmentation, and is what a failure with memory to spare looks like.
    pub allocator_largest_free_run_bytes: u64,
    /// 1 when the backend supplied an allocator report, 0 when it does not implement one.
    ///
    /// Only DX12 implements it today, so a Vulkan session reports zeroes for every field above.
    /// Without this flag those zeroes read as a perfectly unfragmented allocator, which is the
    /// opposite of what they mean.
    pub allocator_report_available: u64,
}

impl GpuSample {
    /// Read every counter wgpu exposes for this device.
    pub fn capture(device: &wgpu::Device, at_ms: u64) -> Self {
        let counters = device.get_internal_counters();
        let hal = &counters.hal;
        // Only DX12 implements this today. Vulkan returns nothing, which is why the flag below
        // exists: the zeroes that follow would otherwise read as an allocator with no holes in it.
        let report = device.generate_allocator_report();
        let fragmentation = report
            .as_ref()
            .map(crate::aether_allocator_report::summarise)
            .unwrap_or_default();

        Self {
            at_ms,
            textures: hal.textures.read() as u64,
            texture_views: hal.texture_views.read() as u64,
            buffers: hal.buffers.read() as u64,
            bind_groups: hal.bind_groups.read() as u64,
            bind_group_layouts: hal.bind_group_layouts.read() as u64,
            samplers: hal.samplers.read() as u64,
            render_pipelines: hal.render_pipelines.read() as u64,
            compute_pipelines: hal.compute_pipelines.read() as u64,
            pipeline_layouts: hal.pipeline_layouts.read() as u64,
            shader_modules: hal.shader_modules.read() as u64,
            command_encoders: hal.command_encoders.read() as u64,
            query_sets: hal.query_sets.read() as u64,
            fences: hal.fences.read() as u64,
            memory_allocations: hal.memory_allocations.read() as u64,
            texture_memory_bytes: hal.texture_memory.read() as u64,
            buffer_memory_bytes: hal.buffer_memory.read() as u64,
            allocator_blocks: fragmentation.blocks as u64,
            allocator_reserved_bytes: fragmentation.reserved_bytes,
            allocator_largest_free_run_bytes: fragmentation.largest_free_run_bytes,
            allocator_report_available: u64::from(report.is_some()),
        }
    }

    /// The fields worth charting over time, as (name, value) pairs.
    ///
    /// Returned as a list so the summary, the peak table and the JSON line all walk the same set
    /// and cannot drift apart as fields are added.
    pub fn tracked(&self) -> [(&'static str, u64); 20] {
        [
            ("textures", self.textures),
            ("texture_views", self.texture_views),
            ("buffers", self.buffers),
            ("bind_groups", self.bind_groups),
            ("bind_group_layouts", self.bind_group_layouts),
            ("samplers", self.samplers),
            ("render_pipelines", self.render_pipelines),
            ("compute_pipelines", self.compute_pipelines),
            ("pipeline_layouts", self.pipeline_layouts),
            ("shader_modules", self.shader_modules),
            ("command_encoders", self.command_encoders),
            ("query_sets", self.query_sets),
            ("fences", self.fences),
            ("memory_allocations", self.memory_allocations),
            ("texture_memory_bytes", self.texture_memory_bytes),
            ("buffer_memory_bytes", self.buffer_memory_bytes),
            ("allocator_blocks", self.allocator_blocks),
            ("allocator_reserved_bytes", self.allocator_reserved_bytes),
            (
                "allocator_largest_free_run_bytes",
                self.allocator_largest_free_run_bytes,
            ),
            (
                "allocator_report_available",
                self.allocator_report_available,
            ),
        ]
    }

    /// One JSON object per sample, for the full-history dump.
    ///
    /// Hand-written rather than derived: this crate does not depend on serde, and adding it for a
    /// flat record of integers would be a heavier change than the record deserves.
    pub fn to_json_line(&self) -> String {
        let mut line = String::with_capacity(512);
        line.push('{');
        for (index, (name, value)) in self.tracked().iter().enumerate() {
            if index > 0 {
                line.push(',');
            }
            line.push_str(&format!("\"{name}\":{value}"));
        }
        line.push_str(&format!(",\"at_ms\":{}", self.at_ms));
        line.push('}');
        line
    }
}

/// The rolling record.
pub struct GpuTimeline {
    started: Instant,
    last_sample: Option<Instant>,
    samples: VecDeque<GpuSample>,
    /// Highest value seen for each tracked field, and when. Kept separately because the ring
    /// buffer forgets, and a peak that happened twenty minutes into a long session is exactly the
    /// thing worth knowing about.
    peaks: Vec<(&'static str, u64, u64)>,
    /// Samples dropped off the front of the ring, so a report can say the history is partial
    /// rather than implying the session began where the buffer does.
    dropped: u64,
}

impl Default for GpuTimeline {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuTimeline {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            last_sample: None,
            samples: VecDeque::with_capacity(MAX_SAMPLES),
            peaks: Vec::new(),
            dropped: 0,
        }
    }

    /// Take a sample if enough time has passed. Safe to call every frame.
    ///
    /// Returns the sample when one was taken, so a caller writing the full history to disk can
    /// write exactly the samples that were recorded.
    pub fn maybe_sample(&mut self, device: &wgpu::Device) -> Option<GpuSample> {
        let now = Instant::now();
        if let Some(last) = self.last_sample
            && now.duration_since(last) < SAMPLE_INTERVAL
        {
            return None;
        }
        self.last_sample = Some(now);

        let sample = GpuSample::capture(
            device,
            now.duration_since(self.started)
                .as_millis()
                .min(u64::MAX as u128) as u64,
        );
        self.push(sample);
        Some(sample)
    }

    fn push(&mut self, sample: GpuSample) {
        if self.peaks.is_empty() {
            self.peaks = sample
                .tracked()
                .iter()
                .map(|(name, value)| (*name, *value, sample.at_ms))
                .collect();
        } else {
            for (index, (_, value)) in sample.tracked().iter().enumerate() {
                if *value > self.peaks[index].1 {
                    self.peaks[index] = (self.peaks[index].0, *value, sample.at_ms);
                }
            }
        }

        if self.samples.len() == MAX_SAMPLES {
            self.samples.pop_front();
            self.dropped += 1;
        }
        self.samples.push_back(sample);
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The report that goes into a crash file.
    pub fn report(&self) -> String {
        if self.samples.is_empty() {
            return "No samples were taken before the device was lost.".to_owned();
        }

        let mut report = String::with_capacity(4096);

        let first = self.samples.front().expect("checked non-empty");
        let last = self.samples.back().expect("checked non-empty");

        report.push_str(&format!(
            "{} samples covering {:.1}s to {:.1}s after the renderer started",
            self.samples.len(),
            first.at_ms as f64 / 1000.0,
            last.at_ms as f64 / 1000.0,
        ));
        if self.dropped > 0 {
            report.push_str(&format!(
                " ({} earlier samples dropped from the ring)",
                self.dropped
            ));
        }
        report.push_str("\n\n");

        report.push_str(&self.peak_table(last));
        report.push('\n');
        report.push_str(&self.trajectory_table());

        report
    }

    /// Peak against final, with the time each peak happened.
    ///
    /// A peak well above the final value means something was released, which points at churn. A
    /// peak equal to the final value at the moment of death means it was still climbing.
    fn peak_table(&self, last: &GpuSample) -> String {
        let mut table = String::from("peak vs final:\n");
        table.push_str(&format!(
            "  {:<22} {:>12} {:>12} {:>10}\n",
            "", "peak", "final", "peak at"
        ));

        for (index, (name, final_value)) in last.tracked().iter().enumerate() {
            let (_, peak, peak_at_ms) = self.peaks[index];
            table.push_str(&format!(
                "  {name:<22} {peak:>12} {final_value:>12} {:>9.1}s\n",
                peak_at_ms as f64 / 1000.0,
            ));
        }
        table
    }

    /// The samples themselves, thinned to something readable.
    ///
    /// A crash report nobody reads is worth nothing, so this caps at roughly thirty rows and says
    /// how it thinned them. The full history goes to the JSON dump when that is enabled.
    fn trajectory_table(&self) -> String {
        const MAX_ROWS: usize = 30;

        let step = self.samples.len().div_ceil(MAX_ROWS).max(1);
        let mut table = if step > 1 {
            format!("trajectory (every {step} samples):\n")
        } else {
            String::from("trajectory:\n")
        };

        table.push_str(&format!(
            "  {:>8} {:>9} {:>9} {:>9} {:>7} {:>11} {:>11}\n",
            "at", "textures", "views", "buffers", "allocs", "tex MB", "buf MB"
        ));

        // Always include the last sample: it is the closest look at the failure, and a stride that
        // does not divide evenly would otherwise drop it.
        let last_index = self.samples.len() - 1;
        for (index, sample) in self.samples.iter().enumerate() {
            if index % step != 0 && index != last_index {
                continue;
            }
            table.push_str(&format!(
                "  {:>7.1}s {:>9} {:>9} {:>9} {:>7} {:>11.1} {:>11.1}\n",
                sample.at_ms as f64 / 1000.0,
                sample.textures,
                sample.texture_views,
                sample.buffers,
                sample.memory_allocations,
                sample.texture_memory_bytes as f64 / (1024.0 * 1024.0),
                sample.buffer_memory_bytes as f64 / (1024.0 * 1024.0),
            ));
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::{GpuSample, GpuTimeline, MAX_SAMPLES};

    fn sample(at_ms: u64, textures: u64) -> GpuSample {
        GpuSample {
            at_ms,
            textures,
            ..Default::default()
        }
    }

    /// Only the DX12 backend implements wgpu's allocator report; Vulkan returns nothing at all.
    /// A zero largest-free-run therefore has two very different meanings, and the reader has to be
    /// able to tell "measured, not fragmented" from "this backend never told us". The last report
    /// that could not distinguish those cost a round trip to a tester and a wrong conclusion.
    #[test]
    fn an_unavailable_allocator_report_is_flagged_rather_than_read_as_zero_fragmentation() {
        let tracked: Vec<&'static str> = sample(0, 0)
            .tracked()
            .iter()
            .map(|(name, _)| *name)
            .collect();

        assert!(tracked.contains(&"allocator_report_available"));
        // Default is "we have not been told", not "there is nothing there".
        assert_eq!(sample(0, 0).allocator_report_available, 0);
    }

    /// A device lost to "out of memory" holding half a gigabyte on a sixteen gigabyte card is not
    /// short of memory, so the timeline has to carry the shape of the allocator's free space and not
    /// just its totals. Without the largest free run there is no way to tell a full allocator from a
    /// fragmented one after the fact.
    #[test]
    fn the_timeline_tracks_the_shape_of_free_space_and_not_only_its_size() {
        let tracked: Vec<&'static str> = sample(0, 0)
            .tracked()
            .iter()
            .map(|(name, _)| *name)
            .collect();

        for name in [
            "allocator_blocks",
            "allocator_reserved_bytes",
            "allocator_largest_free_run_bytes",
        ] {
            assert!(tracked.contains(&name), "{name} is not being recorded");
        }
    }

    #[test]
    fn an_empty_timeline_says_so_rather_than_reporting_nothing() {
        let timeline = GpuTimeline::new();

        assert!(timeline.is_empty());
        assert!(timeline.report().contains("No samples"));
    }

    /// The peak is the point of the whole exercise: a count that climbed and then fell would be
    /// invisible in a final reading, which is the mistake this replaces.
    #[test]
    fn a_peak_survives_the_value_falling_back() {
        let mut timeline = GpuTimeline::new();

        timeline.push(sample(1000, 100));
        timeline.push(sample(2000, 9000));
        timeline.push(sample(3000, 50));

        let report = timeline.report();
        assert!(report.contains("9000"), "peak missing from:\n{report}");
        assert!(report.contains("2.0s"), "peak time missing from:\n{report}");
    }

    /// The ring has to forget the oldest samples without forgetting that it did.
    #[test]
    fn the_ring_reports_what_it_dropped() {
        let mut timeline = GpuTimeline::new();

        for index in 0..(MAX_SAMPLES as u64 + 10) {
            timeline.push(sample(index * 1000, index));
        }

        let report = timeline.report();
        assert!(report.contains("10 earlier samples dropped"), "{report}");
        // The peak predates the window, and must have survived it falling out of the ring.
        assert!(report.contains(&(MAX_SAMPLES as u64 + 9).to_string()));
    }

    /// The last sample is the closest look at the failure and must never be thinned away.
    #[test]
    fn the_final_sample_is_always_shown() {
        let mut timeline = GpuTimeline::new();

        for index in 0..101u64 {
            timeline.push(sample(index * 1000, index));
        }

        assert!(
            timeline.report().contains("100.0s"),
            "final sample missing:\n{}",
            timeline.report()
        );
    }

    #[test]
    fn a_sample_serialises_every_field_it_tracks() {
        let line = sample(1500, 42).to_json_line();

        assert!(line.starts_with('{') && line.ends_with('}'));
        assert!(line.contains("\"textures\":42"));
        assert!(line.contains("\"at_ms\":1500"));
        for (name, _) in sample(0, 0).tracked() {
            assert!(line.contains(&format!("\"{name}\":")), "{name} missing");
        }
    }
}
