---
status: accepted
---

# The Core Sim to Shell frame contract

The Shell owns wall-clock time and the Core Sim owns ticks. Each frame the Shell feeds elapsed time to an `Accumulator`, runs the Sim Ticks it is told to, and draws an interpolated blend of the two most recent `Snapshot`s. The accumulator and the interpolation both live in the Core Sim rather than the Shell, because they are deterministic logic with no platform content and belong where they can be tested headless.

A `Snapshot` carries only `glam` primitives: a tick number, a list of bodies (stable id, kind, position, facing) and a list of attached Tongue lines. Nothing engine-shaped crosses the boundary, so ADR-0001 holds.

## The accumulator is clamped, and the excess is dropped

`MAX_CATCHUP_TICKS` is 5. Debt beyond that is discarded rather than deferred.

Without the clamp a stalled frame tries to repay its whole debt at once, which stalls the next frame harder, which grows the debt again. The specific case that forces it on this project is not hypothetical: Macroquad computes frame delta inside `draw()`, and `draw()` is skipped while the app is suspended, so the first post-resume delta equals the entire suspend duration. Three minutes backgrounded owes roughly 10,800 ticks in one frame. There is a test asserting that exact scenario yields 5 ticks and that the following frame is immediately back to one.

Dropping rather than deferring means simulated time falls behind the wall clock after a stall. That trade is deliberate: determinism here is a property of the tick sequence, not of agreement with real time, and a game that silently slows for a moment is better than one that freezes. The accumulator keeps the sub-tick remainder so `alpha` stays smooth across the discontinuity, and counts what it threw away for diagnostics.

The accumulator also rejects non-finite, negative, and zero deltas, because a poisoned accumulator is unrecoverable and clocks do misbehave on the first frame.

## Correspondence, which is the substance of the contract

Blending `previous` against `current` is only meaningful for things present in both, so matching is by stable `EntityId` and never by slice position. Indices shift the moment anything spawns or despawns, and index matching would blend a surviving hero against a dead Enemy's coordinates and slide it across the level. There is a regression test for precisely that.

Four rules follow:

- A body in both snapshots interpolates.
- A body only in `current` spawned this tick and is drawn at its current position with no blending, since it has no previous position and would otherwise slide in from somewhere arbitrary.
- A body only in `previous` despawned and is not drawn at all.
- A body whose `kind` changed, an Enemy becoming Thrown, snaps rather than blends, because that is a discrete event.

Tongues interpolate only while the same slot is attached to the same anchor in both snapshots. A newly fired Tongue snaps, otherwise it is drawn stretching out of nowhere, and a re-fired one snaps, otherwise it sweeps across the level. ADR-0002 required that discrete events be excluded from interpolation, and this is where that is honoured.

**A `TongueLine` carries the whole `Anchor`, and identifying a Fixed Anchor by a nullable body id is a defect.** The first implementation carried an `Option<EntityId>`, which is `None` for every Fixed Anchor, so two different terrain points compared equal and the predicate above could not tell them apart. That was harmless for exactly as long as a slot held one Tongue for a whole run, which is why ticket #12 shipped it. Release made it reachable: a second tap Releases, a third re-fires at different terrain, and the Tongue blended a room's width across the level. The `Anchor` enum already distinguishes both cases correctly, comparing `Fixed` by position and `Movable` by identity.

Comparing the line's far endpoint instead would be wrong, and wrong in the opposite direction: that point moves every tick for a Tongue attached to a moving Enemy, so it would snap on every single frame and judder for the whole attachment. The two cases genuinely need different notions of sameness, position for terrain and identity for a body, and carrying the `Anchor` is what supplies both. Discovered while implementing ticket #13, and covered by a regression test that re-targets a slot between two terrain Anchors.

The reachability argument generalises past Release. The Shell draws more frames than there are Sim Ticks, so it calls `apply_touches` more than once between one tick and the next. A frame that Releases and a frame that re-fires can therefore both fall inside a single tick, and then no `Snapshot` ever shows the slot empty: the slot goes straight from one Anchor to another and the empty-slot snap never gets a chance to save it.

## Facing is a unit vector, not an angle

Linear interpolation of angles is wrong across the wrap boundary: blending 359 degrees toward 1 degree sweeps backwards through 180. The contract therefore carries facing as a unit vector and blends by lerp-then-renormalize, with a guard for the near-opposite case, which would otherwise normalize a near-zero vector into NaN. The Shell draws the vector directly and never reconstructs an angle through `atan2`.

## A new entity cannot silently fail to render

This was the ticket's original question and it is answered mechanically. `BodyKind` is a plain enum and the Shell's `draw_body` matches it exhaustively with no catch-all arm, so adding a variant is a compile error. Verified by adding a `Projectile` variant, which produced `error[E0004]: non-exhaustive patterns: BodyKind::Projectile not covered`. The Shell additionally sets `clippy::wildcard_enum_match_arm` and CI runs clippy with `-D warnings`, so silencing the break with `_ =>` is also a build failure.

## Consequences

Two snapshots plus a reused interpolation buffer are held at all times. `interpolate_into` writes into a caller-owned buffer so rendering allocates nothing per frame, and there is a test asserting the buffer's capacity does not grow across a hundred frames. Body lookup is currently a linear scan, which is correct and fine at the entity counts the acceptance bar implies; if the Enemy count grows into the hundreds this becomes the obvious place to add an index.

Snapshot construction is a copy per tick. At this simulation's size that is irrelevant, and it buys the Shell a stable view that cannot tear while being read.
