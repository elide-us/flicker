//! The app shell: the intro splash and the menu / pause / settings screens, plus
//! the reusable flat [`Modal`] that drives them (`scripts/logo.lua` +
//! `scripts/modal.lua` + the `screens` section of `ui_elements.json`). Ported
//! from flicker-world's shell, trimmed for sol2.
//!
//! Flow: `Logo` (two full-frame splash logos) → `Menu` (Start / Settings /
//! Quit) → `Sim`; `Sim` pushes `Pause` (overlay: Resume / Settings / Quit) on
//! Esc; both Menu and Pause push `Settings` (overlay). sol2 has no game-settings
//! infrastructure yet, so `Settings` is a lean placeholder modal — the actual
//! tunables live on the sim control panel.

use std::time::Duration;

use flicker::app::{InputState, Key};
use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, Transition};
use flicker::script::{ScriptHost, ValueMap};
use flicker::ui::{load_ui_json, load_widgets, render_hud};

use crate::scene::Sim;

const MODAL_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/modal.lua");
const LOGO_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/logo.lua");
const UI_ELEMENTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui_elements.json");

/// The startup splash logos, in play order: `(texture name, png path)`.
const LOGO_IMAGES: [(&str, &str); 2] = [
    ("elideus", concat!(env!("CARGO_MANIFEST_DIR"), "/assets/elideus_productions_yellow.png")),
    ("clay", concat!(env!("CARGO_MANIFEST_DIR"), "/assets/clay_engine_infinity_grey.png")),
];

fn load_image_texture(renderer: &mut Renderer, path: &str) -> Option<(TextureHandle, u32, u32)> {
    match image::open(path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            Some((renderer.load_texture(&rgba, w, h), w, h))
        }
        Err(e) => {
            tracing::error!("failed to load logo image {path}: {e}");
            None
        }
    }
}

/// Intro splash: plays the Elideus Productions + Clay Engine logos full-frame
/// (fade in / hold / fade out, driven by `logo.lua` + `UI.logo`), then replaces
/// itself with the menu. Skippable with click / Space / Esc.
pub struct Logo {
    script: Option<ScriptHost>,
    /// `[white, <logos…>]` — index = the texture id the script references.
    textures: Vec<TextureHandle>,
    /// Each logo's native pixel size (parallel to `UI.logo.images`).
    sizes: Vec<(u32, u32)>,
    elapsed: Duration,
}

impl Logo {
    pub fn new() -> Self {
        Self {
            script: None,
            textures: Vec::new(),
            sizes: Vec::new(),
            elapsed: Duration::ZERO,
        }
    }

    fn model(&self) -> ValueMap {
        let mut m = ValueMap::new().with("elapsed", self.elapsed.as_secs_f32());
        for (i, &(w, h)) in self.sizes.iter().enumerate() {
            m = m.with(format!("img{}_w", i + 1), w).with(format!("img{}_h", i + 1), h);
        }
        m
    }
}

impl Default for Logo {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene for Logo {
    fn enter(&mut self, renderer: &mut Renderer) {
        // id 0 = white (backdrop rect); the logos follow.
        let mut textures = vec![renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1)];
        let mut ids: Vec<(&str, u32)> = vec![("white", 0)];
        let mut sizes = Vec::new();
        for (name, path) in LOGO_IMAGES {
            if let Some((handle, w, h)) = load_image_texture(renderer, path) {
                ids.push((name, textures.len() as u32));
                textures.push(handle);
                sizes.push((w, h));
            } else {
                sizes.push((1, 1)); // keep imgN indices aligned on load failure
            }
        }
        self.script = match ScriptHost::from_file(LOGO_SCRIPT) {
            Ok(s) => {
                let _ = s.set_texture_ids(&ids);
                load_ui_json(&s, UI_ELEMENTS);
                load_widgets(&s);
                Some(s)
            }
            Err(e) => {
                tracing::error!("logo script load failed ({LOGO_SCRIPT}): {e}");
                None
            }
        };
        self.textures = textures;
        self.sizes = sizes;
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        self.elapsed += dt;
        let skip =
            input.mouse_left_pressed || input.key_down(Key::Space) || input.key_down(Key::Escape);
        let done = match self.script.as_ref() {
            Some(s) => {
                let _ = s.set_model(&self.model());
                let size = renderer.size();
                s.update(input, size.x, size.y).map(|r| r.is_on("done")).unwrap_or(true)
            }
            None => true,
        };
        if skip || done {
            return Transition::Replace(Box::new(Menu::new()));
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(s) = self.script.as_ref() else {
            return;
        };
        let _ = s.set_model(&self.model());
        let size = renderer.size();
        if let Ok(cmds) = s.draw(size.x, size.y) {
            if let Some(&white) = self.textures.first() {
                render_hud(renderer, &cmds, white, &self.textures);
            }
        }
    }
}

/// A reusable flat modal panel (title + vertical buttons) driven by a named
/// `screens` entry. `update` returns `{ <item id> = clicked }`.
pub struct Modal {
    script: Option<ScriptHost>,
    white: Option<TextureHandle>,
    last_mouse: Vec2,
}

impl Modal {
    pub fn new(renderer: &mut Renderer) -> Self {
        let white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        let script = match ScriptHost::from_file(MODAL_SCRIPT) {
            Ok(s) => {
                load_ui_json(&s, UI_ELEMENTS);
                load_widgets(&s);
                Some(s)
            }
            Err(e) => {
                tracing::warn!("modal script load failed ({MODAL_SCRIPT}): {e}");
                None
            }
        };
        Self {
            script,
            white,
            last_mouse: Vec2::ZERO,
        }
    }

    pub fn update(&mut self, input: &InputState, w: f32, h: f32, screen: &str) -> ValueMap {
        self.last_mouse = input.mouse_position;
        if let Some(s) = &self.script {
            let model = ValueMap::new()
                .with("screen", screen)
                .with("mx", input.mouse_position.x)
                .with("my", input.mouse_position.y);
            let _ = s.set_model(&model);
            return s.update(input, w, h).unwrap_or_default();
        }
        ValueMap::new()
    }

    pub fn render(&self, renderer: &mut Renderer, screen: &str) {
        if let (Some(s), Some(white)) = (&self.script, self.white) {
            let size = renderer.size();
            let model = ValueMap::new()
                .with("screen", screen)
                .with("mx", self.last_mouse.x)
                .with("my", self.last_mouse.y);
            let _ = s.set_model(&model);
            match s.draw(size.x, size.y) {
                Ok(cmds) => render_hud(renderer, &cmds, white, &[]),
                Err(e) => tracing::warn!("modal draw error: {e}"),
            }
        }
    }
}

fn modal_update(modal: &mut Option<Modal>, input: &InputState, renderer: &Renderer, screen: &str) -> ValueMap {
    let size = renderer.size();
    modal.as_mut().map(|m| m.update(input, size.x, size.y, screen)).unwrap_or_default()
}

/// The main menu: Start → drop into the Stage-1 simulation, Settings → push
/// settings, Quit.
#[derive(Default)]
pub struct Menu {
    modal: Option<Modal>,
}

impl Menu {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Scene for Menu {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.modal = Some(Modal::new(renderer));
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let res = modal_update(&mut self.modal, input, renderer, "menu");
        if res.is_on("start") {
            return Transition::Replace(Box::new(Sim::new()));
        }
        if res.is_on("settings") {
            return Transition::Push(Box::new(Settings::new()));
        }
        if res.is_on("quit") {
            return Transition::Quit;
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some(m) = &self.modal {
            m.render(renderer, "menu");
        }
    }
}

/// The in-sim pause overlay: Resume, Settings, Quit. Esc also resumes.
pub struct Pause {
    modal: Option<Modal>,
    esc_prev: bool,
}

impl Pause {
    pub fn new() -> Self {
        // Assume Esc is held at creation (it pushed us), so it doesn't pop us the
        // same frame; the next press resumes.
        Self {
            modal: None,
            esc_prev: true,
        }
    }
}

impl Scene for Pause {
    fn is_overlay(&self) -> bool {
        true
    }

    fn enter(&mut self, renderer: &mut Renderer) {
        self.modal = Some(Modal::new(renderer));
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let esc = input.key_down(Key::Escape);
        let esc_edge = esc && !self.esc_prev;
        self.esc_prev = esc;
        if esc_edge {
            return Transition::Pop;
        }
        let res = modal_update(&mut self.modal, input, renderer, "pause");
        if res.is_on("resume") {
            return Transition::Pop;
        }
        if res.is_on("settings") {
            return Transition::Push(Box::new(Settings::new()));
        }
        if res.is_on("quit") {
            return Transition::Quit;
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some(m) = &self.modal {
            m.render(renderer, "pause");
        }
    }
}

/// A lean settings overlay — a placeholder modal (the sim's tunables live on the
/// control panel). Back or Esc returns. When sol2 grows real display/input
/// settings this becomes a tabbed workbench like flicker-world's.
pub struct Settings {
    modal: Option<Modal>,
    esc_prev: bool,
}

impl Settings {
    pub fn new() -> Self {
        Self {
            modal: None,
            esc_prev: true,
        }
    }
}

impl Scene for Settings {
    fn is_overlay(&self) -> bool {
        true
    }

    fn enter(&mut self, renderer: &mut Renderer) {
        self.modal = Some(Modal::new(renderer));
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let esc = input.key_down(Key::Escape);
        let esc_edge = esc && !self.esc_prev;
        self.esc_prev = esc;
        let res = modal_update(&mut self.modal, input, renderer, "settings");
        if esc_edge || res.is_on("back") {
            return Transition::Pop;
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some(m) = &self.modal {
            m.render(renderer, "settings");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// modal.lua loads against the real JSON and emits a panel + buttons for each
    /// screen, returning the item ids.
    #[test]
    fn modal_screens_load_and_emit_buttons() {
        let script = ScriptHost::from_file(MODAL_SCRIPT).expect("modal.lua loads");
        load_ui_json(&script, UI_ELEMENTS);
        load_widgets(&script);

        for (screen, action) in [("menu", "start"), ("pause", "resume"), ("settings", "back")] {
            let model = ValueMap::new().with("screen", screen).with("mx", -1.0).with("my", -1.0);
            script.set_model(&model).expect("model");
            let input = InputState::new();
            let res = script.update(&input, 1280.0, 720.0).expect("update");
            assert!(!res.is_on(action), "{screen}: {action} should be up without a click");
            let cmds = script.draw(1280.0, 720.0).expect("draw");
            assert!(!cmds.is_empty(), "{screen} should draw a panel + buttons");
        }
    }
}
