//! The `World`: the one thing that owns simulation state and advances it.
//!
//! This is the single test seam. Everything a player can do is reachable from
//! this module's public API, and the Shell is an adapter over it with no game
//! logic of its own. See ADR-0001 for why the boundary is here, ADR-0002 for the
//! fixed Sim Tick, ADR-0007 for the integrator, and ADR-0009 for the two
//! snapshots this type retains.

use crate::combat;
use crate::combo::{Combo, HeroState};
use crate::despawn;
use crate::frame::{Body, BodyKind, Snapshot, TongueLine};
use crate::input::{self, Claim, GestureTracker, TouchPhase, TouchSample};
use crate::level::{Aabb, Level};
use crate::movable_anchor::{self, Target};
use crate::thrown;
use crate::tongue::{Pulls, Tongue, Tongues};
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
    /// The level's Kill Bound, copied from its data at construction. World-space
    /// and nothing else: the Core Sim has no screen, so there is nothing here a
    /// viewport could leak into. See [`crate::despawn`].
    kill_bound: Aabb,
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
    /// The hero's unbroken run of Slams. Mutated once per Sim Tick, from
    /// [`World::step`] alone, and deliberately unreachable from `body_force`:
    /// see [`crate::combo`].
    combo: Combo,
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
    /// One Sim Tick's working state, one entry per body, kept between ticks so
    /// that a tick allocates nothing once the run is warm. Cleared and refilled
    /// by [`World::step`] and meaningless outside it.
    half_step: Vec<HalfStep>,
}

/// One body between the two halves of a Sim Tick.
///
/// Velocity Verlet needs the first acceleration again after the position step,
/// and a mutual force needs every body's new position before any body's second
/// acceleration can be taken, so the tick cannot be a single pass per body.
/// This is what the first pass leaves behind for the second.
#[derive(Clone, Copy, Debug)]
struct HalfStep {
    /// Whose half-step this is.
    ///
    /// Carried so that anything reading this list after a body has been removed
    /// asks by identity rather than by slice position. The two agree while the
    /// list is intact, and `resolve_impacts` can shorten the body list in the
    /// middle of the tick, after which an index would silently name a different
    /// body. The same rule the frame contract and the Movable Anchor already
    /// follow, for the same reason.
    id: EntityId,
    /// Where the body was when the tick began, before the position step.
    ///
    /// A collision is a question about the path a body travelled and not about
    /// where it ended up, so the Thrown matrix needs both ends of it.
    start: Vec2,
    /// Where the body ended up, after integration and terrain contact.
    pos: Vec2,
    /// The velocity the body began the tick with, until the second pass
    /// overwrites it with the one the body ends the tick with.
    ///
    /// Two meanings in one field, deliberately: ADR-0007 evaluates the second
    /// acceleration at the new position and the **old** velocity, so the old
    /// one has to survive the position write, and the new one has to wait here
    /// until every body has been evaluated so that no body reads another's new
    /// velocity half way through the pass.
    vel: Vec2,
    a0: Vec2,
    contacts: Contacts,
    /// The tension the body's Tongues put on it, from this tick's first
    /// evaluation: the Tongue force alone, over the body's own mass.
    ///
    /// Recorded here rather than recomputed later because it is the tension
    /// that actually acted this tick, read at the configuration the tick began
    /// in. ADR-0011 derives the Thrown promotion from it, and a promotion is a
    /// mutation, so the reading has to survive the integration step and be
    /// applied after it.
    tension: f32,
}

impl World {
    /// Builds a world from level data: a hero at `player_spawn`, and an Enemy
    /// per entry in `enemy_spawns`.
    pub fn from_level(level: &Level) -> Self {
        let mut world = World {
            bodies: Vec::with_capacity(1 + level.enemy_spawns.len()),
            static_terrain: level.static_terrain.clone(),
            kill_bound: level.world_bounds,
            hero: EntityId(0),
            tongues: [None; MAX_TONGUES],
            gestures: GestureTracker::new(),
            hero_pull_strength: DEFAULT_PULL_STRENGTH,
            combo: Combo::default(),
            next_id: 0,
            tick: 0,
            current: Snapshot::default(),
            previous: Snapshot::default(),
            half_step: Vec::with_capacity(1 + level.enemy_spawns.len()),
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
    ///
    /// **Every body takes each evaluation before any body takes the next**, in
    /// two passes rather than one pass per body. With only Fixed Anchors the
    /// two arrangements are bit-identical, because nothing one body's force
    /// reads is anything another body changed. A Tongue on a Movable Anchor
    /// makes them differ: the hero's force depends on where the Enemy is, so a
    /// per-body loop would hand the second body of the pair the first body's
    /// *new* position while the first had seen the second's old one, and the
    /// two ends of one Tongue would pull by different amounts. Two passes let
    /// both ends be computed from one configuration, which is what makes the
    /// mass-ratio pull in ADR-0003 exactly equal and opposite and what makes
    /// one Tongue on terrain and one on an Enemy a single three-body system
    /// rather than two independent pairs.
    pub fn step(&mut self) {
        // Where the hero began the tick, kept for the combat sweep below: an
        // impact is a question about the path the hero travelled and not about
        // where it ended up.
        let hero_start = self
            .bodies
            .iter()
            .find(|b| b.id == self.hero)
            .map(|b| b.pos);

        // Whether the hero ended the tick against terrain, read off the sweep
        // that resolved it rather than re-derived afterwards. The Combo's rest
        // predicate needs it, and the sweep is the only thing that knows.
        let mut hero_touching_terrain = false;

        // Taken out of `self` rather than borrowed, so the force evaluation can
        // keep reading every body while this is being filled. Put back at the
        // end of the tick, so its capacity survives and a warm tick allocates
        // nothing.
        let mut half_step = core::mem::take(&mut self.half_step);
        half_step.clear();

        // First pass: the first of ADR-0007's two evaluations for every body,
        // the position step, and terrain contact. Anchors are resolved here
        // from the state the tick began in.
        for i in 0..self.bodies.len() {
            let body = self.bodies[i];
            let pulls = self.pulls_on(body.id);

            let a0 = body_force(&body, pulls) / body.mass;
            let target = body.pos + body.vel * DT + 0.5 * a0 * DT * DT;

            // Terrain is a constraint on motion and not a force, which is what
            // keeps `body_force` pure: contact is resolved out here, once, and
            // never inside an acceleration that gets evaluated twice.
            let (pos, contacts) = sweep(body.pos, target, PLAYER_RADIUS, &self.static_terrain);
            if body.id == self.hero {
                hero_touching_terrain = contacts.count > 0;
            }
            half_step.push(HalfStep {
                id: body.id,
                start: body.pos,
                pos,
                vel: body.vel,
                a0,
                contacts,
                // The Tongue force alone and not `a0`, because ADR-0011's
                // tension is the Tongue's: gravity is not a yank, and a body
                // with no Tongue on it at all must read zero rather than one
                // gravity. `Pulls::force` is pure, which is what makes reading
                // it a second time here safe: it cannot disagree with the
                // reading inside `body_force`, and it cannot observe or change
                // anything either reading did.
                tension: thrown::tension(pulls.force(body.pos, body.vel), body.mass),
            });
        }

        // The new positions are published between the passes. Velocities are
        // deliberately left alone: ADR-0007's second evaluation wants the new
        // position and the old velocity, and leaving `vel` untouched is what
        // supplies that without carrying a second copy of it around.
        for (body, step) in self.bodies.iter_mut().zip(half_step.iter()) {
            body.pos = step.pos;
        }

        // Second pass: the second evaluation, against the configuration every
        // body is now in, and the velocity step.
        //
        // The new velocities are parked in `half_step` and published only once
        // every body has been evaluated. Writing them straight back would let a
        // body evaluated later in the pass read a counterpart's *new* velocity
        // while the counterpart had read its old one, and the damping term on
        // the two ends of one Tongue would then disagree by `damping` times the
        // tick's whole change in closing speed. That was measured: a 3 unit
        // Enemy 10 units out gained 0.149 of momentum from nowhere on the first
        // tick alone, growing every tick after it.
        for (body, step) in self.bodies.iter().zip(half_step.iter_mut()) {
            let a1 = body_force(body, self.pulls_on(body.id)) / body.mass;

            // Cancelling after the velocity half-step and not before is
            // load-bearing: the half-step adds another tick of gravity, and
            // leaving that in would let a resting body build downward speed
            // against the floor and eventually creep through it.
            step.vel = step
                .contacts
                .cancel_into_surfaces(step.vel + 0.5 * (step.a0 + a1) * DT);
        }
        for (body, step) in self.bodies.iter_mut().zip(half_step.iter()) {
            body.vel = step.vel;
        }

        self.half_step = half_step;

        // Impacts are impulses and not forces, so they are resolved here, once,
        // after the integration step. Putting any of this inside `body_force`
        // would have ADR-0007's integrator apply it twice per Sim Tick.
        let slams = match hero_start {
            Some(start) => self.resolve_impacts(start),
            None => 0,
        };

        // ADR-0011's Thrown state: what a Thrown Enemy's inverted matrix did
        // this tick, and then which Enemies this tick's tension has taken out
        // of their own control. Both are mutations, so they sit here with the
        // impacts rather than anywhere a force could reach them.
        self.resolve_thrown();

        // The Combo, once, after the impacts it counts and after the contact
        // that can break it. It is mutated state, so the same rule that keeps
        // impacts out of `body_force` keeps this out: ADR-0007's integrator
        // evaluates acceleration twice per Sim Tick, and a count advanced from
        // inside a force would advance twice as fast with nothing to say so.
        // Everything it decides on lives in [`crate::combo`]; this is wiring.
        self.combo.after_impacts(
            slams,
            HeroState {
                touching_terrain: hero_touching_terrain,
                // End of tick, so an impact that drained the hero is already in
                // it. A hero whose momentum a Slam just took is slower for the
                // rest predicate's purposes on this tick, not the next one.
                speed: self
                    .body(self.hero)
                    .map(|b| b.vel.length())
                    .unwrap_or_default(),
            },
        );

        // Despawn last, and here rather than anywhere else in the tick. It is a
        // mutation, so it must not be reachable from `body_force`, which
        // ADR-0007's integrator evaluates twice per Sim Tick; and it reads the
        // position motion and contact have already settled, so a body is judged
        // on where the tick actually left it. Removing bodies before the
        // snapshot is written is what makes a despawned body simply absent from
        // the `Snapshot`, which is all the Shell ever needs to know about it.
        self.bodies
            .retain(|b| !despawn::left_kill_bound(&self.kill_bound, b.pos));

        // And last of all, the slots. ADR-0014: a Tongue whose Movable Anchor
        // has left the simulation forces a Release. This runs after *every* way
        // of losing a body: the emptied `Toughness` pool in `resolve_impacts`,
        // ADR-0011's Thrown matrix in `resolve_thrown`, and the Kill Bound
        // above. It is the same rule for all three and one sweep cannot
        // disagree with itself, which is also why this is here rather than
        // inside any of the removals: a slot is judged against the body list
        // the tick actually ended with.
        //
        // Nothing is spent and no impulse is applied, so the hero carries
        // exactly the momentum it had. ADR-0012 makes that the whole of a
        // Release.
        movable_anchor::release_dead_anchors(&mut self.tongues, |id| {
            self.bodies.iter().any(|b| b.id == id)
        });

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
            let Some(claim) = self.gestures.claim(touch.id, vacant) else {
                break;
            };
            match claim {
                Claim::Fire(slot) => self.fire(slot, touch.pos),
                Claim::Release(slot) => self.release(slot),
            }
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
    /// and the ray goes exactly along it. ADR-0012: the aim is never bent, and
    /// the Enemy Assist Arc in [`movable_anchor::cast_anchor`] changes which
    /// body is preferred and never the direction.
    fn fire(&mut self, slot: u8, tapped: Vec2) {
        let Some(hero) = self.body(self.hero) else {
            return;
        };
        let from = hero.pos;
        // Borrowed as an iterator rather than collected, so a tap allocates
        // nothing. Only `Enemy` bodies are targets: the hero cannot Attach to
        // itself, and ADR-0015 decides that a Thrown Enemy is not a target
        // either. A weapon in flight is not grappable, so a filter that was
        // waiting on that question is now the filter that answers it.
        //
        // It forbids re-targeting and nothing else. A Tongue already Attached
        // to an Enemy that becomes Thrown keeps holding it, which is what makes
        // ADR-0011's "if the player does not Release, it barrels back toward the
        // hero" possible at all.
        let targets = self
            .bodies
            .iter()
            .filter(|b| b.kind == BodyKind::Enemy)
            .map(|b| Target {
                id: b.id,
                // The exact centre of mass, which ADR-0012 allows no
                // alternative to: a surface attachment would put the spring
                // force off the line between the two centres and so imply a
                // torque that bodies with no angular dynamics cannot carry.
                centre: b.pos,
                half_extents: combat::ENEMY_HALF_EXTENTS,
            });
        // A ray into open sky meets nothing and fires nothing. ADR-0012 says a
        // fired Tongue always connects, which holds for an enclosed level; a
        // sketch with open sky is a test fixture, and the honest answer there is
        // that the slot stays empty rather than an Anchor being invented.
        if let Some(anchor) =
            movable_anchor::cast_anchor(from, tapped - from, &self.static_terrain, targets)
        {
            self.tongues[slot as usize] = Some(Tongue { anchor });
        }
    }

    /// Releases the Tongue in `slot`, if it holds one.
    ///
    /// The whole of retraction. Clearing the slot is all it takes, and that is
    /// the mechanic rather than a shortcut: the Tongue's only effect on the
    /// hero is the spring force in [`World::pulls_on`], so the tick after this
    /// the hero carries exactly the momentum it had under gravity alone, which
    /// is ADR-0012's instant retraction and the ballistic arc in one.
    ///
    /// Nothing is spent and nothing can fail. There is no in-flight state to
    /// abort, no cooldown, and no cost, so there is no whiff case: ADR-0012.
    fn release(&mut self, slot: u8) {
        self.tongues[slot as usize] = None;
    }

    /// Every Tongue pulling on one body this Sim Tick, with its Anchor resolved
    /// to a world-space point.
    /// Every Tongue pulling on one body this Sim Tick, with its Anchor resolved
    /// to a world-space point and that point's own velocity.
    ///
    /// Only the hero holds Tongues, but a Tongue has two ends. The hero feels
    /// one pull per Attached slot; a body serving as a Movable Anchor feels the
    /// reaction, which ADR-0003 makes the mass-ratio pull: the same force, the
    /// other way round, divided by its own mass rather than the hero's.
    ///
    /// The reaction is a **force** and not an impulse, which is why it is here
    /// and not beside the impacts. ADR-0007's integrator evaluates this twice
    /// per Sim Tick, and anything applied here once per tick would be applied
    /// twice.
    fn pulls_on(&self, body: EntityId) -> Pulls {
        let mut pulls = Pulls::default();
        if body == self.hero {
            for tongue in self.tongues.iter().flatten() {
                if let Some((point, vel)) = self.anchor_state(tongue.anchor) {
                    pulls.push_moving(point, vel, self.hero_pull_strength);
                }
            }
            return pulls;
        }

        let Some(hero) = self.body(self.hero) else {
            return pulls;
        };
        // The hero's `k` and not the Enemy's: Hooke's constant belongs to the
        // Tongue, and so to the hero who fired it. An Enemy has none of its
        // own. The ceiling is applied per pull, so an Enemy held by both slots
        // can feel up to `MAX_TONGUES * DEFAULT_FORCE_CEILING`, the same bound
        // ADR-0003 records for the hero and for the same reason.
        for tongue in self.tongues.iter().flatten() {
            if tongue.anchor == Anchor::Movable(body) {
                pulls.push_moving(hero.pos, hero.vel, self.hero_pull_strength);
            }
        }
        pulls
    }

    /// Where an Anchor is right now, and how fast it is moving.
    ///
    /// The velocity is half the answer rather than an extra: the damping term
    /// is taken against the relative radial velocity, so a Tongue on a moving
    /// Enemy that read the hero's velocity alone would pull the two ends by
    /// different amounts and the pair would gain momentum out of nothing. A
    /// Fixed Anchor reports zero, which is simply true of terrain.
    fn anchor_state(&self, anchor: Anchor) -> Option<(Vec2, Vec2)> {
        match anchor {
            Anchor::Fixed(point) => Some((point, Vec2::ZERO)),
            // Resolved by identity and never by slice position, so a spawn or a
            // despawn elsewhere cannot silently re-point a Tongue. A body that
            // has despawned resolves to nothing rather than to a stale
            // position, which is what makes the force and the drawn line vanish
            // with it; ADR-0014 adds the slot.
            Anchor::Movable(id) => self.body(id).map(|b| (b.pos, b.vel)),
        }
    }

    /// Where an Anchor is right now, for the line the Shell draws.
    fn anchor_point(&self, anchor: Anchor) -> Option<Vec2> {
        self.anchor_state(anchor).map(|(point, _)| point)
    }

    fn body(&self, id: EntityId) -> Option<&BodyState> {
        self.bodies.iter().find(|b| b.id == id)
    }

    fn write_current_snapshot(&mut self) {
        let lines = self.tongue_lines();
        write_snapshot(
            self.tick,
            self.combo.hits(),
            &self.bodies,
            &lines,
            &mut self.current,
        );
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
                anchor: tongue.anchor,
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

    /// Resolves the hero's pass over this Sim Tick against every Enemy, removes
    /// whatever it destroyed, and reports how many Slams landed.
    ///
    /// Thin by design: every rule lives in [`crate::combat`] as a pure function,
    /// and this is the one place in the tick allowed to write the result back.
    /// Enemies are visited in body order, which is stable for a run, so the
    /// outcome of a tick that touches two of them is deterministic.
    ///
    /// The count returned is of Slams and not of impacts: a Kick is not a hit.
    /// More than one can land in a single tick, because a hero fast enough to
    /// clear the pass-through ceiling crosses two Enemy boxes inside one Sim
    /// Tick, and both are hits.
    fn resolve_impacts(&mut self, hero_start: Vec2) -> u32 {
        let Some(h) = self.bodies.iter().position(|b| b.id == self.hero) else {
            return 0;
        };
        let mut slams = 0;

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

            if Combo::counts_as_slam(resolved.band) {
                slams += 1;
            }
        }

        // An emptied pool is destruction, so there is one rule and not two. The
        // hero's pool is infinite, which is how the deferred hero fail state
        // stays unimplemented rather than accidentally arriving here.
        self.bodies.retain(|b| b.toughness > 0.0);
        slams
    }

    /// Resolves ADR-0011's Thrown state for this Sim Tick: what the Thrown
    /// bodies destroyed, and which Enemies became Thrown.
    ///
    /// Thin by design, like [`World::resolve_impacts`]: every rule lives in
    /// [`crate::thrown`] as a pure function and this is wiring plus the one
    /// place allowed to write the result back.
    ///
    /// **The matrix is resolved before the promotion, and the order is the
    /// rule.** ADR-0011 gives the tracking band environmental immunity, so a
    /// body that was below the threshold when this tick's motion began keeps
    /// that immunity for the whole of it: it slides along terrain and is
    /// promoted at the end of the tick, and the inverted matrix governs its
    /// next tick. Promoting first would destroy an Enemy against the very
    /// surface it was immune to while it was being pulled off it.
    fn resolve_thrown(&mut self) {
        for i in 0..self.bodies.len() {
            if self.bodies[i].kind != BodyKind::Thrown {
                continue;
            }
            let Some(step) = self.half_step_of(i).copied() else {
                continue;
            };
            // Terrain, the first row: destroyed instantly. Read off the sweep
            // that already resolved this tick's motion rather than tested
            // again, so "struck terrain" means exactly what the integrator
            // means by it, swept and so immune to tunnelling at any speed.
            //
            // First, and a projectile it destroys ploughs nothing: terrain is
            // the row that ends the state, and a weapon that met a wall and an
            // Enemy on the same tick should not collect the reward on its way
            // out.
            if step.contacts.count > 0 {
                self.destroy(i);
                continue;
            }

            for j in 0..self.bodies.len() {
                if self.bodies[i].toughness <= 0.0 {
                    // Vaporized by an earlier victim in this same tick. A
                    // destroyed weapon strikes nothing further.
                    break;
                }
                // Only an `Enemy` is a target, which is the same filter
                // [`World::fire`] uses and holds for the same reason: ADR-0015
                // takes a Thrown Enemy out of the target set entirely, so two
                // weapons in flight pass through each other.
                //
                // The hero is excluded by that filter, and **that is the
                // deliberate gap.** ADR-0011 makes a Thrown Enemy striking the
                // hero destroy the hero outright, and the hero's fail state,
                // any lose condition, run structure and restart are all
                // deferred in the map's fog, so there is nothing for the branch
                // to trigger. The seam is exactly here: a Thrown body whose
                // swept pass meets the hero is the condition, and what it would
                // do is the thing nobody has decided yet. Confirmed with the
                // driving dev on ticket #18.
                if j == i
                    || self.bodies[j].kind != BodyKind::Enemy
                    || self.bodies[j].toughness <= 0.0
                {
                    continue;
                }
                // Rebuilt per victim from the live projectile, so two strikes
                // in one tick compound rather than the second overwriting the
                // first. The motion is the tick's and does not change: one pass
                // can meet two bodies.
                let projectile = thrown::Projectile {
                    start: step.start,
                    end: self.bodies[i].pos,
                    vel: self.bodies[i].vel,
                    mass: self.bodies[i].mass,
                };
                let victim = thrown::Victim {
                    pos: self.bodies[j].pos,
                    vel: self.bodies[j].vel,
                    mass: self.bodies[j].mass,
                };
                match thrown::plough(&projectile, &victim) {
                    None => {}
                    Some(thrown::Plough::Through { vel }) => {
                        self.destroy(j);
                        self.bodies[i].vel = vel;
                    }
                    Some(thrown::Plough::Mutual) => {
                        self.destroy(j);
                        self.destroy(i);
                    }
                }
            }
        }

        // The promotion. ADR-0011: above the tension threshold the Enemy
        // becomes Thrown, and the change is discrete, so it snaps rather than
        // blending; ADR-0009's interpolation already does that for any change
        // of kind.
        for i in 0..self.bodies.len() {
            let thrown = self
                .half_step_of(i)
                .is_some_and(|s| s.tension >= thrown::THROW_TENSION);
            if thrown && self.bodies[i].kind == BodyKind::Enemy {
                self.bodies[i].kind = BodyKind::Thrown;
            }
        }

        // One removal for everything this function destroyed, through the same
        // emptied pool `resolve_impacts` uses, so destruction is one rule and
        // not two. The hero's pool is infinite, which is how the deferred hero
        // fail state stays unimplemented rather than arriving here by accident.
        self.bodies.retain(|b| b.toughness > 0.0);
    }

    /// This tick's half-step for the body at `index`, asked by identity.
    ///
    /// `None` once a body has been added mid-tick, which nothing does yet, and
    /// never for a body that was integrated this tick. The lookup is by id
    /// rather than by the same index because `resolve_impacts` can remove
    /// bodies before this is read, after which the two lists no longer line up.
    fn half_step_of(&self, index: usize) -> Option<&HalfStep> {
        let id = self.bodies[index].id;
        self.half_step.iter().find(|s| s.id == id)
    }

    /// Marks the body at `index` destroyed, by the one route destruction has.
    ///
    /// An emptied `Toughness` pool, because that is already what destruction
    /// means everywhere else in the Core Sim. A separate flag would be a second
    /// way to be dead for the single `retain` to disagree with.
    fn destroy(&mut self, index: usize) {
        self.bodies[index].toughness = 0.0;
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
    combo: u32,
    bodies: &[BodyState],
    tongues: &[Option<TongueLine>],
    out: &mut Snapshot,
) {
    out.tick = tick;
    out.combo = combo;
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

    use crate::level::EnemySpawn;
    use crate::tongue;
    use crate::{DEFAULT_DAMPING, DEFAULT_FORCE_CEILING, REST_LENGTH};

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

    /// Lifts a finger. The end of a Gesture and not a Release: ADR-0006 makes
    /// attachment outlast the finger, so this frees the slot's *ownership* and
    /// leaves its Tongue Attached.
    fn lift(world: &mut World, id: u64) {
        world.apply_touches(&[TouchSample {
            id,
            phase: TouchPhase::Ended,
            pos: Vec2::ZERO,
        }]);
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

    /// Sends one frame's worth of touches, each aimed relative to the hero.
    ///
    /// One call rather than one per finger, because that is how a platform
    /// reports a frame: every live touch arrives together. It also matters for
    /// what is being tested, since ADR-0006 makes two concurrent Tongues require
    /// two **overlapping** fingers, and fingers that landed in separate calls
    /// would Release each other's slot instead.
    fn touch_frame(world: &mut World, touches: &[(u64, TouchPhase, Vec2)]) {
        let from = hero(world).pos;
        let samples: Vec<TouchSample> = touches
            .iter()
            .map(|(id, phase, aim)| TouchSample {
                id: *id,
                phase: *phase,
                pos: from + *aim,
            })
            .collect();
        world.apply_touches(&samples);
    }

    /// Where the Tongue in `slot` is Attached, or `None` if that slot is empty.
    ///
    /// Looked up by slot rather than by position in the list, because the order
    /// the `Snapshot` lists Tongues in is not part of the contract.
    fn slot_anchor(world: &World, slot: u8) -> Option<Vec2> {
        world
            .snapshot()
            .tongues
            .iter()
            .find(|line| line.slot == slot)
            .map(|line| line.to)
    }

    /// The hero at the origin with a surface exactly 10 units away in each of
    /// the four axis directions, and nothing else in reach.
    ///
    /// Built for the ticket #14 force figures, which were measured with the
    /// Anchor 10 units out: the Stretch is then `10 - REST_LENGTH = 9` and one
    /// Tongue pulls `DEFAULT_PULL_STRENGTH * 9 = 360`. The hero sits at the
    /// origin so that one tick's displacement is exact rather than a difference
    /// of two large coordinates.
    fn cross() -> Level {
        let face = |centre: Vec2| Aabb::new(centre, Vec2::splat(0.5));
        let level = Level {
            world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(500.0)),
            static_terrain: vec![
                face(Vec2::new(0.0, 10.5)),
                face(Vec2::new(0.0, -10.5)),
                face(Vec2::new(10.5, 0.0)),
                face(Vec2::new(-10.5, 0.0)),
            ],
            player_spawn: Vec2::ZERO,
            enemy_spawns: vec![],
        };
        level.validate().expect("fixture must be a valid level");
        level
    }

    /// Two walls 2000 units apart with the hero midway between them.
    ///
    /// The configuration ADR-0003 calls finite and an earlier version of
    /// ADR-0006 called infeasible. Attaching to both at once is a genuine test
    /// of the claim that forces sum rather than fight.
    fn wide_canyon() -> Level {
        let level = Level {
            world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(3_000.0)),
            static_terrain: vec![
                // Inner faces at x = 1000 and x = -1000.
                Aabb::new(Vec2::new(1_500.0, 0.0), Vec2::new(500.0, 1_000.0)),
                Aabb::new(Vec2::new(-1_500.0, 0.0), Vec2::new(500.0, 1_000.0)),
            ],
            player_spawn: Vec2::ZERO,
            enemy_spawns: vec![],
        };
        level.validate().expect("fixture must be a valid level");
        level
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
            line.anchor,
            Anchor::Fixed(anchor),
            "terrain is a Fixed Anchor, carried by position because it has no body"
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
    fn a_second_tap_releases_the_attached_tongue() {
        // ADR-0006, and the traversal loop the ticket names: tap, Swing, tap,
        // fly. The second tap lands on the slot the first one filled, and a tap
        // on an occupied slot is a Release.
        let mut world = World::from_level(&room());
        attach(&mut world, Vec2::new(0.0, 5.0));
        lift(&mut world, 1);

        tap_toward(&mut world, 2, Vec2::new(0.0, 5.0));
        world.step();
        assert!(
            world.snapshot().tongues.is_empty(),
            "the second tap must Release, leaving nothing to draw: {:?}",
            world.snapshot().tongues
        );
    }

    #[test]
    fn releasing_leaves_the_hero_on_the_tangent_in_ballistic_motion() {
        // The headline of the ticket: Release converts Swing momentum into
        // unconstrained ballistic motion. Two claims, and they are separable.
        //
        // The exit velocity is the tangent, which is not something the Release
        // code arranges: it is whatever the Swing had, and the Release's job is
        // to not touch it. So the Tongue is Released at a moment when the
        // radial velocity has just crossed zero, which is exactly when the
        // hero's whole velocity lies along the tangent, and the test asserts
        // the fixture really is at a tangent before asserting anything else.
        //
        // The arc afterwards is then compared against the closed form
        //
        //     p(t) = p0 + v0*t + 0.5*g*t^2
        //
        // which comes from the physics and not from the code. Velocity Verlet
        // is exact under a constant acceleration, so any residual spring force,
        // any retraction impulse, or any velocity the Release scaled would show
        // up here as a divergence from a prediction that knows nothing about
        // the Tongue.
        // A Swing, set up identically every time. Two passes over the same
        // scene, which is sound precisely because the simulation is
        // deterministic: the first finds which tick is the tangent and the
        // second stops there. Reading a tick's velocity and then going back is
        // not something the seam offers, and nothing here needs it to.
        let swing = || {
            let mut world = World::from_level(&open_sky(8.0));
            let hero_id = world.hero_id();
            let anchor = attach(&mut world, Vec2::Y);
            lift(&mut world, 1);
            // Across the pull, so the motion is orbital and there is a tangent
            // to leave on at all.
            world.place(hero_id, hero(&world).pos, Vec2::new(2.5, 0.0));
            (world, anchor)
        };

        let radial = |w: &World, anchor: Vec2| {
            let pos = hero(w).pos;
            let vel = w
                .body_velocity(w.hero_id())
                .expect("the hero is alive")
                .normalize_or_zero();
            vel.dot((anchor - pos).normalize())
        };

        // The tangent is where the velocity has no component along the Tongue
        // at all, so the flattest tick in the first orbit is the moment to
        // leave on. Searched rather than computed, because where it falls is a
        // property of the emergent Swing and not of a formula.
        let (mut world, anchor) = swing();
        let mut best = (f32::INFINITY, 0u32);
        for tick in 1..=240u32 {
            world.step();
            let flatness = radial(&world, anchor).abs();
            if flatness < best.0 {
                best = (flatness, tick);
            }
        }
        assert!(
            best.0 < 0.01,
            "the fixture never came within 1 percent of a tangent: {best:?}"
        );

        let (mut world, anchor) = swing();
        let hero_id = world.hero_id();
        for _ in 0..best.1 {
            world.step();
        }

        let p0 = hero(&world).pos;
        let v0 = world.body_velocity(hero_id).expect("the hero is alive");
        let radial_dir = (anchor - p0).normalize();
        assert!(
            v0.normalize().dot(radial_dir).abs() < 0.01,
            "the fixture is not at a tangent: velocity {v0:?} against radial {radial_dir:?}"
        );
        assert!(v0.length() > 1.0, "a tangent at rest would prove nothing");

        // The second tap. Its aim is irrelevant to a Release, which is part of
        // why a Release cannot miss.
        tap_toward(&mut world, 2, Vec2::Y);

        const TICKS: u32 = 60;
        for _ in 0..TICKS {
            world.step();
        }
        assert!(
            world.snapshot().tongues.is_empty(),
            "a Released Tongue must leave the Snapshot and stop being drawn"
        );

        let t = TICKS as f32 * DT;
        let expected = p0 + v0 * t + 0.5 * GRAVITY * t * t;
        let measured = hero(&world).pos;
        assert!(
            (measured - expected).length() < 1e-3,
            "the arc was not ballistic: predicted {expected:?} from v0 {v0:?}, measured {measured:?}"
        );

        // And the momentum itself survived, rather than the position merely
        // starting out right. Gravity is the only thing that may have changed
        // it, which pins down both the speed and the direction.
        let v_now = world.body_velocity(hero_id).expect("the hero is alive");
        let expected_v = v0 + GRAVITY * t;
        assert!(
            (v_now - expected_v).length() < 1e-3,
            "momentum was not preserved: predicted {expected_v:?}, measured {v_now:?}"
        );
    }

    #[test]
    fn releasing_cannot_miss_and_costs_nothing() {
        // ADR-0012: "Releasing retracts the Tongue instantly with no delay",
        // so there is no whiff case. The specific way that could go wrong here
        // is sharing the fire path, which needs a ray hit: a Release aimed into
        // open sky would then quietly do nothing and the player would be stuck
        // Attached, holding a thumb at the sky.
        let mut world = World::from_level(&open_sky(8.0));
        attach(&mut world, Vec2::Y);
        lift(&mut world, 1);
        assert_eq!(world.snapshot().tongues.len(), 1);

        // Straight down, where this fixture has no terrain whatsoever, so the
        // ray a fire would cast meets nothing.
        tap_toward(&mut world, 2, Vec2::NEG_Y);
        world.step();
        assert!(
            world.snapshot().tongues.is_empty(),
            "a Release aimed at nothing must still Release"
        );

        // Instant, meaning the very next tick and not some ticks later: there
        // is no retraction to wait out. The tick above is the first one after
        // the tap, and the Tongue is already gone.
        //
        // And no cost: the slot is immediately usable again, so Releasing does
        // not spend a Tongue.
        lift(&mut world, 2);
        let anchor = attach(&mut world, Vec2::Y);
        assert_eq!(
            anchor,
            Vec2::new(0.0, 9.5),
            "the Released slot must be free to fire again at once"
        );
    }

    #[test]
    fn a_one_frame_ended_or_cancelled_is_not_missed() {
        // The platform fact from the #2 research: `Ended` and `Cancelled` are
        // visible for exactly one frame. There is no later chance to see them,
        // so a `World` that looked at them after claiming, or only on a frame
        // it also stepped, would silently never free the slot's ownership.
        //
        // That failure is invisible in every #12 test, because a finger that
        // kept its slot forever still left its Tongue Attached and still drew
        // correctly. It becomes visible only now: ownership is what decides
        // which slot the next tap lands on, so a missed `Ended` sends the
        // Release to the wrong slot and fires a second Tongue instead.
        for phase in [TouchPhase::Ended, TouchPhase::Cancelled] {
            let mut world = World::from_level(&room());
            attach(&mut world, Vec2::new(0.0, 5.0));

            // Shown for exactly one frame, then never again, and on a frame
            // with no Sim Tick of its own.
            world.apply_touches(&[TouchSample {
                id: 1,
                phase,
                pos: Vec2::ZERO,
            }]);

            tap_toward(&mut world, 2, Vec2::new(0.0, 5.0));
            world.step();
            assert!(
                world.snapshot().tongues.is_empty(),
                "a one-frame {phase:?} was missed, so the next tap fired instead of \
                 Releasing: {:?}",
                world.snapshot().tongues
            );
        }
    }

    #[test]
    fn a_longer_stretch_at_attach_returns_far_more_speed() {
        // The skill expression of the whole control scheme, and the ticket #8
        // prototype's headline measurement:
        //
        //     attach distance | Stretch | return speed | gain
        //                   1 |    0.00 |         6.96 | 1.00x
        //                   4 |    3.00 |        18.81 | 2.70x
        //                  16 |   15.00 |        88.48 | 12.71x
        //
        // Reproduced as an ordering plus an analytic check rather than by
        // matching those three numbers. The two spring-dominated rows are
        // compared against the closed form instead, which is an independent
        // source of truth: all the Stretch's stored energy becomes kinetic, so
        //
        //     0.5*k*s^2 = 0.5*m*v^2   ->   v = s*sqrt(k/m)
        //
        // giving 18.97 and 94.87 against the prototype's 18.81 and 88.48. A
        // damped spring can only ever return less than it stored, never more,
        // which is the bound asserted below.
        //
        // The `1` row is deliberately not held to a number. Its Stretch is
        // zero, so nothing is stored and the prototype's 6.96 is entirely an
        // artifact of how far its harness let the hero fall first. What it is
        // held to is its place in the ordering, which is the claim that
        // matters: loading a Stretch is what returns speed.
        let k = DEFAULT_PULL_STRENGTH;

        /// A tall wall whose right face is at `x = 0`, and nothing else. The
        /// hero Attaches horizontally, so the pull is perpendicular to gravity
        /// and the return is the spring's alone.
        fn wall_at_origin(distance: f32) -> Level {
            let level = Level {
                world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(4_000.0)),
                static_terrain: vec![Aabb::new(Vec2::new(-500.0, 0.0), Vec2::new(500.0, 1_000.0))],
                player_spawn: Vec2::new(distance, 0.0),
                enemy_spawns: vec![],
            };
            level.validate().expect("fixture must be a valid level");
            level
        }

        // The fastest the hero travels after Attaching at `distance`, which is
        // the speed the slingshot returned.
        let returned = |distance: f32| {
            let mut world = World::from_level(&wall_at_origin(distance));
            let hero_id = world.hero_id();
            let anchor = attach(&mut world, Vec2::NEG_X);
            assert_eq!(
                anchor,
                Vec2::new(0.0, 0.0),
                "the Tongue must Attach to the wall at the hero's own height"
            );
            let mut peak = 0.0f32;
            for _ in 0..120 {
                world.step();
                let speed = world
                    .body_velocity(hero_id)
                    .expect("the hero is alive")
                    .length();
                peak = peak.max(speed);
            }
            peak
        };

        let (near, mid, far) = (returned(1.0), returned(4.0), returned(16.0));
        assert!(
            near < mid && mid < far,
            "the prototype's ordering was not reproduced: {near}, {mid}, {far}"
        );

        // Damping is real, so the undamped figure is a ceiling and not the
        // prediction. The prediction is a quarter cycle of a damped oscillator
        // released from rest at amplitude `stretch`:
        //
        //     zeta  = damping / (2*sqrt(k*m))          = 0.237
        //     decay = exp(-zeta * (pi/2) / sqrt(1 - zeta^2))
        //
        // which gives 12.93 at 4 units and 64.65 at 16. Measured: 13.85 and
        // 69.19, both 1.07x the prediction, because the true peak arrives a
        // little before the boundary crossing this closed form is written at.
        // The bound is therefore 10 percent, and the agreement between the two
        // distances is the real content: a linear spring must return a speed
        // proportional to its Stretch, and this one does to within a tenth of
        // a percent of 5.00x across a five-fold Stretch.
        let zeta = DEFAULT_DAMPING / (2.0 * (k * DEFAULT_PLAYER_MASS).sqrt());
        let decay = (-zeta * core::f32::consts::FRAC_PI_2 / (1.0 - zeta * zeta).sqrt()).exp();
        for (distance, measured) in [(4.0f32, mid), (16.0, far)] {
            let stretch = distance - REST_LENGTH;
            let stored = stretch * (k / DEFAULT_PLAYER_MASS).sqrt();
            assert!(
                measured <= stored * 1.001,
                "a damped spring returned more than it stored at {distance}: \
                 {measured} against a stored {stored}"
            );
            let predicted = stored * decay;
            assert!(
                (measured - predicted).abs() < predicted * 0.10,
                "at {distance} units expected about {predicted}, measured {measured}"
            );
        }

        // The gain the prototype calls the real pass or fail: the long Stretch
        // is worth multiples of the short one and not a few percent. Five times
        // the Stretch returns five times the speed, measured at 5.00x here
        // against the prototype's 4.70x from the same two rows.
        let gain = far / mid;
        assert!(
            (4.5..=5.5).contains(&gain),
            "expected roughly 5x from 4 units to 16, got {gain}x"
        );
        // And the headline against the short Attach, which is what the
        // prototype's 12.71x is about: a deliberately loaded Stretch returns an
        // order of magnitude more speed than Attaching at arm's length.
        assert!(
            far > near * 10.0,
            "a loaded Stretch must be worth many times a short Attach: \
             {far} against {near}"
        );
    }

    #[test]
    fn re_targeting_a_slot_to_a_different_terrain_anchor_snaps_rather_than_sweeping() {
        // ADR-0009: "a re-fired one snaps, otherwise it sweeps across the
        // level". Until this ticket that case was unreachable, because a slot
        // held one Tongue for the whole run, and the frame contract therefore
        // got away with identifying a Fixed Anchor as `None`: two different
        // terrain points compared equal and the blend predicate could not tell
        // them apart. Release makes it reachable, which is why the Anchor had
        // to become part of the contract rather than only the body's id.
        //
        // The specific timing matters and is the reason an empty snapshot does
        // not save us. The Shell draws more frames than there are Sim Ticks, so
        // it calls `apply_touches` more than once between one tick and the
        // next: the frame that Releases and the frame that re-fires can both
        // fall inside a single tick, and then no `Snapshot` ever shows the slot
        // empty. The slot goes straight from one Anchor to another.
        let mut world = World::from_level(&room());
        let ceiling = attach(&mut world, Vec2::new(0.0, 5.0));
        lift(&mut world, 1);

        // Both of these land between the same pair of Sim Ticks.
        tap_toward(&mut world, 2, Vec2::new(0.0, 5.0));
        lift(&mut world, 2);
        tap_toward(&mut world, 3, Vec2::NEG_Y);
        world.step();

        let floor = world.snapshot().tongues[0].to;
        assert_eq!(
            world.snapshot().tongues.len(),
            1,
            "the slot must hold exactly the re-fired Tongue"
        );
        assert_ne!(
            floor, ceiling,
            "the fixture must re-target to a different surface, or this proves nothing"
        );
        assert_eq!(
            world.previous_snapshot().tongues.len(),
            1,
            "the previous tick must still show the old Anchor in that slot, \
             or the empty snapshot would be doing the snapping"
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
                view.tongues[0].to, floor,
                "at alpha {alpha} the Tongue swept from {ceiling:?} toward {floor:?}"
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

    #[test]
    fn the_two_tongues_sum_as_vectors_at_the_prototypes_exact_ratios() {
        // Ticket #14's headline, through the seam and in force units:
        //
        //     one Tongue                  |F| = 360.000   1.000x
        //     two on the same Anchor      |F| = 720.000   2.000x
        //     two exactly opposed         |F| =   0.000   cancels
        //     two perpendicular           |F| = 509.117   1.414x
        //
        // Exact rather than approximate, because every row was measured at zero
        // velocity: ADR-0003's damping term is `damping * radial_vel`, so at
        // rest it is identically zero and nothing is left but Hooke's law summed
        // as vectors. The `cross` fixture puts every Anchor exactly 10 units
        // out, so the Stretch is `10 - REST_LENGTH = 9` and `DEFAULT_PULL_STRENGTH
        // * 9 = 360` is arithmetic rather than a recorded measurement.
        //
        // The same prototype's slingshot *return speeds* are deliberately not
        // asserted anywhere: ticket #13 found they are unreproducible at the
        // shipped `DEFAULT_DAMPING`, and ADR-0003 records why. Forces at rest do
        // not depend on that constant at all, which is the difference.
        // Three decimals, which is the precision the prototype quotes. The
        // recovery is tighter than that: measured 360.0 exactly, 719.99994,
        // 1.2e-7 for the cancelling pair, and 509.11688, so the bound is the
        // figures' precision rather than the method's.
        const TOLERANCE: f32 = 1e-3;
        assert_eq!(
            DEFAULT_PULL_STRENGTH * (10.0 - REST_LENGTH),
            360.0,
            "the prototype's figures assume k 40, a 10 unit Anchor and a 9 unit Stretch"
        );

        // The force the Tongues applied on the first tick after Attaching,
        // recovered from how far the hero moved. Velocity Verlet's position
        // update is `pos + vel*DT + 0.5*a0*DT^2` and the hero starts at rest, so
        // one tick's displacement is exactly `0.5*a0*DT^2`, and `a0` is the
        // acceleration at the moment of the Attach. Gravity is the only other
        // force in this fixture, so subtracting it leaves the resultant pull.
        // Inverting the integrator rather than reading a private field is what
        // keeps this a `Snapshot` assertion.
        let resultant = |aims: &[Vec2]| -> Vec2 {
            let mut world = World::from_level(&cross());
            let from = hero(&world).pos;
            let touches: Vec<(u64, TouchPhase, Vec2)> = aims
                .iter()
                .enumerate()
                .map(|(i, aim)| (i as u64 + 1, TouchPhase::Started, *aim))
                .collect();
            // One frame, every finger down together: ADR-0006 makes two Tongues
            // require two overlapping fingers, so a tap and a lift and another
            // tap would Release rather than hold a second.
            touch_frame(&mut world, &touches);
            world.step();
            assert_eq!(
                world.snapshot().tongues.len(),
                aims.len(),
                "the fixture must Attach one Tongue per aim, or the ratio below means nothing"
            );
            let moved = hero(&world).pos - from;
            DEFAULT_PLAYER_MASS * (2.0 * moved / (DT * DT) - GRAVITY)
        };

        let one = resultant(&[Vec2::Y]);
        assert!(
            (one.length() - 360.0).abs() < TOLERANCE,
            "one Tongue must pull 360.000, measured {}",
            one.length()
        );

        let aligned = resultant(&[Vec2::Y, Vec2::Y]);
        assert!(
            (aligned.length() - 720.0).abs() < TOLERANCE,
            "two Tongues on the same Anchor must pull 720.000 (2.000x), measured {}",
            aligned.length()
        );

        let opposed = resultant(&[Vec2::Y, Vec2::NEG_Y]);
        assert!(
            opposed.length() < TOLERANCE,
            "two exactly opposed Tongues must cancel to 0.000, measured {opposed:?}"
        );

        let perpendicular = resultant(&[Vec2::Y, Vec2::X]);
        let expected = 360.0 * core::f32::consts::SQRT_2;
        assert!(
            (perpendicular.length() - expected).abs() < TOLERANCE,
            "two perpendicular Tongues must pull {expected:.3} (509.117, 1.414x), measured {}",
            perpendicular.length()
        );
        // The direction too, which the magnitude alone does not pin down: a sum
        // that dropped one Tongue and scaled the other would still read 1.414x.
        assert!(
            (perpendicular.normalize() - Vec2::new(1.0, 1.0).normalize()).length() < 1e-3,
            "the resultant must bisect the two Anchors, measured {perpendicular:?}"
        );
    }

    #[test]
    fn both_tongues_may_share_one_anchor_and_both_are_drawn() {
        // Spec story 14, and the drawing half of the AC. Two Tongues on one
        // Anchor are two separate [`TongueLine`]s in the `Snapshot`, not one
        // line counted twice: the Shell draws whatever it is given, so a slot
        // collapsed here is a Tongue the player never sees.
        let mut world = World::from_level(&room());
        touch_frame(
            &mut world,
            &[
                (1, TouchPhase::Started, Vec2::Y),
                (2, TouchPhase::Started, Vec2::Y),
            ],
        );
        world.step();

        let lines = world.snapshot().tongues.clone();
        assert_eq!(lines.len(), MAX_TONGUES, "both slots must hold a Tongue");
        let mut slots: Vec<u8> = lines.iter().map(|l| l.slot).collect();
        slots.sort_unstable();
        assert_eq!(slots, vec![0, 1], "each Tongue must occupy its own slot");
        assert_eq!(
            lines[0].to, lines[1].to,
            "both Tongues were aimed at the same surface"
        );
        assert_eq!(
            lines[0].anchor, lines[1].anchor,
            "sharing an Anchor means sharing the Anchor, not two coincident ones"
        );
        let from = hero(&world).pos;
        assert!(
            lines.iter().all(|l| l.from == from),
            "both Tongues are drawn from the hero: {lines:?}"
        );

        // And both survive the frame contract, which is what "drawn" means for
        // the Shell: it renders an interpolated view and never the raw snapshot.
        let mut view = Snapshot::default();
        crate::frame::interpolate_into(world.previous_snapshot(), world.snapshot(), 0.5, &mut view);
        assert_eq!(
            view.tongues.len(),
            MAX_TONGUES,
            "a Tongue was lost in interpolation: {:?}",
            view.tongues
        );
    }

    #[test]
    fn each_slot_fires_and_releases_without_disturbing_the_other() {
        // The AC's first line, and the whole of ADR-0006's slot rule exercised
        // in one run. A Gesture claims the lowest slot no other finger is
        // driving, and taking an empty slot fires while taking an occupied one
        // Releases, so "independently" is reached by keeping the first finger
        // **down**: it holds slot 0, which sends every later tap to slot 1.
        //
        // The failure this prevents is a Release that reaches for the wrong
        // slot. Both slots hold a Tongue and both tests above would still pass
        // if a Release cleared slot 0 whenever it was asked for slot 1.
        let mut world = World::from_level(&room());

        touch_frame(&mut world, &[(1, TouchPhase::Started, Vec2::Y)]);
        world.step();
        let ceiling = world.snapshot().tongues[0].to;

        // Finger 1 is still down, so finger 2 lands in slot 1 and fires.
        touch_frame(
            &mut world,
            &[
                (1, TouchPhase::Stationary, Vec2::Y),
                (2, TouchPhase::Started, Vec2::X),
            ],
        );
        world.step();
        let right_wall = slot_anchor(&world, 1).expect("slot 1 must have fired");
        assert_eq!(world.snapshot().tongues.len(), MAX_TONGUES);
        assert_ne!(
            ceiling, right_wall,
            "the fixture must Attach the two slots to different surfaces"
        );

        // Finger 2 lifts, which ends its Gesture and leaves its Tongue
        // Attached, then finger 3 lands while finger 1 still holds slot 0. It
        // therefore takes slot 1, finds a Tongue there, and Releases it. Its aim
        // is downward, at a surface neither slot is Attached to, so a Release
        // that re-fired instead would be unmistakable.
        touch_frame(
            &mut world,
            &[
                (1, TouchPhase::Stationary, Vec2::Y),
                (2, TouchPhase::Ended, Vec2::X),
            ],
        );
        touch_frame(
            &mut world,
            &[
                (1, TouchPhase::Stationary, Vec2::Y),
                (3, TouchPhase::Started, Vec2::NEG_Y),
            ],
        );
        world.step();
        assert_eq!(
            slot_anchor(&world, 0),
            Some(ceiling),
            "Releasing slot 1 disturbed slot 0: {:?}",
            world.snapshot().tongues
        );
        assert_eq!(
            slot_anchor(&world, 1),
            None,
            "slot 1 must be Released: {:?}",
            world.snapshot().tongues
        );

        // And slot 1 is immediately usable again while slot 0 keeps Swinging,
        // which is the brace-between-two-Anchors loop from spec story 12.
        touch_frame(
            &mut world,
            &[
                (1, TouchPhase::Stationary, Vec2::Y),
                (3, TouchPhase::Ended, Vec2::NEG_Y),
            ],
        );
        touch_frame(
            &mut world,
            &[
                (1, TouchPhase::Stationary, Vec2::Y),
                (4, TouchPhase::Started, Vec2::NEG_X),
            ],
        );
        world.step();
        let left_wall = slot_anchor(&world, 1).expect("slot 1 must have re-fired");
        assert_eq!(
            slot_anchor(&world, 0),
            Some(ceiling),
            "re-firing slot 1 disturbed slot 0"
        );
        assert_ne!(left_wall, right_wall, "slot 1 re-aimed at the other wall");

        // Now the other way round: finger 1 lifts, a new finger takes the slot
        // it gave up, and Releasing slot 0 leaves slot 1 Attached.
        touch_frame(
            &mut world,
            &[
                (1, TouchPhase::Ended, Vec2::Y),
                (4, TouchPhase::Stationary, Vec2::NEG_X),
            ],
        );
        touch_frame(
            &mut world,
            &[
                (5, TouchPhase::Started, Vec2::NEG_Y),
                (4, TouchPhase::Stationary, Vec2::NEG_X),
            ],
        );
        world.step();
        assert_eq!(
            slot_anchor(&world, 0),
            None,
            "slot 0 must be Released: {:?}",
            world.snapshot().tongues
        );
        assert_eq!(
            slot_anchor(&world, 1),
            Some(left_wall),
            "Releasing slot 0 disturbed slot 1: {:?}",
            world.snapshot().tongues
        );
    }

    #[test]
    fn a_third_finger_leaves_both_attached_tongues_exactly_as_they_were() {
        // Spec story 15: a stray touch must never cost a player a run. The
        // existing third-finger test covers two fingers landing *together*;
        // this covers the dangerous version, a third finger arriving while two
        // Tongues are already Attached and Swinging. Discarded has to mean
        // discarded: not a Release, not a re-target of either slot.
        let mut world = World::from_level(&room());
        touch_frame(
            &mut world,
            &[
                (1, TouchPhase::Started, Vec2::Y),
                (2, TouchPhase::Started, Vec2::X),
            ],
        );
        world.step();
        let before = world.snapshot().tongues.clone();
        assert_eq!(before.len(), MAX_TONGUES, "both slots must be Attached");

        for tick in 0..30 {
            // The stray finger lands on a fresh id every frame, which is the
            // hostile version: an id the tracker has never seen cannot be
            // refused for already owning a slot.
            touch_frame(
                &mut world,
                &[
                    (1, TouchPhase::Stationary, Vec2::Y),
                    (2, TouchPhase::Stationary, Vec2::X),
                    (100 + tick, TouchPhase::Started, Vec2::NEG_Y),
                ],
            );
            world.step();
            let now = world.snapshot().tongues.clone();
            assert_eq!(
                now.len(),
                MAX_TONGUES,
                "a third finger changed the Tongue count at tick {tick}: {now:?}"
            );
            assert_eq!(
                now.iter().map(|l| l.anchor).collect::<Vec<_>>(),
                before.iter().map(|l| l.anchor).collect::<Vec<_>>(),
                "a third finger re-targeted a slot at tick {tick}"
            );
        }
    }

    #[test]
    fn anchors_two_thousand_units_apart_produce_a_finite_resultant() {
        // ADR-0003 withdrew ADR-0006's over-constraint warning, and this is the
        // assertion that the withdrawal was real. Under a distance constraint,
        // two Anchors 2000 units apart with a hero between them is infeasible:
        // no position satisfies both, and a solver either jitters or diverges.
        // Forces merely sum, so there is nothing to satisfy and nothing to
        // iterate, and the only possible outcome is a finite resultant.
        //
        // The regression this prevents is someone reintroducing a solver, a
        // length limit, or a positional correction. Any of those would show up
        // here as a non-finite position or a hero flung out of the level, and
        // nowhere else in the suite: every other Tongue test Attaches within a
        // few units.
        let mut world = World::from_level(&wide_canyon());
        touch_frame(
            &mut world,
            &[
                (1, TouchPhase::Started, Vec2::X),
                (2, TouchPhase::Started, Vec2::NEG_X),
            ],
        );
        world.step();

        let lines = world.snapshot().tongues.clone();
        assert_eq!(lines.len(), MAX_TONGUES, "both walls must be Attached");
        let span = (lines[0].to - lines[1].to).length();
        assert!(
            (span - 2_000.0).abs() < 1e-3,
            "the fixture must span 2000 units, measured {span}"
        );
        // And the pulls really are enormous, or "finite" would be a claim about
        // a mild scene. A 999 unit Stretch asks for 39,960 units of force and
        // gets [`DEFAULT_FORCE_CEILING`], which is the gameplay cap ADR-0003
        // describes. The cap is per Tongue, so two of them can ask for twice it:
        // that is what forces summing means, and the alternative would be a
        // Tongue that weakened because another one existed.
        let single = tongue::pull(
            Vec2::ZERO,
            Vec2::ZERO,
            lines[0].to,
            world.hero_pull_strength(),
        );
        assert!(
            (single.length() - DEFAULT_FORCE_CEILING).abs() < 1e-3,
            "the fixture must load each Tongue to the force ceiling, got {}",
            single.length()
        );

        for tick in 0..3_000 {
            world.step();
            let pos = hero(&world).pos;
            assert!(
                pos.is_finite(),
                "diverged at tick {tick}: {pos:?}, which is what a constraint solver would do"
            );
            // Both pulls are at the force ceiling and very nearly opposed, so
            // the hero can only drift a little around the midpoint. A generous
            // bound: the claim is that it is bounded at all, not where.
            assert!(
                pos.length() < 100.0,
                "the hero was flung to {pos:?} at tick {tick}"
            );
            assert_eq!(
                world.snapshot().tongues.len(),
                MAX_TONGUES,
                "a Tongue was dropped at tick {tick}, so the pulls stopped opposing"
            );
        }
    }

    /// An enclosed corridor with a row of Enemies at the hero's height.
    ///
    /// Written flush to the left margin for the reason `room` is: `from_ascii`
    /// counts leading spaces as columns, so an indented sketch is the same
    /// scene shifted right and none of the coordinates here would hold.
    ///
    /// Walls at `x = 1` and `x = 17`, floor top at `y = 1`, hero at
    /// `(1.5, 2.5)`, Enemies at `(3.5, 2.5)`, `(5.5, 2.5)` and `(7.5, 2.5)`.
    fn corridor() -> Level {
        Level::from_ascii(
            "\
##################
#................#
#@.e.e.e.........#
#................#
##################
",
        )
    }

    /// A short enclosed alley with one Enemy between the hero and the left
    /// wall, so a hero sent left Slams it and then runs out of room.
    ///
    /// Flush left: the coordinates here are absolute. Left wall face at
    /// `x = 1`, floor top at `y = 1`, Enemy at `(2.5, 1.5)`, hero at
    /// `(3.5, 1.5)`, which is exactly its resting height on the floor.
    fn alley() -> Level {
        Level::from_ascii(
            "\
##########
#........#
#.e@.....#
##########
",
        )
    }

    /// A long hall with a floor to land on and a ceiling to Attach to, plus one
    /// Enemy in the hero's path.
    ///
    /// Built by hand rather than sketched because it is 120 units long. Floor
    /// top at `y = 0`, ceiling bottom at `y = 20`, hero spawning at
    /// `(2.0, 10.0)`, Enemy at `(10.0, 10.0)`. The Kill Bound is far outside
    /// all of it, so nothing despawns in the middle of a measurement.
    fn hall() -> Level {
        let level = Level {
            world_bounds: Aabb::new(Vec2::new(60.0, 10.0), Vec2::splat(400.0)),
            static_terrain: vec![
                Aabb::new(Vec2::new(60.0, -0.5), Vec2::new(60.0, 0.5)),
                Aabb::new(Vec2::new(60.0, 20.5), Vec2::new(60.0, 0.5)),
            ],
            player_spawn: Vec2::new(2.0, 10.0),
            enemy_spawns: vec![EnemySpawn {
                pos: Vec2::new(10.0, 10.0),
                toughness: 100.0,
                mass: 1.0,
            }],
        };
        level.validate().expect("fixture must be a valid level");
        level
    }

    /// Launches the hero into the hall's Enemy hard enough to destroy it and
    /// returns the Sim Tick the hit landed on, failing if it never did.
    ///
    /// A Combo cannot be tested without a hit to start it, and a test that
    /// assumed which tick the hit fell on would be asserting the fixture's
    /// arithmetic rather than the rule under test.
    fn slam_the_hall(world: &mut World) -> u64 {
        world.place(world.hero_id(), Vec2::new(2.0, 10.0), Vec2::new(120.0, 0.0));
        for _ in 0..20 {
            world.step();
            if world.snapshot().combo == 1 {
                return world.snapshot().tick;
            }
        }
        panic!("the fixture never landed a Slam, so nothing below proves anything");
    }

    #[test]
    fn each_slam_increments_the_combo_count() {
        // GLOSSARY: a Combo is an unbroken run of Slams, and it is a count of
        // hits. ADR-0011 makes it uncapped and keeps it in the Core Sim.
        let mut world = World::from_level(&corridor());
        assert_eq!(
            world.snapshot().combo,
            0,
            "nothing has been hit, so there is no Combo yet"
        );

        // Momentum assigned rather than earned by Swinging, as the combat
        // tests do: this is about what an impact does to the count.
        world.place(world.hero_id(), Vec2::new(1.5, 2.5), Vec2::new(120.0, 0.0));

        // Two units of travel at two units per tick reaches the first Enemy,
        // so the first hit lands on the first tick.
        world.step();
        assert_eq!(
            world.snapshot().combo,
            1,
            "the first Slam must put the count at one"
        );

        // Three Enemies in the row, each hit once, so the run ends at three.
        // Each impact drains the hero, which is what makes the later hits
        // weaker rather than faster, but every one of them is above the Kick
        // threshold and so is a Slam.
        let mut previous = 1;
        for _ in 0..20 {
            world.step();
            let now = world.snapshot().combo;
            assert!(
                now >= previous,
                "an unbroken run must never count down: {previous} then {now}"
            );
            previous = now;
        }
        assert_eq!(previous, 3, "three Enemies hit once each is three hits");
    }

    #[test]
    fn a_kick_does_not_count_as_a_hit() {
        // The Combo counts Slams, and a Kick is defined as the band below the
        // Slam threshold. Without this the count would be of impacts, and
        // drifting gently into an Enemy would read as combat.
        let mut world = World::from_level(&corridor());
        // One unit per second at the baseline hero's mass is one momentum unit,
        // half the Kick threshold, and the Enemy is a unit away.
        world.place(world.hero_id(), Vec2::new(2.4, 2.5), Vec2::new(1.0, 0.0));

        let enemy_before = *world
            .snapshot()
            .bodies
            .iter()
            .find(|b| b.kind == BodyKind::Enemy)
            .expect("the corridor has Enemies");

        for _ in 0..30 {
            world.step();
            assert_eq!(
                world.snapshot().combo,
                0,
                "a Kick is not a Slam, so it must not start a Combo"
            );
        }

        // And the drift really did reach the Enemy, or the loop above proved
        // only that nothing happened.
        let enemy_now = *world
            .snapshot()
            .body(enemy_before.id)
            .expect("a Kick must not destroy the Enemy");
        assert!(
            enemy_now.pos.x > enemy_before.pos.x,
            "the Enemy was never Kicked: {enemy_before:?} then {enemy_now:?}"
        );
    }

    #[test]
    fn mere_ground_contact_does_not_break_the_combo() {
        // ADR-0011 breaks the Combo on coming to rest and explicitly not on
        // ground contact. A hero skidding along the floor at speed is in
        // contact on every single tick, and the run has to survive all of it.
        let mut world = World::from_level(&alley());
        let hero_id = world.hero_id();
        world.place(hero_id, Vec2::new(3.5, 1.5), Vec2::new(-10.0, 0.0));
        world.step();
        assert_eq!(world.snapshot().combo, 1, "the fixture must land a Slam");

        // The hero slides left along the floor at half the speed it arrived
        // with, the Slam having drained the rest.
        for tick in 0..20 {
            world.step();
            let pos = hero(&world).pos;
            let speed = world
                .body_velocity(hero_id)
                .expect("the hero is alive")
                .length();
            assert!(
                (pos.y - (1.0 + PLAYER_RADIUS)).abs() < 1e-3,
                "the fixture must keep the hero on the floor: tick {tick}, {pos:?}"
            );
            assert!(
                speed > crate::combo::REST_SPEED,
                "the fixture must keep the hero moving: tick {tick}, speed {speed}"
            );
            assert_eq!(
                world.snapshot().combo,
                1,
                "ground contact at speed must not break the Combo: tick {tick}"
            );
        }
    }

    #[test]
    fn the_combo_breaks_when_the_hero_comes_to_rest_on_a_surface() {
        // The other half of the same rule, and the predicate this ticket had to
        // choose: sustained slow contact with terrain. Recorded in ADR-0011.
        let mut world = World::from_level(&alley());
        let hero_id = world.hero_id();
        world.place(hero_id, Vec2::new(3.5, 1.5), Vec2::new(-10.0, 0.0));
        world.step();
        assert_eq!(world.snapshot().combo, 1, "the fixture must land a Slam");
        let slam_tick = world.snapshot().tick;

        // Left along the floor until the wall takes the last of the momentum.
        let mut broke_on = None;
        for _ in 0..200 {
            world.step();
            if world.snapshot().combo == 0 {
                broke_on = Some(world.snapshot().tick);
                break;
            }
        }
        let broke_on = broke_on.expect("the Combo must end once the hero stops");

        // The countdown is 120 ticks at one hit, so a break this early can only
        // be the hero coming to rest. Without this the test would pass for a
        // Combo that merely timed out, which is a different rule.
        let drought = broke_on - slam_tick;
        assert!(
            drought < 60,
            "the Combo lasted {drought} ticks, which is long enough to be the \
             countdown rather than the hero coming to rest"
        );

        // And the hero really is at a standstill against the floor, not merely
        // somewhere the implementation guessed at.
        let speed = world
            .body_velocity(hero_id)
            .expect("the hero is alive")
            .length();
        let pos = hero(&world).pos;
        assert!(
            speed < crate::combo::REST_SPEED,
            "the hero must be still when the Combo breaks, speed {speed}"
        );
        assert!(
            (pos.y - (1.0 + PLAYER_RADIUS)).abs() < 1e-3,
            "the hero must be resting on the floor, got {pos:?}"
        );
        let settled = pos;
        for _ in 0..30 {
            world.step();
        }
        assert!(
            (hero(&world).pos - settled).length() < 1e-3,
            "the hero must have stopped, not slowed: {settled:?} then {:?}",
            hero(&world).pos
        );
    }

    #[test]
    fn the_combo_survives_a_bounce_off_the_ground() {
        // Story 38: a glancing touch of the ground must not punish the player.
        // Terrain contact carries no restitution, so the only way the hero
        // leaves the floor it just hit is a Tongue hauling it back up, which is
        // exactly the case the rule is about: contact the hero does not stay in.
        let mut world = World::from_level(&hall());
        let hero_id = world.hero_id();
        let slam_tick = slam_the_hall(&mut world);

        // Anchored overhead, then dropped at the floor underneath it so the
        // Stretch is loaded and the hero is thrown straight back up.
        let anchor = attach(&mut world, Vec2::new(0.0, 20.0));
        world.place(hero_id, Vec2::new(anchor.x, 0.6), Vec2::new(20.0, -30.0));

        world.step();
        let touched = hero(&world).pos;
        assert!(
            (touched.y - PLAYER_RADIUS).abs() < 1e-3,
            "the fixture must actually reach the floor, got {touched:?}"
        );
        assert_eq!(
            world.snapshot().combo,
            1,
            "touching the ground must not break the Combo"
        );

        // And back up again, with the count intact the whole way.
        let mut left_on = None;
        for _ in 0..30 {
            world.step();
            assert_eq!(
                world.snapshot().combo,
                1,
                "the Combo must survive the whole bounce"
            );
            if hero(&world).pos.y > 2.0 {
                left_on = Some(world.snapshot().tick);
                break;
            }
        }
        let left_on = left_on.expect("the fixture never bounced back off the floor");

        // Inside the window, so this measured a bounce and not a timeout.
        assert!(
            left_on - slam_tick < 120,
            "the bounce took {} ticks, longer than the countdown it has to fit \
             inside",
            left_on - slam_tick
        );
    }

    #[test]
    fn a_single_tick_of_slow_ground_contact_is_not_coming_to_rest() {
        // Why the rest predicate has to be sustained rather than instantaneous,
        // which is the half of it a bounce with sideways speed never exercises.
        // Terrain contact cancels the motion heading into the surface, so a
        // hero arriving straight down at thirty units per second has a speed of
        // almost nothing for exactly one tick while being thrown straight back
        // up. On a one-tick predicate that reads as a standstill and the run
        // dies on the player's best landing.
        let mut world = World::from_level(&hall());
        let hero_id = world.hero_id();
        slam_the_hall(&mut world);

        let anchor = attach(&mut world, Vec2::new(0.0, 20.0));
        world.place(hero_id, Vec2::new(anchor.x, 0.6), Vec2::new(0.2, -30.0));

        world.step();
        let speed = world
            .body_velocity(hero_id)
            .expect("the hero is alive")
            .length();
        let pos = hero(&world).pos;
        assert!(
            (pos.y - PLAYER_RADIUS).abs() < 1e-3,
            "the fixture must land the hero on the floor, got {pos:?}"
        );
        assert!(
            speed < crate::combo::REST_SPEED,
            "the fixture must leave the hero momentarily slower than the rest \
             threshold, or it proves nothing: speed {speed}"
        );
        assert_eq!(
            world.snapshot().combo,
            1,
            "one tick of slow contact is a landing, not a standstill"
        );

        // And it was genuinely one tick: the hero is away again immediately,
        // which is what distinguishes this from the standstill case.
        world.step();
        assert!(
            hero(&world).pos.y > PLAYER_RADIUS,
            "the fixture must leave the floor again, got {:?}",
            hero(&world).pos
        );
        assert_eq!(world.snapshot().combo, 1, "the Combo must have survived");
    }

    #[test]
    fn the_combo_survives_a_release_and_a_re_attach() {
        // Story 37, and ADR-0011's "persisting across Release and re-attach":
        // chaining between Anchors is the high-skill line, so letting go must
        // not cost the run. Every stage below is inside the 120 tick window a
        // one-hit Combo gets.
        let mut world = World::from_level(&hall());
        let slam_tick = slam_the_hall(&mut world);

        attach(&mut world, Vec2::new(0.0, 20.0));
        for _ in 0..10 {
            world.step();
        }
        assert_eq!(world.snapshot().tongues.len(), 1, "the Tongue must Attach");
        assert_eq!(world.snapshot().combo, 1, "Attaching must not cost the run");

        // The second tap on an occupied slot is a Release, per ADR-0006.
        lift(&mut world, 1);
        tap_toward(&mut world, 2, Vec2::Y);
        world.step();
        assert!(
            world.snapshot().tongues.is_empty(),
            "the second tap must have Released: {:?}",
            world.snapshot().tongues
        );
        assert_eq!(
            world.snapshot().combo,
            1,
            "a Release must not break the Combo"
        );

        for _ in 0..10 {
            world.step();
            assert_eq!(
                world.snapshot().combo,
                1,
                "the Combo must survive unconstrained ballistic flight"
            );
        }

        // Re-Attach, which is the half of the rule a Release alone would not
        // catch: the count has to survive the slot being filled again.
        lift(&mut world, 2);
        tap_toward(&mut world, 3, Vec2::new(0.0, 20.0));
        world.step();
        assert_eq!(
            world.snapshot().tongues.len(),
            1,
            "the third tap must re-Attach: {:?}",
            world.snapshot().tongues
        );
        assert_eq!(
            world.snapshot().combo,
            1,
            "a re-Attach must not break the Combo"
        );

        for _ in 0..20 {
            world.step();
            assert_eq!(world.snapshot().combo, 1, "the run must still be live");
        }
        assert!(
            world.snapshot().tick - slam_tick < 120,
            "the sequence outran the countdown, so a break here would be \
             ambiguous"
        );
    }

    /// A straight line of touching Enemies with no terrain anywhere.
    ///
    /// No terrain at all is the point: with nothing to come to rest on, the
    /// only thing that can end a Combo here is the countdown, which is what
    /// makes the window measurable. The Kill Bound is wide enough that a hero
    /// at the pass-through ceiling cannot reach it inside the longest window.
    fn slam_row(count: u32) -> Level {
        let level = Level {
            world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(5_000.0)),
            static_terrain: vec![],
            player_spawn: Vec2::ZERO,
            enemy_spawns: (0..count)
                .map(|i| EnemySpawn {
                    pos: Vec2::new(2.0 + i as f32, 0.0),
                    toughness: 100.0,
                    mass: 1.0,
                })
                .collect(),
        };
        level.validate().expect("fixture must be a valid level");
        level
    }

    /// Runs the hero through `count` Enemies above the pass-through ceiling and
    /// reports the Combo it reached and how many Sim Ticks passed between its
    /// last hit and its expiry.
    ///
    /// Above [`combat::PASS_THROUGH_MOMENTUM`] an impact drains nothing, so the
    /// hero crosses the whole row at one speed and every Enemy in it is a hit.
    /// That is what makes the hit count the fixture's choice rather than an
    /// emergent property of the drain.
    fn measure_window(count: u32) -> (u32, u64) {
        let mut world = World::from_level(&slam_row(count));
        world.place(world.hero_id(), Vec2::ZERO, Vec2::new(160.0, 0.0));

        let mut hits = 0;
        let mut last_hit = 0;
        for _ in 0..2_000 {
            world.step();
            let snapshot = world.snapshot();
            if snapshot.combo > hits {
                hits = snapshot.combo;
                last_hit = snapshot.tick;
            } else if hits > 0 && snapshot.combo == 0 {
                return (hits, snapshot.tick - last_hit);
            }
        }
        panic!("a Combo of {hits} hits never expired");
    }

    #[test]
    fn the_countdown_window_tightens_as_hits_mount() {
        // Story 40: long chains get harder rather than coasting. The windows
        // are stated as times, which is what they were chosen as, and ADR-0002
        // locks the tick so the conversion is exact.
        let (one_hit, short_run) = measure_window(1);
        assert_eq!(one_hit, 1, "the fixture must land exactly one hit");
        assert_eq!(
            short_run, 120,
            "a one-hit Combo gets two seconds to find its next Slam"
        );

        let (six_hits, long_run) = measure_window(6);
        assert_eq!(six_hits, 6, "the fixture must land exactly six hits");
        assert_eq!(
            long_run, 90,
            "six hits should be down to a second and a half"
        );
        assert!(
            long_run < short_run,
            "the window did not tighten: {short_run} ticks at one hit, \
             {long_run} at six"
        );
    }

    #[test]
    fn the_countdown_window_stops_shrinking_at_a_hard_floor() {
        // Story 41: a long Combo has to stay humanly possible, so the
        // tightening settles rather than converging on zero. Twenty hits is
        // past where the floor binds and forty is far past it; both get the
        // same window, and it is not nothing.
        let (twenty, twenty_window) = measure_window(20);
        let (forty, forty_window) = measure_window(40);

        assert_eq!(twenty, 20, "the fixture must land exactly twenty hits");
        assert_eq!(forty, 40, "the fixture must land exactly forty hits");
        assert_eq!(
            twenty_window, 36,
            "a long Combo keeps six tenths of a second"
        );
        assert_eq!(
            forty_window, twenty_window,
            "the floor must hold however long the run gets: {twenty_window} \
             ticks at twenty hits against {forty_window} at forty"
        );
    }

    #[test]
    fn the_combo_is_an_uncapped_count_and_never_a_score() {
        // Story 36, and ADR-0011's standing constraint: the Core Sim tracks a
        // count. Forty Slams is forty, not a multiplier, not a capped value,
        // and not a number scaled by anything.
        let (hits, _) = measure_window(40);
        assert_eq!(hits, 40, "forty Slams must read as exactly forty hits");
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

    /// The Thrown state, through the one seam: a `Level`, Sim Ticks, and
    /// `Snapshot`s. Nothing here reads `World` internals and nothing depends on
    /// the order bodies happen to occupy in a list.
    ///
    /// ADR-0011 is the contract and ADR-0015 holds the two decisions ticket #18
    /// had to make. The rules themselves are pure functions in
    /// [`crate::thrown`], tested beside the module; what these assert is the
    /// wiring, which is the part that can be wrong: whether the promotion is
    /// read at the right point in the tick, and whether the matrix is reached
    /// at all.
    mod the_thrown_state {
        use super::*;
        use crate::combat::ENEMY_HALF_EXTENTS;
        use crate::thrown;

        /// A hero at the origin and one Enemy `distance` units along the x
        /// axis, with **no terrain whatsoever**.
        ///
        /// Horizontal so gravity contributes exactly nothing to the axis being
        /// measured, and terrain-free so nothing in a tension measurement can
        /// be a contact: a Thrown Enemy is destroyed by terrain, so a fixture
        /// that measured promotion in a room would be measuring two rules at
        /// once. The scenes that want terrain build their own below.
        ///
        /// `toughness` is far above anything reachable here, so the hero can
        /// never remove the Enemy and change the body count under a test that
        /// was measuring a state change.
        fn yank(distance: f32, enemy_mass: f32) -> Level {
            let level = Level {
                world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(500.0)),
                static_terrain: vec![],
                player_spawn: Vec2::ZERO,
                enemy_spawns: vec![EnemySpawn {
                    pos: Vec2::new(distance, 0.0),
                    toughness: 100_000.0,
                    mass: enemy_mass,
                }],
            };
            level.validate().expect("fixture must be a valid level");
            level
        }

        /// The one Enemy in a world, by identity rather than by slice position.
        fn enemy_of(world: &World) -> EntityId {
            let hero = world.hero_id();
            world
                .snapshot()
                .bodies
                .iter()
                .find(|b| b.id != hero)
                .map(|b| b.id)
                .expect("the fixture spawns one Enemy")
        }

        /// What kind of body the snapshot says `id` is, or `None` if it has
        /// left the simulation.
        fn kind_of(world: &World, id: EntityId) -> Option<BodyKind> {
            world.snapshot().body(id).map(|b| b.kind)
        }

        /// The tension a `yank` fixture applies on its first tick, which is
        /// pure Hooke's law over mass: the damping term is
        /// `damping * relative_radial_vel` and is identically zero at a
        /// standstill. ADR-0003 records this as the reason a figure measured at
        /// rest is safe to assert against and one measured in motion is not,
        /// which is why every analytic prediction here is a **first** tick out
        /// of a standstill.
        fn tension_at_rest(world: &World, distance: f32, mass: f32) -> f32 {
            world.hero_pull_strength() * (distance - REST_LENGTH) / mass
        }

        /// One long floor with the hero on it and one Enemy `distance` units
        /// along it, both resting exactly on the surface.
        ///
        /// The Tongue is horizontal, so the pull drags the Enemy **along** the
        /// terrain it is already touching. That is the only geometry that can
        /// reach ADR-0011's terrain row at all: the pull is always toward the
        /// hero, so a surface between the two bodies would block the tap under
        /// ADR-0012 and the Enemy would never be Attached in the first place.
        ///
        /// Both bodies rest at exactly `floor_top + PLAYER_RADIUS`, where the
        /// swept test reports a zero-depth contact rather than a crossing, so
        /// the contact is genuine and continuous from the first tick.
        fn shared_floor(distance: f32, enemy_mass: f32) -> Level {
            let level = Level {
                world_bounds: Aabb::new(Vec2::new(0.0, 0.0), Vec2::splat(500.0)),
                static_terrain: vec![Aabb::new(
                    Vec2::new(distance * 0.5, -1.0),
                    Vec2::new(60.0, 1.0),
                )],
                player_spawn: Vec2::new(0.0, PLAYER_RADIUS),
                enemy_spawns: vec![EnemySpawn {
                    pos: Vec2::new(distance, PLAYER_RADIUS),
                    toughness: 100_000.0,
                    mass: enemy_mass,
                }],
            };
            level.validate().expect("fixture must be a valid level");
            level
        }

        /// Far enough that the Tongue genuinely Throws a baseline Enemy, and
        /// the figure every assertion in the terrain pair is sized against:
        /// `DEFAULT_PULL_STRENGTH * (14 - REST_LENGTH)` is 520, which is a mass
        /// 1 Enemy's tension and half of a mass 2 Enemy's.
        const THROW_DISTANCE: f32 = 14.0;

        #[test]
        fn a_change_of_kind_snaps_rather_than_blending() {
            // ADR-0002 keeps discrete events out of interpolation, and an Enemy
            // becoming Thrown is exactly one: there is no state between a
            // target and a weapon, so a blend would draw a body that is half of
            // each for most of a frame.
            //
            // Asserted over a real pair of `Snapshot`s from a real promotion
            // rather than a hand-built pair, because the thing that can be
            // wrong is whether the two snapshots the Shell is handed differ in
            // kind at all.
            let mut world = World::from_level(&yank(THROW_DISTANCE, 1.0));
            let enemy = enemy_of(&world);
            tap(&mut world, 1, Vec2::new(THROW_DISTANCE, 0.0));
            world.step();

            let previous = world.previous_snapshot().clone();
            assert_eq!(
                previous.body(enemy).expect("alive").kind,
                BodyKind::Enemy,
                "the fixture must straddle the promotion, or nothing snaps"
            );
            assert_eq!(
                world.snapshot().body(enemy).expect("alive").kind,
                BodyKind::Thrown
            );

            let mut view = Snapshot::default();
            crate::frame::interpolate_into(&previous, world.snapshot(), 0.5, &mut view);
            let drawn = *view.body(enemy).expect("alive");
            let current = *world.snapshot().body(enemy).expect("alive");
            assert_eq!(
                drawn.kind,
                BodyKind::Thrown,
                "a half-interpolated frame must already draw the weapon"
            );
            assert_eq!(
                drawn.pos, current.pos,
                "the kind changed, so the body must snap to its current position"
            );

            // Vacuity guard, and it is the whole test: on a tick where the kind
            // did *not* change, the very same call blends. Without this the
            // assertion above would pass for an implementation that never
            // interpolated anything at all.
            let before = *world.snapshot().body(enemy).expect("alive");
            world.step();
            let mut blended = Snapshot::default();
            crate::frame::interpolate_into(
                world.previous_snapshot(),
                world.snapshot(),
                0.5,
                &mut blended,
            );
            let midpoint = *blended.body(enemy).expect("alive");
            let now = *world.snapshot().body(enemy).expect("alive");
            assert_eq!(now.kind, BodyKind::Thrown, "both ticks are Thrown now");
            assert!(
                (midpoint.pos - before.pos.lerp(now.pos, 0.5)).length() < 1e-6
                    && (midpoint.pos - now.pos).length() > 1e-6,
                "two ticks of the same kind must blend: {midpoint:?} between \
                 {before:?} and {now:?}"
            );
        }

        /// The `yank` scene with a wall beyond the Enemy, for asking what a tap
        /// Attaches to once the Enemy has become a weapon.
        ///
        /// The wall sits past the Enemy and is tall, so a tap along the axis
        /// meets the Enemy first while the terrain behind it is a genuine
        /// alternative rather than empty sky.
        fn yank_against_a_wall(enemy_mass: f32) -> Level {
            let mut level = yank(THROW_DISTANCE, enemy_mass);
            level.static_terrain.push(Aabb::new(
                Vec2::new(THROW_DISTANCE + 6.5, 0.0),
                Vec2::new(0.5, 20.0),
            ));
            level.validate().expect("fixture must be a valid level");
            level
        }

        #[test]
        fn a_thrown_enemy_is_no_longer_a_target_and_a_tracking_one_still_is() {
            // ADR-0015's first decision: a weapon in flight cannot be grappled.
            // The two runs differ in the Enemy's mass alone, so the second is
            // the mutation of the first: at mass 1 the Enemy is Thrown when the
            // second Gesture goes out and at mass 2 it is still a target, and
            // the tap, the aim and the scene are otherwise identical.
            let second_anchor = |enemy_mass: f32| -> (Anchor, EntityId, Vec2, Vec2) {
                let mut world = World::from_level(&yank_against_a_wall(enemy_mass));
                let enemy = enemy_of(&world);
                tap(&mut world, 1, Vec2::new(THROW_DISTANCE, 0.0));
                world.step();

                // The first finger stays down, which ADR-0006 makes the only
                // way to hold two Tongues: a lift and a second tap would come
                // back to slot 0 and Release it instead.
                let aim = Vec2::new(THROW_DISTANCE, 0.0) - hero(&world).pos;
                touch_frame(
                    &mut world,
                    &[
                        (1, TouchPhase::Stationary, aim),
                        (2, TouchPhase::Started, aim),
                    ],
                );
                let from = hero(&world).pos;
                world.step();

                // Slot 0's link is untouched either way: this decision forbids
                // re-targeting a weapon, not holding one that is already held.
                assert_eq!(
                    world
                        .snapshot()
                        .tongues
                        .iter()
                        .find(|t| t.slot == 0)
                        .map(|t| t.anchor),
                    Some(Anchor::Movable(enemy)),
                    "the Tongue already Attached must survive at Enemy mass {enemy_mass}"
                );
                let second = world
                    .snapshot()
                    .tongues
                    .iter()
                    .find(|t| t.slot == 1)
                    .map(|t| t.anchor)
                    .expect("the second Gesture must Attach to something");
                (second, enemy, from, aim)
            };

            // Thrown: the tap reaches straight past the weapon to the terrain
            // behind it, at exactly the point a pure cast finds. Bit-identical,
            // because a filter that merely deprioritised a Thrown Enemy rather
            // than removing it from the target set would differ in the last few
            // bits of nothing else.
            let (thrown_anchor, _, from, aim) = second_anchor(1.0);
            // The same origin and the same aim the tap itself used, so the two
            // casts differ in nothing but whether the Enemy was a candidate.
            let pure = crate::tongue::cast(from, aim, &yank_against_a_wall(1.0).static_terrain)
                .expect("the ray meets the wall");
            match thrown_anchor {
                Anchor::Fixed(p) => assert!(
                    p.x.to_bits() == pure.x.to_bits() && p.y.to_bits() == pure.y.to_bits(),
                    "a tap past a Thrown Enemy landed at {p:?} and not at the pure {pure:?}"
                ),
                other => panic!("a Thrown Enemy must not be grappable, got {other:?}"),
            }

            // Tracking: the same tap prefers the Enemy, which is what makes the
            // run above a statement about the Thrown state rather than about
            // this fixture's geometry.
            let (tracking_anchor, enemy, _, _) = second_anchor(2.0);
            assert_eq!(
                tracking_anchor,
                Anchor::Movable(enemy),
                "an Enemy that is still a target must still be preferred"
            );
        }

        /// Throws the one Enemy in a `yank` fixture and answers with the world
        /// on the tick it became Thrown, still Attached.
        fn thrown_enemy(mass: f32) -> (World, EntityId) {
            let mut world = World::from_level(&yank(THROW_DISTANCE, mass));
            let enemy = enemy_of(&world);
            tap(&mut world, 1, Vec2::new(THROW_DISTANCE, 0.0));
            world.step();
            assert_eq!(
                kind_of(&world, enemy),
                Some(BodyKind::Thrown),
                "the fixture must Throw the Enemy"
            );
            (world, enemy)
        }

        #[test]
        fn releasing_a_thrown_enemy_slings_it_on_a_ballistic_arc() {
            // ADR-0011: "Releasing the Tongue slings it into a ballistic arc on
            // its residual velocity." Ballistic is a checkable claim rather
            // than a description: with the slot cleared the only force left on
            // the body is gravity, so one tick of its motion is the analytic
            // one and nothing of the spring survives into it.
            let (mut world, enemy) = thrown_enemy(1.0);

            // ADR-0006: the finger that fired still owns slot 0, so it gives up
            // ownership first and the next tap lands on that slot and Releases
            // it. A Release costs nothing and is instant, per ADR-0012.
            lift(&mut world, 1);
            tap(&mut world, 2, Vec2::new(THROW_DISTANCE, 0.0));

            let before = world.snapshot().body(enemy).expect("alive").pos;
            let residual = world.body_velocity(enemy).expect("alive");
            // It really is carrying something, or "on its residual velocity"
            // would be satisfied by a body that had stopped dead.
            assert!(
                residual.x < -1.0,
                "the Throw must leave the Enemy real momentum, got {residual:?}"
            );
            world.step();

            // Velocity Verlet under a constant acceleration is exact: both
            // evaluations see the same gravity, so the velocity step is
            // `0.5 * (g + g) * DT` and the position step is
            // `v * DT + 0.5 * g * DT^2`. The bound is float rounding in those
            // two expressions and nothing else.
            let after = world.snapshot().body(enemy).expect("alive").pos;
            let v = world.body_velocity(enemy).expect("alive");
            let expected_v = residual + GRAVITY * DT;
            let expected_p = before + residual * DT + 0.5 * GRAVITY * DT * DT;
            assert!(
                (v - expected_v).length() < 1e-6,
                "a Released Enemy accelerated by something other than gravity: \
                 {v:?} against the ballistic {expected_v:?}"
            );
            assert!(
                (after - expected_p).length() < 1e-6,
                "a Released Enemy moved to {after:?} against the ballistic {expected_p:?}"
            );
            // The second tap really Released rather than re-firing, asserted
            // where a player would see it: the slot is absent from the
            // `Snapshot`, which is the whole of what a Release looks like.
            assert!(
                world.snapshot().tongues.is_empty(),
                "the second tap must Release rather than re-fire: {:?}",
                world.snapshot().tongues
            );
            // And it stays a weapon. Nothing demotes a Thrown Enemy: the state
            // is terminal, which is what makes a slung Enemy still destroy what
            // it lands among.
            assert_eq!(kind_of(&world, enemy), Some(BodyKind::Thrown));
        }

        #[test]
        fn an_unreleased_thrown_enemy_barrels_back_toward_the_hero() {
            // ADR-0011's other half: "If the player does not Release, it
            // barrels back toward the hero." The Released run above is the
            // control, and the discriminator is the **acceleration** rather
            // than the direction of travel: a Thrown Enemy's residual velocity
            // already points at the hero at the moment of Release, so both runs
            // close the gap and only the held one keeps gaining on it.
            //
            // The Tongue is horizontal and gravity has no x component at all,
            // so x alone separates the two cases exactly.
            const WINDOW: u32 = 5;
            let closing_speeds = |release: bool| -> Vec<f32> {
                let (mut world, enemy) = thrown_enemy(1.0);
                if release {
                    lift(&mut world, 1);
                    tap(&mut world, 2, Vec2::new(THROW_DISTANCE, 0.0));
                }
                (0..WINDOW)
                    .map(|_| {
                        world.step();
                        // Toward the hero is negative x in this fixture, so a
                        // positive closing speed is the Enemy coming back.
                        -world.body_velocity(enemy).expect("alive").x
                    })
                    .collect()
            };

            let held = closing_speeds(false);
            let released = closing_speeds(true);

            // Held: it gains on the hero every single tick.
            for pair in held.windows(2) {
                assert!(
                    pair[1] > pair[0],
                    "an unreleased Thrown Enemy stopped gaining on the hero: {held:?}"
                );
            }
            // Released: no horizontal force exists, so the closing speed is
            // constant to the bit. This is the mutation that makes the
            // assertion above mean something: without it, a run where the
            // Tongue had quietly stayed Attached would look identical.
            for speed in &released {
                assert!(
                    (speed - released[0]).abs() < 1e-6,
                    "a Released Enemy changed its closing speed: {released:?}"
                );
            }
            assert!(
                held[WINDOW as usize - 1] > released[WINDOW as usize - 1] * 2.0,
                "holding the Tongue must build real closing speed: {held:?} against {released:?}"
            );

            // And the gap really closes, which is the player-visible half of
            // the claim rather than a statement about velocities.
            let (mut world, enemy) = thrown_enemy(1.0);
            let gap =
                |w: &World| (w.snapshot().body(enemy).expect("alive").pos - hero(w).pos).length();
            let start = gap(&world);
            for _ in 0..WINDOW {
                world.step();
            }
            assert!(
                gap(&world) < start - 0.5,
                "the Enemy did not barrel back: {start} then {}",
                gap(&world)
            );
        }

        /// A hero, a projectile Enemy [`THROW_DISTANCE`] along the x axis, and
        /// a second Enemy parked far above the scene until a test places it.
        ///
        /// No terrain at all, so nothing here can reach the matrix's terrain
        /// row and the Enemy rows are measured on their own.
        ///
        /// The victim starts out of the way and is placed on the flight path
        /// afterwards, which is not laziness. The pull is always *toward* the
        /// hero, so an Enemy sitting on the flight path when the tap went out
        /// would be the nearer target and ADR-0012's arc would Attach to it
        /// instead: the Tongue would never reach the far Enemy and there would
        /// be no projectile. Assigning the geometry is the same scaffolding
        /// `slam_run` uses to assign the hero's momentum rather than earn it,
        /// and for the same reason.
        fn firing_line(victim_mass: f32) -> Level {
            let level = Level {
                world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(500.0)),
                static_terrain: vec![],
                player_spawn: Vec2::ZERO,
                enemy_spawns: vec![
                    EnemySpawn {
                        pos: Vec2::new(THROW_DISTANCE, 0.0),
                        toughness: 100_000.0,
                        mass: 1.0,
                    },
                    // Far off the tapped vector: 70 degrees away from it, well
                    // outside ADR-0012's 10 degree arc, so the tap cannot
                    // possibly prefer it.
                    EnemySpawn {
                        pos: Vec2::new(THROW_DISTANCE, 40.0),
                        toughness: 100_000.0,
                        mass: victim_mass,
                    },
                ],
            };
            level.validate().expect("fixture must be a valid level");
            level
        }

        /// The speed a placed projectile is sent into a victim at.
        ///
        /// Fast enough that one Sim Tick's motion genuinely crosses the gap
        /// below, so the strike is a swept contact rather than an overlap that
        /// happened to be there.
        const PROJECTILE_SPEED: f32 = 120.0;

        /// Runs `firing_line` up to the tick a Thrown Enemy strikes a victim of
        /// `victim_mass`, and answers with the world at that point.
        ///
        /// `victim_x` is where the victim is placed, so one caller can put it
        /// in the path and another can leave it out of reach as a control.
        fn strike(victim_mass: f32, victim_x: f32) -> (World, EntityId, EntityId) {
            let mut world = World::from_level(&firing_line(victim_mass));
            let hero = world.hero_id();
            // By identity and by spawn position rather than by slice order: the
            // two Enemies are told apart by where the fixture put them.
            let ids: Vec<(EntityId, Vec2)> = world
                .snapshot()
                .bodies
                .iter()
                .filter(|b| b.id != hero)
                .map(|b| (b.id, b.pos))
                .collect();
            let projectile = ids
                .iter()
                .find(|(_, p)| p.y == 0.0)
                .expect("the fixture spawns one Enemy on the axis")
                .0;
            let victim = ids
                .iter()
                .find(|(id, _)| *id != projectile)
                .expect("the fixture spawns two Enemies")
                .0;

            tap(&mut world, 1, Vec2::new(THROW_DISTANCE, 0.0));
            world.step();
            assert_eq!(
                kind_of(&world, projectile),
                Some(BodyKind::Thrown),
                "the fixture must Throw the projectile before anything is placed"
            );

            // The projectile is aimed and the victim put in its path. The
            // Tongue stays Attached throughout, which is what lets the lesser
            // row's "keeps its own link" clause be asserted at all.
            world.place(
                projectile,
                Vec2::new(THROW_DISTANCE, 0.0),
                Vec2::new(PROJECTILE_SPEED, 0.0),
            );
            world.place(victim, Vec2::new(victim_x, 0.0), Vec2::ZERO);
            world.step();
            (world, projectile, victim)
        }

        /// Where a victim has to sit to be struck inside one tick, and where it
        /// has to sit to be missed.
        ///
        /// One tick at [`PROJECTILE_SPEED`] covers two units, less the Tongue's
        /// own pull back toward the hero, so a victim two units along is struck
        /// and one twenty units along is nowhere near.
        const IN_THE_PATH: f32 = THROW_DISTANCE + 2.0;
        const OUT_OF_REACH: f32 = THROW_DISTANCE + 20.0;

        #[test]
        fn a_thrown_enemy_destroys_a_lesser_enemy_and_keeps_its_link_and_its_state() {
            // ADR-0011's second inverted row, which has four separate clauses
            // and every one of them is asserted: the target is destroyed, the
            // projectile keeps its link, it keeps its chain state, and it takes
            // minor proportional velocity damping.
            const VICTIM_MASS: f32 = 0.5;
            let (world, projectile, victim) = strike(VICTIM_MASS, IN_THE_PATH);

            assert_eq!(
                kind_of(&world, victim),
                None,
                "a lesser Enemy struck by a Thrown one must be destroyed"
            );
            assert_eq!(
                kind_of(&world, projectile),
                Some(BodyKind::Thrown),
                "the projectile must survive and must still be Thrown: \
                 it keeps its own chain state"
            );
            // Its link: the same slot, still Attached to the same body by
            // identity. A Tongue that had been re-pointed or dropped would show
            // up here and nowhere else.
            assert_eq!(
                world
                    .snapshot()
                    .tongues
                    .iter()
                    .map(|t| (t.slot, t.anchor))
                    .collect::<Vec<_>>(),
                vec![(0u8, Anchor::Movable(projectile))],
                "the projectile must keep its own link: {:?}",
                world.snapshot().tongues
            );

            // "Minor proportional velocity damping", measured against the
            // control: the identical scene with the victim left out of reach,
            // where the only thing acting on the projectile is the Tongue and
            // gravity. The victim exerts no force on anything, so the two runs
            // are the same simulation up to the strike and the ratio is the
            // drag alone.
            let (control, control_projectile, _) = strike(VICTIM_MASS, OUT_OF_REACH);
            assert_eq!(
                kind_of(&control, control_projectile),
                Some(BodyKind::Thrown),
                "the control must not strike anything"
            );
            let struck = world.body_velocity(projectile).expect("alive");
            let unstruck = control
                .body_velocity(control_projectile)
                .expect("the control projectile must survive");
            let expected = 1.0 - thrown::PLOUGH_DRAG * VICTIM_MASS / 1.0;
            for (axis, (a, b)) in [(struck.x, unstruck.x), (struck.y, unstruck.y)]
                .into_iter()
                .enumerate()
            {
                assert!(
                    (a - b * expected).abs() <= 1e-5 * b.abs().max(1.0),
                    "axis {axis}: the strike left {a} where {expected} of the \
                     unstruck {b} is {}",
                    b * expected
                );
            }
            // And minor means minor: the weapon keeps most of its speed, or
            // ploughing would be a cost rather than the reward ADR-0011 makes
            // it. Both sides of the band, because "lost nothing at all" is as
            // wrong as "lost everything" and only one of the two is a vacuity
            // guard.
            let kept = struck.length() / unstruck.length();
            assert!(
                (0.8..1.0).contains(&kept),
                "the projectile kept {kept} of its speed, outside the minor band"
            );
        }

        #[test]
        fn a_thrown_enemy_and_an_equal_weight_class_destroy_each_other_and_free_the_slot() {
            // ADR-0011's third inverted row: mutual destruction, both
            // vaporized, the player's Anchor slot freed. The mutation of the
            // test above, and the only thing that differs between the two
            // scenes is the victim's mass.
            let (world, projectile, victim) = strike(1.0, IN_THE_PATH);

            assert_eq!(
                kind_of(&world, victim),
                None,
                "the target must be vaporized"
            );
            assert_eq!(
                kind_of(&world, projectile),
                None,
                "an equal weight class must take the projectile with it"
            );
            assert!(
                world.snapshot().tongues.is_empty(),
                "mutual destruction must free the player's Anchor slot: {:?}",
                world.snapshot().tongues
            );
            // The hero keeps exactly the momentum it had. ADR-0014 makes this a
            // Release and ADR-0012 makes a Release cost nothing, so losing a
            // weapon must not also shove the hero.
            let hero = world.hero_id();
            assert!(
                world.body_velocity(hero).expect("hero").x > 0.0,
                "the hero must still be carrying its own pull"
            );
        }

        #[test]
        fn a_thrown_enemys_kill_does_not_count_toward_the_combo() {
            // ADR-0015's second decision. The Combo is an unbroken run of
            // Slams, and the GLOSSARY makes a Slam the outcome of **the player
            // colliding** with an Enemy: when a Thrown Enemy destroys a lesser
            // one the player caused it but did not collide with it, and spent
            // none of their own momentum doing it.
            //
            // The control is the same scene, the same victim and the same
            // death, differing only in who delivered it, which is exactly the
            // distinction the decision turns on.
            let (world, _, victim) = strike(0.5, IN_THE_PATH);
            assert_eq!(
                kind_of(&world, victim),
                None,
                "the fixture must land the Thrown kill"
            );
            assert_eq!(
                world.snapshot().combo,
                0,
                "a Thrown Enemy's kill must not count as a Slam"
            );

            // The vacuity guard, and it is doing real work: a scene that could
            // not raise the count at all would satisfy the assertion above for
            // the wrong reason. Here the hero itself drives into the very same
            // body and the very same tick's machinery counts it.
            let mut control = World::from_level(&firing_line(0.5));
            let hero_id = control.hero_id();
            let victim_id = control
                .snapshot()
                .bodies
                .iter()
                .find(|b| b.pos.y > 1.0)
                .map(|b| b.id)
                .expect("the fixture parks its victim above the scene");
            let at = control.snapshot().body(victim_id).expect("alive").pos;
            control.place(
                hero_id,
                at - Vec2::new(2.0, 0.0),
                Vec2::new(PROJECTILE_SPEED, 0.0),
            );
            control.step();
            assert_eq!(
                control.snapshot().combo,
                1,
                "the hero's own collision with the same body must count"
            );
        }

        #[test]
        fn the_hero_passes_through_a_thrown_enemy_because_hero_death_is_deferred() {
            // **The deliberate gap, recorded as a test.** ADR-0011 says a
            // Thrown Enemy striking the hero destroys the hero outright, and
            // the hero's fail state, any lose condition, run structure and
            // restart are all deferred in the map's fog, so there is nothing
            // for that branch to trigger. Ticket #18 therefore leaves it out
            // and this pins what happens instead: nothing at all.
            //
            // Not a Kick, deliberately. ADR-0011 makes the state the arbiter of
            // what an Enemy striking the hero does, and a Kick is what the
            // *tracking* state does; implementing one here would contradict the
            // ADR rather than defer to it. The seam is the `BodyKind::Enemy`
            // filter in `resolve_impacts` and the one in `resolve_thrown`, and
            // whoever implements the fail state writes the branch there.
            //
            // The control is the same collision against a tracking Enemy, which
            // is the only thing that makes "nothing happened" a measurement:
            // the identical geometry at Enemy mass 2 is a Slam.
            let collide = |enemy_mass: f32| -> (World, EntityId, u32) {
                let mut world = World::from_level(&yank(THROW_DISTANCE, enemy_mass));
                let enemy = enemy_of(&world);
                tap(&mut world, 1, Vec2::new(THROW_DISTANCE, 0.0));
                world.step();
                let at = world.snapshot().body(enemy).expect("alive").pos;
                let hero_id = world.hero_id();
                world.place(
                    hero_id,
                    at - Vec2::new(2.0, 0.0),
                    Vec2::new(PROJECTILE_SPEED, 0.0),
                );
                world.step();
                let combo = world.snapshot().combo;
                (world, enemy, combo)
            };

            let (thrown, weapon, thrown_combo) = collide(1.0);
            assert_eq!(
                kind_of(&thrown, weapon),
                Some(BodyKind::Thrown),
                "the fixture must drive the hero into a Thrown Enemy"
            );
            assert!(
                thrown.snapshot().body(thrown.hero_id()).is_some(),
                "the hero must survive, because nothing may destroy it yet"
            );
            assert_eq!(
                thrown_combo, 0,
                "a weapon in flight is not a target, so there is no Slam to count"
            );
            // The hero carries straight on: no Kick impulse reversed it, which
            // is what a tracking Enemy would have done at low speed and what
            // the ADR reserves for the tracking state.
            assert!(
                thrown.body_velocity(thrown.hero_id()).expect("hero").x > 0.0,
                "the hero was repelled by a weapon it should have passed through"
            );

            let (tracking, target, tracking_combo) = collide(2.0);
            assert_eq!(
                kind_of(&tracking, target),
                Some(BodyKind::Enemy),
                "the control must stay in the tracking band"
            );
            assert_eq!(
                tracking_combo, 1,
                "the identical collision against a target must register a Slam"
            );
        }

        #[test]
        fn a_thrown_enemy_in_contact_with_terrain_is_destroyed() {
            // ADR-0011's first inverted row, and the whole reason the state is
            // high risk: "any terrain contact ends it", so using an Enemy as a
            // weapon demands real control rather than being free damage.
            //
            // The mutation of this test is
            // `a_tracking_enemy_slides_along_terrain_and_is_never_destroyed`,
            // which is the same scene with the Enemy's mass doubled and so the
            // tension halved. Nothing else differs between the two.
            let mut world = World::from_level(&shared_floor(THROW_DISTANCE, 1.0));
            let enemy = enemy_of(&world);

            tap(&mut world, 1, Vec2::new(THROW_DISTANCE, PLAYER_RADIUS));
            world.step();

            // One tick of environmental immunity, which is not an accident: the
            // tension is read at the configuration the tick began in, and
            // through the whole of that tick's motion the body was still below
            // the threshold and still immune. Promoting before resolving the
            // matrix would destroy it against the very surface it was immune to
            // while being pulled along it.
            assert_eq!(
                kind_of(&world, enemy),
                Some(BodyKind::Thrown),
                "the pull must Throw the Enemy on the tick its tension exceeded the threshold"
            );

            world.step();
            assert_eq!(
                kind_of(&world, enemy),
                None,
                "a Thrown Enemy in contact with terrain must be destroyed"
            );
            // And the slot went with it. ADR-0014 frees a slot whenever its
            // Movable Anchor leaves the simulation, by either route, and cites
            // this matrix as the precedent it generalised.
            assert!(
                world.snapshot().tongues.is_empty(),
                "a destroyed Anchor must free its slot: {:?}",
                world.snapshot().tongues
            );
        }

        #[test]
        fn a_tracking_enemy_slides_along_terrain_and_is_never_destroyed() {
            // The AC's environmental immunity, and the mutation of
            // `a_thrown_enemy_in_contact_with_terrain_is_destroyed`: the same
            // scene with the Enemy twice as heavy, so the tension is 260
            // against the same 500 threshold. Below it, sliding into terrain
            // does nothing but bounding-box slide physics.
            const MASS: f32 = 2.0;
            // The pair closes under a relative acceleration of roughly 780, so
            // the 13 unit gap takes about 12 ticks. Eight is the whole window
            // before they meet, and the guard below asserts that rather than
            // assuming it: an impact would bring `combat` into a measurement
            // that was about terrain.
            const CLEAN_TICKS: u32 = 8;
            let mut world = World::from_level(&shared_floor(THROW_DISTANCE, MASS));
            let enemy = enemy_of(&world);
            let tension = tension_at_rest(&world, THROW_DISTANCE, MASS);
            assert!(
                tension < thrown::THROW_TENSION,
                "the fixture must stay inside the tracking band, got {tension}"
            );

            tap(&mut world, 1, Vec2::new(THROW_DISTANCE, PLAYER_RADIUS));
            world.step();

            // The first tick out of a standstill, where the damping term is
            // identically zero, so the displacement is the analytic one:
            // Velocity Verlet's position step with `vel` zero is `0.5 a DT^2`.
            // Horizontal only, and that is the substance of "nothing but slide
            // physics": terrain took the part of the motion heading into the
            // surface and left the part running along it untouched, so the
            // along-surface displacement is exactly what it would have been in
            // open space.
            let first = world.snapshot().body(enemy).expect("alive").pos;
            let expected = -0.5 * (tension) * DT * DT;
            assert!(
                (first.x - (THROW_DISTANCE + expected)).abs() <= 1e-4 * expected.abs(),
                "the Enemy slid {} against the analytic {expected}",
                first.x - THROW_DISTANCE
            );

            for _ in 1..CLEAN_TICKS {
                world.step();
            }
            let end = world.snapshot().body(enemy).expect("alive").pos;
            assert_eq!(
                kind_of(&world, enemy),
                Some(BodyKind::Enemy),
                "a tension of {tension} must leave the Enemy a target"
            );
            // Pinned to the surface throughout. A body that had left the floor
            // would have fallen 0.0144 units over this window under moon
            // gravity, which is two orders of magnitude outside this bound, so
            // the contact really was continuous and this test really did
            // measure a body against terrain.
            assert!(
                (end.y - PLAYER_RADIUS).abs() < 1e-6,
                "the Enemy left the floor it was sliding along: {end:?}"
            );
            assert!(
                end.x < THROW_DISTANCE - 1.0,
                "the Enemy barely slid at all, so nothing was measured: {end:?}"
            );
            // And the two never met, so nothing above is combat's doing.
            let hero_x = hero(&world).pos.x;
            assert!(
                end.x - hero_x > 2.0 * PLAYER_RADIUS + ENEMY_HALF_EXTENTS.x,
                "the bodies met inside the window: hero at {hero_x}, Enemy at {end:?}"
            );
        }

        #[test]
        fn a_hard_enough_pull_turns_an_enemy_into_a_thrown_one_and_a_weaker_one_does_not() {
            // ADR-0011: the tension is computed from the hero's
            // `pull_strength`, the relative velocities, and the mass
            // differential; below the threshold the pair merely tracks and
            // above it the Enemy becomes Thrown.
            //
            // The two runs differ in the Enemy's **mass alone**, so the second
            // is the mutation of the first rather than a different scene that
            // might differ for some other reason, and what separates them is
            // exactly the mass differential the AC names.
            const DISTANCE: f32 = 14.0;
            for (mass, expected) in [(1.0f32, BodyKind::Thrown), (2.0, BodyKind::Enemy)] {
                let mut world = World::from_level(&yank(DISTANCE, mass));
                let enemy = enemy_of(&world);
                let tension = tension_at_rest(&world, DISTANCE, mass);
                // The fixture must straddle the threshold, or neither run says
                // anything. Asserted rather than assumed: a later change to
                // `DEFAULT_PULL_STRENGTH` or to the threshold would otherwise
                // leave both runs on the same side of it and both assertions
                // below would still pass for one of them.
                assert_eq!(
                    tension >= thrown::THROW_TENSION,
                    expected == BodyKind::Thrown,
                    "the fixture puts {tension} on the wrong side of \
                     {} at Enemy mass {mass}",
                    thrown::THROW_TENSION
                );

                tap(&mut world, 1, Vec2::new(DISTANCE, 0.0));
                world.step();

                assert!(
                    slot_anchor(&world, 0).is_some(),
                    "the fixture must Attach to the Enemy, or no tension acts at all"
                );
                assert_eq!(
                    kind_of(&world, enemy),
                    Some(expected),
                    "at Enemy mass {mass} the tension was {tension} against a \
                     threshold of {}",
                    thrown::THROW_TENSION
                );
            }
        }
    }
}
