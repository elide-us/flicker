//! Runtime dynamic cloth — garment regions whose vertices swing on jiggle chains.
//!
//! The rigid skin ([`crate::skin::skin`]) places every garment vertex by its body bone.
//! For a dangly region (a bell sleeve, a skirt hem) that reads as stiff. Here each such
//! vertex is additionally displaced by the SWING of a [`JiggleChain`] hung from the
//! region's anchor bone: the chain lags the bone's motion under gravity, and the vertex
//! rides the chain's deviation from its rigidly-carried rest shape.
//!
//! ## Static drape + swing
//! The reference ("no-gravity") polyline is each chain's STRAIGHT hang along its rest
//! direction, carried into the posed frame by the anchor bone's palette matrix. The
//! displacement applied to a vertex is `dynamic − reference` at its bind point — so it
//! captures BOTH the static gravity sag (the chain hangs below its straight rest even while
//! the body is still) and the motion swing (the chain lags when the bone moves). A dangly
//! region therefore drapes downward at rest and swings when it moves. Only bound verts are
//! touched, so the rest of the garment keeps its rigid skin exactly, and a vert bound at a
//! chain's anchor (segment 0) never moves — the attachment stays put.
//!
//! Reuses [`JiggleChain`] verbatim (the necklace is its other user); this module is only
//! the per-region binding + per-frame apply.

use glam::{Mat4, Quat, Vec3};

use crate::format::{Bone, Cloth};
use crate::jiggle::{JiggleChain, JiggleParams};
use crate::skin::SkinnedVertex;

/// Settle passes when snapshotting a chain's bind-space rest shape at build. A short chain
/// settles well within this; it is a one-time cost per chain at load.
const SETTLE_STEPS: usize = 240;

struct Region {
    anchor_bone: usize,
    chains: Vec<JiggleChain>,
    /// Per-chain bind-space anchor point (`positions()[0]` at rest).
    anchor_bind: Vec<Vec3>,
    /// Per-chain bind-space STRAIGHT rest polyline (the "no-gravity" reference), carried
    /// into the posed frame each update by the anchor bone's palette matrix. `dynamic −
    /// reference` is therefore the gravity sag + motion swing, so the region drapes at rest.
    rest: Vec<Vec<Vec3>>,
    /// `(vertex index, chain index, segment k, fraction f along segment k)`.
    binds: Vec<(usize, usize, usize, f32)>,
}

/// The dynamic-cloth state for one garment: a set of regions, each a fan of jiggle chains
/// plus the vertices bound to them. Built once from the garment's [`Cloth`] metadata;
/// stepped and applied every frame over the rigidly-skinned vertices.
pub struct ClothSim {
    regions: Vec<Region>,
}

impl ClothSim {
    /// Build from a garment mesh's `cloth` metadata + the base skeleton. Regions whose
    /// anchor bone is absent from `bones` are skipped. The live chain is settled to gravity
    /// equilibrium here so the first frame starts already draped (no startup pop).
    pub fn build(cloth: &Cloth, bones: &[Bone]) -> Self {
        let mut regions = Vec::new();
        for r in &cloth.regions {
            let Some(anchor_bone) = bones.iter().position(|b| b.name == r.anchor_bone) else {
                eprintln!(
                    "flicker-skeletal: cloth region '{}' anchor bone '{}' not in skeleton; skipped",
                    r.name, r.anchor_bone
                );
                continue;
            };
            let params = JiggleParams {
                gravity: Vec3::from(r.params.gravity),
                stiffness: r.params.stiffness,
                damping: r.params.damping,
                iterations: r.params.iterations,
                max_dt: r.params.max_dt,
            };
            let mut chains = Vec::new();
            let mut anchor_bind = Vec::new();
            let mut rest = Vec::new();
            for c in &r.chains {
                let a = Vec3::from(c.anchor);
                let dir = Vec3::from(c.dir);
                let mut chain = JiggleChain::new(a, dir, c.seg_len, c.segments as usize, params);
                // Settle the LIVE chain so its first frame starts at gravity equilibrium
                // (draped, no startup pop). The reference below is the STRAIGHT no-gravity
                // rest — NOT this settled shape — so the displacement includes the static sag.
                for _ in 0..SETTLE_STEPS {
                    chain.step(a, Quat::IDENTITY, 1.0 / 60.0);
                }
                let straight = (0..=c.segments).map(|i| a + dir * (c.seg_len * i as f32)).collect();
                rest.push(straight);
                anchor_bind.push(a);
                chains.push(chain);
            }
            // Drop binds that point at a chain we didn't build (defensive against bad data).
            let binds = r
                .binds
                .iter()
                .filter(|b| (b.c as usize) < chains.len())
                .map(|b| (b.v as usize, b.c as usize, b.k as usize, b.f))
                .collect();
            regions.push(Region { anchor_bone, chains, anchor_bind, rest, binds });
        }
        Self { regions }
    }

    /// True when there is nothing to simulate (no regions, or none with bound verts).
    pub fn is_empty(&self) -> bool {
        self.regions.iter().all(|r| r.binds.is_empty())
    }

    /// Step every chain from the current pose and displace each bound vertex in place.
    /// `palette` is the skinning palette (`palette[b] = global[b] * inverse_bind[b]`);
    /// `skinned` is the rigidly-skinned vertex buffer to modify. Only bound verts are
    /// touched, so non-cloth verts keep their rigid skin exactly.
    pub fn update(&mut self, palette: &[Mat4], dt: f32, skinned: &mut [SkinnedVertex]) {
        for r in &mut self.regions {
            let Some(pa) = palette.get(r.anchor_bone).copied() else { continue };
            let driver_rot = Quat::from_mat4(&pa).normalize();
            // Step each chain from its posed anchor; cache the posed dynamic (D) polyline
            // and the posed rest reference (RE = pa · bind-rest) polyline per chain.
            let mut dyn_pts: Vec<Vec<Vec3>> = Vec::with_capacity(r.chains.len());
            let mut ref_pts: Vec<Vec<Vec3>> = Vec::with_capacity(r.chains.len());
            for (ci, chain) in r.chains.iter_mut().enumerate() {
                let posed_anchor = pa.transform_point3(r.anchor_bind[ci]);
                chain.step(posed_anchor, driver_rot, dt);
                dyn_pts.push(chain.positions().to_vec());
                ref_pts.push(r.rest[ci].iter().map(|p| pa.transform_point3(*p)).collect());
            }
            for &(v, c, k, f) in &r.binds {
                let (d, re) = (&dyn_pts[c], &ref_pts[c]);
                if k + 1 >= d.len() {
                    continue;
                }
                let disp = d[k].lerp(d[k + 1], f) - re[k].lerp(re[k + 1], f);
                if let Some(sv) = skinned.get_mut(v) {
                    sv.position = (Vec3::from(sv.position) + disp).to_array();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{ClothBind, ClothChain, ClothParams, ClothRegion};

    fn bone(name: &str) -> Bone {
        Bone { name: name.into(), parent: -1, local: Mat4::IDENTITY, inverse_bind: Mat4::IDENTITY }
    }
    fn sv(p: [f32; 3]) -> SkinnedVertex {
        SkinnedVertex { position: p, normal: [0.0, 1.0, 0.0] }
    }

    fn one_region_sim() -> ClothSim {
        // A HORIZONTAL chain (+x) so gravity visibly sags it off its straight rest.
        let cloth = Cloth {
            regions: vec![ClothRegion {
                name: "r".into(),
                anchor_bone: "a".into(),
                params: ClothParams { gravity: [0.0, 0.0, -600.0], stiffness: 0.02, damping: 0.9, iterations: 8, max_dt: 1.0 / 30.0 },
                chains: vec![ClothChain { anchor: [0.0, 0.0, 0.0], dir: [1.0, 0.0, 0.0], seg_len: 5.0, segments: 4 }],
                binds: vec![
                    ClothBind { v: 0, c: 0, k: 0, f: 0.0 }, // at the anchor → never displaced
                    ClothBind { v: 1, c: 0, k: 2, f: 0.5 },
                    ClothBind { v: 2, c: 0, k: 3, f: 1.0 }, // the tip → most sag/swing
                ],
            }],
        };
        ClothSim::build(&cloth, &[bone("a")])
    }

    /// Drive the sim `frames` times, re-applying the rigid skin (`orig`) each frame exactly
    /// as the render loop does — `update` displaces a FRESH rigid buffer, it must not
    /// accumulate across frames.
    fn drive(sim: &mut ClothSim, palette: &[Mat4], orig: &[[f32; 3]], frames: usize) -> Vec<SkinnedVertex> {
        let mut skinned: Vec<SkinnedVertex> = orig.iter().map(|p| sv(*p)).collect();
        for _ in 0..frames {
            for (s, o) in skinned.iter_mut().zip(orig) {
                s.position = *o;
            }
            sim.update(palette, 1.0 / 60.0, &mut skinned);
        }
        skinned
    }

    /// Even at the bind pose the region must DRAPE: the tip vertex sags downward (−z) under
    /// gravity off its straight rest, while the anchor-bound vertex stays pinned.
    #[test]
    fn rest_pose_drapes_downward() {
        let mut sim = one_region_sim();
        let orig = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
        let out = drive(&mut sim, &[Mat4::IDENTITY], &orig, 200);
        assert!(
            (Vec3::from(out[0].position) - Vec3::from(orig[0])).length() < 1e-2,
            "the anchor-bound vert must stay pinned"
        );
        let tip_dz = out[2].position[2] - orig[2][2];
        assert!(tip_dz < -1.0, "the tip must sag downward under gravity at rest (dz {tip_dz})");
        assert!(out.iter().all(|s| Vec3::from(s.position).is_finite()), "cloth must stay finite");
    }

    /// Moving the anchor bone adds swing on top of the drape: the tip displaces, the
    /// anchor-bound vert stays pinned, everything stays bounded.
    #[test]
    fn posed_anchor_swings_the_tip() {
        let mut sim = one_region_sim();
        let orig = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
        let out = drive(&mut sim, &[Mat4::from_translation(Vec3::new(40.0, 0.0, 0.0))], &orig, 30);
        let anchor_off = (Vec3::from(out[0].position) - Vec3::from(orig[0])).length();
        assert!(anchor_off < 1e-3, "the anchor-bound vert must not move (off {anchor_off})");
        let tip_moved = (Vec3::from(out[2].position) - Vec3::from(orig[2])).length();
        assert!(tip_moved > 1.0, "the anchor move must displace the tip (moved {tip_moved})");
        assert!(
            out.iter().all(|s| Vec3::from(s.position).is_finite() && Vec3::from(s.position).length() < 1e4),
            "cloth must stay bounded"
        );
    }

    /// The tool's JSON (mesh.cloth) must round-trip through the serde types and build a sim
    /// — guards the wire-format field names against the Rust structs.
    #[test]
    fn parses_tool_json_and_builds() {
        let json = r#"{
          "vertices":[
            {"p":[0,0,0],"n":[0,1,0],"joints":[0,0,0,0],"weights":[1,0,0,0]},
            {"p":[1,0,-20],"n":[0,1,0],"joints":[0,0,0,0],"weights":[1,0,0,0]}
          ],
          "cloth":{"regions":[{
            "name":"sleeve_l","anchor_bone":"a",
            "params":{"gravity":[0,0,-500],"stiffness":0.25,"damping":0.9,"iterations":8,"max_dt":0.033},
            "chains":[{"anchor":[0,0,0],"dir":[0,0,-1],"seg_len":5,"segments":4}],
            "binds":[{"v":1,"c":0,"k":3,"f":1.0}]
          }]}
        }"#;
        let mesh: crate::format::Mesh = serde_json::from_str(json).expect("parse mesh+cloth");
        assert_eq!(mesh.cloth.regions.len(), 1);
        assert_eq!(mesh.cloth.regions[0].binds.len(), 1);
        let sim = ClothSim::build(&mesh.cloth, &[bone("a")]);
        assert!(!sim.is_empty(), "the bound vert must produce a non-empty sim");
    }
}
