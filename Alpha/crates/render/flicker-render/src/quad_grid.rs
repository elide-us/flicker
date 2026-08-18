//! quad_grid — the reusable multi-view editor viewport.
//!
//! An N-up grid of camera views, each rendered into its own offscreen target and
//! composited back as a framed, labelled panel. The canonical layout is the 2×2 editor
//! grid ([`EDITOR_QUADS`]): the interactive Perspective view top-left, then the three
//! orthographic views — Top, Side, Front.
//!
//! The grid owns the targets, the per-view cameras, the screen layout and the composite;
//! the CALLER owns what is drawn, supplying one closure that draws view `i`. That split is
//! what lets the paperdoll and the asset-pipeline editor share one viewport implementation
//! while drawing different scenes into it.
//!
//! (Resize note: targets are created at the size the grid was built at; a live-resize refit
//! is a follow-up — the same limitation the paperdoll carried before this was extracted.)

use glam::{Vec2, Vec3};

use crate::frame_graph::{CompositeTarget, FrameGraph, Label, PanelFrame, Rect};
use crate::mesh::Camera;
use crate::pipeline_text::FontRole;
use crate::renderer::{RenderTargetHandle, Renderer};

/// One view in the grid.
#[derive(Clone, Copy, Debug)]
pub struct QuadView {
    /// Corner label drawn over the panel in its default (un-flipped) orientation.
    pub label: &'static str,
    /// Label shown when the view is FLIPPED to the opposite side — so the corner text reports which
    /// side is actually on screen (TOP→BOTTOM, LEFT→RIGHT, FRONT→BACK). Equal to `label` for a view
    /// with no opposite side (PERSP, which orbits rather than flips).
    pub label_flipped: &'static str,
    /// `None` = the caller's interactive perspective camera. `Some((eye_dir, up))` = an
    /// orthographic view down `−eye_dir`, where `eye_dir` is the unit direction FROM the
    /// model TO the camera and `up` is the panel's vertical.
    pub ortho: Option<(Vec3, Vec3)>,
}

impl QuadView {
    /// The label to show given this view's flip state — the drawn text and any hit label agree
    /// because both read it here.
    pub fn label_for(&self, flipped: bool) -> &'static str {
        if flipped {
            self.label_flipped
        } else {
            self.label
        }
    }
}

/// The canonical 2×2 editor layout: Perspective TL, Top TR, Side BL, Front BR.
///
/// **World reckoning is Z-UP — ground is XY, up is +Z.** That is how authored content arrives
/// (`source_axis: "Z_up"`, cm) and the one orientation the engine reckons in, so the editor views
/// are Z-up and no view applies a hidden conversion. A character stands along +Z, is widest across
/// X (left/right — `hand_l` is +X) and shallowest across Y (depth).
///
/// **A character faces −Y.** MEASURED on the baked `GolemBase_Low`, from the one pair of bones
/// that settles it: `foot_l` (the ANKLE) sits at y = +3.97 while `ball_l` (the TOES) sits at
/// y = −5.07, so the toes lead toward −Y. An earlier reading compared the ankle against the pelvis
/// (y = −1.92) and concluded "+Y" — but the ankle sits BEHIND the toes, so that inverted it and
/// put FRONT at the model's back. Compare the ANKLE to the BALL, never a foot bone to the spine.
pub const EDITOR_QUADS: [QuadView; 4] = [
    QuadView {
        label: "PERSP",
        label_flipped: "PERSP",
        ortho: None,
    },
    // Looking straight down, with the facing direction (−Y) up the panel; flipped looks up.
    QuadView {
        label: "TOP",
        label_flipped: "BOTTOM",
        ortho: Some((Vec3::Z, Vec3::NEG_Y)),
    },
    // From the character's left (+X); flipped views from the right. Named for the SIDE shown, not
    // "SIDE", so the corner text reports which one — the only way a flip reads as a flip.
    QuadView {
        label: "LEFT",
        label_flipped: "RIGHT",
        ortho: Some((Vec3::X, Vec3::Z)),
    },
    // Face-on from the front (−Y, where the toes point); flipped views from behind.
    QuadView {
        label: "FRONT",
        label_flipped: "BACK",
        ortho: Some((Vec3::NEG_Y, Vec3::Z)),
    },
];

/// Frame, label and clear styling for the composited panels.
#[derive(Clone, Copy)]
pub struct QuadStyle {
    /// Clear colour of each offscreen view.
    pub clear: [f64; 4],
    /// Gap (pixels) between a panel and its grid cell edge.
    pub margin: f32,
    /// Backing frame drawn behind each panel.
    pub frame: PanelFrame,
    /// Tint applied to the composited view image.
    pub tint: [f32; 4],
    pub label_offset: Vec2,
    pub label_size: f32,
    pub label_color: [f32; 4],
    pub label_role: FontRole,
}

impl Default for QuadStyle {
    fn default() -> Self {
        const FRAME_FILL: [f32; 4] = [0.10, 0.12, 0.15, 1.0];
        Self {
            clear: [0.05, 0.06, 0.08, 1.0],
            margin: 2.0,
            frame: PanelFrame {
                fill: FRAME_FILL,
                fill2: FRAME_FILL,
                grad: 0.0,
                radius: 3.0,
                border: 1.5,
                border_color: [0.45, 0.50, 0.60, 1.0],
                feather: 1.0,
                inset: 2.0,
            },
            tint: [1.0, 1.0, 1.0, 1.0],
            label_offset: Vec2::new(6.0, 4.0),
            label_size: 14.0,
            label_color: [0.80, 0.85, 0.92, 1.0],
            label_role: FontRole::Body,
        }
    }
}

/// A grid of offscreen camera views composited into screen cells.
pub struct QuadGrid {
    views: &'static [QuadView],
    targets: Vec<RenderTargetHandle>,
    cols: usize,
    /// Per-view flip: swaps an orthographic view to the opposite side (top↔bottom /
    /// left↔right / front↔back). Index of a perspective view is unused.
    pub flipped: Vec<bool>,
    pub style: QuadStyle,
    /// When `Some`, the grid tiles inside this screen sub-rectangle (a laid-out holder panel)
    /// instead of the whole window — so the 2×2 can sit BESIDE a tool rail as a first-class layout
    /// region rather than as a full-bleed backdrop. `None` = full-screen (the paperdoll's framing).
    viewport: Option<Rect>,
}

impl QuadGrid {
    /// Build the grid, creating one offscreen target per view sized to its screen cell.
    pub fn new(renderer: &mut Renderer, views: &'static [QuadView], cols: usize) -> Self {
        let cols = cols.max(1);
        let rows = views.len().div_ceil(cols);
        let s = renderer.size();
        let (w, h) = (
            (s.x / cols as f32).max(64.0) as u32,
            (s.y / rows as f32).max(64.0) as u32,
        );
        Self {
            views,
            targets: views
                .iter()
                .map(|_| renderer.create_render_target(w, h))
                .collect(),
            cols,
            flipped: vec![false; views.len()],
            style: QuadStyle::default(),
            viewport: None,
        }
    }

    /// The canonical 2×2 editor grid (Z-up — see [`EDITOR_QUADS`]).
    pub fn editor(renderer: &mut Renderer) -> Self {
        Self::new(renderer, &EDITOR_QUADS, 2)
    }

    pub fn views(&self) -> &'static [QuadView] {
        self.views
    }

    /// Confine the grid to a screen sub-rectangle (a laid-out holder), instead of tiling the whole
    /// window; `None` restores full-screen tiling. Every layout query (`cell`, `cell_at`,
    /// `local_cursor`, `label_rect`) and the composite honour it, so seating the 2×2 in a framed
    /// panel beside a tool rail is one call a frame. Targets keep their build-time resolution — a
    /// smaller viewport composites them downscaled (the same no-live-resize limitation noted above).
    pub fn set_viewport(&mut self, rect: Option<Rect>) {
        self.viewport = rect;
    }

    /// The tiling area: the confined viewport when one is set, else the whole screen.
    fn area(&self, screen: Vec2) -> Rect {
        self.viewport.unwrap_or(Rect {
            pos: Vec2::ZERO,
            size: screen,
        })
    }

    /// The camera for view `i`: the caller's `persp` camera for a perspective view, else an
    /// orthographic camera framing a model of `radius` about the origin.
    /// The camera for view `i`: the caller's `persp` camera for a perspective view, else an
    /// orthographic camera framing a model of `radius`.
    ///
    /// Every view shares ONE look-at point — `persp.target`. That is what lets a caller pan: it
    /// moves the perspective camera's target and the three orthographic views follow, so all four
    /// stay on the same subject instead of the orthos being pinned to the origin while the
    /// perspective view wanders off it. A caller that never pans passes a target of `ZERO` and
    /// gets exactly the previous behaviour.
    pub fn camera(&self, i: usize, radius: f32, persp: &Camera) -> Camera {
        let Some((base_dir, up)) = self.views[i].ortho else {
            return *persp;
        };
        let dir = if self.flipped[i] { -base_dir } else { base_dir };
        let r = radius.max(0.25);
        Camera {
            position: persp.target + dir * (r * 4.0),
            target: persp.target,
            up,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 0.01,
            // Measured from the shared look-at point, so a panned-away view cannot clip the model.
            far: r * 12.0,
            ortho_height: Some(r * 2.2),
        }
    }

    /// View `i`'s panel rectangle in screen pixels (margin already applied).
    pub fn cell(&self, i: usize, screen: Vec2) -> Rect {
        let area = self.area(screen);
        let rows = self.views.len().div_ceil(self.cols);
        let size = Vec2::new(area.size.x / self.cols as f32, area.size.y / rows as f32);
        let (col, row) = (i % self.cols, i / self.cols);
        let m = self.style.margin;
        Rect {
            pos: area.pos + Vec2::new(col as f32 * size.x + m, row as f32 * size.y + m),
            size: Vec2::new(size.x - m * 2.0, size.y - m * 2.0),
        }
    }

    /// Which view the cursor is over, if any.
    pub fn cell_at(&self, cursor: Vec2, screen: Vec2) -> Option<usize> {
        let area = self.area(screen);
        // Relative to the tiling area, so a cursor left of / above a confined holder (e.g. over the
        // editor rail) is outside every view — not saturated into view 0.
        let local = cursor - area.pos;
        if local.x < 0.0 || local.y < 0.0 {
            return None;
        }
        let rows = self.views.len().div_ceil(self.cols);
        let size = Vec2::new(area.size.x / self.cols as f32, area.size.y / rows as f32);
        let (col, row) = ((local.x / size.x) as usize, (local.y / size.y) as usize);
        if col >= self.cols || row >= rows {
            return None;
        }
        let i = row * self.cols + col;
        (i < self.views.len()).then_some(i)
    }

    /// The clickable corner-label box in cell `i` — where the label is drawn, grown to hold the
    /// widest label so a click on the visible text lands. The flip control lives here (the
    /// asset-pipeline editor toggles a view's side on a click inside it), and the geometry is
    /// defined beside the label draw so the hit region can never drift from the drawn text.
    pub fn label_rect(&self, i: usize, screen: Vec2) -> Rect {
        let cell = self.cell(i, screen);
        let pad = Vec2::new(4.0, 3.0);
        Rect {
            pos: cell.pos + self.style.label_offset - pad,
            // Wide enough for the longest label ("BOTTOM") at `label_size`, tall enough for the line.
            size: Vec2::new(
                self.style.label_size * 5.2,
                self.style.label_size + pad.y * 2.0,
            ),
        }
    }

    /// Whether `cursor` is inside view `i`'s clickable label box (see [`Self::label_rect`]).
    pub fn label_hit(&self, i: usize, cursor: Vec2, screen: Vec2) -> bool {
        let r = self.label_rect(i, screen);
        cursor.x >= r.pos.x
            && cursor.y >= r.pos.y
            && cursor.x <= r.pos.x + r.size.x
            && cursor.y <= r.pos.y + r.size.y
    }

    /// The cursor in view `i`'s local pixel space, for picking — `None` when outside it.
    /// Pair with [`Self::cell`]'s `size` as the pick viewport.
    pub fn local_cursor(&self, i: usize, cursor: Vec2, screen: Vec2) -> Option<Vec2> {
        let rect = self.cell(i, screen);
        let local = cursor - rect.pos;
        (local.x >= 0.0 && local.y >= 0.0 && local.x <= rect.size.x && local.y <= rect.size.y)
            .then_some(local)
    }

    /// Render every view into its target and composite them into their screen cells at `layer`,
    /// with ONE shared camera — the perspective view uses `persp`, the ortho views are built around
    /// its look-at (so they pan together). `draw` receives the renderer and the view index, with
    /// that view's camera already set. For INDEPENDENT per-view cameras, use [`Self::render_with`].
    pub fn render<F>(&self, r: &mut Renderer, layer: f32, radius: f32, persp: &Camera, draw: F)
    where
        F: Fn(&mut Renderer, usize) + Copy,
    {
        let cameras: Vec<Camera> = (0..self.targets.len())
            .map(|i| self.camera(i, radius, persp))
            .collect();
        self.render_with(r, layer, &cameras, draw);
    }

    /// Like [`Self::render`] but with an EXPLICIT camera per view: `cameras[i]` is view `i`'s camera.
    /// This is what lets a caller drive each panel INDEPENDENTLY — the asset-pipeline editor gives
    /// every quad its own pan/zoom so panning one leaves the others put. A view with no matching
    /// camera (short slice) is skipped rather than panicking.
    pub fn render_with<F>(&self, r: &mut Renderer, layer: f32, cameras: &[Camera], draw: F)
    where
        F: Fn(&mut Renderer, usize) + Copy,
    {
        let screen = r.size();
        let mut fg = FrameGraph::new();
        for (i, target) in self.targets.iter().copied().enumerate() {
            let Some(&cam) = cameras.get(i) else { continue };
            fg.target(target, self.style.clear, move |rr| {
                rr.set_camera(&cam);
                draw(rr, i);
            });
            fg.composite_panel(
                target,
                CompositeTarget::Screen,
                self.cell(i, screen),
                layer,
                self.style.tint,
                Some(self.style.frame),
                Some(Label {
                    text: self.views[i].label_for(self.flipped[i]),
                    offset: self.style.label_offset,
                    size: self.style.label_size,
                    color: self.style.label_color,
                    role: self.style.label_role,
                }),
            );
        }
        fg.execute(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4-view grid at 2 columns lays out TL, TR, BL, BR — the order `EDITOR_QUADS`
    /// documents, which the paperdoll's click routing depends on.
    #[test]
    fn editor_cells_are_screen_quadrants_in_tl_tr_bl_br_order() {
        let grid = bare_grid();
        let screen = Vec2::new(1000.0, 800.0);
        let corners: Vec<(f32, f32)> = (0..4)
            .map(|i| {
                let c = grid.cell(i, screen);
                (c.pos.x, c.pos.y)
            })
            .collect();
        assert_eq!(
            corners,
            vec![(2.0, 2.0), (502.0, 2.0), (2.0, 402.0), (502.0, 402.0)]
        );
        // Each panel is its quadrant inset by the margin on both sides.
        let c = grid.cell(0, screen);
        assert_eq!((c.size.x, c.size.y), (496.0, 396.0));
    }

    #[test]
    fn cell_at_maps_cursor_to_view_and_rejects_outside() {
        let grid = bare_grid();
        let screen = Vec2::new(1000.0, 800.0);
        assert_eq!(grid.cell_at(Vec2::new(10.0, 10.0), screen), Some(0));
        assert_eq!(grid.cell_at(Vec2::new(900.0, 10.0), screen), Some(1));
        assert_eq!(grid.cell_at(Vec2::new(10.0, 700.0), screen), Some(2));
        assert_eq!(grid.cell_at(Vec2::new(900.0, 700.0), screen), Some(3));
        // Negative cursors must not saturate into view 0.
        assert_eq!(grid.cell_at(Vec2::new(-5.0, 10.0), screen), None);
        assert_eq!(grid.cell_at(Vec2::new(10.0, -5.0), screen), None);
        assert_eq!(grid.cell_at(Vec2::new(1200.0, 10.0), screen), None);
    }

    /// A confined viewport tiles the 2×2 inside a holder sub-rectangle: cells shift to the holder
    /// origin and shrink to its size, and a cursor outside the holder (over a sibling tool rail) is
    /// on no view — the property that lets the editor rail sit beside the quad without stealing drags.
    #[test]
    fn a_viewport_confines_the_grid_to_a_sub_rectangle() {
        let mut grid = bare_grid();
        let screen = Vec2::new(1000.0, 800.0);
        // A 600×400 holder at (200,100).
        grid.set_viewport(Some(Rect {
            pos: Vec2::new(200.0, 100.0),
            size: Vec2::new(600.0, 400.0),
        }));
        // Top-left cell sits at the holder origin + margin and is a quadrant of the HOLDER (300×200).
        let c = grid.cell(0, screen);
        assert_eq!((c.pos.x, c.pos.y), (202.0, 102.0));
        assert_eq!((c.size.x, c.size.y), (296.0, 196.0));
        // Left of / above the holder → outside every view (this is where the tool rail lives).
        assert_eq!(grid.cell_at(Vec2::new(50.0, 50.0), screen), None);
        // Inside the holder's top-right quadrant → view 1.
        assert_eq!(grid.cell_at(Vec2::new(560.0, 160.0), screen), Some(1));
        // Past the holder's right edge (it ends at x=800) → outside again.
        assert_eq!(grid.cell_at(Vec2::new(860.0, 160.0), screen), None);
    }

    #[test]
    fn local_cursor_is_relative_to_the_panel_and_none_outside() {
        let grid = bare_grid();
        let screen = Vec2::new(1000.0, 800.0);
        // Top-right view: 502 px from the left, so a cursor at 512 sits 10 px into it.
        assert_eq!(
            grid.local_cursor(1, Vec2::new(512.0, 12.0), screen),
            Some(Vec2::new(10.0, 10.0))
        );
        // The same cursor is not inside the top-LEFT view.
        assert_eq!(grid.local_cursor(0, Vec2::new(512.0, 12.0), screen), None);
    }

    /// The perspective view defers to the caller's camera; the ortho views look down
    /// `−eye_dir`, and `flipped` swaps one to the opposite side.
    #[test]
    fn cameras_are_perspective_at_zero_and_flippable_ortho_elsewhere() {
        let mut grid = bare_grid();
        let persp = Camera {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..default_camera()
        };
        assert_eq!(grid.camera(0, 10.0, &persp).position, persp.position);
        assert!(grid.camera(0, 10.0, &persp).ortho_height.is_none());

        // Z-UP reckoning: a character FACES −Y (measured — the toes/`ball_l` lead the ankle
        // toward −Y), so FRONT stands at −Y to look the model in the face. At +Y it would be
        // BEHIND it, which is exactly the bug this pins.
        let front = grid.camera(3, 10.0, &persp);
        assert_eq!(
            front.position,
            Vec3::NEG_Y * 40.0,
            "FRONT must be in front, not behind"
        );
        assert_eq!(front.up, Vec3::Z);
        assert_eq!(front.ortho_height, Some(22.0));
        grid.flipped[3] = true;
        assert_eq!(
            grid.camera(3, 10.0, &persp).position,
            Vec3::Y * 40.0,
            "flip shows the back"
        );

        // TOP looks straight down with the FACING direction up the panel — so its up is −Y, the
        // way the model faces. A +Y up would draw every character upside-down in that panel.
        let top = grid.camera(1, 10.0, &persp);
        assert_eq!(top.position, Vec3::Z * 40.0);
        assert_eq!(
            top.up,
            Vec3::NEG_Y,
            "the facing direction reads up the TOP panel"
        );

        // Every ORTHO view stands the model up along +Z — the one world reckoning (ground = XY).
        // TOP is the exception: looking straight down, its panel-vertical is the facing axis.
        assert_eq!(grid.camera(2, 10.0, &persp).up, Vec3::Z, "SIDE is Z-up");
        assert_eq!(
            grid.camera(1, 10.0, &persp).position,
            Vec3::Z * 40.0,
            "TOP looks down from +Z"
        );
    }

    /// Every view shares ONE look-at point, so a caller that pans its perspective camera drags the
    /// orthographic panels along with it. Pinned because the orthos were previously hard-targeted
    /// at the origin: panning then moved only PERSP and the four views disagreed about the subject.
    #[test]
    fn ortho_views_follow_the_perspective_look_at_point() {
        let grid = bare_grid();
        let pan = Vec3::new(3.0, -4.0, 12.0);
        let at_origin = Camera {
            position: Vec3::X * 50.0,
            ..default_camera()
        };
        let panned = Camera {
            position: pan + Vec3::X * 50.0,
            target: pan,
            ..default_camera()
        };

        for i in 1..4 {
            let a = grid.camera(i, 10.0, &at_origin);
            let b = grid.camera(i, 10.0, &panned);
            assert_eq!(
                a.target,
                Vec3::ZERO,
                "an unpanned caller keeps exactly the old framing"
            );
            assert_eq!(b.target, pan, "view {i} looks at the shared point");
            // The panel TRANSLATED with the point — same direction, same zoom, shifted position.
            assert_eq!(
                b.position - a.position,
                pan,
                "view {i} translated by the pan"
            );
            assert_eq!(
                b.position - b.target,
                a.position - a.target,
                "view direction unchanged"
            );
            assert_eq!(
                b.ortho_height, a.ortho_height,
                "and the framing height is untouched"
            );
        }
        // The perspective view is still passed through verbatim, pan and all.
        assert_eq!(grid.camera(0, 10.0, &panned).target, pan);
    }

    /// A grid with no renderer — the layout/camera math is pure, so it is testable without a GPU.
    fn bare_grid() -> QuadGrid {
        QuadGrid {
            views: &EDITOR_QUADS,
            targets: Vec::new(),
            cols: 2,
            flipped: vec![false; 4],
            style: QuadStyle::default(),
            viewport: None,
        }
    }

    fn default_camera() -> Camera {
        Camera {
            position: Vec3::ZERO,
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y_radians: 1.0,
            near: 0.01,
            far: 100.0,
            ortho_height: None,
        }
    }
}
