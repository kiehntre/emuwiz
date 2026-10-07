# Organisation shared-test migration and History integration

Current phase: a separate integration candidate based on promoted History
`fdc8acbdb7c84d12f6a2ccbbd74c536f2bbfc20d`. The seven outdated Organisation
assertions are migrated, actual action clicks are exercised, and visibility is
checked at 1024×600. The History test module and shared App fixture remain
byte-identical to promoted main. Final execution results are recorded below.

The preparation and independent-validation sections below are historical records
from before History landed. The original dirty Organisation worktree and backups
are preserved; no dirty rebase, reset or stash was used.

## Preserved work and ownership

Organisation worktree: `/home/davedap/emuwiz-gui-organisation-playing-library`.
Branch: `fix/gui-v2-organisation-playing-library`.
Starting HEAD: `ac37f78e2b59e53fdcdae5f44da8c1571accd04b`.
The four modified Rust files, new presentation helper and Sunshine checklist were
backed up outside Git before continuation. The work remains uncommitted.

Independent scope is the existing dedicated Organisation, Playing Library and
equivalent-duplicate presenters, their feature-local tests, and these testing
documents. No shared routes, GUI root, History page, fixture or assertion is
modified in this phase. The intended four task surfaces replace the old five-card
primary layout; do not restore that layout to satisfy outdated assertions.

## Assertions to migrate after History lands

Names below refer to the starting tree; find the functions by name after integration
rather than relying on line numbers.

| Existing test | Old assumption | Replacement assertion |
| --- | --- | --- |
| `gui_v2_organisation_landing_uses_user_intents` | Rename, build, Fix MAME and Analyse MAME labels appear on first paint | Playing Library is the primary task; Review duplicates, Review collection and Open MAME are the secondary task actions. Check actual viewport visibility, scrolling when necessary. |
| `gui_v2_organisation_is_a_native_plain_english_workflow` | Rename/build/Fix MAME are all normal primary cards; safety wording is `Source untouched` | Normal mode explains linked output, preserved originals, review-before-confirm, and the four task choices. Verify safety meaning rather than the old card list. |
| `gui_v2_organisation_advanced_options_stay_native` | Fix MAME is visible beside Advanced options | Advanced controls remain native; MAME is reached through Open MAME and the canonical MAME surface. No legacy window is launched. |
| `gui_v2_organisation_sidebar_title_and_all_normal_flows_are_reachable` | Generic flow title is `Generic Library` | Generic title is `Playing Library`; RomM, ES-DE and RetroDECK retain their destination-specific titles and source-preservation guidance. |
| `gui_v2_organisation_landing_remains_usable_at_supported_viewports` | Rename is the first visible action at every size | Preview Playing Library is visible and clickable at the supported sizes. Add 1024×600 and verify the clipped rectangle, not just emitted text. |
| `gui_v2_organisation_landing_shows_the_plain_english_explainer_alongside_actions` | All five destination/naming cards are initially expanded | Review guidance and the primary Playing Library action are initially visible. Expand `Other output destinations and verified-file naming` before checking those five existing actions. |
| `gui_v2_organisation_each_target_card_routes_to_its_own_destination` | Setting the state directly proves card click routing | Retain destination rendering checks and add real clicks: primary generic action; expand the disclosure, then click each existing destination action. Assert route, subview, destination and back navigation. |
| `gui_v2_organisation_landing_is_reachable_at_narrow_1280x720` | Rename is the first card; comments assume five normal cards | Assert hero, preservation/review guidance and Preview Playing Library; update comments for task-oriented navigation and collapsible secondary destinations. |

The broad 1024×600 primary-action sweep (currently containing a Section::Build
list of Rename/build/Organisation) must explicitly include `Preview Playing
Library`. Its existing `Organisation` title fallback may let the test pass without
proving the primary action remains usable. The core action-card definition test
in `organisation.rs` still validates all five backed actions, not five expanded
normal-mode cards; its purpose must remain clear.

## Every existing action remains accessible

| Existing capability | Intended access | Existing destination/handler |
| --- | --- | --- |
| Generic Playing Library | Primary Preview Playing Library; also existing advanced card | `OrganisationView::PlayingLibrary`, Generic destination, existing preview job |
| Verified-file naming | Expand Other output destinations and verified-file naming → Rename verified games | `OrganisationView::VerifiedGames`, existing canonical rename preview/apply |
| RomM output | Same disclosure → Organise for RomM | Existing RomM destination and visibility checks |
| ES-DE output | Same disclosure → Export to ES-DE | Existing ES-DE destination; metadata publication stays separate |
| RetroDECK output | Same disclosure → Prepare for RetroDECK | Existing RetroDECK destination and sandbox visibility checks |
| Duplicate review | Review duplicates | `Route::Section(Section::Duplicates)`; existing exact/equivalent review and quarantine confirmation |
| Collection review | Review collection | `Route::Section(Section::Problems)`; existing current evidence projection |
| MAME organisation | Open MAME, then existing canonical MAME organisation workflow | `Route::Section(Section::Mame)` and existing `Route::MameWorkflow`; no generic MAME shortcut |
| Region/language/revision preferences | Existing Playing Library preferences and Advanced organisation options | Existing policy fields; no new election rules |
| Supported undo/recovery | Existing workflow recovery and History & Undo | Existing durable journals/receipts; no parallel History implementation |

The landing action opens the existing preview workflow. It must not create output
or start an unconfigured audit simply by navigating. Review/preview/apply remain
distinct stages. Rename changes originals only through its existing reviewed
executor; linked Playing Library preserves originals. Duplicate quarantine moves
reviewed redundant files, preserves the preferred copy and does not free disk space.

## Required regression assertions

- Real clicks on all four task actions reach the existing canonical destinations.
- Expand the disclosure and click every existing naming/output action; do not
  replace accessibility checks with state assignment alone.
- One primary landing action; no duplicate equal-weight MAME buttons, force
  overwrite, automatic delete or unreviewed apply.
- MAME set organisation does not offer the removed generic Playing Library shortcut.
- Preview summary, destination and preservation information precede Apply.
  Preview-only/unverified visibility/conflicting destinations explain refusal.
- Planner reasons are visible normally; Advanced retains DAT, region/revision,
  rejected-candidate, source/output and conflict evidence.
- Filename-only/unmatched files and unresolved elections are Needs review;
  unrepresented files are not labelled with an invented specific failure reason.
- Unfiltered counts survive filtering and page changes. Companion files already
  represented by an accepted launcher are not counted as unknown.
- Review page boundaries work across selected, needs-review, conflicts and excluded
  categories; later rows remain reachable; filter changes reset presentation page.
- Empty source, no verified eligible games, no conflicts, filtered-empty and
  already-correct output are distinct states.
- Paint-only frames emit no operation intent or filesystem writes and do not
  change the reviewed plan or selected family. Repeated row controls use semantic IDs.
- Duplicate preferred-copy/reason display precedes confirmation; redundant-byte
  estimates are not labelled freed disk space. Quarantine remains explicit and undoable.
- Prior receipts/recovery banners are historical operation state, never inputs to
  current selection counts. Current Organisation and History remain separate surfaces.

Feature-local tests cover many of these independently. Shared App-level navigation
and viewport assertions remain blocked until History's fixtures are integrated.

## Fixtures and History interactions

Preserve the promoted History fixture exactly, including its new `history_view`
state, transaction entry helpers and two-frame History rendering where required.
Do not copy the old App fixture over it or revert promoted History assertions.
Existing `fixture`, `frame`, `text` and click helpers can drive Organisation tests;
use disposable synthetic files for preview/apply integration. Initialise new
presentation state through `PlayingLibraryPageState::default()`; no DB migration,
production save/game data or new filesystem engine is needed.

Keep History assertions for real entries, truthful empty states, filters, bounded
pages, availability/refusal of undo, cancellation and operation provenance. Any
Organisation test that visits History must assert the promoted wording/model,
not resurrect `Built Playing Library` / `Ready to undo` old labels. Preserve test
helpers and imports added by History. Test both cached-model frames when necessary.

## Integration sequence and gate

1. History must be safely promoted first. Fetch and verify origin/main against
   `git ls-remote origin refs/heads/main`; confirm the promoted History commit is
   included, not merely that an unrelated commit advanced main.
2. Re-inspect both lanes and preserve a fresh complete backup of Organisation's
   tracked and untracked files. Do not rebase this dirty worktree or reset/stash it.
3. After verifying History promotion, use a clean candidate based on the verified
   main and replay only the reviewed Organisation diff and
   untracked files, preserving the original dirty worktree. Stop on unexpected
   conflicts. No unrelated merges or pushes.
4. Update only Organisation assertions in the now-integrated shared tests. Preserve
   all History fixtures/assertions, then add actual click/viewport regression checks.
5. Run focused Organisation/Playing Library and History regression tests, then
   broader GUI-v2 tests and workspace checks on that exact candidate. Keep failures
   visible; classify genuine defects separately from migrated wording assumptions.
6. Confirm no unintended files changed. Report the final base, diff, test counts,
   remaining failures and readiness. Commit/push only within the authorisation then
   in force. Until this gate passes the Organisation candidate is not integration-ready.

## Independent validation record

Results are recorded below after execution. A prior 22-test pass predates the latest
paging refinement and must not be substituted for the current run.

### Executed independent results

Target: `/home/davedap/.cache/emuwiz-organisation-playing-library-target`.
All Cargo commands use `--offline --locked`, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_INCREMENTAL=0` and `CARGO_BUILD_JOBS=2`.
Debug-symbol/incremental settings limit disk usage in this dedicated target; they
do not replace the tests or disable assertions.

- The first current `organisation_usability` run: **22 passed, 1 failed**. The
  paging refinement passed. The Advanced-details test clicked an ambiguous
  whole-page disclosure label also used by the asynchronous catalogue picker.
  Its feature-local harness now exercises the intended preview disclosure with
  real pointer events. No production code changed in this continuation.
- `cargo test -p archivefs-gui --lib playing_library_page:: --no-fail-fast`:
  **65 passed, 0 failed**, including the corrected disclosure test and current
  second-page rendering/count checks.
- `cargo test -p archivefs-gui --lib gui_v2::organisation:: --no-fail-fast`:
  **10 passed, 0 failed**.
- `cargo test -p archivefs-gui --lib gui_v2::equivalent_duplicates:: --no-fail-fast`:
  **12 passed, 0 failed**.
- Total current feature-local suites: **87 passed, 0 failed**. The first failed
  run is retained in `/home/davedap/.cache/organisation-independent-focused.log`;
  it is not described as a pass.
- Diagnostic `cargo test -p archivefs-gui --lib gui_v2_organisation --no-fail-fast`
  against the unmodified shared file: **2 passed, 7 failed**. Passing tests are
  destination state/rendering and navigation-away/back checks. They do not prove
  actual landing-card click access.

Observed shared failures:

1. `gui_v2_organisation_landing_uses_user_intents`: expects visible Rename.
2. `gui_v2_organisation_is_a_native_plain_english_workflow`: expects visible Rename.
3. `gui_v2_organisation_advanced_options_stay_native`: expects Fix my MAME library.
4. `gui_v2_organisation_sidebar_title_and_all_normal_flows_are_reachable`: expects Generic Library.
5. `gui_v2_organisation_landing_remains_usable_at_supported_viewports`: expects Rename at 1280×720.
6. `gui_v2_organisation_landing_shows_the_plain_english_explainer_alongside_actions`: expects expanded Rename/all five cards.
7. `gui_v2_organisation_landing_is_reachable_at_narrow_1280x720`: expects visible Rename.

Diagnostic log:
`/home/davedap/.cache/organisation-independent-shared-organisation-diagnostics.log`.
These are intentionally unmigrated assertions, not passing tests. The new landing
has not been weakened to satisfy them. App-level action access/viewport regression
coverage must still be added and validated after History promotion.

- `cargo check --offline --locked --workspace --all-targets`: **passed**, using
  the dedicated target above. Warnings are in untouched files; no unrelated
  warning fixes were made.
- `cargo fmt --all -- --check`: **passed** after the feature-local test update.
- `git diff --check`: **passed**; file-scope and History overlap checks passed.
- Final live `git ls-remote origin refs/heads/main` and local origin/main both
  remain `ac37f78e2b59e53fdcdae5f44da8c1571accd04b`. History has not landed.
- Organisation branch/HEAD remain unchanged; all work is deliberately uncommitted.
  No shared file edits, Git reset/stash/rebase/merge, commit or push were performed.
- Broader GUI-v2 tests against integrated History, History regressions, release
  build/version and manual Sunshine checks are **not executed in this phase**.

**Integration readiness: NO.** Wait for verified History promotion, then reconcile
on a clean verified-main candidate while retaining this dirty worktree and its
backups, migrate the shared assertions, preserve the new History fixture, and run
both lanes' regression and broader GUI validation before claiming readiness.

## Integration execution record — promoted History base

Fresh fetch and independent live-remote checks agree on
`fdc8acbdb7c84d12f6a2ccbbd74c536f2bbfc20d`. Origin is the GitHub repository
`kiehntre/emuwiz`. Integration worktree:
`/home/davedap/emuwiz-gui-organisation-history-integration`; branch:
`integration/gui-v2-organisation-history-20261007`.

The four tracked changes and all three untracked files were copied from the
preserved Organisation worktree. Original HEAD, porcelain status and SHA-256 of
each copied file were recorded and checked unchanged. Backups were untouched.
History's implementation files, shared App fixture and History test module are
unchanged from promoted main; no History cache, undo or recovery code is replaced.

The seven failing shared assertions now describe the four-task landing. Related
navigation assertions click all existing naming/output actions after disclosure,
check their destination and back control, exercise Advanced preferences and the
three review handoffs, and refuse generic Playing Library output in MAME. The
primary-action sweep requires the actual button, without a title fallback.

Two integration issues were retained in the logs and resolved:

- Initial shared run: 8 passed, 2 failed. The 1024×600 primary action was clipped;
  it now precedes wrapping explanatory text within the Playing Library card.
  The disclosure-content test now scrolls to inspect the last action. Its real
  click regression also independently reaches every existing destination.
- Initial Playing Library run: 64 passed, 1 failed. The RomM rollback receipt
  was below the old fixed-height frame after review/catalogue content expanded.
  The receipt test now renders the complete profile at 2600px, retains every
  message/filesystem assertion and additionally asserts `RolledBack` state.
  Fixed-viewport accessibility remains a separate real clipped-rectangle test.

Current focused runs, all `--offline --locked` with `--test-threads=1`:

- `playing_library_page::`: 65 passed.
- `gui_v2::organisation::`: 10 passed.
- `gui_v2::equivalent_duplicates::`: 12 passed.
- `gui_v2_organisation`: 11 passed.
- `history_view::`: 21 passed.
- `history_page::`: 63 passed.

Dedicated Cargo target:
`/home/davedap/.cache/emuwiz-organisation-history-integration-target`.
Existing compiled artifacts were copied as a build-cache seed; Cargo validation
and release compilation run against the integration source tree. Debug and test
profiles use no debug symbols or incremental compilation, with two build jobs.

The History Sunshine checklist remains outstanding as a release acceptance
requirement. No automated test result substitutes for that manual review.

Broader validation on the integrated Rust tree:

- Complete `gui_v2::` suite, serial: **769 passed, 0 failed, 8 ignored**.
- `cargo check --offline --locked --workspace --all-targets`: **passed**.
  Warnings refer to untouched files; no unrelated warning fixes were made.
- `cargo fmt --all -- --check`: **passed**.
- `git diff --check`: **passed**.

- `cargo build --offline --locked -p archivefs-gui --release --bin emuwiz`:
  **passed** with the normal optimized release profile.
- Headless release `--version`: **`emuwiz 0.9.0 · GUI v2 (native-v2)`**.
  No desktop session or `DISPLAY=:0` was launched.

Automated integration validation is complete. The local candidate contains only
Organisation presentation, test migration and these documents; no backend
engine, planner, DAT/catalogue behaviour, database migration or History
implementation is added or changed. Promotion requires separate authorisation.
Manual History Sunshine acceptance remains outstanding for release acceptance.
