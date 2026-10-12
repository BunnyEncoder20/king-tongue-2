---
status: accepted
---

# Slam, Kick, Combo, and the Thrown state

Combat is driven entirely by momentum. There is no health, no damage number, and no attack input: the player's velocity is the weapon, and an Enemy's resistance is denominated in the same units as the thing hitting it. This ADR exists because that is unusual enough that a reader would otherwise try to add a hit-point pool, and because the Thrown state's collision rules are non-obvious and easy to simplify incorrectly.

Collision is the player's bounding circle against an Enemy's bounding box. Attachment is not required at impact, so momentum carried over from an earlier Swing still counts.

## Impact is gated on the normal component, not total speed

The gate is `|m * v · n|`, the momentum component along the collision normal, never total momentum magnitude. Using total magnitude means a fast tangential pass that barely grazes an Enemy registers as a full hit, which both feels wrong and makes targeting tuning fight combat tuning.

## Three bands against one Toughness value

Each Enemy carries a single `toughness`, in momentum units. The impact's normal momentum is measured against it:

| band | outcome |
|---|---|
| below the Kick threshold | **Kick**: fixed minimal damage plus an elastic repel impulse that restores distance between the bodies |
| above Kick, below `toughness` | momentum is spent against the `toughness` pool, and the Enemy enters **Thrown** |
| at or above `toughness` | the Enemy is destroyed outright, the one-shot case |

One concept rather than two. A separate hit-point notion was rejected: it would need its own units, its own tuning, and a conversion to momentum at every impact. A pure threshold model with no pool was also rejected, because it locks a light hero out of ever harming a heavy Enemy; the pool lets a weak hero chip a heavy one over several passes.

## Momentum drain is proportional, capped, and eventually zero

The player loses momentum in proportion to the `toughness` actually consumed, clamped so one impact can never drain more than 50 percent of the incoming momentum vector. Above a high momentum ceiling the drain is zero: direction and speed are preserved and the hero passes straight through Enemies.

A flat percentage was rejected. It makes destroying a trivial Enemy cost exactly as much as destroying a heavy one, which flattens chaining from a skill into a tuning constant.

## Combo

An uncapped hit count held in the Core Sim, maintained while airborne and persisting across Release and re-attach. It is a count, not a score: the score multiplier and its cap are presentation and live in the Shell.

Expiry is a decaying countdown window that tightens as hits mount and settles at a hard floor. The countdown governs airborne droughts only. The Combo breaks when the hero **comes to rest** on a surface, not on mere ground contact, so a bounce preserves it and a standstill ends it. The countdown freezes entirely during wave transitions and resumes when the first Enemy of the new wave becomes active, which presumes a wave concept that does not exist yet.

This rule is explicitly provisional and expected to be retuned against real gameplay.

**"Comes to rest" is a speed threshold sustained in terrain contact: under 0.5 world units per second for 6 consecutive Sim Ticks.** The phrase was undefined everywhere it appeared, and ticket #19 had to choose a predicate to implement one. Half a hero radius per second sits far above the float residue of a resting contact, which is a few thousandths of a unit per second, and far below anything a Swing, a fall, or a bounce travels at, so the band between the two is wide and nothing has to be measured precisely for the rule to read correctly. Deliberately the most obvious predicate available rather than the best one, because the whole rule is provisional: both numbers are one constant each, and the tuning they will get is a different value in the same shape.

**Sustaining it is the half that is load-bearing, and an instantaneous version is a real defect rather than a simplification.** Terrain contact cancels the component of motion heading into the surface, so a hero arriving nearly straight down has almost no speed at all for exactly the tick it lands, while being thrown straight back up by a loaded Tongue. On a one-tick predicate that reads as a standstill, and the run dies on the player's best landing. A tenth of a second is long enough that a bounce cannot reach it and short enough to stay under a sixth of the shortest window, so it never competes with the countdown.

The predicate reads terrain and not an Enemy. ADR-0011 breaks the Combo on coming to rest **on a surface**, and an Enemy is a target rather than a surface: coming to rest against one is already the Kick band's business, and the hero drifting to a halt on an Enemy it cannot hurt should not be a second way to lose a run.

**Both bands above the Kick threshold count as hits.** The GLOSSARY defines a Slam as a collision carrying momentum above the Kick threshold, which makes spending momentum against a `toughness` pool and emptying it outright the same hit landed against different Enemies. A Kick is the band that explicitly is not a Slam. The count is of Slams and not of impacts, and more than one can land in a single Sim Tick, because a hero past the pass-through ceiling crosses two Enemy boxes inside one tick.

The window is 120 Sim Ticks at one hit, tightening by 6 per hit to a floor of 36: two seconds down to six tenths, which is roughly the time a Release and a re-Attach take at the baseline hero's pull strength. The floor binds from 15 hits on. A Sim Tick that lands a Slam is not counted as drought, so the tolerated gap between two hits is exactly the window rather than one tick less than it.

"The countdown governs airborne droughts only" is read as saying what the countdown is not, rather than as a condition on when it runs: it is the rule for gaps between hits, and it is not the rule for touching the ground, which the rest predicate owns. The countdown therefore keeps running while the hero is in contact with terrain and moving. The alternative reading pauses it during contact, and that is wrong in a world with no friction: a hero skidding along a floor just above the rest threshold would hold a Combo open indefinitely without hitting anything.

**The wave freeze is not implemented, and that is deliberate.** There is no wave system: the spec defers it in the map's fog, and the paragraph above already notes that this rule presumes a concept that does not exist. A freeze implemented now would have nothing to start it and nothing to end it, so building it would mean inventing the wave system from the Combo's side. The seam is the Core Sim's single per-tick Combo entry point, where a wave transition arrives as one more reason the countdown declines to advance, beside the case where no Combo is live.

The Combo count is published in the `Snapshot` and carried through interpolation whole rather than blended, because a count has no value between two hits and ADR-0002 keeps discrete events out of interpolation. That is the whole of what the Shell gets: ADR-0009's snapshot is the only thing the Shell reads, and the multiplier derived from the count stays there.

Recorded while implementing ticket #19.

## The Thrown state

A Tongue attached to an Enemy computes a tension force from the hero's `pull_strength`, the relative velocities, and the mass differential. Below the tension threshold the pair tracks toward each other by mass ratio, and the Enemy has environmental immunity: sliding into terrain does nothing but bounding-box slide physics.

Above the threshold the Enemy becomes Thrown: a weaponized projectile yanked along the resultant of the hero's arc. Releasing the Tongue slings it into a ballistic arc on its residual velocity. If the player does not Release, it barrels back toward the hero.

State is the arbiter of what happens when an Enemy strikes the hero. In the tracking state it is a Kick. Thrown, it destroys the hero outright, whether or not the Tongue is still attached.

While Thrown, its collision rules invert completely:

| target | outcome |
|---|---|
| static terrain (infinite mass) | the Thrown Enemy is destroyed instantly |
| a lesser Enemy | target destroyed; the projectile keeps its link and chain state, taking minor proportional velocity damping |
| an equal weight class | mutual destruction, both vaporized, the player's Anchor slot freed |

Deliberately high risk and high reward: any terrain contact ends it, so using an Enemy as a weapon demands real control rather than being free damage.

## Despawn is world-space

A destroyed or Thrown Enemy leaving the level's Kill Bound is removed. The bound is declared in level data as a world-space rectangle and is never a screen-space test, because screen dimensions vary across the device floor and a screen-space test would make outcomes device-dependent, voiding the determinism ADR-0002 exists to provide.

## Out of scope

Announcer voice lines and the score multiplier's derivation are Shell concerns and outside this map. Triple-tap special moves are out. The hero's one-shot fail state, straight-line rockets, heat-seeking missiles, the armored grappable Enemy that spikes on attach, and the hero archetype roster are all deferred in the map's fog until the acceptance bar is met.
