---
status: accepted
---

# The Camera lives wholly in the Shell, always following and always zoomed

The world is always larger than the screen. The whole level is never shown at once, in any situation. The Camera is always zoomed in and always following the hero, and it derives its follow position from the hero's position in the `Snapshot` on every frame.

The Camera exists entirely in the Shell. The Core Sim has no Camera, no viewport, and no concept of what is on screen.

## Why it belongs in the Shell

ADR-0001 says the Shell owns no game state. A Camera is not game state: it is view state. It changes what the player sees and never what the simulation computes. Putting it in the Core Sim would make the simulation's output depend on a screen, which is precisely what ADR-0001 exists to prevent, and it would reintroduce the device dependence that made screen-space despawn unacceptable in ADR-0011.

The Camera reads the **interpolated** snapshot rather than the raw tick state, so follow is smooth at render rate rather than stepping at 60Hz. Following the raw tick position would judder on a 120Hz panel even though the simulation is correct.

## What is deliberately not decided

The fixed zoom factor is left open and will be tuned per device later. `PIXELS_PER_UNIT` in the Shell is currently a bare constant, which is a placeholder and not the answer. Device tuning waits until something runs on a device, which is gated on the Android SDK work.

## Consequences

Golden-trace replay will capture simulation state and not the view, because the Camera is not in the `Snapshot`. That is correct: determinism is a property of the tick sequence, and two machines replaying the same trace should agree on the simulation whatever their screens are doing.

The ruling specifies a stateless derivation, position straight from the hero each frame. Worth flagging honestly rather than discovering later: a hard, undamped follow in a fast swinging game is the usual cause of the world appearing to move instead of the hero, and of motion discomfort at speed. If that proves true in play, the fix is damping or lookahead, which requires the Shell to retain the Camera's own position between frames. That does not breach ADR-0001, since the Shell already retains snapshots and touch slots, and Camera position remains view state. The decision here is the stateless version; this records the escape hatch and why taking it would be legitimate.

Because the whole level is never visible, level authoring cannot rely on offscreen context being readable, and the Kill Bound in ADR-0010 stays world-space and unrelated to what the Camera can see.
