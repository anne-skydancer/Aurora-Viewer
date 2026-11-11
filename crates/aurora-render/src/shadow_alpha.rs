//! Semi-transparent shadows, Legacy or Aurora.
//!
//! **Legacy** (the default) is `fs_shadow` of object.wgsl, untouched: a
//! blended caster casts nothing under 5 % opacity, every other texel column
//! under 88 % (59.8 % for rigged faces), a window pane (texture alpha under
//! 0.598) nothing, and two layers never darken each other. Port of
//! LLPipeline::renderAlphaObjects (indra/newview/pipeline.cpp, originally
//! LGPL 2.1).
//!
//! **Aurora** casts in proportion to the opacity. A blended face writes
//! `R = floor(4 alpha + xi)` of the 4 texels of each 2x2 block of the shadow
//! map, a random R-subset, so the map holds the opacity as a coverage that
//! the receivers' filter averages back, and independent layers multiply
//! (two faces of 0.5 leave 0.25 of the light). The rule is the stochastic
//! coverage rule of the order-independent transparency (PR #3291,
//! `st_hash` / `st_mask` of object.wgsl) at S = 4 samples with the 2x2 block
//! standing for the samples; it is copied in `shaders/shadow_alpha.wgsl` with
//! an `sst_` prefix and pinned to the same golden values by the tests below.
//!
//! - The blocks are anchored in the world: a cascade's cell grid does not
//!   move while its anchor, sun and radius stay (`snap_to_texels`,
//!   `cascade_origin`), so a shadow does not crawl as the camera moves.
//!   A projector's map has its own frame.
//! - The seed is the face (its draw-record slot), the triangle
//!   (`PRIMITIVE_INDEX` when the device has it) and the side, rotated with
//!   the frame only under TAA or FSR in the plain main view (`frame_seed`).
//! - Masked faces keep their cutoff, opaque ones are unchanged.
//! - Cascades and projector maps use Aurora, the probes stay Legacy.
//!
//! The rule lives in a second shader module (common, object and env, then
//! `shadow_alpha.wgsl`) and one extra pipeline, built on a worker thread the
//! first time the mode is Aurora (`AuroraCaster`): Legacy is drawn until it
//! is ready, and for the session if it cannot be built.

use glam::{IVec2, Mat4, Vec3};
use std::sync::mpsc::{Receiver, TryRecvError};

/// How the blended faces cast shadows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShadowAlpha {
    /// The Legacy Viewer's rule (`fs_shadow`).
    #[default]
    Legacy,
    /// In proportion to the opacity, layers adding up.
    Aurora,
}

/// The mode for an `AURORA_SHADOW_ALPHA` value (`legacy` or `aurora`,
/// whatever the setting; anything else, or none, leaves it to the setting).
pub(crate) fn mode_with(env: Option<&str>, setting: ShadowAlpha) -> ShadowAlpha {
    match env.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("legacy") => ShadowAlpha::Legacy,
        Some(v) if v.eq_ignore_ascii_case("aurora") => ShadowAlpha::Aurora,
        _ => setting,
    }
}

/// `AURORA_SHADOW_ALPHA` read once, applied to the setting.
pub(crate) fn mode(setting: ShadowAlpha) -> ShadowAlpha {
    static VALUE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let env = VALUE.get_or_init(|| std::env::var("AURORA_SHADOW_ALPHA").ok());
    mode_with(env.as_deref(), setting)
}

/// `ShadowStParams` of shadow_alpha.wgsl (group 3, binding 2), one per
/// shadow map.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ShadowStU {
    /// Where the world-anchored cells start in the map, in shadow texels.
    pub origin: [i32; 2],
    pub frame_seed: u32,
    /// Bit 0: the seed rotates with the frame.
    pub flags: u32,
}

/// The frame seed and flags of a map: the frame number modulo 8 and bit 0
/// when `rotating` (TAA or FSR in the plain main view: not a snapshot, a
/// photo tile or a cut-out), else both 0 so that the pattern is fixed.
pub(crate) fn frame_seed(frame_index: u64, rotating: bool) -> (u32, u32) {
    if rotating { ((frame_index & 7) as u32, 1) } else { (0, 0) }
}

/// A cascade's view translated so that the world point `anchor` lands on a
/// whole texel (no shimmering as the camera moves), with the texel count `n`
/// of that translation. A pixel of the cascade's tile is then
/// `floor(R(p - anchor) / texel) + n + res / 2` along x (and the mirror along
/// y), whatever the camera does while the anchor, sun and radius stay.
pub(crate) fn snap_to_texels(view: Mat4, anchor: Vec3, texel: f32) -> (Mat4, IVec2) {
    let origin = view.transform_point3(anchor);
    let n = IVec2::new((origin.x / texel).round() as i32, (origin.y / texel).round() as i32);
    let snapped = Vec3::new(n.x as f32 * texel, n.y as f32 * texel, origin.z);
    (Mat4::from_translation(snapped - origin) * view, n)
}

/// Where the world-anchored cells of a cascade start in the atlas, for its
/// tile corner, its resolution `res` (even) and the `n` of `snap_to_texels`:
/// `cell = floor(clip.xy) - origin`.
pub(crate) fn cascade_origin(tile: (u32, u32), res: u32, n: IVec2) -> [i32; 2] {
    [tile.0 as i32 + (res / 2) as i32 + n.x, tile.1 as i32 + (res / 2) as i32 - n.y]
}

/// The WGSL of the Aurora module: `enable primitive_index;` when `prim`, then
/// the scene's common, object and env code, then `shadow_alpha.wgsl` with
/// the triangle index in the seed when `prim`.
pub(crate) fn source_with(prim: bool) -> String {
    let rule = include_str!("shaders/shadow_alpha.wgsl");
    let rule = if prim {
        rule.replace("/*SST_PRIM_PARAM*/", "@builtin(primitive_index) prim_id: u32,")
            .replace("/*SST_PRIM*/0u", "prim_id")
    } else {
        rule.replace("/*SST_PRIM_PARAM*/", "")
    };
    format!(
        "{}{}\n{}\n{}\n{}",
        if prim { "enable primitive_index;\n" } else { "" },
        crate::common_shader_source(),
        crate::water_shader_source(&["object.wgsl"], include_str!("shaders/object.wgsl")),
        crate::water_shader_source(&["env.wgsl"], include_str!("shaders/env.wgsl")),
        rule
    )
}

enum State {
    Idle,
    Building(Receiver<Option<wgpu::RenderPipeline>>),
    Ready(wgpu::RenderPipeline),
    /// Could not be built: Legacy for the rest of the session.
    Failed,
}

/// The Aurora caster's pipeline, made on first use.
pub(crate) struct AuroraCaster {
    state: State,
}

impl AuroraCaster {
    pub fn new() -> Self {
        Self { state: State::Idle }
    }

    /// The pipeline when it is ready. The first call starts building it on a
    /// worker thread (`layouts`: records, textures, shadow), so the frame
    /// never waits: `None` means "draw Legacy".
    pub fn get(&mut self, device: &wgpu::Device, layouts: [&wgpu::BindGroupLayout; 3]) -> Option<wgpu::RenderPipeline> {
        if matches!(self.state, State::Idle) {
            self.state = Self::start(device, layouts);
        }
        if let State::Building(done) = &self.state {
            match done.try_recv() {
                Ok(Some(pipeline)) => self.state = State::Ready(pipeline),
                Ok(None) => self.state = State::Failed,
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    log::warn!("semi-transparent shadows: the Aurora caster's worker stopped, Legacy kept");
                    self.state = State::Failed;
                }
            }
        }
        match &self.state {
            State::Ready(pipeline) => Some(pipeline.clone()),
            _ => None,
        }
    }

    fn start(device: &wgpu::Device, layouts: [&wgpu::BindGroupLayout; 3]) -> State {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let device_w = device.clone();
        let [records, tex, shadow] = layouts.map(Clone::clone);
        let spawned = std::thread::Builder::new().name("aurora-shadow-alpha".into()).spawn(move || {
            let _ = tx.send(build(&device_w, &records, &tex, &shadow));
        });
        match spawned {
            Ok(_) => State::Building(rx),
            Err(e) => {
                log::warn!("semi-transparent shadows: cannot start the worker ({e}), building on this thread");
                match build(device, layouts[0], layouts[1], layouts[2]) {
                    Some(p) => State::Ready(p),
                    None => State::Failed,
                }
            }
        }
    }
}

/// The module and pipeline; `None` (said once in the log) when the device
/// refuses them.
fn build(
    device: &wgpu::Device,
    records: &wgpu::BindGroupLayout,
    tex: &wgpu::BindGroupLayout,
    shadow: &wgpu::BindGroupLayout,
) -> Option<wgpu::RenderPipeline> {
    let started = std::time::Instant::now();
    let prim = device.features().contains(wgpu::Features::PRIMITIVE_INDEX);
    if !prim {
        log::info!("semi-transparent shadows: no PRIMITIVE_INDEX, the seed is the face and its side");
    }
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadow alpha aurora"),
        source: wgpu::ShaderSource::Wgsl(source_with(prim).into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadow alpha aurora"),
        bind_group_layouts: &[None, Some(records), Some(tex), Some(shadow)],
        immediate_size: 0,
    });
    let pipeline = crate::renderer::shadow_alpha_pipeline(device, &module, &layout);
    if let Some(e) = pollster::block_on(scope.pop()) {
        log::warn!("semi-transparent shadows: the Aurora caster could not be built, Legacy kept: {e}");
        return None;
    }
    log::info!("semi-transparent shadows: the Aurora caster built in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
    Some(pipeline)
}

// ---------------------------------------------------------------- mirror
// The shader's rule in Rust, f32 operation for operation: the tests and the
// GPU test hold the two together.

#[cfg(test)]
mod mirror {
    pub fn hash(v: u32) -> u32 {
        let mut x = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
        x = ((x >> ((x >> 28) + 4)) ^ x).wrapping_mul(277_803_737);
        (x >> 22) ^ x
    }

    pub fn seed(tie: u32, prim: u32, frame_seed: u32, rotate: bool) -> u32 {
        let mut s = hash(hash(tie.wrapping_mul(0x9e37_79b9).wrapping_add(0x7f4a_7c15)) ^ hash(prim.wrapping_add(0x632b_e5ab)));
        if rotate {
            s = hash(s.wrapping_add(frame_seed));
        }
        s
    }

    fn unit(v: u32) -> f32 {
        (v >> 8) as f32 / 16_777_216.0
    }

    /// The dither of a block `(px, py)` (16 bits each) for a seed.
    pub fn dither(px: u32, py: u32, seed: u32) -> f32 {
        let pv = unit(hash(px | (py << 16)));
        let x = pv + unit(seed);
        x - x.floor()
    }

    /// The subset pick of a block for a seed.
    pub fn pick(px: u32, py: u32, seed: u32) -> f32 {
        unit(hash(seed ^ hash(px | (py << 16)) ^ 0x5bd1_e995))
    }

    /// The 4-bit subsets by size R: entries in popcount order, the first
    /// entry of each R and the count (the shader's packed constants).
    pub fn subsets() -> ([u32; 16], [u32; 5], [u32; 5]) {
        let mut entries = [0u32; 16];
        let (mut off, mut cnt) = ([0u32; 5], [0u32; 5]);
        let mut i = 0;
        for r in 0..=4u32 {
            off[r as usize] = i as u32;
            for m in 0..16u32 {
                if m.count_ones() == r {
                    entries[i] = m;
                    i += 1;
                    cnt[r as usize] += 1;
                }
            }
        }
        (entries, off, cnt)
    }

    /// Does the face of `seed` and alpha `a` cast the texel `cell`?
    pub fn keep(cell: (i32, i32), seed: u32, a: f32) -> bool {
        let (bx, by) = (((cell.0 >> 1) as u32) & 0xffff, ((cell.1 >> 1) as u32) & 0xffff);
        let r = ((a * 4.0 + dither(bx, by, seed)).floor() as u32).min(4);
        let (entries, off, cnt) = subsets();
        let n = cnt[r as usize];
        let i = off[r as usize] + ((pick(bx, by, seed) * n as f32) as u32).min(n - 1);
        let bit = (cell.0 & 1) as u32 | (((cell.1 & 1) as u32) << 1);
        entries[i as usize] & (1 << bit) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::mirror::*;
    use super::*;

    fn wgsl() -> String {
        include_str!("shaders/shadow_alpha.wgsl").replace("\r\n", "\n")
    }

    /// The same values as PR #3291's `st_hash`, seed, dither and pick
    /// (oit_stochastic.rs and object.wgsl), computed from its formulas.
    #[test]
    fn hash_seed_and_dither_match_the_stochastic_rule_of_pr_3291() {
        assert_eq!(hash(0), 0x07bb_2fe2);
        assert_eq!(hash(1), 0xa8be_ea3c);
        assert_eq!(hash(7), 0x7e67_121c);
        assert_eq!(hash(0xdead_beef), 0x6729_9972);
        assert_eq!(seed(5, 3, 0, false), 0xb56d_4236);
        assert_eq!(seed(5, 3, 2, true), 0x1866_f6d6);
        assert_eq!(seed(123_456, 0, 0, false), 0x416e_f572);
        let s = seed(5, 3, 0, false);
        assert_eq!(dither(0, 0, s), 0.738_898_336_887_359_6_f64 as f32);
        assert_eq!(pick(0, 0, s), 0.226_074_159_145_355_22_f64 as f32);
        assert_eq!(dither(17, 42, s), 0.339_648_485_183_715_8_f64 as f32);
        assert_eq!(pick(17, 42, s), 0.839_809_000_492_096_f64 as f32);
        assert_eq!(dither(65_535, 1, s), 0.754_177_093_505_859_4_f64 as f32);
        assert_eq!(pick(65_535, 1, s), 0.713_838_696_479_797_4_f64 as f32);
    }

    /// The shader's hash is #3291's, character for character.
    #[test]
    fn the_shader_hash_is_the_one_of_pr_3291() {
        let squash = |s: &str| s.split_whitespace().collect::<String>();
        let text = wgsl();
        let start = text.find("fn sst_hash").expect("sst_hash");
        let end = start + text[start..].find("\n}").expect("end") + 2;
        let body = squash(&text[start..end].replace("sst_hash", "st_hash"));
        let theirs = squash(
            "fn st_hash(v: u32) -> u32 {
                var x = v * 747796405u + 2891336453u;
                x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
                return (x >> 22u) ^ x;
            }",
        );
        assert_eq!(body, theirs);
    }

    fn constant(text: &str, name: &str) -> u32 {
        let head = format!("const {name}: u32 = 0x");
        let start = text.find(&head).expect(name) + head.len();
        let hex: String = text[start..].chars().take_while(char::is_ascii_hexdigit).collect();
        u32::from_str_radix(&hex, 16).expect("hex")
    }

    #[test]
    fn the_packed_subset_tables_are_the_generated_ones() {
        let text = wgsl();
        let (entries, off, cnt) = subsets();
        let pack = |v: &[u32]| v.iter().enumerate().fold(0u32, |a, (i, &n)| a | (n << (4 * i)));
        assert_eq!(constant(&text, "SST_LO"), pack(&entries[..8]));
        assert_eq!(constant(&text, "SST_HI"), pack(&entries[8..]));
        assert_eq!(constant(&text, "SST_OFF"), pack(&off));
        assert_eq!(constant(&text, "SST_CNT"), pack(&cnt));
        for r in 0..=4usize {
            for k in 0..cnt[r] as usize {
                assert_eq!(entries[off[r] as usize + k].count_ones(), r as u32);
            }
            assert!(off[r] + cnt[r] <= 16);
        }
        assert_eq!(cnt.iter().sum::<u32>(), 16);
    }

    /// Fraction of the texels of a `size` x `size` region that no layer
    /// casts, for layers `(tie, prim, back, alpha)`.
    fn visibility(layers: &[(u32, u32, u32, f32)], size: i32) -> f32 {
        let mut lit = 0;
        for y in 0..size {
            for x in 0..size {
                let hit = layers.iter().any(|&(tie, prim, back, a)| keep((x, y), seed(tie, prim * 2 + back, 0, false), a));
                lit += i32::from(!hit);
            }
        }
        lit as f32 / (size * size) as f32
    }

    #[test]
    fn coverage_follows_the_opacity_and_layers_multiply() {
        let near = |v: f32, want: f32| assert!((v - want).abs() < 0.03, "{v} vs {want}");
        near(visibility(&[(1, 0, 0, 0.5)], 128), 0.50);
        near(visibility(&[(1, 0, 0, 0.3)], 128), 0.70);
        near(visibility(&[(1, 0, 0, 0.8)], 128), 0.20);
        assert_eq!(visibility(&[(1, 0, 0, 0.0)], 128), 1.0);
        assert_eq!(visibility(&[(1, 0, 0, 1.0)], 128), 0.0);
        // two faces
        near(visibility(&[(1, 0, 0, 0.5), (2, 0, 0, 0.5)], 128), 0.25);
        // two triangles of one face
        near(visibility(&[(1, 0, 0, 0.5), (1, 1, 0, 0.5)], 128), 0.25);
        // the front and the back of one triangle (no PRIMITIVE_INDEX)
        near(visibility(&[(1, 0, 0, 0.5), (1, 0, 1, 0.5)], 128), 0.25);
    }

    #[test]
    fn a_negative_cell_is_not_another_pattern() {
        // the 16-bit masking keeps x and y apart in the block hash
        let s = seed(9, 0, 0, false);
        let mut hits = 0;
        for y in -32..32 {
            for x in -32..32 {
                hits += i32::from(keep((x, y), s, 0.5));
            }
        }
        assert_eq!(hits, 64 * 64 / 2);
        // the pattern repeats every 2^17 texels, as documented
        assert_eq!(keep((5, 7), s, 0.4), keep((5 + (1 << 17), 7 + (1 << 17)), s, 0.4));
    }

    /// The cell of a world point in the cascade of a camera at `eye`.
    fn cell(eye: Vec3, p: Vec3, radius: f32, res: u32, tile: (u32, u32)) -> (i32, i32) {
        let texel = 2.0 * radius / res as f32;
        let anchor = Vec3::new(40.0, 24.0, 8.0);
        let view = glam::camera::rh::view::look_at_mat4(eye, eye + Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
        let (view, n) = snap_to_texels(view, anchor, texel);
        let proj = glam::camera::rh::proj::directx::orthographic(-radius, radius, -radius, radius, 0.1, 100.0);
        let c = (proj * view).project_point3(p);
        let half = res as f32 / 2.0;
        let (px, py) = ((c.x * half + half).floor() as i32 + tile.0 as i32, (half - c.y * half).floor() as i32 + tile.1 as i32);
        let o = cascade_origin(tile, res, n);
        (px - o[0], py - o[1])
    }

    #[test]
    fn the_cells_stay_in_the_world_when_the_camera_moves() {
        let (radius, res) = (8.0, 1024);
        let mut points = Vec::new();
        for i in 0..100u32 {
            let h = hash(i);
            let c = |s: u32| ((h >> s) & 0xfff) as f32 / 128.0 + 1.0 / 256.0;
            points.push(Vec3::new(30.0 + c(0), 14.0 + c(8) / 2.0, 2.0 + c(16) / 8.0));
        }
        // two camera positions that snap to the same anchor cell
        let (e1, e2) = (Vec3::new(40.0, 24.0, 20.0), Vec3::new(40.0 + 0.4, 24.0 - 0.3, 20.0));
        for tile in [(0, 0), (1024, 0), (0, 1024)] {
            for &p in &points {
                let (a, b) = (cell(e1, p, radius, res, tile), cell(e2, p, radius, res, tile));
                assert_eq!(a, b, "{p:?} tile {tile:?}");
            }
        }
        assert_eq!(cascade_origin((0, 0), 1024, IVec2::new(3, -2)), [515, 514]);
        assert_eq!(cascade_origin((1024, 0), 1024, IVec2::new(0, 0)), [1536, 512]);
        assert_eq!(cascade_origin((0, 1024), 1024, IVec2::new(-1, 1)), [511, 1535]);
    }

    #[test]
    fn the_frame_seed_rotates_only_when_asked() {
        assert_eq!(frame_seed(13, true), (5, 1));
        assert_eq!(frame_seed(13, false), (0, 0));
        let period: Vec<u32> = (0..16).map(|i| frame_seed(i, true).0).collect();
        assert_eq!(period[..8], period[8..]);
        assert_eq!(period[..8].iter().collect::<std::collections::HashSet<_>>().len(), 8);
    }

    #[test]
    fn the_mode_follows_the_setting_unless_forced() {
        use ShadowAlpha::*;
        assert_eq!(mode_with(None, Legacy), Legacy);
        assert_eq!(mode_with(None, Aurora), Aurora);
        assert_eq!(mode_with(Some("aurora"), Legacy), Aurora);
        assert_eq!(mode_with(Some(" Legacy "), Aurora), Legacy);
        assert_eq!(mode_with(Some("1"), Aurora), Aurora);
        assert_eq!(mode_with(Some(""), Legacy), Legacy);
    }

    #[test]
    fn the_uniform_matches_the_shader_struct() {
        assert_eq!(std::mem::size_of::<ShadowStU>(), 16);
        let text = wgsl();
        let at = |name: &str| text.find(name).unwrap_or_else(|| panic!("{name}"));
        assert!(at("origin: vec2<i32>") < at("frame_seed: u32") && at("frame_seed: u32") < at("flags: u32"));
    }

    fn valid(src: &str) -> (naga::Module, naga::valid::ModuleInfo) {
        let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
        (module, info)
    }

    #[test]
    fn the_aurora_module_is_valid_with_and_without_the_triangle_index() {
        for prim in [false, true] {
            let src = source_with(prim);
            // (the second marker is a comment before the literal 0 without the index)
            assert!(!src.contains("SST_PRIM_PARAM") && src.contains("/*SST_PRIM*/") != prim);
            assert_eq!(src.contains("prim_id"), prim);
            assert_eq!(src.starts_with("enable primitive_index;"), prim);
            let (module, info) = valid(&src);
            // what the drivers get: Vulkan's SPIR-V and Metal's MSL for the entry
            let ep = module.entry_points.iter().find(|e| e.name == "fs_shadow_aurora").expect("entry");
            // (pipeline-overridable constants resolved at their defaults, as wgpu does)
            let (module, info) = naga::back::pipeline_constants::process_overrides(
                &module,
                &info,
                Some((ep.stage, ep.name.as_str())),
                &naga::back::PipelineConstants::default(),
            )
            .unwrap_or_else(|e| panic!("overrides, prim {prim}: {e:?}"));
            let (module, info) = (module.as_ref(), info.as_ref());
            let msl = naga::back::msl::PipelineOptions {
                entry_point: Some((ep.stage, ep.name.clone())),
                ..Default::default()
            };
            naga::back::msl::write_string(
                module,
                info,
                &naga::back::msl::Options {
                    lang_version: (2, 4),
                    ..Default::default()
                },
                &msl,
            )
            .unwrap_or_else(|e| panic!("MSL, prim {prim}: {e:?}"));
            let spv = naga::back::spv::PipelineOptions {
                shader_stage: ep.stage,
                entry_point: ep.name.clone(),
            };
            naga::back::spv::write_vec(module, info, &naga::back::spv::Options::default(), Some(&spv)).expect("SPIR-V");
        }
    }

    #[test]
    fn the_default_scene_module_does_not_know_the_aurora_rule() {
        for src in [include_str!("shaders/object.wgsl"), include_str!("shaders/env.wgsl"), crate::COMMON_SHADER] {
            assert!(!src.contains("fs_shadow_aurora") && !src.contains("shadow_st") && !src.contains("sst_"));
        }
    }

    // ------------------------------------------------------------ on the GPU

    /// What a shadow map keeps of full-screen panes drawn through the
    /// extracted `sst_*` functions: the depth of a 256 x 256 map, for panes
    /// `[alpha bits, tie, depth bits, seed prim]`.
    fn gpu_depths(device: &wgpu::Device, queue: &wgpu::Queue, panes: &[[u32; 4]], origin: [i32; 2]) -> Vec<f32> {
        use crate::pipeline_cache::CachedPipelines;
        use wgpu::util::DeviceExt;
        let text = wgsl();
        let start = text.find("// ---- the stochastic coverage rule").expect("the rule");
        let end = text.find("// fs_shadow's casters").expect("end of the rule");
        let source = format!(
            r#"
{rule}
@group(0) @binding(0) var<storage, read> panes: array<vec4<u32>>;
@group(0) @binding(1) var<uniform> origin: vec4<i32>;
struct Out {{ @builtin(position) clip: vec4<f32>, @location(0) @interpolate(flat) pane: u32 }};
@vertex fn vs(@builtin(vertex_index) i: u32, @builtin(instance_index) inst: u32) -> Out {{
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0))[i];
    return Out(vec4<f32>(p, bitcast<f32>(panes[inst].z), 1.0), inst);
}}
@fragment fn fs(in: Out) {{
    let pane = panes[in.pane];
    let seed = sst_seed(pane.y, pane.w, 0u, false);
    let cell = vec2<i32>(floor(in.clip.xy)) - origin.xy;
    if (!sst_keep(cell, seed, bitcast<f32>(pane.x))) {{ discard; }}
}}
"#,
            rule = &text[start..end]
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow alpha harness"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_render_pipeline_cached(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow alpha harness"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            multiview_mask: None,
            cache: None,
        });
        let pane_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(panes),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let origin_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[origin[0], origin[1], 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: pane_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: origin_buffer.as_entire_binding(),
                },
            ],
        });
        let (texture, view) = crate::renderer::make_view(
            device,
            "shadow alpha depth",
            256,
            256,
            wgpu::TextureFormat::Depth32Float,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 256 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..panes.len() as u32);
        }
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256 * 4),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |r| r.expect("readback"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("GPU completion");
        let bytes = readback.get_mapped_range(..).expect("pixels").to_vec();
        bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    }

    fn pane(alpha: f32, tie: u32, depth: f32) -> [u32; 4] {
        [alpha.to_bits(), tie, depth.to_bits(), 0]
    }

    /// The share of the central 128 x 128 texels no pane reached: what a
    /// receiver behind them sees of the light once its filter has averaged
    /// the map.
    fn lit(depths: &[f32]) -> f32 {
        let mut lit = 0;
        for y in 64..192 {
            for x in 64..192 {
                lit += usize::from(depths[y * 256 + x] >= 0.9);
            }
        }
        lit as f32 / 16_384.0
    }

    #[test]
    #[ignore = "requires a GPU"]
    fn gpu_shadow_alpha_coverage() {
        let _gpu = crate::test_gpu::lock();
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).expect("a GPU");
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("GPU device");
        let near = |name: &str, v: f32, want: f32| {
            println!("{name}: {v:.4} (want {want:.2} +- 0.03)");
            assert!((v - want).abs() < 0.03, "{name}: {v} vs {want}");
        };
        near("one 0.5 pane", lit(&gpu_depths(&device, &queue, &[pane(0.5, 1, 0.3)], [0, 0])), 0.50);
        near(
            "two stacked 0.5 panes",
            lit(&gpu_depths(&device, &queue, &[pane(0.5, 1, 0.3), pane(0.5, 2, 0.4)], [0, 0])),
            0.25,
        );
        near("alpha 0.3", lit(&gpu_depths(&device, &queue, &[pane(0.3, 1, 0.3)], [0, 0])), 0.70);
        near("alpha 0", lit(&gpu_depths(&device, &queue, &[pane(0.0, 1, 0.3)], [0, 0])), 1.0);
        near("alpha 1", lit(&gpu_depths(&device, &queue, &[pane(1.0, 1, 0.3)], [0, 0])), 0.0);

        // the GPU keeps the texels the Rust mirror keeps
        let one = gpu_depths(&device, &queue, &[pane(0.37, 7, 0.3)], [0, 0]);
        let s = seed(7, 0, 0, false);
        let wrong = (0..256 * 256)
            .filter(|&i| (one[i] >= 0.9) == keep(((i % 256) as i32, (i / 256) as i32), s, 0.37))
            .count();
        println!("mirror mismatches: {wrong} of 65536");
        assert!(wrong <= 65, "{wrong} texels differ from the Rust mirror");

        // the same map with the origin moved by two texels is the same image
        // moved by two texels
        let shifted = gpu_depths(&device, &queue, &[pane(0.37, 7, 0.3)], [2, 2]);
        for y in 8..248 {
            for x in 8..248 {
                assert_eq!(shifted[(y + 2) * 256 + x + 2] >= 0.9, one[y * 256 + x] >= 0.9, "({x}, {y})");
            }
        }
    }
}
