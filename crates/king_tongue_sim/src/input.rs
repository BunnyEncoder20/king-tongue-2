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

    /// Hands `id` a slot, or `None` if there is nothing to give it.
    ///
    /// `vacant` says which slots hold no Tongue. A slot is grantable only when
    /// it is both unowned and vacant:
    ///
    /// - unowned, because two fingers down at once must drive two different
    ///   slots, and a third concurrent finger has nowhere to go and is
    ///   discarded per ADR-0006 rather than doing something surprising.
    /// - vacant, because a tap aimed at a slot that already holds a Tongue is a
    ///   Release, which arrives with ticket #13. Until it does, that tap is
    ///   inert and the hero Swings indefinitely, which is the interim state
    ///   ticket #12 expects.
    ///
    /// Idempotent for an id that already owns a slot, so the `Stationary` a
    /// held finger reports every frame cannot re-fire it.
    pub fn claim(&mut self, id: u64, vacant: [bool; MAX_TONGUES]) -> Option<u8> {
        if self.owner_slot(id).is_some() {
            return None;
        }
        let slot = (0..MAX_TONGUES).find(|&s| self.owners[s].is_none() && vacant[s])?;
        self.owners[slot] = Some(id);
        Some(slot as u8)
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
        assert_eq!(tracker.claim(10, BOTH_VACANT), Some(0));
        assert_eq!(tracker.claim(20, BOTH_VACANT), Some(1));
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
        assert_eq!(tracker.claim(7, BOTH_VACANT), Some(0));
        assert_eq!(tracker.claim(7, BOTH_VACANT), None);
    }

    #[test]
    fn an_occupied_slot_is_not_granted() {
        // Slot 0 already holds a Tongue, so the next finger must be given the
        // free slot rather than stealing the Attached one.
        let mut tracker = GestureTracker::new();
        assert_eq!(tracker.claim(1, [false, true]), Some(1));
        assert_eq!(tracker.claim(2, [false, true]), None);
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
        assert_eq!(tracker.claim(1, BOTH_VACANT), Some(0));
        tracker.release_owner(1);
        // The slot is unowned again, though in the `World` it is no longer
        // vacant: the Tongue it fired is still Attached.
        assert_eq!(tracker.claim(2, BOTH_VACANT), Some(0));
    }
}
