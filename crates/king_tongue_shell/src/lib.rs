//! The Shell: rendering, input, and the Android entry point. Owns no game state.
//!
//! ADR-0001 puts all state in `king_tongue_sim`; this crate reads it and feeds it
//! Gesture intents. ADR-0004 picks Macroquad. ADR-0005 packages with cargo-ndk
//! plus a hand-owned Gradle shell.

use king_tongue_sim as sim;

/// Android entry point, and a trap worth understanding before touching it.
///
/// miniquad declares `extern "C" { fn quad_main(); }` and calls it from
/// `activityOnCreate`, but it never defines it. `cargo-apk` style tooling injects
/// this shim invisibly; our cargo-ndk pipeline does not, so we must supply it.
///
/// Omit it and `System.loadLibrary` fails at runtime with
/// `UnsatisfiedLinkError: cannot locate symbol "quad_main"`, with no compile error
/// and no link error, because cdylib links tolerate undefined symbols. See ADR-0005.
///
/// Note this calls `run`, not `main`: a cdylib has no `main`.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn quad_main() {
    run();
}

/// Converts a Core Sim position into a Macroquad one.
///
/// These are different types despite sharing a name. Macroquad 0.4.16 depends on
/// `glam 0.27`, while the workspace and the Core Sim use `glam 0.30`, so both are
/// linked and `glam::Vec2` means two incompatible things depending on which crate
/// you are in. Converting explicitly here is deliberate: it keeps the Core Sim
/// free to track `glam` independently of Macroquad's release cadence, and it stops
/// simulation types leaking into draw calls, which is what ADR-0001 is for.
///
/// The cost is a second `glam` in the binary. If that ever matters more than the
/// decoupling, pin the workspace to Macroquad's `glam` and delete this.
#[inline]
pub fn to_screen(v: glam::Vec2) -> macroquad::math::Vec2 {
    macroquad::math::Vec2::new(v.x, v.y)
}

/// Converts a Macroquad position, such as a touch point, into a Core Sim one.
#[inline]
pub fn from_screen(v: macroquad::math::Vec2) -> glam::Vec2 {
    glam::Vec2::new(v.x, v.y)
}

/// Window and surface configuration. Kept here so the desktop bin and the Android
/// cdylib cannot drift apart.
pub fn window_conf() -> macroquad::window::Conf {
    macroquad::window::Conf {
        window_title: "King Tongue 2".to_owned(),
        high_dpi: true,
        ..Default::default()
    }
}

/// The Shell's entry point, shared by the Android cdylib and the desktop bin.
pub fn run() {
    macroquad::Window::from_config(window_conf(), async move {
        // Macroquad synthesises mouse events from touches by default, which would
        // double-report every Gesture. Research on ticket #2 confirmed this
        // defaults to true, so it must be turned off explicitly.
        macroquad::input::simulate_mouse_with_touch(false);

        loop {
            // Accumulator and interpolation land here once the Core Sim to Shell
            // contract is settled on ticket #7. ADR-0002 requires the accumulator
            // debt to be clamped, because Macroquad computes frame delta inside
            // draw(), which is skipped while suspended: the first post-resume delta
            // equals the whole suspend duration.
            let _ = sim::DT;

            macroquad::window::clear_background(macroquad::color::BLACK);
            macroquad::window::next_frame().await;
        }
    });
}

/// Reads the current touch set into Gesture intents.
///
/// Three constraints confirmed by the ticket #2 research, each of which is a bug
/// if ignored: `touches()` collects from a `HashMap` so iteration order is not
/// stable between frames and entries must be keyed by `id` rather than by index;
/// `Ended` and `Cancelled` are visible for exactly one frame; and a held finger
/// reports `Stationary`, never `Moved`.
///
/// Slot assignment: the first `Started` id claims a free slot, later ids take the
/// second, and any third concurrent id is discarded per ADR-0006.
pub fn poll_gestures(_claimed: &mut [Option<u64>; sim::MAX_TONGUES]) -> Vec<sim::GestureIntent> {
    // Implemented against the interpolation contract from ticket #7.
    Vec::new()
}
