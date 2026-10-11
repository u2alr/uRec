//! The Instant Replay indicator: the overlay dot briefly turns into a pill that says
//! "Instant Replay Saved" (green check) or "Instant Replay Failed" (red cross), then shrinks
//! back into the dot.
//!
//! Platform-neutral so it can be unit-tested: `Indicator` is the timeline (driven by a
//! monotonic clock, never by frame counts) and `render` draws one frame into premultiplied
//! BGRA pixels. The Windows overlay only feeds it events and puts the pixels on screen.

use std::time::{Duration, Instant};

pub const EXPAND_MS: u64 = 180;
pub const REVEAL_MS: u64 = 160;
pub const HOLD_SUCCESS_MS: u64 = 2000;
pub const HOLD_FAILURE_MS: u64 = 2500;
pub const COLLAPSE_MS: u64 = 180;
/// The dot breathes while a save is running; give up after this long without a result.
pub const SAVING_TIMEOUT: Duration = Duration::from_secs(90);

pub const SUCCESS_TEXT: &str = "Instant Replay Saved";
pub const FAILURE_TEXT: &str = "Instant Replay Failed";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Success,
    Failure,
}

impl Kind {
    pub fn text(self) -> &'static str {
        match self {
            Kind::Success => SUCCESS_TEXT,
            Kind::Failure => FAILURE_TEXT,
        }
    }
    fn hold(self) -> Duration {
        Duration::from_millis(match self {
            Kind::Success => HOLD_SUCCESS_MS,
            Kind::Failure => HOLD_FAILURE_MS,
        })
    }
    pub fn accent(self) -> (u8, u8, u8) {
        match self {
            Kind::Success => (52, 199, 89),
            Kind::Failure => (229, 72, 77),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Idle,
    /// Growing from `from` (0 = dot) to the full pill.
    Expanding {
        start: Instant,
        from: f32,
    },
    Holding {
        until: Instant,
    },
    Collapsing {
        start: Instant,
    },
}

/// One frame of the animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// 0 = the plain dot, 1 = the full pill (already eased).
    pub expand: f32,
    /// Opacity of the icon and text, 0..1 (already eased).
    pub content: f32,
    pub kind: Kind,
    /// The kind being replaced while the width morphs to the new message, with progress 0..1.
    pub morph: Option<(Kind, f32)>,
}

/// What the overlay should draw right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// The normal status dot, at this opacity (1.0 = steady; less while a save is running).
    Dot {
        alpha: f32,
    },
    Pill(Frame),
}

pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

fn progress(since: Instant, now: Instant, ms: u64) -> f32 {
    if ms == 0 {
        return 1.0;
    }
    (now.saturating_duration_since(since).as_secs_f32() * 1000.0 / ms as f32).clamp(0.0, 1.0)
}

/// The indicator's timeline. Results are never lost or reordered against each other:
/// - another success while a success is showing extends it (no duplicate pop-ups);
/// - a failure always replaces a success that is showing (an older success never hides a
///   newer failure);
/// - a success arriving while a failure is showing waits until the failure has been seen.
#[derive(Clone, Debug)]
pub struct Indicator {
    phase: Phase,
    kind: Kind,
    /// Previous message while the pill morphs to a new one.
    morph: Option<(Kind, Instant)>,
    reveal_start: Option<Instant>,
    pending: Option<Kind>,
    saving_since: Option<Instant>,
    reduced_motion: bool,
}

impl Default for Indicator {
    fn default() -> Self {
        Indicator {
            phase: Phase::Idle,
            kind: Kind::Success,
            morph: None,
            reveal_start: None,
            pending: None,
            saving_since: None,
            reduced_motion: false,
        }
    }
}

impl Indicator {
    /// Instant transitions instead of motion (Windows "Show animations" off, or the app setting).
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.reduced_motion = on;
    }

    fn ms(&self, ms: u64) -> u64 {
        if self.reduced_motion {
            0
        } else {
            ms
        }
    }

    /// A save has started (the dot breathes until the result arrives).
    pub fn saving(&mut self, now: Instant) {
        self.saving_since = Some(now);
    }

    /// A save has finished. Only call with `Success` once the file is finalized.
    pub fn finished(&mut self, kind: Kind, now: Instant) {
        self.saving_since = None;
        match self.phase {
            Phase::Idle => self.open(kind, now, 0.0),
            Phase::Collapsing { start } => {
                // Grow back from wherever the collapse got to.
                let current = 1.0 - ease_in_out(progress(start, now, self.ms(COLLAPSE_MS)));
                if kind != self.kind {
                    self.morph = Some((self.kind, now));
                }
                self.open(kind, now, current);
            }
            Phase::Expanding { .. } | Phase::Holding { .. } => match (self.kind, kind) {
                (Kind::Success, Kind::Success) | (Kind::Failure, Kind::Failure) => {
                    // Same message again: keep it up a little longer instead of popping twice.
                    if let Phase::Holding { .. } = self.phase {
                        self.phase = Phase::Holding { until: now + kind.hold() };
                    }
                }
                (Kind::Success, Kind::Failure) => {
                    self.morph = Some((Kind::Success, now));
                    self.kind = Kind::Failure;
                    self.reveal_start = Some(now);
                    if let Phase::Holding { .. } = self.phase {
                        self.phase = Phase::Holding { until: now + Kind::Failure.hold() };
                    }
                }
                (Kind::Failure, Kind::Success) => self.pending = Some(Kind::Success),
            },
        }
    }

    fn open(&mut self, kind: Kind, now: Instant, from: f32) {
        self.kind = kind;
        self.phase = Phase::Expanding { start: now, from };
        // The message starts fading in shortly before the pill is fully open.
        let lead = self.ms(EXPAND_MS).saturating_sub(40);
        self.reveal_start = Some(now + Duration::from_millis(lead));
    }

    /// Advance the timeline and say what to draw.
    pub fn look(&mut self, now: Instant) -> Look {
        if let Some(s) = self.saving_since {
            if now.saturating_duration_since(s) > SAVING_TIMEOUT {
                self.saving_since = None;
            }
        }
        // Phase transitions.
        loop {
            match self.phase {
                Phase::Expanding { start, .. } if progress(start, now, self.ms(EXPAND_MS)) >= 1.0 => {
                    let end = start + Duration::from_millis(self.ms(EXPAND_MS));
                    self.phase = Phase::Holding { until: end + self.kind.hold() };
                }
                Phase::Holding { until } if now >= until => {
                    self.phase = Phase::Collapsing { start: until };
                }
                Phase::Collapsing { start } if progress(start, now, self.ms(COLLAPSE_MS)) >= 1.0 => {
                    self.phase = Phase::Idle;
                    self.morph = None;
                    self.reveal_start = None;
                    if let Some(k) = self.pending.take() {
                        let at = start + Duration::from_millis(self.ms(COLLAPSE_MS));
                        self.open(k, at.min(now), 0.0);
                        continue;
                    }
                }
                _ => break,
            }
        }
        let reveal = self.reveal_start.map(|r| ease_out(progress(r, now, self.ms(REVEAL_MS)))).unwrap_or(0.0);
        let morph = self.morph.and_then(|(k, at)| {
            let p = ease_in_out(progress(at, now, self.ms(REVEAL_MS)));
            (p < 1.0).then_some((k, p))
        });
        if morph.is_none() {
            self.morph = None;
        }
        let frame = |expand: f32, content: f32| Frame { expand, content, kind: self.kind, morph };
        match self.phase {
            Phase::Idle => {
                let alpha = match self.saving_since {
                    Some(_) if self.reduced_motion => 0.6,
                    Some(s) => {
                        let t = now.saturating_duration_since(s).as_secs_f32();
                        0.725 + 0.275 * (t * std::f32::consts::TAU / 1.2).cos()
                    }
                    None => 1.0,
                };
                Look::Dot { alpha }
            }
            Phase::Expanding { start, from } => {
                let p = ease_out(progress(start, now, self.ms(EXPAND_MS)));
                Look::Pill(frame(from + (1.0 - from) * p, reveal))
            }
            Phase::Holding { .. } => Look::Pill(frame(1.0, reveal)),
            Phase::Collapsing { start } => {
                let p = progress(start, now, self.ms(COLLAPSE_MS));
                // Message fades out during the first half, then the pill settles into the dot.
                Look::Pill(frame(1.0 - ease_in_out(p), reveal.min(1.0 - (p * 2.0).min(1.0))))
            }
        }
    }

    /// True while something moves, so the overlay knows to keep its timer running.
    pub fn animating(&self) -> bool {
        !matches!(self.phase, Phase::Idle) || (self.saving_since.is_some() && !self.reduced_motion)
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle) && self.pending.is_none()
    }
}

// ------------------------------------------------------------------ drawing

/// Text coverage (0..255 per pixel), rendered by the platform (GDI on Windows).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextMask {
    pub w: usize,
    pub h: usize,
    pub cov: Vec<u8>,
}

/// Sizes for one display scale.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    /// Diameter of the status dot.
    pub dot: f32,
    /// Halo around dot and pill (dark, for contrast on bright scenes).
    pub halo: f32,
    pub height: f32,
    pub icon: f32,
    pub gap: f32,
    pub pad_end: f32,
}

impl Metrics {
    pub fn for_scale(k: f32) -> Metrics {
        Metrics { dot: 13.2 * k, halo: 2.64 * k, height: 30.0 * k, icon: 18.0 * k, gap: 8.0 * k, pad_end: 13.0 * k }
    }

    /// Full pill width for a message of `text_w` pixels.
    pub fn width(&self, text_w: usize) -> f32 {
        self.height / 2.0 + self.icon / 2.0 + self.gap + text_w as f32 + self.pad_end
    }
}

/// A rendered frame: premultiplied BGRA, top-down. `anchor_x/anchor_y` is where the dot's
/// centre is inside this image, so the caller can keep the dot in exactly the same place.
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
    pub anchor_x: f32,
    pub anchor_y: f32,
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Distance from p to segment ab.
fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (cx, cy) = (ax + dx * t - px, ay + dy * t - py);
    (cx * cx + cy * cy).sqrt()
}

/// Paint `src` (straight RGB, alpha 0..1) over the premultiplied pixel at `i`.
fn over(px: &mut [u8], i: usize, rgb: (u8, u8, u8), a: f32) {
    if a <= 0.0 {
        return;
    }
    let a = a.min(1.0);
    let inv = 1.0 - a;
    px[i] = (rgb.2 as f32 * a + px[i] as f32 * inv).round() as u8;
    px[i + 1] = (rgb.1 as f32 * a + px[i + 1] as f32 * inv).round() as u8;
    px[i + 2] = (rgb.0 as f32 * a + px[i + 2] as f32 * inv).round() as u8;
    px[i + 3] = (255.0 * a + px[i + 3] as f32 * inv).round().min(255.0) as u8;
}

/// Draw one frame. `dot_color` is the status dot's colour (red recording / amber replay),
/// `grow_left` is true for right-hand corners (the pill grows away from the screen edge).
/// `texts` gives the mask for each kind's message.
pub fn render(
    frame: &Frame,
    m: &Metrics,
    dot_color: (u8, u8, u8),
    grow_left: bool,
    mask_for: &dyn Fn(Kind) -> TextMask,
) -> Image {
    let mask = mask_for(frame.kind);
    let full_w = match frame.morph {
        Some((old, p)) => mix(m.width(mask_for(old).w), m.width(mask.w), p),
        None => m.width(mask.w),
    };
    let e = frame.expand.clamp(0.0, 1.0);
    let h = mix(m.dot, m.height, e);
    let w = mix(m.dot, full_w, e).max(h);
    let canvas_w = (full_w.max(m.width(mask.w)) + 2.0 * m.halo + 4.0).ceil() as usize;
    let canvas_h = (m.height + 2.0 * m.halo + 4.0).ceil() as usize;
    // Pixel-centre aligned so the icon is symmetric.
    let cy = (canvas_h as f32 / 2.0).floor() + 0.5;
    // The dot/icon centre stays put; the pill extends away from the screen edge.
    let anchor_x =
        if grow_left { canvas_w as f32 - m.halo - 2.0 - m.height / 2.0 } else { m.halo + 2.0 + m.height / 2.0 }.floor()
            + 0.5;
    let r = h / 2.0;
    let (left, right) = if grow_left { (anchor_x + r - w, anchor_x + r) } else { (anchor_x - r, anchor_x - r + w) };
    // Stadium centre line.
    let (l0, l1) = (left + r, right - r);
    let accent = frame.kind.accent();
    let fill_dark = (24u8, 24u8, 27u8);
    let fill = (
        mix(dot_color.0 as f32, fill_dark.0 as f32, e) as u8,
        mix(dot_color.1 as f32, fill_dark.1 as f32, e) as u8,
        mix(dot_color.2 as f32, fill_dark.2 as f32, e) as u8,
    );
    let fill_a = mix(1.0, 0.88, e);
    let icon_r = mix(0.0, m.icon / 2.0, frame.content);
    let stroke = (m.icon * 0.11).max(1.2);
    let k = m.icon / 18.0;
    // Text placement: next to the icon, sliding in from the icon side.
    let slide = (1.0 - frame.content) * 6.0 * k;
    let text_x0 = if grow_left {
        anchor_x - m.icon / 2.0 - m.gap - mask.w as f32 + slide
    } else {
        anchor_x + m.icon / 2.0 + m.gap - slide
    };
    let text_y0 = cy - mask.h as f32 / 2.0;
    let mut px = vec![0u8; canvas_w * canvas_h * 4];
    for y in 0..canvas_h {
        for x in 0..canvas_w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = seg_dist(fx, fy, l0, cy, l1, cy) - r;
            let i = (y * canvas_w + x) * 4;
            // Halo, then the body (anti-aliased by distance).
            let halo = (0.5 - (d - m.halo)).clamp(0.0, 1.0) * 0.35;
            over(&mut px, i, (0, 0, 0), halo * (1.0 - (0.5 - d).clamp(0.0, 1.0)));
            let body = (0.5 - d).clamp(0.0, 1.0);
            if body <= 0.0 {
                continue;
            }
            over(&mut px, i, fill, body * fill_a);
            // Hairline border in the accent colour once open.
            let edge = (1.0 - (d + 1.0).abs()).clamp(0.0, 1.0) * 0.5 * e * frame.content;
            over(&mut px, i, accent, edge * body);
            if frame.content <= 0.0 {
                continue;
            }
            // Icon: accent disc with a white check or cross (shape differs, not only colour).
            let di = ((fx - anchor_x).powi(2) + (fy - cy).powi(2)).sqrt();
            let disc = (0.5 - (di - icon_r)).clamp(0.0, 1.0) * frame.content;
            if disc > 0.0 {
                over(&mut px, i, accent, disc * body);
                let (ix, iy) = ((fx - anchor_x) / k, (fy - cy) / k);
                let sd = match frame.kind {
                    Kind::Success => seg_dist(ix, iy, -4.3, 0.2, -1.4, 3.1).min(seg_dist(ix, iy, -1.4, 3.1, 4.4, -3.0)),
                    Kind::Failure => seg_dist(ix, iy, -3.4, -3.4, 3.4, 3.4).min(seg_dist(ix, iy, -3.4, 3.4, 3.4, -3.4)),
                } * k;
                let mark = (0.5 - (sd - stroke / 2.0)).clamp(0.0, 1.0) * frame.content;
                over(&mut px, i, (255, 255, 255), mark * disc.min(1.0));
            }
            // Text, clipped to the pill.
            let (tx, ty) = (fx - text_x0, fy - text_y0);
            if tx >= 0.0 && ty >= 0.0 && (tx as usize) < mask.w && (ty as usize) < mask.h {
                let c = mask.cov[ty as usize * mask.w + tx as usize] as f32 / 255.0;
                over(&mut px, i, (240, 240, 242), c * frame.content * body);
            }
        }
    }
    Image { w: canvas_w, h: canvas_h, px, anchor_x, anchor_y: cy }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn success_timeline() {
        let t0 = Instant::now();
        let mut ind = Indicator::default();
        assert_eq!(ind.look(t0), Look::Dot { alpha: 1.0 });
        assert!(!ind.animating());
        ind.finished(Kind::Success, t0);
        assert!(ind.animating());
        let Look::Pill(f) = ind.look(ms(t0, 90)) else { panic!() };
        assert!(f.expand > 0.5 && f.expand < 1.0, "half-way through expanding: {}", f.expand);
        assert_eq!(f.kind, Kind::Success);
        let Look::Pill(f) = ind.look(ms(t0, 400)) else { panic!() };
        assert_eq!(f.expand, 1.0);
        assert_eq!(f.content, 1.0);
        let Look::Pill(f) = ind.look(ms(t0, 2100)) else { panic!() };
        assert_eq!(f.expand, 1.0, "still holding at 2.1 s");
        let Look::Pill(f) = ind.look(ms(t0, 2180 + 90)) else { panic!() };
        assert!(f.expand < 1.0 && f.content < 0.05, "collapsing, message gone: {f:?}");
        // Total ≈ 180 + 2000 + 180 ms, then back to an idle dot with nothing left running.
        assert_eq!(ind.look(ms(t0, 2361)), Look::Dot { alpha: 1.0 });
        assert!(!ind.animating());
        assert!(ind.is_idle());
    }

    #[test]
    fn failure_holds_longer() {
        let t0 = Instant::now();
        let mut ind = Indicator::default();
        ind.finished(Kind::Failure, t0);
        let Look::Pill(f) = ind.look(ms(t0, 2600)) else { panic!() };
        assert_eq!((f.kind, f.expand), (Kind::Failure, 1.0));
        assert_eq!(ind.look(ms(t0, 2861)), Look::Dot { alpha: 1.0 });
    }

    #[test]
    fn repeated_successes_extend_instead_of_duplicating() {
        let t0 = Instant::now();
        let mut ind = Indicator::default();
        ind.finished(Kind::Success, t0);
        ind.look(ms(t0, 1000));
        ind.finished(Kind::Success, ms(t0, 1000));
        let Look::Pill(f) = ind.look(ms(t0, 2900)) else { panic!("extended") };
        assert_eq!(f.expand, 1.0);
        assert_eq!(ind.look(ms(t0, 3200)), Look::Dot { alpha: 1.0 });
    }

    #[test]
    fn newer_failure_replaces_success_and_older_success_never_hides_failure() {
        let t0 = Instant::now();
        let mut ind = Indicator::default();
        ind.finished(Kind::Success, t0);
        ind.look(ms(t0, 500));
        ind.finished(Kind::Failure, ms(t0, 500));
        let Look::Pill(f) = ind.look(ms(t0, 900)) else { panic!() };
        assert_eq!(f.kind, Kind::Failure);
        // A success arriving now waits until the failure has been shown in full.
        ind.finished(Kind::Success, ms(t0, 900));
        let Look::Pill(f) = ind.look(ms(t0, 2900)) else { panic!() };
        assert_eq!(f.kind, Kind::Failure, "failure keeps its full 2.5 s");
        // Failure: hold until 500+2500 = 3000, collapse to 3180, then the queued success opens.
        let Look::Pill(f) = ind.look(ms(t0, 3500)) else { panic!("queued success shown") };
        assert_eq!(f.kind, Kind::Success);
        assert!(!ind.is_idle());
    }

    #[test]
    fn result_during_collapse_regrows_smoothly() {
        let t0 = Instant::now();
        let mut ind = Indicator::default();
        ind.finished(Kind::Success, t0);
        let Look::Pill(a) = ind.look(ms(t0, 2270)) else { panic!() };
        ind.finished(Kind::Success, ms(t0, 2270));
        let Look::Pill(b) = ind.look(ms(t0, 2271)) else { panic!() };
        assert!((a.expand - b.expand).abs() < 0.05, "no jump: {} -> {}", a.expand, b.expand);
    }

    #[test]
    fn saving_breathes_then_stops() {
        let t0 = Instant::now();
        let mut ind = Indicator::default();
        ind.saving(t0);
        assert!(ind.animating());
        let Look::Dot { alpha } = ind.look(ms(t0, 600)) else { panic!() };
        assert!(alpha < 0.6, "dimmed mid-breath: {alpha}");
        ind.finished(Kind::Success, ms(t0, 700));
        assert!(matches!(ind.look(ms(t0, 800)), Look::Pill(_)));
        // A save that never reports back doesn't keep the timer alive forever.
        let mut stuck = Indicator::default();
        stuck.saving(t0);
        stuck.look(t0 + SAVING_TIMEOUT + Duration::from_secs(1));
        assert!(!stuck.animating());
    }

    #[test]
    fn reduced_motion_jumps_between_states() {
        let t0 = Instant::now();
        let mut ind = Indicator::default();
        ind.set_reduced_motion(true);
        ind.saving(t0);
        assert!(!ind.animating(), "no breathing with reduced motion");
        assert_eq!(ind.look(t0), Look::Dot { alpha: 0.6 });
        ind.finished(Kind::Success, t0);
        let Look::Pill(f) = ind.look(t0) else { panic!() };
        assert_eq!((f.expand, f.content), (1.0, 1.0), "full message immediately");
        assert_eq!(ind.look(ms(t0, 2001)), Look::Dot { alpha: 1.0 });
    }

    fn mask(k: Kind) -> TextMask {
        let w = if k == Kind::Success { 120 } else { 124 };
        TextMask { w, h: 14, cov: vec![255; w * 14] }
    }

    #[test]
    fn closed_pill_is_the_dot_and_open_pill_has_message() {
        let m = Metrics::for_scale(1.0);
        let dot = render(
            &Frame { expand: 0.0, content: 0.0, kind: Kind::Success, morph: None },
            &m,
            (229, 72, 77),
            true,
            &mask,
        );
        let a = |img: &Image, x: f32, y: f32| img.px[((y as usize) * img.w + x as usize) * 4 + 3];
        let rgb = |img: &Image, x: f32, y: f32| {
            let i = ((y as usize) * img.w + x as usize) * 4;
            (img.px[i + 2], img.px[i + 1], img.px[i])
        };
        // Closed: just the red dot at the anchor, transparent elsewhere.
        assert_eq!(a(&dot, dot.anchor_x, dot.anchor_y), 255);
        assert_eq!(rgb(&dot, dot.anchor_x, dot.anchor_y), (229, 72, 77));
        assert_eq!(a(&dot, dot.anchor_x - 30.0, dot.anchor_y), 0);
        assert_eq!(a(&dot, 1.0, 1.0), 0);
        // Open: the icon (green) sits where the dot was; the body extends to the left.
        let open = render(
            &Frame { expand: 1.0, content: 1.0, kind: Kind::Success, morph: None },
            &m,
            (229, 72, 77),
            true,
            &mask,
        );
        assert_eq!(open.anchor_x, dot.anchor_x, "dot position is stable");
        let (r, g, _) = rgb(&open, open.anchor_x + 6.0, open.anchor_y - 6.0);
        assert!(g > 150 && r < 90, "green icon at the anchor: {r},{g}");
        assert!(a(&open, open.anchor_x - 60.0, open.anchor_y) > 200, "pill body extends left");
        assert_eq!(a(&open, open.w as f32 - 1.0, open.anchor_y), 0, "nothing past the screen-edge side");
        // Left corners mirror it.
        let lft = render(
            &Frame { expand: 1.0, content: 1.0, kind: Kind::Failure, morph: None },
            &m,
            (217, 164, 65),
            false,
            &mask,
        );
        assert!(a(&lft, lft.anchor_x + 60.0, lft.anchor_y) > 200);
        let (r, g, _) = rgb(&lft, lft.anchor_x + 6.0, lft.anchor_y - 6.0);
        assert!(r > 180 && g < 120, "red icon for failure: {r},{g}");
    }

    #[test]
    fn check_and_cross_differ_in_shape() {
        let m = Metrics::for_scale(2.0);
        let f = |kind| render(&Frame { expand: 1.0, content: 1.0, kind, morph: None }, &m, (229, 72, 77), true, &mask);
        let (s, x) = (f(Kind::Success), f(Kind::Failure));
        // White pixels inside the icon disc.
        let whites = |img: &Image| -> Vec<(i32, i32)> {
            let mut v = Vec::new();
            for dy in -8..=8 {
                for dx in -8..=8 {
                    let (px, py) = (img.anchor_x as i32 + dx, img.anchor_y as i32 + dy);
                    let i = (py as usize * img.w + px as usize) * 4;
                    if img.px[i] > 220 && img.px[i + 1] > 220 && img.px[i + 2] > 220 {
                        v.push((dx, dy));
                    }
                }
            }
            v
        };
        let (ws, wx) = (whites(&s), whites(&x));
        assert!(!ws.is_empty() && !wx.is_empty());
        assert_ne!(ws, wx);
        // The cross is symmetric left-right; the check is not.
        let mirrored = |v: &Vec<(i32, i32)>| v.iter().all(|(dx, dy)| v.contains(&(-dx, *dy)));
        assert!(mirrored(&wx));
        assert!(!mirrored(&ws));
    }
}
