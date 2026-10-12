---
status: accepted
---

# Discrete tap control with two concurrent Tongues

Control is discrete, not continuous: a tap fires a Tongue along the tapped vector, it attaches on contact and stays locked with no input held, and a second tap Releases it. The Shell tracks up to two independent touch ids concurrently, so two simultaneous taps fire two Tongues that attach and Release independently. This replaces the earlier decision of one continuous touch (down fires, hold Swings, lift Releases), and it is a deliberate departure from the project brief's "one-touch/one-gesture interface" line, taken because two independently anchored Tongues are the mechanic the game is actually built around.

## Consequences

This supersedes the control half of the locked boundaries.

An earlier version of this ADR warned that two Tongues made the simulation over-constrained, with infeasible configurations and jitter. That warning is **withdrawn**: ADR-0003 now models the Tongue as a spring force rather than a distance constraint, and forces simply sum as vectors. Two Tongues pulling the same way double the force, opposed pulls cancel, and no configuration is invalid. The dual-Tongue mechanic costs nothing in solver complexity.

The Macroquad resume behaviour changes meaning. Macroquad flips all live touches to `Ended` when the app minimizes, which previously handed the hold-to-Swing state machine a free Release. Under discrete tap control, `Ended` no longer implies Release, so attached Tongues survive a suspend and the input layer must not infer Release from touch loss.

**Keying touches by id is necessary but not sufficient for determinism.** Macroquad's `touches()` iterates a `HashMap`, so two fingers landing in the same frame arrive in hashing order. Claiming slots in arrival order would therefore make slot assignment a function of the platform's hashing, and the same two taps would produce different simulations between runs, which silently voids ADR-0002. Slots are claimed from `Started` touches **in ascending id order**, which makes assignment a function of the touch set rather than of its iteration order. Discovered while implementing ticket #12, and covered by a named regression test.

**Which slot a tap takes is decided by ownership alone, and what it does there is decided by whether that slot holds a Tongue.** A Gesture claims the lowest slot no other finger is currently driving; taking an empty slot fires, and taking an occupied one Releases. Both halves of this ADR then fall out of one rule rather than two. One finger tapping repeatedly always returns to slot 0 and alternates fire, Release, fire, which is the traversal loop. Two fingers down **at the same time** drive slots 0 and 1 and so hold two Tongues independently, which is what "two concurrent Tongues" means here.

The tempting alternative is to look for an empty slot first, and it is wrong: it spends the player's second tap on a second Tongue and leaves no way to let go at all. Ticket #12 did exactly that, correctly, because a tap on an occupied slot had nothing to do yet. The consequence worth knowing is that **two Tongues now require two overlapping fingers**, where under #12 a tap, a lift, and another tap also produced two. A scripted input sequence written against the old behaviour ends with no Tongues rather than two, which is how this was noticed. Discovered while implementing ticket #13.

Tongue state now lives in the Core Sim rather than being implied by whether a finger is down, since attachment outlasts the Gesture that created it. The Shell's job narrows to mapping taps to fire and Release intents against at most two slots, discarding any third concurrent touch id. Triple-tap special moves are explicitly out of scope.
