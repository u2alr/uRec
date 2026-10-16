//! Noticing a capture that technically runs but delivers almost no new frames.
//!
//! Windows Graphics Capture can stall (a fullscreen game that bypasses the desktop compositor
//! delivers about one new picture per second). FFmpeg then pads the output to the full frame
//! rate with copies, so the file looks fine on paper but plays like a slideshow. The tell is
//! the share of output frames that are copies.

/// Length of one measuring window, seconds.
const WINDOW: f64 = 20.0;
/// Fewer real frames than this share of the requested rate counts as stalled.
const SLOW_SHARE: f64 = 0.2;
/// Consecutive stalled windows before it is reported.
const WINDOWS_NEEDED: u32 = 2;

#[derive(Default, Debug)]
pub struct StallWatch {
    mark: Option<(f64, u64, u64)>,
    slow_windows: u32,
}

impl StallWatch {
    pub fn reset(&mut self) {
        *self = StallWatch::default();
    }

    /// Feed the latest counters (`now` in seconds from any fixed point). True once capture
    /// has been stalled for long enough to act on.
    pub fn observe(&mut self, now: f64, frame: u64, dup_frames: u64, fps: u32) -> bool {
        let Some((t0, f0, d0)) = self.mark else {
            self.mark = Some((now, frame, dup_frames));
            return false;
        };
        let dt = now - t0;
        if dt < WINDOW {
            return false;
        }
        let real = frame.saturating_sub(f0).saturating_sub(dup_frames.saturating_sub(d0)) as f64;
        let slow = real < fps as f64 * dt * SLOW_SHARE;
        self.mark = Some((now, frame, dup_frames));
        self.slow_windows = if slow { self.slow_windows + 1 } else { 0 };
        self.slow_windows >= WINDOWS_NEEDED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_healthy_capture_is_never_reported() {
        let mut w = StallWatch::default();
        for i in 0..10u64 {
            assert!(!w.observe(i as f64 * 20.0, i * 2400, i * 100, 120));
        }
    }

    #[test]
    fn a_stalled_capture_is_reported_after_two_windows() {
        let mut w = StallWatch::default();
        assert!(!w.observe(0.0, 0, 0, 120));
        // 20 s: 2400 output frames, 2380 of them copies.
        assert!(!w.observe(20.0, 2400, 2380, 120));
        assert!(w.observe(40.0, 4800, 4760, 120));
    }

    #[test]
    fn a_recovery_resets_the_count() {
        let mut w = StallWatch::default();
        w.observe(0.0, 0, 0, 60);
        assert!(!w.observe(20.0, 1200, 1190, 60));
        assert!(!w.observe(40.0, 2400, 1190, 60));
        assert!(!w.observe(60.0, 3600, 2380, 60));
    }
}
