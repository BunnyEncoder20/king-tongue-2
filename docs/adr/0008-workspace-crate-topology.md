---
status: accepted
---

# Two crates, with the Core Sim boundary enforced by a build check

The workspace holds exactly two crates. `king_tongue_sim` is the Core Sim: all game state and rules, depending on `glam` and nothing else in its default feature set. `king_tongue_shell` is the platform layer: the Macroquad event loop, Android touch handling, asset handles, and drawing from simulation coordinates. Dependencies point one way, Shell to Sim, and never back.

```
king-tongue-2/
├── Cargo.toml                       workspace, shared dependency versions
├── crates/
│   ├── king_tongue_sim/             glam only; serde optional, default off
│   └── king_tongue_shell/           cdylib + rlib + desktop bin; macroquad
├── android/
│   ├── AndroidManifest.xml          hand-owned; glEsVersion 0x00030000
│   ├── java/quad_native/            vendored miniquad glue, pinned to 0.4.11
│   └── jniLibs/                     cargo-ndk output, gitignored
├── scripts/
│   ├── check-sim-deps.sh            the boundary guard
│   ├── sim-deps.expected            the allowlist it diffs against
│   └── vendor-java-glue.sh          re-syncs the Java glue on a miniquad bump
└── .github/workflows/ci.yml         runs the guard, the headless tests, the cross-compile
```

## The boundary is a build failure, not a convention

ADR-0001 is worth nothing if a reviewer has to notice a bad import. `scripts/check-sim-deps.sh` runs `cargo tree` against `king_tongue_sim` with default features off and diffs the result against a checked-in allowlist that contains exactly `glam` and the crate itself. Any new dependency fails the build and prints what was added and what to do about it. It was negative-tested by adding `ahash` to the Core Sim: the guard fired and named `ahash`, `cfg-if`, `getrandom`, and `libc`.

A lighter mechanism was preferred to `cargo-deny`, which would be another tool to install and configure for one rule. CI also runs the Core Sim's tests with `--no-default-features` and no Android toolchain present, which is what keeps determinism tests cheap enough to run on every commit.

`serde` sits behind an optional, default-off feature on the Core Sim, which ADR-0001 explicitly permits. The default build therefore stays provably `glam`-only and the guard can assert it, while the Shell turns the feature on so level data deserializes straight into Core Sim types with no duplicate DTO layer and no orphan-rule gymnastics. The level *format* remains open on its own ticket; this ADR only fixes where parsing lives.

## The Shell is a cdylib, not a bin

This is not stylistic. On Android, miniquad's Java glue calls `System.loadLibrary`, so there must be a `.so`; a bin target produces no library and there would be nothing to load. The Shell therefore declares `crate-type = ["cdylib", "rlib"]`, with the `rlib` letting a thin desktop bin link the same code. Desktop stays a test harness and is never a ship target.

`quad_main` is defined in the Shell's library, gated to `target_os = "android"`, and it calls `run` rather than `main`, because a cdylib has no `main`. Per ADR-0005 this symbol fails only at runtime when missing, so CI asserts `nm` finds exactly one of it in each shipping ABI rather than trusting that it is there.

## Consequences

Two incompatible `glam` versions are linked in the Shell. Macroquad 0.4.16 depends on `glam 0.27` while the workspace and the Core Sim use `glam 0.30`, so `glam::Vec2` denotes two different types depending on which crate you are reading. The Shell converts explicitly at the seam through `to_screen` and `from_screen`. This was chosen over pinning the workspace to Macroquad's older `glam`, because it keeps the Core Sim free to track `glam` independently of Macroquad's release cadence and stops simulation types leaking into draw calls. The cost is a second `glam` in the binary, and if that ever outweighs the decoupling, pin the workspace and delete the two converters.

The vendored Java glue is pinned to miniquad 0.4.11 and nothing version-checks it, so `scripts/vendor-java-glue.sh` must be re-run whenever that version moves. Macroquad 0.4.16 pins `miniquad = "=0.4.11"` exactly, so the pin cannot drift without a Macroquad bump, which is the only thing making this safe.

Verified at the time of writing: both shipping ABIs cross-compile, `quad_main` is exported from each, 11 `Java_quad_native_*` JNI endpoints are present, the desktop bin builds, and the Core Sim's tests pass headless. Not verified: anything requiring Gradle, which is blocked until the Android SDK is completed.
