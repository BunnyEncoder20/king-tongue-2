# King Tongue 2

A 2D side-view Android game built entirely in Rust, reimplementing the core grapple mechanics of a tongue-based swinging brawler: attaching to the world, converting orbital momentum into flight, and slamming enemies with that momentum.

## Language

### Mechanics

**Tongue**:
An extensible appendage the player fires to attach to an Anchor. It is a means of attachment, not a weapon. The player may have up to two attached independently at once.
_Avoid_: Grapple, hook, rope, whip

**Anchor**:
A point or body in the world a Tongue can attach to.
_Avoid_: Grapple point, hook point, attachment node

**Fixed Anchor**:
An Anchor that cannot move, such as a wall or ceiling. Attaching pulls only the player.
_Avoid_: Static anchor, immovable anchor, world anchor

**Movable Anchor**:
An Anchor that is itself a body with mass, such as an Enemy. Attaching exchanges momentum, so both bodies are pulled toward each other.
_Avoid_: Dynamic anchor, enemy anchor

**Attach**:
The state of a Tongue being connected to an Anchor, pulling the player toward it. There is no bounded radius and no maximum length.
_Avoid_: Latch, grab, hook on

**Swing**:
The orbital motion that emerges while Attached, as the Tongue pulls inward and the player's tangential momentum carries across it. Nothing in the simulation models a pendulum.
_Avoid_: Pendulum, arc, sway

**Stretch**:
The extension of a Tongue beyond its rest length, which sets how hard it pulls. Loading a Stretch by moving away from an Anchor before being slung back is a deliberate manoeuvre.
_Avoid_: Extension, slack, recoil

**Release**:
Detaching the Tongue, converting Swing momentum into unconstrained ballistic motion.
_Avoid_: Let go, detach, drop, fire off

**Slam**:
The outcome of the player colliding with an Enemy while carrying momentum above the Kick threshold.
_Avoid_: Hit, attack, bonk, smash

**Kick**:
The outcome of the player colliding with an Enemy below the Slam threshold: a small fixed damage and an elastic repel that restores distance between the two bodies.
_Avoid_: Bump, nudge, push, weak hit

**Combo**:
An unbroken run of Slams, counted while the player is airborne and persisting across Release and re-attach. It is a count of hits, not a score.
_Avoid_: Streak, chain, multiplier, rally

**Thrown**:
The state of an Enemy that has lost control of its own momentum, having been pulled hard enough by a Tongue or struck hard enough to be weaponized. A Thrown Enemy destroys lesser Enemies it hits and is itself destroyed by terrain. It is no longer an Anchor: a Tongue already Attached keeps holding it, and no new one can be fired at it.
_Avoid_: Vicious pull, wrecking ball, stunned, ragdoll

**Enemy**:
A body in the world that can be Slammed.
_Avoid_: Mob, foe, NPC

**Toughness**:
An Enemy's resistance to being destroyed, denominated in momentum units. An impact spends momentum against it, and reaching zero destroys the Enemy.
_Avoid_: Health, hit points, HP, armour, durability

**Kill Bound**:
The world-space rectangle outside which a body is removed from the simulation. Declared per level and never derived from screen size, which varies by device.
_Avoid_: World bounds, despawn zone, screen edge, boundary

### Input

**Gesture**:
A discrete tap, which either fires a Tongue along the tapped vector or Releases one already attached. It is not a continuous hold.
_Avoid_: Input, control, swipe, drag

**Enemy Assist Arc**:
The narrow region around the Gesture's direction inside which a Tongue prefers an Enemy over the terrain behind it. It never bends the aim: terrain is hit by pure ray cast along the tapped vector.
_Avoid_: Tolerance cone, aim assist, snap radius, targeting cone

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

**Camera**:
The Shell's view onto the world, always zoomed in and always following the hero. It is view state, never game state, and the Core Sim has no knowledge of it. The whole level is never visible at once.
_Avoid_: Viewport, view, lens, screen

**Placeholder Geometry**:
Untextured primitives standing in for art, sufficient to demonstrate a mechanic and deliberately not a visual target.
_Avoid_: Stub art, programmer art, mockup
