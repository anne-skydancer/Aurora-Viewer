//! AURORA_DEMO_PROJECTOR=shade: a pierced lamp shade round a projector, to
//! see that a projector's shadow depends on the projector and what stands in
//! its beam, never on the camera (beam_detail.rs). A hollow cylinder wears a
//! 1024 px alpha-masked texture (holes in a grid) that goes through the
//! streamer like a downloaded one (`DemoSource`, as demo_texture_res.rs); a
//! projector hangs inside it, 3.2 m above a slab, and the light that gets
//! through the holes makes a pattern of dots on the slab. The camera then
//! dollies away from the slab and back (`camera`) along one line of sight.
//! With `AURORA_PROJECTOR_TRACE=1` the viewer logs,
//! every 15 frames, a hash of what each map draws (`projector casters:`): it
//! must stay the same over the whole dolly. Two captures, camera near and far
//! (`AURORA_CAPTURE_FRAMES`), show the same dots.

use super::{HANDLE, floor_at, prim, shape, te};
use crate::scene::jobs::{AlphaKind, Jobs};
use crate::scene::textures::{DemoSource, TexSource, TextureStreamer, texture_result};
use aurora_net::NetEvent;
use aurora_prim::extra::LightImageParams;
use aurora_prim::params::*;
use aurora_prim::te::BLANK_TEXTURE;
use aurora_prim::{ExtraParams, LightParams};
use aurora_render::Renderer;
use glam::{Quat, Vec3};
use std::sync::Arc;
use uuid::Uuid;

/// The shade's texture.
const TEXTURE: u128 = 0xde7e_7e51_0000_4000_8000_0000_0000_0001;
const FIRST_LOCAL_ID: u32 = 82_100;
/// Side of the texture at full resolution.
const SIDE: u32 = 1024;
/// Holes across and down the texture.
const COLUMNS: u32 = 16;
const ROWS: u32 = 4;
/// The shade arrives once the loading screen is gone.
const ARRIVE_FRAME: u64 = 300;
/// The level shown first, as a texture's first bytes (32 px).
const FIRST_LEVEL: u8 = 5;
/// Where the projector hangs (x, y); the slab is under it.
const CENTER: (f32, f32) = (136.0, 120.0);
/// The projector's height above the slab's top, and its field of view.
const HEIGHT: f32 = 3.2;
const FOV: f32 = 2.4;

/// The dolly, in cycles of 1200 frames from `START`: far for 200, in for 300,
/// near for 200, away for 300, far for 200. It starts far, so the shade is
/// first seen from where the camera alone would ask for a coarse texture; the
/// floor must have it fine all the same.
const START: u64 = 700;
const CYCLE: u64 = 1200;
const NEAR: f32 = 5.0;
const FAR: f32 = 28.0;

fn texture() -> Uuid {
    Uuid::from_u128(TEXTURE)
}

/// The mip chain of the shade's texture from a discard level down to 1 × 1:
/// warm opaque cloth with a round hole in each cell, the same pattern at
/// every level (a coarse level has it rougher, and that shows).
pub fn texture_mips(discard: u32) -> Vec<(u32, u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut side = (SIDE >> discard.min(10)).max(1);
    loop {
        let mut px = Vec::with_capacity((side * side * 4) as usize);
        for y in 0..side {
            for x in 0..side {
                let (u, v) = ((x as f32 + 0.5) / side as f32, (y as f32 + 0.5) / side as f32);
                // position in the cell, from its middle, in cell units
                let (cx, cy) = ((u * COLUMNS as f32).fract() - 0.5, (v * ROWS as f32).fract() - 0.5);
                let hole = cx * cx + cy * cy < 0.3 * 0.3;
                px.extend_from_slice(&[235, 150, 60, if hole { 0 } else { 255 }]);
            }
        }
        out.push((side, side, px));
        if side == 1 {
            break;
        }
        side /= 2;
    }
    out
}

fn source() -> DemoSource {
    DemoSource {
        has: |id| *id == texture(),
        texels: |_, discard| texture_mips(discard as u32),
        alpha: (AlphaKind::Mask, true),
    }
}

/// Camera of the dolly at `frame`: on the same line of sight from the slab,
/// 5 to 28 m away.
pub fn camera(frame: u64) -> (Vec3, Vec3) {
    let floor = floor_at(CENTER.0, CENTER.1);
    let target = Vec3::new(CENTER.0, CENTER.1, floor + 0.1);
    let distance = distance(frame);
    // 38 degrees above the slab, from the south-west
    let (elevation, azimuth) = (38f32.to_radians(), 215f32.to_radians());
    let eye = target
        + Vec3::new(
            distance * elevation.cos() * azimuth.cos(),
            distance * elevation.cos() * azimuth.sin(),
            distance * elevation.sin(),
        );
    (eye, target)
}

/// Distance of the camera from the slab at `frame`.
pub fn distance(frame: u64) -> f32 {
    let t = frame.saturating_sub(START) % CYCLE;
    let ease = |k: f32| k * k * (3.0 - 2.0 * k);
    match t {
        0..200 => FAR,
        200..500 => FAR + (NEAR - FAR) * ease((t - 200) as f32 / 300.0),
        500..700 => NEAR,
        700..1000 => NEAR + (FAR - NEAR) * ease((t - 700) as f32 / 300.0),
        _ => FAR,
    }
}

pub struct ProjectorShadeDemo {
    sent: bool,
}

impl ProjectorShadeDemo {
    pub fn from_env() -> Option<Self> {
        (std::env::var("AURORA_DEMO_PROJECTOR").ok()?.trim() == "shade").then_some(ProjectorShadeDemo { sent: false })
    }

    pub fn tick(
        &mut self,
        frame: u64,
        world: &mut crate::world::World,
        textures: &mut TextureStreamer,
        renderer: &mut Renderer,
        jobs: &Jobs,
    ) {
        if self.sent || frame < ARRIVE_FRAME {
            return;
        }
        self.sent = true;
        textures.demo_source = Some(source());
        // held by the scenario, showing its first pixels as a download
        // would; the streamer then asks for the level it needs
        let _ = textures.acquire(renderer, texture(), TexSource::Asset);
        let pool = textures.staging.clone();
        jobs.spawn_many(vec![0u32], move |_| {
            texture_result(
                texture(),
                FIRST_LEVEL,
                texture_mips(FIRST_LEVEL as u32),
                (AlphaKind::Mask, true),
                None,
                pool.as_deref(),
            )
        });
        let boxp = shape(LL_PCODE_PATH_LINE, LL_PCODE_PROFILE_SQUARE, 100, 0, 0);
        let tube = shape(LL_PCODE_PATH_LINE, LL_PCODE_PROFILE_CIRCLE, 100, 40_000, 0);
        let floor = floor_at(CENTER.0, CENTER.1);
        let top = floor + 0.1;
        let mut cloth = (*te([1.0; 4], 0, false, 0.0)).clone();
        for f in &mut cloth.faces {
            f.texture = texture();
        }
        let light = ExtraParams {
            light: Some(LightParams {
                color: [1.0, 0.95, 0.85, 1.0],
                radius: 12.0,
                cutoff: 0.0,
                falloff: 0.0,
            }),
            light_image: Some(LightImageParams {
                texture: BLANK_TEXTURE,
                params: Vec3::new(FOV, 0.0, 0.0),
            }),
            ..Default::default()
        };
        let objects = vec![
            // the slab the dots fall on
            prim(
                FIRST_LOCAL_ID,
                Vec3::new(CENTER.0, CENTER.1, floor + 0.05),
                Quat::IDENTITY,
                Vec3::new(16.0, 16.0, 0.1),
                boxp,
                te([0.85, 0.85, 0.85, 1.0], 0, false, 0.0),
                ExtraParams::default(),
                "",
            ),
            // the shade: a hollow cylinder round the projector, its lower
            // part inside the beam
            prim(
                FIRST_LOCAL_ID + 1,
                Vec3::new(CENTER.0, CENTER.1, top + HEIGHT - 0.4),
                Quat::IDENTITY,
                Vec3::new(0.9, 0.9, 0.8),
                tube,
                Arc::new(cloth),
                ExtraParams::default(),
                "",
            ),
            // the projector, turned down (-Z), in the middle of the shade
            prim(
                FIRST_LOCAL_ID + 2,
                Vec3::new(CENTER.0, CENTER.1, top + HEIGHT),
                Quat::IDENTITY,
                Vec3::new(0.12, 0.12, 0.06),
                boxp,
                te([1.0, 1.0, 1.0, 1.0], 0, true, 0.0),
                light,
                "",
            ),
        ];
        world.apply(NetEvent::ObjectUpdates { handle: HANDLE, objects });
        log::info!("demo projector shade: a pierced shade round a projector, camera dolly from frame {START}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_texture_is_masked_with_a_hole_in_each_cell() {
        let mips = texture_mips(0);
        assert_eq!(mips.len(), 11);
        assert_eq!((mips[0].0, mips[0].1), (SIDE, SIDE));
        let alpha = |level: usize| mips[level].2.chunks(4).map(|p| p[3]).collect::<Vec<_>>();
        for level in 0..4 {
            let a = alpha(level);
            assert!(a.iter().all(|&v| v == 0 || v == 255), "level {level} is a mask");
            let holes = a.iter().filter(|&&v| v == 0).count() as f32 / a.len() as f32;
            // a round hole of radius 0.3 cell: about 28 % of the cloth
            assert!((0.2..0.36).contains(&holes), "level {level}: {holes}");
        }
        assert_eq!(texture_mips(FIRST_LEVEL as u32)[0].0, SIDE >> FIRST_LEVEL);
        assert!((source().has)(&texture()));
        assert!(!(source().has)(&Uuid::from_u128(7)));
    }

    #[test]
    fn the_dolly_goes_away_and_back_on_one_line_of_sight() {
        assert_eq!(distance(0), FAR);
        assert_eq!(distance(START + 100), FAR);
        assert_eq!(distance(START + 600), NEAR);
        assert_eq!(distance(START + 1100), FAR);
        // a cycle later, the same
        assert_eq!(distance(START + CYCLE + 600), NEAR);
        let direction = |frame: u64| {
            let (eye, target) = camera(frame);
            (eye - target).normalize()
        };
        let (a, b) = (direction(START + 100), direction(START + 600));
        assert!((a - b).length() < 1e-5, "{a:?} {b:?}");
        let (eye_far, target) = camera(START + 100);
        let (eye_near, _) = camera(START + 600);
        assert!(((eye_near - target).length() - NEAR).abs() < 1e-3);
        assert!(((eye_far - target).length() - FAR).abs() < 1e-3);
        // always moving the same way within a leg
        let legs: Vec<f32> = (START + 200..START + 500).step_by(10).map(distance).collect();
        assert!(legs.windows(2).all(|w| w[0] >= w[1]));
    }
}
