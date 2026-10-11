---
status: accepted
---

# Hand-rolled Tongue constraint instead of a physics crate

The Tongue constraint, gravity, and collision are written by hand against `glam`, with no physics crate in the Core Sim. A reader will reasonably expect Rapier2d here and may try to "fix" this by adding it, so the reasoning is recorded deliberately: Rapier is not `glam`-only, so it breaks the engine-free boundary in ADR-0001; its cross-platform determinism is not guaranteed, which undercuts the deterministic fixed step in ADR-0002; and a library distance or rope joint gives a spring where a grapple wants a taut radius, which is the usual reason grapple games feel mushy. The simulation is also genuinely small: one player point mass, one constraint, gravity, a handful of static bodies, and some Enemies.

## Consequences

Amended by ADR-0006: the player can hold two Tongues at once, so the simulation solves two rope constraints rather than one, and the taut-taut case is over-constrained. The decision not to use a physics crate still stands, but the solver is harder than this ADR originally assumed.

We own the stability and the tuning, including whether one constraint iteration per Sim Tick is enough at 60Hz. We get exact determinism and headless tests in exchange. This ADR fixes the decision not to use a physics crate; it does not fix the constraint's formulation (hard radius clamp, position-based distance constraint, or stiff spring), which the Tongue prototype resolves. If the simulation later grows to need general rigid-body dynamics, stacking, or arbitrary convex collision, this decision should be revisited rather than worked around, because hand-rolling that is a different scale of problem entirely.
