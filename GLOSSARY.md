# King Tongue 2

A 2D side-view Android game built entirely in Rust, reimplementing the core grapple mechanics of a tongue-based swinging brawler: attaching to the world, converting pendulum momentum into flight, and slamming enemies with that momentum.

## Language

### Mechanics

**Tongue**:
The extensible appendage the player fires to attach to an Anchor. It is a means of attachment, not a weapon.
_Avoid_: Grapple, hook, rope, whip

**Anchor**:
A point in the world the Tongue can attach to.
_Avoid_: Grapple point, hook point, attachment node

**Attach**:
The state of the Tongue being connected to an Anchor, holding the player at a bounded radius from it.
_Avoid_: Latch, grab, hook on

**Swing**:
Pendulum motion of the player about an Anchor while Attached.
_Avoid_: Pendulum, arc, sway

**Release**:
Detaching the Tongue, converting Swing momentum into unconstrained ballistic motion.
_Avoid_: Let go, detach, drop, fire off

**Slam**:
The outcome of the player colliding with an Enemy while carrying velocity above a threshold.
_Avoid_: Hit, attack, bonk, smash

**Enemy**:
A body in the world that can be Slammed.
_Avoid_: Mob, foe, NPC

### Input

**Gesture**:
One continuous touch, from touch down to lift, that drives a full fire, Swing, and Release cycle. The interface has exactly one.
_Avoid_: Input, control, tap, swipe

**Tolerance Cone**:
The angular region around the Gesture's direction inside which the Tongue selects the best valid Anchor, rather than requiring a precise hit.
_Avoid_: Aim assist, snap radius, targeting cone

### Architecture

**Core Sim**:
The deterministic, engine-free simulation that owns all game state and rules.
_Avoid_: Engine, game logic, model, world

**Shell**:
The thin layer that renders Core Sim state and feeds it Gesture input. It owns no game state.
_Avoid_: Frontend, view, platform layer, renderer

**Sim Tick**:
One fixed-duration step of the Core Sim, advanced independently of render frames.
_Avoid_: Frame, update, step

**Placeholder Geometry**:
Untextured primitives standing in for art, sufficient to demonstrate a mechanic and deliberately not a visual target.
_Avoid_: Stub art, programmer art, mockup
