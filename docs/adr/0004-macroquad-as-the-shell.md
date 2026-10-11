---
status: accepted
---

# Macroquad is the Shell, and Bevy was rejected

Macroquad provides the Shell crate: rendering, touch input, and the Android entry point. Because ADR-0001 puts all game state in an engine-free Core Sim, the engine only draws and reads input, which removes most of the reason to take a full framework. Bevy was rejected as redundant and expensive against that shape: its ECS would duplicate the state ownership the Core Sim already has, while charging large APKs, minute-scale compile times, and the weakest Android story of the candidates. Macroquad was then verified rather than assumed (see the research ticket), and it is the only candidate that both ships first-class Android support and solves GL surface loss internally.

## Considered Options

raylib-rs 6.0.0 states plainly that Android is not supported. ggez describes Android as unofficial. comfy has no Android support at all. Going direct to wgpu plus winit plus android-activity is viable and well maintained, but winit 0.31 is still beta and its contract requires the app to drop all render surfaces before the `Suspended` callback returns, which is strictly more work for a layer that is only a render and input shell. Dropping to raw miniquad was also rejected: Macroquad runs all user code inside `draw()`, which is gated on a non-null surface, and that is what makes it structurally immune to the open miniquad crash where apps allocate GL resources while the context is NULL.

## Consequences

Macroquad is community-carried rather than author-driven, with a stalled `0.5` branch and Android build instructions effectively dropped from its README, so upstream support is a standing risk we accept knowingly. `macroquad 0.4.16` pins `miniquad = "=0.4.11"` exactly, so miniquad cannot drift without a Macroquad bump, which is a small mercy for reproducibility. Three constraints follow directly and belong in the spec: shaders must stay at GLES 2.0 syntax because miniquad's real floor is 2.0, touch handling must key on pointer `id` rather than index because `touches()` iterates a `HashMap`, and `set_simulate_mouse_with_touch(false)` must be called because it defaults to true. Nothing has run on a device or emulator yet, so the suspend, resume, touch, and GLES behaviour is sourced from upstream code review rather than execution.
