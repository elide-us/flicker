//! The jiggle soft-body simulation — a 2D **pressurized soft body** per ball.
//!
//! No 2D rigid/soft-body solver existed in the engine (flicker-mechanics is the 3D
//! collision-GEOMETRY layer; `drop.rs` is a 1-axis settle), so this is new dynamics —
//! but not a parallel reimplementation of an existing system: it reuses the mechanics
//! contact query [`flicker_mechanics::penetration`] for coarse ball-vs-ball overlap +
//! merge, and mirrors the engine's own position-based verlet idiom (flicker-skeletal
//! `jiggle.rs`: verlet step + Gauss-Seidel distance-constraint relaxation).
//!
//! Each ball is a ring of point-masses joined by distance constraints (the membrane),
//! plus an internal gas-pressure constraint that drives the enclosed area toward a rest
//! target — so it holds a round shape but yields, squishes, and jiggles. The membrane
//! resolution is ADAPTIVE: segment LENGTH is held ~constant, so a bigger ball gets more
//! segments (smoother, and the squish granularity stays even). Screen space: origin
//! top-left, **y-down, gravity +y**.

use flicker_mechanics::{penetration, Shape};
use glam::{Vec2, Vec3};

const GRAVITY: f32 = 1300.0; // px/s² — lower = floatier, softer landings
const DAMP: f32 = 0.990; // verlet velocity retention — higher = more lingering jiggle
const ITERS: usize = 10; // Gauss-Seidel constraint passes per step (a floppy skin needs more)
const MEMBRANE_K: f32 = 0.36; // edge stiffness — LOW = a floppy, soppy-sponge skin (VERY squishy)
const PRESS_K: f32 = 0.5; // area (pressure) restore — holds volume so it wobbles, not collapses
const STATIC_EPS: f32 = 1.0; // tangential speed (px/step) below which a contact GRIPS fully
const GENERAL_GRIP: f32 = 0.34; // global friction boost — lower = stickier everything
const WALL_GRIP: f32 = 0.30; // extra grip on the walls (resist sliding down the slope)
/// Anti-collapse spoke: a node that has caved IN past this fraction of the rest radius is
/// pushed back out so the membrane can't invert. It NEVER pulls an outward-BULGING node
/// back toward r0 — that one-sidedness is the whole point: pressure alone holds volume, so
/// a loaded bubble stays squished (flat under weight, wide at the sides) instead of
/// snapping back to a circle. Raising SPOKE_FLOOR limits how far a bubble can be crushed.
const SPOKE_FLOOR: f32 = 0.34; // a heavy ball can crush a light one to ~1/3 height before this bites
const SPOKE_K: f32 = 0.5;
/// Cohesion floor: EVERY bubble is at least this sticky, so even the "slippery" tiers cling
/// a little and the field reads as sticky suds, not loose balls. Per-tier `stick` adds more.
const BASE_STICK: f32 = 0.35;
/// Same-tier balls merge when their slightly-padded circles touch — the squish holds
/// resting balls at ~contact distance, so CONTACT, not deep overlap, is the trigger.
const MERGE_PAD: f32 = 1.05;

/// Bucket top half-width as a fraction of screen width. Tune here for a wider/tighter
/// tray; the ball sizes scale off the bucket, so the play ratio holds at any size.
pub const BUCKET_TOP_FRAC: f32 = 0.10;
/// Bucket height as a multiple of its top width — 0.75 = a 4:3 trapezoid (wide top).
const BUCKET_ASPECT: f32 = 0.75;

/// Target membrane segment length (px). The node count per ball is chosen so segments
/// stay ~this long, so bigger balls get more (smoother) segments.
const SEG_LEN: f32 = 11.0;
const MIN_SEG: usize = 12;
const MAX_SEG: usize = 48;

/// Node count for a ball of circumradius `r` so each segment is ~[`SEG_LEN`] px.
pub fn seg_count(r: f32) -> usize {
    ((std::f32::consts::TAU * r / SEG_LEN).round() as usize).clamp(MIN_SEG, MAX_SEG)
}

/// Per-tier physical personality (sticky / bouncy / slippery / rolly) + payout.
#[derive(Clone, Copy)]
pub struct Tier {
    pub color: [f32; 4],
    pub rest: f32,  // restitution (bounciness)
    pub fric: f32,  // contact friction: 0 = frictionless (slippery), ~0.7 = grips (sticky)
    pub stick: f32, // ball-to-ball cohesion (stickiness) — things cling to it
    pub dens: f32,  // areal density → mass (heavier squishes lighter)
    pub press: f32, // rest-area inflation (firmness)
    pub score: u32, // points a merge into this tier pays (× combo)
}

/// One membrane point: current + previous position (velocity is implicit, verlet).
#[derive(Clone, Copy)]
pub struct Node {
    pub pos: Vec2,
    pub prev: Vec2,
}

/// A pressurized soft-body ball at a merge tier.
pub struct Ball {
    pub id: u32,
    pub tier: usize,
    pub nodes: Vec<Node>,
    pub r0: f32,       // rest circumradius
    pub area0: f32,    // rest area the pressure constraint targets (pressure folded in)
    pub inv_mass: f32, // per-node inverse mass (heavier tier → smaller → resists)
    pub reff: f32,     // effective radius from live area (for contact + merge + draw)
    pub rest: f32,
    pub fric: f32,
    pub stick: f32,
    pub held: bool,
    /// Latched once the ball clears a top lip — it's leaving the bucket, so it stops
    /// colliding with walls/floor and free-falls off the screen (the failure).
    pub escaped: bool,
    /// Sim-seconds of cooldown after being CREATED by a merge, during which it will not
    /// merge again — so a cascade advances one tier at a time with a beat between,
    /// instead of small+small→medium+medium→large collapsing in a single step.
    pub merge_cd: f32,
    /// Set true once this ball has touched a wall, the floor, or another ball. The drop
    /// gate uses it: the next ball can't launch until the last dropped one has landed.
    pub contacted: bool,
    /// Combo lineage: `chain` is the cascade depth (0 = a settled ball), kept "hot" for
    /// `chain_ttl` sim-seconds after forming. Once it cools, chain resets to 0, so only a
    /// genuine chain reaction (products merging before they cool) climbs the combo.
    pub chain: u32,
    pub chain_ttl: f32,
}

impl Ball {
    /// A fresh round ball of `tier` centered at `c` with circumradius `r`.
    pub fn new(id: u32, tier: usize, t: &Tier, c: Vec2, r: f32) -> Self {
        let n = seg_count(r);
        let mut nodes = Vec::with_capacity(n);
        for i in 0..n {
            let a = (i as f32 / n as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            let p = c + Vec2::new(a.cos(), a.sin()) * r;
            nodes.push(Node { pos: p, prev: p });
        }
        let area0 = 0.5 * n as f32 * r * r * (std::f32::consts::TAU / n as f32).sin() * t.press;
        Ball {
            id,
            tier,
            nodes,
            r0: r,
            area0,
            inv_mass: 1.0 / ((r * r * t.dens) / n as f32),
            reff: r,
            rest: t.rest,
            fric: t.fric,
            stick: t.stick,
            held: false,
            escaped: false,
            merge_cd: 0.0,
            contacted: false,
            chain: 0,
            chain_ttl: 0.0,
        }
    }

    pub fn n(&self) -> usize {
        self.nodes.len()
    }

    pub fn centroid(&self) -> Vec2 {
        let s: Vec2 = self.nodes.iter().fold(Vec2::ZERO, |a, node| a + node.pos);
        s / self.nodes.len() as f32
    }

    /// Signed polygon area (shoelace), always positive here.
    pub fn area(&self) -> f32 {
        let n = self.nodes.len();
        let mut a = 0.0;
        for i in 0..n {
            let p = self.nodes[i].pos;
            let q = self.nodes[(i + 1) % n].pos;
            a += p.x * q.y - q.x * p.y;
        }
        (a * 0.5).abs()
    }

    /// Shrink toward the centroid (the new-tier unlock zoom-out for headroom).
    pub fn shrink(&mut self, f: f32) {
        let c = self.centroid();
        for node in &mut self.nodes {
            node.pos = c + (node.pos - c) * f;
            node.prev = c + (node.prev - c) * f;
        }
        self.r0 *= f;
        self.area0 *= f * f;
    }

    /// Compress the membrane toward its centroid WITHOUT changing the rest size — the
    /// pressure constraint then re-inflates it over a few frames. Used for the merge
    /// morph-in (a new ball swells into place instead of snapping). Zeroes velocity so
    /// it swells cleanly rather than launching.
    pub fn compress(&mut self, f: f32) {
        let c = self.centroid();
        for node in &mut self.nodes {
            node.pos = c + (node.pos - c) * f;
            node.prev = node.pos;
        }
    }
}

/// The 4:3 trapezoid bucket: flat floor + two straight slanted-outward walls (`\___/`).
pub struct Bucket {
    pub rim_y: f32,
    pub floor_y: f32,
    pub tl: Vec2,
    pub tr: Vec2,
    pub bl: Vec2,
    pub br: Vec2,
    pub left: (Vec2, Vec2),
    pub right: (Vec2, Vec2),
}

impl Bucket {
    /// Build a compact, centered 4:3 trapezoid from the screen size and a taper.
    pub fn new(screen: Vec2, taper: f32) -> Self {
        let cx = screen.x * 0.5;
        let top_half = (screen.x * BUCKET_TOP_FRAC).max(48.0);
        let bot_half = top_half * taper;
        let height = top_half * 2.0 * BUCKET_ASPECT; // 4:3 → height = 0.75 × top width
        let floor_y = screen.y * 0.86;
        let rim_y = (floor_y - height).max(96.0);
        let tl = Vec2::new(cx - top_half, rim_y);
        let bl = Vec2::new(cx - bot_half, floor_y);
        let tr = Vec2::new(cx + top_half, rim_y);
        let br = Vec2::new(cx + bot_half, floor_y);
        Bucket {
            rim_y,
            floor_y,
            tl,
            tr,
            bl,
            br,
            left: (tl, inward_normal(tl, bl, 1.0)),
            right: (tr, inward_normal(tr, br, -1.0)),
        }
    }

    pub fn rail_span(&self) -> (f32, f32) {
        (self.left.0.x, self.right.0.x)
    }

    /// Top (mouth) width — the balls scale off this so the play ratio is size-agnostic.
    pub fn top_width(&self) -> f32 {
        self.tr.x - self.tl.x
    }

    /// Is this centroid still contained — below the rim BETWEEN the (interpolated) walls,
    /// or above the open mouth within its width (+2px tolerance)? A ball can only leave
    /// by clearing a lip, so this never false-flags a ball that is genuinely inside
    /// (its centroid sits a full radius clear of the wall it rests against).
    pub fn contains(&self, c: Vec2) -> bool {
        if c.y < self.rim_y {
            c.x >= self.tl.x - 2.0 && c.x <= self.tr.x + 2.0
        } else if c.y <= self.floor_y {
            let t = (c.y - self.rim_y) / (self.floor_y - self.rim_y).max(1e-3);
            let lx = self.tl.x + (self.bl.x - self.tl.x) * t;
            let rx = self.tr.x + (self.br.x - self.tr.x) * t;
            c.x >= lx - 2.0 && c.x <= rx + 2.0
        } else {
            false // below the floor — fell through (should not happen for a contained ball)
        }
    }
}

/// Inward unit normal of the wall segment a→b, oriented so its x-sign matches `inward_x`.
fn inward_normal(a: Vec2, b: Vec2, inward_x: f32) -> Vec2 {
    let d = b - a;
    let mut n = Vec2::new(-d.y, d.x).normalize_or_zero();
    if n.x.signum() != inward_x.signum() {
        n = -n;
    }
    n
}

/// Advance the whole field one fixed step: integrate, relax constraints + walls, then
/// resolve mutual ball squish. Merges are detected separately by [`find_merges`].
pub fn simulate(balls: &mut [Ball], bucket: &Bucket, dt: f32, top_tier: usize) {
    for b in balls.iter_mut() {
        b.merge_cd = (b.merge_cd - dt).max(0.0);
        if b.chain_ttl > 0.0 {
            b.chain_ttl -= dt;
            if b.chain_ttl <= 0.0 {
                b.chain = 0; // cooled off — no longer part of a live cascade
            }
        }
        integrate(b, dt);
    }
    for _ in 0..ITERS {
        for b in balls.iter_mut() {
            constrain(b);
            collide_walls(b, bucket);
        }
    }
    for b in balls.iter_mut() {
        b.reff = (b.area() / std::f32::consts::PI).sqrt().max(2.0);
    }
    let n = balls.len();
    for i in 0..n {
        for j in (i + 1)..n {
            let (lo, hi) = balls.split_at_mut(j);
            ball_squish(&mut lo[i], &mut hi[0], top_tier);
        }
    }
}

fn integrate(b: &mut Ball, dt: f32) {
    if b.held {
        return;
    }
    let g = GRAVITY * dt * dt;
    let cap = b.r0 * 0.8; // max displacement per step — an anti-explosion clamp
    for node in &mut b.nodes {
        let mut vel = (node.pos - node.prev) * DAMP;
        if vel.length() > cap {
            vel = vel.normalize_or_zero() * cap;
        }
        node.prev = node.pos;
        node.pos += vel + Vec2::new(0.0, g);
    }
}

fn constrain(b: &mut Ball) {
    // Internal constraints must only RESHAPE, never translate the body (else the
    // Gauss-Seidel sweep + discrete pressure push drift it sideways — balls "roll" at
    // rest, dropped balls veer). Snapshot COM, restore it after; only gravity/contacts
    // (integrate/walls/squish) move a body.
    let com0 = b.centroid();
    let n = b.nodes.len();
    let l0 = 2.0 * b.r0 * (std::f32::consts::PI / n as f32).sin();
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, c) = (b.nodes[i].pos, b.nodes[j].pos);
        let d = c - a;
        let len = d.length().max(1e-6);
        let corr = d * ((len - l0) / len * 0.5 * MEMBRANE_K);
        b.nodes[i].pos += corr;
        b.nodes[j].pos -= corr;
    }
    // Anti-collapse spoke — ONE-SIDED: only shove a caved-in node back out (never pull a
    // bulging one in). This is what stops the bubble snapping back to a circle: pressure
    // (below) is the only thing restoring volume, so under load it squishes and STAYS
    // squished, and unloaded it eases round again on its own.
    let ct = b.centroid();
    let floor = b.r0 * SPOKE_FLOOR;
    for node in &mut b.nodes {
        let d = node.pos - ct;
        let len = d.length().max(1e-6);
        if len < floor {
            node.pos += d / len * ((floor - len) * SPOKE_K);
        }
    }
    let target = b.area0;
    let err = target - b.area();
    if err.abs() > 1e-3 {
        let ct = b.centroid();
        let push = (err / target.max(1.0)) * PRESS_K * b.r0 * 0.5;
        for node in &mut b.nodes {
            let d = (node.pos - ct).normalize_or_zero();
            node.pos += d * push;
        }
    }
    let shift = com0 - b.centroid();
    for node in &mut b.nodes {
        node.pos += shift;
    }
}

fn collide_walls(b: &mut Ball, bucket: &Bucket) {
    if b.held {
        return;
    }
    // Escape latch: once the centroid clears a top lip (only possible above the rim,
    // where the mouth is open), the ball is leaving — free-fall it out of the bucket and
    // off the screen. That, NOT rising above the rim, is the failure.
    let c = b.centroid();
    if !bucket.contains(c) {
        b.escaped = true;
    }
    if b.escaped {
        return;
    }
    let rest = b.rest;
    let base = ((1.0 - b.fric) * GENERAL_GRIP).clamp(0.0, 1.0);
    let wall_slip = base * WALL_GRIP; // walls grip harder than the floor
    let mut hit = false;
    for node in &mut b.nodes {
        // Floor — only within the bucket footprint, so an escaped ball finds no floor.
        if node.pos.y > bucket.floor_y
            && node.pos.x > bucket.bl.x - 4.0
            && node.pos.x < bucket.br.x + 4.0
        {
            node.pos.y = bucket.floor_y;
            reflect(node, Vec2::new(0.0, -1.0), rest, base);
            hit = true;
        }
        // Slanted walls, below the rim only — the top is open, so balls pile above it.
        if node.pos.y > bucket.rim_y - 4.0 {
            for (p0, nrm) in [bucket.left, bucket.right] {
                let s = (node.pos - p0).dot(nrm);
                if s < 0.0 {
                    node.pos -= nrm * s;
                    reflect(node, nrm, rest, wall_slip);
                    hit = true;
                }
            }
        }
    }
    if hit {
        b.contacted = true;
    }
}

/// Restitution + friction on a node against a surface (verlet velocity = pos − prev).
/// `slip` is tangential retention: 1.0 slides freely (slippery), low grips (sticky). A
/// slow tangential slide (< [`STATIC_EPS`]) is STATIC friction — it stops dead, so balls
/// settle and stop rebalancing and cling to walls instead of creeping down them; faster
/// slides keep `slip` of their speed (kinetic friction). Only ever called at a contact,
/// so free-fall is untouched.
fn reflect(node: &mut Node, nrm: Vec2, rest: f32, slip: f32) {
    let v = node.pos - node.prev;
    let vn = v.dot(nrm);
    let mut vt = v - nrm * vn;
    if vt.length() < STATIC_EPS {
        vt = Vec2::ZERO;
    } else {
        vt *= slip;
    }
    let nv = vt - nrm * (vn * rest);
    node.prev = node.pos - nv;
}

/// Mutual squish + viscosity between two balls: nodes pushed out of the other's
/// effective circle (split by inverse mass) with each ball's OWN restitution/friction
/// (so a slippery ball slides off a sticky one), plus cohesion from the stickier ball.
fn ball_squish(a: &mut Ball, b: &mut Ball, top_tier: usize) {
    // Same-tier MERGEABLE balls must not spring apart — they are destined to merge, so
    // let them overlap and coalesce (the merge resolves them) instead of the squish
    // flinging a fresh product out of the cup. Top-tier same-tier balls don't merge, so
    // they collide normally.
    if a.tier == b.tier && a.tier < top_tier {
        return;
    }
    let (ca, cb) = (a.centroid(), b.centroid());
    if (cb - ca).length() > (a.reff + b.reff) * 1.4 {
        return;
    }
    let wsum = a.inv_mass + b.inv_mass;
    let (wa, wb) = (a.inv_mass / wsum, b.inv_mass / wsum);
    let cohesion = a.stick.max(b.stick).max(BASE_STICK);
    contact(a, cb, b.reff, wa, cohesion);
    contact(b, ca, a.reff, wb, cohesion);
}

fn contact(ball: &mut Ball, other_c: Vec2, other_r: f32, wshare: f32, cohesion: f32) {
    if ball.held {
        return;
    }
    // Ball-to-ball restitution is softened (×0.5) vs. walls — a merge's swell shoves
    // neighbours far less springily.
    let (rest, slip) = (
        ball.rest * 0.5,
        ((1.0 - ball.fric) * GENERAL_GRIP).clamp(0.0, 1.0),
    );
    let reach = ball.reff * 0.6; // cohesion range beyond touching
    let mut hit = false;
    for node in &mut ball.nodes {
        let d = node.pos - other_c;
        let dist = d.length().max(1e-6);
        let dir = d / dist;
        let pen = other_r - dist;
        if pen > 0.0 {
            node.pos += dir * (pen * (0.5 + 0.5 * wshare));
            reflect(node, dir, rest, slip);
            hit = true;
        } else if cohesion > 0.0 && dist < other_r + reach {
            // Near but not touching: the sticky body pulls this node toward it.
            let pull = cohesion * 0.05 * (1.0 - (dist - other_r) / reach).clamp(0.0, 1.0);
            node.pos -= dir * pull;
        }
    }
    if hit {
        ball.contacted = true;
    }
}

/// Same-tier pairs overlapping past the padded-contact threshold — reuses the mechanics
/// contact query on each ball's effective circle. Returns stable id pairs.
pub fn find_merges(balls: &[Ball]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for i in 0..balls.len() {
        for j in (i + 1)..balls.len() {
            let (a, b) = (&balls[i], &balls[j]);
            if a.tier != b.tier || a.held || b.held || a.merge_cd > 0.0 || b.merge_cd > 0.0 {
                continue;
            }
            let sa = Shape::Sphere {
                center: Vec3::new(a.centroid().x, a.centroid().y, 0.0),
                radius: a.reff * MERGE_PAD,
            };
            let sb = Shape::Sphere {
                center: Vec3::new(b.centroid().x, b.centroid().y, 0.0),
                radius: b.reff * MERGE_PAD,
            };
            if penetration(&sa, &sb).is_some() {
                out.push((a.id, b.id));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier() -> Tier {
        Tier {
            color: [1.0, 0.4, 0.4, 1.0],
            rest: 0.3,
            fric: 0.1,
            stick: 0.0,
            dens: 1.0,
            press: 1.1,
            score: 10,
        }
    }

    #[test]
    fn segment_count_grows_with_radius_but_stays_bounded() {
        assert!(
            seg_count(10.0) < seg_count(60.0),
            "bigger ball → more segments"
        );
        assert!(seg_count(1.0) >= MIN_SEG);
        assert!(seg_count(10_000.0) <= MAX_SEG);
    }

    #[test]
    fn a_ball_falls_and_rests_inside_the_bucket_without_blowing_up() {
        let screen = Vec2::new(560.0, 760.0);
        let bucket = Bucket::new(screen, 0.62);
        let t = tier();
        let mut balls = vec![Ball::new(1, 0, &t, Vec2::new(280.0, 200.0), 22.0)];
        for _ in 0..1200 {
            simulate(&mut balls, &bucket, 1.0 / 120.0, 9);
        }
        let c = balls[0].centroid();
        assert!(c.is_finite(), "no NaN/inf after settling");
        assert!(
            c.y < bucket.floor_y + 1.0 && c.y > bucket.rim_y,
            "ball settles inside the bucket, got y={}",
            c.y
        );
        assert!(
            balls[0].area() > 0.4 * balls[0].area0,
            "the membrane does not collapse under its own rest"
        );
    }

    #[test]
    fn a_dropped_ball_falls_straight_down_no_sideways_drift() {
        let screen = Vec2::new(560.0, 760.0);
        let bucket = Bucket::new(screen, 0.62);
        let t = tier();
        let x0 = 280.0; // bucket center — any solver drift shows as x moving off it
        let mut balls = vec![Ball::new(1, 0, &t, Vec2::new(x0, 150.0), 22.0)];
        for _ in 0..90 {
            simulate(&mut balls, &bucket, 1.0 / 120.0, 9);
        }
        let c = balls[0].centroid();
        assert!(c.y > 160.0, "it actually fell");
        assert!(
            (c.x - x0).abs() < 0.5,
            "no sideways drift in free fall: x moved from {x0} to {}",
            c.x
        );
    }

    #[test]
    fn two_same_tier_balls_in_contact_are_flagged_to_merge() {
        let t = tier();
        let balls = vec![
            Ball::new(1, 0, &t, Vec2::new(276.0, 300.0), 24.0),
            Ball::new(2, 0, &t, Vec2::new(300.0, 300.0), 24.0), // overlapping
        ];
        assert_eq!(find_merges(&balls), vec![(1, 2)]);
    }

    #[test]
    fn containment_holds_inside_and_releases_only_over_a_lip() {
        let screen = Vec2::new(560.0, 760.0);
        let bucket = Bucket::new(screen, 0.62);
        let cx = 280.0;
        let mid_y = (bucket.rim_y + bucket.floor_y) * 0.5;
        // Well inside the body, and piled above the open mouth within its width: contained.
        assert!(bucket.contains(Vec2::new(cx, mid_y)));
        assert!(bucket.contains(Vec2::new(cx, bucket.rim_y - 40.0)));
        // Over a lip (above the rim, past a corner) or leaked outside a wall: NOT contained.
        assert!(!bucket.contains(Vec2::new(bucket.tr.x + 30.0, bucket.rim_y - 40.0)));
        assert!(!bucket.contains(Vec2::new(bucket.tl.x - 30.0, mid_y)));
    }

    #[test]
    fn different_tiers_never_merge() {
        let t = tier();
        let balls = vec![
            Ball::new(1, 0, &t, Vec2::new(276.0, 300.0), 24.0),
            Ball::new(2, 1, &t, Vec2::new(300.0, 300.0), 24.0),
        ];
        assert!(find_merges(&balls).is_empty());
    }
}
