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
//! must stay the same over the whole dolly. Three captures, camera far, near and far
//! (`AURORA_CAPTURE_FRAMES`), show the same dots.
//!
//! `AURORA_DEMO_PROJECTOR=shade,axes` runs a second camera path instead: from
//! one reference pose, one phase per camera axis (`Phase`: dolly, truck, boom,
//! yaw, pitch, roll, zoom, close), each coming back to the reference pose and
//! holding it for a capture; `shade,close` only the reference and the close
//! phase. Both give the shade a 4096 px texture, so that the close phase can
//! make the camera ask for a finer level than the projector's map needs. The
//! trace names the phase (`phase=`) and the texture's resident level beside
//! the level the map asks for (`tex=<resident>/<asked>`).

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
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

/// The shade's texture.
const TEXTURE: u128 = 0xde7e_7e51_0000_4000_8000_0000_0000_0001;
const FIRST_LOCAL_ID: u32 = 82_100;
/// Side of the texture at full resolution.
const SIDE: u32 = 1024;
/// The same in the variants that measure the camera's own level of detail:
/// the map asks for 1024 to 2048 px of the texture, so each face repeats
/// it `AXES_REPEATS` times (the map then asks for half), with as many cells
/// fewer in the texture, which keeps the look of the dolly's shade; the
/// camera, which is capped at 2048, can ask for a finer level.
const AXES_SIDE: u32 = 4096;
const AXES_REPEATS: u32 = 2;
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
    texture_mips_of(SIDE, discard, 1)
}

/// [`texture_mips`] of a texture `full` pixels wide at level 0, with
/// `repeats` times fewer cells.
fn texture_mips_of(full: u32, discard: u32, repeats: u32) -> Vec<(u32, u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut side = (full >> discard.min(12)).max(1);
    loop {
        let mut px = Vec::with_capacity((side * side * 4) as usize);
        for y in 0..side {
            for x in 0..side {
                let (u, v) = ((x as f32 + 0.5) / side as f32, (y as f32 + 0.5) / side as f32);
                // position in the cell, from its middle, in cell units
                let (cx, cy) = (
                    (u * (COLUMNS / repeats) as f32).fract() - 0.5,
                    (v * (ROWS / repeats) as f32).fract() - 0.5,
                );
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

fn source_axes() -> DemoSource {
    DemoSource {
        has: |id| *id == texture(),
        texels: |_, discard| texture_mips_of(AXES_SIDE, discard as u32, AXES_REPEATS),
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

/// One phase of the camera path of `shade,axes`: the camera leaves the
/// reference pose along one axis, as far as the shade leaves the view (or as
/// the axis goes), and comes back to hold the reference pose for a capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Holds the reference pose: two captures with nothing between them give
    /// the noise floor.
    Reference,
    /// Along the line of sight, in then out.
    Dolly,
    /// Sideways.
    Truck,
    /// Up then down.
    Boom,
    /// Turning until the shade is behind the camera.
    Yaw,
    /// Tilting up until the shade is out of view.
    Pitch,
    /// About the line of sight.
    Roll,
    /// The field of view narrowed then widened, the camera still.
    Zoom,
    /// So close to the shade that the camera asks for a finer level of its
    /// texture than the map does.
    Close,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Reference => "reference",
            Phase::Dolly => "dolly",
            Phase::Truck => "truck",
            Phase::Boom => "boom",
            Phase::Yaw => "yaw",
            Phase::Pitch => "pitch",
            Phase::Roll => "roll",
            Phase::Zoom => "zoom",
            Phase::Close => "close",
        }
    }

    /// How far each excursion goes, in the unit of the axis: metres (dolly
    /// along the line of sight, toward the slab negative; truck, boom),
    /// radians (yaw, pitch, roll), a factor of the field of view (zoom), the
    /// fraction of the way to the shade (close). One excursion out and back
    /// per value.
    fn extremes(self) -> &'static [f32] {
        match self {
            Phase::Reference => &[],
            Phase::Dolly => &[-9.0, 14.0],
            Phase::Truck => &[30.0],
            Phase::Boom => &[60.0, -6.0],
            Phase::Yaw => &[std::f32::consts::PI],
            Phase::Pitch => &[75.0 * std::f32::consts::PI / 180.0],
            Phase::Roll => &[std::f32::consts::FRAC_PI_2],
            Phase::Zoom => &[0.35, 1.7],
            Phase::Close => &[1.0],
        }
    }

    /// Frames of the phase: its excursions, then the hold (the whole of it
    /// for the reference).
    pub fn frames(self) -> u64 {
        match self {
            Phase::Reference => REFERENCE_FRAMES,
            p => EXCURSION * p.extremes().len() as u64 + HOLD,
        }
    }
}

/// The phases of `shade,axes`, in order.
pub const AXES: [Phase; 9] = [
    Phase::Reference,
    Phase::Dolly,
    Phase::Truck,
    Phase::Boom,
    Phase::Yaw,
    Phase::Pitch,
    Phase::Roll,
    Phase::Zoom,
    Phase::Close,
];

/// A camera pose of the path: the eye, the point looked at, the roll about
/// the line of sight and the field of view as a factor of the setting's.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub eye: Vec3,
    pub target: Vec3,
    pub roll: f32,
    pub fov_scale: f32,
}

/// Frames the reference pose is held at the start (two captures).
const REFERENCE_FRAMES: u64 = 200;
/// An excursion: 80 frames out, 40 at the far end, 80 back.
const RAMP: u64 = 80;
const DWELL: u64 = 40;
const EXCURSION: u64 = 2 * RAMP + DWELL;
/// The reference pose held after the excursions of a phase.
const HOLD: u64 = 100;
/// Distance of the reference eye from the slab, on the dolly's line of sight.
const REFERENCE_DISTANCE: f32 = 14.0;
/// Distance of the close eye from the shade's middle.
const CLOSE_DISTANCE: f32 = 1.2;

/// The phase and the frame within it at `frame`: before `START` the first
/// frame of the reference, after the last phase its last.
fn locate(phases: &[Phase], frame: u64) -> (Phase, u64) {
    let mut t = frame.saturating_sub(START);
    for &p in phases {
        if t < p.frames() {
            return (p, t);
        }
        t -= p.frames();
    }
    (Phase::Reference, REFERENCE_FRAMES - 1)
}

/// Frame at which phase `index` of `phases` starts.
pub fn phase_start(phases: &[Phase], index: usize) -> u64 {
    START + phases[..index].iter().map(|p| p.frames()).sum::<u64>()
}

/// Frames to capture: the reference twice (the noise floor), then the last
/// frames of each phase's hold. `AURORA_CAPTURE_FRAMES` for the path.
pub fn capture_frames(phases: &[Phase]) -> Vec<u64> {
    let mut out = Vec::new();
    for (i, &p) in phases.iter().enumerate() {
        let start = phase_start(phases, i);
        let end = start + p.frames();
        if p == Phase::Reference {
            out.extend([start + REFERENCE_FRAMES / 2 - 10, end - 10]);
        } else {
            out.push(end - 10);
        }
    }
    out
}

/// How far into its excursions a phase is at frame `t` of it: the index of
/// the extreme and how far toward it, 0 to 1 eased; (0, 0) in the hold.
fn excursion(phase: Phase, t: u64) -> (usize, f32) {
    let n = phase.extremes().len() as u64;
    if t >= n * EXCURSION {
        return (0, 0.0);
    }
    let ease = |k: f32| k * k * (3.0 - 2.0 * k);
    let (i, u) = ((t / EXCURSION) as usize, t % EXCURSION);
    let k = if u < RAMP {
        ease(u as f32 / RAMP as f32)
    } else if u < RAMP + DWELL {
        1.0
    } else {
        ease(1.0 - (u - RAMP - DWELL) as f32 / RAMP as f32)
    };
    (i, k)
}

/// The slab's middle, which the reference pose looks at.
fn slab() -> Vec3 {
    Vec3::new(CENTER.0, CENTER.1, floor_at(CENTER.0, CENTER.1) + 0.1)
}

/// The shade's middle.
fn shade_middle() -> Vec3 {
    Vec3::new(CENTER.0, CENTER.1, slab().z + HEIGHT - 0.4)
}

/// Direction from a point toward an eye at `elevation` above it, from the
/// south-west as the dolly's.
fn line_of_sight(elevation: f32) -> Vec3 {
    let azimuth = 215f32.to_radians();
    Vec3::new(elevation.cos() * azimuth.cos(), elevation.cos() * azimuth.sin(), elevation.sin())
}

/// The camera at `frame` of a path of `phases`.
pub fn axes_pose(phases: &[Phase], frame: u64) -> Pose {
    let (phase, t) = locate(phases, frame);
    let (i, k) = excursion(phase, t);
    let extreme = phase.extremes().get(i).copied().unwrap_or(0.0);
    let value = extreme * k;
    let target = slab();
    let dir = line_of_sight(38f32.to_radians());
    let eye = target + dir * REFERENCE_DISTANCE;
    let forward = (target - eye).normalize();
    let right = forward.cross(Vec3::Z).normalize();
    let mut pose = Pose {
        eye,
        target,
        roll: 0.0,
        fov_scale: 1.0,
    };
    match phase {
        Phase::Reference => {}
        Phase::Dolly => pose.eye = target + dir * (REFERENCE_DISTANCE + value),
        Phase::Truck => {
            pose.eye += right * value;
            pose.target += right * value;
        }
        Phase::Boom => {
            pose.eye += Vec3::Z * value;
            pose.target += Vec3::Z * value;
        }
        Phase::Yaw => pose.target = eye + Quat::from_axis_angle(Vec3::Z, value) * (target - eye),
        Phase::Pitch => pose.target = eye + Quat::from_axis_angle(right, value) * (target - eye),
        Phase::Roll => pose.roll = value,
        Phase::Zoom => pose.fov_scale = 1.0 + (extreme - 1.0) * k,
        Phase::Close => {
            let near = shade_middle() + line_of_sight(15f32.to_radians()) * CLOSE_DISTANCE;
            pose.eye = eye.lerp(near, value);
            pose.target = target.lerp(shade_middle(), value);
        }
    }
    pose
}

/// What `AURORA_DEMO_PROJECTOR` asked of this scenario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Variant {
    /// `shade`: the dolly, a 1024 px texture.
    Dolly,
    /// `shade,axes` or `shade,close`: a camera path of phases, a 4096 px
    /// texture.
    Path(Vec<Phase>),
}

impl Variant {
    /// None when the value is not this scenario's.
    pub fn parse(value: &str) -> Option<Variant> {
        let mut parts = value.split(',').map(str::trim);
        if parts.next()? != "shade" {
            return None;
        }
        match parts.next() {
            None => Some(Variant::Dolly),
            Some("axes") => Some(Variant::Path(AXES.to_vec())),
            Some("close") => Some(Variant::Path(vec![Phase::Reference, Phase::Close])),
            Some(_) => None,
        }
    }
}

fn variant() -> Option<&'static Variant> {
    static VARIANT: OnceLock<Option<Variant>> = OnceLock::new();
    VARIANT
        .get_or_init(|| Variant::parse(&std::env::var("AURORA_DEMO_PROJECTOR").ok()?))
        .as_ref()
}

/// The camera of the scenario at `frame`, when it is running: the dolly's
/// or a path's.
pub fn scenario_pose(frame: u64) -> Option<Pose> {
    Some(match variant()? {
        Variant::Dolly => {
            let (eye, target) = camera(frame);
            Pose {
                eye,
                target,
                roll: 0.0,
                fov_scale: 1.0,
            }
        }
        Variant::Path(phases) => axes_pose(phases, frame),
    })
}

/// The name of the phase of a path at `frame`; None for the dolly.
pub fn phase_name(frame: u64) -> Option<&'static str> {
    match variant()? {
        Variant::Path(phases) => Some(locate(phases, frame).0.name()),
        Variant::Dolly => None,
    }
}

pub struct ProjectorShadeDemo {
    sent: bool,
}

impl ProjectorShadeDemo {
    pub fn from_env() -> Option<Self> {
        variant().map(|_| ProjectorShadeDemo { sent: false })
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
        textures.demo_source = Some(if matches!(variant(), Some(Variant::Path(_))) {
            source_axes()
        } else {
            source()
        });
        // held by the scenario, showing its first pixels as a download
        // would; the streamer then asks for the level it needs
        let _ = textures.acquire(renderer, texture(), TexSource::Asset);
        let pool = textures.staging.clone();
        let path = matches!(variant(), Some(Variant::Path(_)));
        let (full, repeats) = if path { (AXES_SIDE, AXES_REPEATS) } else { (SIDE, 1) };
        jobs.spawn_many(vec![0u32], move |_| {
            texture_result(
                texture(),
                FIRST_LEVEL,
                texture_mips_of(full, FIRST_LEVEL as u32, repeats),
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
            f.scale_s = repeats as f32;
            f.scale_t = repeats as f32;
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
        match variant() {
            Some(Variant::Path(phases)) => log::info!(
                "demo projector shade: a pierced shade round a projector, camera path {:?} from frame {START}; AURORA_CAPTURE_FRAMES={:?}",
                phases.iter().map(|p| p.name()).collect::<Vec<_>>(),
                capture_frames(phases)
            ),
            _ => log::info!("demo projector shade: a pierced shade round a projector, camera dolly from frame {START}"),
        }
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

    #[test]
    fn every_phase_comes_back_to_the_reference_pose() {
        let reference = axes_pose(&AXES, START);
        for (i, &p) in AXES.iter().enumerate() {
            let (start, end) = (phase_start(&AXES, i), phase_start(&AXES, i) + p.frames());
            // the hold, from its first frame to its last
            let hold = if p == Phase::Reference { 0 } else { HOLD };
            for frame in [end - hold, end - 1] {
                let pose = axes_pose(&AXES, frame);
                assert!((pose.eye - reference.eye).length() < 1e-3, "{p:?} at {frame}: {:?}", pose.eye);
                assert!((pose.target - reference.target).length() < 1e-3, "{p:?} at {frame}");
                assert!(pose.roll.abs() < 1e-5 && (pose.fov_scale - 1.0).abs() < 1e-5, "{p:?} at {frame}");
            }
            if p != Phase::Reference {
                // and it does leave it, by the axis of the phase
                let far = (start..end - HOLD)
                    .map(|f| axes_pose(&AXES, f))
                    .map(|q| {
                        (q.eye - reference.eye).length() + (q.target - reference.target).length() + q.roll.abs() + (q.fov_scale - 1.0).abs()
                    })
                    .fold(0.0, f32::max);
                assert!(far > 0.5, "{p:?} stays at the reference: {far}");
            }
        }
        // before the path and after it, the reference
        let last = AXES.iter().map(|p| p.frames()).sum::<u64>() + START;
        for frame in [0, START - 1, last, last + 500] {
            let pose = axes_pose(&AXES, frame);
            assert!((pose.eye - reference.eye).length() < 1e-3 && pose.roll == 0.0);
        }
    }

    #[test]
    fn each_axis_moves_only_its_own() {
        let at = |phase: Phase, k: f32| {
            let i = AXES.iter().position(|p| *p == phase).unwrap();
            let t = phase_start(&AXES, i) + (RAMP as f32 * k) as u64;
            axes_pose(&AXES, t.max(phase_start(&AXES, i) + RAMP))
        };
        let reference = axes_pose(&AXES, START);
        let forward = (reference.target - reference.eye).normalize();
        // (at the far end of the first excursion of each)
        let dolly = at(Phase::Dolly, 1.0);
        assert!(((dolly.target - dolly.eye).normalize() - forward).length() < 1e-4);
        assert!((dolly.eye - reference.eye).dot(forward).abs() > 8.0);
        let truck = at(Phase::Truck, 1.0);
        assert!(((truck.target - truck.eye).normalize() - forward).length() < 1e-4);
        assert!((truck.eye - reference.eye).dot(forward).abs() < 1e-3);
        let boom = at(Phase::Boom, 1.0);
        assert!((boom.eye - reference.eye).x.abs() < 1e-4 && (boom.eye - reference.eye).z > 50.0);
        let yaw = at(Phase::Yaw, 1.0);
        assert_eq!(yaw.eye, reference.eye);
        // (turned about the vertical: its horizontal bearing is the opposite one)
        let flat = |v: Vec3| Vec3::new(v.x, v.y, 0.0).normalize();
        assert!(flat(yaw.target - yaw.eye).dot(flat(forward)) < -0.999, "the shade is behind");
        let pitch = at(Phase::Pitch, 1.0);
        assert_eq!(pitch.eye, reference.eye);
        let tilt = (pitch.target - pitch.eye).normalize().dot(forward).acos().to_degrees();
        assert!((tilt - 75.0).abs() < 0.5, "{tilt}");
        assert!((pitch.target - pitch.eye).z > forward.z, "tilted up");
        let roll = at(Phase::Roll, 1.0);
        assert_eq!((roll.eye, roll.target), (reference.eye, reference.target));
        assert!((roll.roll - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        let zoom = at(Phase::Zoom, 1.0);
        assert_eq!((zoom.eye, zoom.target, zoom.roll), (reference.eye, reference.target, 0.0));
        assert!((zoom.fov_scale - 0.35).abs() < 1e-5);
        // the second excursion goes the other way
        let i = AXES.iter().position(|p| *p == Phase::Zoom).unwrap();
        let wide = axes_pose(&AXES, phase_start(&AXES, i) + EXCURSION + RAMP);
        assert!((wide.fov_scale - 1.7).abs() < 1e-5);
        // the close phase ends at the shade, at CLOSE_DISTANCE from it
        let close = at(Phase::Close, 1.0);
        assert!((close.target - shade_middle()).length() < 1e-3);
        assert!(((close.eye - shade_middle()).length() - CLOSE_DISTANCE).abs() < 1e-3);
    }

    #[test]
    fn the_phases_are_named_and_the_variants_parse() {
        let names: Vec<_> = AXES.iter().map(|p| p.name()).collect();
        assert_eq!(
            names,
            ["reference", "dolly", "truck", "boom", "yaw", "pitch", "roll", "zoom", "close"]
        );
        assert_eq!(locate(&AXES, phase_start(&AXES, 3) + 7), (Phase::Boom, 7));
        assert_eq!(Variant::parse("shade"), Some(Variant::Dolly));
        assert_eq!(Variant::parse("shade,axes"), Some(Variant::Path(AXES.to_vec())));
        assert_eq!(
            Variant::parse("shade,close"),
            Some(Variant::Path(vec![Phase::Reference, Phase::Close]))
        );
        assert_eq!(Variant::parse("cluster"), None);
        assert_eq!(Variant::parse("shade,other"), None);
        // two captures of the reference, then one per phase, each in its hold
        let frames = capture_frames(&AXES);
        assert_eq!(frames.len(), AXES.len() + 1);
        assert!(frames.windows(2).all(|w| w[0] < w[1]));
        for (i, &p) in AXES.iter().enumerate().skip(1) {
            // (the last frames of the phase, where its pose is the reference)
            assert_eq!(locate(&AXES, frames[i + 1]).0, p);
            assert!(frames[i + 1] >= phase_start(&AXES, i) + p.frames() - HOLD);
        }
    }
}
