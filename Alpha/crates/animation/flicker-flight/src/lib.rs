//! flicker-flight — a **camera-cinematic ("flight path") service**.
//!
//! A camera cinematic is a *description of an animation* — authored data, replayed
//! by a runtime — much like a `.pack`, but its own file type (`.flight`) because a
//! camera path is neither a skeleton/skin (`.rig`) nor an animation state graph
//! (`.pack`). This crate parses a `.flight` and plays it back, driving a camera
//! through a scripted sequence.
//!
//! # The format (`.flight`, JSON)
//!
//! A flight is a `target` (the orbit centre / look-at) plus an ordered list of
//! **segments**. Each segment eases from one [`OrbitPose`] to another over a
//! `duration` (seconds). The **last** segment may `loop` — an endless idle "coast"
//! tail (e.g. a slow orbit) so a cinematic never dead-stops.
//!
//! ```json
//! {
//!   "format": "flicker.flight", "version": 1,
//!   "target": [0.0, 0.0, 0.0],
//!   "segments": [
//!     { "name": "glide", "duration": 36.0, "ease": "smooth_step",
//!       "from": { "yaw": 0.10, "pitch": -0.24, "distance": 45.0 },
//!       "to":   { "yaw": 1.00, "pitch": 0.52,  "distance": 6.24 } },
//!     { "name": "coast", "duration": 251.33, "ease": "linear", "loop": true,
//!       "from": { "yaw": 1.00,     "pitch": 0.52, "distance": 6.24 },
//!       "to":   { "yaw": 7.283185, "pitch": 0.52, "distance": 6.24 } }
//!   ]
//! }
//! ```
//!
//! # Playing it
//!
//! ```ignore
//! let mut player = FlightPlayer::new(Flight::load("intro.flight")?);
//! // each frame, while the cinematic is driving the camera:
//! let pose = player.advance(dt.as_secs_f32());
//! camera.set_pose(pose.yaw, pose.pitch, pose.distance); // your engine's orbit camera
//! ```
//!
//! **Render-agnostic on purpose:** the service emits [`OrbitPose`]s (and the
//! flight's [`Flight::target`]); the client maps them to its own camera (e.g.
//! `flicker_render::Camera::orbit(target, distance, yaw, pitch)`). No render / GPU
//! dependency, so it's usable from tools and headless tests too.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// An orbit-camera pose: yaw + pitch (radians) and distance from the flight's
/// target. This is the pose type the runtime interpolates and hands back.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct OrbitPose {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl OrbitPose {
    /// Component-wise linear blend `a → b` at `u ∈ [0,1]`.
    fn lerp(a: Self, b: Self, u: f32) -> Self {
        let l = |x: f32, y: f32| x + (y - x) * u;
        Self {
            yaw: l(a.yaw, b.yaw),
            pitch: l(a.pitch, b.pitch),
            distance: l(a.distance, b.distance),
        }
    }
}

/// The easing applied to a segment's normalised time before interpolating.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    /// Constant speed.
    #[default]
    Linear,
    /// Ease in and out (`3t²−2t³`) — a gentle start *and* arrival.
    SmoothStep,
    /// Ease in (`t²`) — slow start, fast finish.
    EaseIn,
    /// Ease out (`t·(2−t)`) — fast start, gentle finish.
    EaseOut,
}

impl Ease {
    /// Map a normalised time `t ∈ [0,1]` through the curve.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            Ease::SmoothStep => t * t * (3.0 - 2.0 * t),
            Ease::EaseIn => t * t,
            Ease::EaseOut => t * (2.0 - t),
        }
    }
}

/// One leg of a flight: an eased interpolation `from → to` over `duration`
/// seconds. Only the **final** segment may `loop` (an endless idle tail).
#[derive(Debug, Clone, Deserialize)]
pub struct Segment {
    #[serde(default)]
    pub name: String,
    pub duration: f32,
    #[serde(default)]
    pub ease: Ease,
    #[serde(default, rename = "loop")]
    pub looping: bool,
    pub from: OrbitPose,
    pub to: OrbitPose,
}

/// A parsed `.flight` — a camera cinematic as authored data.
#[derive(Debug, Clone, Deserialize)]
pub struct Flight {
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub version: u32,
    /// The orbit centre / look-at point (world space).
    #[serde(default)]
    pub target: [f32; 3],
    pub segments: Vec<Segment>,
}

impl Flight {
    /// Parse and validate a `.flight` from JSON text.
    pub fn from_json(json: &str) -> Result<Self> {
        let flight: Flight = serde_json::from_str(json).context("parsing .flight JSON")?;
        flight.validate()?;
        Ok(flight)
    }

    /// Read and parse a `.flight` file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading .flight {}", path.display()))?;
        Self::from_json(&text).with_context(|| format!("in {}", path.display()))
    }

    fn validate(&self) -> Result<()> {
        if self.segments.is_empty() {
            bail!("a .flight needs at least one segment");
        }
        let last = self.segments.len() - 1;
        for (i, s) in self.segments.iter().enumerate() {
            if s.duration <= 0.0 {
                bail!("segment {i} ('{}') has non-positive duration {}", s.name, s.duration);
            }
            if s.looping && i != last {
                bail!("segment {i} ('{}') loops, but only the final segment may loop", s.name);
            }
        }
        Ok(())
    }

    /// The orbit centre / look-at point.
    pub fn target(&self) -> [f32; 3] {
        self.target
    }

    /// `true` if the flight ends in a looping tail (plays forever).
    pub fn loops(&self) -> bool {
        self.segments.last().map(|s| s.looping).unwrap_or(false)
    }

    /// The finite **lead-in** duration: the sum of every segment before a looping
    /// tail (i.e. the whole one-shot run). A scene's reveal/dust clock tracks this
    /// so it stays in step with the fly-in.
    pub fn lead_in(&self) -> f32 {
        self.segments.iter().take_while(|s| !s.looping).map(|s| s.duration).sum()
    }

    /// Total run length, or `None` if the flight loops forever.
    pub fn total(&self) -> Option<f32> {
        if self.loops() {
            None
        } else {
            Some(self.segments.iter().map(|s| s.duration).sum())
        }
    }

    /// The interpolated pose at `time` seconds. Time past a non-looping end holds
    /// the final pose; a looping tail wraps within its own duration.
    pub fn pose_at(&self, time: f32) -> OrbitPose {
        let mut acc = 0.0;
        let last = self.segments.len() - 1;
        for (i, seg) in self.segments.iter().enumerate() {
            let end = acc + seg.duration;
            if time < end || i == last {
                let local = if seg.looping {
                    (time - acc).rem_euclid(seg.duration)
                } else {
                    (time - acc).clamp(0.0, seg.duration)
                };
                let u = seg.ease.apply(local / seg.duration);
                return OrbitPose::lerp(seg.from, seg.to, u);
            }
            acc = end;
        }
        self.segments[last].to // unreachable (segments validated non-empty)
    }

    /// The name of the segment active at `time` (for HUD / debug).
    pub fn segment_at(&self, time: f32) -> &str {
        let mut acc = 0.0;
        let last = self.segments.len() - 1;
        for (i, seg) in self.segments.iter().enumerate() {
            if time < acc + seg.duration || i == last {
                return &seg.name;
            }
            acc += seg.duration;
        }
        ""
    }
}

/// Stateful playback of a [`Flight`]: advance it each frame and read the pose.
pub struct FlightPlayer {
    flight: Flight,
    time: f32,
    finished: bool,
}

impl FlightPlayer {
    pub fn new(flight: Flight) -> Self {
        Self { flight, time: 0.0, finished: false }
    }

    /// Restart from the opening pose.
    pub fn restart(&mut self) {
        self.time = 0.0;
        self.finished = false;
    }

    /// Advance the clock by `dt` seconds and return the current pose. A non-looping
    /// flight clamps at its end (and reports [`is_finished`](Self::is_finished)).
    pub fn advance(&mut self, dt: f32) -> OrbitPose {
        self.time += dt.max(0.0);
        if let Some(total) = self.flight.total() {
            if self.time >= total {
                self.time = total;
                self.finished = true;
            }
        }
        self.pose()
    }

    /// The current pose without advancing.
    pub fn pose(&self) -> OrbitPose {
        self.flight.pose_at(self.time)
    }

    /// `true` once a non-looping flight reaches its end (never true while looping).
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Elapsed seconds since the start / last restart.
    pub fn elapsed(&self) -> f32 {
        self.time
    }

    /// Lead-in progress `0..1`: 0 at the start, 1 once the finite lead-in
    /// (everything before a looping tail) completes. Drives a scene's dust /
    /// reveal clock so it stays in sync with the fly-in.
    pub fn progress(&self) -> f32 {
        let lead = self.flight.lead_in();
        if lead <= 0.0 {
            1.0
        } else {
            (self.time / lead).clamp(0.0, 1.0)
        }
    }

    /// Name of the currently-active segment (e.g. `"glide"` / `"coast"`).
    pub fn segment_name(&self) -> &str {
        self.flight.segment_at(self.time)
    }

    /// The underlying flight (e.g. for its [`Flight::target`]).
    pub fn flight(&self) -> &Flight {
        &self.flight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "format": "flicker.flight", "version": 1,
      "target": [0.0, 0.0, 0.0],
      "segments": [
        { "name": "glide", "duration": 36.0, "ease": "smooth_step",
          "from": { "yaw": 0.10, "pitch": -0.24, "distance": 45.0 },
          "to":   { "yaw": 1.00, "pitch": 0.52,  "distance": 6.24 } },
        { "name": "coast", "duration": 251.33, "ease": "linear", "loop": true,
          "from": { "yaw": 1.00,     "pitch": 0.52, "distance": 6.24 },
          "to":   { "yaw": 7.283185, "pitch": 0.52, "distance": 6.24 } }
      ]
    }"#;

    #[test]
    fn parses_and_bounds_the_endpoints() {
        let f = Flight::from_json(SAMPLE).unwrap();
        assert_eq!(f.segments.len(), 2);
        assert!(f.loops());
        assert!(f.total().is_none()); // looping tail → no finite end
        assert_eq!(f.lead_in(), 36.0);
        // t=0 is the glide's opening pose.
        assert_eq!(f.pose_at(0.0), f.segments[0].from);
        // End of the glide is its target pose (smoothstep hits 1 at t=1).
        let end = f.pose_at(36.0);
        assert!((end.yaw - 1.00).abs() < 1e-4 && (end.distance - 6.24).abs() < 1e-4);
    }

    #[test]
    fn progress_and_segment_track_the_fly_in() {
        let mut p = FlightPlayer::new(Flight::from_json(SAMPLE).unwrap());
        assert_eq!(p.progress(), 0.0);
        assert_eq!(p.segment_name(), "glide");
        p.advance(18.0);
        assert!((p.progress() - 0.5).abs() < 1e-4);
        p.advance(18.0); // reach the end of the glide
        assert!(p.progress() >= 1.0);
        assert_eq!(p.segment_name(), "coast");
        assert!(!p.is_finished()); // looping tail never finishes
    }

    #[test]
    fn coast_loops_seamlessly() {
        let f = Flight::from_json(SAMPLE).unwrap();
        // One full loop period later, the wrapped pose matches the loop start.
        let a = f.pose_at(36.0 + 0.0);
        let b = f.pose_at(36.0 + 251.33);
        assert!((a.yaw - b.yaw).abs() < 1e-3);
    }

    #[test]
    fn rejects_a_non_final_loop() {
        let bad = r#"{ "segments": [
          { "duration": 1.0, "loop": true, "from": {"yaw":0.0,"pitch":0.0,"distance":1.0}, "to": {"yaw":0.0,"pitch":0.0,"distance":1.0} },
          { "duration": 1.0, "from": {"yaw":0.0,"pitch":0.0,"distance":1.0}, "to": {"yaw":0.0,"pitch":0.0,"distance":1.0} }
        ] }"#;
        assert!(Flight::from_json(bad).is_err());
    }

    #[test]
    fn rejects_empty_and_bad_duration() {
        assert!(Flight::from_json(r#"{ "segments": [] }"#).is_err());
        let z = r#"{ "segments": [ { "duration": 0.0, "from": {"yaw":0.0,"pitch":0.0,"distance":1.0}, "to": {"yaw":0.0,"pitch":0.0,"distance":1.0} } ] }"#;
        assert!(Flight::from_json(z).is_err());
    }
}
