---
status: accepted
---

# Fixed 60Hz simulation with render interpolation

The Core Sim advances in fixed 60Hz Sim Ticks driven by an accumulator, and the Shell interpolates between the two most recent simulation states when it draws. Stepping grapple and pendulum physics on a variable frame delta makes the Swing both unstable and frame-rate dependent, and Android frame pacing is genuinely variable: 60Hz against 120Hz panels, thermal throttling, and long stalls on app resume. A fixed step means the Swing feels identical on every device, and it makes the simulation deterministic, which is what allows it to be tested without a renderer at all.

## Consequences

The Shell must tolerate a frame in which the accumulator produced zero Sim Ticks, and a frame in which it produced several. Interpolation applies only to continuous quantities; interpolating a discrete event such as an Attach or a Slam impulse produces visible artifacts, so the contract has to say explicitly what is not interpolated. A long stall, most likely on app resume, can leave the accumulator holding a large debt, so it needs a clamp rather than an unbounded catch-up loop that would spike a frame or spiral. The exact shape of the state the Shell reads is a separate open decision.
