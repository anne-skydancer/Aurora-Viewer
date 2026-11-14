//! Shadow map slots of the projectors, kept by emitter.
//!
//! A projector's light and shadow change only when the projector or
//! something in its beam moves, never because the camera moves or turns. The
//! slot of a shadow map therefore belongs to its emitter (the object's
//! `full_id`, checked every frame since object slots are reused) and the
//! camera takes no part in who holds one: a projector that stays a
//! candidate keeps its slot, and only a full budget makes anyone give one up.
//!
//! This differs from the Legacy Viewer on purpose. It picks the two
//! projectors of the largest apparent size from the camera
//! (LLPipeline::generateSunShadow mTargetShadowSpotLight,
//! LLVOVolume::updateSpotLightPriority in indra/newview/llvovolume.cpp), and
//! lets one fade out as fast as the camera moves (pipeline.cpp, the fade of
//! mShadowSpotLight), so a turn of the head changes which lights have shadows
//! (originally LGPL 2.1, Linden Research).
//!
//! Grounding: "if nothing changes within a shadow map's frustum, that shadow
//! map does not need to be recomputed" (Akenine-Moller et al., Real-Time
//! Rendering 4th ed., section 7.4.1); a shadow atlas keyed by light and kept
//! (Sousa and Geffroy, "The Devil is in the Details: idTech 666", SIGGRAPH
//! 2016). Their camera-based parts (screen size, time slicing) are left out.

use uuid::Uuid;

/// How long a projector takes to give up a map to another (and the other to
/// take it), whatever the camera does.
pub const FADE_SECONDS: f32 = 0.5;

/// Band 0 is under twice this many metres (8 m), then each band doubles:
/// `band`.
const BAND_BASE: f32 = 4.0;

/// A projector that may hold a shadow map this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitter {
    /// The object's `full_id`: the key of its slot.
    pub id: Uuid,
    /// The object's index in the world this frame (it may change, an id does
    /// not).
    pub idx: usize,
    /// From the camera to the light's sphere (the draw distance's measure).
    pub dist: f32,
    /// What the projector gives, whatever the view: `intrinsic_score`.
    pub score: f32,
}

/// The brightest channel of the light's colour (already times its
/// intensity) times its reach squared: the same for every camera.
pub fn intrinsic_score(color: [f32; 3], reach: f32) -> f32 {
    color[0].max(color[1]).max(color[2]) * reach * reach
}

/// Distance band: floor(log2(dist / 4 m)); band 0 is under 8 m, then each
/// band doubles (8 to 16 m is band 1, 16 to 32 m band 2...).
fn band(dist: f32) -> i32 {
    (dist.max(BAND_BASE) / BAND_BASE).log2().floor() as i32
}

/// Order of two emitters for the free maps and the overflow: nearer band
/// first, then the greater intrinsic score, then the smaller id.
fn rank(a: &Emitter, b: &Emitter) -> std::cmp::Ordering {
    band(a.dist)
        .cmp(&band(b.dist))
        .then(b.score.total_cmp(&a.score))
        .then(a.id.cmp(&b.id))
}

/// A challenger replaces an incumbent only when it is at least two bands
/// nearer or, in the same band, at least twice as strong: a clear change, not
/// a tie that the camera's steps would flip.
fn beats(challenger: &Emitter, incumbent: &Emitter) -> bool {
    let (c, i) = (band(challenger.dist), band(incumbent.dist));
    i - c >= 2 || (c == i && challenger.score >= 2.0 * incumbent.score)
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    id: Uuid,
    idx: usize,
    /// 0 to 1: how much of its shadow shows.
    fade: f32,
    /// Giving up the map to this emitter: fading out first.
    leaving_for: Option<Uuid>,
}

/// The projectors' shadow maps and how far each has faded in.
#[derive(Debug, Default)]
pub struct ProjectorSlots {
    slots: Vec<Option<Slot>>,
}

impl ProjectorSlots {
    /// The maps in use: (map, object index, fade), in map order.
    pub fn assigned(&self) -> impl Iterator<Item = (usize, usize, f32)> + '_ {
        self.slots.iter().enumerate().filter_map(|(k, s)| s.map(|s| (k, s.idx, s.fade)))
    }

    /// Every map as `AURORA_PROJECTOR_TRACE` shows it.
    pub fn trace(&self) -> (Vec<Option<Uuid>>, Vec<f32>) {
        (
            self.slots.iter().map(|s| s.map(|s| s.id)).collect(),
            self.slots.iter().map(|s| s.map_or(0.0, |s| s.fade)).collect(),
        )
    }

    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// One frame: `candidates` are the projectors that cast shadows and may
    /// hold a map (visible, within the draw distance), `limit` the budget.
    ///
    /// A budget that shrinks does not drop the maps past it at once: their
    /// owners fade out (`FADE_SECONDS`) and the maps are freed when they are
    /// out, and no one is given a map past the limit meanwhile. (The renderer
    /// draws only the maps its atlas holds: when the atlas shrinks with the
    /// budget, a setting change, they vanish with it.)
    pub fn update(&mut self, candidates: &[Emitter], limit: usize, dt: f32) {
        if self.slots.len() < limit {
            self.slots.resize(limit, None);
        }
        let amount = dt.max(0.0) / FADE_SECONDS;
        let find = |id: Uuid| candidates.iter().find(|c| c.id == id);
        // owners: gone ones free their map at once (their light is gone
        // too), the rest follow their object; fades move
        for (k, slot) in self.slots.iter_mut().enumerate() {
            let Some(s) = slot else { continue };
            let Some(c) = find(s.id) else {
                *slot = None;
                continue;
            };
            s.idx = c.idx;
            // past the budget: fade out, then the map is free (and goes)
            if k >= limit {
                s.leaving_for = None;
                s.fade = (s.fade - amount).max(0.0);
                if s.fade == 0.0 {
                    *slot = None;
                }
                continue;
            }
            match s.leaving_for {
                // the successor is gone: keep the map
                Some(next) if find(next).is_none() => s.leaving_for = None,
                Some(_) => s.fade = (s.fade - amount).max(0.0),
                None => s.fade = (s.fade + amount).min(1.0),
            }
        }
        while self.slots.len() > limit && self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
        // a map faded out goes to its successor, fading in
        for k in 0..limit.min(self.slots.len()) {
            let Some(Slot {
                leaving_for: Some(next),
                fade,
                ..
            }) = self.slots[k]
            else {
                continue;
            };
            if fade > 0.0 {
                continue;
            }
            let holds = |slots: &[Option<Slot>]| slots.iter().flatten().any(|s| s.id == next);
            self.slots[k] = match find(next) {
                Some(c) if !holds(&self.slots) => Some(Slot {
                    id: c.id,
                    idx: c.idx,
                    fade: 0.0,
                    leaving_for: None,
                }),
                _ => None,
            };
        }
        // the others, best first: free maps, then the overflow rule
        let mut waiting: Vec<Emitter> = candidates
            .iter()
            .filter(|c| !self.slots.iter().flatten().any(|s| s.id == c.id || s.leaving_for == Some(c.id)))
            .copied()
            .collect();
        waiting.sort_by(rank);
        for c in waiting {
            // a projector that appears has its shadow at once: light and
            // shadow come together
            if let Some(free) = self.slots.iter_mut().take(limit).find(|s| s.is_none()) {
                *free = Some(Slot {
                    id: c.id,
                    idx: c.idx,
                    fade: 1.0,
                    leaving_for: None,
                });
                continue;
            }
            // the weakest incumbent that is not on its way out
            let weakest = self
                .slots
                .iter()
                .enumerate()
                .take(limit)
                .filter_map(|(k, s)| {
                    let s = s.as_ref().filter(|s| s.leaving_for.is_none())?;
                    Some((k, find(s.id).copied()?))
                })
                .max_by(|a, b| rank(&a.1, &b.1));
            if let Some((k, _)) = weakest.filter(|(_, i)| beats(&c, i))
                && let Some(s) = &mut self.slots[k]
            {
                s.leaving_for = Some(c.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn emitter(n: u128, dist: f32, score: f32) -> Emitter {
        Emitter {
            id: id(n),
            idx: n as usize,
            dist,
            score,
        }
    }

    fn map_of(slots: &ProjectorSlots) -> Vec<Option<Uuid>> {
        slots.trace().0
    }

    /// A small deterministic generator, for the camera positions.
    fn lcg(state: &mut u64) -> f32 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*state >> 40) as f32 / (1u64 << 24) as f32
    }

    /// The owner's principle: with no more projectors than maps, no camera
    /// position or direction changes who holds one, nor any fade.
    #[test]
    fn no_camera_changes_the_assignment_when_every_projector_has_a_map() {
        let lights = [
            Vec3::new(10.0, 5.0, 3.0),
            Vec3::new(-30.0, 8.0, 3.0),
            Vec3::new(2.0, -60.0, 20.0),
            Vec3::new(90.0, 90.0, 3.0),
            Vec3::new(5.0, 6.0, 3.0),
        ];
        let reach = 12.0;
        let build = |eye: Vec3| -> Vec<Emitter> {
            lights
                .iter()
                .enumerate()
                .map(|(n, p)| {
                    emitter(
                        n as u128 + 1,
                        (p.distance(eye) - reach).max(0.0),
                        intrinsic_score([1.0, 0.8, 0.6], reach),
                    )
                })
                .collect()
        };
        let mut slots = ProjectorSlots::default();
        for _ in 0..40 {
            slots.update(&build(Vec3::ZERO), 8, 0.05);
        }
        let reference = (map_of(&slots), slots.trace().1);
        assert_eq!(slots.assigned().count(), lights.len());
        assert!(slots.assigned().all(|(_, _, fade)| fade == 1.0));
        let mut state = 7;
        for _ in 0..500 {
            let eye = Vec3::new(
                lcg(&mut state) * 400.0 - 200.0,
                lcg(&mut state) * 400.0 - 200.0,
                lcg(&mut state) * 60.0,
            );
            slots.update(&build(eye), 8, 0.016);
            assert_eq!((map_of(&slots), slots.trace().1), reference, "eye {eye}");
        }
    }

    #[test]
    fn a_projector_that_appears_has_its_shadow_at_once() {
        let mut slots = ProjectorSlots::default();
        slots.update(&[emitter(1, 20.0, 5.0)], 4, 0.016);
        assert_eq!(slots.assigned().collect::<Vec<_>>(), [(0, 1, 1.0)]);
        slots.update(&[emitter(1, 20.0, 5.0), emitter(2, 30.0, 5.0)], 4, 0.016);
        assert_eq!(slots.assigned().collect::<Vec<_>>(), [(0, 1, 1.0), (1, 2, 1.0)]);
    }

    #[test]
    fn a_projector_keeps_its_map_while_it_stays_a_candidate() {
        let mut slots = ProjectorSlots::default();
        let all = [emitter(1, 20.0, 1.0), emitter(2, 30.0, 1.0), emitter(3, 40.0, 1.0)];
        slots.update(&all, 3, 0.1);
        let before = map_of(&slots);
        // the order of the candidates, their distances and scores swap: no
        // change as long as every one has a map
        for _ in 0..10 {
            slots.update(&[emitter(3, 5.0, 9.0), emitter(1, 90.0, 0.1), emitter(2, 5.0, 3.0)], 3, 0.1);
            assert_eq!(map_of(&slots), before);
        }
        // one leaves: its map is free at once, the others stay
        slots.update(&[emitter(3, 5.0, 9.0), emitter(1, 90.0, 0.1)], 3, 0.1);
        assert_eq!(map_of(&slots), [Some(id(1)), None, Some(id(3))]);
        assert!(slots.assigned().all(|(_, _, f)| f == 1.0));
    }

    /// Object slots are reused: another object at the same index is another
    /// emitter, and the first one's map does not stay with it.
    #[test]
    fn a_reused_object_slot_does_not_inherit_the_map() {
        let mut slots = ProjectorSlots::default();
        let mut a = emitter(1, 20.0, 1.0);
        a.idx = 17;
        let other = emitter(3, 20.0, 1.0);
        slots.update(&[a, other], 2, 0.1);
        assert_eq!(map_of(&slots), [Some(id(1)), Some(id(3))]);
        // in one frame: emitter 1 is gone, emitter 2 stands at its index
        let mut b = emitter(2, 20.0, 1.0);
        b.idx = 17;
        slots.update(&[b, other], 2, 0.1);
        assert_eq!(map_of(&slots), [Some(id(2)), Some(id(3))]);
        assert_eq!(slots.assigned().collect::<Vec<_>>(), [(0, 17, 1.0), (1, 3, 1.0)]);
        // both emitters again, the index of 2 changed: the map follows the id
        let mut moved = b;
        moved.idx = 40;
        slots.update(&[moved, other], 2, 0.1);
        assert_eq!(slots.assigned().collect::<Vec<_>>(), [(0, 40, 1.0), (1, 3, 1.0)]);
    }

    /// A candidate that leaves (out of the draw distance, derezzed) frees its
    /// map at once and gets a map again when it returns, never the others'.
    #[test]
    fn a_candidate_that_leaves_and_returns_takes_a_free_map() {
        let mut slots = ProjectorSlots::default();
        let (a, b, c) = (emitter(1, 20.0, 1.0), emitter(2, 20.0, 1.0), emitter(3, 20.0, 1.0));
        slots.update(&[a, b, c], 3, 0.1);
        slots.update(&[a, c], 3, 0.1);
        assert_eq!(map_of(&slots), [Some(id(1)), None, Some(id(3))]);
        slots.update(&[a, b, c], 3, 0.1);
        assert_eq!(map_of(&slots), [Some(id(1)), Some(id(2)), Some(id(3))]);
        assert!(slots.assigned().all(|(_, _, f)| f == 1.0));
    }

    /// More projectors than maps: a camera that walks about inside one
    /// distance band (distances change, bands and scores do not) changes
    /// nothing.
    #[test]
    fn a_moving_eye_inside_one_band_keeps_the_owners() {
        let mut slots = ProjectorSlots::default();
        let build = |wobble: f32| -> Vec<Emitter> { (1..=5).map(|n| emitter(n, 17.0 + wobble * n as f32, 5.0)).collect() };
        for _ in 0..10 {
            slots.update(&build(0.0), 3, 0.1);
        }
        let reference = (map_of(&slots), slots.trace().1);
        assert_eq!(reference.0, [Some(id(1)), Some(id(2)), Some(id(3))]);
        let mut state = 11;
        for _ in 0..300 {
            // 16 to 32 m: one band whatever the wobble
            let wobble = lcg(&mut state) * 2.5;
            slots.update(&build(wobble), 3, 0.016);
            assert_eq!((map_of(&slots), slots.trace().1), reference);
        }
    }

    #[test]
    fn overflow_needs_a_clear_win_and_the_old_map_fades_out_first() {
        let mut slots = ProjectorSlots::default();
        // two maps, two incumbents in the 16 to 32 m band
        let a = emitter(1, 20.0, 10.0);
        let b = emitter(2, 20.0, 10.0);
        slots.update(&[a, b], 2, 0.1);
        // one band nearer: not enough
        let near = emitter(3, 9.0, 10.0);
        slots.update(&[a, b, near], 2, 0.1);
        assert_eq!(map_of(&slots), [Some(id(1)), Some(id(2))]);
        // same band, 1.9x: not enough; 2x: enough
        let strong = emitter(4, 20.0, 19.0);
        slots.update(&[a, b, strong], 2, 0.1);
        assert_eq!(map_of(&slots), [Some(id(1)), Some(id(2))]);
        let stronger = emitter(4, 20.0, 20.0);
        slots.update(&[a, b, stronger], 2, 0.25);
        // the weaker of equals is the larger id: map 1 (id 2) is to go
        assert_eq!(slots.trace(), (vec![Some(id(1)), Some(id(2))], vec![1.0, 1.0]));
        slots.update(&[a, b, stronger], 2, 0.25);
        assert_eq!(slots.trace(), (vec![Some(id(1)), Some(id(2))], vec![1.0, 0.5]));
        slots.update(&[a, b, stronger], 2, 0.25);
        // faded out: handed over, fading in
        assert_eq!(slots.trace(), (vec![Some(id(1)), Some(id(4))], vec![1.0, 0.0]));
        slots.update(&[a, b, stronger], 2, 0.25);
        assert_eq!(slots.trace().1, [1.0, 0.5]);
        slots.update(&[a, b, stronger], 2, 0.25);
        assert_eq!(slots.trace().1, [1.0, 1.0]);
        // the loser waits: no flip back
        for _ in 0..20 {
            slots.update(&[a, b, stronger], 2, 0.1);
            assert_eq!(map_of(&slots), [Some(id(1)), Some(id(4))]);
        }
    }

    #[test]
    fn two_bands_nearer_wins_and_the_far_incumbent_is_the_one_replaced() {
        let mut slots = ProjectorSlots::default();
        let near = emitter(1, 5.0, 1.0);
        let far = emitter(2, 100.0, 1.0);
        slots.update(&[near, far], 2, 0.1);
        let arrival = emitter(3, 30.0, 1.0);
        // 30 m is band 2, 100 m is band 4: two bands nearer
        slots.update(&[near, far, arrival], 2, 0.1);
        slots.update(&[near, far, arrival], 2, 0.1);
        assert_eq!(slots.trace().0, [Some(id(1)), Some(id(2))]);
        assert!(slots.trace().1[1] < 1.0, "the far one fades out");
    }

    #[test]
    fn the_overflow_is_deterministic() {
        let all: Vec<Emitter> = (1..=6).map(|n| emitter(n, 20.0, 5.0)).collect();
        let reversed: Vec<Emitter> = all.iter().rev().copied().collect();
        let (mut one, mut two) = (ProjectorSlots::default(), ProjectorSlots::default());
        for _ in 0..30 {
            one.update(&all, 4, 0.1);
            two.update(&reversed, 4, 0.1);
        }
        assert_eq!(map_of(&one), map_of(&two));
        // equal emitters: the smaller ids take the maps
        assert_eq!(map_of(&one), [Some(id(1)), Some(id(2)), Some(id(3)), Some(id(4))]);
    }

    #[test]
    fn a_smaller_budget_gives_the_last_maps_up_and_a_larger_one_fills_at_once() {
        let mut slots = ProjectorSlots::default();
        let all: Vec<Emitter> = (1..=4).map(|n| emitter(n, 20.0, 5.0)).collect();
        slots.update(&all, 4, 0.1);
        // the maps past the budget fade out first, and nobody takes them
        slots.update(&all, 2, 0.1);
        assert_eq!(map_of(&slots), [Some(id(1)), Some(id(2)), Some(id(3)), Some(id(4))]);
        let fades = slots.trace().1;
        assert!(
            fades[..2] == [1.0, 1.0] && fades[2..].iter().all(|f| (f - 0.8).abs() < 1e-6),
            "{fades:?}"
        );
        for _ in 0..6 {
            slots.update(&all, 2, 0.1);
        }
        assert_eq!(map_of(&slots), [Some(id(1)), Some(id(2))]);
        assert!(slots.assigned().all(|(_, _, f)| f == 1.0));
        slots.update(&all, 4, 0.1);
        assert_eq!(slots.assigned().count(), 4);
        assert!(slots.assigned().all(|(_, _, f)| f == 1.0));
    }

    #[test]
    fn the_distance_band_starts_at_eight_metres_and_doubles() {
        assert_eq!([0.0, 7.9, 8.0, 15.9, 16.0, 31.9, 32.0, 100.0].map(band), [0, 0, 1, 1, 2, 2, 3, 4]);
    }

    #[test]
    fn the_intrinsic_score_does_not_know_the_camera() {
        assert_eq!(intrinsic_score([0.5, 1.0, 0.25], 10.0), 100.0);
        assert!(intrinsic_score([1.0, 1.0, 1.0], 20.0) > intrinsic_score([1.0, 1.0, 1.0], 10.0));
    }
}
