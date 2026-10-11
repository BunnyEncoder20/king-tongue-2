---
status: accepted
---

# The Tongue is a hand-rolled damped spring, not a rope constraint

The Tongue applies a one-way damped spring force along the vector between the hero and the Anchor, written by hand against `glam`, with no physics crate in the Core Sim. It is not a distance constraint and there is no maximum length. Force magnitude grows with stretch, so a long Tongue pulls harder than a short one, and a hero who moves away from an Anchor after attaching loads the spring and is then slung back toward it. The reference feel is Spider-Man's webs.

This replaces an earlier version of this ADR that specified a position-based distance constraint at a bounded radius. That framing was wrong: a rope forbids a radius but has no direction or strength to scale, so it cannot express a pull that varies with length, and it cannot express two Tongues whose pulls add and cancel as vectors.

## Why no physics crate

A reader will expect Rapier2d and may try to add it, so the reasoning is recorded. Rapier is not `glam`-only, so it breaks the engine-free boundary in ADR-0001. Its cross-platform determinism is not guaranteed, which undercuts the deterministic fixed step in ADR-0002. Its spring and rope joints are also the wrong primitives: we need a one-way spring with no push phase and a force law we tune directly. The simulation is small, being one hero body, up to two springs, low gravity, static terrain, and some Enemies.

## Consequences

Swing is **emergent**, not modelled. The spring pulls inward while tangential momentum is conserved, and under the deliberately moon-scale gravity the result is spiralling orbital arcs that read as swinging. Release exits on the tangent. Nothing in the simulation represents a pendulum.

Two Tongues are no longer a constraint problem. Forces sum as vectors, so there is no infeasible configuration, no solver iteration count, and no jitter: pulls in the same direction double, opposed pulls cancel. This supersedes the over-constraint consequence recorded in ADR-0006.

A Tongue on a Movable Anchor pulls both bodies by their mass ratio, so a hero tongued to an Enemy and to terrain at once is a genuine three-body system that must be solved as one, not as two independent pairs. A Tongue attaching to a Movable Anchor always attaches to its exact center of mass.

The rest boundary is geometry-bound rather than freely tuned: `REST_LENGTH = 2.0 * PLAYER_RADIUS`. Below it the Tongue is slack and applies nothing, and past it Hooke's law acts on the Stretch. Equilibrium hang length is therefore `REST_LENGTH + m*g/k`, so a stronger pull hangs closer to the boundary and `REST_LENGTH` is a hard floor no hero gets inside. No separate short-range rule is needed, because Hooke's law already tapers force continuously to zero at the boundary.

The numerics were measured in the Tongue prototype for ticket #8 rather than guessed. The integrator question escalated into its own decision, ADR-0007, because explicit Euler turned out to be unusable rather than merely imprecise. A force ceiling is required, but for gameplay rather than stability: force grows linearly with Stretch and there is no maximum length, so an uncapped attach at 1000 units produces a peak speed above 5000 units per second. Damping must stay underdamped, roughly `zeta` between 0.1 and 0.25, since an undamped spring never settles and a critically damped one destroys the snap the mechanic exists for.
