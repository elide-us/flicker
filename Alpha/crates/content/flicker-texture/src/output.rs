//! The output stage — how one mixed field becomes a surface.
//!
//! The rack produces a single scalar `h ∈ [0,1]` per texel. Everything a renderer
//! needs is a projection of that one field: colour by sampling a ramp with it,
//! relief by differentiating it, roughness and metalness by modulating a base
//! level with it, occlusion by comparing a texel to its neighbourhood.
//!
//! Keeping one field and projecting it is what makes the console legible — every
//! map moves together when you turn a channel knob, the way a real material's
//! maps are all consequences of the same physical surface. Maps that could drift
//! independently would let you author a surface that cannot exist.

use serde::{Deserialize, Serialize};

/// One stop of a colour ramp: a position in `[0,1]` and the linear RGB there.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RampStop {
    /// Where on the field this colour sits.
    pub at: f32,
    /// Linear RGB, each `0..=1`.
    pub color: [f32; 3],
}

/// The field → colour mapping: a list of stops, sampled by linear interpolation.
///
/// Stops are kept **sorted by position** by [`ColorRamp::sample`] rather than
/// trusted, because a recipe can be hand-edited and an out-of-order stop should
/// produce the author's obvious intent instead of a garbled gradient.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorRamp {
    pub stops: Vec<RampStop>,
}

impl Default for ColorRamp {
    /// A neutral stone gradient — dark in the lows, light on the crests. Enough of
    /// a surface to see the field's shape the moment a channel is switched on.
    fn default() -> Self {
        Self {
            stops: vec![
                RampStop {
                    at: 0.0,
                    color: [0.09, 0.09, 0.10],
                },
                RampStop {
                    at: 0.5,
                    color: [0.36, 0.34, 0.32],
                },
                RampStop {
                    at: 1.0,
                    color: [0.78, 0.76, 0.72],
                },
            ],
        }
    }
}

impl ColorRamp {
    /// Colour at field value `t`, linearly interpolated between the bracketing
    /// stops and clamped at the ends.
    ///
    /// An empty ramp returns mid-grey — a ramp with no stops is an authoring
    /// mistake, and a visible neutral surface is easier to diagnose than black.
    pub fn sample(&self, t: f32) -> [f32; 3] {
        if self.stops.is_empty() {
            return [0.5, 0.5, 0.5];
        }
        let t = t.clamp(0.0, 1.0);
        // Sorted lazily on read: the list is 2-4 entries, so this costs nothing
        // measurable and removes a whole class of "the file was edited" bug.
        let mut stops: Vec<&RampStop> = self.stops.iter().collect();
        stops.sort_by(|a, b| a.at.total_cmp(&b.at));

        if t <= stops[0].at {
            return stops[0].color;
        }
        let last = stops[stops.len() - 1];
        if t >= last.at {
            return last.color;
        }
        for w in stops.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t >= a.at && t <= b.at {
                let span = b.at - a.at;
                // Coincident stops are a hard colour step, not a divide by zero.
                let k = if span > f32::EPSILON {
                    (t - a.at) / span
                } else {
                    0.0
                };
                return [
                    a.color[0] + (b.color[0] - a.color[0]) * k,
                    a.color[1] + (b.color[1] - a.color[1]) * k,
                    a.color[2] + (b.color[2] - a.color[2]) * k,
                ];
            }
        }
        last.color
    }

    /// The ramp's representative `(hue, saturation)`, read from its most saturated
    /// stop — where the hue is best defined, since a near-black or near-grey stop
    /// barely has one. `(0, 0)` for an empty or fully greyscale ramp.
    ///
    /// The ramp is the ONE representation of a recipe's colour; this is a reading
    /// of it for the bench's Hue/Saturation knobs, not a second copy that could
    /// drift from it.
    pub fn tint(&self) -> (f32, f32) {
        self.stops
            .iter()
            .map(|st| hsv_of(st.color))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or((0.0, 0.0), |(h, s, _)| (h, s))
    }

    /// Re-tint every stop onto a single `hue`/`saturation`, keeping each stop's
    /// position and BRIGHTNESS. The dark→light value profile that gives the surface
    /// its relief and contrast survives; only the chroma is swept onto one hue — so
    /// a grey stone becomes gold without losing its light-to-dark shape.
    ///
    /// This is the write side of the bench's colour knobs. It collapses a
    /// multi-hue authored ramp to a single-hue sweep by design: the moment you
    /// reach for the Hue knob, one hue is what you are asking for.
    pub fn recolor(&mut self, hue: f32, saturation: f32) {
        for st in &mut self.stops {
            let (_, _, v) = hsv_of(st.color);
            st.color = hsv(hue, saturation, v);
        }
    }
}

/// HSV → linear RGB. `h` wraps, so a hue walked past 1.0 keeps going round the
/// wheel rather than clamping to magenta. The ONE hue→rgb conversion in the crate,
/// shared by the randomiser and the bench's tint knobs.
pub(crate) fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

/// Linear RGB → `(hue, saturation, value)`, the inverse of [`hsv`]. Hue is `0` for
/// a greyscale colour, where it is undefined rather than meaningful.
fn hsv_of(rgb: [f32; 3]) -> (f32, f32, f32) {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let c = max - min;
    let v = max;
    let s = if max > f32::EPSILON { c / max } else { 0.0 };
    // The dominant channel picks the hue sextant — chosen by ORDERING, never by
    // float-equality against `max`.
    let h = if c <= f32::EPSILON {
        0.0
    } else if r >= g && r >= b {
        ((g - b) / c).rem_euclid(6.0)
    } else if g >= b {
        (b - r) / c + 2.0
    } else {
        (r - g) / c + 4.0
    };
    (h / 6.0, s, v)
}

/// How the mixed field projects into the PBR maps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputStage {
    /// Field → base colour.
    pub ramp: ColorRamp,
    /// Normal-map strength: how far the height gradient tips the surface normal.
    /// `0` is a flat surface (the map is all `(128,128,255)`); `1` is pronounced.
    pub relief: f32,
    /// Roughness at field midpoint, `0` mirror .. `1` fully matte.
    pub roughness: f32,
    /// How much the field modulates roughness about `roughness`. Signed: negative
    /// makes the crests the *shiny* part (polished high points, worn edges).
    pub roughness_mod: f32,
    /// Metalness at field midpoint. Physically this wants to be 0 or 1 — a
    /// blend is only meaningful where two materials share a texel.
    pub metalness: f32,
    /// How much the field modulates metalness — for ore veins in dielectric rock.
    pub metalness_mod: f32,
    /// Ambient-occlusion strength: how much a texel darkens for sitting below its
    /// neighbourhood. `0` writes a fully-open map.
    pub ao: f32,

    /// The glow's COLOUR, linear RGB. A colour rather than a scalar because
    /// emission has its own hue independent of the albedo under it — a blue rune
    /// cut into dark iron, lava in black basalt.
    #[serde(default)]
    pub emissive: [f32; 3],
    /// How brightly it glows. `0` (the default) writes a black map, and the shader
    /// then adds nothing — so an ordinary material costs emission nothing.
    #[serde(default)]
    pub emissive_strength: f32,
    /// WHERE it glows, as a FRACTION of the field's own range: `bake` seats it into
    /// the field's real `[min, max]`. It is NOT an absolute field value — the field
    /// rarely spans a full 0..1, so a raw threshold would sit above it and bake black
    /// (the bug that made hand-set emit controls do nothing). `0.75` lights only the
    /// crests; `0` lights everything above the floor. This is what makes emission
    /// follow the STRUCTURE — lava in the cracks between plates, not a uniform wash.
    #[serde(default)]
    pub emissive_band: f32,
}

impl Default for OutputStage {
    /// A matte, non-metallic stone with moderate relief — the surface a fresh
    /// recipe opens on, chosen so the very first channel you enable is legible.
    fn default() -> Self {
        Self {
            ramp: ColorRamp::default(),
            relief: 0.5,
            roughness: 0.75,
            roughness_mod: 0.15,
            metalness: 0.0,
            metalness_mod: 0.0,
            ao: 0.5,
            // Unlit by default: emission is opt-in, so an ordinary surface writes a
            // black map and the shader's unconditional add contributes nothing.
            emissive: [1.0, 0.55, 0.15],
            emissive_strength: 0.0,
            emissive_band: 0.7,
        }
    }
}

impl OutputStage {
    /// Roughness at a texel whose field value is `h`.
    pub fn roughness_at(&self, h: f32) -> f32 {
        (self.roughness + (h - 0.5) * 2.0 * self.roughness_mod).clamp(0.0, 1.0)
    }

    /// Metalness at a texel whose field value is `h`.
    pub fn metalness_at(&self, h: f32) -> f32 {
        (self.metalness + (h - 0.5) * 2.0 * self.metalness_mod).clamp(0.0, 1.0)
    }

    /// How strongly a texel at field value `h` emits, `0..=1`.
    ///
    /// Ramps from the band edge to the top of the field rather than switching, so
    /// a glow fades into the surface instead of stamping a hard-edged shape — the
    /// difference between lava cooling into rock and a decal.
    pub fn emissive_at(&self, h: f32) -> f32 {
        if self.emissive_strength <= 0.0 {
            return 0.0;
        }
        let band = self.emissive_band.clamp(0.0, 0.999);
        let t = ((h - band) / (1.0 - band)).clamp(0.0, 1.0);
        t * self.emissive_strength.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_clamps_at_the_ends_and_interpolates_between() {
        let ramp = ColorRamp {
            stops: vec![
                RampStop {
                    at: 0.0,
                    color: [0.0, 0.0, 0.0],
                },
                RampStop {
                    at: 1.0,
                    color: [1.0, 1.0, 1.0],
                },
            ],
        };
        assert_eq!(ramp.sample(-1.0), [0.0, 0.0, 0.0]);
        assert_eq!(ramp.sample(2.0), [1.0, 1.0, 1.0]);
        let mid = ramp.sample(0.5);
        assert!((mid[0] - 0.5).abs() < 1e-6, "midpoint {mid:?}");
    }

    /// A hand-edited recipe with stops out of order should still gradient the way
    /// the author obviously meant.
    #[test]
    fn ramp_tolerates_unsorted_stops() {
        let sorted = ColorRamp {
            stops: vec![
                RampStop {
                    at: 0.0,
                    color: [0.0, 0.0, 0.0],
                },
                RampStop {
                    at: 1.0,
                    color: [1.0, 1.0, 1.0],
                },
            ],
        };
        let jumbled = ColorRamp {
            stops: sorted.stops.iter().rev().copied().collect(),
        };
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert_eq!(sorted.sample(t), jumbled.sample(t), "at {t}");
        }
    }

    /// Degenerate ramps must not panic or divide by zero — both are reachable
    /// from an edited file.
    #[test]
    fn degenerate_ramps_are_survivable() {
        assert_eq!(ColorRamp { stops: vec![] }.sample(0.5), [0.5, 0.5, 0.5]);
        let coincident = ColorRamp {
            stops: vec![
                RampStop {
                    at: 0.5,
                    color: [1.0, 0.0, 0.0],
                },
                RampStop {
                    at: 0.5,
                    color: [0.0, 0.0, 1.0],
                },
            ],
        };
        let v = coincident.sample(0.5);
        assert!(v.iter().all(|c| c.is_finite()), "{v:?}");
    }

    /// The modulation is signed and pivots on the midpoint — negative
    /// `roughness_mod` must make the crests shinier, not merely different.
    #[test]
    fn modulation_pivots_on_the_midpoint_and_clamps() {
        let out = OutputStage {
            roughness: 0.5,
            roughness_mod: 0.25,
            ..OutputStage::default()
        };
        assert!((out.roughness_at(0.5) - 0.5).abs() < 1e-6);
        assert!(out.roughness_at(1.0) > out.roughness_at(0.0));

        let polished = OutputStage {
            roughness_mod: -0.25,
            ..out.clone()
        };
        assert!(polished.roughness_at(1.0) < polished.roughness_at(0.0));

        let extreme = OutputStage {
            roughness: 0.9,
            roughness_mod: 5.0,
            ..OutputStage::default()
        };
        assert_eq!(extreme.roughness_at(1.0), 1.0);
        assert_eq!(extreme.roughness_at(0.0), 0.0);
    }

    /// The colour knobs' round trip: `recolor` sweeps the ramp onto one hue while
    /// keeping each stop's brightness, and `tint` reads that hue/saturation back —
    /// so the bench's knob echoes exactly what it just set instead of drifting
    /// under the hand.
    #[test]
    fn recolor_sets_a_readable_tint_and_keeps_the_value_profile() {
        let mut ramp = ColorRamp::default(); // the neutral stone sweep
        let values: Vec<f32> = ramp.stops.iter().map(|s| hsv_of(s.color).2).collect();

        ramp.recolor(0.13, 0.8); // gold
        let (h, s) = ramp.tint();
        assert!((h - 0.13).abs() < 1e-3, "hue reads back what recolor set: {h}");
        assert!((s - 0.8).abs() < 1e-3, "saturation reads back: {s}");

        for (st, v0) in ramp.stops.iter().zip(&values) {
            // Only the chroma moved — the dark→light shape is untouched.
            assert!((hsv_of(st.color).2 - v0).abs() < 1e-6, "brightness preserved");
            // Gold: red and green sit over blue at every stop.
            assert!(
                st.color[0] > st.color[2] && st.color[1] > st.color[2],
                "not a warm hue: {:?}",
                st.color
            );
        }
    }

    /// A fully greyscale ramp has no hue to report — `tint` says so rather than
    /// inventing an angle, so the knob rests at zero.
    #[test]
    fn a_grey_ramp_reports_no_tint() {
        let grey = ColorRamp {
            stops: vec![
                RampStop {
                    at: 0.0,
                    color: [0.1, 0.1, 0.1],
                },
                RampStop {
                    at: 1.0,
                    color: [0.8, 0.8, 0.8],
                },
            ],
        };
        let (h, s) = grey.tint();
        assert!(h.abs() < 1e-6 && s.abs() < 1e-6, "grey has a tint: h{h} s{s}");
    }
}

#[cfg(test)]
mod emissive_tests {
    use super::*;

    /// Emission is OPT-IN. A default output stage must emit nothing, because the
    /// shader adds the map unconditionally — a non-zero default would make every
    /// material in the game self-lit.
    #[test]
    fn emission_is_off_by_default() {
        let out = OutputStage::default();
        for h in [0.0, 0.5, 0.9, 1.0] {
            assert_eq!(out.emissive_at(h), 0.0, "unlit default emits at {h}");
        }
    }

    /// The band selects WHERE it glows and ramps rather than switching — a hard
    /// edge reads as a decal stuck on the surface, not as light coming out of it.
    #[test]
    fn the_band_ramps_from_its_edge_upward() {
        let out = OutputStage {
            emissive_strength: 1.0,
            emissive_band: 0.6,
            ..Default::default()
        };
        assert_eq!(out.emissive_at(0.0), 0.0, "below the band must be dark");
        assert_eq!(
            out.emissive_at(0.6),
            0.0,
            "the band edge is where it starts"
        );
        assert!(
            out.emissive_at(0.8) > 0.0 && out.emissive_at(0.8) < 1.0,
            "mid-band ramps"
        );
        assert!(
            (out.emissive_at(1.0) - 1.0).abs() < 1e-6,
            "the crest is full strength"
        );
        // Monotonic: brighter as the field rises.
        assert!(out.emissive_at(0.9) > out.emissive_at(0.7));
    }

    /// A degenerate band must not divide by zero — it is reachable from an edited
    /// recipe.
    #[test]
    fn a_degenerate_band_is_survivable() {
        let out = OutputStage {
            emissive_strength: 1.0,
            emissive_band: 1.0,
            ..Default::default()
        };
        assert!(out.emissive_at(1.0).is_finite());
        let wild = OutputStage {
            emissive_strength: 9.0,
            emissive_band: -3.0,
            ..Default::default()
        };
        assert!((0.0..=1.0).contains(&wild.emissive_at(0.5)));
    }
}
