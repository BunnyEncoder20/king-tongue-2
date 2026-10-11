# Tongue spring prototype: stability and feel findings

Resolves wayfinder ticket #8. Headless, renderless, `glam 0.30.10` as the only dependency, per the crate isolation rule. No Macroquad, no engine types, no window.

Run it with `cargo run --release`. Every number below is reproducible from `src/main.rs`.

## Verdict

The one-way damped spring in ADR-0003 works, and the rest-boundary model is correct. **ADR-0002's fixed 60Hz step survives intact.** One new decision is required: the integrator. Explicit Euler is unusable, not marginal.

## 1. The rest boundary is exactly right

`REST_LENGTH = 2.0 * PLAYER_RADIUS` with Hooke's law past that boundary reproduces the hanging behaviour analytically, to within float noise.

| hero | k | mass | predicted hang | measured | error |
|---|---|---|---|---|---|
| light, weak | 15 | 0.7 | 1.07560 | 1.07560 | 1.6e-6 |
| baseline | 40 | 1.0 | 1.04050 | 1.04050 | 1.3e-6 |
| heavy, strong | 120 | 1.6 | 1.02160 | 1.02160 | 1.2e-6 |
| strongest | 400 | 1.4 | 1.00567 | 1.00567 | 6.0e-7 |

Equilibrium hang length is `REST_LENGTH + m*g/k`. A stronger pull hangs closer to the boundary and the strongest hero sits 0.57 percent past it, which is the "almost zero stretch" behaviour that was asked for. `REST_LENGTH` is a hard floor: no hero hangs closer than `2 * PLAYER_RADIUS` even as `k` tends to infinity. Weaker heroes hang visibly lower, which gives the weight classes a readable silhouette for free.

Zone 3 of the three-zone model needs no special case. Hooke's law already tapers force continuously to zero at the boundary, so deceleration near the anchor is smooth without a separate short-range rule. Adding one would be dead code.

## 2. Explicit Euler is unusable, by a factor of roughly 4500

Undamped orbital swing, 3600 ticks (60 simulated seconds):

| integrator | E initial | E final | drift | verdict |
|---|---|---|---|---|
| Explicit Euler | 444.71 | 2.43e13 | +5.47e12 percent | diverged |
| Semi-implicit Euler | 444.71 | 430.18 | -3.27 percent | stable |
| Velocity Verlet | 444.71 | 443.78 | -0.21 percent | stable |

This is not a tuning problem. Forward Euler is unconditionally unstable on a harmonic oscillator: it advances position on the stale velocity, which injects energy every tick regardless of how small the step is. The maximum stable stiffness sweep makes the scale of it concrete:

| integrator | max stable `pull_strength` | omega*dt at that k |
|---|---|---|
| Explicit Euler | 3 | 0.027 |
| Semi-implicit Euler | 6,668 | 1.361 |
| Velocity Verlet | 13,411 | 1.930 |

The theoretical symplectic bound is `omega*dt < 2`, that is `k < 4m/dt^2 = 14,400` at mass 1. Velocity Verlet reaches 93 percent of it. Explicit Euler tops out at `k = 3`, while the baseline hero needs `k = 40`, so it cannot simulate even the weakest intended hero.

The bound scales with mass (`k < 4m/dt^2`), so heavier heroes tolerate stiffer springs. Against a baseline of 40, Velocity Verlet leaves roughly 335x headroom. The design space is in no danger.

## 3. Dual Tongues sum as vectors, with nothing to solve

Measured spring force on the hero, gravity subtracted:

| configuration | resultant | magnitude | ratio |
|---|---|---|---|
| one Tongue, 10 up | (0.000, 360.000) | 360.000 | 1.000x |
| two on the same Anchor | (0.000, 720.000) | 720.000 | 2.000x |
| two exactly opposed | (0.000, 0.000) | 0.000 | cancels |
| two perpendicular | (360.000, 360.000) | 509.117 | 1.414x |

Exactly as ADR-0003 claims. With the two Anchors 2000 units apart and both Tongues attached, the resultant is finite and there is no violated constraint, because no constraint exists. The over-constraint warning originally recorded in ADR-0006 is confirmed withdrawn: the dual-Tongue mechanic costs nothing in solver complexity.

## 4. The loaded-Stretch slingshot works, and it is strong

Hero travelling away from the Anchor at 8 units per second attaches at the listed distance, gravity excluded to isolate the spring:

| attach distance | Stretch | return speed | gain |
|---|---|---|---|
| 1 | 0.00 | 6.96 | 1.00x |
| 2 | 1.00 | 9.07 | 1.30x |
| 4 | 3.00 | 18.81 | 2.70x |
| 8 | 7.00 | 41.60 | 5.98x |
| 16 | 15.00 | 88.48 | 12.71x |

Deliberately loading a long Stretch returns nearly 13 times the speed of a short attach. The manoeuvre is real, it rewards planning, and it is the skill ceiling the control scheme was designed around.

## 5. A force ceiling is needed, for gameplay rather than stability

Long attaches stay numerically stable at every distance tested, up to 1000 units. They are not playable:

| distance | initial force | peak speed, uncapped | with cap 400 | with cap 1200 |
|---|---|---|---|---|
| 5 | 160 | 20.1 | 20.1 | 20.1 |
| 20 | 760 | 96.3 | 89.1 | 96.3 |
| 100 | 3,960 | 502.9 | 254.9 | 395.0 |
| 1000 | 39,960 | 5,076.6 | 870.6 | 1,477.5 |

5,077 units per second crosses any plausible level inside a frame. A ceiling turns the pull into constant acceleration past the cap distance, so travel time grows with distance instead of speed growing without bound. That is also exactly the stated intent that "distance costs time, not reach".

A cap of 1200 leaves everything at 20 units and below completely untouched, so close and mid-range feel is unaffected. A cap of 400 starts flattening mid-range pulls and is probably too aggressive.

## 6. Damping: aim for underdamped

Critical damping at `k = 40`, mass 1 is `c_crit = 12.65`. Pulled taut at 6 units, released from rest:

| damping | zeta | overshoots | settle ticks | feel |
|---|---|---|---|---|
| 0.0 | 0.000 | 595 | never | perpetual |
| 0.3 | 0.024 | 79 | never | springy |
| 0.6 | 0.047 | 45 | 18,971 | springy |
| 1.5 | 0.119 | 21 | 8,552 | springy |
| 3.0 | 0.237 | 12 | 3,987 | springy |
| 6.0 | 0.474 | 7 | 1,336 | controlled |
| 12.6 | 0.996 | 4 | 835 | dead |
| 25.0 | 1.976 | 3 | 466 | dead |

Undamped never settles, which is unplayable. Anything at or above critical damping kills the web snap entirely. The usable band is roughly `zeta` between 0.1 and 0.25, which is `damping` 1.5 to 3.0 at these values.

## 7. Three-body coupling behaves

A hero tongued to terrain and to a 2.5 mass Enemy at once pulls both bodies. The gap between them oscillates 6.00, 5.17, 2.69, 2.85, 6.00 over 240 ticks while the whole pair is dragged toward the Fixed Anchor. Hero and Enemy pair momentum is not conserved, correctly, because the Fixed Anchor is an external infinite-mass body injecting impulse into the system.

## Recommendations

1. **Velocity Verlet.** Twice the stiffness headroom of semi-implicit Euler and 15 times less energy drift, for one extra acceleration evaluation per Sim Tick. Needs its own ADR.
2. **Keep ADR-0002 exactly as written.** The fixed 60Hz step is not the problem and does not need revisiting.
3. **Force ceiling of 1200**, as a tunable. Gameplay requirement, not a stability one.
4. **Default damping 3.0**, `zeta` 0.237, with per-hero variation inside the 1.5 to 3.0 band.
5. **Keep `REST_LENGTH = 2.0 * PLAYER_RADIUS`** and implement no separate short-range rule, because Hooke's law already tapers correctly.

## Honest limits

Nothing here measures feel. Every number is a stability, energy, or kinematics measurement, and "springy" against "dead" in the damping table is inferred from overshoot counts and settle times, not from playing it. The recommended constants are a defensible starting point for tuning on a device, not final values. Nothing has been run on Android, and no renderer was involved at any point.
