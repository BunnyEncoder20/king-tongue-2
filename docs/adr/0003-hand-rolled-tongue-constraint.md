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

Three numerical details are left to the Tongue prototype and are not fixed here, because they are tuning rather than architecture: whether the spring needs a force ceiling to stay stable at long stretch under a 60Hz step, how much damping is needed to stop perpetual in-and-out oscillation, and whether an explicit integrator is adequate or a semi-implicit one is required, since stiff springs are the classic instability for explicit Euler at a fixed step.
