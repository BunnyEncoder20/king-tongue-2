//! The `World`: the one thing that owns simulation state and advances it.
//!
//! This is the single test seam. Everything a player can do is reachable from
//! this module's public API, and the Shell is an adapter over it with no game
//! logic of its own. See ADR-0001 for why the boundary is here, ADR-0002 for the
//! fixed Sim Tick, ADR-0007 for the integrator, and ADR-0009 for the two
//! snapshots this type retains.

use crate::combat;
use crate::frame::{Body, BodyKind, Snapshot, TongueLine};
use crate::input::{self, GestureTracker, TouchPhase, TouchSample};
use crate::level::{Aabb, Level};
use crate::tongue::{self, Pulls, Tongue, Tongues};
use crate::{
    Anchor, EntityId, DEFAULT_PLAYER_MASS, DEFAULT_PULL_STRENGTH, DT, GRAVITY, MAX_TONGUES,
    PLAYER_RADIUS,
};
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
    /// The hero's Tongue slots. Indexed by slot, so a Tongue's slot and its
    /// position can never disagree.
    tongues: Tongues,
    /// Which finger currently drives which slot. Input state rather than
    /// simulation state, but it lives here because ADR-0006 makes attachment
    /// outlast the Gesture that created it, so the two have to be decided
    /// together and the Shell must own neither.
    gestures: GestureTracker,
    /// Hooke's `k` for the hero's Tongues. A per-hero parameter like `mass`,
    /// not a character system.
    hero_pull_strength: f32,
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
            tongues: [None; MAX_TONGUES],
            gestures: GestureTracker::new(),
            hero_pull_strength: DEFAULT_PULL_STRENGTH,
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

        world.write_current_snapshot();
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

            // Resolved once per body per tick, before either evaluation, and
            // `Copy`. Both evaluations therefore see the same Anchors, and
            // neither can reach the `World` to look one up.
            let pulls = self.pulls_on(body.id);

            let a0 = body_force(&body, pulls) / body.mass;
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
            let a1 = body_force(&probe, pulls) / body.mass;

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
        self.write_current_snapshot();
    }

    /// Feeds this frame's raw touches in.
    ///
    /// Everything about which finger does what is inside: slot claiming, the
    /// third concurrent finger, and the ray cast that picks the Anchor. The
    /// Shell converts a Macroquad `Touch` into a [`TouchSample`] and calls this,
    /// which is the whole of its input job. ADR-0001 and the spec's standing
    /// constraint: a conditional in the Shell that decided a game outcome would
    /// be a defect, because the Shell is untested by design.
    ///
    /// Call this between Sim Ticks. A Tongue fired here appears in the
    /// [`Snapshot`] the next [`World::step`] writes, never retroactively in the
    /// one already published for a tick that has been simulated.
    pub fn apply_touches(&mut self, touches: &[TouchSample]) {
        // Claims are given up first, and in the same call they are seen.
        // `Ended` and `Cancelled` are visible for exactly one frame, so there
        // is no later chance to notice them. Doing this before any claim is
        // what lets a finger that lifts this frame hand its slot to a finger
        // that lands this frame.
        for touch in touches {
            if matches!(touch.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                // The end of a Gesture, not a Release: ADR-0006 makes
                // attachment outlast the finger, so the Tongue stays. Macroquad
                // flips every live touch to `Ended` when the app minimizes, and
                // inferring Release from that would drop both Tongues on every
                // suspend.
                self.gestures.release_owner(touch.id);
            }
        }

        // One Gesture per slot at most, so this cannot spin on a finger it
        // keeps refusing, and the third concurrent finger is never reached.
        for _ in 0..MAX_TONGUES {
            let Some(touch) = input::next_gesture(touches, &self.gestures) else {
                break;
            };
            let vacant = core::array::from_fn(|slot| self.tongues[slot].is_none());
            let Some(slot) = self.gestures.claim(touch.id, vacant) else {
                break;
            };
            self.fire(slot, touch.pos);
        }
    }

    /// Hooke's `k` for the hero's Tongues.
    pub fn hero_pull_strength(&self) -> f32 {
        self.hero_pull_strength
    }

    /// Sets Hooke's `k` for the hero's Tongues.
    ///
    /// A tuning knob rather than a character system. The spec's device-tuning
    /// work and the hero archetype roster both sit outside this slice; what this
    /// exists for now is asserting that hang equilibrium tracks
    /// `REST_LENGTH + m*g/k` as `k` varies, which is the measurable claim
    /// ADR-0003 makes.
    ///
    /// Values at or above [`crate::max_stable_pull_strength`] diverge under any
    /// integrator at the locked `DT` and reopen ADR-0002 rather than ADR-0007.
    pub fn set_hero_pull_strength(&mut self, pull_strength: f32) {
        self.hero_pull_strength = pull_strength;
    }

    /// Fires a Tongue from the hero toward `tapped`, if the ray meets anything.
    ///
    /// The Gesture's direction is the vector from the hero to the tapped point,
    /// and the ray goes exactly along it. ADR-0012: the aim is never bent.
    fn fire(&mut self, slot: u8, tapped: Vec2) {
        let Some(hero) = self.body(self.hero) else {
            return;
        };
        let from = hero.pos;
        // A ray into open sky meets nothing and fires nothing. ADR-0012 says a
        // fired Tongue always connects, which holds for an enclosed level; a
        // sketch with open sky is a test fixture, and the honest answer there is
        // that the slot stays empty rather than an Anchor being invented.
        if let Some(point) = tongue::cast(from, tapped - from, &self.static_terrain) {
            self.tongues[slot as usize] = Some(Tongue {
                anchor: Anchor::Fixed(point),
            });
        }
    }

    /// Every Tongue pulling on one body this Sim Tick, with its Anchor resolved
    /// to a world-space point.
    fn pulls_on(&self, body: EntityId) -> Pulls {
        let mut pulls = Pulls::default();
        // Only the hero holds Tongues. The equal and opposite pull a Movable
        // Anchor feels by mass ratio is ADR-0003's and arrives with the Enemy
        // Anchor ticket; it will add to this, not replace it.
        if body != self.hero {
            return pulls;
        }
        for tongue in self.tongues.iter().flatten() {
            if let Some(point) = self.anchor_point(tongue.anchor) {
                pulls.push(point, self.hero_pull_strength);
            }
        }
        pulls
    }

    /// Where an Anchor is right now.
    fn anchor_point(&self, anchor: Anchor) -> Option<Vec2> {
        match anchor {
            Anchor::Fixed(point) => Some(point),
            // Resolved by identity and never by slice position, so a spawn or a
            // despawn elsewhere cannot silently re-point a Tongue. A body that
            // has despawned resolves to nothing rather than to a stale
            // position. Nothing fires at an Enemy yet: this ticket's ray cast
            // tests `static_terrain` only.
            Anchor::Movable(id) => self.body(id).map(|b| b.pos),
        }
    }

    fn body(&self, id: EntityId) -> Option<&BodyState> {
        self.bodies.iter().find(|b| b.id == id)
    }

    fn write_current_snapshot(&mut self) {
        let lines = self.tongue_lines();
        write_snapshot(self.tick, &self.bodies, &lines, &mut self.current);
    }

    /// The Attached Tongues as the Shell draws them: hero to Anchor.
    fn tongue_lines(&self) -> [Option<TongueLine>; MAX_TONGUES] {
        let from = self.body(self.hero).map(|b| b.pos);
        core::array::from_fn(|slot| {
            let tongue = self.tongues[slot]?;
            Some(TongueLine {
                slot: slot as u8,
                from: from?,
                to: self.anchor_point(tongue.anchor)?,
                anchor_id: tongue::anchor_id(tongue.anchor),
            })
        })
    }

    /// Reads a body's velocity, for headless tests.
    ///
    /// Gated for the same reason as [`World::place`], which is the companion
    /// that writes it. Velocity is absent from the [`Snapshot`] because the
    /// Shell has no use for it, but an energy budget cannot be computed without
    /// it, and deriving it from two snapshot positions would fold the
    /// integrator's own discretisation into the number being checked.
    #[cfg(any(test, feature = "test-util"))]
    pub fn body_velocity(&self, id: EntityId) -> Option<Vec2> {
        self.body(id).map(|b| b.vel)
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
/// Every force added later goes here under the same rule. The Tongue spring is
/// the first, and it is why `pulls` is passed by value: the Anchors are resolved
/// before either evaluation, so this cannot reach the `World` to look one up and
/// cannot observe a change one evaluation made to the other.
fn body_force(body: &BodyState, pulls: Pulls) -> Vec2 {
    // Gravity, scaled by mass so the caller can divide by mass uniformly and
    // every force in this function means the same thing.
    body.mass * GRAVITY + pulls.force(body.pos, body.vel)
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
fn write_snapshot(
    tick: u64,
    bodies: &[BodyState],
    tongues: &[Option<TongueLine>],
    out: &mut Snapshot,
) {
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
    // Only Attached Tongues appear. An empty slot is absent rather than present
    // and degenerate, because the frame contract draws whatever it is given and
    // treats a slot missing from the previous snapshot as a fresh Attach to
    // snap to.
    out.tongues.extend(tongues.iter().flatten().copied());
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{DEFAULT_DAMPING, REST_LENGTH};

    /// The hero as the seam exposes it: looked up by identity, never by slice
    /// position, because spawn order is not part of the contract.
    fn hero(world: &World) -> Body {
        *world
            .snapshot()
            .body(world.hero_id())
            .expect("the hero must be in every snapshot")
    }

    /// A room with a surface on every side, so a ray in any direction meets
    /// terrain. ADR-0012 assumes exactly this: every surface is anchorable, so
    /// the aim never needs to be bent.
    ///
    /// Walls at `x = 1` and `x = 8`, floor top at `y = 1`, ceiling bottom at
    /// `y = 4`, hero at `(4.5, 2.5)`.
    ///
    /// Written flush to the left margin deliberately. `from_ascii` counts one
    /// character as one world unit from column zero, leading spaces included,
    /// so an indented sketch is the same scene shifted right by its own
    /// indentation and none of the coordinates above would hold.
    fn room() -> Level {
        Level::from_ascii(
            "\
#########
#.......#
#...@...#
#.......#
#########
",
        )
    }

    /// A single Anchor surface in open space, for measuring the spring without
    /// a floor or a wall contributing anything.
    ///
    /// Ceiling bottom at `y = 9.5`, narrow enough that a Swing beneath it never
    /// reaches it. The hero starts at `(0, spawn_y)`, so `9.5 - spawn_y` is the
    /// Tongue's length at the moment it Attaches.
    fn open_sky(spawn_y: f32) -> Level {
        let level = Level {
            world_bounds: Aabb::new(Vec2::new(0.0, 0.0), Vec2::splat(500.0)),
            static_terrain: vec![Aabb::new(Vec2::new(0.0, 10.0), Vec2::new(0.5, 0.5))],
            player_spawn: Vec2::new(0.0, spawn_y),
            enemy_spawns: vec![],
        };
        level.validate().expect("fixture must be a valid level");
        level
    }

    /// Taps at a world point, as one finger going down.
    fn tap(world: &mut World, id: u64, at: Vec2) {
        world.apply_touches(&[TouchSample {
            id,
            phase: TouchPhase::Started,
            pos: at,
        }]);
    }

    /// Taps in a direction from wherever the hero is.
    fn tap_toward(world: &mut World, id: u64, direction: Vec2) {
        let from = hero(world).pos;
        tap(world, id, from + direction);
    }

    /// Fires a Tongue and returns where it Attached, failing if it did not.
    fn attach(world: &mut World, direction: Vec2) -> Vec2 {
        tap_toward(world, 1, direction);
        world.step();
        let tongues = &world.snapshot().tongues;
        assert_eq!(
            tongues.len(),
            1,
            "a tap toward {direction:?} must Attach exactly one Tongue"
        );
        tongues[0].to
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
        // Both buffers, with Tongues Attached: the Tongue list is rebuilt every
        // tick from the hero's slots, so it is the same hazard as the body list
        // and was not covered while nothing could Attach.
        let mut world = World::from_level(&room());
        let from = hero(&world).pos;
        world.apply_touches(&[
            TouchSample {
                id: 1,
                phase: TouchPhase::Started,
                pos: from + Vec2::Y,
            },
            TouchSample {
                id: 2,
                phase: TouchPhase::Started,
                pos: from + Vec2::X,
            },
        ]);
        world.step();
        world.step();
        assert_eq!(world.snapshot().tongues.len(), MAX_TONGUES);
        let bodies = world.snapshot().bodies.capacity();
        let tongues = world.snapshot().tongues.capacity();
        for _ in 0..200 {
            world.step();
        }
        assert_eq!(
            world.snapshot().bodies.capacity(),
            bodies,
            "the body buffer must not grow per tick"
        );
        assert_eq!(
            world.snapshot().tongues.capacity(),
            tongues,
            "the Tongue buffer must not grow per tick"
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

    #[test]
    fn swinging_along_a_tiled_floor_does_not_catch_on_a_seam() {
        // The same defect as the `sweep` test above, provoked through the
        // public API now that something can move the hero sideways. That test
        // asserts the geometry; this one asserts the player-visible
        // consequence, which is that a Swing along the ground does not stop
        // dead at every tile boundary.
        //
        // The hero starts at rest on a tiled floor with a wall to its right,
        // and Attaches to that wall at exactly its own height, so the pull is
        // purely horizontal and the floor contact is the only thing that could
        // interfere.
        // Flush left, as in `room`: an indented sketch is the same scene moved
        // sideways, and the wall's face below is an absolute coordinate.
        let level = Level::from_ascii(
            "\
..........#
..........#
@.........#
###########
",
        );
        let mut world = World::from_level(&level);
        let start = hero(&world).pos;
        let anchor = attach(&mut world, Vec2::X);
        assert_eq!(
            anchor,
            Vec2::new(10.0, start.y),
            "the Anchor must be the wall at the hero's own height"
        );

        // Ten ticks, because the pull is strong enough to cover the room: by
        // tick 18 the hero is against the far wall, and a contact with the
        // Anchor's own surface would stop the slide for an honest reason and
        // mask the one this test is looking for.
        let mut previous = hero(&world).pos;
        for tick in 0..10 {
            world.step();
            let now = hero(&world).pos;
            assert!(
                now.x > previous.x,
                "caught on a tile seam at tick {tick}: {previous:?} then {now:?}"
            );
            assert!(
                (now.y - start.y).abs() < 1e-3,
                "left the floor at tick {tick}: {now:?}"
            );
            previous = now;
        }
        assert!(
            previous.x - start.x > 3.0,
            "expected to cross several tile seams, only reached {previous:?}"
        );
    }

    #[test]
    fn a_tap_fires_a_tongue_that_attaches_to_the_surface_it_points_at() {
        let mut world = World::from_level(&room());
        let start = hero(&world).pos;
        assert!(
            world.snapshot().tongues.is_empty(),
            "nothing is Attached before a Gesture"
        );

        let anchor = attach(&mut world, Vec2::new(0.0, 5.0));
        assert_eq!(
            anchor,
            Vec2::new(start.x, 4.0),
            "straight up must Attach to the ceiling directly overhead"
        );

        let line = world.snapshot().tongues[0];
        assert_eq!(line.slot, 0, "the first Gesture claims the first slot");
        assert_eq!(
            line.from,
            hero(&world).pos,
            "the Tongue is drawn from where the hero is now"
        );
        assert_eq!(
            line.anchor_id, None,
            "terrain is a Fixed Anchor and has no body"
        );
    }

    #[test]
    fn every_surface_is_anchorable_including_the_floor_and_the_lower_walls() {
        // ADR-0012 calls universal anchorability load-bearing rather than a
        // simplification: Attaching downward to the floor is how descent speed
        // is built, and the canonical approach to a tough ground Enemy is to
        // hop up, Attach to the floor, and strike from above.
        let cases = [
            (Vec2::NEG_Y, Vec2::new(4.5, 1.0), "the floor below"),
            (Vec2::X, Vec2::new(8.0, 2.5), "the wall to the right"),
            (Vec2::NEG_X, Vec2::new(1.0, 2.5), "the wall to the left"),
            (Vec2::Y, Vec2::new(4.5, 4.0), "the ceiling above"),
        ];
        for (direction, expected, what) in cases {
            let mut world = World::from_level(&room());
            let anchor = attach(&mut world, direction);
            assert_eq!(anchor, expected, "failed to Attach to {what}");
        }
    }

    #[test]
    fn the_aim_is_never_bent_toward_a_closer_surface() {
        // ADR-0012, through the seam. The hero stands in a room whose nearest
        // surface by far is the floor one unit below, and taps at the ceiling
        // three times further away. A tolerance cone, which an earlier version
        // of the design carried and which a future reader will want back, would
        // pick the floor.
        let mut world = World::from_level(&room());
        let anchor = attach(&mut world, Vec2::new(0.0, 5.0));
        assert_eq!(anchor.y, 4.0, "the aim was bent downward: {anchor:?}");
    }

    #[test]
    fn hang_equilibrium_matches_theory_across_pull_strengths() {
        // ADR-0003's measurable claim: the hero hangs at REST_LENGTH + m*g/k,
        // so a stronger pull hangs closer to the rest boundary and no hero ever
        // gets inside it. Three values of k, each compared against the closed
        // form rather than against a recorded number, with the error bound
        // stated: 1e-3 world units, which is a thousandth of the hero's radius.
        //
        // The spread is deliberate. At k = 20 the hero is damped at zeta 0.335
        // and barely overshoots; at k = 120 it is at zeta 0.137, overshoots far
        // enough to carry the Tongue back inside its rest boundary, and has to
        // settle through a slack phase where no restoring force exists at all.
        // Both must land on the same formula.
        const TOLERANCE: f32 = 1e-3;
        for pull_strength in [20.0f32, 40.0, 120.0] {
            let mut world = World::from_level(&open_sky(8.0));
            world.set_hero_pull_strength(pull_strength);
            let anchor = attach(&mut world, Vec2::Y);

            for _ in 0..20_000 {
                world.step();
            }

            let expected = REST_LENGTH + DEFAULT_PLAYER_MASS * GRAVITY.length() / pull_strength;
            let measured = (anchor - hero(&world).pos).length();
            assert!(
                (measured - expected).abs() < TOLERANCE,
                "at k {pull_strength} expected to hang at {expected}, measured {measured}"
            );
        }
    }

    #[test]
    fn a_slack_tongue_leaves_the_hero_in_free_fall() {
        // The first of ADR-0003's three zones: inside the rest boundary a
        // Tongue is slack and applies nothing. A web never pushes, so an
        // Attached but slack Tongue must be indistinguishable from no Tongue at
        // all.
        let level = open_sky(8.8);
        let spawn = level.player_spawn;
        let mut world = World::from_level(&level);
        let anchor = attach(&mut world, Vec2::Y);
        assert!(
            (anchor - spawn).length() < REST_LENGTH,
            "fixture must start inside the rest boundary"
        );
        assert_eq!(
            world.snapshot().tongues.len(),
            1,
            "the Tongue must really be Attached, or this test proves nothing"
        );

        // One tick was spent Attaching, so this is tick 20 of the fall.
        const TICKS: u32 = 20;
        for _ in 1..TICKS {
            world.step();
        }

        // The closed form, from the physics rather than from the code. Velocity
        // Verlet is exact under constant acceleration, so a slack Tongue must
        // leave this untouched to float accuracy.
        let t = TICKS as f32 * DT;
        let expected = spawn.y - 0.5 * GRAVITY.length() * t * t;
        let after = hero(&world).pos;
        assert!(
            (after.y - expected).abs() < 1e-5,
            "a slack Tongue changed the fall: expected {expected}, got {}",
            after.y
        );
        assert_eq!(after.x, spawn.x, "a slack Tongue pulled sideways");
        assert!(
            (anchor - after).length() < REST_LENGTH,
            "the fixture left the slack zone before the measurement ended"
        );
    }

    #[test]
    fn the_tongue_never_pushes_even_when_the_hero_races_inward() {
        // The clamp in the force law, through the seam. The hero is fired at
        // its own Anchor fast enough that the damping term alone is four times
        // the spring term, so an unclamped force would invert and shove the
        // hero back down hard. Clamped, the only force left is gravity, and the
        // hero travels exactly as far as a thrown stone would.
        let mut world = World::from_level(&open_sky(0.0));
        let anchor = attach(&mut world, Vec2::Y);
        let start = hero(&world).pos;

        let inward = 500.0;
        let stretch = (anchor - start).length() - REST_LENGTH;
        assert!(
            DEFAULT_DAMPING * inward > world.hero_pull_strength() * stretch,
            "fixture must race in hard enough for damping to dominate"
        );
        world.place(world.hero_id(), start, Vec2::new(0.0, inward));

        world.step();
        let expected = start + Vec2::new(0.0, inward) * DT + 0.5 * GRAVITY * DT * DT;
        let measured = hero(&world).pos;
        assert!(
            (measured - expected).length() < 1e-4,
            "the spring pushed back: ballistic travel would end at {expected:?}, got {measured:?}"
        );
    }

    #[test]
    fn tangential_velocity_and_an_inward_pull_produce_a_bounded_swing() {
        // The mechanic itself. Nothing in the simulation models a pendulum:
        // the spring pulls inward, the hero's own momentum carries across it,
        // and the Swing is what emerges. ADR-0003.
        //
        // The claim under test is that it stays bounded. An integrator that
        // injected energy would not fail visibly for a few hundred ticks, so
        // this runs a full minute of Sim Ticks and budgets the total energy,
        //
        //     E = 0.5*m*v^2 + m*g*y + 0.5*k*stretch^2
        //
        // which a one-way damped spring can only spend. The force ceiling never
        // binds at this stretch, so that spring term is the exact potential and
        // not an approximation of it.
        let mut world = World::from_level(&open_sky(8.0));
        let hero_id = world.hero_id();
        let k = world.hero_pull_strength();
        let anchor = attach(&mut world, Vec2::Y);
        // Across the pull rather than along it, which is what makes the motion
        // orbital rather than a bounce up and down the Tongue.
        world.place(hero_id, hero(&world).pos, Vec2::new(2.5, 0.0));

        let energy = |world: &World| {
            let pos = hero(world).pos;
            let vel = world.body_velocity(hero_id).expect("the hero is alive");
            let stretch = ((anchor - pos).length() - REST_LENGTH).max(0.0);
            0.5 * DEFAULT_PLAYER_MASS * vel.length_squared()
                + DEFAULT_PLAYER_MASS * GRAVITY.length() * pos.y
                + 0.5 * k * stretch * stretch
        };

        const TICKS: u32 = 3600;
        let budget = energy(&world);
        let (mut left, mut right) = (false, false);
        let (mut late_left, mut late_right) = (false, false);
        let mut furthest = 0.0f32;
        for tick in 0..TICKS {
            world.step();
            let pos = hero(&world).pos;
            assert!(pos.is_finite(), "diverged at tick {tick}: {pos:?}");
            // 1e-3 of the budget: Velocity Verlet evaluates its second
            // acceleration at the old velocity, which is not exactly
            // symplectic, so a little drift is expected and a little is all
            // that is allowed.
            assert!(
                energy(&world) <= budget * 1.001,
                "energy grew at tick {tick}: budget {budget}, now {}",
                energy(&world)
            );
            left |= pos.x < anchor.x - 0.5;
            right |= pos.x > anchor.x + 0.5;
            if tick > TICKS - 600 {
                late_left |= pos.x < anchor.x;
                late_right |= pos.x > anchor.x;
            }
            furthest = furthest.max((anchor - pos).length());
        }

        assert!(
            left && right,
            "the hero never swung across the Anchor, so this was a fall and not a Swing"
        );
        assert!(
            furthest < 4.0,
            "the Swing escaped: reached {furthest} from the Anchor"
        );
        // Damping is real, so some energy is gone by the end.
        assert!(
            energy(&world) < budget,
            "a damped spring must have spent something over a minute of Swinging"
        );
        // And the hero is still Swinging, which is not a defect but the point.
        // ADR-0003's damping term is `damping * radial_vel`, so it opposes
        // motion along the Tongue and nothing else. A Swing is almost entirely
        // across the Tongue, where the radial velocity is near zero, so the
        // angular motion is barely damped at all and a minute of it does not
        // die out under the player. That asymmetry is why the Tongue settles to
        // a hang length when the hero is dropped straight down it and keeps
        // orbiting when the hero arrives sideways, and it is worth knowing
        // before anyone tunes `DEFAULT_DAMPING` expecting a Swing to decay.
        assert!(
            late_left && late_right,
            "the Swing had died out, which radial damping alone cannot do"
        );
    }

    #[test]
    fn a_newly_attached_tongue_snaps_rather_than_blending() {
        // Spec story 47: a Tongue that has just Attached must appear already
        // connected rather than stretching out of nowhere. The Core Sim's half
        // of that is leaving the slot absent from the previous snapshot, which
        // is what makes the frame contract snap instead of lerping the far end
        // in from a stale position.
        let mut world = World::from_level(&room());
        let anchor = attach(&mut world, Vec2::new(0.0, 5.0));
        assert!(
            world.previous_snapshot().tongues.is_empty(),
            "the tick before the Attach must carry no Tongue in that slot"
        );

        let mut view = Snapshot::default();
        for alpha in [0.0f32, 0.25, 0.5, 1.0] {
            crate::frame::interpolate_into(
                world.previous_snapshot(),
                world.snapshot(),
                alpha,
                &mut view,
            );
            assert_eq!(
                view.tongues[0].to, anchor,
                "at alpha {alpha} the Tongue was drawn short of its Anchor"
            );
        }
    }

    #[test]
    fn lifting_the_finger_does_not_release_an_attached_tongue() {
        // ADR-0006, and the specific platform trap it names: Macroquad flips
        // every live touch to `Ended` when the app minimizes. Under the earlier
        // hold-to-Swing scheme that was a free Release on every suspend. Under
        // discrete tap control attachment outlasts the Gesture, so the input
        // layer must never infer Release from touch loss.
        let mut world = World::from_level(&room());
        attach(&mut world, Vec2::new(0.0, 5.0));

        for phase in [TouchPhase::Ended, TouchPhase::Cancelled] {
            world.apply_touches(&[TouchSample {
                id: 1,
                phase,
                pos: Vec2::ZERO,
            }]);
            world.step();
            assert_eq!(
                world.snapshot().tongues.len(),
                1,
                "{phase:?} must not Release the Tongue"
            );
        }
    }

    #[test]
    fn a_held_finger_fires_once_and_not_once_per_frame() {
        // A held finger reports `Stationary` every frame, never `Moved`. If
        // that re-fired, one thumb pressed on the screen would claim both slots
        // on consecutive frames and the hero would be Attached twice from a
        // single tap.
        let mut world = World::from_level(&room());
        tap_toward(&mut world, 1, Vec2::new(0.0, 5.0));
        for _ in 0..30 {
            world.apply_touches(&[TouchSample {
                id: 1,
                phase: TouchPhase::Stationary,
                pos: hero(&world).pos + Vec2::new(5.0, 0.0),
            }]);
            world.step();
        }
        assert_eq!(
            world.snapshot().tongues.len(),
            1,
            "holding the finger down fired more than once"
        );
    }

    #[test]
    fn a_third_concurrent_finger_is_discarded() {
        // ADR-0006 caps the hero at two Tongues. A stray touch while two
        // fingers are already down must do nothing at all rather than something
        // surprising, because it is the kind of accident that would otherwise
        // cost a player a run.
        let mut world = World::from_level(&room());
        let from = hero(&world).pos;
        let down = |id, aim: Vec2| TouchSample {
            id,
            phase: TouchPhase::Started,
            pos: from + aim,
        };
        world.apply_touches(&[
            down(1, Vec2::new(0.0, 5.0)),
            down(2, Vec2::new(-5.0, 0.0)),
            down(3, Vec2::new(5.0, 0.0)),
        ]);
        world.step();

        let tongues = &world.snapshot().tongues;
        assert_eq!(tongues.len(), MAX_TONGUES, "a third Tongue was Attached");
        let slots: Vec<u8> = tongues.iter().map(|t| t.slot).collect();
        assert_eq!(slots, vec![0, 1], "both slots must be filled exactly once");
    }

    #[test]
    fn slot_assignment_does_not_depend_on_the_order_touches_arrive_in() {
        // The regression this prevents is invisible until it desynchronises a
        // replay. Macroquad's `touches()` collects from a `HashMap`, so two
        // fingers landing in the same frame arrive in whatever order that
        // hashing produces. Taking them as given would assign slots by the
        // platform's hashing, and the same two taps would produce a different
        // Snapshot sequence between runs even though the physics agreed.
        let ceiling = Vec2::new(0.0, 5.0);
        let wall = Vec2::new(-5.0, 0.0);
        let traces: Vec<Vec<TongueLine>> =
            [[(7u64, ceiling), (3, wall)], [(3, wall), (7, ceiling)]]
                .iter()
                .map(|order| {
                    let mut world = World::from_level(&room());
                    let from = hero(&world).pos;
                    let touches: Vec<TouchSample> = order
                        .iter()
                        .map(|(id, aim)| TouchSample {
                            id: *id,
                            phase: TouchPhase::Started,
                            pos: from + *aim,
                        })
                        .collect();
                    world.apply_touches(&touches);
                    world.step();
                    world.snapshot().tongues.clone()
                })
                .collect();

        assert_eq!(traces[0].len(), MAX_TONGUES);
        assert_eq!(
            traces[0], traces[1],
            "slot assignment followed the iteration order rather than the touch ids"
        );
    }

    #[test]
    fn a_tap_on_the_hero_itself_does_not_attach_anything() {
        // Hostile input. A tap landing exactly on the hero is a zero-length aim
        // vector, and normalizing that produces NaN, which fired into the level
        // as an Anchor would poison every tick after it.
        let mut world = World::from_level(&room());
        let on_the_hero = hero(&world).pos;
        tap(&mut world, 1, on_the_hero);
        for _ in 0..60 {
            world.step();
        }
        assert!(
            world.snapshot().tongues.is_empty(),
            "a directionless tap Attached something"
        );
        assert!(
            hero(&world).pos.is_finite(),
            "a directionless tap produced {:?}",
            hero(&world).pos
        );
    }

    #[test]
    fn a_tap_into_open_sky_attaches_nothing_and_leaves_the_slot_free() {
        // An open sketch has no surface in every direction, unlike the enclosed
        // levels ADR-0012 assumes. Inventing an Anchor at some arbitrary range
        // would be worse than Attaching nothing.
        let mut world = World::from_level(&open_sky(8.0));
        tap_toward(&mut world, 1, Vec2::NEG_Y);
        world.step();
        assert!(world.snapshot().tongues.is_empty());

        // And the slot is still usable by the next tap, which is the part worth
        // checking: a miss must not consume a Tongue.
        world.apply_touches(&[TouchSample {
            id: 1,
            phase: TouchPhase::Ended,
            pos: Vec2::ZERO,
        }]);
        let anchor = attach(&mut world, Vec2::Y);
        assert_eq!(anchor, Vec2::new(0.0, 9.5));
    }

    #[test]
    fn two_tongues_on_one_anchor_pull_harder_than_one() {
        // Ticket #14 owns the exact ratios. What belongs here is that the sum
        // happens at all, because a second Tongue that silently replaced the
        // first would look identical in every single-Tongue test above.
        let pulled = |tongues: usize| {
            let mut world = World::from_level(&open_sky(0.0));
            let from = hero(&world).pos;
            let touches: Vec<TouchSample> = (0..tongues)
                .map(|i| TouchSample {
                    id: i as u64,
                    phase: TouchPhase::Started,
                    pos: from + Vec2::Y,
                })
                .collect();
            world.apply_touches(&touches);
            world.step();
            assert_eq!(world.snapshot().tongues.len(), tongues);
            hero(&world).pos.y - from.y
        };

        let one = pulled(1);
        let two = pulled(2);
        assert!(one > 0.0, "a single Tongue did not pull the hero up");
        assert!(
            two > one * 1.5,
            "two Tongues on one Anchor pulled {two} against one Tongue's {one}"
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
