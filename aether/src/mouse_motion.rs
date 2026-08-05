use std::time::{Duration, Instant};

pub const MOUSE_MOTION_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 60);

#[derive(Debug, PartialEq)]
pub struct MouseMotionDecision<T> {
    pub dispatch: Option<T>,
    pub coalesced: bool,
}

impl<T> MouseMotionDecision<T> {
    fn dispatch(position: T, coalesced: bool) -> Self {
        Self {
            dispatch: Some(position),
            coalesced,
        }
    }

    fn queued(coalesced: bool) -> Self {
        Self {
            dispatch: None,
            coalesced,
        }
    }
}

pub struct MouseMotionCoalescer<T> {
    enabled: bool,
    pending: Option<T>,
    next_dispatch_at: Option<Instant>,
}

impl<T> MouseMotionCoalescer<T> {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            pending: None,
            next_dispatch_at: None,
        }
    }

    pub fn push(&mut self, position: T, now: Instant) -> MouseMotionDecision<T> {
        if !self.enabled {
            return MouseMotionDecision::dispatch(position, false);
        }

        let coalesced = self.pending.replace(position).is_some();
        if !coalesced {
            self.next_dispatch_at = Some(now + MOUSE_MOTION_INTERVAL);
        }
        MouseMotionDecision::queued(coalesced)
    }

    pub fn take_due(&mut self, now: Instant) -> Option<T> {
        let deadline = self.next_dispatch_at?;
        if self.pending.is_none() || now < deadline {
            return None;
        }

        self.next_dispatch_at = None;
        self.pending.take()
    }

    pub fn take_for_render(&mut self, _now: Instant) -> Option<T> {
        self.next_dispatch_at = None;
        self.pending.take()
    }

    pub fn flush(&mut self, _now: Instant) -> Option<T> {
        let position = self.pending.take()?;
        self.next_dispatch_at = None;
        Some(position)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.as_ref().and(self.next_dispatch_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn disabled_policy_dispatches_every_position_immediately() {
        let start = Instant::now();
        let mut policy = MouseMotionCoalescer::new(false);

        assert_eq!(policy.push(1, start).dispatch, Some(1));
        assert_eq!(
            policy.push(2, start + Duration::from_millis(1)).dispatch,
            Some(2)
        );
        assert_eq!(policy.next_deadline(), None);
    }

    #[test]
    fn enabled_policy_queues_motion_and_idle_fallback_keeps_latest() {
        let start = Instant::now();
        let mut policy = MouseMotionCoalescer::new(true);

        assert_eq!(policy.push(1, start), MouseMotionDecision::queued(false));
        assert_eq!(
            policy.push(2, start + Duration::from_millis(1)),
            MouseMotionDecision::queued(true)
        );
        assert_eq!(
            policy.push(3, start + Duration::from_millis(2)),
            MouseMotionDecision::queued(true)
        );
        assert_eq!(
            policy.take_due(start + MOUSE_MOTION_INTERVAL - Duration::from_nanos(1)),
            None
        );
        assert_eq!(policy.take_due(start + MOUSE_MOTION_INTERVAL), Some(3));
        assert_eq!(policy.next_deadline(), None);
    }

    #[test]
    fn a_new_position_at_the_idle_deadline_replaces_the_pending_position() {
        let start = Instant::now();
        let mut policy = MouseMotionCoalescer::new(true);

        assert_eq!(policy.push(1, start), MouseMotionDecision::queued(false));
        assert_eq!(
            policy.push(2, start + Duration::from_millis(1)),
            MouseMotionDecision::queued(true)
        );
        assert_eq!(
            policy.push(3, start + MOUSE_MOTION_INTERVAL),
            MouseMotionDecision::queued(true)
        );
        assert_eq!(policy.take_due(start + MOUSE_MOTION_INTERVAL), Some(3));
    }

    #[test]
    fn flush_preserves_the_latest_position_before_a_discrete_event() {
        let start = Instant::now();
        let mut policy = MouseMotionCoalescer::new(true);

        assert_eq!(policy.push(1, start), MouseMotionDecision::queued(false));
        policy.push(2, start + Duration::from_millis(1));
        policy.push(3, start + Duration::from_millis(2));

        assert_eq!(policy.flush(start + Duration::from_millis(3)), Some(3));
        assert_eq!(policy.next_deadline(), None);
    }

    #[test]
    fn enabled_motion_is_delivered_once_on_the_next_rendered_frame() {
        let start = Instant::now();
        let mut policy = MouseMotionCoalescer::new(true);

        assert_eq!(policy.push(1, start), MouseMotionDecision::queued(false));
        assert_eq!(
            policy.push(2, start + Duration::from_millis(1)),
            MouseMotionDecision::queued(true)
        );

        assert_eq!(
            policy.take_for_render(start + Duration::from_millis(2)),
            Some(2)
        );
        assert_eq!(
            policy.take_for_render(start + Duration::from_millis(2)),
            None
        );
    }
}
