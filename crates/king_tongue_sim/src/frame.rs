//! The Core Sim to Shell frame contract: fixed-step accumulation and snapshot
//! interpolation.
//!
//! ADR-0002 fixes the Sim Tick at 60Hz and makes the Shell interpolate between
//! the two most recent snapshots. This module owns both halves of that, and it
//! lives in the Core Sim rather than the Shell because it is deterministic logic
//! with no platform content, so it is testable headless.

use crate::{EntityId, DT};
use glam::Vec2;

/// The maximum number of Sim Ticks one frame may run.
///
/// This is the spiral-of-death guard. Without it a stalled frame tries to catch
/// up its entire debt at once, which stalls the next frame harder, which grows
/// the debt again. The specific mobile case that forces this: Macroquad computes
/// frame delta inside `draw()`, which is skipped while the app is suspended, so
/// the first post-resume delta equals the whole suspend duration. Three minutes
/// backgrounded is roughly 10,800 ticks owed in a single frame.
pub const MAX_CATCHUP_TICKS: u32 = 5;

/// Fixed-timestep accumulator. The Shell feeds it wall-clock frame time; it
/// answers how many Sim Ticks to run and how far between them to draw.
#[derive(Clone, Copy, Debug, Default)]
pub struct Accumulator {
    debt: f32,
    /// Ticks discarded by the clamp, cumulative. Diagnostic only: a nonzero
    /// value means the device could not keep up, or resumed from suspend.
    pub dropped_ticks: u64,
}

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a frame's elapsed time and returns how many Sim Ticks to run now.
    ///
    /// Never returns more than [`MAX_CATCHUP_TICKS`]. Debt beyond that ceiling is
    /// dropped rather than deferred, so simulated time falls behind wall-clock
    /// time after a stall instead of the frame budget spiralling. That trade is
    /// deliberate: determinism is a property of the tick sequence, not of
    /// agreement with the wall clock.
    pub fn feed(&mut self, frame_time: f32) -> u32 {
        // A negative or non-finite delta would poison the accumulator. Clocks do
        // go backwards, and Macroquad's first frame can report oddities.
        if !frame_time.is_finite() || frame_time <= 0.0 {
            return 0;
        }
        self.debt += frame_time;

        let mut ticks = 0;
        while self.debt >= DT && ticks < MAX_CATCHUP_TICKS {
            self.debt -= DT;
            ticks += 1;
        }

        if self.debt >= DT {
            // Over the ceiling. Keep only the sub-tick remainder so `alpha` stays
            // smooth, and account for what was thrown away.
            let discarded = (self.debt / DT).floor();
            self.dropped_ticks += discarded as u64;
            self.debt -= discarded * DT;
        }

        ticks
    }

    /// The blend factor for rendering, in `0.0..1.0`. Zero draws the previous
    /// snapshot exactly, approaching one draws the current snapshot.
    pub fn alpha(&self) -> f32 {
        (self.debt / DT).clamp(0.0, 1.0)
    }
}

/// What kind of thing a body is, which decides how the Shell draws it.
///
/// Adding a variant here is intended to break the Shell's match. The Shell sets
/// `clippy::wildcard_enum_match_arm` to deny, so a catch-all arm cannot be used
/// to silence that break, which is what stops a new entity silently failing to
/// render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Hero,
    Enemy,
    /// An Enemy that has lost control of its own momentum. Drawn differently
    /// because it is a weapon rather than a target.
    Thrown,
}

/// One interpolatable body in a snapshot. Purely `glam` primitives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Stable across ticks. Interpolation matches on this, never on slice index,
    /// because indices shift when anything spawns or despawns.
    pub id: EntityId,
    pub kind: BodyKind,
    pub pos: Vec2,
    /// Facing as a unit vector, deliberately not an angle. Linearly interpolating
    /// an angle travels the wrong way around the wrap boundary: blending 359
    /// degrees toward 1 degree sweeps backwards through 180.
    pub facing: Vec2,
}

/// An attached Tongue, as a line the Shell draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TongueLine {
    pub slot: u8,
    pub from: Vec2,
    pub to: Vec2,
    /// Identifies what the far end is attached to. Interpolation is only valid
    /// while this is unchanged: if it differs between snapshots the Tongue was
    /// re-fired, and blending would draw it sweeping across the level.
    pub anchor_id: Option<EntityId>,
}

/// Everything the Shell needs to draw one Sim Tick. The Core Sim keeps the two
/// most recent of these.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub tick: u64,
    pub bodies: Vec<Body>,
    /// Only attached Tongues appear. An absent slot is not drawn at all.
    pub tongues: Vec<TongueLine>,
}

impl Snapshot {
    /// Finds a body by identity.
    ///
    /// Public because the Camera needs the hero's position out of an
    /// *interpolated* snapshot, and the only honest way for the Shell to ask
    /// that question is by the id the Core Sim hands it. Searching for
    /// `BodyKind::Hero` in the Shell instead would put a game-logic branch
    /// where ADR-0001 forbids one.
    pub fn body(&self, id: EntityId) -> Option<&Body> {
        self.bodies.iter().find(|b| b.id == id)
    }

    fn tongue(&self, slot: u8) -> Option<&TongueLine> {
        self.tongues.iter().find(|t| t.slot == slot)
    }
}

/// Shortest-path interpolation between two facing vectors.
///
/// Lerp then renormalize. Two guards matter: a near-zero result, which happens
/// when the vectors are close to opposite and would normalize to NaN, and a
/// degenerate input.
fn lerp_facing(a: Vec2, b: Vec2, t: f32) -> Vec2 {
    let blended = a.lerp(b, t);
    if blended.length_squared() < 1e-6 {
        // Near-opposite facings have no meaningful midpoint; snap to the target
        // rather than emit NaN.
        return if b.length_squared() > 1e-6 {
            b.normalize()
        } else {
            Vec2::X
        };
    }
    blended.normalize()
}

/// Builds the interpolated view the Shell draws, writing into a reused buffer so
/// rendering allocates nothing per frame.
///
/// Correspondence rules, which are the substance of this contract:
///
/// - A body in both snapshots is interpolated.
/// - A body only in `current` spawned this tick. It is drawn at its current
///   position with no blending, because it has no previous position to blend
///   from and lerping against a stale slot would slide it in from elsewhere.
/// - A body only in `previous` despawned this tick. It is not drawn at all.
/// - A Tongue is interpolated only when the same slot is attached in both
///   snapshots to the same anchor. Any other case draws the current state
///   directly, which is what keeps a newly fired Tongue from appearing to
///   stretch out of nowhere. ADR-0002 requires discrete events be excluded.
pub fn interpolate_into(previous: &Snapshot, current: &Snapshot, alpha: f32, out: &mut Snapshot) {
    let t = alpha.clamp(0.0, 1.0);
    out.tick = current.tick;
    out.bodies.clear();
    out.tongues.clear();

    for cur in &current.bodies {
        let body = match previous.body(cur.id) {
            Some(prev) if prev.kind == cur.kind => Body {
                id: cur.id,
                kind: cur.kind,
                pos: prev.pos.lerp(cur.pos, t),
                facing: lerp_facing(prev.facing, cur.facing, t),
            },
            // Spawned this tick, or changed kind. A kind change (an Enemy
            // becoming Thrown) is a discrete event, so snap rather than blend.
            _ => *cur,
        };
        out.bodies.push(body);
    }

    for cur in &current.tongues {
        let line = match previous.tongue(cur.slot) {
            Some(prev) if prev.anchor_id == cur.anchor_id => TongueLine {
                slot: cur.slot,
                from: prev.from.lerp(cur.from, t),
                to: prev.to.lerp(cur.to, t),
                anchor_id: cur.anchor_id,
            },
            _ => *cur,
        };
        out.tongues.push(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(id: u32, kind: BodyKind, pos: Vec2) -> Body {
        Body {
            id: EntityId(id),
            kind,
            pos,
            facing: Vec2::X,
        }
    }

    #[test]
    fn accumulator_runs_one_tick_per_dt() {
        let mut acc = Accumulator::new();
        assert_eq!(acc.feed(DT), 1);
        assert_eq!(acc.feed(DT * 2.0), 2);
        assert_eq!(acc.dropped_ticks, 0);
    }

    #[test]
    fn accumulator_never_exceeds_the_clamp() {
        let mut acc = Accumulator::new();
        // A frame owing 100 ticks.
        assert_eq!(acc.feed(DT * 100.0), MAX_CATCHUP_TICKS);
        assert!(acc.dropped_ticks > 0);
    }

    #[test]
    fn three_minute_suspend_does_not_run_ten_thousand_ticks() {
        // The exact mobile failure ADR-0002 and the #2 research warned about:
        // Macroquad's first post-resume frame delta is the whole suspend.
        let mut acc = Accumulator::new();
        let ticks = acc.feed(180.0);
        assert_eq!(ticks, MAX_CATCHUP_TICKS);
        assert!(
            acc.dropped_ticks > 10_000,
            "expected to drop most of a 3 minute debt, dropped {}",
            acc.dropped_ticks
        );
        // And the next frame must be back to normal, not still draining.
        assert_eq!(acc.feed(DT), 1);
    }

    #[test]
    fn alpha_stays_in_range_even_after_a_stall() {
        let mut acc = Accumulator::new();
        acc.feed(180.0);
        let a = acc.alpha();
        assert!((0.0..=1.0).contains(&a), "alpha out of range: {a}");
        assert!(a.is_finite());
    }

    #[test]
    fn accumulator_ignores_hostile_deltas() {
        let mut acc = Accumulator::new();
        assert_eq!(acc.feed(-1.0), 0);
        assert_eq!(acc.feed(f32::NAN), 0);
        assert_eq!(acc.feed(0.0), 0);
        assert!(acc.alpha().is_finite());
    }

    #[test]
    fn matched_bodies_interpolate() {
        let prev = Snapshot {
            tick: 1,
            bodies: vec![body(7, BodyKind::Hero, Vec2::new(0.0, 0.0))],
            tongues: vec![],
        };
        let cur = Snapshot {
            tick: 2,
            bodies: vec![body(7, BodyKind::Hero, Vec2::new(10.0, 0.0))],
            tongues: vec![],
        };
        let mut out = Snapshot::default();
        interpolate_into(&prev, &cur, 0.5, &mut out);
        assert_eq!(out.bodies[0].pos, Vec2::new(5.0, 0.0));
    }

    #[test]
    fn interpolation_matches_on_id_not_slice_position() {
        // The regression this contract exists to prevent. An Enemy despawns, so
        // the hero's index shifts. Index matching would blend the hero against
        // the dead Enemy's position and slide it across the level.
        let prev = Snapshot {
            tick: 1,
            bodies: vec![
                body(1, BodyKind::Enemy, Vec2::new(-100.0, 0.0)),
                body(2, BodyKind::Hero, Vec2::new(0.0, 0.0)),
            ],
            tongues: vec![],
        };
        let cur = Snapshot {
            tick: 2,
            bodies: vec![body(2, BodyKind::Hero, Vec2::new(2.0, 0.0))],
            tongues: vec![],
        };
        let mut out = Snapshot::default();
        interpolate_into(&prev, &cur, 0.5, &mut out);
        assert_eq!(out.bodies.len(), 1, "the despawned Enemy must not be drawn");
        assert_eq!(out.bodies[0].id, EntityId(2));
        assert_eq!(out.bodies[0].pos, Vec2::new(1.0, 0.0));
    }

    #[test]
    fn spawned_body_does_not_slide_in_from_elsewhere() {
        let prev = Snapshot {
            tick: 1,
            bodies: vec![],
            tongues: vec![],
        };
        let cur = Snapshot {
            tick: 2,
            bodies: vec![body(9, BodyKind::Enemy, Vec2::new(50.0, 50.0))],
            tongues: vec![],
        };
        let mut out = Snapshot::default();
        interpolate_into(&prev, &cur, 0.5, &mut out);
        assert_eq!(
            out.bodies[0].pos,
            Vec2::new(50.0, 50.0),
            "must snap, not blend"
        );
    }

    #[test]
    fn kind_change_snaps_because_it_is_a_discrete_event() {
        let prev = Snapshot {
            tick: 1,
            bodies: vec![body(4, BodyKind::Enemy, Vec2::new(0.0, 0.0))],
            tongues: vec![],
        };
        let cur = Snapshot {
            tick: 2,
            bodies: vec![body(4, BodyKind::Thrown, Vec2::new(8.0, 0.0))],
            tongues: vec![],
        };
        let mut out = Snapshot::default();
        interpolate_into(&prev, &cur, 0.5, &mut out);
        assert_eq!(out.bodies[0].kind, BodyKind::Thrown);
        assert_eq!(out.bodies[0].pos, Vec2::new(8.0, 0.0));
    }

    #[test]
    fn newly_fired_tongue_does_not_stretch_out_of_nowhere() {
        let prev = Snapshot {
            tick: 1,
            bodies: vec![],
            tongues: vec![],
        };
        let cur = Snapshot {
            tick: 2,
            bodies: vec![],
            tongues: vec![TongueLine {
                slot: 0,
                from: Vec2::new(0.0, 0.0),
                to: Vec2::new(30.0, 30.0),
                anchor_id: None,
            }],
        };
        let mut out = Snapshot::default();
        interpolate_into(&prev, &cur, 0.5, &mut out);
        assert_eq!(
            out.tongues[0].to,
            Vec2::new(30.0, 30.0),
            "must snap on attach"
        );
    }

    #[test]
    fn retargeted_tongue_does_not_sweep_across_the_level() {
        let prev = Snapshot {
            tick: 1,
            bodies: vec![],
            tongues: vec![TongueLine {
                slot: 0,
                from: Vec2::ZERO,
                to: Vec2::new(-500.0, 0.0),
                anchor_id: Some(EntityId(1)),
            }],
        };
        let cur = Snapshot {
            tick: 2,
            bodies: vec![],
            tongues: vec![TongueLine {
                slot: 0,
                from: Vec2::ZERO,
                to: Vec2::new(500.0, 0.0),
                anchor_id: Some(EntityId(2)),
            }],
        };
        let mut out = Snapshot::default();
        interpolate_into(&prev, &cur, 0.5, &mut out);
        assert_eq!(
            out.tongues[0].to,
            Vec2::new(500.0, 0.0),
            "anchor changed, must snap"
        );
    }

    #[test]
    fn facing_interpolation_survives_the_wrap_boundary() {
        // Facings 10 degrees apart either side of due right. Angle lerp would be
        // fine here, but storing unit vectors means the 359-to-1 case is the same
        // code path. Check the blend stays near due right and stays normalized.
        let a = Vec2::from_angle(-0.0873);
        let b = Vec2::from_angle(0.0873);
        let mid = lerp_facing(a, b, 0.5);
        assert!((mid.length() - 1.0).abs() < 1e-5, "must stay a unit vector");
        assert!(mid.x > 0.99, "blend should sit near due right, got {mid:?}");
    }

    #[test]
    fn opposite_facings_do_not_produce_nan() {
        let mid = lerp_facing(Vec2::X, -Vec2::X, 0.5);
        assert!(mid.is_finite(), "got {mid:?}");
        assert!((mid.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn interpolation_reuses_the_output_buffer() {
        let prev = Snapshot {
            tick: 1,
            bodies: vec![],
            tongues: vec![],
        };
        let cur = Snapshot {
            tick: 2,
            bodies: vec![body(1, BodyKind::Hero, Vec2::ZERO)],
            tongues: vec![],
        };
        let mut out = Snapshot::default();
        interpolate_into(&prev, &cur, 1.0, &mut out);
        let cap = out.bodies.capacity();
        for _ in 0..100 {
            interpolate_into(&prev, &cur, 1.0, &mut out);
        }
        assert_eq!(out.bodies.capacity(), cap, "must not grow per frame");
        assert_eq!(out.bodies.len(), 1);
    }
}
