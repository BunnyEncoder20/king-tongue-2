//! Headless Tongue spring simulation. No renderer, no engine types, glam only.
//!
//! Models ADR-0003: the Tongue is a one-way damped spring whose force grows with
//! Stretch past a geometry-bound rest boundary. Swing is emergent, never modelled.

use glam::Vec2;

/// Player hitbox radius. All lengths are in world units; treat one unit as one metre.
pub const PLAYER_RADIUS: f32 = 0.5;

/// The rest boundary, tied to player geometry rather than tuned freely.
pub const REST_LENGTH: f32 = 2.0 * PLAYER_RADIUS;

/// Locked by ADR-0002.
pub const DT: f32 = 1.0 / 60.0;

/// Moon-scale, per the Anchor validity resolution. Earth is -9.81.
pub const GRAVITY: Vec2 = Vec2::new(0.0, -1.62);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integrator {
    /// pos += v*dt, then v += a*dt. Position advanced on the stale velocity.
    ExplicitEuler,
    /// v += a*dt, then pos += v*dt. Symplectic.
    SemiImplicitEuler,
    /// Velocity Verlet. Symplectic, second order.
    VelocityVerlet,
}

impl Integrator {
    pub fn name(self) -> &'static str {
        match self {
            Integrator::ExplicitEuler => "Explicit Euler",
            Integrator::SemiImplicitEuler => "Semi-implicit Euler",
            Integrator::VelocityVerlet => "Velocity Verlet",
        }
    }
}

/// A hero or an Enemy. One end of every Tongue is always the hero.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    pub pos: Vec2,
    pub vel: Vec2,
    pub mass: f32,
}

impl Body {
    pub fn new(pos: Vec2, mass: f32) -> Self {
        Body { pos, vel: Vec2::ZERO, mass }
    }
}

/// What a Tongue is attached to.
#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    /// Terrain. Infinite mass, so only the hero is pulled.
    Fixed(Vec2),
    /// An Enemy or other movable body, indexed into `World::enemies`.
    /// Always attaches at the exact centre of mass.
    Movable(usize),
}

/// One of at most two Tongue slots. Attachment outlives the Gesture that made it.
#[derive(Clone, Copy, Debug)]
pub struct Tongue {
    pub anchor: Anchor,
    /// Spring constant, the hero's `pull_strength`.
    pub k: f32,
    /// Damping along the Tongue axis.
    pub damping: f32,
}

/// Hero tuning. Per-hero, so weight classes differ in pull and launch speed.
#[derive(Clone, Copy, Debug)]
pub struct Hero {
    pub mass: f32,
    pub pull_strength: f32,
    pub damping: f32,
}

impl Hero {
    /// Mid-tier reference hero.
    pub fn baseline() -> Self {
        Hero { mass: 1.0, pull_strength: 40.0, damping: 2.0 }
    }
}

pub struct World {
    pub hero: Body,
    pub enemies: Vec<Body>,
    /// At most two, per ADR-0006. A third concurrent touch id is discarded.
    pub tongues: [Option<Tongue>; 2],
    pub integrator: Integrator,
    pub gravity: Vec2,
    /// Optional cap on spring force magnitude, in newtons. None means uncapped.
    pub force_ceiling: Option<f32>,
    prev_accel: Vec2,
}

impl World {
    pub fn new(hero: Body, integrator: Integrator) -> Self {
        World {
            hero,
            enemies: Vec::new(),
            tongues: [None, None],
            integrator,
            gravity: GRAVITY,
            force_ceiling: None,
            prev_accel: Vec2::ZERO,
        }
    }

    fn anchor_pos(&self, a: Anchor) -> Vec2 {
        match a {
            Anchor::Fixed(p) => p,
            Anchor::Movable(i) => self.enemies[i].pos,
        }
    }

    /// One-way damped spring. Returns the force on the hero, pulling toward the anchor.
    ///
    /// Three zones, per the rest-boundary model:
    ///   1. len <= REST_LENGTH            -> slack, zero force. A web never pushes.
    ///   2. len > REST_LENGTH             -> Hooke's law on the Stretch past the boundary.
    ///   3. approaching REST_LENGTH       -> force tapers to zero continuously, which
    ///                                       falls out of zone 2 rather than needing a
    ///                                       special case, giving smooth deceleration.
    pub fn tongue_force(&self, t: Tongue, body_pos: Vec2, body_vel: Vec2) -> Vec2 {
        let to_anchor = self.anchor_pos(t.anchor) - body_pos;
        let len = to_anchor.length();
        if len <= REST_LENGTH || len < 1e-6 {
            return Vec2::ZERO;
        }
        let dir = to_anchor / len;
        let stretch = len - REST_LENGTH;
        let radial_vel = body_vel.dot(dir);
        let mut mag = t.k * stretch - t.damping * radial_vel;
        // One-way: never push the hero away from the anchor.
        if mag < 0.0 {
            mag = 0.0;
        }
        if let Some(cap) = self.force_ceiling {
            if mag > cap {
                mag = cap;
            }
        }
        dir * mag
    }

    /// Net acceleration on the hero. Forces from both Tongues sum as vectors, so
    /// aligned pulls double and opposed pulls cancel. No constraint solver exists.
    pub fn hero_accel(&self, pos: Vec2, vel: Vec2) -> Vec2 {
        let mut force = self.gravity * self.hero.mass;
        for slot in self.tongues.iter().flatten() {
            force += self.tongue_force(*slot, pos, vel);
        }
        force / self.hero.mass
    }

    /// Reaction force on each Movable Anchor, so a tongued Enemy is pulled toward
    /// the hero by mass ratio. This is what makes a hero tongued to both terrain and
    /// an Enemy a genuine three-body system.
    fn apply_enemy_reactions(&mut self) {
        let mut reactions: Vec<(usize, Vec2)> = Vec::new();
        for slot in self.tongues.iter().flatten() {
            if let Anchor::Movable(i) = slot.anchor {
                let f = self.tongue_force(*slot, self.hero.pos, self.hero.vel);
                reactions.push((i, -f));
            }
        }
        for (i, f) in reactions {
            let m = self.enemies[i].mass;
            self.enemies[i].vel += (f / m) * DT;
        }
    }

    pub fn step(&mut self) {
        match self.integrator {
            Integrator::ExplicitEuler => {
                let a = self.hero_accel(self.hero.pos, self.hero.vel);
                self.hero.pos += self.hero.vel * DT;
                self.hero.vel += a * DT;
            }
            Integrator::SemiImplicitEuler => {
                let a = self.hero_accel(self.hero.pos, self.hero.vel);
                self.hero.vel += a * DT;
                self.hero.pos += self.hero.vel * DT;
            }
            Integrator::VelocityVerlet => {
                let a = self.hero_accel(self.hero.pos, self.hero.vel);
                self.hero.pos += self.hero.vel * DT + 0.5 * a * DT * DT;
                let a_new = self.hero_accel(self.hero.pos, self.hero.vel);
                self.hero.vel += 0.5 * (a + a_new) * DT;
                self.prev_accel = a_new;
            }
        }
        self.apply_enemy_reactions();
        for e in &mut self.enemies {
            e.pos += e.vel * DT;
        }
    }

    /// Total mechanical energy. Used to measure integrator drift: a symplectic
    /// integrator keeps this bounded, an unstable one grows it without limit.
    pub fn energy(&self) -> f32 {
        let kinetic = 0.5 * self.hero.mass * self.hero.vel.length_squared();
        let potential = -self.hero.mass * self.gravity.dot(self.hero.pos);
        let mut elastic = 0.0;
        for slot in self.tongues.iter().flatten() {
            let len = (self.anchor_pos(slot.anchor) - self.hero.pos).length();
            if len > REST_LENGTH {
                let s = len - REST_LENGTH;
                elastic += 0.5 * slot.k * s * s;
            }
        }
        kinetic + potential + elastic
    }

    /// Angular frequency of the spring, which sets the stability bound.
    /// A symplectic integrator is stable while `omega * DT < 2`.
    pub fn omega(&self, k: f32) -> f32 {
        (k / self.hero.mass).sqrt()
    }
}
