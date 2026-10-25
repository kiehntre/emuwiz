use super::*;

fn history(result: CheckResult) -> CheckHistory {
    let mut history = CheckHistory::default();
    history.record(Check {
        installed_sha256: "a".repeat(64),
        checked_at: 100,
        available_version: None,
        result,
    });
    history
}

#[test]
fn current_and_outdated_use_provider_comparison_not_version_sorting() {
    for (result, expected) in [
        (CheckResult::Current, Health::Current),
        (
            CheckResult::UpdateAvailable {
                available_version: "opaque-provider-revision".into(),
            },
            Health::UpdateAvailable,
        ),
    ] {
        let assessment = assess_health(Some(&"a".repeat(64)), &history(result), 101);
        assert_eq!(assessment.health, expected);
        assert_eq!(assessment.last_checked, Some(100));
    }
}

#[test]
fn replaced_and_malformed_hashes_never_inherit_currentness() {
    let history = history(CheckResult::Current);
    for hash in [
        None,
        Some(""),
        Some("invalid"),
        Some("bbbb"),
        Some(&"b".repeat(64)),
    ] {
        assert_eq!(assess_health(hash, &history, 101).health, Health::Unknown);
    }
    assert_eq!(
        assess_health(Some(&"a".repeat(64)), &CheckHistory::default(), 101).health,
        Health::Unknown
    );
}

#[test]
fn stale_and_future_checks_are_not_current_or_eligible() {
    for result in [
        CheckResult::Current,
        CheckResult::UpdateAvailable {
            available_version: "revision".into(),
        },
    ] {
        let history = history(result);
        let stale = assess_health(Some(&"a".repeat(64)), &history, 101 + STALE_AFTER_SECONDS);
        assert_eq!(stale.health, Health::Stale);
        assert_eq!(
            plan_update_all(&stale, true, false),
            PlanDisposition::CheckFirst
        );
        assert_eq!(
            assess_health(Some(&"a".repeat(64)), &history, 99).health,
            Health::Unknown
        );
    }
}

#[test]
fn provider_failure_preserves_success_but_shows_check_failed() {
    let mut history = history(CheckResult::UpdateAvailable {
        available_version: "next".into(),
    });
    let success = history.last_success.clone();
    history.record(Check {
        installed_sha256: "a".repeat(64),
        checked_at: 102,
        available_version: None,
        result: CheckResult::Failed {
            detail: "offline".into(),
        },
    });
    assert_eq!(history.last_success, success);
    let assessment = assess_health(Some(&"a".repeat(64)), &history, 103);
    assert_eq!(assessment.health, Health::CheckFailed);
    assert_eq!(assessment.available_version.as_deref(), Some("next"));
    assert_eq!(
        plan_update_all(&assessment, true, false),
        PlanDisposition::CheckFirst
    );
}

#[test]
fn unavailable_services_never_prove_currentness() {
    for outcome in [
        ManagedDatUpdateOutcome::Offline,
        ManagedDatUpdateOutcome::RateLimited {
            retry_after_seconds: Some(120),
        },
        ManagedDatUpdateOutcome::Failed {
            kind: ManagedDatUpdateFailureKind::Network,
            detail: "offline".into(),
        },
    ] {
        let check = check_from_managed_outcome(&outcome, "a".repeat(64), 100);
        let mut history = CheckHistory::default();
        history.record(check);
        assert_eq!(
            assess_health(Some(&"a".repeat(64)), &history, 101).health,
            Health::CheckFailed
        );
    }
}

#[test]
fn unknown_version_and_unsupported_provider_stay_unknown() {
    let check = check_from_managed_outcome(
        &ManagedDatUpdateOutcome::UpdateAvailable {
            upstream_revision: " ".into(),
        },
        "a".repeat(64),
        100,
    );
    assert_eq!(check.result, CheckResult::Unknown);
    let assessment = assess_health(Some(&"a".repeat(64)), &history(CheckResult::Unknown), 101);
    assert_eq!(
        plan_update_all(&assessment, false, false),
        PlanDisposition::Unsupported
    );
    assert_eq!(
        plan_update_all(&assessment, true, true),
        PlanDisposition::Conflict
    );
}

#[test]
fn current_retains_known_available_revision() {
    let check = check_from_managed_outcome(
        &ManagedDatUpdateOutcome::UpToDate {
            upstream_revision: Some("known".into()),
        },
        "a".repeat(64),
        100,
    );
    let mut history = CheckHistory::default();
    history.record(check);
    assert_eq!(
        assess_health(Some(&"a".repeat(64)), &history, 101)
            .available_version
            .as_deref(),
        Some("known")
    );
}

#[test]
fn current_without_a_provider_revision_remains_unknown() {
    // A 304 with incomplete installed metadata is not proof of currentness.
    for revision in [None, Some(" ".into())] {
        let check = check_from_managed_outcome(
            &ManagedDatUpdateOutcome::UpToDate {
                upstream_revision: revision,
            },
            "a".repeat(64),
            100,
        );
        assert_eq!(check.result, CheckResult::Unknown);
    }
}
