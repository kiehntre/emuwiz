use super::*;
use crate::daedalus::evidence::EvidenceState;

#[test]
fn versions_are_read_from_their_first_two_numbers() {
    assert_eq!(major_minor("0.4.11"), Some((0, 4)));
    assert_eq!(major_minor("v0.3-beta"), Some((0, 3)));
    assert_eq!(major_minor(" 1.21.0 "), Some((1, 21)));
    assert_eq!(major_minor("nightly"), None);
    assert_eq!(major_minor("7"), None);
    assert_eq!(major_minor(""), None);
}

#[test]
fn an_unreadable_version_is_unknown_not_too_old() {
    let entry = &MINIMUM_VERSIONS[0];
    assert_eq!(violated(entry, Some("0.3.5")), Some(true));
    assert_eq!(violated(entry, Some("0.4.0")), Some(false));
    assert_eq!(violated(entry, Some("1.0")), Some(false));
    assert_eq!(violated(entry, Some("git-abcdef")), None);
    assert_eq!(violated(entry, None), None);
}

#[test]
fn recording_marks_an_undeterminable_limit_unavailable() {
    let mut set = EvidenceSet::default();
    assert_eq!(record(&mut set, "openrct2", None).len(), 1);
    assert!(matches!(
        set.items()[0].state,
        EvidenceState::Unavailable { .. }
    ));
    assert!(record(&mut set, "retroarch", Some("1.21.0")).is_empty());

    let mut set = EvidenceSet::default();
    record(&mut set, "openrct2", Some("0.3.0"));
    assert!(matches!(
        set.items()[0].fact,
        Fact::KnownLimitation { violated: true, .. }
    ));
    assert_eq!(set.items()[0].trust, Trust::Measured);
}

#[test]
fn table_entries_are_unique_and_say_where_they_come_from() {
    let mut ids: Vec<&str> = MINIMUM_VERSIONS.iter().map(|entry| entry.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), MINIMUM_VERSIONS.len());
    assert!(
        MINIMUM_VERSIONS
            .iter()
            .all(|entry| !entry.basis.is_empty() && entry.version >= 1)
    );
}
