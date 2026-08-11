//! Measures how long a click takes to reach the screen.
//!
//! A click travels through four stages, and each one can hold it up for a different reason:
//!
//! 1. **Handled.** `Player::handle_event` runs the game's own click handler synchronously, so this
//!    is the game's code, not the host's.
//! 2. **Redraw requested.** Only happens if something decided the screen changed. If the game's
//!    handler alters the scene without the player noticing, nothing is requested here and the
//!    click waits for the next tick instead.
//! 3. **Render started.** The gap from the request is the event loop waiting: either for the next
//!    frame deadline or for the windowing system to deliver the redraw.
//! 4. **Presented.** Submitting the frame. With vsync this blocks once the driver's queue of
//!    presentable images is full, so a deep queue shows up here as a long, steady wait.
//!
//! Reporting them separately is the point. "Clicking feels slow" has at least four distinct
//! causes, they need different fixes, and three of the four would be invisible in a single
//! end-to-end number.
//!
//! Command line only, and off by default. It is a development tool, and a line per click in the
//! log is not something a normal session should be paying for.

use std::time::{Duration, Instant};

/// One click, in flight.
struct PendingClick {
    pressed_at: Instant,
    handled: Option<Duration>,
    redraw_requested: Option<Duration>,
    render_started: Option<Duration>,
    scene_drawn: Option<Duration>,
}

#[derive(Default)]
pub struct ClickLatencyProbe {
    enabled: bool,
    pending: Option<PendingClick>,
    samples: Vec<Duration>,
    /// Optional file, written independently of the log.
    ///
    /// The first version of this only logged, on a target the default filter did not admit, so a
    /// whole session of measurements was taken and discarded. The filter is fixed and tested, but
    /// a diagnostic that a config change can silence is a diagnostic that will be silenced again.
    file: Option<std::io::BufWriter<std::fs::File>>,
}

impl ClickLatencyProbe {
    pub fn new(enabled: bool, path: Option<&std::path::Path>) -> Self {
        let file = path.and_then(|path| match std::fs::File::create(path) {
            Ok(file) => Some(std::io::BufWriter::new(file)),
            Err(error) => {
                tracing::warn!("Could not open {}: {error}", path.display());
                None
            }
        });

        Self {
            // A path on its own is enough to mean yes; asking for the file and not getting the
            // measurements would be its own small trap.
            enabled: enabled || file.is_some(),
            file,
            ..Default::default()
        }
    }

    /// A mouse button went down. Starts the clock.
    ///
    /// A second press before the first has been drawn replaces it rather than queueing, because
    /// the interesting number is how long the most recent click took, and keeping a backlog would
    /// report the wait for an earlier frame as though it belonged to this one.
    pub fn pressed(&mut self) {
        if self.enabled {
            self.pending = Some(PendingClick {
                pressed_at: Instant::now(),
                handled: None,
                redraw_requested: None,
                render_started: None,
                scene_drawn: None,
            });
        }
    }

    pub fn handled(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.handled = Some(pending.pressed_at.elapsed());
        }
    }

    /// Records the first redraw request only. Later ticks request more, and it is the first one
    /// that decides when the click can possibly be drawn.
    pub fn redraw_requested(&mut self) {
        if let Some(pending) = &mut self.pending
            && pending.redraw_requested.is_none()
        {
            pending.redraw_requested = Some(pending.pressed_at.elapsed());
        }
    }

    pub fn render_started(&mut self) {
        if let Some(pending) = &mut self.pending
            && pending.render_started.is_none()
        {
            pending.render_started = Some(pending.pressed_at.elapsed());
        }
    }

    /// `player.render()` has finished building the frame, before it is submitted.
    ///
    /// The split matters: everything after this is the driver and the display, and everything
    /// before it is work we chose to do. They need opposite fixes.
    pub fn scene_drawn(&mut self) {
        if let Some(pending) = &mut self.pending
            && pending.scene_drawn.is_none()
        {
            pending.scene_drawn = Some(pending.pressed_at.elapsed());
        }
    }

    /// The frame carrying this click has been submitted. Reports and clears.
    pub fn presented(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };

        let total = pending.pressed_at.elapsed();
        self.samples.push(total);

        let millis = |stage: Option<Duration>| {
            stage.map_or(f64::NAN, |elapsed| elapsed.as_secs_f64() * 1000.0)
        };

        let handled = millis(pending.handled);
        let redraw = millis(pending.redraw_requested);
        let render = millis(pending.render_started);
        let drawn = millis(pending.scene_drawn);
        let presented = total.as_secs_f64() * 1000.0;

        tracing::info!(
            target: "aether::input_latency",
            handled_ms = handled,
            redraw_requested_ms = redraw,
            render_started_ms = render,
            scene_drawn_ms = drawn,
            presented_ms = presented,
            "click to screen"
        );

        if let Some(file) = &mut self.file {
            use std::io::Write as _;
            let line = format!(
                "{{\"handled_ms\":{handled:.3},\"redraw_requested_ms\":{redraw:.3},\"render_started_ms\":{render:.3},\"scene_drawn_ms\":{drawn:.3},\"presented_ms\":{presented:.3}}}"
            );
            // Flushed per click: this exists to survive the session, not to be tidy.
            if writeln!(file, "{line}")
                .and_then(|()| file.flush())
                .is_err()
            {
                self.file = None;
            }
        }
    }

    /// A summary for the end of the session.
    ///
    /// The median matters more than the mean here: a single frame lost to a texture upload or a
    /// map change would drag an average around and say nothing about how clicking normally feels.
    pub fn summary(&self) -> Option<String> {
        if !self.enabled || self.samples.is_empty() {
            return None;
        }

        let mut sorted: Vec<Duration> = self.samples.clone();
        sorted.sort_unstable();

        let at = |fraction: f64| {
            let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
            sorted[index].as_secs_f64() * 1000.0
        };

        Some(format!(
            "{} clicks: median {:.1} ms, 95th {:.1} ms, worst {:.1} ms",
            sorted.len(),
            at(0.5),
            at(0.95),
            at(1.0),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::ClickLatencyProbe;

    #[test]
    fn a_disabled_probe_records_nothing() {
        let mut probe = ClickLatencyProbe::new(false, None);
        probe.pressed();
        probe.handled();
        probe.presented();

        assert!(probe.summary().is_none());
    }

    #[test]
    fn a_click_is_only_reported_once_it_reaches_the_screen() {
        let mut probe = ClickLatencyProbe::new(true, None);

        probe.pressed();
        probe.handled();
        assert!(probe.summary().is_none(), "not drawn yet");

        probe.presented();
        assert_eq!(
            probe.summary().as_deref().map(|s| &s[..8]),
            Some("1 clicks")
        );
    }

    /// Stages that never happened must not be counted as instant.
    #[test]
    fn a_present_with_no_click_in_flight_is_ignored() {
        let mut probe = ClickLatencyProbe::new(true, None);

        probe.presented();
        assert!(probe.summary().is_none());
    }

    /// A click arriving before the previous one was drawn replaces it, so the reported wait always
    /// belongs to the click it names.
    #[test]
    fn a_second_click_replaces_one_still_in_flight() {
        let mut probe = ClickLatencyProbe::new(true, None);

        probe.pressed();
        probe.pressed();
        probe.presented();
        probe.presented();

        assert_eq!(
            probe.summary().as_deref().map(|s| &s[..8]),
            Some("1 clicks")
        );
    }
}
