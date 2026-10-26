# Job #42: catalogue health and snapshot truth

Base: `0f941734144412d3e2103cdafee175512cf7079f` (origin/main on October 26).
Owned areas: catalogue missing-preview evidence, cached-row wording, Blue file
problem presentation and missing-review UI, their focused tests, and this note.
Job #22 PR #157 owns bezel apply/history; no overlap with that scope.

The reported October 26 counts (19,004 possibly moved, 13,405 source-unreachable,
36 incomplete-scan records) are observations supplied with this task, not counts
reverified against production during this work.

## Evidence contract

- Persisted `last_verified_missing_at`, `last_seen_at`, and `last_known_health`
  describe earlier catalogue observations. Loading a snapshot does not renew them.
- A missing-preview timestamp describes when the preview completed. It never
  changes a historical missing timestamp or makes an unreachable row verified.
- Possibly moved means a candidate location was found. Basename or size matches
  are suggestions; even a verified candidate hash does not authorize relinking.
- Source-unreachable means current availability prevented verification. Historical
  missing evidence is retained for explanation, with no current removal evidence.
- Incomplete scan means coverage cannot establish that an entry is gone. A
  reviewed source binding and complete scan of its current generation are required
  by the existing authority checks before an absent row can be confirmed missing.
- Blue's Problems projection uses cheap path/type checks. These cannot distinguish
  every I/O error from absence, prove source continuity, or verify game contents.
  It presents saved health as needing evidence and describes path observations
  separately. The explicit backend preview owns authoritative classifications.
- Opening the missing review does not start that filesystem preview. The person
  must request it. Results remain a dated snapshot and can be checked again.

No production scan, record removal, game-file edit, or automatic relink is part
of this job. Fixtures create their own temporary databases and synthetic files;
read-only preview tests verify preservation. Existing forget/apply/undo behavior
is outside the implementation scope and remains protected by its existing tests.

## Validation

The three new GUI regressions were run against the baseline before correction.
They failed on the “just now” claim, the `Current` badge for saved evidence, and
a preview job starting merely from opening the panel.

Core validation: 67 passed across `catalogue_health_truth`,
`catalogue_health_adversarial`, and `forget_confirmed_missing`; 25 existing
environment-dependent tests were ignored. New read-only tests check unchanged
catalogue rows/database bytes, offline and incomplete coverage, preserved
historical timestamps, and the selection token's independence from display time.

GUI validation uses `scripts/cargo-iso test --offline --locked -p archivefs-gui
--lib <filter>` for `gui_v2::missing_review::`, `gui_v2::problems::`,
`catalogue_health_ui::`, and the two row-wording tests. The headless egui checks
exercise 1024×600 and 1440×900 layouts, dated unavailable-source results, absence
of removal actions for unavailable entries, and no background preview on opening.
The PR records final GUI results and the independent review of the exact commit.

Targeted rustfmt, diff whitespace, file scope, and GUI-root boundary guards pass.
All Cargo work uses the worktree's private low-debug target. Post-correction
validation also holds the shared heavy build lock; no target belonging to another
task is reused or cleaned.

Independent review found that unavailable-for-verification can mean disabled or
no longer configured even when storage is reachable. The group now names all
three cases and retains the backend's per-entry reason. “Scan coverage
incomplete” also avoids inventing a failed scan for a never-scanned source.
A disabled-but-reachable synthetic fixture protects this distinction.
