//! The Combo: an unbroken run of Slams, counted in the Core Sim.
//!
//! GLOSSARY: "an unbroken run of Slams, counted while the player is airborne
//! and persisting across Release and re-attach. It is a count of hits, not a
//! score." ADR-0011 is the contract, and the whole rule there is marked
//! explicitly provisional and expected to be retuned against real gameplay, so
//! everything in this module is one obvious predicate and five constants rather
//! than anything clever.
//!
//! **A count and never a score.** There is no multiplier, no cap, and no
//! derived number of any kind here. ADR-0011 puts the score multiplier and its
//! cap in the Shell as presentation, and the Core Sim's job stops at how many
//! hits the run is worth.
//!
//! This type is mutated state, which is why it lives in its own module with one
//! entry point. ADR-0007's integrator evaluates acceleration twice per Sim Tick,
//! so anything a force can reach must be a pure function of position and
//! velocity; [`Combo::after_impacts`] is reachable only from
//! [`crate::world::World::step`], after the integration step and after impacts
//! have resolved, and is called exactly once per tick.
//!
//! ## Not implemented, deliberately
//!
//! ADR-0011 also says the countdown "freezes entirely during wave transitions
//! and resumes when the first Enemy of the new wave becomes active". **There is
//! no wave system.** It is deferred in the map's fog, and the ADR itself notes
//! the rule "presumes a wave concept that does not exist yet". A freeze is
//! implemented against nothing: there is no transition to start it and no first
//! Enemy of a new wave to end it, so inventing either here would mean inventing
//! the wave system. The seam is [`Combo::after_impacts`]: a wave system arrives
//! as one more reason that call declines to advance the countdown, beside the
//! `hits == 0` case it already has.

use crate::combat::ImpactBand;

/// The countdown window a Combo of one hit gets, in Sim Ticks.
///
/// Two seconds at ADR-0002's locked tick. Generous on purpose: the first hit of
/// a run is the one made from a standstill, with no momentum banked and no
/// Anchor chosen yet.
pub const MAX_WINDOW_TICKS: u32 = 120;

/// How many Sim Ticks each hit after the first takes off the window.
///
/// A tenth of a second per hit, so the tightening is felt over a run rather
/// than between two hits.
pub const WINDOW_DECAY_TICKS: u32 = 6;

/// The hard floor on the window, in Sim Ticks.
///
/// Six tenths of a second, which is roughly the time a Release-and-re-Attach
/// takes at the baseline hero's pull strength. Below that a long Combo would
/// stop being a skill and start being a frame-perfect demand, which is what
/// ADR-0011's floor exists to prevent. Reached at 15 hits and held from there
/// on, however long the run gets.
pub const MIN_WINDOW_TICKS: u32 = 36;

/// Below this speed, in world units per second, the hero counts as still.
///
/// Half a hero radius per second. The predicate this feeds is the one genuinely
/// undecided part of ADR-0011's Combo rule, and the reasoning for the number is
/// recorded there: it sits far above the float residue of a resting contact and
/// far below anything a Swing, a fall, or a bounce travels at.
pub const REST_SPEED: f32 = 0.5;

/// How many consecutive Sim Ticks of slow terrain contact count as coming to
/// rest.
///
/// A tenth of a second. One tick would be enough to read a bounce as a
/// standstill: a glancing touch of the ground loses the velocity heading into
/// the surface on the tick it lands, so a hero arriving nearly straight down is
/// momentarily slow while still very much in play. Sustaining it for six ticks
/// is the difference between touching the ground and staying there, and it is
/// still a sixth of the shortest window.
pub const REST_CONTACT_TICKS: u32 = 6;

/// How long a Combo of `hits` has to find its next Slam, in Sim Ticks.
///
/// Tightens by [`WINDOW_DECAY_TICKS`] per hit from [`MAX_WINDOW_TICKS`] and
/// stops at [`MIN_WINDOW_TICKS`]. Saturating throughout, because the count is
/// uncapped: a run of four billion hits must give the floor rather than wrap
/// around to a generous window.
pub fn window_ticks(hits: u32) -> u32 {
    let decay = hits.saturating_sub(1).saturating_mul(WINDOW_DECAY_TICKS);
    MAX_WINDOW_TICKS.saturating_sub(decay).max(MIN_WINDOW_TICKS)
}

/// What the hero's end-of-tick state looks like to the Combo.
///
/// Reported by the `World` rather than decided by it: the predicate for coming
/// to rest lives here, so the tuning and the rule stay in one place.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeroState {
    /// Whether the hero ended the Sim Tick in contact with static terrain.
    ///
    /// Terrain and not an Enemy. ADR-0011 breaks the Combo on coming to rest
    /// **on a surface**, and an Enemy is a target rather than a surface: coming
    /// to rest against one is the Kick band's business.
    pub touching_terrain: bool,
    /// The hero's speed at the end of the Sim Tick, in world units per second.
    pub speed: f32,
}

/// An unbroken run of Slams.
#[derive(Clone, Copy, Debug, Default)]
pub struct Combo {
    hits: u32,
    /// Sim Ticks since the last Slam. Only meaningful while `hits > 0`.
    idle_ticks: u32,
    /// Consecutive Sim Ticks of slow terrain contact, which is how "comes to
    /// rest" is measured. Counted whether or not a Combo is live, so that a
    /// Combo started while already at rest is judged on the ticks after it
    /// rather than inheriting the ones before.
    rest_ticks: u32,
}

impl Combo {
    /// The count of hits in the current run. Zero means no Combo.
    pub fn hits(&self) -> u32 {
        self.hits
    }

    /// Advances the Combo by one Sim Tick.
    ///
    /// Called once per tick from [`crate::world::World::step`], after the
    /// integration step and after impacts have resolved, with the number of
    /// Slams that landed this tick and the state the tick left the hero in.
    ///
    /// The order inside is the rule. Coming to rest is decided first, because
    /// ADR-0011 makes it a break and not a pause. A tick that landed a Slam
    /// then refreshes the window and is not counted as drought: the countdown
    /// governs the gaps between hits, so a Slam arriving on the window's last
    /// tick keeps the run alive rather than losing it by one tick.
    pub fn after_impacts(&mut self, slams: u32, hero: HeroState) {
        if hero.touching_terrain && hero.speed < REST_SPEED {
            self.rest_ticks = self.rest_ticks.saturating_add(1);
        } else {
            self.rest_ticks = 0;
        }

        // Coming to rest on a surface, which is not the same as touching one.
        // A bounce is contact the hero leaves; a standstill is contact it
        // stays slow in.
        if self.rest_ticks >= REST_CONTACT_TICKS {
            self.break_run();
            // Not an early return: a Slam landing on the tick a run ended
            // starts the next one, which is the same thing that happens on any
            // other tick with no live Combo.
        }

        if slams > 0 {
            // Saturating because the count is uncapped by ADR-0011 and `u32` is
            // the only ceiling in sight. Four billion hits is not a gameplay
            // cap, and wrapping to zero there would be a lost run rather than a
            // tuning choice.
            self.hits = self.hits.saturating_add(slams);
            self.idle_ticks = 0;
            return;
        }

        if self.hits == 0 {
            return;
        }

        // The airborne drought. Nothing else uses the countdown: contact with
        // the ground is governed by the rest predicate above, and the wave
        // freeze ADR-0011 describes has no wave system to attach to.
        self.idle_ticks += 1;
        if self.idle_ticks >= window_ticks(self.hits) {
            self.break_run();
        }
    }

    /// How many of `band`'s outcomes count as a hit.
    ///
    /// A Slam is a collision above the Kick threshold, per the GLOSSARY, so
    /// both of ADR-0011's upper bands count: spending momentum against a
    /// Toughness pool and emptying it outright are the same hit landed against
    /// different Enemies. A Kick is the band that explicitly is not a Slam, and
    /// nothing at all is not a hit.
    pub fn counts_as_slam(band: Option<ImpactBand>) -> bool {
        matches!(band, Some(ImpactBand::Slam) | Some(ImpactBand::Destroy))
    }

    fn break_run(&mut self) {
        self.hits = 0;
        self.idle_ticks = 0;
    }
}

/// The window in seconds, for reading a tick count in the terms it was chosen
/// in. Test and documentation scaffolding, not used by the simulation.
#[cfg(any(test, feature = "test-util"))]
pub fn window_seconds(hits: u32) -> f32 {
    window_ticks(hits) as f32 * crate::DT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_tightens_with_every_hit_until_it_reaches_the_floor() {
        // The shape of the rule, asserted on the function rather than through
        // the `World`, which is where the player-visible version of this lives.
        // Two independent claims: it is non-increasing in `hits`, and it is
        // strictly decreasing until the floor binds.
        let mut previous = window_ticks(1);
        assert_eq!(previous, MAX_WINDOW_TICKS, "one hit gets the whole window");
        let mut strictly_shorter = 0;
        for hits in 2..=200u32 {
            let now = window_ticks(hits);
            assert!(
                now <= previous,
                "the window grew at {hits} hits: {previous} then {now}"
            );
            if now < previous {
                strictly_shorter += 1;
            }
            assert!(
                now >= MIN_WINDOW_TICKS,
                "the window fell through the floor at {hits} hits: {now}"
            );
            previous = now;
        }
        assert!(
            strictly_shorter > 10,
            "the window hardly tightened at all: {strictly_shorter} steps down"
        );
        assert_eq!(
            previous, MIN_WINDOW_TICKS,
            "a long run must sit exactly on the floor"
        );
    }

    #[test]
    fn an_absurd_hit_count_still_gets_the_floor() {
        // Hostile input. The count is uncapped, so the arithmetic that tightens
        // the window has to survive the top of the type: an unsaturated
        // `(hits - 1) * DECAY` wraps and hands a four-billion-hit Combo a
        // *longer* window than a one-hit one.
        assert_eq!(window_ticks(u32::MAX), MIN_WINDOW_TICKS);
        assert_eq!(window_ticks(u32::MAX / 2), MIN_WINDOW_TICKS);
        // And a run with no hits is asked about by nothing, but must not panic
        // or underflow if it ever is.
        assert_eq!(window_ticks(0), MAX_WINDOW_TICKS);
    }

    #[test]
    fn the_windows_are_the_durations_they_were_chosen_as() {
        // The tick counts above are only meaningful as times, and ADR-0002
        // locks the tick, so these are the numbers ADR-0011 is tuned in.
        assert!(
            (window_seconds(1) - 2.0).abs() < 1e-6,
            "two seconds at one hit"
        );
        assert!(
            (window_seconds(u32::MAX) - 0.6).abs() < 1e-6,
            "six tenths of a second at the floor"
        );
    }

    #[test]
    fn only_the_bands_above_the_kick_threshold_count_as_hits() {
        assert!(Combo::counts_as_slam(Some(ImpactBand::Slam)));
        assert!(
            Combo::counts_as_slam(Some(ImpactBand::Destroy)),
            "destroying an Enemy is a Slam that happened to empty the pool"
        );
        assert!(
            !Combo::counts_as_slam(Some(ImpactBand::Kick)),
            "a Kick is below the Slam threshold by definition"
        );
        assert!(!Combo::counts_as_slam(None));
    }
}
