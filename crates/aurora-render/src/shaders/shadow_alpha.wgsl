// Semi-transparent shadows, the Aurora rule (shadow_alpha.rs). A second
// shader module appended to common, object and env: it reads `records`,
// `portal_clip`, `sample_tex_unbiased`, the FLAG_* constants and the
// `vs_shadow` / `ShadowOut` of object.wgsl, and replaces `fs_shadow` for the
// blended casters. Never part of the default scene module.
//
// A blended face casts R = floor(4 alpha + xi) of the 4 texels of each 2x2
// shadow-texel block, a random R-subset, so the shadow map holds the face's
// opacity as a coverage that the receivers' filter averages back (two
// layers of 0.5 give 0.25 of light: their subsets are independent).

// origin: where the world-anchored cells start in this map (shadow texels);
// frame_seed: the frame's number; flags bit 0: rotate the seed with the frame
// (TAA or FSR in the plain main view)
struct ShadowStParams {
    origin: vec2<i32>,
    frame_seed: u32,
    flags: u32,
};
@group(3) @binding(2) var<uniform> shadow_st: ShadowStParams;

// ---- the stochastic coverage rule of PR #3291 (object.wgsl st_hash and
// st_mask), copied here; shadow_alpha.rs pins both to the same golden values
fn sst_hash(v: u32) -> u32 {
    var x = v * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return (x >> 22u) ^ x;
}

fn sst_seed(tie: u32, prim: u32, frame_seed: u32, rotate: bool) -> u32 {
    var seed = sst_hash(sst_hash(tie * 0x9e3779b9u + 0x7f4a7c15u) ^ sst_hash(prim + 0x632be5abu));
    if (rotate) {
        seed = sst_hash(seed + frame_seed);
    }
    return seed;
}

// The 16 subsets of a 2x2 block's 4 texels by size R, packed in nibbles (no
// array: pure ALU on every backend): SST_LO holds entries 0..7, SST_HI 8..15
// (0 | 1 2 4 8 | 3 5 6 9 10 12 | 7 11 13 14 | 15); SST_OFF is the first entry
// of each R and SST_CNT the number of entries.
const SST_LO: u32 = 0x65384210u;
const SST_HI: u32 = 0xFEDB7CA9u;
const SST_OFF: u32 = 0xFB510u;
const SST_CNT: u32 = 0x14641u;

// Does the face of `seed` and alpha `a` cast this texel? `cell` is the texel
// in the map's world-anchored grid (can be negative).
fn sst_keep(cell: vec2<i32>, seed: u32, a: f32) -> bool {
    // the block's coordinates, 16 bits each (a negative one must not OR away
    // the other)
    let b = vec2<u32>(bitcast<u32>(cell.x >> 1u) & 0xffffu, bitcast<u32>(cell.y >> 1u) & 0xffffu);
    let hp = sst_hash(b.x | (b.y << 16u));
    let pv = vec2<f32>(f32(hp >> 8u), f32(sst_hash(hp) >> 8u)) / 16777216.0;
    let rot = vec2<f32>(f32(seed >> 8u), f32(sst_hash(seed) >> 8u)) / 16777216.0;
    let dither = fract(pv + rot).x;
    let r = min(u32(floor(a * 4.0 + dither)), 4u);
    let pick = f32(sst_hash(seed ^ hp ^ 0x5bd1e995u) >> 8u) / 16777216.0;
    let cnt = (SST_CNT >> (4u * r)) & 0xfu;
    let i = ((SST_OFF >> (4u * r)) & 0xfu) + min(u32(pick * f32(cnt)), cnt - 1u);
    let mask = (select(SST_LO, SST_HI, i >= 8u) >> (4u * (i & 7u))) & 0xfu;
    let bit = u32(cell.x & 1) | (u32(cell.y & 1) << 1u);
    return (mask & (1u << bit)) != 0u;
}

// fs_shadow's casters with the Aurora rule for the blended ones: opaque faces
// write their depth as they do, masked ones keep their cutoff. The two
// SST_PRIM markers below are filled in when the device has PRIMITIVE_INDEX
// (the triangle in the seed); without it the face and its side seed alone.
@fragment
fn fs_shadow_aurora(in: ShadowOut, @builtin(front_facing) front: bool, /*SST_PRIM_PARAM*/) {
    let rec = records[in.record];
    portal_clip(rec, in.world_pos);
    let tex_alpha = sample_tex_unbiased(rec.tex.x, in.uv).a;
    let alpha = tex_alpha * rec.base_color.a;
    if ((rec.flags.x & (FLAG_ALPHA_MASK | FLAG_ALPHA_BLEND)) == 0u) {
        return;
    }
    if ((rec.flags.x & FLAG_ALPHA_BLEND) == 0u) {
        if (alpha < rec.params.w) { discard; }
        return;
    }
    // Window panes (static, not glTF, texture alpha under 0.598) cast in
    // proportion to their alpha here, like every other blended face: the
    // Legacy rule is "a pane under 0.598 casts nothing" (fs_shadow:
    // `if (!pbr && !skinned && tex_alpha < 0.598) { discard; }`).
    let prim = /*SST_PRIM*/0u;
    let seed = sst_seed(in.record, prim * 2u + select(1u, 0u, front), shadow_st.frame_seed, (shadow_st.flags & 1u) != 0u);
    let cell = vec2<i32>(floor(in.clip.xy)) - shadow_st.origin;
    if (!sst_keep(cell, seed, alpha)) { discard; }
}
