---
status: accepted
---

# The Core Sim integrates with Velocity Verlet

The Core Sim advances bodies with Velocity Verlet. This was measured rather than assumed, in the Tongue prototype for ticket #8, because ADR-0003's spring is the kind of force that decides an integrator for you.

Explicit Euler, which is what anyone writes first, is not merely less accurate here: it is unusable. It is unconditionally unstable on a harmonic oscillator, because it advances position using the pre-update velocity and so injects energy every tick no matter how small the step. Over 3600 ticks of undamped orbital Swing it grew total mechanical energy by roughly 5.5e12 percent. Its maximum stable `pull_strength` at the locked 60Hz step is 3, while the baseline hero needs 40, so it cannot simulate even the weakest intended hero.

## Considered Options

Measured at mass 1 and the fixed 60Hz step from ADR-0002:

| integrator | energy drift over 60 s | max stable `pull_strength` |
|---|---|---|
| Explicit Euler | +5.5e12 percent, diverged | 3 |
| Semi-implicit Euler | -3.27 percent | 6,668 |
| Velocity Verlet | -0.21 percent | 13,411 |

Semi-implicit Euler is genuinely viable and cheaper, costing one acceleration evaluation per tick against Verlet's two. Verlet was chosen for twice the stiffness headroom and fifteen times less energy drift, which matters because drift in a deterministic simulation is not just a visual artifact: it is divergence between a recorded trace and a replayed one.

## Consequences

ADR-0002 stands unchanged. The fixed 60Hz step was never the problem, and the theoretical symplectic stability bound is `omega * dt < 2`, that is `pull_strength < 4 * mass / dt^2`, which is 14,400 at mass 1. Velocity Verlet reaches 93 percent of that bound in practice. Against a baseline `pull_strength` of 40 that leaves roughly 335x headroom, and the bound rises with mass, so heavier heroes tolerate stiffer springs.

The cost is two acceleration evaluations per Sim Tick instead of one, which also means force accumulation must be a pure function of position and velocity with no side effects, since it is called twice per tick. Any later force that mutates state during evaluation will break silently under this integrator.

This bound is a real design constraint, not a formality. If a future hero or mechanic wants a `pull_strength` approaching 14,000 at mass 1, the fixed step has to be revisited rather than the integrator, and that reopens ADR-0002.
