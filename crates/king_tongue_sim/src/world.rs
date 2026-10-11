//! The `World`: the one thing that owns simulation state and advances it.
//!
//! This is the single test seam. Everything a player can do is reachable from
//! this module's public API, and the Shell is an adapter over it with no game
//! logic of its own. See ADR-0001 for why the boundary is here, ADR-0002 for the
//! fixed Sim Tick, ADR-0007 for the integrator, and ADR-0009 for the two
//! snapshots this type retains.

use crate::frame::{Body, BodyKind, Snapshot};
use crate::level::Level;
use crate::{EntityId, DEFAULT_PLAYER_MASS};
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

    fn alloc_id(&mut self) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        id
    }
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
