//! The Shell: rendering, input, and the Android entry point. Owns no game state.
//!
//! ADR-0001 puts all state in `king_tongue_sim`; this crate reads snapshots and
//! feeds back Gesture intents. ADR-0004 picks Macroquad. ADR-0005 packages with
//! cargo-ndk plus a hand-owned Gradle shell. ADR-0009 fixes the frame contract.

// The mechanism that stops a new entity silently failing to render. Adding a
// `BodyKind` variant breaks the match in `draw_body`, and this lint forbids
// papering over that break with a catch-all arm. CI runs clippy with
// `-D warnings`, so it is a build failure rather than advice.
#![warn(clippy::wildcard_enum_match_arm)]

use king_tongue_sim as sim;
use sim::{Accumulator, BodyKind, Snapshot};

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

/// World units per screen pixel at the default zoom.
const PIXELS_PER_UNIT: f32 = 24.0;

/// Converts a Core Sim position into Macroquad screen space.
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
pub fn to_screen(v: glam::Vec2, origin: macroquad::math::Vec2) -> macroquad::math::Vec2 {
    // Screen y grows downward, world y grows upward.
    macroquad::math::Vec2::new(
        origin.x + v.x * PIXELS_PER_UNIT,
        origin.y - v.y * PIXELS_PER_UNIT,
    )
}

/// Converts a Macroquad position, such as a touch point, into a Core Sim one.
#[inline]
pub fn from_screen(v: macroquad::math::Vec2, origin: macroquad::math::Vec2) -> glam::Vec2 {
    glam::Vec2::new(
        (v.x - origin.x) / PIXELS_PER_UNIT,
        (origin.y - v.y) / PIXELS_PER_UNIT,
    )
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

/// Draws one interpolated body.
///
/// The match is deliberately exhaustive with no catch-all arm. That is the whole
/// enforcement mechanism for "adding an entity cannot silently fail to render".
fn draw_body(body: &sim::Body, origin: macroquad::math::Vec2) {
    use macroquad::color;
    let p = to_screen(body.pos, origin);
    let r = sim::PLAYER_RADIUS * PIXELS_PER_UNIT;
    let tint = match body.kind {
        BodyKind::Hero => color::SKYBLUE,
        BodyKind::Enemy => color::GRAY,
        BodyKind::Thrown => color::ORANGE,
    };
    macroquad::shapes::draw_circle(p.x, p.y, r, tint);
    // Facing, drawn from the unit vector the contract carries rather than from an
    // angle, so it is never reconstructed through atan2 and re-wrapped.
    let tip = to_screen(body.pos + body.facing * sim::PLAYER_RADIUS * 1.8, origin);
    macroquad::shapes::draw_line(p.x, p.y, tip.x, tip.y, 2.0, color::WHITE);
}

fn draw_frame(view: &Snapshot, origin: macroquad::math::Vec2) {
    use macroquad::color;
    for tongue in &view.tongues {
        let a = to_screen(tongue.from, origin);
        let b = to_screen(tongue.to, origin);
        macroquad::shapes::draw_line(a.x, a.y, b.x, b.y, 3.0, color::PINK);
    }
    for body in &view.bodies {
        draw_body(body, origin);
    }
}

/// Reads a level through Macroquad's asset loader and parses it in the Core Sim.
///
/// The split is the point: the Shell does the I/O, which on Android means APK
/// assets rather than a filesystem path, and hands the Core Sim raw bytes. The
/// Core Sim never learns which platform it is on. ADR-0001, ADR-0010.
pub async fn load_level(path: &str) -> Result<sim::Level, String> {
    let bytes = macroquad::file::load_file(path)
        .await
        .map_err(|e| format!("could not read level {path}: {e}"))?;
    sim::Level::from_bytes(&bytes).map_err(|e| format!("level {path} is unusable: {e}"))
}

/// Holds a readable error on screen. Android gives us no terminal to print to.
async fn fatal_screen(message: &str) {
    loop {
        macroquad::window::clear_background(macroquad::color::MAROON);
        macroquad::text::draw_text(message, 20.0, 40.0, 22.0, macroquad::color::WHITE);
        macroquad::window::next_frame().await;
    }
}

/// The Shell's entry point, shared by the Android cdylib and the desktop bin.
pub fn run() {
    macroquad::Window::from_config(window_conf(), async move {
        // Macroquad synthesises mouse events from touches by default, which would
        // double-report every Gesture. Research on ticket #2 confirmed this
        // defaults to true, so it must be turned off explicitly.
        macroquad::input::simulate_mouse_with_touch(false);

        // On Android, assets resolve from the root of the APK's assets folder;
        // on desktop they resolve relative to the working directory. This call
        // makes one path string work on both, and it must happen before the
        // first load. See ADR-0010.
        macroquad::file::set_pc_assets_folder("assets");

        let level = match load_level("levels/test_scene.ron").await {
            Ok(level) => level,
            Err(e) => {
                // A missing or malformed level is unrecoverable, but it must not
                // be a silent black screen: on Android there is no stderr to
                // read, so the message has to reach the display.
                macroquad::logging::error!("{e}");
                fatal_screen(&e).await;
                return;
            }
        };
        macroquad::logging::info!(
            "level loaded: {} terrain, {} enemies",
            level.static_terrain.len(),
            level.enemy_spawns.len()
        );

        let mut accumulator = Accumulator::new();
        // The two snapshots ADR-0002 requires, plus a reused interpolation buffer
        // so rendering allocates nothing per frame.
        let mut previous = Snapshot::default();
        let mut current = Snapshot::default();
        let mut view = Snapshot::default();
        let mut claimed: [Option<u64>; sim::MAX_TONGUES] = [None; sim::MAX_TONGUES];

        loop {
            // Wall-clock time enters the simulation here and nowhere else.
            // `feed` clamps catch-up to MAX_CATCHUP_TICKS, which is what keeps a
            // resume from suspend from running thousands of ticks in one frame.
            let ticks = accumulator.feed(macroquad::time::get_frame_time());

            let _intents = poll_gestures(&mut claimed);

            for _ in 0..ticks {
                // Rotate before stepping, so `previous` is always the tick before
                // `current` and never the same tick.
                core::mem::swap(&mut previous, &mut current);
                // World stepping lands here. The Core Sim produces `current`.
                current.tick = previous.tick.wrapping_add(1);
            }

            sim::frame::interpolate_into(&previous, &current, accumulator.alpha(), &mut view);

            macroquad::window::clear_background(macroquad::color::BLACK);
            let origin = macroquad::math::Vec2::new(
                macroquad::window::screen_width() * 0.5,
                macroquad::window::screen_height() * 0.5,
            );
            draw_frame(&view, origin);
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
    // Gesture state machine lands with the level data format work; the frame
    // contract it hangs off is settled here.
    Vec::new()
}
