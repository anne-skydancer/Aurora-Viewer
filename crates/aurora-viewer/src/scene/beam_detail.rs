//! The level of detail of a caster in the projectors' shadow maps.
//!
//! A projector's light and shadow change only when the projector, or
//! something in its beam, moves: never because the camera moves, turns or
//! zooms. The camera picks the level of detail an object is drawn at in the
//! main view (`object_lod`); a caster drawn into a projector's shadow map
//! would then change shape with the camera (a pierced lampshade whose
//! pattern coarsens as the user walks away). So each object that stands in
//! a beam gets a second level, its **shadow level**, from the projector
//! alone: `lod_for` with the projector's apex as the camera, its field of
//! view as the view's, and the largest level over the maps whose frustum
//! holds the object (one range per face, `CullFace::shadow`). Only the
//! projectors' shadow bins draw it; the main view, the reflections, the
//! probes, the sun's cascades and the rain's shelter map keep the camera's.
//!
//! Deliberate difference from the Legacy Viewer, which picks levels of
//! detail for the world camera only (`LLDrawable::updateDistance` refuses any
//! other camera, indra/newview/lldrawable.cpp; `LLVOVolume::calcLOD`,
//! indra/newview/llvovolume.cpp; its spot shadow passes draw whatever level
//! the world camera left).
//!
//! The texture floor: an alpha-tested caster's base texture also decides the
//! shape of its shadow (a pierced shade), so the texels the face covers in
//! the map (`Beams::texels`) are noted as a minimum size of that texture
//! (`TextureStreamer::note_usage_beam`), from the beams alone: a shade only
//! its shadow shows still loads, and does not blur as the camera leaves.
//! Memory pressure lowers it like an in-view size. The Legacy Viewer notes
//! texture sizes for the world camera only (`processTextureStats`,
//! indra/newview/llviewertexture.cpp asserts it is not a shadow pass).
//!
//! Not covered, they keep the camera's level (nothing is requested for
//! them): rigged meshes and avatars (their shadow moves with the animation
//! every frame), flexible prims (one geometry per object), trees and grass
//! (their own factor), HUDs.

use super::{FaceDraw, GeomKey, GpuGeom, ObjGpu, lod_for};
use aurora_render::gpu_cull::{sphere_in, view_planes};
use aurora_render::types::Projector;
use glam::Vec3;

/// One assigned projector shadow map, as the renderer culls it with.
#[derive(Debug, Clone, Copy)]
struct Beam {
    /// Frustum planes of the map (`Projector::shadow_matrix`).
    planes: [[f32; 4]; 6],
    apex: Vec3,
    /// Field of view the level is computed with (radians).
    fov: f32,
    /// `Projector::half_tan` and `Projector::near`.
    half_tan: f32,
    near: f32,
    /// Index of the map in the frame's shadow table (the trace).
    slot: usize,
}

/// The projectors holding a shadow map, as of the last frame's assignment
/// (`Scene::local_lights`): one frame late and camera-free.
#[derive(Default)]
pub struct Beams {
    beams: Vec<Beam>,
    /// Texels across one map (its tile of the atlas).
    map_px: f32,
}

impl Beams {
    /// Replace the beams with those of the projectors that hold a map
    /// (`slot`, projector), the maps being `map_px` texels across.
    pub fn set(&mut self, map_px: u32, projectors: impl IntoIterator<Item = (usize, Projector)>) {
        self.beams.clear();
        self.map_px = map_px as f32;
        self.beams.extend(projectors.into_iter().map(|(slot, p)| Beam {
            slot,
            half_tan: p.half_tan,
            near: p.near,
            planes: view_planes(p.shadow_matrix).map(|v| v.to_array()),
            apex: p.origin,
            // (the narrower side, where the map has the most texels per
            // metre, as `Projector::half_tan` is)
            fov: 2.0 * p.half_tan.atan(),
        }));
    }

    /// The shadow level of a caster (sphere `origin`, `radius`): the largest
    /// level over the maps whose frustum holds it, None when none does. It
    /// reads the projectors and the object, never the camera.
    pub fn level(&self, origin: Vec3, radius: f32, lod_factor: f32) -> Option<u8> {
        self.beams
            .iter()
            .filter(|b| sphere_in(&b.planes, origin, radius))
            .map(|b| lod_for(radius, origin.distance(b.apex), lod_factor, b.fov))
            .max()
    }

    /// Texels a face of an object (sphere `origin`, `radius`) with `repeats`
    /// covers in the largest map holding it: the diameter over the width of
    /// the map at its distance, times the map's texels, lowered by the
    /// repeats like any texture size (`texture_importance::per_repeats`).
    /// None when no map holds it. Camera-free.
    pub fn texels(&self, origin: Vec3, radius: f32, repeats: f32) -> Option<f32> {
        self.beams
            .iter()
            .filter(|b| sphere_in(&b.planes, origin, radius))
            .map(|b| {
                let d = origin.distance(b.apex).max(b.near).max(0.01);
                let px = radius * self.map_px / (d * b.half_tan.max(1e-3));
                super::texture_importance::per_repeats(px, repeats)
            })
            .fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))))
    }

    /// The slots of the maps holding the sphere, in the order of the maps.
    pub fn holding(&self, origin: Vec3, radius: f32) -> impl Iterator<Item = usize> + '_ {
        self.beams
            .iter()
            .filter(move |b| sphere_in(&b.planes, origin, radius))
            .map(|b| b.slot)
    }

    pub fn is_empty(&self) -> bool {
        self.beams.is_empty()
    }

    /// The slots of the maps, in order.
    pub fn slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.beams.iter().map(|b| b.slot)
    }
}

/// A face whose shadow is alpha-tested: its base texture shapes the shadow.
/// A blended face only casts when `blend_casts` (`shadow_cull::blend_casts`).
pub fn alpha_tested(pass: super::Pass, blend_casts: bool) -> bool {
    use super::Pass;
    matches!(pass, Pass::Mask | Pass::MaskTwoSided) || (pass == Pass::Blend && blend_casts)
}

/// The radius the projectors test an object with: half the diagonal of a
/// box of its scale (at least 5 cm). It comes from the object's scale alone,
/// as the camera's level does (`SyncCtx::lod_metrics`), not from the bounds
/// of the geometry drawn, which differ from one level of detail to the next.
/// The sync stores it (`ObjGpu::beam_radius`) with the origin.
pub fn radius_of_scale(scale: Vec3) -> f32 {
    (scale.max(Vec3::splat(0.001)) * 0.5).length().max(0.05)
}

/// The level of detail of a geometry (None for the kinds without levels).
pub fn level_of(key: GeomKey) -> Option<u8> {
    match key {
        GeomKey::Prim { lod, .. } | GeomKey::Sculpt { lod, .. } | GeomKey::Mesh { lod, .. } => Some(lod),
        _ => None,
    }
}

/// An object whose shadow level is its own (see the module's list of what
/// is not).
pub fn eligible(g: &ObjGpu) -> bool {
    !g.rigged && !g.is_avatar && !g.hud
}

/// The geometry a caster of camera geometry `camera` draws into the
/// projectors' maps at `level`: the same shape at another level of detail.
/// None for the kinds that keep the camera's.
pub fn shadow_key(camera: GeomKey, level: u8) -> Option<GeomKey> {
    match camera {
        GeomKey::Prim { hash, .. } => Some(GeomKey::Prim { hash, lod: level }),
        GeomKey::Sculpt { hash, .. } => Some(GeomKey::Sculpt { hash, lod: level }),
        GeomKey::Mesh { id, .. } => Some(GeomKey::Mesh { id, lod: level }),
        _ => None,
    }
}

/// Both keys are levels of one shape (a shadow geometry is valid while its
/// object's camera geometry is of the same shape).
pub fn same_shape(a: GeomKey, b: GeomKey) -> bool {
    match (a, b) {
        (GeomKey::Prim { hash: a, .. }, GeomKey::Prim { hash: b, .. })
        | (GeomKey::Sculpt { hash: a, .. }, GeomKey::Sculpt { hash: b, .. }) => a == b,
        (GeomKey::Mesh { id: a, .. }, GeomKey::Mesh { id: b, .. }) => a == b,
        _ => false,
    }
}

/// What to do with an object's shadow geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing changes (what it draws now stays: the camera's range, or the
    /// shadow level it already holds while the new one is pending).
    Keep,
    /// Bind this geometry (ready) and write its ranges.
    Bind(GeomKey),
    /// Hold none: the face entries go back to the camera's range.
    Drop,
}

/// `held`: the shadow geometry bound; `desired`: the one the projectors call
/// for (None: the camera's level will do); `ready` / `failed`: its state.
pub fn plan(held: Option<GeomKey>, desired: Option<GeomKey>, ready: bool, failed: bool) -> Step {
    match desired {
        None => {
            if held.is_some() {
                Step::Drop
            } else {
                Step::Keep
            }
        }
        Some(k) if held == Some(k) => Step::Keep,
        Some(k) if ready => Step::Bind(k),
        // a failed build is not retried: the camera's range
        Some(_) if failed => {
            if held.is_some() {
                Step::Drop
            } else {
                Step::Keep
            }
        }
        // pending: the camera's range (or the level held) stands meanwhile
        Some(_) => Step::Keep,
    }
}

/// What a wanted shadow geometry comes to once a mesh's missing levels are
/// resolved the way the build resolves them.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolved {
    /// It draws the block the camera's level draws: the camera's range will do.
    Camera,
    /// Request and bind this key.
    Key(GeomKey),
    /// The mesh header is not known yet: nothing is asked for, the camera's
    /// range stands meanwhile.
    Wait,
}

/// A mesh asset may hold fewer than four levels: a request for another one
/// draws the nearest block it has (`MeshHeader::actual_lod`), so two keys
/// can stand for one geometry. The shadow key is resolved before it is
/// compared with the camera's, and carries the resolved level, so one block
/// is one geometry. `resolve(id, lod)` is `MeshStreamer::resolved_lod`.
pub fn resolve_level(camera: GeomKey, shadow: GeomKey, resolve: impl Fn(&uuid::Uuid, u8) -> Option<u8>) -> Resolved {
    match (camera, shadow) {
        (GeomKey::Mesh { id, lod: cam }, GeomKey::Mesh { lod, .. }) => match (resolve(&id, lod), resolve(&id, cam)) {
            (Some(s), Some(c)) if s == c => Resolved::Camera,
            (Some(s), Some(_)) => Resolved::Key(GeomKey::Mesh { id, lod: s }),
            _ => Resolved::Wait,
        },
        (cam, shadow) if cam == shadow => Resolved::Camera,
        (_, shadow) => Resolved::Key(shadow),
    }
}

/// The shadow range of the face `te_index` in `geom`: nothing is drawn for a
/// face this level does not have.
pub fn range_of(geom: &GpuGeom, te_index: u8) -> (u32, u32, i32) {
    match geom.faces.get(te_index as usize) {
        Some(Some(a)) => (a.index_offset, a.index_count, a.vertex_offset as i32),
        _ => (0, 0, 0),
    }
}

/// Write (or clear, `geom` None) the shadow range of every face.
pub fn apply(faces: &mut [FaceDraw], geom: Option<&GpuGeom>) {
    for f in faces {
        f.shadow = geom.map(|g| range_of(g, f.te_index));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_render::arena::MeshAlloc;
    use glam::{Mat4, Quat};
    use uuid::Uuid;

    /// A projector at `pos` pointing down (a prim's -Z), 60 degrees, reach 20 m.
    fn projector(pos: Vec3) -> Projector {
        Projector::new(pos, Quat::IDENTITY, Vec3::splat(0.2), 20.0, Vec3::new(1.05, 0.0, 0.0), 0)
    }

    fn beams(ps: &[Projector]) -> Beams {
        let mut b = Beams::default();
        b.set(1024, ps.iter().copied().enumerate());
        b
    }

    #[test]
    fn the_beam_level_and_floor_do_not_change_with_the_level_of_detail_bounds() {
        use super::super::sync_plan::BoxPoints;
        let beams = beams(&[projector(Vec3::new(0.0, 0.0, 3.0))]);
        let world_model = Mat4::from_scale_rotation_translation(Vec3::splat(2.0), Quat::IDENTITY, Vec3::new(0.0, 0.0, -2.0));
        // two levels of one shape: other bounds, so other centres
        let coarse = BoxPoints::of(&world_model, &world_model, Vec3::splat(-0.3), Vec3::splat(0.5), Vec3::ZERO);
        let fine = BoxPoints::of(&world_model, &world_model, Vec3::splat(-0.5), Vec3::splat(0.5), Vec3::ZERO);
        assert!(coarse.center.distance(fine.center) > 0.1);
        let radius = 1.7;
        let level = beams.level(fine.origin, radius, 1.0);
        assert!(level.is_some(), "the object stands in the beam");
        assert_eq!(beams.level(coarse.origin, radius, 1.0), level);
        let texels = beams.texels(fine.origin, radius, 1.0);
        assert!(texels.is_some());
        assert_eq!(beams.texels(coarse.origin, radius, 1.0), texels);
        // (the bounds centres would have given another size: the test can fail)
        assert_ne!(beams.texels(coarse.center, radius, 1.0), texels);
    }

    #[test]
    fn an_anchored_origin_follows_its_anchor_like_its_centre() {
        let mut g = ObjGpu {
            local_center: Vec3::new(0.1, 0.2, 0.3),
            local_origin: Vec3::new(0.0, 0.0, 0.5),
            ..Default::default()
        };
        let at = Mat4::from_rotation_translation(Quat::from_rotation_z(1.2), Vec3::new(5.0, 6.0, 7.0));
        g.follow_anchor(&at);
        assert!(g.center.distance(at.transform_point3(Vec3::new(0.1, 0.2, 0.3))) < 1e-5);
        assert!(g.origin.distance(at.transform_point3(Vec3::new(0.0, 0.0, 0.5))) < 1e-5);
        // the anchor moves: both follow, the distance between them stays
        let gap = g.center.distance(g.origin);
        g.follow_anchor(&Mat4::from_translation(Vec3::new(-9.0, 2.0, 1.0)));
        assert!((g.center.distance(g.origin) - gap).abs() < 1e-5);
    }

    #[test]
    fn the_level_reads_the_object_and_the_projectors_only() {
        // `Beams::level` takes no camera; what could leak it in is the
        // sphere it is given. That comes from the scale alone (the bounds of
        // the geometry drawn change with the camera's level of detail), so
        // two levels of one shape with different bounds give one level.
        let b = beams(&[projector(Vec3::new(0.0, 0.0, 6.0))]);
        let scale = Vec3::new(0.6, 0.5, 0.4);
        let radius = radius_of_scale(scale);
        assert!((radius - scale.length() * 0.5).abs() < 1e-6);
        let level = b.level(Vec3::new(0.3, 0.2, 2.0), radius, 2.0).expect("in the beam");
        // (a camera-level bounds radius a few percent off is not an input)
        assert_eq!(b.level(Vec3::new(0.3, 0.2, 2.0), radius_of_scale(scale), 2.0), Some(level));
        assert_eq!(radius_of_scale(Vec3::ZERO), 0.05);
    }

    #[test]
    fn the_level_is_the_largest_over_the_maps_holding_the_object() {
        let near = projector(Vec3::new(0.0, 0.0, 3.0));
        let far = projector(Vec3::new(0.0, 0.0, 18.0));
        let (center, radius) = (Vec3::new(0.0, 0.0, 1.0), 0.43);
        let (a, b) = (beams(&[near]).level(center, radius, 2.0), beams(&[far]).level(center, radius, 2.0));
        assert!(a > b, "{a:?} {b:?}");
        assert_eq!(beams(&[near, far]).level(center, radius, 2.0), a);
        assert_eq!(beams(&[far, near]).level(center, radius, 2.0), a);
        // a map that does not hold the object counts for nothing
        let beside = projector(Vec3::new(80.0, 0.0, 3.0));
        assert_eq!(beams(&[far, beside]).level(center, radius, 2.0), b);
        assert_eq!(beams(&[beside]).level(center, radius, 2.0), None);
        assert_eq!(Beams::default().level(center, radius, 2.0), None);
    }

    #[test]
    fn the_texture_floor_follows_the_beam_and_never_the_camera() {
        let b = beams(&[projector(Vec3::new(0.0, 0.0, 6.0))]);
        let (center, radius) = (Vec3::new(0.3, 0.2, 2.0), 0.43);
        let floor = b.texels(center, radius, 1.0).expect("in the beam");
        assert!(floor > 50.0, "{floor}");
        // nearer the apex, more texels; repeats lower it; outside, none
        let nearer = b.texels(Vec3::new(0.3, 0.2, 4.0), radius, 1.0).unwrap();
        assert!(nearer > floor);
        assert!(b.texels(center, radius, 4.0).unwrap() < floor);
        assert_eq!(b.texels(Vec3::new(90.0, 0.0, 2.0), radius, 1.0), None);
        assert_eq!(Beams::default().texels(center, radius, 1.0), None);
        // the largest over the maps holding it
        let far = projector(Vec3::new(0.0, 0.0, 18.0));
        let near = projector(Vec3::new(0.0, 0.0, 3.0));
        assert_eq!(
            beams(&[far, near]).texels(center, radius, 1.0),
            beams(&[near]).texels(center, radius, 1.0)
        );
        assert_eq!(beams(&[far, near]).holding(center, radius).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(beams(&[far, near]).slots().collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn only_alpha_tested_faces_get_the_floor() {
        use super::super::Pass;
        for p in [Pass::Mask, Pass::MaskTwoSided] {
            assert!(alpha_tested(p, false), "{p:?}");
        }
        // glass only when it casts
        assert!(alpha_tested(Pass::Blend, true));
        assert!(!alpha_tested(Pass::Blend, false));
        for p in [Pass::Opaque, Pass::OpaqueTwoSided, Pass::Hidden] {
            assert!(!alpha_tested(p, true), "{p:?}");
        }
    }

    #[test]
    fn only_shapes_with_levels_get_a_shadow_geometry() {
        let id = Uuid::from_u128(5);
        assert_eq!(
            shadow_key(GeomKey::Prim { hash: 9, lod: 0 }, 3),
            Some(GeomKey::Prim { hash: 9, lod: 3 })
        );
        assert_eq!(
            shadow_key(GeomKey::Sculpt { hash: 9, lod: 1 }, 2),
            Some(GeomKey::Sculpt { hash: 9, lod: 2 })
        );
        assert_eq!(shadow_key(GeomKey::Mesh { id, lod: 3 }, 0), Some(GeomKey::Mesh { id, lod: 0 }));
        for k in [
            GeomKey::Flexible { id, hash: 1, lod: 0 },
            GeomKey::Tree { species: 1, lod: 0 },
            GeomKey::Grass { id, hash: 1 },
            GeomKey::AvatarPart(2),
            GeomKey::SculptPlaceholder { hash: 1, lod: 0 },
        ] {
            assert_eq!(shadow_key(k, 3), None, "{k:?}");
        }
        assert!(same_shape(GeomKey::Prim { hash: 9, lod: 0 }, GeomKey::Prim { hash: 9, lod: 3 }));
        assert!(!same_shape(GeomKey::Prim { hash: 9, lod: 0 }, GeomKey::Prim { hash: 8, lod: 0 }));
        assert!(!same_shape(GeomKey::Prim { hash: 9, lod: 0 }, GeomKey::Sculpt { hash: 9, lod: 0 }));
    }

    #[test]
    fn rigged_avatar_and_hud_objects_keep_the_camera_level() {
        assert!(eligible(&ObjGpu::default()));
        for g in [
            ObjGpu {
                rigged: true,
                ..Default::default()
            },
            ObjGpu {
                is_avatar: true,
                ..Default::default()
            },
            ObjGpu {
                hud: true,
                ..Default::default()
            },
        ] {
            assert!(!eligible(&g));
        }
    }

    #[test]
    fn a_missing_mesh_level_resolves_like_the_build_does() {
        let id = Uuid::from_u128(5);
        let mesh = |lod| GeomKey::Mesh { id, lod };
        // an asset with levels 1 and 3 only, resolved by the header itself
        // (`MeshHeader::actual_lod`, what the build and `lod_block` use)
        let header = aurora_assets::MeshHeader {
            header_size: 10,
            lods: [None, Some((10, 20)), None, Some((30, 40))],
            skin: None,
            physics_convex: None,
            physics_mesh: None,
            version: 1,
        };
        let resolve = |_: &Uuid, lod: u8| header.actual_lod(lod as usize).map(|a| a as u8);
        // 2 draws level 1's block, so does 0: shadow 0 against camera 2 is the camera's geometry
        assert_eq!(resolve_level(mesh(2), mesh(0), resolve), Resolved::Camera);
        assert_eq!(resolve_level(mesh(1), mesh(2), resolve), Resolved::Camera);
        // another block: its key carries the resolved level
        assert_eq!(resolve_level(mesh(3), mesh(2), resolve), Resolved::Key(mesh(1)));
        assert_eq!(resolve_level(mesh(3), mesh(0), resolve), Resolved::Key(mesh(1)));
        // two requests, one geometry
        assert_eq!(resolve_level(mesh(3), mesh(2), resolve), resolve_level(mesh(3), mesh(1), resolve));
        // header unknown: wait
        assert_eq!(resolve_level(mesh(3), mesh(0), |_, _| None), Resolved::Wait);
        // not a mesh: the keys compare as they are
        let prim = |lod| GeomKey::Prim { hash: 4, lod };
        assert_eq!(resolve_level(prim(3), prim(3), resolve), Resolved::Camera);
        assert_eq!(resolve_level(prim(3), prim(1), resolve), Resolved::Key(prim(1)));
    }

    #[test]
    fn pending_geometry_keeps_the_camera_range() {
        let a = GeomKey::Prim { hash: 1, lod: 3 };
        let b = GeomKey::Prim { hash: 1, lod: 2 };
        // nothing held, the level pending: the camera range stands
        assert_eq!(plan(None, Some(a), false, false), Step::Keep);
        assert_eq!(plan(None, Some(a), true, false), Step::Bind(a));
        // a level held while the next one is pending: it stays
        assert_eq!(plan(Some(a), Some(b), false, false), Step::Keep);
        assert_eq!(plan(Some(a), Some(b), true, false), Step::Bind(b));
        assert_eq!(plan(Some(a), Some(a), true, false), Step::Keep);
        // no level wanted (the camera's will do, or no beam): back to it
        assert_eq!(plan(Some(a), None, false, false), Step::Drop);
        assert_eq!(plan(None, None, false, false), Step::Keep);
        // failed: the camera range, and the held level goes
        assert_eq!(plan(None, Some(a), false, true), Step::Keep);
        assert_eq!(plan(Some(b), Some(a), false, true), Step::Drop);
    }

    fn geom(faces: Vec<Option<MeshAlloc>>) -> GpuGeom {
        GpuGeom {
            faces,
            min: Vec3::ZERO,
            max: Vec3::ZERO,
            joint_bounds: Vec::new(),
            pick_faces: Vec::new(),
        }
    }

    fn alloc(first: u32, count: u32, base: u32) -> MeshAlloc {
        MeshAlloc {
            vertex_offset: base,
            vertex_count: 4,
            index_offset: first,
            index_count: count,
        }
    }

    #[test]
    fn the_shadow_range_survives_a_rebuild_at_the_cameras_level() {
        let shadow = geom(vec![Some(alloc(100, 30, 7)), None, Some(alloc(200, 6, 9))]);
        let face = |te: u8| {
            let mut f = super::super::tests::test_face(super::super::Pass::Opaque, false);
            f.te_index = te;
            f.cmd.index_count = 300;
            f.cmd.first_index = 50;
            f
        };
        let mut faces = vec![face(0), face(1), face(2), face(5)];
        apply(&mut faces, Some(&shadow));
        // a face the level lacks, or past it, draws nothing there
        assert_eq!(
            faces.iter().map(|f| f.shadow).collect::<Vec<_>>(),
            [Some((100, 30, 7)), Some((0, 0, 0)), Some((200, 6, 9)), Some((0, 0, 0))]
        );
        let entry = super::super::cull_face(&faces[0], 4, true);
        assert_eq!((entry.index_count, entry.first_index), (300, 50), "the camera range is kept");
        assert_eq!(entry.shadow_range(), (100, 30, 7));
        assert_ne!(entry.bits & aurora_render::gpu_cull::FACE_SHADOW_RANGE, 0);
        // the camera's level changed: new faces are built, then given the
        // range of the shadow geometry still held (`finish_faces`)
        let mut rebuilt = vec![face(0), face(1), face(2)];
        rebuilt[0].cmd.index_count = 12;
        assert!(rebuilt.iter().all(|f| f.shadow.is_none()));
        apply(&mut rebuilt, Some(&shadow));
        let entry = super::super::cull_face(&rebuilt[0], 4, true);
        assert_eq!((entry.index_count, entry.shadow_range()), (12, (100, 30, 7)));
        // no shadow geometry: the entry carries no range
        apply(&mut rebuilt, None);
        let entry = super::super::cull_face(&rebuilt[0], 4, true);
        assert_eq!((entry.shadow, entry.bits & aurora_render::gpu_cull::FACE_SHADOW_RANGE), ([0; 3], 0));
    }
}
