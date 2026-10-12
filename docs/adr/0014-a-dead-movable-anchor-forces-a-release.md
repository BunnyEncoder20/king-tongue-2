---
status: accepted
---

# A Tongue whose Movable Anchor leaves the simulation forces a Release

When the Enemy a Tongue is Attached to stops existing, whether it was destroyed by an impact or removed by the Kill Bound, that Tongue's slot is freed on the same Sim Tick. Nothing is spent, no impulse is applied, and the hero carries exactly the momentum it already had.

This was undecided rather than merely unimplemented. Resolving a Movable Anchor by identity already yields nothing once its body is gone, so the force and the drawn line both vanish correctly; what nothing handled was the slot, which stayed claimed. Nothing could reach the case before Tongues could Attach to Enemies at all.

## The two alternatives, and why they lose

**Leave the slot claimed until the player taps to Release it**, treating a dead Anchor like any other. Rejected, and it is the worst of the three. ADR-0006 decides what a tap does from whether the slot holds a Tongue, so the player's next tap on that slot would Release rather than fire: they would spend an input discovering that the thing they were holding had already gone, with nothing on screen to tell them, because a Tongue with no Anchor draws no line. The player would be down to one Tongue with no feedback and no way to get the second back except a wasted tap.

**Free the slot silently.** This is the same implementation as the decision above and differs only in how it is described. Calling it a Release is the more useful framing, because a Release in this codebase is *exactly* the clearing of a slot: ADR-0012 makes retraction instant and costless, so "forces a Release" and "frees the slot while the hero keeps its momentum" are one sentence, not two behaviours that have to agree.

The precedent settles it either way. ADR-0011's Thrown matrix already frees the player's Anchor slot on mutual destruction, so an Anchor that dies freeing its slot is the rule this project had chosen in one place and simply had not generalised.

## Where in the tick it happens

One sweep over the slots, after **both** ways of losing a body: the emptied `Toughness` pool in `resolve_impacts` and the Kill Bound retain. Two sweeps, one per removal, would be two places for one rule to drift, and the two removals sit at different points in the Sim Tick.

It runs last, before the `Snapshot` is written, for the same reason the Kill Bound does: a slot is judged against the body list the tick actually ended with. It is also a mutation, so it must stay unreachable from the force evaluation that ADR-0007 runs twice per tick.

## The Shell needs no new channel

A freed slot is simply absent from the `Snapshot`'s Tongue list, which is all the Shell ever reads. ADR-0009's interpolation rules already handle the rest: a slot missing from `current` is not drawn, and a later re-fire into that slot snaps rather than blending, because the slot was absent from `previous`. No event, no flag, and no branching logic in the Shell.

## Consequences

The hero can lose a Tongue without having asked to. That is a deliberate gameplay cost of grappling something destructible, and it is the player's own doing: destroying the Enemy you were Attached to is the usual way it happens.

There is no grace period and no way for a Tongue to survive its Anchor. Should an Enemy ever gain a death animation or a corpse that lingers, that corpse is a body and the Tongue would hold it; this ADR is about a body that has genuinely left the simulation and says nothing about one that is merely dying.

`EntityId` is monotonic and never reused within a run, so there is no window in which a freed-too-late slot could have re-pointed at a different body. The sweep is correctness rather than a guard against aliasing. Decided while implementing ticket #17.
