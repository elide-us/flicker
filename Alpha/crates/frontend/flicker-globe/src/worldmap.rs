//! **The WORLD MAP as one shared component** — a flat, scrollable map view drawn
//! into a `surface` node's reserved slot, with the CONTENT plugged in.
//!
//! [`WorldMap`] owns everything a map VIEW is: the seat, the camera (a centre +
//! visible span over a 2-D map plane), the zoom clamps (out = the whole picture,
//! in = the content's own minimum), the horizontal wrap, the pointer gestures
//! (drag pans, wheel zooms at the cursor, a press-without-drag picks), the
//! Look/Zoom signal channel, the rebake policy and the offscreen target
//! (reusing [`GlobeView`]'s pass + composite plumbing — one target lifecycle).
//!
//! What the map SHOWS is a [`MapContent`] — the plug. [`HexSphereMap`] is the
//! first: the icosahedral hex sphere cut on a seam and laid out flat. Later
//! contents (a region's heightmap, a space map) implement the same trait and
//! the component never changes — the split Aaron asked for: *"don't write the
//! component for one specific purpose, write it to allow for easily pluggable
//! map-like displays."*
//!
//! # The two framings of the sphere content
//!
//! Fully zoomed out the hex sphere shows as the classic UNWRAPPED map — an
//! equirectangular band, the pole caps trimmed (a beach ball cut on a meridian
//! and laid flat). Zoomed in past the regional threshold it re-projects LOCALLY
//! about the view centre (azimuthal equidistant, north up), so panning walks the
//! sphere seamlessly — neighbours stay attached across the cut seam and over the
//! poles, exactly as they do on the ball itself.

use std::collections::HashMap;

use flicker::render::{
    Camera, FrameGraph, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Rect, Renderer,
    StageDef, StageInputs, StageLayer, TextureHandle, TexturedMeshHandle, TexturedVertex, Vec3,
};
use flicker::ui::{SurfacePointer, SurfaceSlot};
use glam::{Mat4, Vec2};

use crate::view::{Arrows, GlobeView, Seat};
use crate::{direct_emissive, graticule, RADIUS};

/// One baked mesh, CPU-side, waiting for a renderer.
type BakedMesh = (Vec<MeshVertex>, Vec<u32>, MeshDrawOptions);

/// Stick pan speed: fraction of the visible span crossed per second at full deflection.
const LOOK_PAN_RATE: f32 = 0.9;
/// Stick zoom speed: exponential span change per second at full deflection.
const ZOOM_SIG_RATE: f32 = 1.2;
/// Wheel zoom: exponential span change per tick.
const WHEEL_ZOOM: f32 = 0.12;
/// A press that travelled no further than this many pixels is a PICK, not a drag.
const CLICK_SLOP_PX: f32 = 6.0;
/// Local-mode bake margin: the baked disc's radius as a multiple of the visible
/// half-diagonal, so a pan has room before the next rebake.
const LOCAL_MARGIN: f32 = 1.7;
/// Rebake when the centre has drifted this fraction of the shorter span axis.
const REBAKE_DRIFT: f32 = 0.3;
/// Draw depths inside the map pass: backdrop under the cells, lines over them.
const BACKDROP_Z: f32 = -0.5;
/// The pictures lie over the cells and under the lines.
const PICTURE_Z: f32 = 0.25;
const LINE_Z: f32 = 0.5;
/// The picked cell's outline draws over the graticule; its second loop is
/// pulled this share toward the cell's centre.
const PICK_Z: f32 = 0.6;
const PICK_WEIGHT: f32 = 0.04;

/// The unwrapped picture's size and topology, in map-plane units.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MapExtent {
    pub w: f32,
    pub h: f32,
    /// The left and right edges are the SAME cut — panning wraps (a sphere's
    /// longitude does; a finite heightmap's does not).
    pub wrap_x: bool,
    /// The smallest visible span (height, plane units) the view may zoom in to.
    pub min_span: f32,
}

/// Which projection the content draws — the whole unwrapped picture, or a local
/// re-projection about the view centre.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MapMode {
    Atlas,
    Local,
}

/// One frame's view of the map plane: where the camera looks, how much it sees,
/// and which projection serves it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MapFrame {
    /// Centre of the view, map-plane units.
    pub center: Vec2,
    /// Visible extent (width, height), map-plane units.
    pub span: Vec2,
    pub mode: MapMode,
}

/// **A picture a content lays on its plane** — a textured polygon in map-plane
/// coordinates with UVs into `texture`: a hex's 2K heightmap drawn IN THE
/// WORLD (V5, Aaron 2026-10-16: *"when I click on a tile, the heightmap is
/// drawn in the world at detail"*). The content projects the polygon like any
/// cell; the component uploads and draws it over the cells, under the lines.
pub struct MapPicture {
    pub verts: Vec<TexturedVertex>,
    pub idx: Vec<u32>,
    pub texture: TextureHandle,
}

/// What one [`MapContent::bake`] hands back: triangle meshes (draw order = list
/// order), the pictures laid over them, and colour-grouped line segments, all
/// in map-plane coordinates (`x` east, `y` north, `z` the fixed draw depths).
#[derive(Default)]
pub struct MapBake {
    pub meshes: Vec<BakedMesh>,
    pub lines: Arrows,
    pub pictures: Vec<MapPicture>,
}

/// **The plug.** A map-like content: it states its unwrapped extent, bakes the
/// drawable picture for a camera frame, and (optionally) owns the topology-aware
/// pan metric, the centre clamp and the pick. The defaults are a flat finite
/// picture — a content that is exactly that implements `extent` + `bake` alone.
pub trait MapContent {
    fn extent(&self) -> MapExtent;

    /// Bake the picture for `frame`. Called by the component only when the data
    /// changed, the mode flipped, or a Local-mode centre drifted past the margin
    /// — never per frame.
    fn bake(&mut self, frame: &MapFrame) -> MapBake;

    /// Which projection serves `span`; `prev` lets an implementation carry
    /// hysteresis. A flat content has one projection.
    fn mode(&self, span: Vec2, prev: MapMode) -> MapMode {
        let (_, _) = (span, prev);
        MapMode::Atlas
    }

    /// Move the view centre by `delta` plane units — the pan METRIC (a sphere
    /// walks great circles in Local mode; a flat picture just adds).
    fn pan(&self, center: Vec2, delta: Vec2, frame: &MapFrame) -> Vec2 {
        let _ = frame;
        center + delta
    }

    /// Keep `center` legal for `frame` (wrap the seam, hold the poles, keep a
    /// finite picture in view). The default boxes the centre into the extent.
    fn clamp(&self, center: Vec2, frame: &MapFrame) -> Vec2 {
        let _ = frame;
        let e = self.extent();
        let half = Vec2::new(e.w, e.h) * 0.5;
        center.clamp(-half, half)
    }

    /// What stands at `plane` — a content-defined id (a hex cell, a body, a
    /// region). `None` for empty space or a content with nothing to pick.
    fn pick(&self, frame: &MapFrame, plane: Vec2) -> Option<u64> {
        let (_, _) = (frame, plane);
        None
    }
}

/// The authored look a map stage carries, read off its `layers` exactly as
/// [`crate::GlobeWorld::authored_shells`] reads a globe's: the FIRST `shell`
/// colour is the backdrop/seam ink, the SECOND is the tile base (its `inset`
/// is the tile inset — the seams between tiles show the ink through the gaps,
/// the same two-shell outline trick the globe draws), a THIRD is the ink the
/// picked cell is outlined in, and a `graticule` layer asks for the flat
/// reference frame.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct MapLook {
    pub ink: Option<[f32; 3]>,
    pub base: Option<[f32; 3]>,
    pub inset: Option<f32>,
    pub graticule: bool,
    pub pick: Option<[f32; 3]>,
}

/// **The shared map view.** Generic over its content so a scene keeps typed
/// access to the plug (`content_mut` marks the picture dirty — the one rebake
/// trigger a data change needs).
pub struct WorldMap<C: MapContent> {
    view: GlobeView,
    stage: StageDef,
    content: C,
    /// View centre, map-plane units.
    center: Vec2,
    /// Visible plane HEIGHT; `<= 0` = unset, fit on the first seated frame.
    span_h: f32,
    mode: MapMode,
    /// The frame the current bake was made for; `None` = never baked.
    baked: Option<MapFrame>,
    /// The content's data changed — rebake at the next seated frame.
    dirty: bool,
    pending: Option<Vec<BakedMesh>>,
    meshes: Vec<(MeshHandle, MeshDrawOptions)>,
    /// The pictures of the current bake, awaiting upload, and uploaded.
    pending_pictures: Option<Vec<MapPicture>>,
    pictures: Vec<(TexturedMeshHandle, TextureHandle)>,
    lines: Arrows,
    seat: Option<Seat>,
    active: bool,
    /// Press latch for the pick gesture: cursor at press + travel since.
    press: Option<(Vec2, f32)>,
    picked: Option<u64>,
}

impl<C: MapContent> WorldMap<C> {
    /// A map authored by `stages.<source>` in the loaded styles — lighting, clear,
    /// passes and the [`MapLook`] shell colours all come from the file. An
    /// unauthored source still shows the picture, default-staged and loudly
    /// warned, exactly as a globe does (rule 4BB12A75).
    pub fn new(source: &str, styles: &serde_json::Value, content: C) -> Self {
        let stage = flicker::ui::stage_def(styles, source).unwrap_or_else(|| {
            tracing::warn!("stages.{source}: the world map draws default-staged");
            StageDef {
                layers: vec![StageLayer::Shells],
                ..StageDef::default()
            }
        });
        Self {
            view: GlobeView::default(),
            stage,
            content,
            center: Vec2::ZERO,
            span_h: 0.0,
            mode: MapMode::Atlas,
            baked: None,
            dirty: true,
            pending: None,
            meshes: Vec::new(),
            pending_pictures: None,
            pictures: Vec::new(),
            lines: Vec::new(),
            seat: None,
            active: false,
            press: None,
            picked: None,
        }
    }

    /// The authored stage — what the scene reads its [`MapLook`] and any other
    /// authored fact from, never a written-down copy.
    pub fn stage(&self) -> &StageDef {
        &self.stage
    }

    /// The stage's [`MapLook`] (see its docs for the layer reading).
    pub fn authored_look(&self) -> MapLook {
        let mut look = MapLook::default();
        for l in &self.stage.layers {
            match *l {
                StageLayer::Shell { color, inset, .. } => {
                    if look.ink.is_none() {
                        look.ink = Some(color);
                    } else if look.base.is_none() {
                        look.base = Some(color);
                        look.inset = Some(inset);
                    } else if look.pick.is_none() {
                        look.pick = Some(color);
                    }
                }
                StageLayer::Graticule { .. } => look.graticule = true,
                _ => {}
            }
        }
        look
    }

    /// The content, read-only.
    pub fn content(&self) -> &C {
        &self.content
    }

    /// The content, to change its DATA — colours, fields, anything drawn. Taking
    /// this marks the picture dirty, which is the whole rebake contract: mutate
    /// through here and the next seated frame rebakes.
    pub fn content_mut(&mut self) -> &mut C {
        self.dirty = true;
        &mut self.content
    }

    /// Replace the content wholesale (a re-tiled world). View centre and zoom
    /// are kept, re-clamped against the new extent on the next frame.
    pub fn replace_content(&mut self, content: C) {
        self.content = content;
        self.dirty = true;
    }

    /// Seat the map in the slot the walker reserved for its `surface` node this
    /// frame (`UiFrame::surface(id)`). `None` = not on screen, and the map then
    /// costs nothing.
    pub fn seat(&mut self, slot: Option<&SurfaceSlot>) {
        self.seat = slot.map(Seat::from);
    }

    /// The rect the map was last seated at.
    pub fn rect(&self) -> Option<Rect> {
        self.seat.map(|s| s.rect)
    }

    /// The current camera frame, or `None` before the first seat.
    pub fn frame(&self) -> Option<MapFrame> {
        let seat = self.seat?;
        Some(self.frame_for(seat.rect))
    }

    /// What a completed PICK gesture landed on since the last take — a press
    /// that travelled under the slop and released on the map.
    pub fn take_pick(&mut self) -> Option<u64> {
        self.picked.take()
    }

    fn aspect(rect: Rect) -> f32 {
        (rect.size.x / rect.size.y.max(1.0)).max(0.01)
    }

    /// The span that shows the WHOLE unwrapped picture in `rect` — the zoom-out clamp.
    fn fit_span(&self, rect: Rect) -> f32 {
        let e = self.content.extent();
        e.h.max(e.w / Self::aspect(rect))
    }

    fn frame_for(&self, rect: Rect) -> MapFrame {
        let span_h = self.span_h.max(1e-3);
        MapFrame {
            center: self.center,
            span: Vec2::new(span_h * Self::aspect(rect), span_h),
            mode: self.mode,
        }
    }

    /// Plane point under a screen cursor, given the current frame.
    fn plane_at(&self, cursor: Vec2, rect: Rect) -> Vec2 {
        let u = self.span_h.max(1e-3) / rect.size.y.max(1.0);
        let off = cursor - (rect.pos + rect.size * 0.5);
        self.center + Vec2::new(off.x * u, -off.y * u)
    }

    /// Multiply the span by `factor`, clamped, keeping the plane point under
    /// `at` (a cursor) put — or the centre put when there is none.
    fn zoom_by(&mut self, factor: f32, at: Option<Vec2>, rect: Rect) {
        let e = self.content.extent();
        let fit = self.fit_span(rect);
        let old = self.span_h.max(1e-3);
        let new = (old * factor).clamp(e.min_span.min(fit), fit);
        if let Some(cur) = at {
            let (u_old, u_new) = (old / rect.size.y.max(1.0), new / rect.size.y.max(1.0));
            let off = cur - (rect.pos + rect.size * 0.5);
            self.center += Vec2::new(off.x * (u_old - u_new), -off.y * (u_old - u_new));
        }
        self.span_h = new;
    }

    /// Does the current bake still cover `frame`? Atlas geometry is
    /// camera-independent; a Local bake covers a margin disc and expires when
    /// the centre drifts or the zoom moves meaningfully.
    fn needs_bake(&self, frame: &MapFrame) -> bool {
        if self.dirty {
            return true;
        }
        let Some(b) = self.baked else { return true };
        if b.mode != frame.mode {
            return true;
        }
        match frame.mode {
            MapMode::Atlas => false,
            MapMode::Local => {
                let drift = (frame.center - b.center).length();
                let scale = frame.span.y / b.span.y.max(1e-3);
                drift > REBAKE_DRIFT * b.span.x.min(b.span.y) || !(0.8..=1.25).contains(&scale)
            }
        }
    }

    /// One frame of map motion: the signal channel (`look` = the same
    /// (pan-x, pan-y, zoom) tuple [`crate::GlobeWorld::look_from`] resolves),
    /// the pointer sample (drag pans while captured, wheel zooms at the cursor,
    /// a press-without-drag release picks), the clamps, and the rebake decision.
    /// `active` is the scene's gate — a closed map moves for nobody.
    pub fn update(
        &mut self,
        dt: f32,
        pointer: Option<&SurfacePointer>,
        look: (f32, f32, f32),
        active: bool,
    ) {
        self.active = active;
        if !active {
            self.press = None;
            return;
        }
        let Some(seat) = self.seat else { return };
        let rect = seat.rect;
        let e = self.content.extent();
        let fit = self.fit_span(rect);
        if self.span_h <= 0.0 {
            self.span_h = fit; // first sight: the whole map, unwrapped and flat
        }
        self.span_h = self.span_h.clamp(e.min_span.min(fit), fit);

        // Signals: pan crosses LOOK_PAN_RATE of the view per second, zoom is
        // exponential — each moment of travel feels equal at every depth.
        let frame = self.frame_for(rect);
        let (dx, dy, dz) = look;
        if dx != 0.0 || dy != 0.0 {
            let step = frame.span * (LOOK_PAN_RATE * dt);
            self.center =
                self.content
                    .pan(self.center, Vec2::new(dx * step.x, dy * step.y), &frame);
        }
        if dz != 0.0 {
            self.zoom_by((-dz * ZOOM_SIG_RATE * dt).exp(), None, rect);
        }

        // The pointer: the walker's barrier sample for this map's surface.
        if let Some(p) = pointer {
            if p.captured && p.left {
                let (start, moved) = self.press.get_or_insert((p.cursor, 0.0));
                let _ = start;
                *moved += p.delta.length();
                if p.delta != Vec2::ZERO {
                    let u = self.span_h / rect.size.y.max(1.0);
                    let frame = self.frame_for(rect);
                    // The picture follows the hand: drag right shows what lies
                    // west of centre, drag down shows what lies north.
                    let dp = Vec2::new(-p.delta.x * u, p.delta.y * u);
                    self.center = self.content.pan(self.center, dp, &frame);
                }
            } else if let Some((start, moved)) = self.press.take() {
                if moved <= CLICK_SLOP_PX {
                    let frame = self.frame_for(rect);
                    let plane = self.plane_at(start, rect);
                    self.picked = self.content.pick(&frame, plane);
                }
            }
            if p.wheel != 0.0 {
                self.zoom_by((-p.wheel * WHEEL_ZOOM).exp(), Some(p.cursor), rect);
            }
        } else {
            self.press = None;
        }

        // Mode + clamps settle after motion, then the rebake decision.
        let span = self.frame_for(rect).span;
        self.mode = self.content.mode(span, self.mode);
        let frame = self.frame_for(rect);
        self.center = self.content.clamp(self.center, &frame);
        let frame = self.frame_for(rect);
        if self.needs_bake(&frame) {
            let bake = self.content.bake(&frame);
            self.pending = Some(bake.meshes);
            self.pending_pictures = Some(bake.pictures);
            self.lines = bake.lines;
            self.baked = Some(frame);
            self.dirty = false;
        }
    }

    /// Draw the map into the slot it was seated in. A no-op while closed or off
    /// screen. `base_layer` is the scene's band; the composite lands at
    /// `base + slot.layer`, exactly as a globe's does.
    pub fn render<'f>(&'f mut self, r: &mut Renderer, fg: &mut FrameGraph<'f>, base_layer: f32) {
        if !self.active {
            return;
        }
        self.upload_pending(r);
        let Some(seat) = self.seat else { return };
        let rect = seat.rect;
        let frame = self.frame_for(rect);
        let e = self.content.extent();

        // The seam is the same cut on both edges: while the view straddles it,
        // draw the whole picture again shifted a full width over — the wrap is a
        // model matrix, never a duplicated bake.
        let mut offsets = vec![0.0f32];
        if e.wrap_x && frame.mode == MapMode::Atlas {
            let half = frame.span.x * 0.5;
            if frame.center.x - half < -e.w * 0.5 {
                offsets.push(-e.w);
            }
            if frame.center.x + half > e.w * 0.5 {
                offsets.push(e.w);
            }
        }

        let camera = Camera {
            position: Vec3::new(frame.center.x, frame.center.y, 100.0),
            target: Vec3::new(frame.center.x, frame.center.y, 0.0),
            up: Vec3::Y,
            fov_y_radians: 1.0,
            near: 0.1,
            far: 200.0,
            ortho_height: Some(frame.span.y),
        };

        let Self {
            view,
            stage,
            meshes,
            pictures,
            lines,
            ..
        } = self;
        let meshes: Vec<(MeshHandle, MeshDrawOptions)> = meshes.clone();
        let pictures: Vec<(TexturedMeshHandle, TextureHandle)> = pictures.clone();
        let lines: &'f Arrows = lines;
        // A map is flown, so it changes whenever it is looked at — no dirty channel to
        // publish; its seat's authored rate is the whole liveness story.
        let inputs = StageInputs::default();
        view.render_pass(r, fg, seat, base_layer, stage, inputs, None, move |r| {
            r.set_camera(&camera);
            for dx in &offsets {
                let m = Mat4::from_translation(Vec3::new(*dx, 0.0, 0.0));
                for (h, opts) in &meshes {
                    r.draw_mesh(*h, m, *opts);
                }
                for (h, tex) in &pictures {
                    r.draw_textured_mesh(*h, *tex, m, MeshDrawOptions::default());
                }
            }
            for (color, segments) in lines {
                r.draw_lines(segments, *color);
            }
        });
    }

    fn upload_pending(&mut self, r: &mut Renderer) {
        if let Some(built) = self.pending.take() {
            for (h, _) in self.meshes.drain(..) {
                r.free_mesh(h);
            }
            self.meshes = built
                .iter()
                .filter(|(_, i, _)| !i.is_empty())
                .map(|(v, i, opts)| (r.upload_mesh(v, MeshIndices::U32(i)), *opts))
                .collect();
        }
        if let Some(built) = self.pending_pictures.take() {
            for (h, _) in self.pictures.drain(..) {
                r.free_textured_mesh(h);
            }
            self.pictures = built
                .iter()
                .filter(|pic| !pic.idx.is_empty())
                .map(|pic| {
                    (
                        r.upload_textured_mesh(&pic.verts, MeshIndices::U32(&pic.idx)),
                        pic.texture,
                    )
                })
                .collect();
        }
    }

    /// Give the GPU back: the meshes, the pictures' polygons (their textures
    /// are the content's owner's) and the offscreen target.
    pub fn free(&mut self, r: &mut Renderer) {
        for (h, _) in self.meshes.drain(..) {
            r.free_mesh(h);
        }
        for (h, _) in self.pictures.drain(..) {
            r.free_textured_mesh(h);
        }
        self.view.free(r);
    }
}

// ─── the icosahedral hex sphere, unwrapped ──────────────────────────────────

/// Local-mode thresholds, as fractions of the unwrapped width: enter below,
/// leave above — hysteresis so the boundary never flickers.
const LOCAL_ENTER_FRAC: f32 = 0.35;
const LOCAL_EXIT_FRAC: f32 = 0.45;
/// How far from a pole the view centre may walk (radians of latitude).
const POLE_STOP: f32 = 0.02;
/// The graticule draws over the cells at this ink alpha scale in map form.
const GRID_JUMP_FRAC: f32 = 0.25;

/// One cell's precomputed ATLAS geometry (equirectangular, branch-fixed).
struct AtlasCell {
    c: Vec2,
    ring: Vec<Vec2>,
}

/// **The hex sphere as a [`MapContent`].** Owns the tiling (cell directions +
/// corner rings), the per-cell colours the scene paints, and both projections.
/// The pole axis is +Y and the prime meridian +X — the graticule's own
/// convention, so the map's equator IS the globe's.
pub struct HexSphereMap {
    dirs: Vec<Vec3>,
    rings: Vec<Vec<Vec3>>,
    /// Per-cell ink; empty = every cell wears `base`.
    colors: Vec<[f32; 3]>,
    /// The backdrop/seam ink (the FIRST authored shell), loud-wrong magenta
    /// until [`Self::set_look`] lands so a missing stage cannot pass silently.
    ink: [f32; 3],
    /// The tile base (the SECOND authored shell) — the colour a cell wears when
    /// the scene has painted nothing.
    base: [f32; 3],
    /// Corner pull toward the cell centre; the gaps show the ink — the same
    /// two-shell outline trick the globe draws, flat.
    inset: f32,
    graticule: bool,
    /// The picked cell's outline ink (the THIRD authored shell), loud-wrong
    /// until the look lands — and the cell it outlines, over its picture.
    pick: [f32; 3],
    picked: Option<u32>,
    /// Atlas trim: latitudes beyond this (radians) are the POLE CAPS the flat
    /// layout cuts away. Local mode shows them; the atlas does not.
    trim: f32,
    /// Plane scale: one radian of arc = this many plane units.
    r: f32,
    /// The atlas layout, built once per tiling: `None` = trimmed away.
    atlas: Vec<Option<AtlasCell>>,
    /// The pictures cells wear (a hex's heightmap drawn in the world), by cell.
    pictures: HashMap<u32, CellPicture>,
    /// The zoom-in floor, in cell spacings across the view.
    zoom_floor_cells: f32,
}

/// A picture a cell wears: its texture, and the tile's span over the planet's
/// radius (`TILE_SPAN_M / radius_m`) — what maps a corner's place in the
/// tile's tangent plane to the picture's UV, exactly as the pixel tier's
/// `TileFrame` lays the pixels (east toward the first corner, north = centre ×
/// east, gnomonic onto the plane).
#[derive(Clone, Copy)]
struct CellPicture {
    texture: TextureHandle,
    span_over_radius: f32,
}

impl HexSphereMap {
    /// Default atlas trim: the polar circles' side of the map reads to ±66.5°,
    /// which keeps ~92% of the sphere's area on the flat sheet.
    pub const DEFAULT_TRIM_DEG: f32 = 66.5;

    /// Build over a tiling — the SAME `dirs` + corner rings the globe draws
    /// (`HexMap`'s `Sphere` + outlines). Owns copies: the map outlives any
    /// borrow the scene could lend it, and a re-tiled world replaces the whole
    /// content (`WorldMap::replace_content`).
    pub fn from_tiling(dirs: &[Vec3], rings: &[Vec<Vec3>]) -> Self {
        let trim = Self::DEFAULT_TRIM_DEG.to_radians();
        let r = RADIUS;
        let atlas = dirs
            .iter()
            .zip(rings)
            .map(|(d, ring)| Self::atlas_cell(*d, ring, trim, r))
            .collect();
        Self {
            dirs: dirs.to_vec(),
            rings: rings.to_vec(),
            colors: Vec::new(),
            ink: [1.0, 0.0, 1.0],
            base: [1.0, 0.0, 1.0],
            inset: 0.12,
            graticule: false,
            pick: [1.0, 0.0, 1.0],
            picked: None,
            trim,
            r,
            atlas,
            pictures: HashMap::new(),
            zoom_floor_cells: Self::DEFAULT_ZOOM_FLOOR_CELLS,
        }
    }

    /// The zoom-in floor the hex sphere opens with: ten cells across the view.
    pub const DEFAULT_ZOOM_FLOOR_CELLS: f32 = 10.0;

    /// How far in the view may zoom, in cell spacings across it (a consumer
    /// that draws pictures on cells wants a hex to fill the view).
    pub fn set_zoom_floor(&mut self, cells: f32) {
        self.zoom_floor_cells = cells.max(0.5);
    }

    /// **Lay a picture on a cell**: `texture` is the cell's tile map (the
    /// pixel tier's picture of the hex), `span_over_radius` the tile's span
    /// over the planet's radius. The next bake draws it over the cell.
    pub fn set_picture(&mut self, cell: u32, texture: TextureHandle, span_over_radius: f32) {
        self.pictures.insert(
            cell,
            CellPicture {
                texture,
                span_over_radius,
            },
        );
    }

    /// Take a cell's picture off; whether it had one.
    pub fn clear_picture(&mut self, cell: u32) -> bool {
        self.pictures.remove(&cell).is_some()
    }

    /// Take every picture off (another world: its pictures are not this one's).
    pub fn clear_pictures(&mut self) {
        self.pictures.clear();
    }

    /// How many cells wear a picture.
    pub fn pictures(&self) -> usize {
        self.pictures.len()
    }

    /// A cell's tile frame — the pixel tier's own (`TileFrame::new`): the
    /// centre, east toward the first corner in the tangent plane, north.
    fn tile_basis(&self, i: usize) -> (Vec3, Vec3, Vec3) {
        let d = self.dirs[i].normalize();
        let toward = self.rings[i]
            .first()
            .map(|c| c.normalize())
            .unwrap_or(if d.x.abs() < 0.9 { Vec3::X } else { Vec3::Y });
        let east = (toward - d * d.dot(toward)).normalize_or_zero();
        let east = if east.length_squared() > 0.5 {
            east
        } else {
            d.any_orthonormal_vector()
        };
        (d, east, d.cross(east))
    }

    /// The picture polygon of cell `i`, its centre and corners already
    /// projected to the plane: a fan from the centre, each corner's UV its
    /// place in the tile's plane.
    fn picture_polygon(&self, i: usize, c: Vec2, ring: &[Vec2]) -> Option<MapPicture> {
        let pic = *self.pictures.get(&(i as u32))?;
        if ring.len() < 3 || ring.len() != self.rings[i].len() {
            return None;
        }
        let (d, east, north) = self.tile_basis(i);
        let uv_of = |p: Vec3| {
            let p = p.normalize();
            let scale = 1.0 / p.dot(d).max(1e-6) / pic.span_over_radius;
            [p.dot(east) * scale + 0.5, p.dot(north) * scale + 0.5]
        };
        let vertex = |q: Vec2, uv: [f32; 2]| TexturedVertex {
            position: [q.x, q.y, PICTURE_Z],
            normal: [0.0, 0.0, 1.0],
            uv,
            tangent: [1.0, 0.0, 0.0, 1.0],
        };
        let mut verts = vec![vertex(c, [0.5, 0.5])];
        for (k, q) in ring.iter().enumerate() {
            verts.push(vertex(*q, uv_of(self.rings[i][k])));
        }
        let n = ring.len() as u32;
        let mut idx = Vec::with_capacity(n as usize * 3);
        for k in 0..n {
            idx.extend_from_slice(&[0, 1 + k, 1 + (k + 1) % n]);
        }
        Some(MapPicture {
            verts,
            idx,
            texture: pic.texture,
        })
    }

    /// Apply the stage's authored look (`WorldMap::authored_look`). A layer the
    /// stage does not author keeps the loud-wrong default — visibly missing,
    /// never quietly invented here (rules 4BB12A75 + AEEF2A68).
    pub fn set_look(&mut self, look: MapLook) {
        if let Some(ink) = look.ink {
            self.ink = ink;
        }
        if let Some(base) = look.base {
            self.base = base;
        }
        if let Some(inset) = look.inset {
            self.inset = inset;
        }
        if let Some(pick) = look.pick {
            self.pick = pick;
        }
        self.graticule = look.graticule;
    }

    /// Paint the cells — one colour per cell, in cell order (the scene derives
    /// these from the SAME shell closures its globe bakes with, so the two
    /// views can never disagree). A short vec paints the head and leaves the
    /// tail on `base`.
    pub fn set_colors(&mut self, colors: Vec<[f32; 3]>) {
        self.colors = colors;
    }

    /// **The picked cell**: outlined on the sheet in the pick ink, over its
    /// picture, at every zoom; `None` outlines nothing.
    pub fn set_picked(&mut self, cell: Option<u32>) {
        self.picked = cell;
    }

    pub fn picked(&self) -> Option<u32> {
        self.picked
    }

    /// The picked cell's ring on the sheet for `frame`: its atlas cell in
    /// atlas mode (none in a trimmed cap), its local projection in local
    /// mode (none past the horizon).
    fn picked_ring(&self, i: usize, frame: &MapFrame) -> Option<Vec<Vec2>> {
        match frame.mode {
            MapMode::Atlas => self.atlas.get(i)?.as_ref().map(|cell| cell.ring.clone()),
            MapMode::Local => {
                let (c, east, north) = self.basis(frame.center);
                self.rings
                    .get(i)?
                    .iter()
                    .map(|p| {
                        Self::to_local(p.normalize_or_zero(), c, east, north, self.r)
                            .map(|q| frame.center + q)
                    })
                    .collect()
            }
        }
    }

    /// A closed loop of segments round `ring`, drawn twice — the ring and
    /// the ring pulled a little toward its centre — so the outline carries
    /// weight over a picture at any zoom.
    fn outline(ring: &[Vec2]) -> Vec<(Vec3, Vec3)> {
        let n = ring.len();
        if n < 2 {
            return Vec::new();
        }
        let c = ring.iter().copied().fold(Vec2::ZERO, |a, p| a + p) / n as f32;
        let mut segments = Vec::with_capacity(n * 2);
        for pull in [0.0f32, PICK_WEIGHT] {
            for k in 0..n {
                let (p, q) = (ring[k], ring[(k + 1) % n]);
                let a = p + (c - p) * pull;
                let b = q + (c - q) * pull;
                segments.push((Vec3::new(a.x, a.y, PICK_Z), Vec3::new(b.x, b.y, PICK_Z)));
            }
        }
        segments
    }

    fn latlon(d: Vec3) -> (f32, f32) {
        (d.y.clamp(-1.0, 1.0).asin(), d.z.atan2(d.x))
    }

    fn dir_of(lat: f32, lon: f32) -> Vec3 {
        Vec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin())
    }

    /// A cell's atlas geometry: equirectangular, corners re-branched to the
    /// centre's side of the ±π seam, `None` when the centre lies in a trimmed cap.
    fn atlas_cell(d: Vec3, ring: &[Vec3], trim: f32, r: f32) -> Option<AtlasCell> {
        let (lat, lon) = Self::latlon(d);
        if lat.abs() > trim {
            return None;
        }
        let c = Vec2::new(lon * r, lat * r);
        let ring = ring
            .iter()
            .map(|p| {
                let (clat, mut clon) = Self::latlon(*p);
                if clon - lon > std::f32::consts::PI {
                    clon -= std::f32::consts::TAU;
                } else if lon - clon > std::f32::consts::PI {
                    clon += std::f32::consts::TAU;
                }
                Vec2::new(clon * r, clat * r)
            })
            .collect();
        Some(AtlasCell { c, ring })
    }

    /// The tangent basis a Local frame projects through: the centre direction,
    /// its east, and its map-north. At a pole east follows the centre's own
    /// longitude on, so the frame never degenerates.
    fn basis(&self, center: Vec2) -> (Vec3, Vec3, Vec3) {
        let (lat, lon) = (center.y / self.r, center.x / self.r);
        let c = Self::dir_of(
            lat.clamp(
                -(std::f32::consts::FRAC_PI_2 - 1e-4),
                std::f32::consts::FRAC_PI_2 - 1e-4,
            ),
            lon,
        );
        let east = Vec3::new(-lon.sin(), 0.0, lon.cos());
        // North is east × centre in this frame (pole +Y, meridian +X, lon
        // toward +Z): centre × east points SOUTH, and that was the local
        // view's upside-down picture (Aaron 2026-10-19). The atlas reads
        // y = lat · r, north up; the local plane must agree with it.
        let north = east.cross(c);
        (c, east, north)
    }

    /// Azimuthal-equidistant projection of `d` about the basis — plane offset
    /// from the frame centre, or `None` past the hemisphere-ish horizon.
    fn to_local(d: Vec3, c: Vec3, east: Vec3, north: Vec3, r: f32) -> Option<Vec2> {
        let cosang = d.dot(c).clamp(-1.0, 1.0);
        let ang = cosang.acos();
        if ang > std::f32::consts::PI * 0.45 {
            return None;
        }
        if ang < 1e-6 {
            return Some(Vec2::ZERO);
        }
        let t = (d - c * cosang).normalize_or_zero();
        Some(Vec2::new(t.dot(east), t.dot(north)) * (ang * r))
    }

    fn cell_color(&self, i: usize) -> [f32; 3] {
        self.colors.get(i).copied().unwrap_or(self.base)
    }

    /// One polygon fan into the mesh under construction, wound to front +Z.
    fn push_cell(
        verts: &mut Vec<MeshVertex>,
        idx: &mut Vec<u32>,
        c: Vec2,
        ring: &[Vec2],
        inset: f32,
        rgb: [f32; 3],
    ) {
        if ring.len() < 3 {
            return;
        }
        let material = direct_emissive(rgb);
        let normal = [0.0, 0.0, 1.0];
        let base = verts.len() as u32;
        verts.push(MeshVertex {
            position: [c.x, c.y, 0.0],
            normal,
            material,
        });
        let inner: Vec<Vec2> = ring.iter().map(|p| *p + (c - *p) * inset).collect();
        for p in &inner {
            verts.push(MeshVertex {
                position: [p.x, p.y, 0.0],
                normal,
                material,
            });
        }
        let n = inner.len();
        for k in 0..n {
            let (a, b) = (inner[k] - c, inner[(k + 1) % n] - c);
            let (i0, i1) = (base + 1 + k as u32, base + 1 + ((k + 1) % n) as u32);
            if a.x * b.y - a.y * b.x >= 0.0 {
                idx.extend([base, i0, i1]);
            } else {
                idx.extend([base, i1, i0]);
            }
        }
    }

    /// An axis-aligned quad at `z`, emissive `rgb` — the backdrop.
    fn push_quad(meshes: &mut Vec<BakedMesh>, min: Vec2, max: Vec2, z: f32, rgb: [f32; 3]) {
        let material = direct_emissive(rgb);
        let normal = [0.0, 0.0, 1.0];
        let v = [
            Vec2::new(min.x, min.y),
            Vec2::new(max.x, min.y),
            Vec2::new(max.x, max.y),
            Vec2::new(min.x, max.y),
        ]
        .map(|p| MeshVertex {
            position: [p.x, p.y, z],
            normal,
            material,
        });
        meshes.push((
            v.to_vec(),
            vec![0, 1, 2, 0, 2, 3],
            MeshDrawOptions::default(),
        ));
    }

    /// The shared graticule, mapped flat through `project` — the same five ink
    /// groups every globe draws, so the map's reference frame IS the globe's.
    /// Segments that straddle a seam or fall off the projection are dropped by
    /// the jump test rather than drawn across the sheet.
    fn flat_graticule(&self, frame: &MapFrame, project: impl Fn(Vec3) -> Option<Vec2>) -> Arrows {
        let jump = (frame.span.x.max(frame.span.y) * GRID_JUMP_FRAC).max(self.r * 0.3);
        graticule(1.0)
            .into_iter()
            .map(|(color, segments)| {
                let flat: Vec<(Vec3, Vec3)> = segments
                    .iter()
                    .filter_map(|(a, b)| {
                        let pa = project(a.normalize_or_zero())?;
                        let pb = project(b.normalize_or_zero())?;
                        if (pa - pb).length() > jump {
                            return None;
                        }
                        Some((Vec3::new(pa.x, pa.y, LINE_Z), Vec3::new(pb.x, pb.y, LINE_Z)))
                    })
                    .collect();
                (color, flat)
            })
            .filter(|(_, s)| !s.is_empty())
            .collect()
    }
}

impl MapContent for HexSphereMap {
    fn extent(&self) -> MapExtent {
        // Cell spacing from the tiling itself (√(sphere area / cells)), so the
        // zoom-in floor holds a handful of hexes at any frequency.
        let n = self.dirs.len().max(1) as f32;
        let spacing = (4.0 * std::f32::consts::PI / n).sqrt() * self.r;
        MapExtent {
            w: std::f32::consts::TAU * self.r,
            h: 2.0 * self.trim * self.r,
            wrap_x: true,
            min_span: spacing * self.zoom_floor_cells,
        }
    }

    fn mode(&self, span: Vec2, prev: MapMode) -> MapMode {
        let frac = span.x / (std::f32::consts::TAU * self.r);
        match prev {
            MapMode::Atlas if frac < LOCAL_ENTER_FRAC => MapMode::Local,
            MapMode::Local if frac > LOCAL_EXIT_FRAC => MapMode::Atlas,
            m => m,
        }
    }

    fn bake(&mut self, frame: &MapFrame) -> MapBake {
        let mut verts: Vec<MeshVertex> = Vec::new();
        let mut idx: Vec<u32> = Vec::new();
        let mut meshes: Vec<BakedMesh> = Vec::new();
        let mut pictures: Vec<MapPicture> = Vec::new();
        match frame.mode {
            MapMode::Atlas => {
                let e = self.extent();
                Self::push_quad(
                    &mut meshes,
                    Vec2::new(-e.w * 0.5, -e.h * 0.5),
                    Vec2::new(e.w * 0.5, e.h * 0.5),
                    BACKDROP_Z,
                    self.ink,
                );
                for (i, cell) in self.atlas.iter().enumerate() {
                    let Some(cell) = cell else { continue };
                    Self::push_cell(
                        &mut verts,
                        &mut idx,
                        cell.c,
                        &cell.ring,
                        self.inset,
                        self.cell_color(i),
                    );
                    if let Some(pic) = self.picture_polygon(i, cell.c, &cell.ring) {
                        pictures.push(pic);
                    }
                }
            }
            MapMode::Local => {
                let (c, east, north) = self.basis(frame.center);
                let radius = (frame.span.length() * 0.5 * LOCAL_MARGIN).min(self.r * 1.4);
                let min_dot = (radius / self.r).min(std::f32::consts::PI * 0.44).cos();
                Self::push_quad(
                    &mut meshes,
                    frame.center - Vec2::splat(radius),
                    frame.center + Vec2::splat(radius),
                    BACKDROP_Z,
                    self.ink,
                );
                for (i, d) in self.dirs.iter().enumerate() {
                    if d.dot(c) < min_dot {
                        continue;
                    }
                    let Some(pc) = Self::to_local(*d, c, east, north, self.r) else {
                        continue;
                    };
                    let pc = frame.center + pc;
                    let ring: Option<Vec<Vec2>> = self.rings[i]
                        .iter()
                        .map(|p| {
                            Self::to_local(p.normalize_or_zero(), c, east, north, self.r)
                                .map(|q| frame.center + q)
                        })
                        .collect();
                    let Some(ring) = ring else { continue };
                    Self::push_cell(
                        &mut verts,
                        &mut idx,
                        pc,
                        &ring,
                        self.inset,
                        self.cell_color(i),
                    );
                    if let Some(pic) = self.picture_polygon(i, pc, &ring) {
                        pictures.push(pic);
                    }
                }
            }
        }
        meshes.push((verts, idx, MeshDrawOptions::default()));
        let mut lines = if self.graticule {
            match frame.mode {
                MapMode::Atlas => {
                    let (trim, r) = (self.trim, self.r);
                    self.flat_graticule(frame, move |d| {
                        let (lat, lon) = Self::latlon(d);
                        (lat.abs() <= trim).then(|| Vec2::new(lon * r, lat * r))
                    })
                }
                MapMode::Local => {
                    let (c, east, north) = self.basis(frame.center);
                    let (center, r) = (frame.center, self.r);
                    self.flat_graticule(frame, move |d| {
                        Self::to_local(d, c, east, north, r).map(|q| center + q)
                    })
                }
            }
        } else {
            Vec::new()
        };
        if let Some(ring) = self
            .picked
            .and_then(|i| self.picked_ring(i as usize, frame))
        {
            let [r, g, b] = self.pick;
            lines.push(([r, g, b, 1.0], Self::outline(&ring)));
        }
        MapBake {
            meshes,
            lines,
            pictures,
        }
    }

    fn pan(&self, center: Vec2, delta: Vec2, frame: &MapFrame) -> Vec2 {
        match frame.mode {
            // The atlas pans in its own plane: a pixel is a pixel of the sheet.
            MapMode::Atlas => center + delta,
            // Local mode walks the SPHERE: east–west shortens with latitude, so
            // the picture under the hand moves at the speed the hand does.
            MapMode::Local => {
                let (lat, lon) = (center.y / self.r, center.x / self.r);
                let coslat = lat.cos().max(0.05);
                let lon = lon + delta.x / (self.r * coslat);
                let lat = lat + delta.y / self.r;
                Vec2::new(lon * self.r, lat * self.r)
            }
        }
    }

    fn clamp(&self, center: Vec2, frame: &MapFrame) -> Vec2 {
        let e = self.extent();
        // The seam wraps in every mode — longitude is periodic.
        let mut x = center.x;
        let half_w = e.w * 0.5;
        while x > half_w {
            x -= e.w;
        }
        while x < -half_w {
            x += e.w;
        }
        let y = match frame.mode {
            // The flat sheet scrolls only as far as it reaches: the view stays
            // on the band (centred when the whole height is visible).
            MapMode::Atlas => {
                let slack = (e.h - frame.span.y).max(0.0) * 0.5;
                center.y.clamp(-slack, slack)
            }
            // The sphere walk stops AT the pole, where "further north" ends.
            MapMode::Local => {
                let stop = (std::f32::consts::FRAC_PI_2 - POLE_STOP) * self.r;
                center.y.clamp(-stop, stop)
            }
        };
        Vec2::new(x, y)
    }

    fn pick(&self, frame: &MapFrame, plane: Vec2) -> Option<u64> {
        let d = match frame.mode {
            MapMode::Atlas => {
                let (lat, lon) = (plane.y / self.r, plane.x / self.r);
                if lat.abs() > self.trim {
                    return None; // the trimmed caps are not on the sheet
                }
                Self::dir_of(lat, lon)
            }
            MapMode::Local => {
                let (c, east, north) = self.basis(frame.center);
                let off = plane - frame.center;
                let ang = off.length() / self.r;
                if ang < 1e-6 {
                    c
                } else if ang > std::f32::consts::FRAC_PI_2 {
                    return None;
                } else {
                    let t = (east * off.x + north * off.y) / off.length();
                    (c * ang.cos() + t * ang.sin()).normalize_or_zero()
                }
            }
        };
        let mut best = (f32::MIN, None);
        for (i, dir) in self.dirs.iter().enumerate() {
            let dot = dir.dot(d);
            if dot > best.0 {
                best = (dot, Some(i as u64));
            }
        }
        best.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny synthetic tiling: a ring of cells around the equator plus one at
    /// each pole — enough to exercise trim, seam and both projections.
    fn tiling() -> (Vec<Vec3>, Vec<Vec<Vec3>>) {
        let mut dirs: Vec<Vec3> = (0..8)
            .map(|k| {
                let a = k as f32 / 8.0 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        dirs.push(Vec3::Y);
        dirs.push(-Vec3::Y);
        let rings = dirs
            .iter()
            .map(|d| {
                let seed = if d.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
                let u = d.cross(seed).normalize();
                let w = d.cross(u);
                (0..6)
                    .map(|k| {
                        let a = k as f32 / 6.0 * std::f32::consts::TAU;
                        (*d + (u * a.cos() + w * a.sin()) * 0.15).normalize()
                    })
                    .collect()
            })
            .collect();
        (dirs, rings)
    }

    fn content() -> HexSphereMap {
        let (dirs, rings) = tiling();
        let mut c = HexSphereMap::from_tiling(&dirs, &rings);
        c.set_look(MapLook {
            ink: Some([0.0, 0.0, 0.0]),
            base: Some([0.5, 0.5, 0.5]),
            inset: Some(0.1),
            graticule: true,
            pick: Some([1.0, 0.8, 0.2]),
        });
        c
    }

    fn frame(center: Vec2, span: Vec2, mode: MapMode) -> MapFrame {
        MapFrame { center, span, mode }
    }

    /// **The atlas is the unwrapped band with the caps cut.** The extent is the
    /// full circumference by the trimmed band; equator cells land at their
    /// longitudes on y = 0; the pole cells are not on the sheet at all.
    #[test]
    fn the_atlas_is_the_trimmed_unwrapped_band() {
        let c = content();
        let e = c.extent();
        assert!((e.w - std::f32::consts::TAU * RADIUS).abs() < 1e-3);
        assert!((e.h - 2.0 * HexSphereMap::DEFAULT_TRIM_DEG.to_radians() * RADIUS).abs() < 1e-3);
        assert!(e.wrap_x, "longitude wraps");
        // Equator ring: on the sheet at y = 0, at its own longitude.
        let cell = c.atlas[2]
            .as_ref()
            .expect("an equator cell is on the sheet");
        assert!(cell.c.y.abs() < 1e-3);
        let (_, lon) = HexSphereMap::latlon(c.dirs[2]);
        assert!((cell.c.x - lon * RADIUS).abs() < 1e-3);
        // The poles are the trimmed caps.
        assert!(c.atlas[8].is_none() && c.atlas[9].is_none(), "caps are cut");
    }

    /// **A cell straddling the ±π seam keeps itself in one piece**: its corners
    /// are re-branched to its centre's side rather than smeared across the sheet.
    #[test]
    fn a_seam_cell_is_rebranched_whole() {
        let c = content();
        // Cell 4 sits at longitude π — ON the cut.
        let cell = c.atlas[4].as_ref().expect("the seam cell is on the sheet");
        let spread = cell
            .ring
            .iter()
            .map(|p| (p.x - cell.c.x).abs())
            .fold(0.0f32, f32::max);
        assert!(
            spread < RADIUS,
            "corners stay beside their centre (spread {spread}), never a full sheet away"
        );
    }

    /// **Local mode keeps neighbours attached where the atlas cannot.** About a
    /// centre on the seam, the cells either side of the cut project a short step
    /// apart; the same pair on the atlas sheet are a full width apart.
    #[test]
    fn local_mode_attaches_across_the_seam() {
        let c = content();
        // Cells 3 and 5 flank the seam cell at ±45° of longitude from it.
        let seam_center = Vec2::new(std::f32::consts::PI * RADIUS, 0.0);
        let f = frame(seam_center, Vec2::splat(RADIUS), MapMode::Local);
        let (bc, east, north) = c.basis(f.center);
        let a = HexSphereMap::to_local(c.dirs[3], bc, east, north, RADIUS).unwrap();
        let b = HexSphereMap::to_local(c.dirs[5], bc, east, north, RADIUS).unwrap();
        let apart = (a - b).length();
        let arc = c.dirs[3].angle_between(c.dirs[5]) * RADIUS;
        assert!(
            (apart - arc).abs() < arc * 0.05,
            "flanking cells sit their true arc apart ({apart} vs {arc})"
        );
        // And the atlas puts them at opposite edges of the sheet.
        let (xa, xb) = (
            c.atlas[3].as_ref().unwrap().c.x,
            c.atlas[5].as_ref().unwrap().c.x,
        );
        assert!((xa - xb).abs() > RADIUS * 4.0, "the sheet separates them");
    }

    /// **The mode thresholds carry hysteresis** — enter Local below 35% of the
    /// width, return to Atlas above 45%, and hold in between.
    #[test]
    fn mode_switches_with_hysteresis() {
        let c = content();
        let w = c.extent().w;
        let span = |f: f32| Vec2::new(w * f, w * f * 0.6);
        assert_eq!(c.mode(span(0.9), MapMode::Atlas), MapMode::Atlas);
        assert_eq!(c.mode(span(0.30), MapMode::Atlas), MapMode::Local);
        assert_eq!(c.mode(span(0.40), MapMode::Local), MapMode::Local, "held");
        assert_eq!(c.mode(span(0.40), MapMode::Atlas), MapMode::Atlas, "held");
        assert_eq!(c.mode(span(0.50), MapMode::Local), MapMode::Atlas);
    }

    /// **Panning wraps the seam and stops at the limits.** Atlas: x wraps a full
    /// circumference, y stays on the band. Local: the walk stops at the pole.
    #[test]
    fn pan_wraps_x_and_clamps_y() {
        let c = content();
        let e = c.extent();
        let f = frame(Vec2::ZERO, Vec2::new(e.w * 0.2, e.h * 0.5), MapMode::Atlas);
        let over = c.pan(Vec2::new(e.w * 0.55, 0.0), Vec2::new(e.w * 0.1, 0.0), &f);
        let wrapped = c.clamp(over, &f);
        assert!(wrapped.x.abs() <= e.w * 0.5, "x wrapped into the sheet");
        let high = c.clamp(Vec2::new(0.0, e.h), &f);
        assert!(
            (high.y - (e.h - f.span.y).max(0.0) * 0.5).abs() < 1e-3,
            "y stops where the band ends"
        );
        let lf = frame(Vec2::ZERO, Vec2::splat(RADIUS * 0.5), MapMode::Local);
        let polar = c.clamp(Vec2::new(0.0, RADIUS * 3.0), &lf);
        assert!(
            polar.y <= (std::f32::consts::FRAC_PI_2 - POLE_STOP) * RADIUS + 1e-3,
            "the sphere walk stops at the pole"
        );
    }

    /// **Pick inverts both projections.** The plane point of a cell picks that
    /// cell — through the atlas and through a local frame — and the trimmed cap
    /// picks nothing on the sheet.
    #[test]
    fn pick_inverts_both_projections() {
        let c = content();
        let cell = c.atlas[2].as_ref().unwrap();
        let f = frame(Vec2::ZERO, Vec2::splat(RADIUS), MapMode::Atlas);
        assert_eq!(c.pick(&f, cell.c), Some(2));
        assert_eq!(
            c.pick(&f, Vec2::new(0.0, std::f32::consts::FRAC_PI_2 * RADIUS)),
            None,
            "the cap is off the sheet"
        );
        // Local: centre the frame on cell 3; picking dead centre finds it, and
        // the pole cell is reachable — the caps exist in local mode.
        let (_, lon) = HexSphereMap::latlon(c.dirs[3]);
        let lf = frame(
            Vec2::new(lon * RADIUS, 0.0),
            Vec2::splat(RADIUS),
            MapMode::Local,
        );
        assert_eq!(c.pick(&lf, lf.center), Some(3));
        let north = frame(
            Vec2::new(0.0, (std::f32::consts::FRAC_PI_2 - POLE_STOP) * RADIUS),
            Vec2::splat(RADIUS),
            MapMode::Local,
        );
        assert_eq!(
            c.pick(&north, north.center),
            Some(8),
            "the pole cell exists locally"
        );
    }

    /// **A bake paints ink, base and painted cells — and only the sheet's cells
    /// in atlas mode.** The backdrop quad wears the ink; unpainted cells wear
    /// the base; a painted head wears its own colour; trimmed cells emit nothing.
    #[test]
    fn bake_paints_ink_base_and_painted_cells() {
        let mut c = content();
        c.set_colors(vec![[1.0, 0.0, 0.0]]);
        let e = c.extent();
        let f = frame(Vec2::ZERO, Vec2::new(e.w, e.h), MapMode::Atlas);
        let bake = c.bake(&f);
        assert_eq!(bake.meshes.len(), 2, "the backdrop quad + the cell sheet");
        let (bv, _, _) = &bake.meshes[0];
        assert_eq!(bv.len(), 4, "the backdrop is one quad");
        let (cv, ci, _) = &bake.meshes[1];
        // 8 equator cells on the sheet, 7 verts + 18 indices each; caps absent.
        assert_eq!(cv.len(), 8 * 7, "the sheet holds the band's cells only");
        assert_eq!(ci.len(), 8 * 18);
        assert!(!bake.lines.is_empty(), "the authored graticule draws flat");
        // Every triangle fronts +Z — the winding gate, both ring orders having
        // been through the projection.
        for tri in ci.chunks(3) {
            let p = |k: u32| {
                let v = &cv[k as usize];
                Vec2::new(v.position[0], v.position[1])
            };
            let (a, b, c2) = (p(tri[0]), p(tri[1]), p(tri[2]));
            let cross = (b - a).x * (c2 - a).y - (b - a).y * (c2 - a).x;
            assert!(cross > 0.0, "triangle {tri:?} fronts the camera");
        }
    }

    /// **The local plane is the atlas's way up**: about a centre on the
    /// equator, the north pole lands at positive local y and a point east
    /// at positive local x — the same handedness as the flat sheet (y =
    /// lat · r), so zooming from the atlas into the local view never turns
    /// the picture over.
    #[test]
    fn the_local_plane_keeps_north_up() {
        let c = content();
        let (centre, east, north) = c.basis(Vec2::ZERO);
        assert!((centre - Vec3::X).length() < 1e-5);
        // A point north of the centre (the pole itself lies past the local
        // view's 81° horizon).
        let n = HexSphereMap::to_local(HexSphereMap::dir_of(0.3, 0.0), centre, east, north, c.r)
            .expect("in view");
        assert!(n.y > 0.0 && n.x.abs() < 1e-4, "north is up: {n:?}");
        let e = HexSphereMap::to_local(HexSphereMap::dir_of(0.0, 0.3), centre, east, north, c.r)
            .expect("in view");
        assert!(e.x > 0.0 && e.y.abs() < 1e-4, "east is right: {e:?}");
        // The atlas agrees: the same two directions, flat.
        let (lat_n, _) = HexSphereMap::latlon(HexSphereMap::dir_of(0.3, 0.0));
        let (_, lon_e) = HexSphereMap::latlon(HexSphereMap::dir_of(0.0, 0.3));
        assert!(lat_n > 0.0 && lon_e > 0.0);
    }

    /// **The picked cell is outlined over its picture**: a pick adds one line
    /// group in the pick ink — a closed loop twice round the cell, above the
    /// graticule; no pick, no group; a pick in a trimmed cap draws nothing on
    /// the atlas.
    #[test]
    fn the_picked_cell_is_outlined() {
        let mut c = content();
        let e = c.extent();
        let f = frame(Vec2::ZERO, Vec2::new(e.w, e.h), MapMode::Atlas);
        let plain = c.bake(&f).lines.len();
        c.set_picked(Some(0));
        assert_eq!(c.picked(), Some(0));
        let bake = c.bake(&f);
        assert_eq!(bake.lines.len(), plain + 1, "one group for the pick");
        let (ink, segments) = bake.lines.last().expect("the pick's group");
        assert_eq!(*ink, [1.0, 0.8, 0.2, 1.0], "the authored pick ink");
        let ring = c.atlas[0]
            .as_ref()
            .expect("cell 0 is on the sheet")
            .ring
            .len();
        assert_eq!(segments.len(), ring * 2, "two loops round the cell");
        assert!(segments
            .iter()
            .all(|(a, b)| a.z == PICK_Z && b.z == PICK_Z && a.z > LINE_Z));
        c.set_picked(Some(8));
        assert_eq!(
            c.bake(&f).lines.len(),
            plain,
            "a trimmed cap outlines nothing on the atlas"
        );
        c.set_picked(None);
        assert_eq!(c.bake(&f).lines.len(), plain);
    }

    /// **The component's zoom clamps hold at both ends, and the first sight is
    /// the whole map.** Driven through `update` with no pointer and no signals.
    #[test]
    fn worldmap_opens_fit_and_clamps_zoom() {
        let styles = serde_json::json!({ "stages": { "test_map": {
            "layers": [
                { "draw": "shell", "radius_scale": 1.0, "inset": 0.0, "color": [0.0, 0.0, 0.0, 1.0] },
                { "draw": "shell", "radius_scale": 1.0, "inset": 0.1, "color": [0.5, 0.5, 0.5, 1.0] },
                { "draw": "graticule", "radius_scale": 1.0 }
            ] } } });
        // A denser equator ring, so the content's zoom floor sits INSIDE the
        // fit clamp (the 10-cell ring's min_span is coarser than its own fit).
        let mut dirs: Vec<Vec3> = (0..800)
            .map(|k| {
                let a = k as f32 / 800.0 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        dirs.push(Vec3::Y);
        let rings: Vec<Vec<Vec3>> = dirs
            .iter()
            .map(|d| {
                let seed = if d.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
                let u = d.cross(seed).normalize();
                let w = d.cross(u);
                (0..6)
                    .map(|k| {
                        let a = k as f32 / 6.0 * std::f32::consts::TAU;
                        (*d + (u * a.cos() + w * a.sin()) * 0.02).normalize()
                    })
                    .collect()
            })
            .collect();
        let mut wm = WorldMap::new(
            "test_map",
            &styles,
            HexSphereMap::from_tiling(&dirs, &rings),
        );
        let look = wm.authored_look();
        assert_eq!(look.ink, Some([0.0, 0.0, 0.0]));
        assert_eq!(look.base, Some([0.5, 0.5, 0.5]));
        assert!(look.graticule);
        wm.content_mut().set_look(look);

        let slot = flicker::ui::SurfaceSlot {
            id: "map".into(),
            source: String::new(),
            scene: String::new(),
            params: Default::default(),
            x: 0.0,
            y: 0.0,
            w: 900.0,
            h: 560.0,
            layer: 0.0,
            rate: Default::default(),
            tint: [1.0; 4],
            layout: flicker::render::ViewportLayout::Single,
        };
        wm.seat(Some(&slot));
        wm.update(0.016, None, (0.0, 0.0, 0.0), true);
        let f = wm.frame().expect("seated");
        let fit = wm.fit_span(Rect {
            pos: Vec2::ZERO,
            size: Vec2::new(900.0, 560.0),
        });
        assert!((f.span.y - fit).abs() < 1e-3, "opens showing the whole map");
        assert_eq!(f.mode, MapMode::Atlas);

        // Zoom all the way in: clamped at the content's floor, mode goes Local.
        for _ in 0..600 {
            wm.update(0.05, None, (0.0, 0.0, 1.0), true);
        }
        let f = wm.frame().unwrap();
        assert!(
            (f.span.y - wm.content().extent().min_span).abs() < 1.0,
            "zoom-in stops at min_span (span {})",
            f.span.y
        );
        assert_eq!(f.mode, MapMode::Local, "regional zoom re-projects locally");

        // And back out: clamped at fit again, atlas again.
        for _ in 0..600 {
            wm.update(0.05, None, (0.0, 0.0, -1.0), true);
        }
        let f = wm.frame().unwrap();
        assert!(
            (f.span.y - fit).abs() < 1.0,
            "zoom-out stops at the whole map"
        );
        assert_eq!(f.mode, MapMode::Atlas);

        // An inactive map holds still.
        let before = wm.frame().unwrap();
        wm.update(0.05, None, (1.0, 1.0, 1.0), false);
        assert_eq!(wm.frame().unwrap(), before, "a closed map moves for nobody");
    }
}
