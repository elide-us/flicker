//! hello-sprite: open a window and prove all three primitive pipelines work.
//!
//! Draws a solid-color triangle, a textured quad whose texture is a
//! procedurally-generated checkerboard, and a couple of lines of text using
//! the system's default sans-serif font.

use anyhow::Result;
use flicker::app::{run, App};
use flicker::render::{Renderer, TextureHandle, Vec2};

struct HelloSprite {
    texture: Option<TextureHandle>,
}

impl App for HelloSprite {
    fn init(&mut self, renderer: &mut Renderer) {
        // Build a 16x16 RGBA8 checkerboard: alternating warm/cold tiles, 4 px each.
        const W: u32 = 16;
        const H: u32 = 16;
        const TILE: u32 = 4;
        let mut pixels = Vec::with_capacity((W * H * 4) as usize);
        for y in 0..H {
            for x in 0..W {
                let on = ((x / TILE) + (y / TILE)).is_multiple_of(2);
                let rgba = if on {
                    [0xff, 0xb0, 0x4a, 0xff]
                } else {
                    [0x1d, 0x4e, 0x89, 0xff]
                };
                pixels.extend_from_slice(&rgba);
            }
        }
        self.texture = Some(renderer.load_texture(&pixels, W, H));
        tracing::info!("hello-sprite initialized");
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let size = renderer.size();

        // 1. Solid triangle on the left.
        let cx = size.x * 0.25;
        let cy = size.y * 0.5;
        let r = 100.0_f32;
        renderer.draw_triangle(
            Vec2::new(cx, cy - r),
            Vec2::new(cx - r, cy + r),
            Vec2::new(cx + r, cy + r),
            [0.95, 0.35, 0.45, 1.0],
        );

        // 2. Textured quad in the middle (scaled up so the 16x16 checker is visible).
        if let Some(tex) = self.texture {
            let sprite_size = Vec2::new(192.0, 192.0);
            let position = Vec2::new(
                size.x * 0.5 - sprite_size.x * 0.5,
                size.y * 0.5 - sprite_size.y * 0.5,
            );
            renderer.draw_sprite(tex, position, sprite_size, [1.0, 1.0, 1.0, 1.0]);
        }

        // 3. Text in the top-left, plus a label below the sprite.
        renderer.draw_text(
            "flicker — hello primitives",
            Vec2::new(16.0, 16.0),
            28.0,
            [1.0, 1.0, 1.0, 1.0],
        );
        renderer.draw_text(
            "triangle · textured quad · system-font text",
            Vec2::new(16.0, 52.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "hello_sprite=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    let app = HelloSprite { texture: None };
    run(app)?;
    Ok(())
}
