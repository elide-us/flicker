//! THE ISLAND AS A SURFACE — G3 of the gait generator (37704D6B; Aaron's ruling CD36B9BE: the
//! branch-walking test bed is THIS scene, the island heightfield plus a dead winter tree). What
//! a foot lands on here, as the generator's [`SurfaceQuery`]: the island's height function
//! composed with the tree's branches ([`island_support`]).
//!
//! The generator works in CENTIMETRES with +Z UP; the voxel world is Y-up at
//! [`CM_PER_VOXEL`] per voxel. The ONE conversion between the two is the engine's
//! (`flicker_voxel::frame`: [`gait_to_world`] / [`world_to_gait`]) — the grass scatter's
//! scale, the tree's draw matrix and the support's frame all come off it, so a creature
//! walking the island is drawn exactly where its feet were cast.

use std::f32::consts::FRAC_PI_2;

use glam::{Mat4, Vec3};

use flicker_mechanics::gait::{Composite, MeshSupport};
use flicker_mechanics::{HeightFn, SupportKind};
use flicker_primitive::heightmap::island_height;
use flicker_skeletal::format::Mesh;

/// The generator ↔ world bridge is the ENGINE'S (`flicker_voxel::frame`, off
/// `clayengine::FEET_PER_VOXEL`); this scene only stands its tree and its island in it.
pub use flicker_voxel::frame::{gait_to_world, world_to_gait, CM_PER_VOXEL};

/// The dead winter tree stood on the island: the packaged prop under
/// `package/props/environment/<TREE_PROP>/` (baked from `Environment/CommonTree_Dead_1.fbx`
/// through `import_prop` + `promote_asset`, real centimetres, Z up, its base at the origin).
pub const TREE_PROP: &str = "CommonTree_Dead_1";

/// Where the tree stands — voxel X, Z on the plateau ahead of the boot camera (the island's
/// dome is centred on 384, 384; the camera boots there facing +Z).
pub const TREE_STAND: [f32; 2] = [420.0, 480.0];

/// The tree's turn about its trunk, radians — its canopy (which spreads along its own Y)
/// turned across the boot view.
pub const TREE_YAW: f32 = FRAC_PI_2;

/// The island's surface in the generator's frame: its height (cm) over the plane point
/// `(x, y)` — [`island_height`] sampled at the voxel column that point maps to.
pub fn island_height_cm(x: f32, y: f32) -> f32 {
    island_height(x / CM_PER_VOXEL, -y / CM_PER_VOXEL) * CM_PER_VOXEL
}

/// The island as ground: the generator's height-function support over [`island_height_cm`].
pub fn island_ground() -> HeightFn<fn(f32, f32) -> f32> {
    HeightFn {
        height: island_height_cm,
        step: CM_PER_VOXEL * 0.5,
    }
}

/// The tree's base in the generator's frame: on the island's surface at [`TREE_STAND`].
pub fn tree_base() -> Vec3 {
    let [x, z] = TREE_STAND;
    world_to_gait().transform_point3(Vec3::new(x, island_height(x, z), z))
}

/// The prop's placement in the generator's frame: stood on its base, turned by [`TREE_YAW`].
pub fn tree_placement() -> Mat4 {
    Mat4::from_translation(tree_base()) * Mat4::from_rotation_z(TREE_YAW)
}

/// The world model the scene draws the prop with — the same stand, in voxels, Y up.
pub fn tree_model() -> Mat4 {
    gait_to_world() * tree_placement()
}

/// The packaged tree mesh (real centimetres, Z up, as baked), or `None` — with a warning —
/// when the prop is not promoted in this tree: the island then stands bare.
pub fn tree_mesh() -> Option<Mesh> {
    let path = flicker_content::roots()
        .package()
        .join("props/environment")
        .join(TREE_PROP)
        .join(format!("{TREE_PROP}.json"));
    match flicker_skeletal::format::load_mesh(&path) {
        Ok(mesh) => Some(mesh),
        Err(e) => {
            tracing::warn!(
                "the dead tree {TREE_PROP} is not packaged at {} — the island stands bare: {e:#}",
                path.display()
            );
            None
        }
    }
}

/// The tree's faces as a support, stood on the island in the generator's frame. One kind for
/// the whole prop: it is a dead tree of bare BRANCHES (Aaron), trunk included.
pub fn tree_support(mesh: &Mesh) -> MeshSupport {
    MeshSupport::new(
        mesh.vertices.iter().map(|v| Vec3::from(v.p)).collect(),
        mesh.indices.clone(),
        SupportKind::Branch,
    )
    .transformed(tree_placement())
}

/// THE ISLAND AS A SURFACE: the heightfield, and the tree's branches when the prop is packaged
/// — the [`SurfaceQuery`] a creature walking this scene plants its feet on.
///
/// [`SurfaceQuery`]: flicker_mechanics::SurfaceQuery
pub fn island_support() -> Composite {
    let world = Composite::new().with(island_ground());
    match tree_mesh() {
        Some(mesh) => world.with(tree_support(&mesh)),
        None => world,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_mechanics::gait::CapsuleSupport;
    use flicker_mechanics::{GaitKind, Locomotion, LocomotionFamily, SurfaceQuery};

    fn close(a: Mat4, b: Mat4) -> bool {
        a.to_cols_array()
            .iter()
            .zip(b.to_cols_array())
            .all(|(p, q)| (p - q).abs() < 1e-4)
    }

    /// **GATE — the tree stands on the island exactly where it is drawn.** The support's
    /// placement carried into the world IS the draw matrix, and that matrix is the props'
    /// convention (the grass's: translate to the ground · turn about Y · Z-up → Y-up · cm →
    /// voxels); the two frame conversions invert each other; the base sits on the island's
    /// surface, on the dome (higher than the seabed out at the field's edge); and the ground
    /// support answers Ground at the base with an upward normal.
    #[test]
    fn the_tree_stands_on_the_island_where_it_is_drawn() {
        let [x, z] = TREE_STAND;
        let ground = island_height(x, z);
        let grass_style = Mat4::from_translation(Vec3::new(x, ground, z))
            * Mat4::from_rotation_y(TREE_YAW)
            * Mat4::from_rotation_x(-FRAC_PI_2)
            * Mat4::from_scale(Vec3::splat(1.0 / CM_PER_VOXEL));
        assert!(
            close(tree_model(), grass_style),
            "the draw matrix is the props' convention:\n{:?}\n{:?}",
            tree_model(),
            grass_style
        );
        assert!(close(gait_to_world() * world_to_gait(), Mat4::IDENTITY));
        let base_world = tree_model().transform_point3(Vec3::ZERO);
        assert!(
            (base_world - Vec3::new(x, ground, z)).length() < 1e-3,
            "the prop's origin lands on the ground at the stand: {base_world}"
        );
        let base = tree_base();
        assert!(
            (gait_to_world().transform_point3(base) - base_world).length() < 1e-3,
            "the gait-frame base is the same point"
        );
        assert!(
            (island_height_cm(base.x, base.y) - base.z).abs() < 1e-3,
            "the base sits on the surface: {} vs {}",
            island_height_cm(base.x, base.y),
            base.z
        );
        assert!(
            ground > island_height(x + 300.0, z) + 10.0,
            "the stand is on the dome, not the seabed"
        );
        let c = island_ground()
            .support(base, Vec3::Z, 30.0)
            .expect("ground under the tree");
        assert_eq!(c.kind, SupportKind::Ground);
        assert!((c.point - base).length() < 1e-3, "{c:?}");
        assert!(c.normal.z > 0.9, "the plateau is nearly level: {c:?}");
        // A cast a metre up the tree's trunk line finds no ground within reach.
        assert!(island_ground()
            .support(base + Vec3::Z * 100.0, Vec3::Z, 30.0)
            .is_none());
    }

    /// **GATE — the island's support answers Ground away from the tree and Branch on it**, on
    /// the REAL packaged prop (skips, with a note, when it is not promoted in this tree): three
    /// metres off the stand the ground at the island's height; over the largest up-facing face a
    /// metre or more above the base, a Branch contact at that face (or whatever lies above it),
    /// facing up, well clear of the ground.
    #[test]
    fn the_island_support_answers_ground_off_the_tree_and_branch_on_it() {
        let Some(mesh) = tree_mesh() else {
            eprintln!("skip: {TREE_PROP} is not packaged in this tree");
            return;
        };
        let world = island_support();
        let base = tree_base();
        let off = base + Vec3::new(300.0, 0.0, 0.0);
        let near = Vec3::new(off.x, off.y, island_height_cm(off.x, off.y));
        let g = world
            .support(near, Vec3::Z, 30.0)
            .expect("ground off the tree");
        assert_eq!(g.kind, SupportKind::Ground);
        assert!((g.point - near).length() < 1e-3, "{g:?}");

        let tree = tree_support(&mesh);
        let (top, _) = tree
            .triangles()
            .filter_map(|[a, b, c]| {
                let n = (b - a).cross(c - a);
                let centroid = (a + b + c) / 3.0;
                (n.normalize_or_zero().z > 0.7 && centroid.z > base.z + 100.0)
                    .then_some((centroid, n.length()))
            })
            .max_by(|p, q| p.1.total_cmp(&q.1))
            .expect("the canopy has an up-facing face a metre up");
        let c = world
            .support(top, Vec3::Z, 20.0)
            .expect("a branch under the foot");
        assert_eq!(c.kind, SupportKind::Branch, "{c:?}");
        assert!(
            c.point.z >= top.z - 1e-3,
            "that face or one above it: {c:?}"
        );
        assert!((c.point.truncate() - top.truncate()).length() < 1e-3);
        assert!(c.normal.z > 0.5, "facing up: {c:?}");
        assert!(
            c.point.z > island_height_cm(c.point.x, c.point.y) + 100.0,
            "a metre or more above the ground"
        );
    }

    /// A synthetic hoofed quadruped at rest, as the generator's own tests build it: root on
    /// the ground, the trunk 60 cm long at 56 cm, four three-joint legs ending in hooves on the
    /// ground (stifles forward, elbows back) — every length times `scale` (0.35 is a raccoon).
    fn quadruped(scale: f32) -> (Vec<&'static str>, Vec<i32>, Vec<Mat4>) {
        let mut names = Vec::new();
        let mut parents = Vec::new();
        let mut rest = Vec::new();
        let mut push = |n: &'static str, p: i32, at: Vec3| -> i32 {
            names.push(n);
            parents.push(p);
            rest.push(Mat4::from_translation(at * scale));
            rest.len() as i32 - 1
        };
        push("root", -1, Vec3::ZERO);
        push("pelvis", 0, Vec3::new(0.0, 30.0, 56.0));
        push("spine_03", 1, Vec3::new(0.0, -30.0, 58.0));
        for (side, sign) in [("l", 1.0), ("r", -1.0)] {
            let x = 9.0 * sign;
            let names_h: [&'static str; 5] = match side {
                "l" => ["thigh_l", "calf_l", "foot_l", "ball_l", "hoof_l"],
                _ => ["thigh_r", "calf_r", "foot_r", "ball_r", "hoof_r"],
            };
            let mut i = push(names_h[0], 1, Vec3::new(x, 30.0, 53.0));
            i = push(names_h[1], i, Vec3::new(x, 18.0, 36.0));
            i = push(names_h[2], i, Vec3::new(x, 36.0, 18.0));
            i = push(names_h[3], i, Vec3::new(x, 33.0, 6.0));
            push(names_h[4], i, Vec3::new(x, 33.0, 0.0));
            let names_f: [&'static str; 5] = match side {
                "l" => [
                    "upperarm_l",
                    "lowerarm_l",
                    "hand_l",
                    "foredigit_l",
                    "forehoof_l",
                ],
                _ => [
                    "upperarm_r",
                    "lowerarm_r",
                    "hand_r",
                    "foredigit_r",
                    "forehoof_r",
                ],
            };
            let mut j = push(names_f[0], 2, Vec3::new(x, -34.0, 48.0));
            j = push(names_f[1], j, Vec3::new(x, -22.0, 30.0));
            j = push(names_f[2], j, Vec3::new(x, -34.0, 12.0));
            j = push(names_f[3], j, Vec3::new(x, -35.0, 6.0));
            push(names_f[4], j, Vec3::new(x, -35.0, 0.0));
        }
        (names, parents, rest)
    }

    /// **GATE — a climber walks a branch with four feet on it and clings.** The quadruped walks
    /// as a Climber along a horizontal capsule BRANCH (radius 16 cm, its top at the ground
    /// height), its body 4 cm off the branch's axis so its left feet stand on the flank and its
    /// right feet near the crest. Over three seconds: the gait is Climb; every landing is a
    /// Branch contact ON the capsule's surface (a radius from the axis); all four feet are
    /// planted together at some tick; and the body's up follows its stance (cling = 1): with
    /// three or more feet down it stands perpendicular to the plane through their contacts,
    /// rolled toward the flank side — never merely the world's up.
    #[test]
    fn a_climber_walks_a_branch_with_four_feet_on_it_and_clings() {
        const RADIUS: f32 = 16.0;
        let branch = CapsuleSupport {
            a: Vec3::new(0.0, 300.0, -RADIUS),
            b: Vec3::new(0.0, -900.0, -RADIUS),
            radius: RADIUS,
            kind: SupportKind::Branch,
        };
        let (names, parents, rest) = quadruped(1.0);
        let mut climber = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Climber);
        climber.position = Vec3::new(4.0, 0.0, 0.0);
        let mut globals = rest.clone();
        let dt = 1.0 / 60.0;
        let mut all_four = false;
        let mut landings = 0;
        let mut rolled = false;
        for tick in 0..180 {
            let stance: Vec<Vec3> = climber
                .feet
                .iter()
                .filter_map(|f| f.contact().map(|c| c.point))
                .collect();
            let frame = climber.step(&mut globals, &parents, &rest, -Vec3::Y, 30.0, &branch, dt);
            assert_eq!(climber.gait, GaitKind::Climb);
            if stance.len() >= 3 {
                let mean = stance.iter().copied().sum::<Vec3>() / stance.len() as f32;
                for c in &stance {
                    assert!(
                        (*c - mean).dot(frame.up).abs() < 1.5,
                        "the body's up stands off the plane of its feet at tick {tick}: up {} \
                         contact {c} mean {mean}",
                        frame.up
                    );
                }
                assert!(frame.up.z > 0.7, "upright on a branch: {}", frame.up);
                if frame.up.x > 0.1 {
                    rolled = true;
                }
            }
            let planted = climber
                .feet
                .iter()
                .filter(|f| f.contact().is_some())
                .count();
            all_four |= planted == 4;
            for foot in &climber.feet {
                if let Some(c) = foot.contact() {
                    assert_eq!(c.kind, SupportKind::Branch, "tick {tick}: {c:?}");
                    let off_axis = (c.point.x.powi(2) + (c.point.z + RADIUS).powi(2)).sqrt();
                    assert!(
                        (off_axis - RADIUS).abs() < 1e-2,
                        "on the capsule's surface at tick {tick}: {c:?}"
                    );
                    assert!(
                        (c.normal - Vec3::new(c.point.x, 0.0, c.point.z + RADIUS) / RADIUS)
                            .length()
                            < 1e-3,
                        "the normal points out of the branch: {c:?}"
                    );
                    landings += 1;
                }
            }
        }
        assert!(all_four, "all four feet held the branch together");
        assert!(landings > 0);
        assert!(
            rolled,
            "the body rolled toward the flank its left feet stand on: {}",
            climber.frame.up
        );
        assert!(
            climber.frame.origin.y < -80.0,
            "the body walked the branch (30 cm/s for 3 s): {}",
            climber.frame.origin
        );
    }

    /// **GATE — a raccoon-sized climber walks a branch of the REAL tree** (skips, with a note,
    /// when the prop is not packaged). A near-level run of branch top — found by casting a grid
    /// down through the canopy: ~24 cm long, 8–22 cm wide, ~125 cm up, in the prop's own frame
    /// from (−9.6, 46.8, 134.1) heading (0.96, −0.28) — is turned to run along −Y from the
    /// origin, and the 0.35-scale quadruped walks it as a Climber for three seconds at 8 cm/s.
    /// Every foot that plants, plants on a BRANCH contact of the tree; all four feet hold the
    /// branch together at some tick; after the first cycle the climber never has fewer than two
    /// feet on it; and the body advances the length of the run. (The low-poly branch's facets
    /// tilt the feet every which way — the roll the body takes from them is the generator's
    /// tuning, not asserted here; the summary line reports it.)
    #[test]
    fn a_climber_walks_a_branch_of_the_real_tree() {
        let Some(mesh) = tree_mesh() else {
            eprintln!("skip: {TREE_PROP} is not packaged in this tree");
            return;
        };
        let start = Vec3::new(-9.6, 46.8, 134.1);
        let along = Vec3::new(0.96, -0.28, 0.0).normalize();
        let turn = glam::Quat::from_rotation_arc(along, -Vec3::Y);
        let placement = Mat4::from_translation(-(turn * start)) * Mat4::from_quat(turn);
        let branch = MeshSupport::new(
            mesh.vertices.iter().map(|v| Vec3::from(v.p)).collect(),
            mesh.indices.clone(),
            SupportKind::Branch,
        )
        .transformed(placement);
        let top = branch
            .support(Vec3::ZERO, Vec3::Z, 10.0)
            .expect("the branch top is at the origin");
        assert!(top.point.z.abs() < 1.0 && top.normal.z > 0.7, "{top:?}");

        let (names, parents, rest) = quadruped(0.35);
        let mut climber = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Climber);
        let mut globals = rest.clone();
        let dt = 1.0 / 60.0;
        let mut four = 0;
        let mut fewest_after_a_cycle = usize::MAX;
        let mut blind = 0;
        let mut roll = 0.0f32;
        for tick in 0..180 {
            let frame = climber.step(&mut globals, &parents, &rest, -Vec3::Y, 8.0, &branch, dt);
            assert_eq!(climber.gait, GaitKind::Climb);
            let mut planted = 0;
            for foot in &climber.feet {
                match foot.state {
                    flicker_mechanics::FootState::Stance(c) => {
                        assert_eq!(c.kind, SupportKind::Branch, "tick {tick}: {c:?}");
                        planted += 1;
                    }
                    flicker_mechanics::FootState::Swing { to: None, .. } => blind += 1,
                    _ => {}
                }
            }
            if planted == 4 {
                four += 1;
            }
            if tick >= 100 {
                fewest_after_a_cycle = fewest_after_a_cycle.min(planted);
            }
            roll = roll.max(frame.up.truncate().length());
        }
        eprintln!(
            "the real branch: four feet down {four}/180 ticks, fewest after a cycle \
             {fewest_after_a_cycle}, blind swings {blind}, steepest body tilt {:.0}°, worst \
             residual {:.2} cm",
            roll.asin().to_degrees(),
            climber.residual
        );
        assert!(four > 0, "all four feet held the branch together");
        assert!(
            fewest_after_a_cycle >= 2,
            "never fewer than two feet on the branch: {fewest_after_a_cycle}"
        );
        assert!(
            climber.frame.origin.y < -20.0,
            "the body walked the run: {}",
            climber.frame.origin
        );
    }
}
