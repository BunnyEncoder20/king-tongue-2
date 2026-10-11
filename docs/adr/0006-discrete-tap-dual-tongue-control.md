---
status: accepted
---

# Discrete tap control with two concurrent Tongues

Control is discrete, not continuous: a tap fires a Tongue along the tapped vector, it attaches on contact and stays locked with no input held, and a second tap Releases it. The Shell tracks up to two independent touch ids concurrently, so two simultaneous taps fire two Tongues that attach and Release independently. This replaces the earlier decision of one continuous touch (down fires, hold Swings, lift Releases), and it is a deliberate departure from the project brief's "one-touch/one-gesture interface" line, taken because two independently anchored Tongues are the mechanic the game is actually built around.

## Consequences

This supersedes the control half of the locked boundaries and amends the premise of ADR-0003, which scoped the simulation to one point mass under one constraint. Two rope constraints on one body is an over-constrained system: when the two Anchors are separated by more than the sum of the Tongue lengths, no valid player position exists, and a position-based solver converges to a least-violation state that reads on screen as stretching or jitter. Rope constraints are inequalities, so in practice usually only one is taut, but the solver must handle the taut-taut case deliberately rather than by accident. The Tongue prototype carries this.

The Macroquad resume behaviour changes meaning. Macroquad flips all live touches to `Ended` when the app minimizes, which previously handed the hold-to-Swing state machine a free Release. Under discrete tap control, `Ended` no longer implies Release, so attached Tongues survive a suspend and the input layer must not infer Release from touch loss.

Tongue state now lives in the Core Sim rather than being implied by whether a finger is down, since attachment outlasts the Gesture that created it. The Shell's job narrows to mapping taps to fire and Release intents against at most two slots, discarding any third concurrent touch id. Triple-tap special moves are explicitly out of scope.
