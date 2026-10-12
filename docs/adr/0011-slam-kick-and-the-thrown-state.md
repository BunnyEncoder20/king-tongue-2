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

The freed Anchor slot in that last row is now the general rule rather than a special case of mutual destruction: ADR-0014 frees the slot whenever a Movable Anchor leaves the simulation, by either route, and cites this matrix as the precedent it generalises.

## Despawn is world-space

A destroyed or Thrown Enemy leaving the level's Kill Bound is removed. The bound is declared in level data as a world-space rectangle and is never a screen-space test, because screen dimensions vary across the device floor and a screen-space test would make outcomes device-dependent, voiding the determinism ADR-0002 exists to provide.

## Out of scope

Announcer voice lines and the score multiplier's derivation are Shell concerns and outside this map. Triple-tap special moves are out. The hero's one-shot fail state, straight-line rockets, heat-seeking missiles, the armored grappable Enemy that spikes on attach, and the hero archetype roster are all deferred in the map's fog until the acceptance bar is met.
