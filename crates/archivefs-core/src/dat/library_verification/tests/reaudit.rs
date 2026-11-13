use super::*;
use crate::dat::reaudit::{
    ReauditBlock, ReauditLimits, ReauditPlan, ReauditProgress, ReauditReport, ReauditStop,
    plan_reaudit, run_reaudit,
};
use crate::database::SavedDatEvidenceStatus as Status;
use sha1::{Digest as _, Sha1};
use std::cell::Cell;

fn body(n: usize) -> Vec<u8> {
    format!("synthetic tape number {n:04}").into_bytes()
}

impl Fixture {
    /// A catalogue listing `games` tapes; game `n` is `body(n)`.
    fn write_games_dat(&self, games: usize, version: &str) {
        let mut xml = format!(
            "<datafile><header><name>Sinclair ZX Spectrum - Games - [TAP]</name><description>Sinclair ZX Spectrum - Games - [TAP]</description><version>{version}</version><author>TOSEC</author></header>"
        );
        for n in 0..games {
            let content = body(n);
            let sha1: String = Sha1::digest(&content)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            xml.push_str(&format!(
                "<game name=\"Game {n:04}\"><rom name=\"Game {n:04}.tap\" size=\"{}\" sha1=\"{sha1}\"/></game>",
                content.len()
            ));
        }
        xml.push_str("</datafile>");
        std::fs::write(&self.dat, xml).unwrap();
    }
    /// `games` library rows (ids 1..=games) whose content matches the
    /// catalogue, verified and then turned into pre-proof legacy rows.
    fn legacy_library(games: usize) -> Self {
        let f = Fixture::new();
        f.write_games_dat(games, "v1");
        for n in 0..games {
            f.add(
                n as i64 + 1,
                &format!("g{n:04}.tap"),
                Some(&body(n)),
                Some("ZX Spectrum"),
            );
        }
        let t = f.run();
        assert_eq!((t.verified, t.saved), (games, games));
        f.strip_proof();
        f.sized();
        f
    }
    fn plan(&self) -> ReauditPlan {
        self.sized();
        plan_reaudit(&self.db, &self.configured()).unwrap()
    }
    fn reaudit_with(
        &self,
        plan: &ReauditPlan,
        per_set: usize,
        cancel: &AtomicBool,
        progress: &dyn Fn(ReauditProgress),
    ) -> Result<ReauditReport, String> {
        run_reaudit(
            &self.db,
            plan,
            &|_| Ok(self.resolved()),
            &TrustedRoots::none(),
            cancel,
            ReauditLimits {
                max_games_per_set: per_set,
                max_bytes_per_set: u64::MAX,
            },
            progress,
        )
    }
    fn reaudit(&self, per_set: usize) -> ReauditReport {
        self.reaudit_with(&self.plan(), per_set, &AtomicBool::new(false), &|_| {})
            .unwrap()
    }
    fn statuses(&self) -> BTreeMap<i64, Status> {
        self.sized();
        Database::open_read_only(&self.db)
            .unwrap()
            .saved_dat_evidence_status(&self.configured())
            .unwrap()
            .into_iter()
            .map(|row| (row.archive_id, row.status))
            .collect()
    }
    fn history(&self) -> Vec<(i64, String, String, serde_json::Value)> {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.prepare("SELECT archive_id, verification_state, replaced_by_state, facts_json FROM library_dat_identity_history ORDER BY id")
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    serde_json::from_slice(&r.get::<_, Vec<u8>>(3)?).unwrap(),
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
    fn notes(&self) -> Vec<crate::database::DatRecheckAttempt> {
        Database::open_read_only(&self.db)
            .unwrap()
            .dat_recheck_attempts()
            .unwrap()
    }
}

#[test]
fn plan_lists_legacy_rows_and_promotes_nothing() {
    let f = Fixture::legacy_library(3);
    let before = f.dat_rows();
    let plan = f.plan();
    assert_eq!((plan.games(), plan.current, plan.saved_rows), (3, 0, 3));
    assert_eq!(plan.by_status().get(&Status::MissingProof), Some(&3));
    assert!(plan.blocked.is_empty());
    // Planning is read-only: rows, history and notes are untouched, and the
    // legacy rows still are not current evidence.
    assert_eq!(f.dat_rows(), before);
    assert!(f.history().is_empty() && f.notes().is_empty());
    assert!(f.evidence().states.is_empty());
}

#[test]
fn a_full_run_gives_current_proof_and_keeps_the_replaced_rows() {
    let f = Fixture::legacy_library(5);
    let legacy = f.facts(1);
    let report = f.reaudit(2);
    assert_eq!(
        (
            report.planned,
            report.verified,
            report.saved,
            report.sets_saved
        ),
        (5, 5, 5, 3)
    );
    assert!(report.stopped.is_none() && report.not_checked.is_empty());
    assert!(report.contradictions.is_empty() && report.not_reached == 0);
    assert!(f.statuses().values().all(|s| *s == Status::Current));
    assert_eq!(f.evidence().exact.len(), 5);
    // Every replaced legacy row is kept byte for byte, and is not proof.
    let history = f.history();
    assert_eq!(history.len(), 5);
    assert_eq!(history[0].3, legacy);
    assert!(history.iter().all(|h| h.3.get("audit_file").is_none()));
    // Nothing left to do; a second run reads nothing and keeps no duplicates.
    let again = f.plan();
    assert_eq!((again.games(), again.current), (0, 5));
    let report = f.reaudit(2);
    assert_eq!((report.saved, report.sets_saved), (0, 0));
    assert_eq!(f.history().len(), 5);
}

#[test]
fn a_stopped_run_keeps_finished_sets_and_resumes_with_the_rest() {
    let f = Fixture::legacy_library(5);
    let cancel = AtomicBool::new(false);
    let report = f
        .reaudit_with(&f.plan(), 2, &cancel, &|p| {
            if matches!(p, ReauditProgress::SetSaved { .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
    assert_eq!(report.stopped, Some(ReauditStop::Cancelled));
    assert_eq!(
        (report.verified, report.sets_saved, report.not_reached),
        (2, 1, 3)
    );
    let statuses = f.statuses();
    assert_eq!(
        statuses.values().filter(|s| **s == Status::Current).count(),
        2
    );
    assert_eq!(
        statuses
            .values()
            .filter(|s| **s == Status::MissingProof)
            .count(),
        3
    );
    // "After a restart": a fresh plan from the database alone is the rest.
    let rest = f.plan();
    assert_eq!((rest.games(), rest.current), (3, 2));
    let report = f.reaudit(2);
    assert_eq!((report.verified, report.saved), (3, 3));
    assert!(f.statuses().values().all(|s| *s == Status::Current));
}

#[test]
fn a_stop_during_hashing_saves_nothing_from_that_set() {
    let f = Fixture::legacy_library(4);
    let before = f.dat_rows();
    let cancel = AtomicBool::new(false);
    let report = f
        .reaudit_with(&f.plan(), 4, &cancel, &|p| {
            if matches!(p, ReauditProgress::Audit(DatAuditProgress::Hashing { .. })) {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
    assert_eq!(report.stopped, Some(ReauditStop::Cancelled));
    assert_eq!(
        (report.saved, report.sets_saved, report.not_reached),
        (0, 0, 4)
    );
    assert_eq!(f.dat_rows(), before);
    assert!(f.history().is_empty());
}

#[test]
fn content_that_no_longer_matches_is_reported_and_the_old_match_is_kept_for_the_record() {
    let f = Fixture::legacy_library(3);
    std::fs::write(f.root.join("g0001.tap"), b"not the tape it used to be").unwrap();
    let report = f.reaudit(10);
    assert_eq!((report.verified, report.no_match), (2, 1));
    assert_eq!(report.contradictions.len(), 1);
    assert_eq!(report.contradictions[0].archive_id, 2);
    let e = f.evidence();
    assert_eq!(e.states.get(&2), Some(&LibraryDatState::NoMatch));
    assert!(!e.exact.contains_key(&2));
    let kept: Vec<_> = f.history().into_iter().filter(|h| h.0 == 2).collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        (kept[0].1.as_str(), kept[0].2.as_str()),
        ("verified_single_match", "no_match")
    );
}

#[test]
fn a_file_replaced_while_it_is_hashed_is_left_out_and_the_rest_is_saved() {
    let f = Fixture::legacy_library(4);
    let swapped = Cell::new(false);
    let target = f.root.join("g0002.tap");
    let report = f
        .reaudit_with(&f.plan(), 10, &AtomicBool::new(false), &|p| {
            if matches!(p, ReauditProgress::Audit(DatAuditProgress::Hashing { .. }))
                && !swapped.replace(true)
            {
                // Same bytes, new object: the content read can no longer be
                // tied to the file that is there now.
                let replacement = target.with_extension("new");
                std::fs::write(&replacement, body(2)).unwrap();
                std::fs::rename(&replacement, &target).unwrap();
            }
        })
        .unwrap();
    assert!(report.stopped.is_none(), "{report:?}");
    assert_eq!((report.verified, report.saved), (3, 3));
    assert_eq!(report.not_checked.len(), 1);
    assert_eq!(report.not_checked[0].archive_id, 3);
    let statuses = f.statuses();
    assert_eq!(statuses[&3], Status::MissingProof);
    assert_eq!(
        statuses.values().filter(|s| **s == Status::Current).count(),
        3
    );
    // The reason survives a restart, and goes once the row is checked.
    let notes = f.notes();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].archive_id, 3);
    assert!(notes[0].reason.contains("changed"), "{notes:?}");
    let report = f.reaudit(10);
    assert_eq!((report.verified, report.saved), (1, 1));
    assert!(f.notes().is_empty());
}

#[test]
fn a_file_changed_after_the_save_is_not_current() {
    let f = Fixture::legacy_library(2);
    f.reaudit(10);
    assert!(f.statuses().values().all(|s| *s == Status::Current));
    std::fs::write(f.root.join("g0000.tap"), b"rewritten after the check!!").unwrap();
    assert_eq!(f.statuses()[&1], Status::FileChanged);
    assert!(!f.evidence().exact.contains_key(&1));
    // Same bytes written back are a new object: still not current.
    std::fs::remove_file(f.root.join("g0001.tap")).unwrap();
    std::fs::write(f.root.join("g0001.tap"), body(1)).unwrap();
    assert_ne!(f.statuses()[&2], Status::Current);
    assert_eq!(f.plan().games(), 2);
}

#[test]
fn a_catalogue_replaced_during_a_run_never_lends_its_old_identity() {
    let f = Fixture::legacy_library(3);
    let old_sha = f.preview().sha256.clone();
    let swapped = Cell::new(false);
    let report = f
        .reaudit_with(&f.plan(), 10, &AtomicBool::new(false), &|p| {
            if matches!(p, ReauditProgress::Audit(DatAuditProgress::Hashing { .. }))
                && !swapped.replace(true)
            {
                f.write_games_dat(2, "v2");
            }
        })
        .unwrap();
    // The first attempt is refused; the retry is bound to the new bytes,
    // which no longer list the third game.
    assert!(report.stopped.is_none(), "{report:?}");
    assert_eq!((report.verified, report.no_match), (2, 1));
    let new_sha = f.preview().sha256.clone();
    assert_ne!(old_sha, new_sha);
    for id in 1..=3 {
        assert_eq!(f.facts(id)["source"]["catalogue_sha256"], new_sha.as_str());
    }
    assert!(f.statuses().values().all(|s| *s == Status::Current));
}

#[test]
fn a_catalogue_changed_after_the_save_makes_the_evidence_stale() {
    let f = Fixture::legacy_library(2);
    f.reaudit(10);
    f.write_games_dat(2, "v2");
    assert!(
        f.statuses()
            .values()
            .all(|s| *s == Status::CatalogueChanged)
    );
    assert!(f.evidence().states.is_empty());
    let plan = f.plan();
    assert_eq!(plan.by_status().get(&Status::CatalogueChanged), Some(&2));
    let report = f.reaudit(10);
    assert_eq!(report.verified, 2);
    assert!(f.statuses().values().all(|s| *s == Status::Current));
}

#[test]
fn a_second_run_on_the_same_library_is_refused_while_one_is_running() {
    let f = Fixture::legacy_library(2);
    let plan = f.plan();
    let refused = Cell::new(None);
    let report = f
        .reaudit_with(&plan, 10, &AtomicBool::new(false), &|p| {
            if matches!(p, ReauditProgress::SetStarted { .. }) {
                refused.set(Some(
                    f.reaudit_with(&plan, 10, &AtomicBool::new(false), &|_| {})
                        .map(|_| ()),
                ));
            }
        })
        .unwrap();
    let refused = refused.take().unwrap().unwrap_err();
    assert!(refused.contains("already running"), "{refused}");
    assert_eq!(report.verified, 2);
}

#[test]
fn missing_files_symlinks_and_unknown_systems_are_never_checked_or_changed() {
    let f = Fixture::legacy_library(4);
    let before = f.dat_rows();
    // 1: storage gone. 2: swapped for a symlink to another game's file.
    // 3: system no longer known.
    std::fs::remove_file(f.root.join("g0000.tap")).unwrap();
    std::fs::remove_file(f.root.join("g0001.tap")).unwrap();
    std::os::unix::fs::symlink(f.root.join("g0003.tap"), f.root.join("g0001.tap")).unwrap();
    rusqlite::Connection::open(&f.db)
        .unwrap()
        .execute("DELETE FROM platform_assignments WHERE archive_id = 3", [])
        .unwrap();
    let plan = plan_reaudit(&f.db, &f.configured()).unwrap();
    let blocks: BTreeMap<i64, ReauditBlock> = plan
        .blocked
        .iter()
        .map(|b| (b.archive_id, b.block))
        .collect();
    assert_eq!(blocks.get(&1), Some(&ReauditBlock::FileUnavailable));
    assert_eq!(blocks.get(&3), Some(&ReauditBlock::SystemUnknown));
    assert_eq!(plan.games(), 2);
    let report = f
        .reaudit_with(&plan, 10, &AtomicBool::new(false), &|_| {})
        .unwrap();
    assert_eq!((report.verified, report.saved), (1, 1));
    assert_eq!(report.not_checked.len(), 1);
    assert_eq!(report.not_checked[0].archive_id, 2);
    assert!(report.not_checked[0].reason.contains("symlink"));
    // Rows 1 to 3 are byte for byte what they were.
    let after = f.dat_rows();
    for id in 1..=3 {
        assert_eq!(
            before.iter().find(|r| r.0 == id),
            after.iter().find(|r| r.0 == id)
        );
    }
}

#[test]
fn an_unavailable_catalogue_blocks_its_rows_instead_of_guessing() {
    let f = Fixture::legacy_library(2);
    let plan = plan_reaudit(&f.db, &Default::default()).unwrap();
    assert_eq!(plan.games(), 0);
    assert!(
        plan.blocked
            .iter()
            .all(|b| b.block == ReauditBlock::CatalogueUnavailable)
    );
    assert_eq!(plan.blocked.len(), 2);
}

#[test]
fn a_run_only_touches_rows_in_its_plan() {
    let f = Fixture::legacy_library(4);
    let plan = f.plan().truncated(1);
    assert_eq!(plan.games(), 1);
    let report = f
        .reaudit_with(&plan, 10, &AtomicBool::new(false), &|_| {})
        .unwrap();
    assert_eq!((report.verified, report.saved), (1, 1));
    assert_eq!(
        f.statuses()
            .values()
            .filter(|s| **s == Status::MissingProof)
            .count(),
        3
    );
}

#[test]
fn a_library_that_keeps_changing_stops_the_run_without_saving_that_set() {
    let f = Fixture::legacy_library(2);
    let next = Cell::new(100);
    let report = f
        .reaudit_with(&f.plan(), 10, &AtomicBool::new(false), &|p| {
            if matches!(p, ReauditProgress::Audit(DatAuditProgress::Hashing { .. })) {
                let id = next.replace(next.get() + 1);
                f.add(id, &format!("new{id}.tap"), Some(b"x"), None);
            }
        })
        .unwrap();
    assert!(
        matches!(&report.stopped, Some(ReauditStop::Interrupted(reason)) if reason.contains("membership")),
        "{report:?}"
    );
    assert_eq!((report.saved, report.not_reached), (0, 2));
    assert!(f.statuses().values().all(|s| *s == Status::MissingProof));
}

// ---- Eligible, not applicable, skipped: every saved result counted once ----

impl Fixture {
    /// A game on another system with an old "not in this catalogue" result
    /// from this (Spectrum) catalogue: what a whole-library check against
    /// the wrong system's catalogue used to leave behind.
    fn add_wrong_system_no_match(&self, id: i64, name: &str, platform: Option<&str>) {
        self.add(id, name, Some(b"some other system's game"), platform);
        let mut facts = self.facts(1);
        facts["verification_state"] = serde_json::json!({"state": "no_match"});
        for key in [
            "matched_entries",
            "ambiguous_candidates",
            "candidate_provenance",
        ] {
            facts[key] = serde_json::json!([]);
        }
        facts["canonical"] = serde_json::json!({"canonical_dat_name": null, "canonical_rom_name": null, "region": null, "revision": null});
        facts["hash_evidence"]["matched_algorithm"] = serde_json::Value::Null;
        facts["hash_evidence"]["matched_value"] = serde_json::Value::Null;
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.execute(
            "INSERT INTO library_dat_identities(archive_id,dat_source_id,dat_ecosystem,source_revision,verification_state,completeness,audited_at,facts_json,created_at,updated_at) \
             SELECT ?1, dat_source_id, dat_ecosystem, source_revision, 'no_match', completeness, audited_at, ?2, created_at, updated_at \
             FROM library_dat_identities WHERE archive_id = 1",
            rusqlite::params![id, serde_json::to_vec(&facts).unwrap()],
        )
        .unwrap();
    }
}

#[test]
fn old_no_match_results_from_another_systems_catalogue_are_not_applicable_and_left_alone() {
    let f = Fixture::legacy_library(3);
    // The owner's real case: other systems' files once compared with this
    // catalogue. One has no known system at all.
    f.add_wrong_system_no_match(10, "disk.d88", Some("NEC PC-8801"));
    f.add_wrong_system_no_match(11, "storyware.md", Some("Sega Pico"));
    f.add_wrong_system_no_match(12, "mystery.bin", None);
    let before = f.dat_rows();
    let plan = f.plan();
    let tally = plan.tally();
    assert!(tally.is_consistent(), "{tally:?}");
    assert_eq!(
        (
            tally.saved_results,
            tally.eligible,
            tally.not_applicable,
            tally.skipped,
            tally.current
        ),
        (6, 3, 3, 0, 0)
    );
    assert!(
        plan.blocked
            .iter()
            .all(|b| b.block == ReauditBlock::DoesNotApply && b.status == Status::DoesNotApply)
    );
    let report = f
        .reaudit_with(&plan, 10, &AtomicBool::new(false), &|_| {})
        .unwrap();
    let tally = report.tally(&plan);
    assert!(tally.is_consistent(), "{tally:?}");
    assert_eq!(
        (
            tally.completed,
            tally.verified,
            tally.failed,
            tally.not_reached,
            tally.not_applicable
        ),
        (3, 3, 0, 0, 3)
    );
    // The historical results are kept byte for byte: not rewritten, not
    // deleted, not copied to the replaced-evidence table, no note.
    let after = f.dat_rows();
    for id in 10..=12 {
        assert_eq!(
            before.iter().find(|r| r.0 == id),
            after.iter().find(|r| r.0 == id),
            "row {id}"
        );
    }
    assert!(f.history().iter().all(|h| h.0 < 10));
    assert!(f.notes().is_empty());
    // And they never become evidence of anything.
    let e = f.evidence();
    assert!((10..=12).all(|id| !e.states.contains_key(&id)));
    // Afterwards nothing is eligible; they are still counted, apart.
    let tally = f.plan().tally();
    assert_eq!(
        (tally.eligible, tally.current, tally.not_applicable),
        (0, 3, 3)
    );
}

#[test]
fn a_saved_match_for_a_game_now_on_another_system_is_checked_refused_and_counted_as_failed() {
    let f = Fixture::legacy_library(3);
    // Game 2 was verified against this catalogue; it is now assigned to a
    // system the catalogue is not for. The saved positive is not silently
    // dropped as "not applicable": it is eligible, and the check refuses it.
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    conn.execute(
        "UPDATE platform_assignments SET platform = 'Amstrad CPC' WHERE archive_id = 2",
        [],
    )
    .unwrap();
    let before = f.dat_rows();
    let plan = f.plan();
    assert_eq!(
        (plan.games(), plan.not_applicable(), plan.groups.len()),
        (3, 0, 2)
    );
    let report = f
        .reaudit_with(&plan, 10, &AtomicBool::new(false), &|_| {})
        .unwrap();
    let tally = report.tally(&plan);
    assert!(tally.is_consistent(), "{tally:?}");
    assert_eq!((tally.verified, tally.failed, tally.not_reached), (2, 1, 0));
    assert!(report.stopped.is_none(), "{report:?}");
    assert_eq!(report.not_checked[0].archive_id, 2);
    assert!(
        report.not_checked[0].reason.contains("incompatible"),
        "{:?}",
        report.not_checked
    );
    // Its old row is untouched, it is not current, and the reason is kept.
    assert_eq!(
        before.iter().find(|r| r.0 == 2),
        f.dat_rows().iter().find(|r| r.0 == 2)
    );
    assert_eq!(f.statuses()[&2], Status::MissingProof);
    assert_eq!(
        f.notes().iter().map(|n| n.archive_id).collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn a_catalogue_that_names_several_systems_verifies_nothing_until_a_system_is_chosen() {
    let f = Fixture::legacy_library(2);
    // The same catalogue file is replaced by one whose header names more
    // than one system. Nobody chose which: nothing may be verified from it.
    let entries = std::fs::read_to_string(&f.dat).unwrap();
    std::fs::write(
        &f.dat,
        entries.replace(
            "Sinclair ZX Spectrum - Games - [TAP]",
            "IBM - PC and Compatibles",
        ),
    )
    .unwrap();
    let before = f.dat_rows();
    let plan = f.plan();
    assert_eq!(plan.games(), 2);
    let report = f
        .reaudit_with(&plan, 10, &AtomicBool::new(false), &|_| {})
        .unwrap();
    let tally = report.tally(&plan);
    assert!(tally.is_consistent(), "{tally:?}");
    assert_eq!(
        (tally.completed, tally.failed, tally.not_reached),
        (0, 2, 0)
    );
    assert_eq!(f.dat_rows(), before);
    assert!(f.evidence().states.is_empty());
}

#[test]
fn a_limited_or_stopped_run_still_counts_every_saved_result_once() {
    let f = Fixture::legacy_library(5);
    f.add_wrong_system_no_match(10, "disk.d88", Some("NEC PC-8801"));
    let plan = f.plan().truncated(4);
    let tally = plan.tally();
    assert!(tally.is_consistent(), "{tally:?}");
    assert_eq!(
        (tally.eligible, tally.skipped, tally.not_applicable),
        (4, 1, 1)
    );
    let cancel = AtomicBool::new(false);
    let report = f
        .reaudit_with(&plan, 2, &cancel, &|p| {
            if matches!(p, ReauditProgress::SetSaved { .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
    let tally = report.tally(&plan);
    assert!(tally.is_consistent(), "{tally:?}");
    assert_eq!(
        (
            tally.saved_results,
            tally.eligible,
            tally.completed,
            tally.not_reached,
            tally.skipped,
            tally.not_applicable
        ),
        (6, 4, 2, 2, 1, 1)
    );
}

// ---- Process kill and restart -------------------------------------------
//
// A child process runs the re-check and is killed (SIGKILL) at arbitrary
// moments. After every kill the database must hold only two kinds of row:
// the untouched legacy row, or complete current proof that an independent
// re-hash of the file agrees with. Nothing in between may be visible.

const KILL_DIR: &str = "EMUWIZ_TEST_REAUDIT_KILL_DIR";
const KILL_SET: usize = 3;

/// The child half: only does anything when the parent set [`KILL_DIR`].
#[test]
#[ignore = "child process of the kill tests; does nothing on its own"]
fn kill_child_runs_the_recheck() {
    let Some(dir) = std::env::var_os(KILL_DIR).map(PathBuf::from) else {
        return;
    };
    let f = Fixture {
        dir: tempfile::tempdir().unwrap(),
        db: dir.join("library.sqlite3"),
        dat: dir.join("spectrum.dat"),
        root: dir.join("roms"),
    };
    let plan = plan_reaudit(&f.db, &f.configured()).unwrap();
    std::fs::write(dir.join("child-started"), b"").unwrap();
    let report = f
        .reaudit_with(&plan, KILL_SET, &AtomicBool::new(false), &|_| {})
        .unwrap();
    assert!(report.stopped.is_none(), "{report:?}");
    std::fs::write(dir.join("child-finished"), b"").unwrap();
}

fn spawn_kill_child(dir: &Path) -> std::process::Child {
    let _ = std::fs::remove_file(dir.join("child-started"));
    let _ = std::fs::remove_file(dir.join("child-finished"));
    std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "dat::library_verification::tests::reaudit::kill_child_runs_the_recheck",
            "--ignored",
            "--test-threads=1",
        ])
        .env(KILL_DIR, dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_for(path: &Path, child: &mut std::process::Child) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !path.exists() {
        if child.try_wait().unwrap().is_some() || std::time::Instant::now() > deadline {
            return path.exists();
        }
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
    true
}

/// What a killed writer may leave: SQLite's own rollback journal, over which
/// EmuWiz refuses to open the library. The way back is the copy-first
/// recovery: prepare (which must leave the library untouched and still
/// refused), then the explicit apply. Returns whether that was needed.
fn recover_like_the_documented_procedure(f: &Fixture) -> bool {
    if Database::open_read_only(&f.db).is_ok() {
        return false;
    }
    let mut journal = f.db.as_os_str().to_os_string();
    journal.push("-journal");
    let before = (
        std::fs::read(&f.db).unwrap(),
        std::fs::read(&journal).unwrap(),
    );
    let receipt = crate::prepare_journal_recovery(&f.db).unwrap();
    assert_eq!(
        before,
        (
            std::fs::read(&f.db).unwrap(),
            std::fs::read(&journal).unwrap()
        ),
        "preparing a recovery changed the library"
    );
    assert!(Database::open_read_only(&f.db).is_err());
    assert!(receipt.opens_as_current_library, "{receipt:?}");
    crate::apply_journal_recovery(&f.db, &receipt.recovery_dir).unwrap();
    Database::open_read_only(&f.db).expect("the library opens after recovery");
    true
}

/// Every saved row is either the untouched legacy row or complete current
/// proof that an independent re-hash agrees with. Returns the current count.
fn assert_only_whole_states(f: &Fixture, legacy: &BTreeMap<i64, Vec<u8>>) -> usize {
    let catalogue_sha256 = hex(&Sha256::digest(std::fs::read(&f.dat).unwrap()));
    let statuses = f.statuses();
    let rows: BTreeMap<i64, Vec<u8>> = f.dat_rows().into_iter().collect();
    assert_eq!(rows.len(), legacy.len(), "no row may appear or vanish");
    let history = f.history();
    let mut current = 0;
    for (id, bytes) in &rows {
        if bytes == &legacy[id] {
            assert_eq!(statuses[id], Status::MissingProof, "row {id}");
            assert!(
                history.iter().all(|h| h.0 != *id),
                "row {id} is unchanged yet has a replaced copy"
            );
            continue;
        }
        current += 1;
        assert_eq!(statuses[id], Status::Current, "row {id} is a partial state");
        let facts: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(
            facts["verification_state"]["state"],
            "verified_single_match"
        );
        assert_eq!(
            facts["source"]["catalogue_sha256"],
            catalogue_sha256.as_str()
        );
        let path = PathBuf::from(facts["audit_file"]["path"].as_str().unwrap());
        let content = std::fs::read(&path).unwrap();
        assert_eq!(
            content,
            body(*id as usize - 1),
            "row {id} names another file"
        );
        let sha1: String = Sha1::digest(&content)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(facts["audited_hashes"]["sha1"], sha1.as_str(), "row {id}");
        // Its legacy row was kept exactly once, byte for byte.
        let kept: Vec<_> = history.iter().filter(|h| h.0 == *id).collect();
        assert_eq!(kept.len(), 1, "row {id}");
        assert_eq!(
            kept[0].3,
            serde_json::from_slice::<serde_json::Value>(&legacy[id]).unwrap()
        );
    }
    assert_eq!(history.len(), current);
    // Sets are saved whole: never a part of one.
    assert_eq!(current % KILL_SET, 0, "a set was saved in part");
    assert_eq!(f.evidence().exact.len(), current);
    current
}

#[test]
fn killing_the_process_at_any_moment_leaves_only_whole_sets_and_a_resumable_library() {
    const GAMES: usize = 60;
    let f = Fixture::legacy_library(GAMES);
    let legacy: BTreeMap<i64, Vec<u8>> = f.dat_rows().into_iter().collect();
    let dir = f.dir.path().to_path_buf();

    // How long one uninterrupted run takes here, on a copy.
    let copy = dir.join("calibration.sqlite3");
    std::fs::copy(&f.db, &copy).unwrap();
    let started = std::time::Instant::now();
    Fixture {
        dir: tempfile::tempdir().unwrap(),
        db: copy.clone(),
        dat: f.dat.clone(),
        root: f.root.clone(),
    }
    .reaudit(KILL_SET);
    let full_run = started.elapsed();
    std::fs::remove_file(&copy).unwrap();
    let _ = std::fs::remove_file(dir.join("calibration.sqlite3.recheck.lock"));

    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let (mut kills, mut journals, mut progressed) = (0, 0, 0);
    let mut last = 0;
    for _ in 0..80 {
        if last == GAMES {
            break;
        }
        let mut child = spawn_kill_child(&dir);
        assert!(
            wait_for(&dir.join("child-started"), &mut child) || child.try_wait().unwrap().is_some()
        );
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        // Anywhere within about two sets' worth of work, so that kills
        // land in hashing, in commits and between sets, and most children
        // are killed before they finish.
        let window = full_run.as_micros() as u64 * 2 * KILL_SET as u64 / GAMES as u64;
        std::thread::sleep(std::time::Duration::from_micros(seed % window.max(1)));
        let finished = dir.join("child-finished").exists();
        child.kill().unwrap();
        child.wait().unwrap();
        if !finished {
            kills += 1;
        }
        if recover_like_the_documented_procedure(&f) {
            journals += 1;
        }
        let current = assert_only_whole_states(&f, &legacy);
        assert!(current >= last, "saved work was lost: {last} -> {current}");
        if current > last {
            progressed += 1;
        }
        last = current;
    }
    eprintln!(
        "kill test: {kills} kills, {journals} left a rollback journal, {progressed} runs saved sets, {last}/{GAMES} current before the final run"
    );
    assert!(kills >= 5, "the test never interrupted a run ({kills})");
    // The lock died with the process; an ordinary run finishes the rest.
    let report = f.reaudit(KILL_SET);
    assert!(report.stopped.is_none(), "{report:?}");
    assert_eq!(report.verified, GAMES - last);
    assert_eq!(assert_only_whole_states(&f, &legacy), GAMES);
    assert!(f.plan().groups.is_empty());
}

// ---- Discovery import Undo ------------------------------------------------

#[test]
fn a_recheck_marks_only_the_discovery_imports_of_the_games_it_rewrote() {
    let f = Fixture::legacy_library(3);
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    // One import record for a re-checked game, one for a game the run never
    // reads, both untouched so far (as hand-built rows; a real import of game
    // 1 would already be marked by the write of its first DAT row).
    conn.execute_batch(
        "INSERT INTO discovery_import_operations VALUES('op','committed','then',NULL,x'7b7d');
         INSERT INTO discovery_import_records VALUES('op',1,1,x'61',x'2f612f61',x'7b7d',0);
         INSERT INTO discovery_import_records VALUES('op',900,1,x'62',x'2f612f62',x'7b7d',0);",
    )
    .unwrap();
    // Before the run, Undo of game 1's import is already refused: its saved
    // DAT row is a protected dependent row.
    let dependents: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM library_dat_identities WHERE archive_id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dependents, 1);
    let report = f.reaudit(10);
    assert_eq!(report.verified, 3);
    let touched: BTreeMap<i64, i64> = conn
        .prepare("SELECT archive_id, touched FROM discovery_import_records")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(touched[&1], 1, "a rewritten game's import stays protected");
    assert_eq!(touched[&900], 0, "an unrelated import must not be marked");
}

// ---- Independent review tests (engine/recovery review of 8abe9b90), unmodified ----

/// A1. The group-level failure branch (reaudit.rs 605-626) pushes a note for
/// "the rest of the group". Every planned row must be counted exactly once,
/// and rows from the set that was already saved must not be among the notes.
#[test]
fn review_a_catalogue_that_fails_mid_group_accounts_for_every_row_once() {
    let f = Fixture::legacy_library(5);
    let plan = f.plan();
    let calls = Cell::new(0usize);
    let report = run_reaudit(
        &f.db,
        &plan,
        &|_| {
            let n = calls.replace(calls.get() + 1);
            if n == 0 {
                Ok(f.resolved())
            } else {
                Err("the catalogue went away".to_string())
            }
        },
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        ReauditLimits {
            max_games_per_set: 2,
            max_bytes_per_set: u64::MAX,
        },
        &|_| {},
    )
    .unwrap();
    assert_eq!(report.planned, 5);
    assert_eq!(
        (report.verified, report.saved, report.sets_saved),
        (2, 2, 1),
        "{report:?}"
    );
    let mut noted: Vec<i64> = report.not_checked.iter().map(|n| n.archive_id).collect();
    noted.sort();
    assert_eq!(noted, vec![3, 4, 5], "{report:?}");
    assert_eq!(report.not_reached, 0, "{report:?}");
    assert_eq!(
        report.planned,
        report.verified
            + report.no_match
            + report.ambiguous
            + report.not_checked.len()
            + report.not_reached
    );
    let statuses = f.statuses();
    for id in [1, 2] {
        assert_eq!(statuses[&id], Status::Current, "row {id}");
    }
    for id in [3, 4, 5] {
        assert_eq!(statuses[&id], Status::MissingProof, "row {id}");
    }
    assert_eq!(f.evidence().exact.len(), 2);
}

/// A2. The same situation: the reason those three rows were not checked
/// should survive a restart like every other "not checked" reason does.
#[test]
fn review_rows_skipped_with_their_catalogue_keep_a_saved_reason() {
    let f = Fixture::legacy_library(5);
    let plan = f.plan();
    let calls = Cell::new(0usize);
    let report = run_reaudit(
        &f.db,
        &plan,
        &|_| {
            let n = calls.replace(calls.get() + 1);
            if n == 0 {
                Ok(f.resolved())
            } else {
                Err("the catalogue went away".to_string())
            }
        },
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        ReauditLimits {
            max_games_per_set: 2,
            max_bytes_per_set: u64::MAX,
        },
        &|_| {},
    )
    .unwrap();
    assert_eq!(report.not_checked.len(), 3, "{report:?}");
    let mut saved_reasons: Vec<i64> = f.notes().iter().map(|n| n.archive_id).collect();
    saved_reasons.sort();
    assert_eq!(
        saved_reasons,
        vec![3, 4, 5],
        "the report lists these rows as not checked but no reason was saved for them"
    );
}

/// A3. One more file is replaced on every attempt of the same set. The retry
/// loop must end, every row must be reported once, and nothing may be saved
/// or promoted.
#[test]
fn review_a_different_file_changing_on_every_attempt_terminates() {
    let f = Fixture::legacy_library(4);
    let before = f.dat_rows();
    let armed = Cell::new(false);
    let next = Cell::new(0usize);
    let refusals = Cell::new(0usize);
    let report = f
        .reaudit_with(&f.plan(), 10, &AtomicBool::new(false), &|p| match p {
            ReauditProgress::SetStarted { .. } => armed.set(true),
            ReauditProgress::SetNotSaved { retrying, .. } => {
                refusals.set(refusals.get() + 1);
                armed.set(retrying);
            }
            ReauditProgress::Audit(DatAuditProgress::Hashing { .. }) => {
                if armed.replace(false) {
                    let n = next.replace(next.get() + 1);
                    if n < 4 {
                        let target = f.root.join(format!("g{n:04}.tap"));
                        let replacement = target.with_extension("new");
                        std::fs::write(&replacement, body(n)).unwrap();
                        std::fs::rename(&replacement, &target).unwrap();
                    }
                }
            }
            _ => {}
        })
        .unwrap();
    assert!(refusals.get() <= 5, "{} refusals", refusals.get());
    assert!(report.stopped.is_none(), "{report:?}");
    assert_eq!(
        (report.verified, report.saved, report.sets_saved),
        (0, 0, 0),
        "{report:?}"
    );
    let mut noted: Vec<i64> = report.not_checked.iter().map(|n| n.archive_id).collect();
    noted.sort();
    assert_eq!(noted, vec![1, 2, 3, 4], "{report:?}");
    assert_eq!(report.not_reached, 0, "{report:?}");
    assert_eq!(f.dat_rows(), before, "a refused set changed saved rows");
    assert!(f.history().is_empty());
    assert!(f.statuses().values().all(|s| *s == Status::MissingProof));
    assert!(f.evidence().states.is_empty());
    assert_eq!(f.notes().len(), 4);
}

/// A4. At 756d9740 `current_library_dat_evidence` skipped a row whose
/// catalogue is not configured *before* parsing its saved facts. It now
/// parses first (dat_current.rs 227-230), so one unreadable row belonging to
/// a removed catalogue fails the whole read that patches, trainers and the
/// library view depend on.
#[test]
fn review_an_unreadable_row_of_an_unconfigured_catalogue_is_still_skipped() {
    let f = Fixture::legacy_library(2);
    f.reaudit(10);
    assert_eq!(f.evidence().exact.len(), 2);
    rusqlite::Connection::open(&f.db)
        .unwrap()
        .execute(
            "INSERT INTO library_dat_identities \
             (archive_id, dat_source_id, dat_ecosystem, source_revision, verification_state, \
              completeness, audited_at, revision_marked_stale, facts_json, created_at, updated_at) \
             VALUES (1, 'a-catalogue-that-was-removed', NULL, NULL, 'verified_single_match', \
                     'exhaustive', '2026-01-01T00:00:00Z', 0, x'7b7d', 'now', 'now')",
            [],
        )
        .unwrap();
    f.sized();
    let evidence = Database::open_read_only(&f.db)
        .unwrap()
        .current_library_dat_evidence(&f.configured());
    let outcome = evidence
        .as_ref()
        .map(|e| e.exact.len())
        .map_err(|e| e.to_string());
    assert_eq!(
        outcome,
        Ok(2),
        "a row of a catalogue that is not configured used to be skipped unread"
    );
}

// ---- Independent review tests (Discovery gate review of 8abe9b90), unmodified ----

fn gate_touched(f: &Fixture) -> BTreeMap<i64, i64> {
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    let mut statement = conn
        .prepare("SELECT archive_id, touched FROM discovery_import_records")
        .unwrap();
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect::<BTreeMap<i64, i64>>();
    rows
}

/// An untouched import record for a real library row, bound to its real path
/// and source folder so the path-keyed triggers are exercised as well.
fn gate_import_record(f: &Fixture, archive_id: i64, name: &str) {
    let path = f.root.join(name);
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    conn.execute(
        "INSERT INTO discovery_import_operations SELECT 'gate','committed','then',NULL,x'7b7d' WHERE NOT EXISTS(SELECT 1 FROM discovery_import_operations WHERE operation_id='gate')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discovery_import_records VALUES('gate',?1,1,?2,?3,x'7b7d',0)",
        rusqlite::params![
            archive_id,
            name.as_bytes(),
            std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str())
        ],
    )
    .unwrap();
}

fn gate_dependents(f: &Fixture, archive_id: i64) -> i64 {
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    [
        "archive_scan_observations",
        "library_dat_identities",
        "library_dat_identity_history",
        "dat_recheck_attempts",
        "verified_identity_facts",
        "screenscraper_enrichments",
        "dat_set_audit_results",
    ]
    .iter()
    .map(|table| {
        conn.query_row(
            &format!("SELECT count(*) FROM {table} WHERE archive_id=?1"),
            [archive_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    })
    .sum()
}

#[test]
fn gate_a_bulk_recheck_and_its_disc_order_refresh_leave_an_unrelated_real_import_alone() {
    let f = Fixture::legacy_library(3);
    // A real library game on the same system and source folder that has
    // never been checked, so it is in no plan.
    f.add(
        50,
        "other.tap",
        Some(b"an unrelated game nobody checked"),
        Some("ZX Spectrum"),
    );
    f.sized();
    gate_import_record(&f, 50, "other.tap");
    let plan = f.plan();
    assert_eq!(plan.games(), 3);
    assert_eq!(gate_touched(&f)[&50], 0, "planning must be read-only");
    let report = f.reaudit(2);
    assert_eq!((report.verified, report.saved), (3, 3));
    assert!(
        matches!(report.disc_order, Some(Ok(_))),
        "{:?}",
        report.disc_order
    );
    assert_eq!(gate_touched(&f)[&50], 0, "an unrelated import was marked");
    assert_eq!(gate_dependents(&f, 50), 0);
    // A second run has nothing to do and still marks nothing.
    let again = f.reaudit(2);
    assert_eq!((again.saved, again.sets_saved), (0, 0));
    assert_eq!(gate_touched(&f)[&50], 0);
    assert_eq!(gate_dependents(&f, 50), 0);
}

/// The whole-platform path. An import is marked exactly when the check saved
/// a row for that game (existing design: a first DAT row is a protected
/// dependent). A game on another system is never read, written or marked.
#[test]
fn gate_a_whole_platform_check_marks_an_import_only_when_it_saves_a_row_for_that_game() {
    let f = Fixture::legacy_library(2);
    f.add(
        50,
        "other.tap",
        Some(b"an unrelated game nobody checked"),
        Some("ZX Spectrum"),
    );
    f.add(
        51,
        "elsewhere.gb",
        Some(b"another system"),
        Some("Game Boy"),
    );
    f.sized();
    gate_import_record(&f, 50, "other.tap");
    gate_import_record(&f, 51, "elsewhere.gb");
    let _totals = f.run();
    let touched = gate_touched(&f);
    eprintln!(
        "GATE-INFO whole-platform check: same-system import touched={} dependents={}",
        touched[&50],
        gate_dependents(&f, 50)
    );
    assert_eq!(
        touched[&50] == 1,
        gate_dependents(&f, 50) > 0,
        "marked without a saved row, or a saved row without the mark"
    );
    assert_eq!(touched[&51], 0, "a game on another system was marked");
    assert_eq!(gate_dependents(&f, 51), 0);
}

#[test]
fn gate_a_recheck_stopped_while_hashing_marks_no_import_at_all() {
    let f = Fixture::legacy_library(3);
    f.add(
        50,
        "other.tap",
        Some(b"an unrelated game nobody checked"),
        Some("ZX Spectrum"),
    );
    f.sized();
    gate_import_record(&f, 50, "other.tap");
    let cancel = AtomicBool::new(false);
    let report = f
        .reaudit_with(&f.plan(), 10, &cancel, &|p| {
            if matches!(p, ReauditProgress::Audit(_)) {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
    assert_eq!(report.saved, 0);
    assert_eq!(gate_touched(&f)[&50], 0);
    assert_eq!(gate_dependents(&f, 50), 0);
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    for table in ["library_dat_identity_history", "dat_recheck_attempts"] {
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

/// EXPECTED TO FAIL on 8abe9b90 (finding F2 in REVIEW.md).
///
/// A plan names game 2. Before the run reaches it, game 2 is forgotten and a
/// different game is given the free id 2 (as a Discovery import would be).
/// The run then writes a "not checked" note under id 2, which marks the new
/// game's import as modified and leaves it a dependent row, although the run
/// never read or wrote anything about the new game.
#[test]
fn f2_a_stale_plan_never_marks_or_annotates_a_newer_game_that_was_given_the_same_id() {
    let f = Fixture::legacy_library(2);
    let plan = f.plan();
    assert_eq!(plan.games(), 2);
    // Forget game 2 the way forget_missing / missing_cleanup do.
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    conn.execute_batch(
        "DELETE FROM screenscraper_enrichments WHERE archive_id=2;
         DELETE FROM library_dat_identities WHERE archive_id=2;
         DELETE FROM verified_identity_facts WHERE archive_id=2;
         DELETE FROM platform_assignments WHERE archive_id=2;
         DELETE FROM archive_scan_observations WHERE archive_id=2;
         DELETE FROM archives WHERE id=2;",
    )
    .unwrap();
    drop(conn);
    std::fs::remove_file(f.root.join("g0001.tap")).unwrap();
    // A different game arrives and is given id 2.
    f.add(
        2,
        "fresh.tap",
        Some(b"a brand new game, never checked"),
        Some("ZX Spectrum"),
    );
    f.sized();
    gate_import_record(&f, 2, "fresh.tap");
    assert_eq!(gate_touched(&f)[&2], 0);
    let report = f
        .reaudit_with(&plan, 10, &AtomicBool::new(false), &|_| {})
        .unwrap();
    eprintln!(
        "GATE-INFO stale plan run: verified={} saved={} not_checked={:?} stopped={:?}",
        report.verified, report.saved, report.not_checked, report.stopped
    );
    assert_eq!(
        gate_dependents(&f, 2),
        0,
        "the run left a row under the new game's id"
    );
    assert_eq!(
        gate_touched(&f)[&2],
        0,
        "the run marked an import it never read or wrote"
    );
}
