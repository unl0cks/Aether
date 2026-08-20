//! Splits a frame's command stream into several submissions instead of one enormous one.
//!
//! A device lost to "Out of memory" on a 16 GB card holding 0.42 GB, refusing a 128x128 texture, is
//! not a card that ran out of memory. Ruffle already knew why: `ActiveFrame::maybe_flush` carries
//! the note that "if we do too much during one frame, the submission ends up being way too large
//! and goes OutOfMemory", and splits the bitmap-cache loop to avoid it.
//!
//! That split only covers the cache loop. The scene itself is never broken up, and a crowded AQW map
//! encodes 690 to 1016 render passes into a single submission -- every blend sub-target, every
//! composite, every mask. This counts passes as they are encoded and submits once the count gets
//! high, so the driver is handed several moderate command buffers rather than one it may refuse.
//!
//! The counter is a thread-local because passes are encoded from inside a recursion -- a blend's
//! sub-target renders through `Surface::draw_commands_at`, which builds and executes its own chunks
//! -- and threading a counter through every signature on that path would touch far more code than
//! the fix is worth. Rendering happens on one thread, so the cell is only ever touched by it.

use std::cell::Cell;

/// How many render passes may be encoded before the frame's commands are handed over.
///
/// Swept on the reporter's 6800 XT, all six runs in one session on the same content, cost per pass
/// measured over the seconds busy enough to matter:
///
/// ```text
///     threshold   busy s   passes/frame   render ms   us/pass
///          2048        2            584        93.7     160.3
///          1024       14            722        67.1      93.0
///           512       66            841        57.0      67.8
///           128       56            802        45.9      57.2
///            16       58            759        55.5      73.1
///             2       38            742        87.8     118.2
/// ```
///
/// Splitting is not the cost it looks like. One submission leaves the GPU idle while the CPU
/// encodes two thousand passes and then hands over everything at once; splitting lets the GPU start
/// while the CPU is still encoding, and 1024 to 512 to 128 improves monotonically because of it.
/// Below that the per-submit overhead takes over and 2 is worse than not splitting sensibly at all.
///
/// The large end also fails outright: 2048 and 1024 lost the device in 9 and 22 seconds, where
/// everything from 512 down ran for 69 to 86. So this is bounded on both sides, and 128 sits at the
/// bottom of the curve.
pub const DEFAULT_MAX_PASSES_PER_SUBMISSION: u32 = 128;

thread_local! {
    static PASSES_SINCE_SUBMIT: Cell<u32> = const { Cell::new(0) };
}

/// Whether `passes` encoded so far warrants handing the command buffer over.
///
/// Split out from the thread-local so the policy can be tested without a GPU.
pub fn should_split(passes_since_submit: u32, max_passes: u32) -> bool {
    // A zero maximum would submit after every pass and make a busy frame pathological, so it is
    // treated as "never split" rather than "always".
    max_passes > 0 && passes_since_submit >= max_passes
}

/// Note that a render pass has been encoded, returning whether the caller should submit now.
pub fn note_pass(max_passes: u32) -> bool {
    PASSES_SINCE_SUBMIT.with(|counter| {
        let passes = counter.get().saturating_add(1);
        if should_split(passes, max_passes) {
            counter.set(0);
            true
        } else {
            counter.set(passes);
            false
        }
    })
}

/// Forget any passes counted so far.
///
/// Called when the frame is submitted for its own reasons, so the next frame starts from zero
/// rather than inheriting a part-full count and splitting earlier than it needs to.
pub fn reset() {
    PASSES_SINCE_SUBMIT.with(|counter| counter.set(0));
}

/// The ceiling in force, honouring an override.
///
/// Read from the environment so the theory can be tested both ways from one binary: setting it to 0
/// restores the single-submission behaviour exactly, which is the control this needs.
pub fn max_passes_per_submission() -> u32 {
    static MAX: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| {
        let Ok(raw) = std::env::var("AETHER_MAX_PASSES_PER_SUBMISSION") else {
            return DEFAULT_MAX_PASSES_PER_SUBMISSION;
        };
        match raw.trim().parse::<u32>() {
            Ok(value) => value,
            Err(_) => {
                tracing::warn!("Ignoring unreadable AETHER_MAX_PASSES_PER_SUBMISSION {raw:?}");
                DEFAULT_MAX_PASSES_PER_SUBMISSION
            }
        }
    })
}

/// Hand everything encoded so far to the driver and carry on into a fresh encoder.
///
/// The staging belt is finished before the submit and recalled after, in that order, because the
/// commands being submitted read from it. This mirrors `ActiveFrame::submit_direct`, which has been
/// doing the same thing between bitmap-cache entries for as long as that flush has existed.
pub fn split_submission(
    descriptors: &crate::descriptors::Descriptors,
    encoder: &mut wgpu::CommandEncoder,
    staging_belt: &mut wgpu::util::StagingBelt,
) {
    // Nothing may be submitted once the device has faulted. `CommandEncoder::finish` ends the
    // underlying Vulkan command buffer, and wgpu-hal nulls its handle *before* making that
    // fallible call -- so when it fails the encoder is left in exactly the state the discard
    // that follows asserts against, and the process dies on an `assert_ne!` inside the driver
    // wrapper rather than through the fault path this crate exists to reach.
    //
    // Reported upstream as an integrated-GPU crash class and closed as not planned, so the
    // guard is ours to hold: see `note_pass_and_maybe_split` for where encoding stops.
    if descriptors.device_status.is_faulted() {
        return;
    }

    staging_belt.finish();
    let finished = std::mem::replace(
        encoder,
        descriptors
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: create_debug_label!("Split frame submission").as_deref(),
            }),
    );
    descriptors.queue.submit(Some(finished.finish()));
    staging_belt.recall();
    #[cfg(feature = "aether_metrics")]
    crate::aether_metrics::record_submission_split();
}

/// Note a pass and split if the count warrants it, returning whether encoding should continue.
///
/// A fault recorded part-way through a frame means every resource in flight may already be
/// invalid. Aether checks for one when a frame begins and again after it submits, but an
/// out-of-memory device loss happens *between* those, while passes are still being encoded --
/// which is precisely the window an integrated GPU dies in. This is the per-pass hook, so it is
/// where that window is closed: the frame stops adding work and unwinds to the fault report that
/// is already built for this, instead of encoding several hundred more passes into a dead device.
pub fn note_pass_and_maybe_split(
    descriptors: &crate::descriptors::Descriptors,
    encoder: &mut wgpu::CommandEncoder,
    staging_belt: &mut wgpu::util::StagingBelt,
) -> bool {
    if descriptors.device_status.is_faulted() {
        return false;
    }

    if note_pass(max_passes_per_submission()) {
        split_submission(descriptors, encoder, staging_belt);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_frame_is_never_split() {
        reset();
        // Around what an idle AQW screen encodes.
        for _ in 0..38 {
            assert!(!note_pass(DEFAULT_MAX_PASSES_PER_SUBMISSION));
        }
    }

    /// The case this exists for: the crowded map that loses the device.
    #[test]
    fn a_crowded_frame_becomes_several_submissions() {
        reset();
        let splits = (0..1900)
            .filter(|_| note_pass(DEFAULT_MAX_PASSES_PER_SUBMISSION))
            .count();

        assert_eq!(splits, 1900 / DEFAULT_MAX_PASSES_PER_SUBMISSION as usize);
    }

    /// Counting has to restart after a split, or the second submission would be one pass long and
    /// every pass after the first threshold would submit again.
    #[test]
    fn the_count_restarts_after_a_split() {
        reset();
        for _ in 0..DEFAULT_MAX_PASSES_PER_SUBMISSION {
            note_pass(DEFAULT_MAX_PASSES_PER_SUBMISSION);
        }
        // The next pass begins a fresh submission rather than immediately ending one.
        assert!(!note_pass(DEFAULT_MAX_PASSES_PER_SUBMISSION));
    }

    #[test]
    fn a_reset_forgets_the_partial_count() {
        reset();
        for _ in 0..(DEFAULT_MAX_PASSES_PER_SUBMISSION - 1) {
            note_pass(DEFAULT_MAX_PASSES_PER_SUBMISSION);
        }
        reset();
        assert!(!note_pass(DEFAULT_MAX_PASSES_PER_SUBMISSION));
    }

    /// A zero maximum means "do not split". Splitting after every pass would turn the busiest frame
    /// into a thousand submissions, which is worse than the problem.
    #[test]
    fn a_zero_maximum_never_splits() {
        reset();
        for _ in 0..500 {
            assert!(!note_pass(0));
        }
        assert!(!should_split(u32::MAX, 0));
    }
}
