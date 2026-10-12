//! The Shell: rendering, input, and the Android entry point. Owns no game state.
//!
//! ADR-0001 puts all state in `king_tongue_sim`; this crate reads snapshots and
//! feeds back Gesture intents. ADR-0004 picks Macroquad. ADR-0005 packages with
//! cargo-ndk plus a hand-owned Gradle shell. ADR-0009 fixes the frame contract.

// The mechanism that stops a new entity silently failing to render. Adding a
// `BodyKind` variant breaks the match in `draw_body`, and these lints forbid
// papering over that break with a catch-all arm. They are only a build failure
// rather than advice because CI lints the whole workspace with `-D warnings`;
// until that step was widened it ran `-p king_tongue_sim` and never reached
// this crate, so the guard was half-wired. Keep the two in step: a crate-scoped
// clippy step turns these back into comments.
//
// Both lints, and the second is not redundant: ticket #20 checked this guard by
// adding a variant and trying to silence the break, and found that
// `wildcard_enum_match_arm` does not fire when the wildcard covers exactly one
// missing variant. Clippy reports that case under
// `match_wildcard_for_single_variants` instead, which is a `pedantic` lint and
// so off by default. One missing variant is the case that actually happens: a
// ticket adds one `BodyKind` at a time. Without the second lint the first was
// a guard against adding two entities and nothing else.
#![warn(clippy::wildcard_enum_match_arm)]
#![warn(clippy::match_wildcard_for_single_variants)]

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

/// Screen pixels per world unit: the Camera's fixed zoom.
///
/// ADR-0013 deliberately leaves the zoom factor open and calls this constant a
/// placeholder, so this is a bound and not an answer. The bound it has to meet
/// is that the whole level is never visible at once. At 64 pixels per unit even
/// a 3840 pixel wide window shows 60 world units across, while the shipped
/// scene's terrain spans 82 and its Kill Bound 160, so the player always sees a
/// slice. Lowering this below roughly 48 breaks that on a wide desktop monitor.
const PIXELS_PER_UNIT: f32 = 64.0;

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

/// Where world-space origin lands on screen so that the hero sits in the middle
/// of it.
///
/// This is the whole Camera, and ADR-0013 puts it here rather than in the Core
/// Sim because it is view state: it changes what the player sees and never what
/// the simulation computes.
///
/// It reads the **interpolated** snapshot and not the tick state. Following the
/// raw tick position judders visibly on a high refresh rate panel even though
/// the simulation underneath is correct, because the Camera would then step at
/// 60Hz while the panel draws at 120.
///
/// Stateless, which is what ADR-0013 decided: position straight from the hero
/// every frame, no damping and no lookahead. The ADR records why taking the
/// damped escape hatch later would be legitimate.
pub fn camera_origin(
    view: &Snapshot,
    hero: sim::EntityId,
    screen: macroquad::math::Vec2,
) -> macroquad::math::Vec2 {
    let centre = screen * 0.5;
    // Inverting `to_screen` for the hero's position: whatever origin puts the
    // hero at the centre of the screen. The fallback is reachable: the Kill
    // Bound removes any body that leaves the level's `world_bounds`, the hero
    // included, and a removed body is simply absent from the Snapshot. Drawing
    // the world origin at the centre is the inert choice, and it stays a
    // drawing fallback rather than a game rule: the Shell learns nothing about
    // despawning and decides nothing because of it.
    view.body(hero).map_or(centre, |body| {
        macroquad::math::Vec2::new(
            centre.x - body.pos.x * PIXELS_PER_UNIT,
            centre.y + body.pos.y * PIXELS_PER_UNIT,
        )
    })
}

/// Draws the level's static terrain.
///
/// Terrain is not in the `Snapshot` because it never moves, so the Shell draws
/// it straight from the `Level` it loaded. Without it the hero falls and lands
/// in a black void with nothing to land on, which is a rendering gap and not a
/// simulation one.
fn draw_terrain(terrain: &[sim::Aabb], origin: macroquad::math::Vec2) {
    for aabb in terrain {
        // Screen y grows downward, so a box's upper left on screen is its
        // minimum x and its maximum y in the world.
        let corner = to_screen(glam::Vec2::new(aabb.min().x, aabb.max().y), origin);
        let size = aabb.half_extents * 2.0 * PIXELS_PER_UNIT;
        macroquad::shapes::draw_rectangle(
            corner.x,
            corner.y,
            size.x,
            size.y,
            macroquad::color::DARKGRAY,
        );
    }
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

        // The Core Sim owns every piece of state from here on. The Shell holds
        // the clock, the touch slots, the interpolation buffer, and the Camera,
        // and nothing else.
        let mut world = sim::World::from_level(&level);
        let hero = world.hero_id();

        let mut accumulator = Accumulator::new();
        // Reused so rendering allocates nothing per frame. The two snapshots
        // ADR-0002 requires live in the `World`, which keeps them in step with
        // the ticks that produced them.
        let mut view = Snapshot::default();
        // Likewise reused. Ten fingers is more than Android reports and far
        // more than ADR-0006 can use.
        let mut touches: Vec<sim::TouchSample> = Vec::with_capacity(10);

        // The Camera origin a touch is converted through is the one that was on
        // screen when the finger landed, which is last frame's. That is the
        // honest answer rather than a convenience: the player aimed at what
        // they could see, and the frame they aimed at has already been drawn.
        let mut screen = macroquad::math::Vec2::new(
            macroquad::window::screen_width(),
            macroquad::window::screen_height(),
        );
        let mut origin = camera_origin(world.snapshot(), hero, screen);

        loop {
            // Wall-clock time enters the simulation here and nowhere else.
            // `feed` clamps catch-up to MAX_CATCHUP_TICKS, which is what keeps a
            // resume from suspend from running thousands of ticks in one frame.
            let ticks = accumulator.feed(macroquad::time::get_frame_time());

            // Input enters the simulation here and nowhere else, and it enters
            // raw. A Tongue fired now appears in the Snapshot the next Sim Tick
            // writes, so it is drawn from the first tick it exists for.
            collect_touches(origin, &mut touches);
            world.apply_touches(&touches);

            for _ in 0..ticks {
                world.step();
            }

            sim::frame::interpolate_into(
                world.previous_snapshot(),
                world.snapshot(),
                accumulator.alpha(),
                &mut view,
            );

            macroquad::window::clear_background(macroquad::color::BLACK);
            screen = macroquad::math::Vec2::new(
                macroquad::window::screen_width(),
                macroquad::window::screen_height(),
            );
            // The Camera follows the interpolated hero, per ADR-0013. Kept for
            // the next frame's touch conversion as well as this frame's
            // drawing, so a tap is read against the view it was aimed at.
            origin = camera_origin(&view, hero, screen);
            draw_terrain(&level.static_terrain, origin);
            draw_frame(&view, origin);
            macroquad::window::next_frame().await;
        }
    });
}

/// Converts Macroquad's touch set into the Core Sim's platform-neutral one.
///
/// This is the entire input job of the Shell, and the absence of anything else
/// here is the point. Which finger claims which Tongue slot, what a third
/// concurrent touch does, which surface a tap Attaches to, and whether a lifted
/// finger Releases anything are all decisions behind `World::apply_touches`,
/// where they are headless-testable. The spec makes that a standing constraint:
/// the Shell is untested by design, so a conditional here that decided a game
/// outcome would be a defect.
///
/// What remains is platform conversion. The phase mapping is a rename between
/// two identical enums, and the position mapping is screen pixels to world
/// units through the Camera origin. Neither looks at game state.
///
/// Keyed by `id` and never by position in the set: `touches()` collects from a
/// `HashMap`, so its iteration order is not stable between frames. Writing into
/// a caller-owned buffer so polling allocates nothing per frame.
pub fn collect_touches(origin: macroquad::math::Vec2, out: &mut Vec<sim::TouchSample>) {
    use macroquad::input::TouchPhase as Mq;
    out.clear();
    for touch in macroquad::input::touches() {
        out.push(sim::TouchSample {
            id: touch.id,
            phase: match touch.phase {
                Mq::Started => sim::TouchPhase::Started,
                Mq::Moved => sim::TouchPhase::Moved,
                Mq::Stationary => sim::TouchPhase::Stationary,
                Mq::Ended => sim::TouchPhase::Ended,
                Mq::Cancelled => sim::TouchPhase::Cancelled,
            },
            pos: from_screen(touch.position, origin),
        });
    }
}
