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

The freed Anchor slot in that last row is now the general rule rather than a special case of mutual destruction: ADR-0014 frees the slot whenever a Movable Anchor leaves the simulation, by either route, and cites this matrix as the precedent it generalises.

**The tension threshold is denominated as an acceleration and not as a force, which is where the mass differential lives.** The sentence above derives the tension from "the hero's `pull_strength`, the relative velocities, and the mass differential", and the first two are already the Tongue's own force law: ADR-0003's spring takes `pull_strength` and damps against the relative radial velocity. The third is dividing by the Anchor's own mass. Comparing a bare force against the threshold would make a heavy Enemy exactly as easy to Throw as a light one, which is the one thing the mass differential is in that sentence to prevent, so `THROW_TENSION` is in world units per second squared and the comparison is against the body's own acceleration from its Tongues.

`THROW_TENSION` is 500, which is a tuning starting point like `KICK_THRESHOLD` and not a measured figure. What fixes its order of magnitude is that **both bands have to stay reachable**. At the baseline hero's `pull_strength` of 40 and `REST_LENGTH` of 1, a mass 1 Enemy reaches 500 at a Stretch of 12.5, so an Attach inside 13.5 units merely tracks it. That keeps the tracking band the common case, which it has to be, because this matrix gives that band rules of its own and a threshold an ordinary Attach cleared would delete the band and its environmental immunity with it. What does reach the threshold is loading the Stretch, which ADR-0003 already names as the deliberate manoeuvre, grabbing something light, or holding one Enemy with both slots, since two pulls sum. It is also comfortably under the per-Tongue `DEFAULT_FORCE_CEILING` of 1200, so the state is reachable without the clamp being involved at all; that relation is a `const` assertion rather than a test, because nothing a simulation could run would make it more true.

**The promotion is read from a force and applied after integration, which the integrator forces.** ADR-0007 evaluates acceleration twice per Sim Tick, so the tension reading has to be a pure function of position and velocity and the state change it decides has to happen once, after the position and velocity steps, exactly like an impulse. The reading is therefore taken during the tick's first evaluation, parked in the per-body half-step, and spent after the integration loop.

**Weight class is mass, with a band, and heavier folds into the equal row.** The matrix says "a lesser Enemy" and "an equal weight class": a weight class is a weight, and `Toughness` is already spoken for by the three bands above. The band is a tenth, because mass arrives from level data as a float and two Enemies authored at the same weight must not fall into different rows on a last-bit difference when the row decides whether the player keeps a weapon or loses one. A target *heavier* than the projectile is a fourth case the matrix does not have; it folds into mutual destruction, because the only alternative readings are that it destroys the projectile alone or that nothing happens, and both make the risk of wielding a weapon go *down* as the target gets heavier, which inverts the whole band structure above.

**The lesser row's "minor proportional velocity damping" is proportional to the target's weight relative to the projectile's.** Proportionality needs something to be proportional to, and the target's weight is the only candidate in the collision that is not arbitrary: ploughing something a tenth of your weight should not cost what ploughing something at the edge of your own weight class does. The coefficient is a fifth, and because a target at or above the band is mutual destruction and never reaches this row, a surviving projectile keeps at least four fifths of its speed. The velocity is **scaled** rather than reduced along the contact normal, for the same reason `momentum_drain` scales the hero's: the drag then only ever shortens the vector and never turns it, and a weapon that changed direction on every body it passed through could not be aimed.

**Terrain is any contact, read off the integrator's own sweep.** "Any terrain contact ends it" is taken literally: a Thrown Enemy resting on a floor is destroyed by it, not only one that arrives at speed. The test is the contact count the swept motion already produced rather than a fresh geometry test, which makes "struck terrain" mean exactly what the integrator means by it and immune to tunnelling at any speed, since the sweep is continuous.

**The matrix is resolved before the promotion, and that ordering is a rule rather than an arrangement.** A body below the threshold when a tick's motion began keeps its environmental immunity for the whole of that motion: it slides along terrain, and it is promoted at the end of the tick, so the inverted matrix governs its next tick. Promoting first would destroy an Enemy against the very surface it was immune to while being dragged along it, which is the one outcome the immunity clause exists to forbid. The visible consequence is one tick of Thrown before the terrain row can reach it, which is correct and also means the kind change is published for at least one `Snapshot` before the body leaves.

Within the matrix, **terrain is resolved before the Enemy rows and a projectile it destroys ploughs nothing**. Terrain is the row that ends the state, and a weapon that met a wall and an Enemy on the same tick should not collect the reward on its way out.

**The state is terminal: nothing demotes a Thrown Enemy back to a target.** There is no cooling off and no route back, which is what makes a Released weapon still destroy whatever it lands among rather than becoming harmless the moment the Tongue is gone. A Thrown Enemy ends by being destroyed or by leaving the Kill Bound.

Recorded while implementing ticket #18.

### Hero death is not implemented, and that is deliberate

"Thrown, it destroys the hero outright" is **not built**. The hero's one-shot fail state, any lose condition, run structure and restart are all deferred in the map's fog at the bottom of this ADR, so there is nothing for the branch to trigger: no run to end, no screen to show, no restart to offer. The hero's `Toughness` pool is infinite, which says the same thing in the same terms everything else uses. Confirmed with the project owner on ticket #18.

What happens instead is nothing: a Thrown Enemy and the hero pass through each other. **Not a Kick**, which was the tempting alternative and is the wrong one. This ADR makes state the arbiter of what an Enemy striking the hero does and assigns the Kick to the *tracking* state, so implementing a Kick for the Thrown state would contradict the ADR rather than defer to it, and it would do so invisibly, by making a case that is meant to be fatal feel survivable.

The seam is the `BodyKind::Enemy` filter in `resolve_impacts` and the matching one in `resolve_thrown`. A Thrown body whose swept pass meets the hero is the condition, and both filters carry a comment saying so, so whoever implements the fail state has one place to look and nothing to undo.

### The struck route into Thrown is not implemented, and it cannot be as written

The band table above says that an impact above the Kick threshold and below `toughness` spends momentum against the pool **and the Enemy enters Thrown**, and the GLOSSARY defines Thrown as reached by being "pulled hard enough by a Tongue **or struck hard enough** to be weaponized". Only the pulled route is built. The struck route is left out because, taken together with the rest of this ADR, it is self-defeating rather than merely unimplemented, and resolving that needs a decision rather than an implementation choice.

Two clauses of this ADR contradict it directly. The momentum drain section exists so "the pool lets a weak hero chip a heavy one over several passes", and the terrain row destroys a Thrown Enemy on any contact. An Enemy standing on a floor that entered Thrown on its first Slam would be destroyed by the floor it was standing on, on the next tick, whatever its `Toughness` was. One Slam would kill every grounded Enemy in the game and the `Toughness` pool, this ADR's central combat idea, would never be spent at all. ADR-0015 sharpens the same point from the other side: a Thrown Enemy is not a target, so a chipped Enemy would also be immune to the second and third passes the chipping rule is about.

The intent behind the clause is clearly real and worth keeping: Slamming an airborne Enemy into another one is exactly the chaining this combat model is built for. But every way of having both needs a decision nobody has made. Possibilities, none of them chosen here: only a Slam that leaves the Enemy airborne promotes it; a struck Enemy enters Thrown only once its pool is nearly spent; or the terrain row distinguishes a weapon the player is still holding from one that is merely tumbling. Each is a different game, and picking one from inside a ticket would be inventing the answer rather than recording it.

Noted while implementing ticket #18.

## Despawn is world-space

A destroyed or Thrown Enemy leaving the level's Kill Bound is removed. The bound is declared in level data as a world-space rectangle and is never a screen-space test, because screen dimensions vary across the device floor and a screen-space test would make outcomes device-dependent, voiding the determinism ADR-0002 exists to provide.

Two further questions this section left open, whether a Thrown Enemy can be grappled and whether its kills advance the Combo, are both answered no by ADR-0015.

## Out of scope

Announcer voice lines and the score multiplier's derivation are Shell concerns and outside this map. Triple-tap special moves are out. The hero's one-shot fail state, straight-line rockets, heat-seeking missiles, the armored grappable Enemy that spikes on attach, and the hero archetype roster are all deferred in the map's fog until the acceptance bar is met.
