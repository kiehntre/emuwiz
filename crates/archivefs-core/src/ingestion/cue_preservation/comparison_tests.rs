use super::*;
use std::cell::RefCell;
use std::fs::FileTimes;

type ReadHook = Box<dyn FnMut(&Path, u64)>;
thread_local! {
    static READ_HOOK: RefCell<Option<ReadHook>> = RefCell::new(None);
}

pub(super) fn after_source_read(path: &Path, bytes: u64) {
    READ_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(path, bytes);
        }
    });
}

struct HookGuard;
impl HookGuard {
    fn install(hook: impl FnMut(&Path, u64) + 'static) -> Self {
        READ_HOOK.with(|slot| assert!(slot.replace(Some(Box::new(hook))).is_none()));
        Self
    }
}
impl Drop for HookGuard {
    fn drop(&mut self) {
        READ_HOOK.with(|slot| slot.replace(None));
    }
}

const SINGLE: &str = "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 01 00:00:00\n";

struct Fixture {
    _dir: tempfile::TempDir,
    cue: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn new(value: u8) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cue = dir.path().join("disc.cue");
        let bin = dir.path().join("disc.bin");
        std::fs::write(&bin, vec![value; 8192]).unwrap();
        std::fs::write(&cue, SINGLE).unwrap();
        Self {
            _dir: dir,
            cue,
            bin,
        }
    }
    fn evidence(&self) -> Result<DiscLayoutEvidence, CuePreservationError> {
        let evidence = collect_cue_preservation(&self.cue);
        assert!(evidence.is_ok());
        evidence
    }
}

fn edit_restoring_mtime(path: &Path) {
    use std::io::{Seek, SeekFrom, Write};
    let before = std::fs::metadata(path).unwrap();
    let mut file = File::options().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(4096)).unwrap();
    file.write_all(&[9]).unwrap();
    file.set_times(FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    let after = std::fs::metadata(path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
}

fn replace(path: &Path) {
    let replacement = path.with_file_name("replacement.bin");
    std::fs::write(&replacement, vec![1; 8192]).unwrap();
    std::fs::rename(replacement, path).unwrap();
}

fn assert_indeterminate(result: CuePreservationComparison) {
    assert_eq!(result.outcome, CueComparisonOutcome::Indeterminate);
    assert_eq!(result.programs_match, None);
    assert!(result.differences.is_empty());
    assert!(!result.refusals.is_empty());
}

fn on_first_read(trigger: PathBuf, mut mutation: impl FnMut() + 'static) -> HookGuard {
    let mut fired = false;
    HookGuard::install(move |path, _| {
        if path == trigger && !fired {
            fired = true;
            mutation();
        }
    })
}

#[test]
fn left_changes_during_right_validation_with_same_size_and_restored_mtime() {
    let left = Fixture::new(1);
    let right = Fixture::new(1);
    let a = left.evidence();
    let b = right.evidence();
    let bin = left.bin.clone();
    let _hook = on_first_read(right.bin.clone(), move || edit_restoring_mtime(&bin));
    assert_indeterminate(compare_cue_evidence(&a, &b));
}

#[test]
fn right_changes_during_left_validation_with_same_size_and_restored_mtime() {
    let left = Fixture::new(1);
    let right = Fixture::new(1);
    let a = left.evidence();
    let b = right.evidence();
    let bin = right.bin.clone();
    let _hook = on_first_read(left.bin.clone(), move || edit_restoring_mtime(&bin));
    assert_indeterminate(compare_cue_evidence(&a, &b));
}

#[test]
fn atomic_replacement_of_either_operand_during_the_other_validation() {
    for left_is_victim in [true, false] {
        let left = Fixture::new(1);
        let right = Fixture::new(1);
        let a = left.evidence();
        let b = right.evidence();
        let (victim, trigger) = if left_is_victim {
            (left.bin.clone(), right.bin.clone())
        } else {
            (right.bin.clone(), left.bin.clone())
        };
        let _hook = on_first_read(trigger, move || replace(&victim));
        assert_indeterminate(compare_cue_evidence(&a, &b));
    }
}

#[test]
fn final_pair_sweep_rejects_left_replacement_during_final_right_content_read() {
    let left = Fixture::new(1);
    let right = Fixture::new(1);
    let a = left.evidence();
    let b = right.evidence();
    let victim = left.bin.clone();
    let trigger = right.bin.clone();
    let mut reads = 0;
    let _hook = HookGuard::install(move |path, _| {
        if path == trigger {
            reads += 1;
            if reads == 2 {
                replace(&victim);
            }
        }
    });
    assert_indeterminate(compare_cue_evidence(&a, &b));
}

#[test]
fn source_relationship_change_during_other_operand_validation() {
    let left = Fixture::new(1);
    let right = Fixture::new(1);
    let link = left._dir.path().join("link.bin");
    std::os::unix::fs::symlink("disc.bin", &link).unwrap();
    std::fs::write(&left.cue, SINGLE.replace("disc.bin", "link.bin")).unwrap();
    std::fs::write(left._dir.path().join("other.bin"), vec![1; 8192]).unwrap();
    let a = left.evidence();
    let b = right.evidence();
    let _hook = on_first_read(right.bin.clone(), move || {
        let replacement = link.with_file_name("new-link.bin");
        std::os::unix::fs::symlink("other.bin", &replacement).unwrap();
        std::fs::rename(replacement, &link).unwrap();
    });
    assert_indeterminate(compare_cue_evidence(&a, &b));
}

#[test]
fn two_stable_identical_operands_remain_equivalent() {
    let left = Fixture::new(1);
    let right = Fixture::new(1);
    let a = left.evidence();
    let b = right.evidence();
    for _ in 0..3 {
        let result = compare_cue_evidence(&a, &b);
        assert_eq!(result.outcome, CueComparisonOutcome::Equivalent);
        assert_eq!(result.programs_match, Some(true));
        assert!(result.differences.is_empty());
        assert!(result.refusals.is_empty());
    }
}

#[test]
fn two_stable_different_operands_remain_different() {
    let left = Fixture::new(1);
    let right = Fixture::new(2);
    let a = left.evidence();
    let b = right.evidence();
    for _ in 0..3 {
        let result = compare_cue_evidence(&a, &b);
        assert_eq!(result.outcome, CueComparisonOutcome::Different);
        assert_eq!(result.programs_match, Some(false));
        assert!(result.refusals.is_empty());
        assert!(
            result
                .differences
                .contains(&CueLayoutDifference::SourceContent { ordinal: 0 })
        );
        assert!(
            result
                .differences
                .contains(&CueLayoutDifference::ProgramContent { track: 1 })
        );
    }
}

#[test]
fn evidence_stale_before_comparison_is_indeterminate() {
    let left = Fixture::new(1);
    let right = Fixture::new(1);
    let a = left.evidence();
    let b = right.evidence();
    edit_restoring_mtime(&left.bin);
    assert_indeterminate(compare_cue_evidence(&a, &b));
}

#[test]
fn equal_observed_metadata_does_not_replace_content_proof() {
    let left = Fixture::new(1);
    let right = Fixture::new(1);
    let mut a = left.evidence();
    let b = right.evidence();
    validate_evidence(a.as_ref().unwrap()).unwrap();
    validate_evidence(b.as_ref().unwrap()).unwrap();
    edit_restoring_mtime(&left.bin);
    // Model metadata equality using the file's real current metadata and the
    // independently captured earlier digest. No timestamp is fabricated.
    a.as_mut().unwrap().sources[0].identity =
        identity(&std::fs::metadata(&left.bin).unwrap()).unwrap();
    assert_eq!(
        open_source(&left.bin).unwrap().1,
        a.as_ref().unwrap().sources[0].identity
    );
    // Challenge finalisation itself after successful initial validation.
    // A metadata-only pair sweep would accept this coarse-metadata model.
    assert_eq!(
        revalidate_comparison_pair(a.as_ref().unwrap(), b.as_ref().unwrap()),
        vec![changed(&left.bin)]
    );
    assert_indeterminate(compare_cue_evidence(&a, &b));
}

#[test]
fn change_immediately_before_final_verification_is_indeterminate() {
    let left = Fixture::new(1);
    let right = Fixture::new(1);
    let a = left.evidence();
    let b = right.evidence();
    assert_indeterminate(compare_with(&a, &b, || edit_restoring_mtime(&left.bin)));
}

#[test]
fn missing_operand_immediately_before_final_verification_is_indeterminate() {
    let left = Fixture::new(1);
    let right = Fixture::new(1);
    let a = left.evidence();
    let b = right.evidence();
    assert_indeterminate(compare_with(&a, &b, || {
        std::fs::remove_file(&right.bin).unwrap()
    }));
}
