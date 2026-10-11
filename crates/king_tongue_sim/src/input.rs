//! Input: platform-neutral touches, and the Gesture tracker that turns them
//! into slot claims.
//!
//! This module exists so the Shell can be a converter and nothing more. ADR-0001
//! keeps platform types out of the Core Sim, and the spec makes the Shell
//! branch-free: every rule about which finger does what is here, behind the
//! `World` seam, where it is headless-testable. The Shell's whole input job is
//! turning a Macroquad `Touch` into a [`TouchSample`].
//!
//! Three platform facts from the ticket #2 research shape this, each a bug if
//! ignored: the touch set iterates a `HashMap` so it has no frame-stable order
//! and entries must be keyed by `id`; `Ended` and `Cancelled` are visible for
//! exactly one frame; and a held finger reports `Stationary` and never `Moved`.

use crate::MAX_TONGUES;
use glam::Vec2;

/// Where a touch is in its life, mirroring what every touch platform reports.
///
/// Named rather than reduced to a boolean because the distinctions carry rules.
/// Only `Started` is a Gesture: ADR-0006 makes control discrete, so a held
/// finger does nothing and lifting one does not Release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    /// The finger just went down. This, and only this, is a Gesture.
    Started,
    /// The finger moved while down. Not a Gesture: aiming happens at the moment
    /// of the tap and is never revised by dragging.
    Moved,
    /// The finger is down and still. What a held finger reports every frame,
    /// which is why holding must be inert rather than repeating the tap.
    Stationary,
    /// The finger lifted. Visible for exactly one frame.
    Ended,
    /// The platform took the touch away, typically because the app lost focus.
    /// Visible for exactly one frame.
    Cancelled,
}

/// One touch, as the Core Sim understands it.
///
/// `pos` is in **world space**, already converted by the Shell: screen pixels
/// and the Camera origin are the Shell's business, and a Core Sim that knew
/// about either would be a Core Sim that knew about the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchSample {
    /// The platform's identity for this finger, stable for its whole life.
    ///
    /// Everything here is keyed on this and never on position in the touch set,
    /// because that set has no frame-stable order.
    pub id: u64,
    pub phase: TouchPhase,
    pub pos: Vec2,
}

/// What a Gesture does to the slot it claimed.
///
/// One tap does one of these and never both, and which one is decided here
/// rather than in the `World`: it is the last rule ADR-0006 describes that is
/// not pure physics, so it belongs behind the seam with the rest of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    /// The slot was empty, so the Gesture fires a Tongue into it along the
    /// tapped vector.
    Fire(u8),
    /// The slot already held a Tongue, so the Gesture Releases it. The tapped
    /// vector is ignored: a Release is not aimed at anything, which is why it
    /// cannot miss and costs nothing. ADR-0012.
    Release(u8),
}

/// Which touch currently owns which Tongue slot.
///
/// A slot is owned for as long as the finger is down, and the Tongue it fired
/// outlasts that ownership. That split is ADR-0006: attachment survives the
/// Gesture that created it, so the input layer must never infer Release from a
/// touch ending. Macroquad flips every live touch to `Ended` when the app
/// minimizes, and under a hold-to-Swing scheme that would have been a free
/// Release on every suspend.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GestureTracker {
    owners: [Option<u64>; MAX_TONGUES],
}

impl GestureTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hands `id` the lowest slot no other finger is driving, and says whether
    /// taking it fires a Tongue or Releases the one already there.
    ///
    /// Two separate questions, and keeping them separate is what makes the
    /// whole of ADR-0006 fall out of one rule:
    ///
    /// - **Which** slot is decided by ownership alone. A third concurrent
    ///   finger finds both slots owned, gets nothing, and is discarded rather
    ///   than doing something surprising.
    /// - **What** taking it does is decided by `vacant`, which says which slots
    ///   hold no Tongue. An empty slot is a [`Claim::Fire`]; an occupied one is
    ///   a [`Claim::Release`].
    ///
    /// So one finger tapping repeatedly always returns to slot 0 and alternates
    /// fire, Release, fire, which is the traversal loop: tap, Swing, tap, fly.
    /// Two fingers down *at once* drive slots 0 and 1 and so hold two Tongues
    /// independently, which is the other half of ADR-0006. Scanning for a
    /// vacant slot first would instead spend the second tap on a second Tongue
    /// and leave the player no way to let go.
    ///
    /// Idempotent for an id that already owns a slot, so the `Stationary` a
    /// held finger reports every frame can neither re-fire nor Release it.
    pub fn claim(&mut self, id: u64, vacant: [bool; MAX_TONGUES]) -> Option<Claim> {
        if self.owner_slot(id).is_some() {
            return None;
        }
        let slot = (0..MAX_TONGUES).find(|&s| self.owners[s].is_none())?;
        self.owners[slot] = Some(id);
        // A Release claims the slot exactly as a fire does. The finger now
        // holds a slot that has gone empty under it, so holding it down cannot
        // immediately re-fire, and a second finger landing meanwhile still
        // finds the other slot.
        Some(if vacant[slot] {
            Claim::Fire(slot as u8)
        } else {
            Claim::Release(slot as u8)
        })
    }

    /// Gives up `id`'s slot, if it held one. The Tongue in that slot stays
    /// Attached: this is the end of a Gesture, not a Release.
    pub fn release_owner(&mut self, id: u64) {
        if let Some(slot) = self.owner_slot(id) {
            self.owners[slot] = None;
        }
    }

    /// Whether `id` is already driving a slot.
    pub fn owns(&self, id: u64) -> bool {
        self.owner_slot(id).is_some()
    }

    fn owner_slot(&self, id: u64) -> Option<usize> {
        (0..MAX_TONGUES).find(|&s| self.owners[s] == Some(id))
    }
}

/// The lowest-id `Started` touch that is not already driving a slot.
///
/// Ascending id order, and that is the determinism rule rather than a tidiness
/// one. Two fingers landing in the same frame arrive in whatever order a
/// `HashMap` iterates, so taking them in the order given would make slot
/// assignment depend on the platform's hashing: the same two taps would fire
/// into swapped slots between runs, and the `Snapshot` sequence would differ
/// even though the physics agreed. Sorting by the identity the platform gave us
/// makes the claim a function of the touch *set*.
pub(crate) fn next_gesture(
    touches: &[TouchSample],
    tracker: &GestureTracker,
) -> Option<TouchSample> {
    touches
        .iter()
        .filter(|t| t.phase == TouchPhase::Started && !tracker.owns(t.id))
        .min_by_key(|t| t.id)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH_VACANT: [bool; MAX_TONGUES] = [true; MAX_TONGUES];

    fn started(id: u64) -> TouchSample {
        TouchSample {
            id,
            phase: TouchPhase::Started,
            pos: Vec2::ZERO,
        }
    }

    #[test]
    fn a_third_concurrent_finger_gets_no_slot() {
        let mut tracker = GestureTracker::new();
        assert_eq!(tracker.claim(10, BOTH_VACANT), Some(Claim::Fire(0)));
        assert_eq!(tracker.claim(20, BOTH_VACANT), Some(Claim::Fire(1)));
        assert_eq!(
            tracker.claim(30, BOTH_VACANT),
            None,
            "ADR-0006 caps the hero at two Tongues, so a third id is discarded"
        );
    }

    #[test]
    fn a_held_finger_cannot_claim_twice() {
        // A held finger reports Stationary every frame and the caller may well
        // offer it again. Granting a second slot to one finger would let one tap
        // fire both Tongues.
        let mut tracker = GestureTracker::new();
        assert_eq!(tracker.claim(7, BOTH_VACANT), Some(Claim::Fire(0)));
        assert_eq!(tracker.claim(7, BOTH_VACANT), None);
    }

    #[test]
    fn a_tap_on_an_occupied_slot_releases_it() {
        // This test used to be `an_occupied_slot_is_not_granted`, asserting the
        // interim behaviour ticket #12 recorded as deliberately unfinished:
        // with no way to Release, a tap on an occupied slot had to be inert or
        // steal the slot, and it was given the free one instead.
        //
        // Under ADR-0006 that tap is the Release, so the slot *is* granted and
        // what the Gesture does with it changes. Keeping the old expectation
        // would have spent the player's second tap on a second Tongue and left
        // the traversal loop with no way to let go.
        let mut tracker = GestureTracker::new();
        assert_eq!(tracker.claim(1, [false, true]), Some(Claim::Release(0)));
        assert_eq!(
            tracker.claim(2, [true, true]),
            Some(Claim::Fire(1)),
            "the Releasing finger still holds slot 0, so the next finger fires into slot 1"
        );
    }

    #[test]
    fn which_slot_a_tap_takes_does_not_depend_on_what_is_attached() {
        // Ownership alone picks the slot, and that separation is what makes one
        // finger alternate fire and Release on slot 0 rather than wandering up
        // the slots. A rule that preferred a vacant slot would send the second
        // tap to slot 1 and never Release anything.
        for vacant in [[true, true], [false, true], [true, false], [false, false]] {
            let mut tracker = GestureTracker::new();
            let claim = tracker.claim(1, vacant).expect("slot 0 is unowned");
            assert!(
                matches!(claim, Claim::Fire(0) | Claim::Release(0)),
                "with vacancy {vacant:?} the first tap took {claim:?} instead of slot 0"
            );
        }
    }

    #[test]
    fn each_slot_fires_and_releases_on_its_own() {
        // Two Tongues held independently, which ADR-0006 reaches through one
        // finger staying **down**: it keeps slot 0, so every later Gesture
        // lands on slot 1 and acts on whatever is there. A rule that scanned
        // for a vacant slot instead would send the Release at the third step
        // back to slot 0 and take the wrong Tongue away.
        let mut tracker = GestureTracker::new();
        assert_eq!(tracker.claim(1, [true, true]), Some(Claim::Fire(0)));
        assert_eq!(
            tracker.claim(2, [false, true]),
            Some(Claim::Fire(1)),
            "a second overlapping finger fires the second Tongue"
        );

        // The second finger lifts, which ends its Gesture and leaves its Tongue
        // Attached, so the next finger finds slot 1 unowned and occupied.
        tracker.release_owner(2);
        assert_eq!(
            tracker.claim(3, [false, false]),
            Some(Claim::Release(1)),
            "with slot 0 still owned, the Release must land on slot 1"
        );

        // And slot 1 is immediately usable again while slot 0 stays Attached.
        tracker.release_owner(3);
        assert_eq!(tracker.claim(4, [false, true]), Some(Claim::Fire(1)));
    }

    #[test]
    fn gestures_are_taken_in_ascending_id_order() {
        // The regression this prevents is silent. Two fingers landing in one
        // frame arrive in `HashMap` order, so reading the slice as given would
        // assign slots by the platform's hashing and the same two taps would
        // produce different Snapshots between runs.
        let tracker = GestureTracker::new();
        let one_way = [started(99), started(4), started(50)];
        let other_way = [started(50), started(99), started(4)];
        assert_eq!(next_gesture(&one_way, &tracker).map(|t| t.id), Some(4));
        assert_eq!(next_gesture(&other_way, &tracker).map(|t| t.id), Some(4));
    }

    #[test]
    fn only_started_is_a_gesture() {
        let tracker = GestureTracker::new();
        for phase in [
            TouchPhase::Moved,
            TouchPhase::Stationary,
            TouchPhase::Ended,
            TouchPhase::Cancelled,
        ] {
            let touches = [TouchSample {
                id: 1,
                phase,
                pos: Vec2::ZERO,
            }];
            assert_eq!(
                next_gesture(&touches, &tracker),
                None,
                "{phase:?} must not fire a Tongue"
            );
        }
    }

    #[test]
    fn a_lifted_finger_frees_its_slot_for_the_next_tap() {
        let mut tracker = GestureTracker::new();
        assert_eq!(tracker.claim(1, BOTH_VACANT), Some(Claim::Fire(0)));
        tracker.release_owner(1);
        // The slot is unowned again, and in the `World` it is no longer vacant:
        // the Tongue the first finger fired is still Attached. So the next tap
        // comes back to the same slot and Releases it, which is the loop.
        assert_eq!(tracker.claim(2, [false, true]), Some(Claim::Release(0)));
    }
}
