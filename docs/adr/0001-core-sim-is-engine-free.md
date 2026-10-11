---
status: accepted
---

# The Core Sim is engine-free and depends only on glam

All game state and rules live in a Core Sim crate that depends on `glam` and nothing else: no renderer, no windowing, no engine types. Engine integration lives in a separate Shell crate that reads Core Sim state and feeds it input, and which owns no game state of its own. We chose this so the engine stays swappable and so the simulation compiles, runs, and is tested headless on the host with no Android toolchain and no window present, which makes determinism tests cheap enough to run on every commit.

## Considered Options

The obvious path for a Rust game is to model entities as the engine's own types: Bevy components, or Macroquad structs holding engine handles. We rejected it because it makes the engine unswappable, drags a window and a GPU context into every test, and (with an ECS) duplicates state ownership between the framework and our own model. `glam` was chosen as the math layer specifically because it already sits underneath both Bevy and Macroquad, so depending on it is neutral rather than a bet on either.

## Consequences

The Shell cannot mutate simulation state; data flows one way. Anything the renderer needs must be expressed as Core Sim output, which means the sim-to-Shell contract is a real design artifact rather than an accident. Level parsing cannot live in the Core Sim unless the parser is `glam`-only or sits behind a feature flag. This boundary is worth nothing if it is enforced by review alone, so a mechanical check is required: see the open decision on crate topology and boundary enforcement.
