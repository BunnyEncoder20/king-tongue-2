//! The `World`: the one thing that owns simulation state and advances it.
//!
//! This is the single test seam. Everything a player can do is reachable from
//! this module's public API, and the Shell is an adapter over it with no game
//! logic of its own. See ADR-0001 for why the boundary is here, ADR-0002 for the
//! fixed Sim Tick, ADR-0007 for the integrator, and ADR-0009 for the two
//! snapshots this type retains.

use crate::combat;
use crate::frame::{Body, BodyKind, Snapshot};
use crate::level::{Aabb, Level};
use crate::{EntityId, DEFAULT_PLAYER_MASS, DT, GRAVITY, PLAYER_RADIUS};
use glam::Vec2;

/// How many times one Sim Tick's motion may be redirected by a surface.
///
/// Each pass consumes the part of the motion heading into a surface and carries
/// the rest along it, so a body entering a corner needs two: one per face. Four
/// is slack for an axis-aligned world and bounds the work per tick, which a
/// `while` loop would not.
const MAX_SLIDE_PASSES: usize = 4;

/// How far the swept test pulls a grown box in from its true size, so that
/// touching a surface exactly is not read as crossing into it.
///
/// This exists because of one specific case. A floor is a row of boxes, and a
/// body resting on it sits exactly tangent to the top of the next box along.
/// Growing each box by the body's radius makes that tangency look like a hit on
/// the next box's *side*, reporting a sideways normal, and horizontal motion
/// then stops dead at every tile boundary. In exact circle geometry the contact
/// is a corner graze whose normal points straight up and motion along the floor
/// is untouched, so the sideways hit is an artifact of the square-corner
/// approximation and not real.
///
/// Pulling the box in by a hair makes an exact graze miss, which is the right
/// answer. Genuine contact is still caught: depenetration works on the true
/// geometry, so a resting body ends up at exactly its radius from the surface
/// within a tick of landing.
const GRAZE_EPSILON: f32 = 1e-4;

/// One simulated body. Private on purpose: the Shell and the tests see bodies
/// only through a [`Snapshot`], so the internal representation stays free to
/// grow a Tongue list, a Toughness pool, and a Combo without breaking a test.
#[derive(Clone, Copy, Debug)]
struct BodyState {
    id: EntityId,
    kind: BodyKind,
    pos: Vec2,
    vel: Vec2,
    facing: Vec2,
    mass: f32,
    /// Resistance to being destroyed, in momentum units. GLOSSARY: an impact
    /// spends momentum against it and reaching zero destroys the body, which is
    /// why there is no separate destroyed flag.
    ///
    /// The hero's is infinite. The hero's one-shot fail state is deferred in the
    /// map's fog, so nothing may destroy the hero yet, and an infinite pool says
    /// that in the same terms everything else uses.
    toughness: f32,
    /// Whether the hero was in contact at the end of the previous Sim Tick, so
    /// that one pass costs one impact. See `combat::EnemyTarget::touching`.
    touching_hero: bool,
}

/// All simulation state, and the only way to advance it.
#[derive(Clone, Debug)]
pub struct World {
    bodies: Vec<BodyState>,
    static_terrain: Vec<crate::level::Aabb>,
    hero: EntityId,
    /// Monotonic. Never reused within a run, because interpolation matches
    /// bodies on identity and a recycled id would blend two different bodies
    /// into each other.
    next_id: u32,
    tick: u64,
    /// The two snapshots ADR-0009 requires. `current` is the tick just
    /// simulated; `previous` is the one before it, kept so the Shell can
    /// interpolate without owning any history of its own.
    current: Snapshot,
    previous: Snapshot,
}

impl World {
    /// Builds a world from level data: a hero at `player_spawn`, and an Enemy
    /// per entry in `enemy_spawns`.
    pub fn from_level(level: &Level) -> Self {
        let mut world = World {
            bodies: Vec::with_capacity(1 + level.enemy_spawns.len()),
            static_terrain: level.static_terrain.clone(),
            hero: EntityId(0),
            next_id: 0,
            tick: 0,
            current: Snapshot::default(),
            previous: Snapshot::default(),
        };

        let hero = world.alloc_id();
        world.hero = hero;
        world.bodies.push(BodyState {
            id: hero,
            kind: BodyKind::Hero,
            pos: level.player_spawn,
            vel: Vec2::ZERO,
            // Facing is not driven by anything yet. It is a unit vector rather
            // than an angle per ADR-0009, so it must never be zero.
            facing: Vec2::X,
            mass: DEFAULT_PLAYER_MASS,
            toughness: f32::INFINITY,
            touching_hero: false,
        });

        for spawn in &level.enemy_spawns {
            let id = world.alloc_id();
            world.bodies.push(BodyState {
                id,
                kind: BodyKind::Enemy,
                pos: spawn.pos,
                vel: Vec2::ZERO,
                facing: Vec2::X,
                mass: spawn.mass,
                toughness: spawn.toughness,
                touching_hero: false,
            });
        }

        write_snapshot(world.tick, &world.bodies, &mut world.current);
        world.previous.clone_from(&world.current);
        world
    }

    /// The hero's identity, stable for the whole run.
    ///
    /// Exposed so the Shell can find the hero in an interpolated snapshot to
    /// place the Camera without matching on `BodyKind`, which would be game
    /// logic in the Shell.
    pub fn hero_id(&self) -> EntityId {
        self.hero
    }

    /// The snapshot of the tick just simulated.
    pub fn snapshot(&self) -> &Snapshot {
        &self.current
    }

    /// The snapshot of the tick before it, for interpolation. ADR-0009.
    pub fn previous_snapshot(&self) -> &Snapshot {
        &self.previous
    }

    /// Advances the simulation by exactly one Sim Tick.
    ///
    /// Velocity Verlet, per ADR-0007, in the same arrangement the ticket #8
    /// prototype measured against the analytic solution:
    ///
    /// ```text
    /// a0   = force(pos, vel) / mass
    /// pos += vel * DT + 0.5 * a0 * DT^2
    /// a1   = force(pos_new, vel) / mass
    /// vel += 0.5 * (a0 + a1) * DT
    /// ```
    ///
    /// `a1` is evaluated at the new position and the *old* velocity. The true
    /// `a(t + DT)` would need `v(t + DT)`, which is not known yet, and this is
    /// the arrangement ADR-0007's stability and drift numbers were measured in.
    /// Changing it invalidates those measurements.
    pub fn step(&mut self) {
        // Where the hero began the tick, kept for the combat sweep below: an
        // impact is a question about the path the hero travelled and not about
        // where it ended up.
        let hero_start = self
            .bodies
            .iter()
            .find(|b| b.id == self.hero)
            .map(|b| b.pos);

        for i in 0..self.bodies.len() {
            let body = self.bodies[i];

            let a0 = body_force(&body) / body.mass;
            let target = body.pos + body.vel * DT + 0.5 * a0 * DT * DT;

            // Terrain is a constraint on motion and not a force, which is what
            // keeps `body_force` pure: contact is resolved out here, once, and
            // never inside an acceleration that gets evaluated twice.
            let (pos, contacts) = sweep(body.pos, target, PLAYER_RADIUS, &self.static_terrain);

            // The second of the two evaluations ADR-0007 pays for. It probes a
            // hypothetical state rather than the live body, which is why
            // `body_force` takes a state by value and can touch nothing else.
            let mut probe = body;
            probe.pos = pos;
            let a1 = body_force(&probe) / body.mass;

            // Cancelling after the velocity half-step and not before is
            // load-bearing: the half-step adds another tick of gravity, and
            // leaving that in would let a resting body build downward speed
            // against the floor and eventually creep through it.
            let vel = contacts.cancel_into_surfaces(body.vel + 0.5 * (a0 + a1) * DT);

            self.bodies[i].pos = pos;
            self.bodies[i].vel = vel;
        }

        // Impacts are impulses and not forces, so they are resolved here, once,
        // after the integration step. Putting any of this inside `body_force`
        // would have ADR-0007's integrator apply it twice per Sim Tick.
        if let Some(start) = hero_start {
            self.resolve_impacts(start);
        }

        self.tick += 1;
        // Rotate before writing, so `previous` is the tick before `current` and
        // never the same tick. The buffers are swapped rather than rebuilt, so
        // the snapshot vectors keep their capacity instead of allocating.
        core::mem::swap(&mut self.previous, &mut self.current);
        write_snapshot(self.tick, &self.bodies, &mut self.current);
    }

    /// Resolves the hero's pass over this Sim Tick against every Enemy, and
    /// removes whatever it destroyed.
    ///
    /// Thin by design: every rule lives in [`crate::combat`] as a pure function,
    /// and this is the one place in the tick allowed to write the result back.
    /// Enemies are visited in body order, which is stable for a run, so the
    /// outcome of a tick that touches two of them is deterministic.
    fn resolve_impacts(&mut self, hero_start: Vec2) {
        let Some(h) = self.bodies.iter().position(|b| b.id == self.hero) else {
            return;
        };

        for i in 0..self.bodies.len() {
            if self.bodies[i].kind != BodyKind::Enemy {
                continue;
            }
            // Rebuilt per Enemy from the live hero, so two impacts in one tick
            // compound rather than the second overwriting the first.
            let hero = self.bodies[h];
            let pass = combat::HeroPass {
                start: hero_start,
                end: hero.pos,
                vel: hero.vel,
                mass: hero.mass,
                radius: PLAYER_RADIUS,
            };
            let enemy = self.bodies[i];
            let resolved = combat::resolve_contact(
                &pass,
                &combat::EnemyTarget {
                    pos: enemy.pos,
                    vel: enemy.vel,
                    mass: enemy.mass,
                    toughness: enemy.toughness,
                    touching: enemy.touching_hero,
                },
            );

            self.bodies[h].vel = resolved.hero_vel;
            self.bodies[i].vel = resolved.enemy_vel;
            self.bodies[i].toughness = resolved.toughness;
            self.bodies[i].touching_hero = resolved.touching;
        }

        // An emptied pool is destruction, so there is one rule and not two. The
        // hero's pool is infinite, which is how the deferred hero fail state
        // stays unimplemented rather than accidentally arriving here.
        self.bodies.retain(|b| b.toughness > 0.0);
    }

    fn alloc_id(&mut self) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Places a body at `pos` travelling at `vel`.
    ///
    /// Test scaffolding, gated like [`crate::level::Level::from_ascii`] so it
    /// cannot reach a build that ships. A combat test is about what happens at
    /// the moment of impact, and assigning the momentum rather than earning it
    /// by Swinging is what lets those tests exist before a Tongue does.
    #[cfg(any(test, feature = "test-util"))]
    pub fn place(&mut self, id: EntityId, pos: Vec2, vel: Vec2) {
        if let Some(body) = self.bodies.iter_mut().find(|b| b.id == id) {
            body.pos = pos;
            body.vel = vel;
        }
    }
}

/// The total force on a body in the state it is given.
///
/// **Pure, and that is a hard requirement rather than a style preference.**
/// ADR-0007 has Velocity Verlet evaluate acceleration twice per Sim Tick, so a
/// force that mutated state while being evaluated would be applied twice and
/// the simulation would be silently wrong: no panic, no failing assertion, just
/// the wrong answer. The signature is the enforcement. It takes an immutable
/// body state and returns a vector, and it can reach nothing else: no `&mut`,
/// no `&World`, no clock, no randomness.
///
/// Every force added later, the Tongue spring first, goes here under the same
/// rule.
fn body_force(body: &BodyState) -> Vec2 {
    // Gravity, scaled by mass so the caller can divide by mass uniformly and
    // every force in this function means the same thing.
    body.mass * GRAVITY
}

/// The surfaces a body ended a Sim Tick touching.
///
/// Fixed capacity rather than a `Vec`, because this is built once per body per
/// tick and allocating 60 times a second per body for at most four normals is
/// waste the frame contract already refuses elsewhere.
#[derive(Clone, Copy, Debug, Default)]
struct Contacts {
    normals: [Vec2; MAX_SLIDE_PASSES],
    count: usize,
}

impl Contacts {
    fn push(&mut self, normal: Vec2) {
        if self.count < MAX_SLIDE_PASSES {
            self.normals[self.count] = normal;
            self.count += 1;
        }
    }

    /// Removes any part of `vel` heading into a surface in contact, leaving the
    /// part running along it.
    ///
    /// Motion *away* from a surface is untouched, which is what lets a body
    /// leave a contact it is no longer being pressed into. There is no
    /// restitution here: the hero does not bounce off terrain, and the Combo
    /// rule that survives a bounce is a later ticket's business.
    fn cancel_into_surfaces(&self, mut vel: Vec2) -> Vec2 {
        for normal in &self.normals[..self.count] {
            let into = vel.dot(*normal);
            if into < 0.0 {
                vel -= *normal * into;
            }
        }
        vel
    }
}

/// Moves a circle of `radius` from `start` toward `target`, stopping at the
/// first surface it would cross and carrying the rest of the motion along that
/// surface. Reports where it ended up and what it is touching.
///
/// Swept rather than resolved after the fact, and that is the whole point. At
/// the speeds a Tongue slingshot reaches, a body can cross a thin platform
/// entirely within one Sim Tick: it is above the platform at the start of the
/// tick and below it at the end, never inside it. An overlap test sees nothing
/// to fix and the body sails through. Sweeping asks whether the *path* crossed
/// the surface, which is a question that has an answer at any speed.
fn sweep(start: Vec2, target: Vec2, radius: f32, terrain: &[Aabb]) -> (Vec2, Contacts) {
    let mut pos = start;
    let mut remaining = target - start;
    let mut contacts = Contacts::default();

    for _ in 0..MAX_SLIDE_PASSES {
        // Clear any existing overlap first. This covers a body authored inside
        // terrain, and the float residue at a resting contact where the body
        // sits one ulp inside the surface it is standing on.
        if let Some((corrected, normal)) = depenetrate(pos, radius, terrain) {
            pos = corrected;
            contacts.push(normal);
            remaining -= normal * remaining.dot(normal).min(0.0);
        }

        match first_crossing(pos, remaining, radius, terrain) {
            None => {
                pos += remaining;
                break;
            }
            Some((t, normal)) => {
                pos += remaining * t;
                let leftover = remaining * (1.0 - t);
                // Slide: drop the part heading into the surface, keep the rest.
                remaining = leftover - normal * leftover.dot(normal);
                contacts.push(normal);
                if remaining.length_squared() < 1e-12 {
                    break;
                }
            }
        }
    }

    (pos, contacts)
}

/// Pushes a circle out of the terrain box it is most deeply inside, if any, and
/// reports the surface normal it left through.
///
/// Returns a zero-depth correction when the circle is exactly tangent, which is
/// not a no-op: the normal it reports is how a resting body's contact gets
/// registered at all.
fn depenetrate(pos: Vec2, radius: f32, terrain: &[Aabb]) -> Option<(Vec2, Vec2)> {
    let mut deepest: Option<(f32, Vec2)> = None;

    for aabb in terrain {
        let closest = pos.clamp(aabb.min(), aabb.max());
        let away = pos - closest;
        let dist_sq = away.length_squared();
        if dist_sq > radius * radius {
            continue;
        }

        let (normal, depth) = if dist_sq > 1e-12 {
            let dist = dist_sq.sqrt();
            (away / dist, radius - dist)
        } else {
            // The centre is inside the box, so there is no outward direction to
            // read off the geometry. Leave through the nearest face, which is
            // the shortest way out and the only choice that does not teleport
            // the body across the level.
            let to_min = pos - aabb.min();
            let to_max = aabb.max() - pos;
            let faces = [
                (to_min.x, Vec2::NEG_X),
                (to_max.x, Vec2::X),
                (to_min.y, Vec2::NEG_Y),
                (to_max.y, Vec2::Y),
            ];
            let (dist, normal) =
                faces
                    .iter()
                    .copied()
                    .fold((f32::INFINITY, Vec2::Y), |best, face| {
                        if face.0 < best.0 {
                            face
                        } else {
                            best
                        }
                    });
            (normal, dist + radius)
        };

        if deepest.is_none_or(|(d, _)| depth > d) {
            deepest = Some((depth, normal));
        }
    }

    deepest.map(|(depth, normal)| (pos + normal * depth, normal))
}

/// The first surface the swept circle crosses over this motion, as a fraction
/// of the motion and the normal of the face it meets.
fn first_crossing(
    origin: Vec2,
    motion: Vec2,
    radius: f32,
    terrain: &[Aabb],
) -> Option<(f32, Vec2)> {
    if motion.length_squared() < 1e-12 {
        return None;
    }
    let mut first: Option<(f32, Vec2)> = None;
    for aabb in terrain {
        if let Some(hit) = crossing(origin, motion, *aabb, radius) {
            if first.is_none_or(|(t, _)| hit.0 < t) {
                first = Some(hit);
            }
        }
    }
    first
}

/// Slab test of a ray against one box grown by `radius`.
///
/// Growing the box is the Minkowski trick: a circle sweeping against a box is
/// a point sweeping against that box expanded by the circle's radius. It treats
/// corners as square rather than rounded, which makes a corner hit register
/// marginally early. In an axis-aligned Placeholder Geometry world that is
/// invisible, and the alternative is per-corner circle tests for an accuracy
/// nothing here can see.
fn crossing(origin: Vec2, motion: Vec2, aabb: Aabb, radius: f32) -> Option<(f32, Vec2)> {
    let grown = Vec2::splat(radius - GRAZE_EPSILON);
    let min = aabb.min() - grown;
    let max = aabb.max() + grown;

    let mut t_enter = f32::NEG_INFINITY;
    let mut t_exit = f32::INFINITY;
    let mut normal = Vec2::ZERO;

    for axis in 0..2 {
        let d = motion[axis];
        let o = origin[axis];
        if d.abs() < 1e-12 {
            // Running parallel to this pair of faces: either the ray is between
            // them for the whole motion or it never meets the box at all.
            if o < min[axis] || o > max[axis] {
                return None;
            }
            continue;
        }

        let mut near = (min[axis] - o) / d;
        let mut far = (max[axis] - o) / d;
        // Entering from the low side means the face's outward normal points the
        // negative way along this axis, and the reverse coming from the high
        // side. Decided from the direction of travel, before the two times are
        // put in order.
        let mut face = Vec2::ZERO;
        face[axis] = if d > 0.0 { -1.0 } else { 1.0 };
        if near > far {
            core::mem::swap(&mut near, &mut far);
        }

        if near > t_enter {
            t_enter = near;
            normal = face;
        }
        if far < t_exit {
            t_exit = far;
        }
    }

    if t_enter > t_exit || t_exit < 0.0 || t_enter > 1.0 {
        return None;
    }
    if t_enter < 0.0 {
        // Already overlapping when the motion began. That is depenetration's
        // job, and treating it as a crossing here would place the body at a
        // negative fraction of its own motion.
        return None;
    }
    Some((t_enter, normal))
}

/// Fills `out` with everything the Shell needs to draw one Sim Tick.
///
/// A free function rather than a method so the caller can lend `bodies`
/// immutably and `out` mutably at once, which keeps the snapshot buffers and
/// their capacity across ticks instead of reallocating per tick.
fn write_snapshot(tick: u64, bodies: &[BodyState], out: &mut Snapshot) {
    out.tick = tick;
    out.bodies.clear();
    out.tongues.clear();
    for b in bodies {
        out.bodies.push(Body {
            id: b.id,
            kind: b.kind,
            pos: b.pos,
            facing: b.facing,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hero as the seam exposes it: looked up by identity, never by slice
    /// position, because spawn order is not part of the contract.
    fn hero(world: &World) -> Body {
        *world
            .snapshot()
            .body(world.hero_id())
            .expect("the hero must be in every snapshot")
    }

    #[test]
    fn step_advances_exactly_one_sim_tick() {
        let level = Level::from_ascii("....@....\n.........\n#########\n");
        let mut world = World::from_level(&level);
        assert_eq!(world.snapshot().tick, 0);
        for expected in 1..=10u64 {
            world.step();
            assert_eq!(world.snapshot().tick, expected);
            assert_eq!(
                world.previous_snapshot().tick,
                expected - 1,
                "previous must be the tick before current, never the same tick"
            );
        }
    }

    #[test]
    fn free_fall_matches_the_analytic_solution() {
        // Velocity Verlet is exact for a constant acceleration, so a gravity
        // only fall must land on the closed form d = 0.5 * g * t^2 to within
        // float accumulation over the tick count. The expected value comes from
        // the physics and not from the code.
        //
        // This is also the guard on force purity. ADR-0007 has Verlet evaluate
        // acceleration twice per Sim Tick; a force that mutated state during
        // evaluation, or accumulated into a buffer it did not clear, would be
        // applied twice and show up here as roughly double the fall distance.
        let level = Level::from_ascii(
            "
            ....@....
            .........
            .........
            #########
            ",
        );
        let mut world = World::from_level(&level);
        let start = hero(&world).pos;

        const TICKS: u32 = 60;
        for _ in 0..TICKS {
            world.step();
        }

        let t = TICKS as f32 * DT;
        let expected = start.y - 0.5 * GRAVITY.length() * t * t;
        let measured = hero(&world).pos.y;
        assert!(
            (measured - expected).abs() < 1e-4,
            "after {TICKS} ticks expected y {expected}, measured {measured}"
        );
        assert_eq!(
            hero(&world).pos.x,
            start.x,
            "nothing pushes sideways, so there must be no horizontal drift"
        );
    }

    #[test]
    fn hero_keeps_one_identity_for_the_whole_run() {
        // Interpolation matches bodies on EntityId, so an id that changed
        // between ticks would make the hero snap instead of blend every frame,
        // and the frame contract has no way to notice.
        let level = Level::from_ascii("....@....\n.........\n#########\n");
        let mut world = World::from_level(&level);
        let id = world.hero_id();
        for _ in 0..30 {
            world.step();
            assert_eq!(world.hero_id(), id);
            assert!(world.snapshot().body(id).is_some());
        }
    }

    #[test]
    fn stepping_does_not_allocate_a_snapshot_per_tick() {
        // The frame contract holds two snapshots and reuses their buffers. A
        // snapshot rebuilt from scratch each tick would allocate 60 times a
        // second forever, which nothing would fail on and a profiler would
        // find eventually.
        let level = Level::from_ascii("..e.@..e.\n.........\n#########\n");
        let mut world = World::from_level(&level);
        world.step();
        world.step();
        let capacity = world.snapshot().bodies.capacity();
        for _ in 0..200 {
            world.step();
        }
        assert_eq!(
            world.snapshot().bodies.capacity(),
            capacity,
            "snapshot buffers must not grow per tick"
        );
    }

    #[test]
    fn hero_lands_on_the_floor_and_stays_there() {
        let level = Level::from_ascii(
            "
            ....@....
            .........
            .........
            #########
            ",
        );
        // The floor's top surface, from the level data rather than from a magic
        // number, plus the hero's radius: a circle rests tangent to a surface.
        let floor_top = level
            .static_terrain
            .iter()
            .map(|t| t.max().y)
            .fold(f32::NEG_INFINITY, f32::max);
        let rest_y = floor_top + PLAYER_RADIUS;

        let mut world = World::from_level(&level);
        for _ in 0..200 {
            world.step();
        }

        // Tolerance: one tick of gravity's position term, 0.5 * g * DT^2, which
        // is the most a resting body can dip before the contact pushes it back.
        let slack = 0.5 * GRAVITY.length() * DT * DT * 2.0;
        let landed = hero(&world).pos;
        assert!(
            (landed.y - rest_y).abs() < slack.max(1e-4),
            "expected to rest at y {rest_y}, got {landed:?}"
        );

        // And it stays. A body that keeps accumulating downward velocity against
        // a contact creeps through the floor over a long run.
        for _ in 0..600 {
            world.step();
        }
        let settled = hero(&world).pos;
        assert!(
            (settled.y - rest_y).abs() < slack.max(1e-4),
            "drifted after 600 more ticks: {landed:?} then {settled:?}"
        );
    }

    #[test]
    fn a_fast_hero_does_not_tunnel_through_a_thin_floor() {
        // Hostile on purpose. The hero is dropped from 10,000 units onto a
        // floor 0.2 units thick, arriving at roughly 180 units per second: it
        // covers about 3 units per Sim Tick, while the band in which it would
        // ever be found overlapping the floor is only 1.2 units deep. So for
        // most of the tick it is above the floor and at the end of the tick it
        // is below, never inside, and resolving overlap after the fact sees
        // nothing to fix.
        //
        // The drop is repeated across a range of start heights wider than one
        // tick's travel, which is the point. A single drop can land inside the
        // band by luck and pass while tunnelling in general; sweeping the start
        // height sweeps the phase, so at least one case has to be caught by
        // asking whether the path crossed the surface rather than whether the
        // body ended up inside it.
        for step in 0..13 {
            let spawn_y = 10_000.0 + step as f32 * 0.25;
            let level = Level {
                world_bounds: Aabb::new(Vec2::new(0.0, 5_100.0), Vec2::new(1_000.0, 5_200.0)),
                static_terrain: vec![Aabb::new(Vec2::new(0.0, -0.1), Vec2::new(1_000.0, 0.1))],
                player_spawn: Vec2::new(0.0, spawn_y),
                enemy_spawns: vec![],
            };
            level.validate().expect("fixture must be a valid level");

            let mut world = World::from_level(&level);
            // Impact lands near tick 6667; run past it so it must also rest.
            for _ in 0..7_000 {
                world.step();
            }

            let landed = hero(&world).pos;
            assert!(
                landed.y > 0.0,
                "dropped from {spawn_y}, the hero tunnelled through to {landed:?}"
            );
            assert!(
                (landed.y - PLAYER_RADIUS).abs() < 1e-3,
                "dropped from {spawn_y}, expected to rest at y {PLAYER_RADIUS}, got {landed:?}"
            );
        }
    }

    #[test]
    fn a_hero_spawned_inside_terrain_is_pushed_out_rather_than_trapped() {
        // A spawn point inside a terrain block is an authoring mistake a level
        // file can carry, and `validate` cannot catch it without knowing the
        // hero's radius. Being pushed to the surface is recoverable; being held
        // inside the floor is a level that cannot start.
        let block = Aabb::new(Vec2::ZERO, Vec2::new(10.0, 2.0));
        let level = Level {
            world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(50.0)),
            static_terrain: vec![block],
            // Well inside the block, nearest exit straight up.
            player_spawn: Vec2::new(0.0, 1.0),
            enemy_spawns: vec![],
        };
        level.validate().expect("fixture must be a valid level");

        let mut world = World::from_level(&level);
        for _ in 0..120 {
            world.step();
        }
        let pos = hero(&world).pos;
        assert!(
            (pos.y - (block.max().y + PLAYER_RADIUS)).abs() < 1e-3,
            "expected to be pushed out to rest on the block, got {pos:?}"
        );
    }

    #[test]
    fn hero_spawns_at_the_level_spawn_point() {
        let level = Level::from_ascii(
            "
            .........
            ....@....
            #########
            ",
        );
        let world = World::from_level(&level);
        assert_eq!(hero(&world).pos, level.player_spawn);
        assert_eq!(hero(&world).kind, BodyKind::Hero);
    }

    #[test]
    fn enemies_spawn_with_identities_distinct_from_the_hero() {
        let level = Level::from_ascii("..e...e..\n....@....\n#########\n");
        let world = World::from_level(&level);
        let ids: Vec<EntityId> = world.snapshot().bodies.iter().map(|b| b.id).collect();
        assert_eq!(ids.len(), 3, "one hero plus two Enemies");
        let mut unique = ids.clone();
        unique.sort_by_key(|i| i.0);
        unique.dedup();
        assert_eq!(unique.len(), 3, "identities must not be reused: {ids:?}");
        assert!(ids.contains(&world.hero_id()));
    }

    #[test]
    fn sliding_along_a_tiled_floor_does_not_catch_on_a_seam() {
        // Found by reading the sweep rather than by a failure, and it is tested
        // one level below the `World` seam on purpose: nothing in this ticket
        // moves the hero sideways, so there is no way to provoke it through
        // `World` until the Tongue arrives. `sweep` is a pure function of
        // geometry, so this asserts its contract and not any private state.
        //
        // A floor is a row of one-unit boxes, so a body resting on it sits
        // exactly tangent to the top of the next box along. Growing each box by
        // the body's radius turns that tangency into an apparent hit on the
        // next box's left face, with a sideways normal, and horizontal motion
        // stops dead at every tile boundary. In exact circle geometry the
        // contact is a corner graze whose normal points straight up, and motion
        // along the floor is untouched.
        let floor: Vec<Aabb> = (0..5)
            .map(|i| Aabb::new(Vec2::new(i as f32 + 0.5, 0.5), Vec2::splat(0.5)))
            .collect();
        let start = Vec2::new(0.5, 1.0 + PLAYER_RADIUS);
        let motion = Vec2::new(0.5, 0.0);

        let (end, _) = sweep(start, start + motion, PLAYER_RADIUS, &floor);

        assert!(
            (end.x - (start.x + motion.x)).abs() < 1e-5,
            "caught on a tile seam: expected x {}, got {end:?}",
            start.x + motion.x
        );
        assert!(
            (end.y - start.y).abs() < 1e-5,
            "should have stayed on the floor, got {end:?}"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn the_shipped_scene_drops_the_hero_onto_its_floor() {
        // The tracer bullet, headless: the level file that actually ships, read
        // as bytes and simulated, with the hero ending up at rest on the floor
        // the file declares. Every sketched fixture above could be right while
        // the shipped scene was unplayable, and nothing would have said so.
        //
        // Reading the file here is the test standing in for the Shell. The Core
        // Sim still never touches the filesystem: it is handed bytes, exactly
        // as ADR-0001 and ADR-0010 require.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/levels/test_scene.ron"
        );
        let bytes = std::fs::read(path).expect("the shipped level must be readable");
        let level = Level::from_bytes(&bytes).expect("the shipped level must parse");

        let mut world = World::from_level(&level);
        let spawn = hero(&world).pos;
        assert_eq!(spawn, level.player_spawn);

        // The floor is the widest piece of terrain below the spawn point.
        let floor_top = level
            .static_terrain
            .iter()
            .filter(|t| t.max().y < spawn.y && t.contains(Vec2::new(spawn.x, t.centre.y)))
            .map(|t| t.max().y)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(floor_top.is_finite(), "the shipped scene has no floor");

        // Moon gravity over 7 units takes about 2.9 seconds, so 600 ticks is
        // the fall with room to settle.
        for _ in 0..600 {
            world.step();
        }
        let landed = hero(&world).pos;
        assert!(
            landed.y < spawn.y,
            "the hero never fell: spawned at {spawn:?}, ended at {landed:?}"
        );
        assert!(
            (landed.y - (floor_top + PLAYER_RADIUS)).abs() < 1e-3,
            "expected to rest on the floor at y {}, got {landed:?}",
            floor_top + PLAYER_RADIUS
        );
    }
}
