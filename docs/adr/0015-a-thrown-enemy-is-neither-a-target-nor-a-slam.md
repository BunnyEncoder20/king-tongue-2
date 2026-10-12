---
status: accepted
---

# A Thrown Enemy is neither a target nor a Slam

Two questions were left open deliberately rather than guessed at. ADR-0011 defines the Thrown state and its inverted collision matrix without saying whether a Thrown Enemy can be grappled, and it defines the Combo as "an unbroken run of Slams" without saying whether a Thrown Enemy's kill is one. Nothing could reach either question until ticket #18 produced the state: ticket #17 filtered the target set to `BodyKind::Enemy` and recorded in a comment that it was refusing to guess, and ticket #19 built the Combo to count hits out of `resolve_impacts` without a second source of kills existing.

Both are now decided, in the same direction and for the same underlying reason: **the Thrown state is a thing the player has set loose, not a thing the player is holding.** Everything that treats it as still under the player's control makes the state cheaper than ADR-0011 wants it to be.

## A Thrown Enemy is not a target

A tap cannot Attach to a Thrown Enemy. It is absent from the target set `World::fire` builds, so ADR-0012's Enemy Assist Arc never prefers it and a ray that meets it passes straight through to the terrain behind it, bit-identical to what a pure terrain cast would have found.

This forbids re-targeting and nothing else. **A Tongue already Attached to an Enemy that becomes Thrown keeps holding it**, which is not a concession but the mechanic: ADR-0011's "if the player does not Release, it barrels back toward the hero" is only reachable while the Tongue that Threw it is still Attached.

### Why

The risk is the entire point of the state. ADR-0011 calls it "deliberately high risk and high reward: any terrain contact ends it, so using an Enemy as a weapon demands real control rather than being free damage." A second Tongue on a weapon in flight is the one input that dissolves that: two opposed pulls cancel under ADR-0003, so a player with a free slot could park a Thrown Enemy in mid-air, hold it clear of every surface, and steer it at leisure. The risk would be optional, and an optional risk is not a risk.

Re-grappling after a Release is the same problem by a slower route. ADR-0011 makes Release the commitment: the weapon leaves on a ballistic arc and whatever happens next is the consequence of the arc the player chose. A tap that catches it again mid-flight makes the arc an adjustable suggestion and the commitment nothing.

There is a feel cost too, and it is on the same side. The Assist Arc exists to prefer Enemies over the terrain behind them, so a Thrown Enemy crossing the player's aim would steal taps the player meant for a wall, at exactly the moment the player is trying to get a Tongue onto something stable. A tap stolen by a weapon in flight is worse than a tap that misses.

### The alternative, and why it loses

**Keep a Thrown Enemy grappable, as a skill ceiling.** Catching a weapon mid-flight and re-aiming it is a genuinely attractive high-skill move, and it is the better-feeling of the two options in isolation. It loses on the ceiling it raises: every bit of control it adds is control over the thing ADR-0011 made uncontrollable. The state is called Thrown because the momentum is no longer the Enemy's **and no longer the player's**, and the clause "it barrels back toward the hero" exists precisely because the player cannot steer it any more. A rule that hands the steering back contradicts the state's definition rather than extending it.

If it ever comes back, it comes back as its own mechanic with its own cost, not as the absence of this filter. The armored grappable Enemy already deferred in ADR-0011's fog is the shape that would take.

## A Thrown Enemy's kill is not a Slam

A Thrown Enemy destroying another Enemy does not advance the Combo. The count still comes from `resolve_impacts` alone, and nothing in `resolve_thrown` touches it.

### Why

The GLOSSARY defines a Slam as "the outcome of **the player colliding** with an Enemy while carrying momentum above the Kick threshold", and ADR-0011 defines the Combo as an unbroken run of Slams. A Thrown kill is neither: the player caused it, but the player did not collide with anything and no momentum of the player's was spent on it.

That is the decisive part rather than a technicality. The Combo measures **the player's own momentum staying alive**: ADR-0011 makes the expiry window a drought timer on hits and breaks the run when the hero comes to rest, because what the count is really tracking is a hero that has not had to stop. A Thrown kill costs the hero nothing, happens at whatever distance the weapon has reached, and can land while the hero is standing still. Counting it would reward the opposite of what the Combo exists to reward.

The window makes it concrete. A weapon ploughing a row of five lesser Enemies inside one Sim Tick would add five hits at once and crush the window to its 36 tick floor, while the hero might be nowhere near any of it and about to lose the run to the rest predicate anyway. The player would watch a long Combo appear and die for reasons they had no way to read.

And the reward is already paid. The Thrown state clears Enemies without spending hero momentum, which is its own distinct pay-off; it does not need the Combo's as well, and giving it both makes the Thrown route strictly better than Slamming rather than different from it.

### The alternatives, and why they lose

**Count it as a Slam.** One line in `resolve_thrown`, and it reads naturally as "chaining". It loses on the definition above, and it would need the GLOSSARY's definition of Slam rewritten to drop the collision, which would then also have to say what else counts. Two definitions of a hit is how a count becomes a score, and ADR-0011 is explicit that the score lives in the Shell.

**Count it as something else, a second number beside the Combo.** Rejected as premature. There is nothing to spend a second count on: the multiplier and its cap are the Shell's, and no scoring system exists to receive it. Inventing a channel for a number nobody reads would put a field in ADR-0009's `Snapshot` that the Shell has no use for.

## Consequences

The conservative direction is deliberate on both counts. Wiring a Thrown kill into the Combo later is one line and arrives as a buff to a number players can see; taking it away later is a nerf to a number players have learned. Likewise, making a Thrown Enemy grappable later adds a move, while removing grappability later would break whatever the player had built on it.

Two Thrown Enemies pass through each other and nothing happens. That falls out of the first decision rather than being a separate one: the matrix's rows are terrain, a lesser Enemy and an equal weight class, and a Thrown Enemy is not an Enemy target. Two weapons in flight meeting is a case ADR-0011 does not have, and the honest answer for a case it does not have is nothing.

Decided while implementing ticket #18, which is the ticket that first produced the state and so the first one able to answer either question.
