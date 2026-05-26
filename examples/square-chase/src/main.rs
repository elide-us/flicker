//! SquareChase: port of the DXTK / DirectXTK SquareChase sample to flicker.
//!
//! A 75×75 square spawns at a random location every 0.75 seconds. Click inside
//! the square before it disappears to score a point; the square's color cycles
//! R → G → B with each hit. Escape quits.
//!
//! The whole game is ~30 lines of logic; the rest is window/title setup.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, App, InputState, Key};
use flicker::render::{Renderer, TextureHandle, Vec2};

const SQUARE_SIZE: f32 = 75.0;
const TIME_PER_SQUARE: f32 = 0.75;

/// Tints cycled by `score % 3`. Solid red, green, blue.
const COLORS: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 1.0],
    [0.0, 1.0, 0.0, 1.0],
    [0.0, 0.0, 1.0, 1.0],
];

struct SquareChase {
    /// 1×1 white texture; the sprite shader multiplies the white texel by our
    /// per-quad tint, so one texture serves all three colors.
    white: Option<TextureHandle>,
    square_pos: Vec2,
    score: u32,
    /// Seconds until the current square despawns. Starts at 0 so the first
    /// frame spawns the initial square.
    time_remaining: f32,
    quit: bool,
}

impl SquareChase {
    fn new() -> Self {
        Self {
            white: None,
            square_pos: Vec2::ZERO,
            score: 0,
            time_remaining: 0.0,
            quit: false,
        }
    }

    fn spawn(&mut self, screen: Vec2) {
        // Clamp the spawn window so the square always fits fully on-screen
        // (fixes the original's `rand() % size.x - 75` precedence bug).
        let max_x = ((screen.x - SQUARE_SIZE) as u32).max(1);
        let max_y = ((screen.y - SQUARE_SIZE) as u32).max(1);
        self.square_pos = Vec2::new(
            fastrand::u32(0..max_x) as f32,
            fastrand::u32(0..max_y) as f32,
        );
        self.time_remaining = TIME_PER_SQUARE;
    }

    fn square_contains(&self, p: Vec2) -> bool {
        p.x >= self.square_pos.x
            && p.x < self.square_pos.x + SQUARE_SIZE
            && p.y >= self.square_pos.y
            && p.y < self.square_pos.y + SQUARE_SIZE
    }
}

impl App for SquareChase {
    fn init(&mut self, renderer: &mut Renderer) {
        let white_pixel = [0xff, 0xff, 0xff, 0xff];
        self.white = Some(renderer.load_texture(&white_pixel, 1, 1));
        renderer.window().set_title("SquareChase");
        tracing::info!("square-chase ready");
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) {
        if input.key_down(Key::Escape) {
            self.quit = true;
            return;
        }

        let screen = renderer.size();
        let elapsed = dt.as_secs_f32();

        if self.time_remaining == 0.0 {
            self.spawn(screen);
        }

        // Hit detection matches the original: scores every frame the button is
        // held while the cursor is inside the rect (auto-fire is a feature).
        if input.mouse_left && self.square_contains(input.mouse_position) {
            self.score += 1;
            self.time_remaining = 0.0; // force respawn next frame
        }

        self.time_remaining = (self.time_remaining - elapsed).max(0.0);
    }

    fn should_quit(&self) -> bool {
        self.quit
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some(white) = self.white {
            let color = COLORS[(self.score as usize) % COLORS.len()];
            renderer.draw_sprite(
                white,
                self.square_pos,
                Vec2::new(SQUARE_SIZE, SQUARE_SIZE),
                color,
            );
        }

        renderer.draw_text(
            &format!("Score: {}", self.score),
            Vec2::new(16.0, 16.0),
            28.0,
            [1.0, 1.0, 1.0, 1.0],
        );
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "square_chase=info,flicker_app=info".into()),
        )
        .init();

    run(SquareChase::new())?;
    Ok(())
}
