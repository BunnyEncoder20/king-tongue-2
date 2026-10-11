---
status: accepted
---

# Levels are RON, parsed from bytes inside the Core Sim

Level files are RON, authored by hand under `assets/levels/`. The Shell reads them through Macroquad's asset loader and passes raw bytes to the Core Sim, which parses and validates them. The Core Sim never touches the filesystem.

## Why not YAML or JSON

YAML was the stated preference and it is not available. `serde_yaml`'s latest published version is literally `0.9.34+deprecated`: the maintainer encoded the deprecation into the version string and archived the repository. Adopting YAML would mean adopting an unmaintained parser for the one file format the game's content depends on.

JSON is maintained and universally tooled, but it cannot carry comments. These files are hand-tuned physics and layout data where a bare number is meaningless without a sentence explaining it, and comments are most of what made YAML attractive in the first place.

RON keeps that, is maintained, and maps Rust structs and enums without the tagging contortions JSON needs. Because serde is format-agnostic, this is also a cheap decision to reverse: swapping to JSON is changing `ron::from_str` to `serde_json::from_slice` and the dependency line.

ASCII level text is rejected for production, but `Level::from_ascii` exists behind a `test-util` feature for headless tests, because a nine character sketch is a far more readable test fixture than a RON blob.

## The I/O boundary

`Level::from_bytes(&[u8])` takes bytes, never a path. On Android there is no ordinary filesystem path, only APK assets, and the Core Sim must not know the difference. The Shell calls `macroquad::file::load_file`, which is async and resolves against the APK's assets root on Android and the working directory on desktop. `set_pc_assets_folder("assets")` is called before the first load so one path string works on both, and it must happen before any load or the desktop path silently differs.

A missing or malformed level holds a readable message on screen rather than failing to a black screen, because Android offers no stderr to read.

## The schema

```rust
Level {
    world_bounds: Aabb,          // the kill bound
    static_terrain: Vec<Aabb>,   // every one of these is anchorable
    player_spawn: Vec2,
    enemy_spawns: Vec<EnemySpawn { pos, toughness, mass }>,
}
```

`Aabb` is centre plus half-extents, which makes containment and overlap cheaper and matches how terrain is authored.

Three corrections to the schema as first drafted. `world_bounds` is an `Aabb` and not a single `Vec2`, because a lone vector cannot describe a rectangle without silently assuming an origin corner, and the Slam resolution needs a real world-space kill bound precisely so despawn stays device-independent. `player_spawn` was absent and the scene cannot be built without it. `spawn_frontier` was renamed `enemy_spawns`, because "frontier" already means the set of takeable tickets in this project's own vocabulary.

There is no separate anchor list. The Anchor validity resolution ruled out ungrappable surfaces, so `static_terrain` *is* the anchor set.

There are no waves, Enemy archetypes, projectiles, or art in this schema, deliberately. Those sit in the map's fog, and every field added here becomes something the implementation has to honour.

## Validation at the boundary

`from_bytes` validates before returning: positive world extents, a `player_spawn` inside the kill bound, and positive `toughness` and `mass` on every Enemy. A hero spawned outside the kill bound would despawn on tick one, which is far cheaper to reject at parse time than to debug in motion.

## Consequences

`ron` and `serde` are optional, default-off features on the Core Sim, which ADR-0001 permits and which keeps the default dependency set provably `glam`-only so `scripts/check-sim-deps.sh` can keep asserting it. The Shell enables them.

CI parses the shipped level asset on every run. A level file that stops parsing is a broken game, and it would otherwise only be discovered by launching.
