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

/// Draws one interpolated body, in the shape that actually collides.
///
/// The match is deliberately exhaustive with no catch-all arm. That is the whole
/// enforcement mechanism for "adding an entity cannot silently fail to render",
/// and it is why the shape is decided inside the same match as the tint rather
/// than beside it: a new `BodyKind` has to answer both questions or fail to
/// build.
///
/// **Per kind, because the collision shapes differ.** The hero is a bounding
/// circle of `PLAYER_RADIUS`; an Enemy is a bounding box of
/// `ENEMY_HALF_EXTENTS`. Drawing every body as a circle put hits up to about
/// 0.21 units outside the drawn shape at an Enemy's corners, which is exactly
/// what Placeholder Geometry exists not to do: the sketch has to mean what it
/// looks like, or tuning is done against a lie.
///
/// A Thrown Enemy is drawn as the same box. It sweeps as its box's inscribed
/// circle when it is the projectile, which is the approximation the integrator
/// has made for every body since ticket #11, but it is still a box when it is
/// the thing being struck, and the box is the outer of the two.
fn draw_body(body: &sim::Body, origin: macroquad::math::Vec2) {
    use macroquad::color;
    let p = to_screen(body.pos, origin);
    let (tint, half_extents) = match body.kind {
        BodyKind::Hero => (color::SKYBLUE, None),
        BodyKind::Enemy => (color::GRAY, Some(sim::combat::ENEMY_HALF_EXTENTS)),
        BodyKind::Thrown => (color::ORANGE, Some(sim::combat::ENEMY_HALF_EXTENTS)),
    };
    match half_extents {
        None => {
            let r = sim::PLAYER_RADIUS * PIXELS_PER_UNIT;
            macroquad::shapes::draw_circle(p.x, p.y, r, tint);
        }
        Some(half) => {
            // Screen y grows downward, so a box's upper left on screen is its
            // minimum x and its maximum y in the world. Same conversion as
            // `draw_terrain`, which draws the other boxes in the scene.
            let corner = to_screen(body.pos + glam::Vec2::new(-half.x, half.y), origin);
            let size = half * 2.0 * PIXELS_PER_UNIT;
            macroquad::shapes::draw_rectangle(corner.x, corner.y, size.x, size.y, tint);
        }
    }
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
    // hero at the centre of the screen. Routed **through** `to_screen` rather
    // than restating its arithmetic backwards, so the transform lives in one
    // place and the Camera cannot drift away from the thing that draws.
    //
    // What makes that possible is that the origin enters `to_screen` purely
    // additively, so `to_screen(v, o)` is `to_screen(v, 0) + o` and the origin
    // that lands `v` on `centre` is `centre - to_screen(v, 0)`. Checked to be
    // bit-identical to the hand-inverted form over every screen centre a
    // window can report, which is any non-negative pair: the two differ only
    // in the sign of a zero, and only when the centre is itself negative zero.
    //
    // The fallback is reachable: the Kill Bound removes any body that leaves
    // the level's `world_bounds`, the hero included, and a removed body is
    // simply absent from the Snapshot. Drawing the world origin at the centre
    // is the inert choice, and it stays a drawing fallback rather than a game
    // rule: the Shell learns nothing about despawning and decides nothing
    // because of it.
    view.body(hero).map_or(centre, |body| {
        centre - to_screen(body.pos, macroquad::math::Vec2::ZERO)
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

        // The desktop harness's only state: what the mouse looked like last
        // frame, which is what makes a press distinguishable from a hold.
        #[cfg(not(target_os = "android"))]
        let mut mouse = MouseFrame::default();

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
            // The desktop harness, appended to the same set so the Core Sim
            // cannot tell a mouse from a finger. Compiled out on Android.
            #[cfg(not(target_os = "android"))]
            {
                mouse = append_mouse_touches(origin, mouse, &mut touches);
            }
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

// ---------------------------------------------------------------------------
// The desktop mouse harness.
//
// Compiled out on Android, so the shipping library cannot carry it. This exists
// to make the Swing playable by hand on a desktop, which is the only way the
// feel of ADR-0003's spring has ever been judged, and it is a harness rather
// than a control scheme: ADR-0006's real input is two fingers on glass.
// ---------------------------------------------------------------------------

/// How many mouse buttons drive Tongues, and so how many touch ids exist.
///
/// Two, because two concurrent Gestures are the whole point. A Gesture claims
/// the lowest slot no other finger is driving and `Gestures::claim` returns
/// nothing for an id that already owns one, so a single button returns to slot 0
/// forever and alternates fire, Release, fire. One button cannot hold two
/// Tongues however fast it is tapped.
#[cfg(not(target_os = "android"))]
pub const MOUSE_POINTERS: usize = 2;

/// Fails the build if the harness could ever drive more pointers than there are
/// slots. If `MAX_TONGUES` grows, this harness covers only the first two and
/// wants extending rather than trusting.
#[cfg(not(target_os = "android"))]
const _: () = assert!(MOUSE_POINTERS <= sim::MAX_TONGUES);

/// The touch id each mouse button drives, stable for the whole run.
///
/// Stability is load-bearing rather than tidy. Slot ownership is keyed on id, so
/// an id that changed while its button was held would read as a fresh `Started`
/// every frame and re-fire the Tongue continuously.
#[cfg(not(target_os = "android"))]
const MOUSE_IDS: [u64; MOUSE_POINTERS] = [0, 1];

/// Which mouse buttons were down when a frame was polled.
#[cfg(not(target_os = "android"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseButtons {
    /// Drives touch id 0.
    pub left: bool,
    /// Drives touch id 1.
    pub right: bool,
}

/// What the previous frame's mouse looked like, which is all the state a phase
/// decision needs.
///
/// Screen positions rather than world ones, because a phase is a screen-space
/// question: a finger held still while the Camera follows the hero is
/// `Stationary`, even though the world point under it moves every frame.
#[cfg(not(target_os = "android"))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MouseFrame {
    held: [Option<macroquad::math::Vec2>; MOUSE_POINTERS],
}

/// Maps this frame's mouse into [`sim::TouchSample`]s, and returns the state the
/// next frame needs.
///
/// Pure: it calls nothing from Macroquad, reads no global, and consults no
/// clock. Every transition is therefore reachable without a window, which is
/// what lets the first tests in this crate run headless.
///
/// The phase contract, per frame and per button:
///
/// - **Press** is `Started`, for exactly one frame.
/// - **Held** is `Stationary`, or `Moved` only when the screen position changed.
/// - **Release** is `Ended` for exactly one frame, after which the id is absent.
///
/// `Ended` is self-limiting rather than counted: a released button records no
/// held position, so the following frame sees neither a previous position nor a
/// button down and emits nothing at all.
///
/// Note what is deliberately absent. **Lifting a button does not Release a
/// Tongue.** Pressing again does, because a Gesture on an occupied slot is a
/// Release under ADR-0006. `Ended` must never be read as a Release anywhere:
/// Macroquad flips every live touch to `Ended` when the app minimizes, so that
/// reading would hand the player a free Release on every suspend.
#[cfg(not(target_os = "android"))]
pub fn mouse_to_touches(
    buttons: MouseButtons,
    position: macroquad::math::Vec2,
    origin: macroquad::math::Vec2,
    previous: MouseFrame,
) -> ([Option<sim::TouchSample>; MOUSE_POINTERS], MouseFrame) {
    let down = [buttons.left, buttons.right];
    let mut samples = [None, None];
    let mut next = MouseFrame::default();

    for slot in 0..MOUSE_POINTERS {
        let phase = match (previous.held[slot], down[slot]) {
            (None, true) => Some(sim::TouchPhase::Started),
            (Some(before), true) => Some(if before == position {
                sim::TouchPhase::Stationary
            } else {
                sim::TouchPhase::Moved
            }),
            (Some(_), false) => Some(sim::TouchPhase::Ended),
            (None, false) => None,
        };

        samples[slot] = phase.map(|phase| sim::TouchSample {
            id: MOUSE_IDS[slot],
            phase,
            pos: from_screen(position, origin),
        });

        next.held[slot] = down[slot].then_some(position);
    }

    (samples, next)
}

/// Polls the mouse and appends its pointers to this frame's touch set.
///
/// The impure half, kept as thin as it can be so everything decidable lives in
/// [`mouse_to_touches`].
///
/// A button counts as down if it is held **or** was pressed this frame. The
/// second half matters: a click that goes down and up inside one frame is
/// invisible to down-polling alone, and a dropped click is exactly what a player
/// does to fire a Tongue. Including the press event makes that click a one frame
/// `Started` followed by an `Ended`, which is what a real quick tap produces.
#[cfg(not(target_os = "android"))]
fn append_mouse_touches(
    origin: macroquad::math::Vec2,
    previous: MouseFrame,
    out: &mut Vec<sim::TouchSample>,
) -> MouseFrame {
    use macroquad::input::{is_mouse_button_down, is_mouse_button_pressed, MouseButton};

    let buttons = MouseButtons {
        left: is_mouse_button_down(MouseButton::Left) || is_mouse_button_pressed(MouseButton::Left),
        right: is_mouse_button_down(MouseButton::Right)
            || is_mouse_button_pressed(MouseButton::Right),
    };
    let (x, y) = macroquad::input::mouse_position();
    let (samples, next) =
        mouse_to_touches(buttons, macroquad::math::Vec2::new(x, y), origin, previous);
    out.extend(samples.into_iter().flatten());
    next
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;
    use macroquad::math::Vec2 as Screen;

    /// A Camera origin that is not the identity, so a test that confused screen
    /// and world coordinates cannot pass by them being equal.
    const ORIGIN: Screen = Screen::new(400.0, 300.0);

    fn press(
        buttons: MouseButtons,
        at: Screen,
        previous: MouseFrame,
    ) -> (Vec<sim::TouchSample>, MouseFrame) {
        let (samples, next) = mouse_to_touches(buttons, at, ORIGIN, previous);
        (samples.into_iter().flatten().collect(), next)
    }

    const LEFT: MouseButtons = MouseButtons {
        left: true,
        right: false,
    };
    const RIGHT: MouseButtons = MouseButtons {
        left: false,
        right: true,
    };
    const BOTH: MouseButtons = MouseButtons {
        left: true,
        right: true,
    };
    const NONE: MouseButtons = MouseButtons {
        left: false,
        right: false,
    };

    #[test]
    fn a_press_starts_once_and_then_holds_still() {
        let at = Screen::new(464.0, 300.0);

        let (first, state) = press(LEFT, at, MouseFrame::default());
        assert_eq!(first.len(), 1, "one button down is one touch");
        assert_eq!(first[0].id, 0, "the left button drives id 0");
        assert_eq!(first[0].phase, sim::TouchPhase::Started);

        // The claim this test exists for: Started happens once. A second
        // Started while held would re-fire the Tongue every frame.
        let (second, state) = press(LEFT, at, state);
        assert_eq!(second.len(), 1);
        assert_eq!(
            second[0].phase,
            sim::TouchPhase::Stationary,
            "a held button is Stationary, never Started again"
        );

        let (third, _) = press(LEFT, at, state);
        assert_eq!(third[0].phase, sim::TouchPhase::Stationary);
    }

    #[test]
    fn a_held_button_moves_only_when_the_pointer_does() {
        let at = Screen::new(464.0, 300.0);
        let (_, state) = press(LEFT, at, MouseFrame::default());

        let moved_to = Screen::new(465.0, 300.0);
        let (moved, state) = press(LEFT, moved_to, state);
        assert_eq!(moved[0].phase, sim::TouchPhase::Moved);

        // Still at the new position, so back to Stationary rather than staying
        // Moved. Comparing against the previous frame and not the press point.
        let (still, _) = press(LEFT, moved_to, state);
        assert_eq!(still[0].phase, sim::TouchPhase::Stationary);
    }

    #[test]
    fn a_release_ends_for_exactly_one_frame_and_then_nothing() {
        let at = Screen::new(464.0, 300.0);
        let (_, state) = press(LEFT, at, MouseFrame::default());

        let (ended, state) = press(NONE, at, state);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].phase, sim::TouchPhase::Ended);
        assert_eq!(ended[0].id, 0);

        // The half that a counter would get wrong: the frame after Ended is
        // empty, not another Ended.
        let (after, state) = press(NONE, at, state);
        assert!(
            after.is_empty(),
            "a lifted button reports nothing, got {after:?}"
        );
        let (later, _) = press(NONE, at, state);
        assert!(later.is_empty());
    }

    #[test]
    fn both_buttons_hold_two_touches_at_once_which_is_the_only_route_to_the_second_slot() {
        let at = Screen::new(464.0, 300.0);

        let (both, state) = press(BOTH, at, MouseFrame::default());
        assert_eq!(both.len(), 2, "two buttons down is two concurrent touches");
        let ids: Vec<u64> = both.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![0, 1], "ascending, and left is 0");
        assert!(both.iter().all(|s| s.phase == sim::TouchPhase::Started));

        // Independent: letting the left go leaves the right held, which is what
        // holding two Tongues and Releasing one looks like.
        let (one_up, _) = press(RIGHT, at, state);
        assert_eq!(one_up.len(), 2);
        let left = one_up
            .iter()
            .find(|s| s.id == 0)
            .expect("id 0 reports its lift");
        let right = one_up
            .iter()
            .find(|s| s.id == 1)
            .expect("id 1 is still held");
        assert_eq!(left.phase, sim::TouchPhase::Ended);
        assert_eq!(right.phase, sim::TouchPhase::Stationary);
    }

    #[test]
    fn the_right_button_alone_drives_id_one_and_leaves_id_zero_absent() {
        let at = Screen::new(464.0, 300.0);
        let (only, _) = press(RIGHT, at, MouseFrame::default());
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].id, 1, "the right button drives id 1");
        assert_eq!(only[0].phase, sim::TouchPhase::Started);
    }

    #[test]
    fn an_id_is_stable_across_every_frame_it_is_held() {
        let mut state = MouseFrame::default();
        let mut seen = Vec::new();
        for frame in 0..8 {
            // Move every other frame, so the run covers Started, Moved and
            // Stationary without the id ever changing.
            let at = Screen::new(464.0 + (frame / 2) as f32, 300.0);
            let (samples, next) = press(LEFT, at, state);
            state = next;
            seen.push(samples[0].id);
        }
        assert_eq!(
            seen,
            vec![0; 8],
            "a held button keeps its id for its whole life"
        );
    }

    #[test]
    fn the_position_crosses_into_world_units_through_the_camera_origin() {
        // One unit right of the origin is PIXELS_PER_UNIT screen pixels right,
        // and screen y grows downward while world y grows upward.
        let at = Screen::new(ORIGIN.x + PIXELS_PER_UNIT, ORIGIN.y - 2.0 * PIXELS_PER_UNIT);
        let (samples, _) = press(LEFT, at, MouseFrame::default());

        let pos = samples[0].pos;
        assert!(
            (pos - glam::Vec2::new(1.0, 2.0)).length() < 1e-6,
            "expected world (1, 2), got {pos:?}"
        );
    }

    #[test]
    fn a_press_at_a_new_place_after_a_release_starts_rather_than_moves() {
        let first = Screen::new(464.0, 300.0);
        let (_, state) = press(LEFT, first, MouseFrame::default());
        let (_, state) = press(NONE, first, state);

        // The re-press is a fresh Gesture even though the pointer moved while
        // the button was up, which is the case a stale held position would get
        // wrong by reporting Moved and so firing nothing.
        let elsewhere = Screen::new(600.0, 120.0);
        let (again, _) = press(LEFT, elsewhere, state);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].phase, sim::TouchPhase::Started);
    }
}
