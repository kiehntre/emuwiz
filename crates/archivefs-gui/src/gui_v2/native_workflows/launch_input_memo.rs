//! Display-only reuse of the selected game's launch-readiness input.
//!
//! Building that input walks every installed emulator, re-reads their
//! settings and copies the RetroArch scan result. The pages used to do that
//! on every drawn frame (and several times per frame on Game Details), which
//! halved the frame rate while a game was selected.
//!
//! This memo lets the *drawing* code reuse one built input. It is never read
//! by anything that starts or authorises a launch: Play, the Launch page and
//! the bezel preview build their own input at the moment they are used, and
//! the core launch preflight re-inspects the game, the emulator and the
//! firmware after that.
//!
//! A remembered input is reused only while both hold:
//!
//! * its key is unchanged. The key covers the selected game, the selected
//!   evidence report (by generation), the same emulator-environment
//!   signature the background readiness checks use, the library generation,
//!   the catalogue hints for the game, and how many answers the per-game
//!   background checks have handed back (`emulator_setup::answers`), so the
//!   frame a worker wakes always builds again and collects its answer;
//! * it is younger than its allowed age. Some inputs are not visible to the
//!   key (controllers that answer only after a game is selected, files that
//!   changed on disk), so every input is rebuilt on a timer as well. The age
//!   starts short and doubles each time a rebuild gives the same answer, up
//!   to [`SETTLED_AGE`], and drops back to [`SETTLING_AGE`] the moment an
//!   answer differs or the key changes. An answer that is no plan at all
//!   because a check is still running never settles: it keeps the short
//!   age, and a slow build stretches it no further than [`SETTLED_AGE`]. The
//!   age is counted from when the
//!   build finished and is never less than [`BUILD_SHARE`] times what the
//!   build took (up to [`SLOW_BUILD_AGE_CAP`]), so rebuilding cannot occupy
//!   more than a small share of the interface thread on a slow machine or
//!   disk.
//!
//! Every reuse reports how long the input may still be drawn, so the page
//! can ask for one more frame at that moment and never depends on some other
//! timer to notice a change the key does not cover.

use std::cell::{Cell, RefCell};
use std::hash::Hasher;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::launch_readiness_page::LaunchReadinessInput;

/// How long an input is reused right after anything changed.
pub(super) const SETTLING_AGE: Duration = Duration::from_millis(250);
/// How long an unchanged input is reused once its answer has settled, unless
/// the build itself was slow (see [`BUILD_SHARE`]). A changed BIOS file is
/// noticed when the input is next built, so a firmware fact on screen is
/// normally at most this old plus the time its new answer takes to verify;
/// `firmware_answers` lists the changes that are only picked up by its
/// backstop.
pub(super) const SETTLED_AGE: Duration = Duration::from_secs(2);
/// An input is reused for at least this many times as long as it took to
/// build: at most one part in nine of the thread's time goes on rebuilding.
pub(super) const BUILD_SHARE: u32 = 8;
/// The most a slow build can stretch the reuse of its own answer.
pub(super) const SLOW_BUILD_AGE_CAP: Duration = Duration::from_secs(10);

struct Entry {
    key: u64,
    /// When the build finished.
    built_at: Instant,
    /// The settling step this build was given (250 ms doubling to 2 s).
    step: Duration,
    /// How long it may be reused: `step`, or longer after a slow build.
    allowed_age: Duration,
    /// What the input says, so an unchanged rebuild can be told from a
    /// changed one without comparing the inputs themselves.
    digest: u64,
    input: Rc<LaunchReadinessInput>,
}

/// One remembered input for the game on screen.
#[derive(Default)]
pub(super) struct LaunchInputMemo {
    entry: RefCell<Option<Entry>>,
    builds: Cell<u64>,
    /// Tests stop the clock so that reuse never depends on how fast the
    /// machine running them is.
    #[cfg(test)]
    frozen: Cell<Option<Instant>>,
}

/// An input for drawing, and how long it may still be reused (`None` when it
/// was not remembered, because every call then builds again anyway).
pub(super) struct Shown {
    pub(super) input: Rc<LaunchReadinessInput>,
    pub(super) recheck_in: Option<Duration>,
}

impl LaunchInputMemo {
    /// The time this memo measures ages against.
    pub(super) fn now(&self) -> Instant {
        #[cfg(test)]
        if let Some(frozen) = self.frozen.get() {
            return frozen;
        }
        Instant::now()
    }

    /// Stop the clock at `at` (tests only).
    #[cfg(test)]
    pub(super) fn freeze(&self, at: Instant) {
        self.frozen.set(Some(at));
    }

    /// The input to draw. `key` is `None` when nothing may be reused (the
    /// evidence or an emulator scan is unfinished); `build` then runs every
    /// time, exactly as before this memo existed.
    pub(super) fn shown(
        &self,
        key: Option<u64>,
        build: impl FnOnce() -> LaunchReadinessInput,
    ) -> Shown {
        let Some(key) = key else {
            self.entry.borrow_mut().take();
            return Shown {
                input: Rc::new(self.counted(build)),
                recheck_in: None,
            };
        };
        let started = self.now();
        // What the last build under this key said, unless it can still be drawn.
        let previous = {
            let entry = self.entry.borrow();
            match entry.as_ref() {
                Some(current) if current.key == key => {
                    let age = started.saturating_duration_since(current.built_at);
                    if age < current.allowed_age {
                        return Shown {
                            input: Rc::clone(&current.input),
                            recheck_in: Some(current.allowed_age - age),
                        };
                    }
                    Some((current.digest, current.step))
                }
                _ => None,
            }
        };
        // No borrow is held while the build runs.
        let input = self.counted(build);
        let built_at = self.now();
        let digest = digest(&input);
        let build_time = built_at.saturating_duration_since(started);
        let (step, allowed_age) = if still_checking(&input) {
            // An answer that is about to change: it never settles, however
            // often it repeats, and a slow build stretches it only as far as
            // a settled answer would be reused anyway.
            let stretched = (build_time * BUILD_SHARE).min(SETTLED_AGE);
            (SETTLING_AGE, SETTLING_AGE.max(stretched))
        } else {
            let step = match previous {
                Some((last_digest, last_step)) if last_digest == digest => {
                    (last_step * 2).min(SETTLED_AGE)
                }
                _ => SETTLING_AGE,
            };
            let stretched = (build_time * BUILD_SHARE).min(SLOW_BUILD_AGE_CAP);
            (step, step.max(stretched))
        };
        let input = Rc::new(input);
        *self.entry.borrow_mut() = Some(Entry {
            key,
            built_at,
            step,
            allowed_age,
            digest,
            input: Rc::clone(&input),
        });
        Shown {
            input,
            recheck_in: Some(allowed_age),
        }
    }

    /// Whether nothing is remembered (tests only).
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.entry.borrow().is_none()
    }

    /// Drop the remembered input: the next draw builds a new one.
    pub(super) fn forget(&self) {
        self.entry.borrow_mut().take();
    }

    /// Runs a build that is not remembered (Play and other actions), counted
    /// with the rest.
    pub(super) fn fresh(
        &self,
        build: impl FnOnce() -> LaunchReadinessInput,
    ) -> LaunchReadinessInput {
        self.counted(build)
    }

    fn counted(&self, build: impl FnOnce() -> LaunchReadinessInput) -> LaunchReadinessInput {
        self.builds.set(self.builds.get() + 1);
        build()
    }

    /// How many inputs have been built, remembered or not.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn builds(&self) -> u64 {
        self.builds.get()
    }
}

/// Whether the input says a check is still running: the evidence is not
/// loaded (with a key, that is the MAME machine probe still at work) or
/// RetroArch has not been scanned.
///
/// A plan built before an emulator scan finished is deliberately not
/// counted. For some games that scan never runs or has ended in an error
/// (Xenia outside the cheats workflow, a failed PCSX2 or Dolphin scan, an
/// Amiga game with no WHDLoad or ADF context), so the plan would be rebuilt
/// four times a second for as long as the game stays selected. Every change
/// to those scan states already changes the key.
fn still_checking(input: &LaunchReadinessInput) -> bool {
    matches!(
        input,
        LaunchReadinessInput::EvidenceNotLoaded | LaunchReadinessInput::RetroArchNotScanned
    )
}

/// A fingerprint of what the input says: the variant, and for a plan its
/// candidates, blockers and scan flags. Only ever compared with the digest
/// of the previous build for the same key.
fn digest(input: &LaunchReadinessInput) -> u64 {
    struct Feed<'a, H: Hasher>(&'a mut H);
    impl<H: Hasher> std::fmt::Write for Feed<'_, H> {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0.write(text.as_bytes());
            Ok(())
        }
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&std::mem::discriminant(input), &mut hasher);
    match input {
        LaunchReadinessInput::Plan {
            plan,
            retroarch,
            retroarch_scanned,
            standalone_scans_complete,
            ..
        } => {
            let _ = std::fmt::write(&mut Feed(&mut hasher), format_args!("{plan:?}"));
            hasher.write_u8(u8::from(retroarch.is_some()));
            hasher.write_u8(u8::from(*retroarch_scanned));
            hasher.write_u8(u8::from(*standalone_scans_complete));
        }
        LaunchReadinessInput::DiscFilesUnresolved(reason) => hasher.write(reason.as_bytes()),
        _ => {}
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memo_at(now: Instant) -> LaunchInputMemo {
        let memo = LaunchInputMemo::default();
        memo.freeze(now);
        memo
    }

    #[test]
    fn an_unchanged_key_within_its_age_builds_once() {
        let start = Instant::now();
        let memo = memo_at(start);
        for frame in 0..30 {
            memo.freeze(start + Duration::from_millis(frame * 8));
            let shown = memo.shown(Some(7), || LaunchReadinessInput::IdentityConflicting);
            assert!(matches!(
                *shown.input,
                LaunchReadinessInput::IdentityConflicting
            ));
        }
        assert_eq!(memo.builds(), 1, "30 frames in 240 ms reuse one build");
    }

    #[test]
    fn a_changed_key_rebuilds_at_once_and_never_returns_the_old_answer() {
        let memo = memo_at(Instant::now());
        let first = memo.shown(Some(1), || LaunchReadinessInput::IdentityConflicting);
        assert!(matches!(
            *first.input,
            LaunchReadinessInput::IdentityConflicting
        ));
        // Another game in the very same instant.
        let second = memo.shown(Some(2), || LaunchReadinessInput::InspectionIncomplete);
        assert!(matches!(
            *second.input,
            LaunchReadinessInput::InspectionIncomplete
        ));
        // And back: the first answer was dropped, not kept beside the second.
        let third = memo.shown(Some(1), || LaunchReadinessInput::DosBoxReview);
        assert!(matches!(*third.input, LaunchReadinessInput::DosBoxReview));
        assert_eq!(memo.builds(), 3);
    }

    #[test]
    fn no_key_means_every_call_builds_and_nothing_is_kept() {
        let memo = memo_at(Instant::now());
        memo.shown(Some(1), || LaunchReadinessInput::IdentityConflicting);
        for _ in 0..3 {
            let shown = memo.shown(None, || LaunchReadinessInput::EvidenceNotLoaded);
            assert!(matches!(
                *shown.input,
                LaunchReadinessInput::EvidenceNotLoaded
            ));
            assert!(shown.recheck_in.is_none());
        }
        // The keyed answer from before the gap is gone too.
        let after = memo.shown(Some(1), || LaunchReadinessInput::DosBoxReview);
        assert!(matches!(*after.input, LaunchReadinessInput::DosBoxReview));
        assert_eq!(memo.builds(), 5);
    }

    #[test]
    fn the_allowed_age_doubles_while_the_answer_repeats_and_resets_when_it_differs() {
        let mut now = Instant::now();
        let memo = memo_at(now);
        let mut ages = Vec::new();
        for _ in 0..6 {
            let shown = memo.shown(Some(1), || LaunchReadinessInput::IdentityConflicting);
            ages.push(shown.recheck_in);
            // Step just past whatever age this build was given.
            now += memo.entry.borrow().as_ref().unwrap().allowed_age + Duration::from_millis(1);
            memo.freeze(now);
        }
        assert_eq!(
            ages,
            [250, 500, 1000, 2000, 2000, 2000].map(|ms| Some(Duration::from_millis(ms))),
            "settling steps, then the settled age, each with its own recheck time"
        );
        assert_eq!(
            memo.entry.borrow().as_ref().unwrap().allowed_age,
            SETTLED_AGE
        );
        assert_eq!(memo.builds(), 6);
        // A different answer under the same key starts the short age again.
        let changed = memo.shown(Some(1), || LaunchReadinessInput::InspectionIncomplete);
        assert_eq!(changed.recheck_in, Some(SETTLING_AGE));
    }

    #[test]
    fn an_expired_answer_is_rebuilt_even_when_the_key_is_unchanged() {
        let now = Instant::now();
        let memo = memo_at(now);
        memo.shown(Some(1), || LaunchReadinessInput::IdentityConflicting);
        memo.freeze(now + SETTLING_AGE - Duration::from_millis(1));
        let still = memo.shown(Some(1), || LaunchReadinessInput::InspectionIncomplete);
        assert!(matches!(
            *still.input,
            LaunchReadinessInput::IdentityConflicting
        ));
        memo.freeze(now + SETTLING_AGE);
        let later = memo.shown(Some(1), || LaunchReadinessInput::InspectionIncomplete);
        assert!(matches!(
            *later.input,
            LaunchReadinessInput::InspectionIncomplete
        ));
    }

    /// The flaw a first version had: the age was counted from before the
    /// build, so a build slower than the age expired itself and every frame
    /// rebuilt. A slow build must be reused for longer, not for no time.
    #[test]
    fn a_slow_build_is_reused_for_a_multiple_of_the_time_it_took() {
        let start = Instant::now();
        let memo = memo_at(start);
        let slow = Duration::from_millis(400);
        let shown = memo.shown(Some(1), || {
            // The build takes 400 ms of the memo's clock.
            memo.freeze(start + slow);
            LaunchReadinessInput::IdentityConflicting
        });
        assert_eq!(shown.recheck_in, Some(slow * BUILD_SHARE));
        // The very next frame, and frames for the next three seconds, reuse it.
        for after_ms in [0, 16, 250, 1000, 3000] {
            memo.freeze(start + slow + Duration::from_millis(after_ms));
            memo.shown(Some(1), || {
                unreachable!("reused {after_ms} ms after a slow build")
            });
        }
        assert_eq!(memo.builds(), 1);
        memo.freeze(start + slow + slow * BUILD_SHARE);
        memo.shown(Some(1), || LaunchReadinessInput::IdentityConflicting);
        assert_eq!(memo.builds(), 2);
    }

    #[test]
    fn a_very_slow_build_stretches_reuse_only_up_to_the_cap() {
        let start = Instant::now();
        let memo = memo_at(start);
        let slow = Duration::from_secs(5);
        let shown = memo.shown(Some(1), || {
            memo.freeze(start + slow);
            LaunchReadinessInput::IdentityConflicting
        });
        assert_eq!(shown.recheck_in, Some(SLOW_BUILD_AGE_CAP));
        memo.freeze(start + slow + SLOW_BUILD_AGE_CAP);
        memo.shown(Some(1), || LaunchReadinessInput::IdentityConflicting);
        assert_eq!(memo.builds(), 2, "rebuilt at the cap, not after 40 s");
    }

    #[test]
    fn an_answer_that_says_a_check_is_still_running_never_settles() {
        let mut now = Instant::now();
        let memo = memo_at(now);
        for _ in 0..5 {
            let shown = memo.shown(Some(1), || LaunchReadinessInput::EvidenceNotLoaded);
            assert_eq!(
                shown.recheck_in,
                Some(SETTLING_AGE),
                "the same answer again does not lengthen its reuse"
            );
            now += SETTLING_AGE;
            memo.freeze(now);
        }
        assert_eq!(memo.builds(), 5);
        // A slow build stretches it to the settled age at most, never to the
        // ten-second cap a settled answer gets.
        let slow = Duration::from_secs(5);
        let shown = memo.shown(Some(2), || {
            memo.freeze(now + slow);
            LaunchReadinessInput::RetroArchNotScanned
        });
        assert_eq!(shown.recheck_in, Some(SETTLED_AGE));
        // The moment the answer is a real one, the normal steps apply: it
        // starts short and doubles when it repeats.
        let mut at = now + slow + SETTLED_AGE;
        memo.freeze(at);
        let settled = memo.shown(Some(2), || LaunchReadinessInput::IdentityConflicting);
        assert_eq!(settled.recheck_in, Some(SETTLING_AGE));
        at += SETTLING_AGE;
        memo.freeze(at);
        let repeated = memo.shown(Some(2), || LaunchReadinessInput::IdentityConflicting);
        assert_eq!(repeated.recheck_in, Some(SETTLING_AGE * 2));
    }

    #[test]
    fn a_reused_answer_always_says_when_it_will_be_rebuilt() {
        let start = Instant::now();
        let memo = memo_at(start);
        memo.shown(Some(1), || LaunchReadinessInput::IdentityConflicting);
        memo.freeze(start + Duration::from_millis(100));
        let reused = memo.shown(Some(1), || unreachable!());
        assert_eq!(reused.recheck_in, Some(Duration::from_millis(150)));
    }

    /// The build may itself read the memo (it does not today). That must not
    /// be a double borrow.
    #[test]
    fn a_build_that_reads_the_memo_does_not_panic() {
        let memo = memo_at(Instant::now());
        memo.shown(Some(1), || {
            memo.forget();
            assert!(memo.is_empty());
            memo.fresh(|| LaunchReadinessInput::DosBoxReview)
        });
        assert_eq!(memo.builds(), 2);
        assert!(!memo.is_empty());
    }

    #[test]
    fn forgetting_and_fresh_builds_never_reuse_the_remembered_answer() {
        let memo = memo_at(Instant::now());
        memo.shown(Some(1), || LaunchReadinessInput::IdentityConflicting);
        // An action builds its own input and does not read the memo.
        let fresh = memo.fresh(|| LaunchReadinessInput::InspectionIncomplete);
        assert!(matches!(fresh, LaunchReadinessInput::InspectionIncomplete));
        // ...and does not replace what the page is drawing either.
        let drawn = memo.shown(Some(1), || LaunchReadinessInput::DosBoxReview);
        assert!(matches!(
            *drawn.input,
            LaunchReadinessInput::IdentityConflicting
        ));
        memo.forget();
        let rebuilt = memo.shown(Some(1), || LaunchReadinessInput::DosBoxReview);
        assert!(matches!(*rebuilt.input, LaunchReadinessInput::DosBoxReview));
    }
}
