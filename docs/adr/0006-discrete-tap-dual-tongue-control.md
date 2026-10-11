---
status: accepted
---

# Discrete tap control with two concurrent Tongues

Control is discrete, not continuous: a tap fires a Tongue along the tapped vector, it attaches on contact and stays locked with no input held, and a second tap Releases it. The Shell tracks up to two independent touch ids concurrently, so two simultaneous taps fire two Tongues that attach and Release independently. This replaces the earlier decision of one continuous touch (down fires, hold Swings, lift Releases), and it is a deliberate departure from the project brief's "one-touch/one-gesture interface" line, taken because two independently anchored Tongues are the mechanic the game is actually built around.

## Consequences

This supersedes the control half of the locked boundaries.

An earlier version of this ADR warned that two Tongues made the simulation over-constrained, with infeasible configurations and jitter. That warning is **withdrawn**: ADR-0003 now models the Tongue as a spring force rather than a distance constraint, and forces simply sum as vectors. Two Tongues pulling the same way double the force, opposed pulls cancel, and no configuration is invalid. The dual-Tongue mechanic costs nothing in solver complexity.

The Macroquad resume behaviour changes meaning. Macroquad flips all live touches to `Ended` when the app minimizes, which previously handed the hold-to-Swing state machine a free Release. Under discrete tap control, `Ended` no longer implies Release, so attached Tongues survive a suspend and the input layer must not infer Release from touch loss.

Tongue state now lives in the Core Sim rather than being implied by whether a finger is down, since attachment outlasts the Gesture that created it. The Shell's job narrows to mapping taps to fire and Release intents against at most two slots, discarding any third concurrent touch id. Triple-tap special moves are explicitly out of scope.
