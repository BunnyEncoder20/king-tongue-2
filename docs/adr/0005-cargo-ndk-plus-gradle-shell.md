---
status: accepted
---

# APKs are built with cargo-ndk plus a hand-owned Gradle shell

Android packaging uses `cargo-ndk` to produce the `jniLibs` tree and a Gradle project we own and check in to build the APK. A reader will expect `cargo-apk` or `xbuild` here, so the rejection is recorded: both exist to package a cdylib behind `NativeActivity` with no Java compilation step, while miniquad uses a plain `MainActivity extends Activity` plus its own `quad_native.QuadNative` class carrying eleven native methods. The APK those tools produce therefore has no class to call into miniquad. That is structural and permanent, not version skew. `xbuild` additionally self-declares unmaintained, and the upstream `notfl3/cargo-apk` Docker image bakes NDK r25 and rustc 1.79, so it cannot honour the NDK r30 this project targets.

## Consequences

We own the Java glue (`MainActivity.java` and `QuadNative.java`), vendored from a pinned miniquad tag, and nothing version-checks it, so it must be re-synced deliberately on every miniquad bump. We also own the manifest, which must declare `glEsVersion="0x00030000"` explicitly.

One consequence is sharp enough to be a trap. miniquad declares `extern "C" { fn quad_main(); }` and calls it from `activityOnCreate`, but never defines it; `cargo-apk` style tooling injects the shim invisibly. A `cargo-ndk` build must supply it by hand:

```rust
#[no_mangle]
pub extern "C" fn quad_main() { main(); }
```

Omit it and `System.loadLibrary` throws `UnsatisfiedLinkError: cannot locate symbol "quad_main"` at runtime, with no compile error and no link error, because cdylib links tolerate undefined symbols. This was verified by inspecting exported symbols before and after adding the shim.

`cargo-quad-apk` from git remains a fallback, but it hardcodes `d8 --min-api 26`, which contradicts our `minSdk 24`; the Gradle pipeline avoids that entirely. Note also that the platform flag is `--platform`, since `-p` is cargo's `--package` and panics.
