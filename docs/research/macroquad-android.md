# Macroquad on Android: build pipeline, lifecycle, GLES floor, touch

Research ticket: [#2](https://github.com/BunnyEncoder20/king-tongue-2/issues/2). Parent map: [#1](https://github.com/BunnyEncoder20/king-tongue-2/issues/1).
Researched 2026-10-11. Constraints assumed: NDK r30 (30.0.16248370), `minSdk 24`, ABIs `arm64-v8a` + `armeabi-v7a`, GLES 3.0 renderer floor, Core Sim is `glam`-only with zero engine types.

## Verdict

**Yes. Keep Macroquad as the Shell crate.** It is the right thin 2D render + input shell for this project, and no better-maintained option has appeared.

The reasons, in order of weight:

1. Macroquad is the only candidate in the field that both ships first-class Android support *and* solves the Android surface-loss problem internally. Every wgpu-based alternative pushes surface teardown and recreation onto the application.
2. It cross-compiles cleanly to both shipping ABIs on this exact toolchain. Verified below, not assumed.
3. Multi-touch with stable per-pointer ids is exposed directly, which is exactly what the one-Gesture input model needs.
4. The competition is worse: raylib-rs states outright that Android is not supported, and ggez calls Android "not officially supported".

The build tool decision does **not** go the way the upstream README implies. Use **`cargo-ndk` plus a hand-owned Gradle shell**. The upstream Docker image is two years stale, `cargo-apk` and `xbuild` are structurally incompatible with miniquad, and `xbuild` is self-declared unmaintained.

Two things to be honest about. First, Macroquad is community-maintained and slow rather than actively driven; its Android build documentation has effectively been dropped. Second, this machine **cannot package an APK today**: the Android SDK installation is missing `build-tools` and `platforms` and is not writable. See [Open risks](#open-risks).

The locked crate boundary (Core Sim depends on `glam` and nothing else) is what makes this verdict low-risk: if Macroquad does stall, only the Shell crate is replaced.

---

## 1. Build tool

### Summary table

| Tool | Latest release | Repo last push | Verdict |
| --- | --- | --- | --- |
| `cargo-ndk` + Gradle shell | 4.1.2, 2025-08-09 | 2026-10-10 | **Recommended** |
| `not-fl3/cargo-quad-apk` (git master) | crates.io 0.1.4, 2022-07-04 | 2026-07-28 | Viable fallback, install from git only |
| `notfl3/cargo-apk` Docker image | `latest` tag pushed 2024-07-21 | n/a | Stale escape hatch; cannot honour NDK r30 |
| `rust-mobile/cargo-apk` | 0.10.0, 2023-11-30 | 2026-04-08 (commits stop 2025-07-10) | **Incompatible.** Reject |
| `rust-mobile/xbuild` | 0.2.0, 2022-12-21 | 2026-07-21 (commits stop 2025-08-15) | **Unmaintained and incompatible.** Reject |

Version and date sources: [crates.io API](https://crates.io/api/v1/crates/macroquad) for each crate, GitHub repo metadata, and the [Docker Hub tag API for `notfl3/cargo-apk`](https://hub.docker.com/v2/repositories/notfl3/cargo-apk/tags).

### Why `cargo-apk` and `xbuild` cannot work, regardless of maintenance

This is a structural incompatibility, not a version-skew problem, and it rules both out permanently.

miniquad does **not** use `NativeActivity`, `GameActivity`, or `android-activity`. It uses its own plain Java Activity. From [`miniquad/java/MainActivity.java`](https://github.com/not-fl3/miniquad/blob/master/java/MainActivity.java):

```java
public class MainActivity extends Activity {
```

It renders into a raw `SurfaceView` subclass (`QuadSurface`) and marshals every callback through a separate class, [`quad_native.QuadNative`](https://github.com/not-fl3/miniquad/blob/master/java/QuadNative.java), which declares exactly eleven native methods:

```java
public native static void activityOnCreate(Object activity);
public native static void activityOnResume();
public native static void activityOnPause();
public native static void activityOnDestroy();
public native static void surfaceOnSurfaceCreated(Surface surface);
public native static void surfaceOnSurfaceDestroyed(Surface surface);
public native static void surfaceOnTouch(int id, int phase, float x, float y);
public native static void surfaceOnSurfaceChanged(Surface surface, int width, int height);
public native static void surfaceOnKeyDown(int keycode);
public native static void surfaceOnKeyUp(int keycode);
public native static void surfaceOnCharacter(int character);
```

`rust-mobile/cargo-apk` and `xbuild` both exist to package a Rust `cdylib` behind `NativeActivity` or `GameActivity` with **no custom Java compilation step**. They cannot compile `MainActivity.java` and `QuadNative.java`, so the APK they produce has no class to call into miniquad. The library would load and then nothing would ever happen. Any build tool for Macroquad must compile Java.

On top of that, [`rust-mobile/xbuild`](https://github.com/rust-mobile/xbuild)'s own GitHub repository description reads:

> Cross compile rust to any platform **(unmaintained)**

That is the maintainers' own word, and it is the end of the argument for `xbuild`.

### Why the upstream Docker image is not the pipeline

[`miniquad`'s README](https://github.com/not-fl3/miniquad/blob/master/README.md) still recommends it:

```sh
docker run --rm -v $(pwd)":/root/src" -w /root/src notfl3/cargo-apk cargo quad-apk build --example quad
```

Problems, all verified:

- The `latest` tag on Docker Hub was last pushed **2024-07-21**, over two years ago.
- Its [`Dockerfile`](https://github.com/not-fl3/cargo-quad-apk/blob/master/Dockerfile) pins `rustup toolchain install 1.79.0` and `rustup default 1.79`, and installs **NDK r25** (`ENV NDK_HOME /usr/local/android-ndk-r25`).
- Because it bakes its own NDK, it structurally **cannot** honour the locked NDK r30 decision. The image's toolchain is the build, and the host's `/opt/android-ndk` is ignored.
- `macroquad`'s own README no longer documents Android at all. It lists Android under "Supported Platforms" and claims "Single command deploy for both WASM and Android", but there is no Android build section; the README jumps Linux, Windows, WASM, iOS. Verified by grepping all `android`/`apk`/`docker` mentions in [the current README](https://github.com/not-fl3/macroquad/blob/master/README.md): only lines 16, 22, 202 and 218, none of which are instructions.
- Macroquad issue [#1036 "How to build apk / aab"](https://github.com/not-fl3/macroquad/issues/1036) (opened 2026-04-24) is open with **zero comments**. The documentation gap is real and unanswered.

Keep the image as a last-resort escape hatch for reproducing an upstream bug report. It is not the project pipeline.

### `cargo-quad-apk` from git: viable fallback, with sharp edges

[`not-fl3/cargo-quad-apk`](https://github.com/not-fl3/cargo-quad-apk) is the fork the Docker image wraps, and it is the only turnkey tool that actually compiles miniquad's Java. Its git master is alive: last commits **2026-07-28**. Critically, one of those commits is:

> `ca23fe59e` (2026-07-28) check whether lib64 dir exists or not otherwise fallback to lib since it changed in NDK 29 now.

That fix is **mandatory for NDK r30**. Verified on this machine: `/opt/android-ndk/toolchains/llvm/prebuilt/linux-x86_64/lib64` does not exist; only `lib` does. The crates.io release `0.1.4` (2022-07-04) predates the fix by four years, so **never install this from crates.io**; use `--git`.

Sharp edges found by reading [`src/ops/build.rs`](https://github.com/not-fl3/cargo-quad-apk/blob/master/src/ops/build.rs):

- It hardcodes `d8 --min-api 26`:
  ```rust
  d8_cmd.arg("--no-desugaring");
  d8_cmd.arg("--min-api").arg("26");
  ```
  This **conflicts with the locked `minSdk 24`**. The manifest would declare 24 while the dex is built for 26. Flagged as a risk; unverified whether it actually fails to install or verify on an API 24 or 25 device, since no such device was available.
- It calls `javac -source 1.8 -target 1.8`. Verified this still works on the installed Java 26 (`openjdk 26.0.2.1`, build 2026-08-18): exit code 0 with four obsolescence warnings, including "source value 8 is obsolete and will be removed in a future release". Not a blocker today, but it is on borrowed time.
- It requires `NDK_HOME` (not `ANDROID_NDK_HOME`) plus `ANDROID_HOME` or `ANDROID_SDK_HOME`, and shells out to `aapt2`, `d8`, `zipalign`, `apksigner`, `keytool` and `zip`.
- It rewrites the crate type, from [`src/ops/build/compile.rs`](https://github.com/not-fl3/cargo-quad-apk/blob/master/src/ops/build/compile.rs): "Change crate-type from bin to cdylib". So it builds a `bin` target, not a `lib`.

### The decisive detail nobody documents: `quad_main`

This is the single most important finding in this section, and it is the thing that makes a naive `cargo-ndk` plus Gradle attempt fail at runtime with no compile error.

[`miniquad/src/native/android.rs`](https://github.com/not-fl3/miniquad/blob/master/src/native/android.rs) declares an external symbol it does not define:

```rust
extern "C" {
    fn quad_main();
}
```

and calls it from the activity entry point:

```rust
#[no_mangle]
pub unsafe extern "C" fn Java_quad_1native_QuadNative_activityOnCreate(
    _: *mut ndk_sys::JNIEnv,
    _: ndk_sys::jobject,
    activity: ndk_sys::jobject,
) {
    let env = attach_jni_env();
    ACTIVITY = (**env).NewGlobalRef.unwrap()(env, activity);
    quad_main();
}
```

`quad_main` must be provided by the **top-level crate**. `cargo-quad-apk` provides it invisibly by injecting [`src/native/android/mod_inject.rs`](https://github.com/not-fl3/miniquad/blob/master/src/native/android/mod_inject.rs) into the crate root:

```rust
#[no_mangle]
pub extern "C" fn quad_main() {
    let _ = super::main();
}
```

The in-source comment explains the original rationale: extern `"C"` functions living in a dependency were reportedly invisible to JNI unless redeclared in the top crate ("Why? I do not know.").

Two things verified by experiment on this machine:

1. **The "invisible to JNI" claim is stale.** Building a `cdylib` with rustc 1.99.0 exported all eleven `Java_quad_1native_*` symbols from the dependency without any injection. Confirmed with `nm -D --defined-only`. So the Gradle path does not need the injection hack for the JNI bridges.
2. **`quad_main` genuinely is missing and must be written by hand.** A `cdylib` with `#[macroquad::main]` in `src/lib.rs` linked successfully but contained **no `quad_main` symbol at all**, because a `cdylib` link tolerates undefined symbols and resolves them at `dlopen`. On device, `System.loadLibrary` would fail with `UnsatisfiedLinkError: cannot locate symbol "quad_main"`. The macro's generated `amain` was even reported by rustc as dead code, which is the visible tell.

Adding an eight-line shim fixed it, and the symbol then appeared in both ABIs. This shim is a required part of the recommended pipeline:

```rust
/// miniquad declares `extern "C" { fn quad_main(); }` and calls it from
/// Java_quad_native_QuadNative_activityOnCreate. cargo-quad-apk injects this
/// shim automatically; in a cargo-ndk + Gradle build we must provide it.
#[no_mangle]
pub extern "C" fn quad_main() {
    main();
}
```

### What was actually run, and what happened

A throwaway crate was built in the scratchpad (not committed) depending on `macroquad = "0.4.16"` with `crate-type = ["cdylib"]`.

Command:

```sh
export ANDROID_NDK_HOME=/opt/android-ndk
cargo ndk -t arm64-v8a -t armeabi-v7a --platform 24 -o ./jniLibs build --release
```

Result: **success, exit 0**, "Finished `release` profile [optimized] target(s) in 5.40s". It pulled and compiled `miniquad v0.4.11`, `macroquad v0.4.16`, `glam v0.27.0`, `fontdue v0.9.4`, `image v0.24.9`, `ndk-sys v0.2.2`. One dead-code warning on `amain` (the tell described above).

Artifacts and inspection:

```
jniLibs/arm64-v8a/libmqtest.so      628720 bytes   ELF64 / AArch64
jniLibs/armeabi-v7a/libmqtest.so    380896 bytes   ELF32 / ARM
```

`readelf -d` on both shows only three `NEEDED` entries: `libandroid.so`, `libdl.so`, `libc.so`. There is deliberately **no** `NEEDED` entry for `libEGL.so` or `libGLESv2.so`, because miniquad `dlopen`s them at runtime via `LibEgl::try_load()`. That is worth knowing: a missing GL driver surfaces as a runtime panic ("Cant load LibEGL"), not a link error.

`nm -D --defined-only` confirmed all eleven `Java_quad_1native_*` entry points are exported, exactly matching `QuadNative.java`. After adding the `quad_main` shim and rebuilding, `quad_main` was exported from both ABIs (`T quad_main` in each) and no longer appeared in the undefined-symbol list.

Note on the CLI flag, confirming the parent map's warning: `cargo ndk --help` on 4.1.2 shows `-P, --platform <PLATFORM>` with default `21`. The short form is capital `-P`; lowercase `-p` is cargo's `--package`. Also confirmed: `-o, --output-dir <DIR>` "Output to a `jniLibs` directory in the correct sub-directories", which is precisely the layout Gradle's `sourceSets` consumes. That flag is why this pipeline is clean.

`cargo-ndk` itself is the healthiest tool in the entire field: 4.1.2 published 2025-08-09, repo last pushed **2026-10-10** (yesterday), and only **5 open issues**.

---

## 2. Suspend and resume

**Yes. Macroquad still handles Android suspend and resume without losing the GL surface, and it does so better than the DIY alternatives.** This was historically the hard problem for Rust Android games; miniquad's design sidesteps the classic failure mode.

### The mechanism

Read from [`miniquad/src/native/android.rs`](https://github.com/not-fl3/miniquad/blob/master/src/native/android.rs) at `0.4.11`/master.

miniquad does **not** use `GLSurfaceView`, which is where the classic context loss came from (`GLSurfaceView` destroys the EGL context on pause unless `setPreserveEGLContextOnPause(true)`). Instead it runs its own EGL on a dedicated render thread over a raw `SurfaceView`.

The EGL **context** is created exactly once, in the thread spawned by `run()`, and destroyed only at process quit:

```rust
(s.libegl.eglMakeCurrent)(s.egl_display, null_mut(), null_mut(), null_mut());
(s.libegl.eglDestroySurface)(s.egl_display, s.surface);
(s.libegl.eglDestroyContext)(s.egl_display, s.egl_context);
(s.libegl.eglTerminate)(s.egl_display);
```

Only the **EGLSurface** churns across the lifecycle:

```rust
unsafe fn destroy_surface(&mut self) {
    (self.libegl.eglMakeCurrent)(self.egl_display, null_mut(), null_mut(), null_mut());
    (self.libegl.eglDestroySurface)(self.egl_display, self.surface);
    self.surface = std::ptr::null_mut();
}
```

and `update_surface()` recreates it with `eglCreateWindowSurface` and rebinds it with `eglMakeCurrent` against **the same `egl_context`**.

**Consequence: GL object names survive. Nothing needs reloading on resume.** Textures, shaders and buffers remain valid because the context was never destroyed. There is no asset reload, no atlas rebuild, no shader recompile.

Drawing is correctly gated so no GL work is attempted while the surface is gone:

```rust
fn frame(&mut self) {
    self.event_handler.update();

    if self.surface.is_null() == false {
        self.update_requested = false;
        self.event_handler.draw();
        unsafe { (self.libegl.eglSwapBuffers)(self.egl_display, self.surface); }
    }
}
```

Note carefully: `draw()` is guarded, `update()` is **not**.

### Why that asymmetry is safe for Macroquad specifically

This matters, because it is where a real open bug lives.

Miniquad issue [#572](https://github.com/not-fl3/miniquad/issues/572), "android: null pointer dereference (SIGSEGV) fatal crash inside GLES `miniquad::native::gl::glDrawElementsInstanced()`" (opened 2025-08-01, last activity 2025-11-30, 8 comments, **still open**), is exactly this hazard. The reporter diagnosed it precisely:

> ok I found the issue. It seems the EGL context becomes null, and if you allocate buffers then the driver does nothing. [...] Previous buffers survive since we keep the GL context alive, but we should not allocate any buffers between `surfaceDestroyed` and `surfaceCreated`.

Because `destroy_surface()` calls `eglMakeCurrent(display, NULL, NULL, NULL)`, the render thread has **no current context** while suspended, yet `update()` keeps being called. Any GL allocation there is silently dropped or crashes.

**Macroquad is structurally immune to this**, which is the good news. Its `EventHandler::update` does almost nothing, from [`macroquad/src/lib.rs`](https://github.com/not-fl3/macroquad/blob/master/src/lib.rs):

```rust
fn update(&mut self) {
    let _z = telemetry::ZoneGuard::new("Event::update");
    miniquad::window::set_cursor_grab(get_context().cursor_grabbed);
    #[cfg(not(target_arch = "wasm32"))]
    { std::thread::yield_now(); }
}
```

**All** user code runs inside `draw()`, where the async `main` future is resumed and `next_frame().await` returns. Since `draw()` is skipped while the surface is null, a Macroquad app simply freezes while backgrounded and never issues a GL call without a context. Issue #572 bites applications that implement `miniquad::EventHandler` by hand and allocate in `update()`. **That is an argument for staying on Macroquad rather than dropping to raw miniquad.**

Macroquad also does useful lifecycle housekeeping for free:

```rust
fn window_minimized_event(&mut self) {
    let context = get_context();
    #[cfg(target_os = "android")]
    context.audio_context.pause();
    context.mouse_released.extend(context.mouse_down.drain());
    context.keys_released.extend(context.keys_down.drain());
    // Announce all touches as released
    for (_, touch) in context.touches.iter_mut() {
        touch.phase = input::TouchPhase::Ended;
    }
    ...
}
```

and `window_restored_event()` calls `context.audio_context.resume()` on Android.

### What the app must do on resume

1. **Clamp the first post-resume frame delta. This is mandatory and is the one real correctness bug waiting to happen.** Macroquad computes frame time inside `draw()`:
   ```rust
   get_context().frame_time = date::now() - get_context().last_frame_time;
   get_context().last_frame_time = date::now();
   ```
   Because `draw()` does not run while suspended, the first `get_frame_time()` after resume equals the **entire suspend duration**. Fed raw into the locked fixed-60Hz accumulator, a three-minute backgrounding produces roughly 10,800 catch-up steps in one frame: a multi-second freeze, or a hang. Clamp the delta (for example to 0.25s, around 15 steps) before it reaches the accumulator, and cap steps-per-frame as a second line of defence.
2. **Nothing for GL state.** No reload, no re-upload, no shader recompile. The context survived.
3. **Treat the minimize-time touch `Ended` as a genuine Gesture Release.** Macroquad already flips every live touch to `Ended` on minimize, so the one-Gesture state machine gets a clean Release and the Tongue detaches rather than sticking across a suspend. This is desirable; just do not filter it out.
4. **Do not allocate GL resources outside `draw()`.** Free as long as the Shell stays on the Macroquad API. If a custom `miniquad::EventHandler` is ever introduced, issue #572 becomes live and `eglGetCurrentContext()` must be checked.
5. **Audio needs no handling** if using Macroquad's `audio` feature; pause and resume are automatic on Android. (Out of scope for the acceptance bar anyway.)

### Secondary issue worth recording

While backgrounded, the render thread **busy-spins**. With the default `blocking_event_loop: false`, the loop is:

```rust
while !s.quit {
    ...
    if !conf.platform.blocking_event_loop || s.update_requested {
        s.frame();
    }
    thread::yield_now();
}
```

Normally `eglSwapBuffers` provides vsync throttling, but it is skipped when the surface is null, leaving only `thread::yield_now()`. The result is one core pegged while the app is in the background: battery drain and thermal load, not a crash. Mitigations available in [`miniquad/src/conf.rs`](https://github.com/not-fl3/miniquad/blob/master/src/conf.rs): `blocking_event_loop: bool` and `sleep_interval_ms: Option<u32>`, the latter documented as "Currently supported only on Android". Not verified on a device.

---

## 3. GLES floor

**GLES 3.0 is safe as a declared floor, because miniquad's actual floor is GLES 2.0. But do not assume you receive a GLES 3 context, and do not write `#version 300 es` shaders for Android.**

### What miniquad requests

From [`miniquad/src/native/egl.rs`](https://github.com/not-fl3/miniquad/blob/master/src/native/egl.rs), the context attributes are hardcoded to ES 2:

```rust
let ctx_attributes = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
```

The config attributes do not specify `EGL_RENDERABLE_TYPE` at all; they request only `EGL_SURFACE_TYPE`/`EGL_WINDOW_BIT`, RGB 8/8/8, alpha 0 or 8, `EGL_DEPTH_SIZE 16`, `EGL_STENCIL_SIZE 0` and `EGL_SAMPLES`.

Miniquad's own [README](https://github.com/not-fl3/miniquad/blob/master/README.md) lists its Android support as "Android, GLES 2, GLES 3", and Macroquad's built-in shaders in [`src/quad_gl.rs`](https://github.com/not-fl3/macroquad/blob/master/src/quad_gl.rs) are GLSL ES 1.00:

```glsl
#version 100
attribute vec2 position;
varying lowp vec2 uv;
```

So Macroquad renders correctly on a plain GLES 2.0 device. A GLES 3.0 floor is comfortably **above** what the engine needs, and the locked decision is safe.

### How ES3 features are actually unlocked

Capability detection is purely from the runtime version string, in `gl_info()` in [`miniquad/src/graphics/gl.rs`](https://github.com/not-fl3/miniquad/blob/master/src/graphics/gl.rs):

```rust
let gl_version_string = /* glGetString(GL_VERSION) */;
let gl2 = gl_version_string.is_empty()
    || gl_version_string.starts_with("2")
    || gl_version_string.starts_with("OpenGL ES 2");
let features = Features { instancing: !gl2, resolve_attachments: !webgl1 && !gl2 };
...
let gles3 = gl_version_string.contains("OpenGL ES 3");
if gles3 { glsl_support.v300es = true; }
```

So instancing and GLSL `300 es` are enabled only if the driver reports `"OpenGL ES 3..."`, despite the ES 2 request. In practice Android drivers do return an ES 3.x context for an ES 2 request, since ES3 is backward compatible. **Corroborating evidence from the tracker rather than from memory:** miniquad issue [#572](https://github.com/not-fl3/miniquad/issues/572) is a crash *inside* `glDrawElementsInstanced` on Android, and `draw()` only reaches that call when `self.info.features.instancing` is true, which requires `gl2 == false`. A real Android device therefore reported `OpenGL ES 3.x` from this exact ES2-requesting code path.

**Unverified:** no device or emulator was available, so the `GL_VERSION` string on our actual target hardware was not observed. Treat "we get an ES3 context" as probable but unconfirmed. The safe engineering position is unchanged: rely on nothing above GLES 2.0 in Shell shader code, and let miniquad opportunistically use instancing when the driver offers it.

### Declaring the floor in the manifest

The manifest is a separate axis from the context version and must be set explicitly, since the default is 2.0.

- `cargo-quad-apk` path, in `Cargo.toml` per its [README](https://github.com/not-fl3/cargo-quad-apk/blob/master/README.md) ("The maximum supported OpenGL ES version, as claimed by the manifest. Defaults to 2.0."):
  ```toml
  [package.metadata.android]
  opengles_version_major = 3
  opengles_version_minor = 0
  ```
- Gradle path, in `AndroidManifest.xml`:
  ```xml
  <uses-feature android:glEsVersion="0x00030000" android:required="true" />
  ```

This only filters the Play Store device list; it does not change which context miniquad asks for.

---

## 4. Touch input

**Yes. Macroquad exposes full multi-touch with stable per-pointer ids, and it is a direct fit for the one-Gesture model.** The API surface is confirmed as adequate for identifying and ignoring stray second fingers.

### The API

From [`macroquad/src/input.rs`](https://github.com/not-fl3/macroquad/blob/master/src/input.rs):

```rust
pub enum TouchPhase { Started, Stationary, Moved, Ended, Cancelled }

pub struct Touch {
    pub id: u64,
    pub phase: TouchPhase,
    pub position: Vec2,
}

/// Return touches with positions in pixels.
pub fn touches() -> Vec<Touch>

/// Return touches with positions in range [-1; 1].
pub fn touches_local() -> Vec<Touch>
```

### Where the ids come from, and why they are stable

The ids are Android's own pointer ids, passed straight through. From [`miniquad/java/MainActivity.java`](https://github.com/not-fl3/miniquad/blob/master/java/MainActivity.java), the `onTouch` handler explicitly handles the secondary-pointer actions:

```java
case MotionEvent.ACTION_POINTER_DOWN: {
    final int pointerIndex = event.getActionIndex();
    final int id = event.getPointerId(pointerIndex);
    final float x = event.getX(pointerIndex);
    final float y = event.getY(pointerIndex);
    QuadNative.surfaceOnTouch(id, 2, x, y);
}
```

`ACTION_MOVE` iterates every pointer via `event.getPointerId(i)`, and `ACTION_POINTER_UP` reports the lifted pointer by its own id. The integer phase codes are mapped in [`android.rs`](https://github.com/not-fl3/miniquad/blob/master/src/native/android.rs):

```rust
let phase = match action {
    0 => TouchPhase::Moved,
    1 => TouchPhase::Ended,
    2 => TouchPhase::Started,
    3 => TouchPhase::Cancelled,
    x => panic!("Unsupported touch phase: {}", x),
};
```

`MotionEvent.getPointerId` is contractually stable for the life of a pointer, so a finger keeps one id from down to up. That is exactly the guarantee the one-Gesture model needs.

### Per-frame bookkeeping, and the gotchas

Macroquad stores touches in a map keyed by id (`touches: HashMap<u64, input::Touch>`), upserting on each event, and ages them in `end_frame()`:

```rust
// remove all touches that were Ended or Cancelled
self.touches.retain(|_, touch| {
    touch.phase != input::TouchPhase::Ended && touch.phase != input::TouchPhase::Cancelled
});
// change all Started or Moved touches to Stationary
for touch in self.touches.values_mut() {
    if touch.phase == input::TouchPhase::Started || touch.phase == input::TouchPhase::Moved {
        touch.phase = input::TouchPhase::Stationary;
    }
}
```

Three practical consequences for the Shell's input mapping:

1. **Key on `id`, never on index.** `touches()` collects from a `HashMap`, so the returned `Vec` order is **not** stable between frames. Any code resembling `touches()[0]` is a latent bug.
2. **`Ended` and `Cancelled` are visible for exactly one frame**, then dropped. Poll every frame and do not expect to see a release twice.
3. **A held finger reports `Stationary`, not `Moved`.** Hold-to-Swing must treat `Stationary` as "still down", not as "no input".

### Shape of the one-Gesture filter

This pattern was compiled and verified against `macroquad 0.4.16` as part of the build check:

```rust
let mut active: Option<u64> = None;
loop {
    for t in touches() {
        match t.phase {
            // first finger down claims the gesture; later ids are ignored
            TouchPhase::Started if active.is_none() => active = Some(t.id),
            // only the claiming finger can release it
            TouchPhase::Ended | TouchPhase::Cancelled if active == Some(t.id) => active = None,
            _ => {}
        }
    }
    if let Some(id) = active {
        if let Some(t) = touches().into_iter().find(|t| t.id == id) {
            // hold: feed t.position to the Swing
        }
    }
    next_frame().await
}
```

A stray second finger arrives with a different id, fails the `active.is_none()` guard, and is discarded. Its lift cannot release the Gesture. **Confirmed: the API surface is sufficient.**

### One configuration note

`simulate_mouse_with_touch` defaults to **`true`** (`simulate_mouse_with_touch: true` in context construction), which synthesises `MouseButton::Left` down/up/motion events from touches. For a touch-only game that also reads mouse input on the desktop test harness, this causes double handling. Call `set_simulate_mouse_with_touch(false)` on Android, or read touches exclusively.

---

## 5. Is Macroquad still the right choice?

**Yes.** Nothing better-maintained has appeared for this specific job, and the main alternative is strictly more work for a project whose engine is only a render + input shell.

### Macroquad's actual health

Not abandoned, but not energetically driven either. Honest assessment:

Positive:
- `macroquad` **0.4.16** published **2026-07-30**; repo last pushed **2026-08-18**.
- `miniquad` **0.4.11** published **2026-06-03**; repo last pushed **2026-07-25**.
- Recent commits are substantive engine work, not just housekeeping: "fix(texture): only emit MSAA resolve attachment when sample_count > 1" (2026-06-15), "text: avoid spurious font atlas growth" (2026-08-18), a run of Metal MSAA work, and "ios: drive draws from CADisplayLink instead of a polling render thread" (2026-06-16).
- Crucially for us, there is still active Android-aware maintenance: "cfg-gate Android-only `set_or_replace_display`" (2026-06-15).
- 1.85M downloads for `macroquad`, 2.08M for `miniquad`.

Negative, and worth recording:
- Maintenance has shifted away from the original author. Of the last 50 `miniquad` commits, roughly 22 are from Benoît Rouleau and 10 from Pewnack, with Fedor Logachev (`not-fl3`) at 4. `not-fl3` is still committing, so this is succession rather than abandonment, but the project is now community-carried.
- **347 open issues** on `macroquad`, **183** on `miniquad`.
- The `0.5` branch has been **stalled since 2024-09-22** ("New camera API"). There is no next major version coming; `0.4.x` is the line.
- Neither repo publishes GitHub Releases or a `CHANGELOG.md` (both return 404). Version history is only legible via crates.io and "Bump version" commits.
- Android build documentation has effectively been dropped from `macroquad`'s README, and the unanswered issue [#1036](https://github.com/not-fl3/macroquad/issues/1036) confirms users are hitting it.

### The alternatives, checked against primary sources

**raylib-rs: eliminated.** Version 6.0.0 published 2026-06-10, and actively developed. But its [README](https://github.com/raylib-rs/raylib-rs/blob/master/README.md) platform matrix marks Android with a cross for every single API surface (`raylib` core, `raygui`, `rlgl`, `software_renderer`), and states plainly:

> Android is **not** supported.

That is decisive and needs no further analysis.

**ggez: eliminated.** Repo is alive (last push 2026-08-24, "Update `wgpu`, `glam` and other deps"), but its [README](https://github.com/ggez/ggez/blob/master/README.md) says:

> - Fully supported: Windows, Linux, MacOS
> - Not officially supported but might work anyway: Android, iOS, Web

"Might work anyway" is not a basis for a shipping Android target.

**comfy: eliminated.** A wgpu plus winit 2D engine whose listed targets are Windows, Linux, macOS and WASM. No Android.

**Bevy: already rejected, and correctly.** 0.20.0 published 2026-10-08, extremely healthy. But it was rejected in the parent map as redundant and heavyweight, and that reasoning holds: the Core Sim owns all logic and depends on `glam` alone, so an ECS buys nothing and imports a very large dependency tree.

**wgpu + winit + android-activity: the real alternative, and still the wrong trade.** All three are well maintained: `wgpu` 30.0.1 (2026-08-22), `android-activity` 0.6.1 (2026-03-24, 28.3M downloads), `winit` 0.30.13 stable (2026-03-02). Two concrete objections:

1. **`winit` 0.31 is still in beta.** Latest is `0.31.0-beta.3` (2026-09-04), with betas running since 2025-11-16. Adopting this stack means choosing between an aging stable line and a long-running beta.
2. **It hands the Android surface-loss problem straight back to us.** This is the decisive point, and it is exactly the historical problem this ticket asked about. The [`winit` 0.30.13 `ApplicationHandler` docs](https://docs.rs/winit/0.30.13/winit/application/trait.ApplicationHandler.html) state:

   > On Android, the `Suspended` event is only sent when the application's associated `SurfaceView` is destroyed. [...] **After being `Suspended` on Android applications must drop all render surfaces before the event callback completes**, which may be re-created when the application is next `Resumed`.

   and for `resumed`:

   > Applications must also assume that if they are `Suspended`, then their render surfaces are invalid and should be dropped.

   So every `wgpu::Surface` must be dropped and rebuilt around the lifecycle, by us. Macroquad already solves this internally by preserving the EGL context and swapping only the `EGLSurface` (section 2). Additionally, `android-activity`'s [README](https://github.com/rust-mobile/android-activity/blob/main/README.md) warns that `android_main` "may be called multiple times if your Activity is destroyed and recreated", requiring `OnceLock` guards on all global state.

   For a project where the engine is a thin shell over a `glam`-only simulation, taking on manual surface lifecycle management plus activity-recreation safety to gain capabilities we do not need is a clear net loss.

**No newer entrant found.** A search for current thin 2D Rust engines with Android support turned up only RayQuad (a Macroquad fork restarting at 0.1.0 with a Raylib-shaped API, inheriting Android support *from miniquad*, so not an independent option) alongside the already-rejected candidates. Nothing new and better-maintained exists for this niche.

### Why the risk is contained

The parent map's locked boundary is doing real work here. Because the Core Sim depends on `glam` and nothing else, with zero engine types, Macroquad is confined to drawing and polling input. If it stalls, the replacement cost is one crate (Shell), not the game. That is the right structure given a community-maintained engine, and it is why "yes" is a safe answer rather than a hopeful one.

---

## Recommended build pipeline

**`cargo-ndk` for the native libraries plus a hand-owned Gradle shell carrying miniquad's two Java files.** This is the only option that honours NDK r30, `minSdk 24` and both shipping ABIs while keeping every version under our control. `cargo-quad-apk` from git is the fallback if the Gradle shell proves more work than expected.

**Prerequisite, not yet satisfied on this machine.** APK packaging needs SDK components that are absent, and `/opt/android-sdk` is root-owned and not writable, so this step needs the operator and a password:

```sh
# Needs sudo or a user-writable SDK. NOT runnable by the research agent.
# Note: sdkmanager now prints a deprecation notice pointing at the new `android` CLI.
sudo /opt/android-sdk/cmdline-tools/latest/bin/sdkmanager \
    --sdk_root=/opt/android-sdk \
    "platforms;android-36" "build-tools;36.0.0" "platform-tools"

# Newer equivalent via the replacement CLI:
sudo /opt/android-sdk/cmdline-tools/latest/bin/android --sdk=/opt/android-sdk \
    sdk install "platforms;android-36" "build-tools;36.0.0"
```

### Step 1: Shell crate manifest

```toml
# crates/shell/Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
macroquad = "0.4.16"

# Upstream tip: makes dependencies compile at opt-level 3 even in debug,
# which keeps asset loading fast without hurting Shell compile times.
[profile.dev.package."*"]
opt-level = 3
```

### Step 2: The mandatory `quad_main` shim

Without this, `System.loadLibrary` fails at runtime with `UnsatisfiedLinkError: cannot locate symbol "quad_main"`. It does **not** fail at compile or link time. Put it in the crate root:

```rust
// crates/shell/src/lib.rs

/// miniquad declares `extern "C" { fn quad_main(); }` and calls it from
/// Java_quad_native_QuadNative_activityOnCreate. cargo-quad-apk injects this
/// shim automatically; in a cargo-ndk + Gradle build we must provide it.
#[no_mangle]
pub extern "C" fn quad_main() {
    main();
}

#[macroquad::main("KingTongue2")]
async fn main() {
    // Shell: interpolate the two most recent Core Sim states and draw.
    loop {
        macroquad::prelude::next_frame().await
    }
}
```

### Step 3: Build both shipping ABIs

```sh
export ANDROID_NDK_HOME=/opt/android-ndk
export ANDROID_HOME=/opt/android-sdk

cargo ndk \
    -t arm64-v8a \
    -t armeabi-v7a \
    --platform 24 \
    -o android/app/src/main/jniLibs \
    build --release
```

The flag is `--platform` (short form capital `-P`); lowercase `-p` is cargo's `--package` and panics. `-o` writes the exact `jniLibs/<abi>/` tree Gradle consumes. **Verified working on this machine** against `macroquad 0.4.16` with rustc 1.99.0 and NDK r30.

Verify the two symbols that matter:

```sh
nm -D --defined-only android/app/src/main/jniLibs/arm64-v8a/libshell.so | grep quad_main
nm -D --defined-only android/app/src/main/jniLibs/arm64-v8a/libshell.so | grep QuadNative | wc -l   # expect 11
```

### Step 4: Vendor miniquad's Java glue

Copy both files from miniquad `0.4.11`, pinned to the same version as the `miniquad` dependency, and re-copy on every miniquad bump:

```sh
mkdir -p android/app/src/main/java/quad_native
mkdir -p android/app/src/main/java/rust/kingtongue2

curl -sL -o android/app/src/main/java/quad_native/QuadNative.java \
    https://raw.githubusercontent.com/not-fl3/miniquad/0.4.11/java/QuadNative.java

curl -sL -o android/app/src/main/java/rust/kingtongue2/MainActivity.java \
    https://raw.githubusercontent.com/not-fl3/miniquad/0.4.11/java/MainActivity.java
```

Then edit `MainActivity.java`, which ships as a template with two placeholders and miniquad's `//%` plugin markers:

- `package TARGET_PACKAGE_NAME;` becomes `package rust.kingtongue2;`
- `System.loadLibrary("LIBRARY_NAME")` becomes `System.loadLibrary("shell")`, matching the crate's `lib.name`
- leave every `//% MARKER` comment in place; with no plugins they are inert

`QuadNative.java` must stay in package `quad_native` verbatim. Its own comment explains why: JNI symbol names are derived from the package, miniquad exports the fixed `Java_quad_1native_QuadNative_*` names, so renaming that package breaks every callback.

### Step 5: Gradle shell

```gradle
// android/app/build.gradle
android {
    namespace 'rust.kingtongue2'
    compileSdk 36

    defaultConfig {
        applicationId "rust.kingtongue2"
        minSdk 24          // locked; we control it here, unlike cargo-quad-apk
        targetSdk 36
        ndk { abiFilters 'arm64-v8a', 'armeabi-v7a' }
    }

    sourceSets {
        main {
            jniLibs.srcDirs = ['src/main/jniLibs']
            java.srcDirs = ['src/main/java']
        }
    }
}
```

```xml
<!-- android/app/src/main/AndroidManifest.xml -->
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <uses-feature android:glEsVersion="0x00030000" android:required="true" />
    <application android:label="King Tongue 2" android:hardwareAccelerated="true">
        <activity
            android:name="rust.kingtongue2.MainActivity"
            android:configChanges="orientation|keyboardHidden|screenSize"
            android:screenOrientation="landscape"
            android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
```

### Step 6: Assemble, install, watch

```sh
cd android && ./gradlew assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n rust.kingtongue2/rust.kingtongue2.MainActivity

# miniquad logs under the "SAPP" tag; panics arrive via its android_panic_hook
adb logcat -c && adb logcat SAPP:V RustPanic:V *:S
```

Surface lifecycle can be watched directly, since `MainActivity.java` logs `surfaceCreated`, `surfaceChanged` and `surfaceDestroyed` under `SAPP`. Background and foreground the app and confirm the destroy/create pair, with no GL errors after.

### Fallback pipeline: `cargo-quad-apk` from git

Only if the Gradle shell stalls. **Install from git, never crates.io**: the 2022 release lacks the NDK 29+ `lib64` fallback that r30 requires.

```sh
cargo install --git https://github.com/not-fl3/cargo-quad-apk

export NDK_HOME=/opt/android-ndk      # note: NDK_HOME, not ANDROID_NDK_HOME
export ANDROID_HOME=/opt/android-sdk
export PATH="$ANDROID_HOME/build-tools/36.0.0:$PATH"   # for apksigner

cargo quad-apk build --release
# APK lands in target/android-artifacts/release/apk
```

with, in `Cargo.toml`:

```toml
[package.metadata.android]
build_targets = ["aarch64-linux-android", "armv7-linux-androideabi"]
min_sdk_version = 24
target_sdk_version = 36
android_version = 36
opengles_version_major = 3
opengles_version_minor = 0
fullscreen = true
```

Accept two caveats: this path builds a `bin` target rewritten to `cdylib` (so `quad_main` is injected and the shim above is not needed), and its hardcoded `d8 --min-api 26` contradicts `min_sdk_version = 24`.

### Required Shell-side resume handling

Non-negotiable, per section 2:

```rust
const MAX_FRAME_DELTA: f32 = 0.25;  // ~15 fixed steps; covers a hitch, not a suspend
const FIXED_DT: f32 = 1.0 / 60.0;
const MAX_STEPS_PER_FRAME: u32 = 15;

// macroquad computes frame_time inside draw(), and draw() is skipped while the
// Android surface is destroyed. The first frame after resume therefore reports
// the entire suspend duration. Unclamped, a 3-minute backgrounding yields
// ~10,800 catch-up steps in one frame.
let dt = get_frame_time().min(MAX_FRAME_DELTA);
accumulator += dt;

let mut steps = 0;
while accumulator >= FIXED_DT && steps < MAX_STEPS_PER_FRAME {
    sim.step(FIXED_DT);
    accumulator -= FIXED_DT;
    steps += 1;
}
```

---

## Open risks

1. **BLOCKING: no APK can be built on this machine today.** `/opt/android-sdk` contains only `cmdline-tools/latest` and `ndk`. There is **no `build-tools`** directory (so no `aapt2`, `zipalign` or `apksigner`) and **no `platforms`** directory (so no `android.jar`). Verified: `ls /opt/android-sdk/build-tools/` and `ls /opt/android-sdk/platforms/` both return "No such file or directory". Worse, `/opt/android-sdk` is root-owned and **not writable** by the user, and `sdkmanager --list_installed` fails with `java.nio.file.AccessDeniedException: /opt/android-sdk/.sdk`. Every packaging tool fails at this hurdle, `cargo-quad-apk` immediately (it reads `sdk/build-tools` to pick a version). **Needs operator action with sudo.** Only the `.so` cross-compile is proven; APK packaging is entirely unverified.
2. **Nothing was run on a real device or emulator.** All runtime claims in sections 2, 3 and 4 derive from reading upstream source and the issue tracker, not from execution. Specifically unverified: that suspend/resume preserves GL state in practice; that touch ids behave as the code implies; actual frame pacing. The first device smoke test should background and foreground the app repeatedly.
3. **The actual `GL_VERSION` string on target hardware is unverified.** miniquad requests `EGL_CONTEXT_CLIENT_VERSION = 2` and enables instancing and GLSL `300 es` only if the driver reports `"OpenGL ES 3"`. Evidence from miniquad [#572](https://github.com/not-fl3/miniquad/issues/572) implies Android devices do report ES3, but this was not observed directly. Consequence if wrong: instancing silently disabled and `draw_calls` falls back to non-instanced paths, a performance matter rather than a correctness one.
4. **`cargo-quad-apk`'s `d8 --min-api 26` contradicts `minSdk 24`.** Hardcoded in `src/ops/build.rs`. If the fallback pipeline is ever adopted, verify install and launch on an actual API 24 or 25 device, or patch the fork. The recommended Gradle pipeline avoids this entirely.
5. **Background CPU spin.** While the surface is destroyed, miniquad's render loop has no `eglSwapBuffers` to throttle it and only calls `thread::yield_now()`, pegging one core while the app sits in the background. Battery and thermal impact, not a crash. Mitigations (`blocking_event_loop`, `sleep_interval_ms`) exist but are untested, and `sleep_interval_ms` is documented as applying only when `blocking_event_loop` is enabled.
6. **Miniquad [#572](https://github.com/not-fl3/miniquad/issues/572) remains open** (since 2025-08-01). Plain Macroquad is structurally immune because all user code runs in `draw()`, which is surface-gated. **This immunity is lost the moment anything implements `miniquad::EventHandler` directly or allocates GL resources outside the Macroquad draw path.** Treat this as a standing architectural constraint on the Shell crate, and record it as such.
7. **Vendored Java glue must be re-synced on every miniquad bump.** `MainActivity.java` and `QuadNative.java` are copied out of the miniquad repo and are not version-checked by anything. A miniquad upgrade that changes the JNI surface (for example the eleven `QuadNative` methods) will fail at runtime, not at compile time. Pin the `curl` URLs to the exact miniquad tag, as shown above, and re-copy deliberately.
8. **`javac -source/-target 1.8` is living on borrowed time.** `cargo-quad-apk` hardcodes it; Java 26 accepts it but warns "source value 8 is obsolete and will be removed in a future release". A future JDK will break the fallback pipeline. The Gradle path sets its own `compileOptions` and is unaffected.
9. **`sdkmanager` is now deprecated.** Running it prints: "The SDK Manager CLI tool (sdkmanager) is deprecated. Android CLI will be used instead. [...] 'android sdk' is the replacement for 'sdkmanager'." Scripted SDK provisioning should migrate to the [`android` CLI](https://developer.android.com/tools/agents/android-cli) (page last updated 2026-09-25). That documentation page does **not** itself confirm the deprecation, so this rests on the installed tool's own output.
10. **Macroquad's bus factor and issue backlog.** Community-carried rather than author-driven, 347 plus 183 open issues, `0.5` branch stalled since 2024-09-22, no releases or changelog published, Android docs effectively dropped. Not a reason to reject it, but it is a reason to keep the Shell boundary strict and to pin `macroquad = "0.4.16"` exactly rather than tracking a range. Note that `macroquad 0.4.16` itself pins `miniquad = "=0.4.11"` exactly, so miniquad cannot drift underneath us without a macroquad bump.
11. **Signing and release packaging remain out of scope.** The parent map already lists this. The commands above produce a debug APK for `adb` sideloading only.

---

## Sources

All checked 2026-10-11. Versions and dates come from the crates.io API, the GitHub API, and Docker Hub's tag API.

Upstream repositories and source files:
- [not-fl3/macroquad](https://github.com/not-fl3/macroquad): 0.4.16 (2026-07-30), repo pushed 2026-08-18, 347 open issues. [README](https://github.com/not-fl3/macroquad/blob/master/README.md), [src/lib.rs](https://github.com/not-fl3/macroquad/blob/master/src/lib.rs), [src/input.rs](https://github.com/not-fl3/macroquad/blob/master/src/input.rs), [src/quad_gl.rs](https://github.com/not-fl3/macroquad/blob/master/src/quad_gl.rs)
- [not-fl3/miniquad](https://github.com/not-fl3/miniquad): 0.4.11 (2026-06-03), repo pushed 2026-07-25, 183 open issues, MSRV 1.70. [README](https://github.com/not-fl3/miniquad/blob/master/README.md), [src/native/android.rs](https://github.com/not-fl3/miniquad/blob/master/src/native/android.rs), [src/native/egl.rs](https://github.com/not-fl3/miniquad/blob/master/src/native/egl.rs), [src/graphics/gl.rs](https://github.com/not-fl3/miniquad/blob/master/src/graphics/gl.rs), [src/conf.rs](https://github.com/not-fl3/miniquad/blob/master/src/conf.rs), [src/native/android/mod_inject.rs](https://github.com/not-fl3/miniquad/blob/master/src/native/android/mod_inject.rs), [java/MainActivity.java](https://github.com/not-fl3/miniquad/blob/master/java/MainActivity.java), [java/QuadNative.java](https://github.com/not-fl3/miniquad/blob/master/java/QuadNative.java)
- [not-fl3/cargo-quad-apk](https://github.com/not-fl3/cargo-quad-apk): crates.io 0.1.4 (2022-07-04), repo pushed 2026-07-28. [README](https://github.com/not-fl3/cargo-quad-apk/blob/master/README.md), [Dockerfile](https://github.com/not-fl3/cargo-quad-apk/blob/master/Dockerfile), [src/ops/build.rs](https://github.com/not-fl3/cargo-quad-apk/blob/master/src/ops/build.rs), [src/ops/build/compile.rs](https://github.com/not-fl3/cargo-quad-apk/blob/master/src/ops/build/compile.rs). NDK 29 `lib64` fix: commit `ca23fe59e` (2026-07-28)
- [bbqsrc/cargo-ndk](https://github.com/bbqsrc/cargo-ndk): 4.1.2 (2025-08-09), repo pushed 2026-10-10, 5 open issues
- [rust-mobile/cargo-apk](https://github.com/rust-mobile/cargo-apk): 0.10.0 (2023-11-30), last commit 2025-07-10, 41 open issues
- [rust-mobile/xbuild](https://github.com/rust-mobile/xbuild): 0.2.0 (2022-12-21), last commit 2025-08-15, description self-declares "(unmaintained)"
- [rust-mobile/android-activity](https://github.com/rust-mobile/android-activity): 0.6.1 (2026-03-24), MSRV 1.85.0. [README](https://github.com/rust-mobile/android-activity/blob/main/README.md)

Issue tracker evidence:
- [miniquad #572](https://github.com/not-fl3/miniquad/issues/572): Android SIGSEGV in `glDrawElementsInstanced`, opened 2025-08-01, last activity 2025-11-30, **open**. The null-EGL-context diagnosis and the ES3-on-Android corroboration
- [macroquad #1036](https://github.com/not-fl3/macroquad/issues/1036): "How to build apk / aab", opened 2026-04-24, **open, zero comments**
- [macroquad #1069](https://github.com/not-fl3/macroquad/issues/1069): "MSAA arm86 > opengl translation miss", opened 2026-09-06

Alternatives:
- [raylib-rs](https://github.com/raylib-rs/raylib-rs): 6.0.0 (2026-06-10). [README](https://github.com/raylib-rs/raylib-rs/blob/master/README.md): "Android is **not** supported"
- [ggez](https://github.com/ggez/ggez): repo pushed 2026-08-24. [README](https://github.com/ggez/ggez/blob/master/README.md): Android "not officially supported but might work anyway"
- [winit 0.30.13 `ApplicationHandler`](https://docs.rs/winit/0.30.13/winit/application/trait.ApplicationHandler.html): the Android `suspended`/`resumed` surface-dropping contract. Stable 0.30.13 (2026-03-02); 0.31.0-beta.3 (2026-09-04)
- [wgpu](https://github.com/gfx-rs/wgpu): 30.0.1 (2026-08-22)
- [bevy](https://github.com/bevyengine/bevy): 0.20.0 (2026-10-08). Rejected in the parent map
- [Docker Hub `notfl3/cargo-apk`](https://hub.docker.com/r/notfl3/cargo-apk): `latest` tag pushed 2024-07-21
- [Android CLI documentation](https://developer.android.com/tools/agents/android-cli): page last updated 2026-09-25

Local verification on this machine (2026-10-11): rustc 1.99.0 (b940084d7, 2026-09-28), cargo 1.99.0, OpenJDK 26.0.2.1 (2026-08-18), NDK r30 (`Pkg.Revision = 30.0.16248370`), `cargo-ndk` 4.1.2 at `~/.cargo/bin`. Throwaway crate built in the session scratchpad, not committed.
