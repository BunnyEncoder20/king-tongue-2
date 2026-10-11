//! The `World`: the one thing that owns simulation state and advances it.
//!
//! This is the single test seam. Everything a player can do is reachable from
//! this module's public API, and the Shell is an adapter over it with no game
//! logic of its own. See ADR-0001 for why the boundary is here, ADR-0002 for the
//! fixed Sim Tick, ADR-0007 for the integrator, and ADR-0009 for the two
//! snapshots this type retains.

use crate::frame::{Body, BodyKind, Snapshot};
use crate::level::Level;
use crate::{EntityId, DEFAULT_PLAYER_MASS, DT, GRAVITY};
use glam::Vec2;

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
        for i in 0..self.bodies.len() {
            let body = self.bodies[i];

            let a0 = body_force(&body) / body.mass;
            let target = body.pos + body.vel * DT + 0.5 * a0 * DT * DT;

            // The second of the two evaluations ADR-0007 pays for. It probes a
            // hypothetical state rather than the live body, which is why
            // `body_force` takes a state by value and can touch nothing else.
            let mut probe = body;
            probe.pos = target;
            let a1 = body_force(&probe) / body.mass;

            self.bodies[i].pos = target;
            self.bodies[i].vel = body.vel + 0.5 * (a0 + a1) * DT;
        }

        self.tick += 1;
        // Rotate before writing, so `previous` is the tick before `current` and
        // never the same tick. The buffers are swapped rather than rebuilt, so
        // the snapshot vectors keep their capacity instead of allocating.
        core::mem::swap(&mut self.previous, &mut self.current);
        write_snapshot(self.tick, &self.bodies, &mut self.current);
    }

    fn alloc_id(&mut self) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        id
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
            hero(&world).pos.x, start.x,
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
}
