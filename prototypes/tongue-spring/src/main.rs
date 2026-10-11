mod sim;

use glam::Vec2;
use sim::*;

const ALL: [Integrator; 3] = [
    Integrator::ExplicitEuler,
    Integrator::SemiImplicitEuler,
    Integrator::VelocityVerlet,
];

fn hero_world(h: Hero, integrator: Integrator, pos: Vec2) -> World {
    let mut w = World::new(Body::new(pos, h.mass), integrator);
    w.hero.mass = h.mass;
    w
}

fn fixed_tongue(h: Hero, anchor: Vec2) -> Tongue {
    Tongue { anchor: Anchor::Fixed(anchor), k: h.pull_strength, damping: h.damping }
}

/// 1. Hang equilibrium. Attach to a ceiling anchor and let it settle.
/// Analytic prediction: equilibrium length = REST_LENGTH + m*g/k.
fn test_hang_equilibrium() {
    println!("\n=== 1. Hang equilibrium (settles under damping) ===");
    println!("Analytic: len_eq = REST_LENGTH + m*g/k   [REST_LENGTH = {REST_LENGTH}]");
    println!();
    println!("{:<14} {:>5} {:>6} {:>11} {:>11} {:>9}", "hero", "k", "mass", "predicted", "measured", "err");
    let heroes = [
        ("light/weak", Hero { mass: 0.7, pull_strength: 15.0, damping: 2.0 }),
        ("baseline", Hero::baseline()),
        ("heavy/strong", Hero { mass: 1.6, pull_strength: 120.0, damping: 6.0 }),
        ("strongest", Hero { mass: 1.4, pull_strength: 400.0, damping: 8.0 }),
    ];
    for (name, h) in heroes {
        let anchor = Vec2::new(0.0, 10.0);
        let mut w = hero_world(h, Integrator::SemiImplicitEuler, Vec2::new(0.0, 0.0));
        w.tongues[0] = Some(fixed_tongue(h, anchor));
        for _ in 0..20_000 {
            w.step();
        }
        let measured = (anchor - w.hero.pos).length();
        let predicted = REST_LENGTH + h.mass * GRAVITY.length() / h.pull_strength;
        println!(
            "{:<14} {:>5.0} {:>6.1} {:>11.5} {:>11.5} {:>9.2e}",
            name, h.pull_strength, h.mass, predicted, measured, (predicted - measured).abs()
        );
    }
    println!("\nStronger pull hangs closer to the rest boundary, exactly as described.");
    println!("REST_LENGTH is the floor: no hero hangs closer than {REST_LENGTH} even as k -> inf.");
}

/// 2. Energy drift. Undamped orbital swing. A symplectic integrator keeps total
/// mechanical energy bounded; an unstable one grows it without limit.
fn test_energy_drift() {
    println!("\n=== 2. Energy drift, undamped orbit, 3600 ticks (60 s) ===");
    println!("{:<22} {:>12} {:>12} {:>12} {:>10}", "integrator", "E initial", "E final", "drift", "verdict");
    for integ in ALL {
        let h = Hero { mass: 1.0, pull_strength: 40.0, damping: 0.0 };
        let anchor = Vec2::new(0.0, 8.0);
        // Start off-axis with tangential velocity so it orbits rather than bobs.
        let mut w = hero_world(h, integ, Vec2::new(4.0, 4.0));
        w.hero.vel = Vec2::new(3.0, 0.0);
        w.tongues[0] = Some(fixed_tongue(h, anchor));
        let e0 = w.energy();
        let mut blew_up = false;
        for _ in 0..3600 {
            w.step();
            if !w.hero.pos.is_finite() || w.hero.pos.length() > 1.0e6 {
                blew_up = true;
                break;
            }
        }
        let e1 = w.energy();
        let drift = (e1 - e0) / e0.abs() * 100.0;
        let verdict = if blew_up {
            "DIVERGED".to_string()
        } else if drift.abs() < 1.0 {
            "stable".to_string()
        } else {
            format!("{drift:+.1}%")
        };
        println!("{:<22} {:>12.4} {:>12.4} {:>11.2}% {:>10}", integ.name(), e0, e1, drift, verdict);
    }
}

/// 3. Stiffness stability sweep. Find the largest pull_strength each integrator
/// survives at the locked 60Hz step. Theory for a symplectic method: omega*dt < 2,
/// i.e. k < 4*m/dt^2 = 14400*m.
fn test_stiffness_limit() {
    println!("\n=== 3. Maximum stable pull_strength at 60Hz (mass = 1.0, undamped) ===");
    println!("Theory, symplectic bound: omega*dt < 2  =>  k < 4m/dt^2 = {:.0}", 4.0 / (DT * DT));
    println!();
    println!("{:<22} {:>14} {:>14}", "integrator", "max stable k", "omega*dt there");
    for integ in ALL {
        let mut last_ok = 0.0f32;
        let mut k = 1.0f32;
        while k <= 400_000.0 {
            let h = Hero { mass: 1.0, pull_strength: k, damping: 0.0 };
            let anchor = Vec2::new(0.0, 6.0);
            let mut w = hero_world(h, integ, Vec2::new(0.0, 0.0));
            w.tongues[0] = Some(fixed_tongue(h, anchor));
            let e0 = w.energy();
            let mut ok = true;
            for _ in 0..3600 {
                w.step();
                if !w.hero.pos.is_finite() || w.hero.pos.length() > 1.0e4 {
                    ok = false;
                    break;
                }
            }
            // Also require bounded energy, not merely finite coordinates.
            if ok && (w.energy() - e0).abs() / e0.abs().max(1e-6) > 10.0 {
                ok = false;
            }
            if ok {
                last_ok = k;
            } else {
                break;
            }
            k *= 1.15;
        }
        let omega_dt = (last_ok / 1.0f32).sqrt() * DT;
        println!("{:<22} {:>14.0} {:>14.3}", integ.name(), last_ok, omega_dt);
    }
}

/// 4. Long-stretch behaviour. Force grows linearly with Stretch and there is no
/// maximum Tongue length, so check what a very distant attach actually produces.
fn test_long_stretch() {
    println!("\n=== 4. Long-stretch attach, force and peak speed ===");
    let h = Hero::baseline();
    println!("k = {}, mass = {}, damping = {}", h.pull_strength, h.mass, h.damping);
    println!();
    println!("{:>10} {:>14} {:>14} {:>14} {:>10}", "distance", "initial force", "peak speed", "ticks to rest", "stable");
    for dist in [2.0f32, 5.0, 20.0, 100.0, 1000.0] {
        let anchor = Vec2::new(0.0, dist);
        let mut w = hero_world(h, Integrator::SemiImplicitEuler, Vec2::ZERO);
        w.tongues[0] = Some(fixed_tongue(h, anchor));
        let f0 = w.tongue_force(w.tongues[0].unwrap(), w.hero.pos, w.hero.vel).length();
        let mut peak = 0.0f32;
        let mut ticks_to_rest = None;
        let mut stable = true;
        for i in 0..60_000 {
            w.step();
            let s = w.hero.vel.length();
            if s > peak {
                peak = s;
            }
            if !w.hero.pos.is_finite() {
                stable = false;
                break;
            }
            if ticks_to_rest.is_none() && i > 10 && s < 0.01 {
                ticks_to_rest = Some(i);
            }
        }
        println!(
            "{:>10.0} {:>14.1} {:>14.2} {:>14} {:>10}",
            dist, f0, peak,
            ticks_to_rest.map(|t| t.to_string()).unwrap_or_else(|| ">60000".into()),
            if stable { "yes" } else { "NO" }
        );
    }
}

/// 5. Dual Tongue vector summation. ADR-0003 claims aligned pulls double and
/// opposed pulls cancel, with no constraint solver and no infeasible configuration.
fn test_dual_tongue() {
    println!("\n=== 5. Dual Tongue vector summation ===");
    let h = Hero::baseline();
    let probe = |a: Vec2, b: Option<Vec2>| -> Vec2 {
        let mut w = hero_world(h, Integrator::SemiImplicitEuler, Vec2::ZERO);
        w.tongues[0] = Some(fixed_tongue(h, a));
        w.tongues[1] = b.map(|p| fixed_tongue(h, p));
        let g = w.gravity * w.hero.mass;
        w.hero_accel(w.hero.pos, w.hero.vel) * w.hero.mass - g
    };
    let single = probe(Vec2::new(0.0, 10.0), None);
    let same = probe(Vec2::new(0.0, 10.0), Some(Vec2::new(0.0, 10.0)));
    let opposed = probe(Vec2::new(0.0, 10.0), Some(Vec2::new(0.0, -10.0)));
    let perp = probe(Vec2::new(0.0, 10.0), Some(Vec2::new(10.0, 0.0)));
    println!("one Tongue, 10 up          : {:>22}  |F| = {:.3}", fmt(single), single.length());
    println!("two, same Anchor           : {:>22}  |F| = {:.3}   ratio {:.3}x",
        fmt(same), same.length(), same.length() / single.length());
    println!("two, exactly opposed       : {:>22}  |F| = {:.3}", fmt(opposed), opposed.length());
    println!("two, perpendicular         : {:>22}  |F| = {:.3}   ratio {:.3}x, expect sqrt(2) = 1.414",
        fmt(perp), perp.length(), perp.length() / single.length());
    println!("\nInfeasibility check: anchors 2000 apart, both Tongues attached.");
    let far = probe(Vec2::new(-1000.0, 0.0), Some(Vec2::new(1000.0, 0.0)));
    println!("  resultant {} finite = {}  -> no over-constraint, nothing to solve", fmt(far), far.is_finite());
}

/// 6. The loaded-Stretch manoeuvre: travel away, attach, let the spring reverse
/// your momentum, get slung back. This is the headline mechanic.
fn test_loaded_stretch() {
    println!("\n=== 6. Loaded-Stretch slingshot ===");
    let h = Hero { mass: 1.0, pull_strength: 40.0, damping: 0.6 };
    println!("k = {}, damping = {}. Hero starts at the Anchor moving away at 8 u/s,", h.pull_strength, h.damping);
    println!("attaches at the listed distance, and we measure the return speed back past the Anchor.");
    println!();
    println!("{:>14} {:>14} {:>16} {:>12}", "attach dist", "stretch", "return speed", "gain");
    let mut baseline_speed = None;
    for dist in [1.0f32, 2.0, 4.0, 8.0, 16.0] {
        let anchor = Vec2::ZERO;
        let mut w = hero_world(h, Integrator::SemiImplicitEuler, Vec2::new(dist, 0.0));
        w.gravity = Vec2::ZERO; // isolate the spring from gravity for this measurement
        w.hero.vel = Vec2::new(8.0, 0.0); // still travelling away at the moment of attach
        w.tongues[0] = Some(fixed_tongue(h, anchor));
        let mut best = 0.0f32;
        for _ in 0..6000 {
            w.step();
            // Measure the inbound speed once it is heading back toward the anchor.
            if w.hero.vel.x < 0.0 {
                let s = w.hero.vel.length();
                if s > best {
                    best = s;
                }
            }
        }
        if baseline_speed.is_none() {
            baseline_speed = Some(best);
        }
        println!(
            "{:>14.0} {:>14.2} {:>16.2} {:>11.2}x",
            dist, (dist - REST_LENGTH).max(0.0), best, best / baseline_speed.unwrap()
        );
    }
}

/// 7. Three-body coupling: hero tongued to terrain and to an Enemy at once.
fn test_three_body() {
    println!("\n=== 7. Three-body coupling (terrain + Movable Anchor) ===");
    let h = Hero::baseline();
    let mut w = hero_world(h, Integrator::SemiImplicitEuler, Vec2::ZERO);
    w.gravity = Vec2::ZERO;
    w.enemies.push(Body::new(Vec2::new(6.0, 0.0), 2.5));
    w.tongues[0] = Some(fixed_tongue(h, Vec2::new(0.0, 12.0)));
    w.tongues[1] = Some(Tongue { anchor: Anchor::Movable(0), k: h.pull_strength, damping: h.damping });
    let p0 = w.hero.vel * w.hero.mass + w.enemies[0].vel * w.enemies[0].mass;
    println!("hero mass {:.1}, enemy mass {:.1}", w.hero.mass, w.enemies[0].mass);
    println!("{:>7} {:>22} {:>22} {:>14}", "tick", "hero pos", "enemy pos", "gap");
    for i in 0..=240 {
        if i % 60 == 0 {
            let gap = (w.enemies[0].pos - w.hero.pos).length();
            println!("{:>7} {:>22} {:>22} {:>14.3}", i, fmt(w.hero.pos), fmt(w.enemies[0].pos), gap);
        }
        w.step();
    }
    let p1 = w.hero.vel * w.hero.mass + w.enemies[0].vel * w.enemies[0].mass;
    println!("\nhero-enemy pair momentum initial {} final {}", fmt(p0), fmt(p1));
    println!("Not conserved, and correctly so: the Fixed Anchor is an external infinite-mass");
    println!("body injecting impulse into the system. Both bodies are pulled, as specified.");
}

/// 8. Damping sweep. Critical damping for a spring is c = 2*sqrt(k*m); the ratio
/// zeta = c / c_crit says how bouncy it feels. A game wants underdamped, not dead.
fn test_damping() {
    println!("\n=== 8. Damping sweep (k = 40, mass = 1, c_crit = {:.2}) ===", 2.0 * (40.0f32 * 1.0).sqrt());
    println!("Pulled taut at 6 units, released from rest, no gravity. Settle = |v| < 0.05 sustained.");
    println!();
    println!("{:>9} {:>8} {:>12} {:>14} {:>13}", "damping", "zeta", "overshoots", "settle ticks", "feel");
    let c_crit = 2.0 * (40.0f32 * 1.0).sqrt();
    for c in [0.0f32, 0.3, 0.6, 1.5, 3.0, 6.0, 12.6, 25.0] {
        let h = Hero { mass: 1.0, pull_strength: 40.0, damping: c };
        let mut w = hero_world(h, Integrator::VelocityVerlet, Vec2::new(6.0, 0.0));
        w.gravity = Vec2::ZERO;
        w.tongues[0] = Some(fixed_tongue(h, Vec2::ZERO));
        let mut overshoots = 0;
        let mut last_sign = 0.0f32;
        let mut settle = None;
        for i in 0..20_000 {
            w.step();
            let s = w.hero.vel.x.signum();
            if last_sign != 0.0 && s != last_sign {
                overshoots += 1;
            }
            last_sign = s;
            if settle.is_none() && w.hero.vel.length() < 0.05 && i > 30 {
                settle = Some(i);
            } else if w.hero.vel.length() >= 0.05 {
                settle = None;
            }
        }
        let zeta = c / c_crit;
        let feel = if zeta < 0.02 { "perpetual" } else if zeta < 0.3 { "springy" }
            else if zeta < 0.9 { "controlled" } else { "dead" };
        println!("{:>9.1} {:>8.3} {:>12} {:>14} {:>13}", c, zeta, overshoots,
            settle.map(|t| t.to_string()).unwrap_or_else(|| "never".into()), feel);
    }
}

/// 9. Force ceiling. Long-stretch attaches are numerically stable but produce
/// absurd speeds, so the cap is a gameplay requirement rather than a stability one.
fn test_force_ceiling() {
    println!("\n=== 9. Force ceiling against long-stretch attaches ===");
    println!("{:>10} {:>16} {:>16} {:>16}", "distance", "uncapped peak", "cap 400 peak", "cap 1200 peak");
    for dist in [5.0f32, 20.0, 100.0, 1000.0] {
        let mut row = vec![];
        for cap in [None, Some(400.0f32), Some(1200.0f32)] {
            let h = Hero::baseline();
            let mut w = hero_world(h, Integrator::VelocityVerlet, Vec2::ZERO);
            w.force_ceiling = cap;
            w.tongues[0] = Some(fixed_tongue(h, Vec2::new(0.0, dist)));
            let mut peak = 0.0f32;
            for _ in 0..20_000 {
                w.step();
                peak = peak.max(w.hero.vel.length());
            }
            row.push(peak);
        }
        println!("{:>10.0} {:>16.1} {:>16.1} {:>16.1}", dist, row[0], row[1], row[2]);
    }
    println!("\nA cap turns the pull into constant acceleration past the cap distance, so");
    println!("travel time grows with distance instead of speed growing without bound.");
}

fn fmt(v: Vec2) -> String {
    format!("({:>8.3}, {:>8.3})", v.x, v.y)
}

fn main() {
    println!("Tongue spring prototype, ticket #8");
    println!("ADR-0003 one-way damped spring, headless, glam {} only", "0.30");
    println!("DT = 1/60 s (ADR-0002), gravity = {:?} (moon-scale), REST_LENGTH = {REST_LENGTH}", GRAVITY.y);
    test_hang_equilibrium();
    test_energy_drift();
    test_stiffness_limit();
    test_long_stretch();
    test_dual_tongue();
    test_loaded_stretch();
    test_three_body();
    test_damping();
    test_force_ceiling();
}
