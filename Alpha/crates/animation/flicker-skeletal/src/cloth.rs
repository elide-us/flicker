//! Runtime dynamic cloth — garment regions whose vertices FOLLOW jiggle chains.
//!
//! The rigid skin places a dangly region (a bell sleeve, a skirt hem) by its body bone,
//! which reads as stiff — and merely *adding* a swing on top reads as jello (the modelled
//! shape, quivering). Instead each cloth vertex is POSITIONED by a [`JiggleChain`] hung from
//! the region's anchor bone: it rides a point along the chain plus its own fixed offset from
//! that chain (the tube cross-section / flare), rotated to follow the chain's bend. The
//! chain hangs under gravity and lags the bone, so the whole region DRAPES downward out of
//! its modelled pose and swings — it collapses into a hanging shape instead of holding the
//! pose and wobbling.
//!
//! Reuses [`JiggleChain`] verbatim (the necklace is its other user); this module is the
//! per-vertex binding + per-frame placement over the rigidly-skinned buffer (only bound
//! verts are overwritten, so the rest of the garment keeps its rigid skin exactly).

use glam::{Mat4, Quat, Vec3};

use crate::format::{Bone, Cloth, Vertex};
use crate::jiggle::{JiggleChain, JiggleParams};
use crate::skin::SkinnedVertex;

/// Settle passes when relaxing a chain to its bind gravity-hang at build (one-time).
const SETTLE_STEPS: usize = 240;

struct Bind {
    v: usize,
    chain: usize,
    k: usize,
    f: f32,
    /// Vertex offset from its chain point, in BIND space — the tube cross-section / flare.
    /// Rotated by the segment's bend + the bone twist each frame so the tube is preserved.
    offset: Vec3,
    /// Bind-space vertex normal, rotated the same way so lighting follows the drape.
    normal: Vec3,
}

struct Region {
    anchor_bone: usize,
    chains: Vec<JiggleChain>,
    /// Per chain: the chain's bind-space anchor point (`positions()[0]` at rest).
    anchor_bind: Vec<Vec3>,
    /// Per chain: the straight rest direction (bind space) — the "no-bend" reference the
    /// per-segment bend rotation is measured against each frame.
    rest_dir: Vec<Vec3>,
    binds: Vec<Bind>,
}

/// The dynamic-cloth state for one garment: regions of jiggle chains + the vertices that
/// ride them. Built once from the `cloth` metadata + the bind-pose mesh; stepped and applied
/// every frame.
pub struct ClothSim {
    regions: Vec<Region>,
}

impl ClothSim {
    /// Build from a garment's `cloth` metadata, its bind-pose `verts` (each bound vertex's
    /// rest position + normal → its offset from the chain), and the base skeleton. Regions
    /// whose anchor bone is missing from `bones` are skipped. Chains are settled to their
    /// bind gravity-hang here so the first frame starts already draped (no startup pop).
    pub fn build(cloth: &Cloth, verts: &[Vertex], bones: &[Bone]) -> Self {
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
            let mut rest_dir = Vec::new();
            for c in &r.chains {
                let a = Vec3::from(c.anchor);
                let dir = Vec3::from(c.dir).normalize_or_zero();
                let mut chain = JiggleChain::new(a, dir, c.seg_len, c.segments as usize, params);
                for _ in 0..SETTLE_STEPS {
                    chain.step(a, Quat::IDENTITY, 1.0 / 60.0);
                }
                chains.push(chain);
                anchor_bind.push(a);
                rest_dir.push(dir);
            }
            let binds = r
                .binds
                .iter()
                .filter_map(|b| {
                    let c = r.chains.get(b.c as usize)?;
                    let a = Vec3::from(c.anchor);
                    let dir = Vec3::from(c.dir).normalize_or_zero();
                    let base_rest = a + dir * (c.seg_len * (b.k as f32 + b.f));
                    let vert = verts.get(b.v as usize)?;
                    Some(Bind {
                        v: b.v as usize,
                        chain: b.c as usize,
                        k: b.k as usize,
                        f: b.f,
                        offset: Vec3::from(vert.p) - base_rest,
                        normal: Vec3::from(vert.n),
                    })
                })
                .collect();
            regions.push(Region {
                anchor_bone,
                chains,
                anchor_bind,
                rest_dir,
                binds,
            });
        }
        Self { regions }
    }

    /// True when there is nothing to simulate.
    pub fn is_empty(&self) -> bool {
        self.regions.iter().all(|r| r.binds.is_empty())
    }

    /// Step every chain from the current pose and PLACE each bound vertex on its chain.
    /// `palette[b] = global[b] * inverse_bind[b]`; `skinned` is overwritten in place for
    /// bound verts only, so non-cloth verts keep their rigid skin.
    pub fn update(&mut self, palette: &[Mat4], dt: f32, skinned: &mut [SkinnedVertex]) {
        for r in &mut self.regions {
            let Some(pa) = palette.get(r.anchor_bone).copied() else {
                continue;
            };
            let driver_rot = Quat::from_mat4(&pa).normalize();
            // Step each chain from its posed anchor; cache its posed points and the per-segment
            // bend rotation (rest direction → current direction, both in posed space).
            let mut dyn_pts: Vec<Vec<Vec3>> = Vec::with_capacity(r.chains.len());
            let mut seg_rot: Vec<Vec<Quat>> = Vec::with_capacity(r.chains.len());
            for (ci, chain) in r.chains.iter_mut().enumerate() {
                let posed_anchor = pa.transform_point3(r.anchor_bind[ci]);
                chain.step(posed_anchor, driver_rot, dt);
                let d = chain.positions().to_vec();
                let rest_dir_posed = (driver_rot * r.rest_dir[ci]).normalize_or_zero();
                let mut qs = Vec::with_capacity(d.len().saturating_sub(1));
                for k in 0..d.len().saturating_sub(1) {
                    let dd = (d[k + 1] - d[k]).normalize_or_zero();
                    qs.push(if dd.length_squared() > 0.5 {
                        Quat::from_rotation_arc(rest_dir_posed, dd)
                    } else {
                        Quat::IDENTITY
                    });
                }
                dyn_pts.push(d);
                seg_rot.push(qs);
            }
            for b in &r.binds {
                let d = &dyn_pts[b.chain];
                if b.k + 1 >= d.len() {
                    continue;
                }
                let base = d[b.k].lerp(d[b.k + 1], b.f);
                let frame = seg_rot[b.chain][b.k] * driver_rot;
                if let Some(sv) = skinned.get_mut(b.v) {
                    sv.position = (base + frame * b.offset).to_array();
                    sv.normal = (frame * b.normal).normalize_or_zero().to_array();
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
        Bone {
            name: name.into(),
            parent: -1,
            local: Mat4::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        }
    }
    fn sv(p: [f32; 3]) -> SkinnedVertex {
        SkinnedVertex {
            position: p,
            normal: [0.0, 1.0, 0.0],
        }
    }
    fn vtx(p: [f32; 3]) -> Vertex {
        Vertex {
            p,
            n: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }

    // A HORIZONTAL chain (+x) with 3 verts sitting ON it, so gravity drapes them off the
    // straight rest. Limp so gravity clearly wins.
    fn one_region_sim() -> (ClothSim, Vec<[f32; 3]>) {
        let orig = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
        let cloth = Cloth {
            regions: vec![ClothRegion {
                name: "r".into(),
                anchor_bone: "a".into(),
                params: ClothParams {
                    gravity: [0.0, 0.0, -600.0],
                    stiffness: 0.005,
                    damping: 0.9,
                    iterations: 8,
                    max_dt: 1.0 / 30.0,
                },
                chains: vec![ClothChain {
                    anchor: [0.0, 0.0, 0.0],
                    dir: [1.0, 0.0, 0.0],
                    seg_len: 5.0,
                    segments: 4,
                }],
                binds: vec![
                    ClothBind {
                        v: 0,
                        c: 0,
                        k: 0,
                        f: 0.0,
                    }, // at the anchor
                    ClothBind {
                        v: 1,
                        c: 0,
                        k: 2,
                        f: 0.0,
                    },
                    ClothBind {
                        v: 2,
                        c: 0,
                        k: 3,
                        f: 1.0,
                    }, // the free tip
                ],
            }],
        };
        let verts: Vec<Vertex> = orig.iter().map(|p| vtx(*p)).collect();
        (ClothSim::build(&cloth, &verts, &[bone("a")]), orig)
    }

    fn drive(
        sim: &mut ClothSim,
        palette: &[Mat4],
        orig: &[[f32; 3]],
        frames: usize,
    ) -> Vec<SkinnedVertex> {
        let mut skinned: Vec<SkinnedVertex> = orig.iter().map(|p| sv(*p)).collect();
        for _ in 0..frames {
            sim.update(palette, 1.0 / 60.0, &mut skinned);
        }
        skinned
    }

    /// The whole region DRAPES off its modelled shape: at the bind pose the tip vertex falls
    /// well below its rest height under gravity, while the anchor-bound vertex stays put.
    #[test]
    fn region_drapes_off_the_rest_shape() {
        let (mut sim, orig) = one_region_sim();
        let out = drive(&mut sim, &[Mat4::IDENTITY], &orig, 300);
        assert!(
            Vec3::from(out[0].position).length() < 1e-2,
            "the anchor-bound vert stays at the anchor"
        );
        assert!(
            out[2].position[2] < -5.0,
            "the tip must drape well below its rest height (z {})",
            out[2].position[2]
        );
        assert!(
            out.iter().all(|s| Vec3::from(s.position).is_finite()),
            "cloth must stay finite"
        );
    }

    /// Moving the anchor bone carries the whole hang with it; the anchor-bound vert tracks
    /// the bone exactly, and everything stays bounded.
    #[test]
    fn anchor_move_carries_the_hang() {
        let (mut sim, orig) = one_region_sim();
        let out = drive(
            &mut sim,
            &[Mat4::from_translation(Vec3::new(40.0, 0.0, 0.0))],
            &orig,
            40,
        );
        assert!(
            (Vec3::from(out[0].position) - Vec3::new(40.0, 0.0, 0.0)).length() < 1e-2,
            "the anchor-bound vert must track the bone (got {:?})",
            out[0].position
        );
        assert!(
            out.iter().all(
                |s| Vec3::from(s.position).is_finite() && Vec3::from(s.position).length() < 1e4
            ),
            "cloth must stay bounded"
        );
    }

    /// The tool's JSON (mesh.cloth) round-trips through the serde types and builds a sim.
    #[test]
    fn parses_tool_json_and_builds() {
        let json = r#"{
          "vertices":[
            {"p":[0,0,0],"n":[0,1,0],"joints":[0,0,0,0],"weights":[1,0,0,0]},
            {"p":[1,0,-20],"n":[0,1,0],"joints":[0,0,0,0],"weights":[1,0,0,0]}
          ],
          "cloth":{"regions":[{
            "name":"sleeve_l","anchor_bone":"a",
            "params":{"gravity":[0,0,-500],"stiffness":0.06,"damping":0.9,"iterations":8,"max_dt":0.033},
            "chains":[{"anchor":[0,0,0],"dir":[0,0,-1],"seg_len":5,"segments":4}],
            "binds":[{"v":1,"c":0,"k":3,"f":1.0}]
          }]}
        }"#;
        let mesh: crate::format::Mesh = serde_json::from_str(json).expect("parse mesh+cloth");
        assert_eq!(mesh.cloth.regions.len(), 1);
        let sim = ClothSim::build(&mesh.cloth, &mesh.vertices, &[bone("a")]);
        assert!(
            !sim.is_empty(),
            "the bound vert must produce a non-empty sim"
        );
    }
}
