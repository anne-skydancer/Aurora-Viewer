//! AURORA_DEMO_BLEND_SHADOWS: alpha-blended prims standing in the sun, to
//! see their shadows (fs_shadow) and what they cost.
//!
//! - `1`: six 2 m panels lying 1.3 m above the ground in a row 14 m along
//!   the start view, left to right: a greenhouse pane (opaque frame, panes at texture alpha 0.5);
//!   the same at face alpha 0.6; an opaque texture at face alpha 0.5, then
//!   0.03, then 0.95; a texture at alpha 0.75 everywhere. As in the Legacy
//!   Viewer, the shadows are: the frame alone, solid; the frame alone,
//!   dithered; dithered; none; solid; dithered.
//! - `alpha`: the stochastic shadows (Settings::shadow_alpha, or
//!   AURORA_SHADOW_ALPHA=legacy|aurora), in a row 2 m panels lying 1.3 m
//!   above the ground, left to right: blank panes at face alpha 0.2, 0.5 and
//!   0.8; two 0.5 panes 0.3 m apart, shifted by half a pane (0.5, then 0.25
//!   where they overlap, then 0.5 of the light); a greenhouse pane (texture
//!   alpha 0.5); and a 0.5 sphere (its front and back layers: 0.25 at the
//!   centre). Legacy: flat dithered shadows, none for the greenhouse panes,
//!   layers never darker. Aurora: 0.2, 0.5 and 0.8 of the light blocked,
//!   0.25 of it left under the overlap, the greenhouse panes half dark.
//! - `dense`: 400 greenhouse panes over a 60 m square ahead, for the cost of
//!   the blended casters (AURORA_PROFILE, AURORA_BLEND_SHADOWS=0 to compare).
//!
//! From the start position, or AURORA_DEMO_POS="x,y".

use super::{BLANK_TEXTURE, HANDLE, height, prim, shape};
use aurora_net::NetEvent;
use aurora_net::objects::ObjectUpdate;
use aurora_prim::params::{LL_PCODE_PATH_LINE, LL_PCODE_PROFILE_SQUARE};
use aurora_prim::{ExtraParams, TextureEntry};
use glam::{Quat, Vec3};
use std::sync::Arc;
use uuid::Uuid;

const FIRST_ID: u32 = 86_000;
/// Opaque frame bars around four panes at alpha 128. (Not ..._0040: the HUD
/// demo's mask panel took that id, and `local_texture` serves it first, so
/// the pane showed that texture and cast a solid shadow.)
const TEX_GREENHOUSE: Uuid = Uuid::from_u128(0xDE40_7E10_0000_0000_0000_0000_0000_0042);
/// Alpha 192 (0.75) everywhere: over the 0.598 cutoff, under 0.88.
const TEX_ALPHA_75: Uuid = Uuid::from_u128(0xDE40_7E10_0000_0000_0000_0000_0000_0041);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Row,
    Alpha,
    Dense,
}

fn mode() -> Option<Mode> {
    match std::env::var("AURORA_DEMO_BLEND_SHADOWS").ok()?.trim() {
        "1" => Some(Mode::Row),
        "alpha" => Some(Mode::Alpha),
        "dense" => Some(Mode::Dense),
        _ => None,
    }
}

fn panel(id: u32, pos: Vec3, rot: Quat, texture: Uuid, alpha: f32) -> ObjectUpdate {
    let mut t = TextureEntry::default();
    for f in t.faces.iter_mut() {
        f.texture = texture;
        f.color = [0.85, 0.95, 0.9, alpha];
    }
    prim(
        id,
        pos,
        rot,
        Vec3::new(0.04, 2.0, 2.0),
        shape(LL_PCODE_PATH_LINE, LL_PCODE_PROFILE_SQUARE, 100, 0, 0),
        Arc::new(t),
        ExtraParams::default(),
        "",
    )
}

pub fn events() -> Vec<NetEvent> {
    let Some(mode) = mode() else {
        return Vec::new();
    };
    let look = Vec3::new(0.8, 0.4, 0.0).normalize();
    let across = Vec3::new(-look.y, look.x, 0.0);
    let yaw = look.y.atan2(look.x);
    let eye = std::env::var("AURORA_DEMO_POS")
        .ok()
        .map(|v| v.split(',').filter_map(|s| s.trim().parse::<f32>().ok()).collect::<Vec<_>>())
        .filter(|p| p.len() >= 2)
        .map_or(Vec3::new(132.0, 126.0, 0.0), |p| Vec3::new(p[0], p[1], 0.0));
    let upright = Quat::from_rotation_z(yaw);
    // the row: roofs lying 1.3 m above the ground, their shadows on it
    let flat = upright * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    let above = |p: Vec3, h: f32| Vec3::new(p.x, p.y, height(p.x, p.y) + h);
    let mut objects = Vec::new();
    match mode {
        Mode::Row => {
            let row = [
                (TEX_GREENHOUSE, 1.0),
                (TEX_GREENHOUSE, 0.6),
                (BLANK_TEXTURE, 0.5),
                (BLANK_TEXTURE, 0.03),
                (BLANK_TEXTURE, 0.95),
                (TEX_ALPHA_75, 1.0),
            ];
            for (k, (texture, alpha)) in row.into_iter().enumerate() {
                // (right of the view: clear of the first-launch windows)
                let p = eye + look * 14.0 - across * (4.0 + (k as f32 - 2.5) * 2.4);
                objects.push(panel(FIRST_ID + k as u32, above(p, 1.3), flat, texture, alpha));
            }
        }
        Mode::Alpha => {
            let at = |slot: f32| eye + look * 14.0 - across * (4.0 + (slot - 4.0) * 2.4);
            let mut id = FIRST_ID;
            let mut add = |objects: &mut Vec<ObjectUpdate>, p: Vec3, h: f32, texture: Uuid, alpha: f32| {
                objects.push(panel(id, above(p, h), flat, texture, alpha));
                id += 1;
            };
            for (k, alpha) in [0.2, 0.5, 0.8].into_iter().enumerate() {
                add(&mut objects, at(k as f32), 1.3, BLANK_TEXTURE, alpha);
            }
            // two 0.5 panes, the upper one shifted by half a pane
            add(&mut objects, at(3.0) - across * 0.5, 1.3, BLANK_TEXTURE, 0.5);
            add(&mut objects, at(3.0) + across * 0.5, 1.6, BLANK_TEXTURE, 0.5);
            add(&mut objects, at(4.5), 1.3, TEX_GREENHOUSE, 1.0);
            let mut t = TextureEntry::default();
            for f in t.faces.iter_mut() {
                f.texture = BLANK_TEXTURE;
                f.color = [0.85, 0.95, 0.9, 0.5];
            }
            let sphere = shape(
                aurora_prim::params::LL_PCODE_PATH_CIRCLE,
                aurora_prim::params::LL_PCODE_PROFILE_CIRCLE_HALF,
                100,
                0,
                0,
            );
            let p = at(5.5);
            objects.push(prim(
                id,
                Vec3::new(p.x, p.y, height(p.x, p.y) + 1.8),
                upright,
                Vec3::splat(1.6),
                sphere,
                Arc::new(t),
                ExtraParams::default(),
                "",
            ));
        }
        Mode::Dense => {
            for gy in 0..20 {
                for gx in 0..20 {
                    let k = gy * 20 + gx;
                    let p = eye + look * (10.0 + gx as f32 * 3.0) + across * ((gy as f32 - 9.5) * 3.0);
                    objects.push(panel(FIRST_ID + k, above(p, 1.2), upright, TEX_GREENHOUSE, 1.0));
                }
            }
        }
    }
    log::info!("demo blend shadows: {mode:?}, {} panels", objects.len());
    vec![NetEvent::ObjectUpdates { handle: HANDLE, objects }]
}

/// Pixels of this scenario's textures (RGBA8, 64×64).
pub fn texture(id: &Uuid) -> Option<(Vec<u8>, u32, u32)> {
    const N: u32 = 64;
    match *id {
        TEX_GREENHOUSE => {
            let mut px = Vec::with_capacity((N * N * 4) as usize);
            for y in 0..N {
                for x in 0..N {
                    // bars 4 texels wide on the edges and across the middle
                    let bar = |v: u32| !(4..60).contains(&v) || (30..34).contains(&v);
                    if bar(x) || bar(y) {
                        px.extend_from_slice(&[70, 82, 76, 255]);
                    } else {
                        px.extend_from_slice(&[200, 230, 220, 128]);
                    }
                }
            }
            Some((px, N, N))
        }
        TEX_ALPHA_75 => Some(([220, 200, 160, 192].repeat((N * N) as usize), N, N)),
        _ => None,
    }
}
