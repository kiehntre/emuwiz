# GUI v2 whole-product UX pass

Starting base: `1571f7e56500d75b383c2aa5c34f632d7f487656`.
Reconciled base: `401af8c9d16478a282f08e7c073736785038cd6b`.
Branch: `feature/gui-v2-whole-product-ux-mega-pass`.

## Product map and ownership

Study covers the 39 top-level GUI-v2 Rust modules (including the shell), their entry points, primary
presentation and worker/state boundaries, plus shared components, existing GUI
tests, the milestone feature map and live-QA notes, and the synthetic lab tool.
The old milestone feature map is historical: current source, not its old
Native/Handoff labels, defines current capabilities.

| User journey | Current ownership / implementation | Work policy |
|---|---|---|
| Home, library, search, Game Details, settings, Advanced, notifications | pages.rs, mod.rs, library.rs, visual_pages.rs, browse_play.rs | Shell/pages owned; improve free focused surfaces only |
| Routes, Back/Home/Escape, controller-relevant keyboard input | routes.rs, pages.rs; egui focus; no new global navigation system | Owned; preserve and test existing behaviour |
| Setup, emulator discovery/install, BIOS, launch readiness/failure | onboarding.rs, environment.rs, launch_readiness_summary.rs, native_workflows.rs, embedded existing setup tools | Native workflow glue owned; focused presentation free |
| Problems, Missing Games, exact/equivalent duplicates | problems.rs, missing_review.rs, equivalent_duplicates.rs; shared repair journals | Preserve reviewed operations and refusals |
| Multi-disc / platform / storage | media_sets.rs, visual_pages.rs, storage_review.rs | Read-only workers; guard stale results and explain retry |
| Organisation, MAME reconstruction | organisation.rs, mame_collection_health.rs, shared canonical core APIs | Organisation owned; do not change backend or publication |
| MAME / DAT / Sources / providers / RomM | native_workflows.rs, sources_providers.rs, routes.rs, backend.rs, romm_library.rs | Active RomM owner; skip integration glue |
| Metadata, artwork, bezels | artwork.rs, media_sources.rs, imagery.rs, bezel.rs; embedded provider setup | Preserve identity and network policy |
| Cheats / mods | mods.rs delegates to canonical cheat/package pages, shared history | GUI-only interpretation; parsing does not prove efficacy |
| Saves / states / Save Vault | saves_states.rs; existing persistent-state worker and separate card tools | Read-only inventory, no invented restore capability |
| Manuals / PDF / CBZ / CBR | documents.rs provides friendly errors and canonical capabilities; pages.rs renders | Wiring blocked by owned pages.rs; no renderer/decoder added |
| Archive inspection | archive_inspector.rs; bounded canonical readers | No extraction, retry explicit |
| Optical / Wii U / conversion | embedded optical conversion panel, wiiu_disc.rs and durable queue controller | Preserve existing conversion/planning semantics |
| History / Undo / diagnostics | pages.rs, activity.rs, native_workflows.rs, shared transaction receipts | Root history UI owned; improve discoverability from free panels |
| Mr Wiz guidance | guidance.rs and focused child modules | Root guidance owned; no competing guidance framework |
| Synthetic QA | scripts/qa/synthetic_library.py, existing manifest/validation contract | Deterministic local synthetic scenarios only |

Initial ownership scan: 391 worktrees. GUI-v2 files owned elsewhere:
`backend.rs`, `native_workflows.rs`, `routes.rs`, `sources_providers.rs`
(RomM browser); `hackhash.rs`; `mod.rs`, `pages.rs`, `tests.rs`,
`organisation.rs`, `guidance.rs` (internal-gold worktree).
Shared `ui/components.rs`, legacy setup/controller/root and several legacy pages
also have owners. None may be overwritten. The MAME backend is out of scope.

## Validation / walkthrough plan

Use an isolated Cargo target. Test the changed states with synthetic inputs,
including 1280x800 and 700x520 headless rendering. Build the release GUI and run
it on an isolated Xvfb display with disposable HOME/XDG/config/data directories.
Never attach input automation to the user's desktop. Inspect screenshots;
automation does not replace human acceptance testing. Keep test logs and images
outside the repository. No real games, saves, BIOS, credentials or profile.

## Initial findings

- Platform game counts currently claim verification setup readiness without proof.
- Archive errors echo raw diagnostics and lack an explicit retry action.
- Save filters can masquerade as missing saves, and refresh remains clickable while busy.
- Storage and multi-disc failures can automatically restart on the next frame.
- Mod history performs filesystem work during rendering; historical changes are called enabled mods.
- Bezel refusals can be green and collapse the only explanation; empty resolution can promise apply.
- Missing-game undo can be submitted twice while work is pending.
- Setup export failures share the same raw status channel as successes.
- MAME Export report copies placeholder text rather than a report.
- The common shell/history/manual fixes must wait for ownership to clear.

The implementation ledger below distinguishes this branch from concurrent main changes.

## Improvement ledger

Each numbered item is a distinct user-facing problem, not a count of changed
strings. Related wording and controls are counted together.

### Beginner / first use
1. Empty Home offered general navigation → an explicit explanation and **Add my game folders** route.
2. Home attention totals required hunting → **Understand what needs attention** opens Problems.
3. Platform counts implied verification readiness → cards explicitly distinguish catalogued games from verification setup, with **Review checks**.

### Error recovery
4. Generic ZIP/path failures could expose raw diagnostics → accessible-file guidance and retained technical details, alongside the newly landed typed 7z/RAR errors and Retry.
5. Setup-file errors exposed raw failure text → retry instructions for file permissions/new export name, private diagnostics under Details.
6. Missing-game failures ended at an error → source-drive guidance, direct Sources/Activity routes and a fresh-plan **Check again**.
7. Equivalent-format failures had no recovery context → inspect History before another move, with direct History/Sources routes and retained technical cause.
8. Bezel preview failures exposed only the cause → explicit refresh/reselect guidance with the diagnostic retained under Details.

### Blocked / disabled states
9. An unresolved bezel could still sound applicable → no-match explanation directs users to local-folder selection and checking.
10. Bezel Apply lacked a visible reason for its disabled state → unsupported plan / missing confirmation is explained beside the control.
11. Wii U key/readiness Rust variants dominated the summary → plain key and inspection limits distinguish missing, invalid, untested and unsupported states.

### Safety / consequences
12. Mod Activity offered cancellation without a connected cancel mechanism → no fake Cancel, and pending work states the limitation.
13. MAME introductory copy implied reconstruction always made a separate output → review output and replacement consequences before applying.
14. Wii U structural completeness could look like game readiness → explicit distinction between container checks, encrypted contents and proof that a game will run.

### Navigation / click reduction
15. Mods without a game required manually hunting the library → **Choose a game**.
16. Unconfirmed mod/game identity lacked a next step → **Check this game** routes with the current game ID.
17. Mod recovery was hard to find → **Open History & Undo** directly from the workshop.
18. Competing old/new setup evidence lacked a direct destination → **Review upgrade and recovery tools** opens Advanced.
19. A per-game save view could hide unassigned records → **Review all saves, including unassigned** opens the full inventory.

### Empty states
20. Save search/filter emptiness looked like missing saves → distinct no-match guidance and **Clear search and filters**.
21. Empty installed-mod records could imply no mods existed → explains that external/unmatched changes are not necessarily recorded here.

### Success / progress
22. Storage failure automatically retried every frame → a failure latch waits for deliberate **Check again**, with Activity details.
23. Old-library storage results could replace current evidence → stale responses are rejected and old figures removed when a new check begins.
24. Multi-disc failure automatically retried → explicit retry and Activity recovery, with no silent job loop.
25. Previous multi-disc results looked current during library changes → clearly marked as old evidence until the new check finishes.
26. Save refresh accepted redundant clicks and hid activity → disabled duplicate refresh and visible read-only progress.
27. Mod history read the filesystem inside rendering → one background history worker, retained prior evidence and visible progress/retry.
28. Partially unreadable mod history looked complete → visible incomplete-history warning and technical evidence.
29. Pending missing-game cleanup/Undo could look idle → visible progress describing catalogue-only consequences.
30. Setup export success omitted location and coverage → exact destination and explicit distinction from a game/save backup.

### Emulator / BIOS
31. Missing firmware lacked context → BIOS is explained as console startup software, with existing setup action retained.
32. An undetected installed emulator could lead to another download → guidance to select the existing executable/profile in setup.
33. Stale launch checks were obscure → explains that old approval is not reused and asks for Recheck.

### Cheats
34. Recognised/installed cheats could imply in-game success → syntax, game version/region/emulator compatibility and runtime activity are explicitly distinct.

### Mods
35. Historical mod receipts were presented as enabled state → the existing Active stack tab opens a **Recorded changes** view explicitly explaining that receipts are not live activation or load order. The tab label is retained because its shared navigation test is owned elsewhere; rename both together later.
36. Conflicts foregrounded opaque transaction IDs → explains overlapping destinations; IDs and paths remain under Details.

### MAME
37. **Export report** copied a placeholder → **Copy report details** copies actual available evidence; no report means no enabled copy action.
38. Set terminology lacked a beginner explanation → optional glossary covers parent/clone, support sets, merged/split/non-merged, software lists and matching the collection's version.

### Saves / states
39. Partial save discovery could look like a complete inventory → visible warning explains unavailable locations and where to reconfigure/recheck them.

### Manuals / documents
Friendly error helper wiring remains blocked by ownership of pages.rs. No parser,
renderer or viewer capability was added. New QA fixtures cover broken PDF/CBZ,
missing documents and natural CBZ page order; these are test data, not support claims.

### History / Undo
40. Missing-game Undo allowed repeat clicks during pending work → disabled until the operation completes.
41. Missing-game Undo said only “Restored” → explains restored catalogue entries, without claiming game files were recreated.

### Accessibility / small window
42. Forget-missing confirmation used a rigid window → viewport-bounded width and vertical scrolling keep consequences/actions reachable.
43. Mod lane wrapping produced mismatched cramped cards → equal-width desktop cards and full-width narrow cards.
44. Custom mod lane cards lacked an explicit button description/focus ring → egui button metadata and the existing theme focus ring.
45. Platform illustrations crowded actions at narrow widths → smaller artwork leaves room for wrapping primary actions.

46. The decorative mod-workshop header hid useful controls at 700×520 → the existing compact workflow header keeps guidance, tabs and game/recovery actions reachable sooner.

### Consistency
47. Archive listing success could imply verified content → explicitly distinguishes a readable member list from complete, undamaged game verification.
48. Wii U issue Debug strings were primary warnings → concise recovery guidance with technical issue evidence collapsed by default.

DAT/provider root changes are deliberately skipped because the native glue is
actively owned. This pass never relabels local evidence as official or changes
provider policy. No separate status framework, persistence schema or backend was
introduced.

## Synthetic QA additions

Eight deterministic fixtures: truncated library 7z, truncated library RAR,
truncated CBZ, truncated PDF, a valid three-page CBZ stored out of natural order,
an explicitly absent manual record, a corrupt bezel PNG, and a preview-only
bezel without identity/configuration permission. Complex outcomes are labelled
ManualReviewExpected/DiagnosticOnly instead of fabricated scanner assertions.
The existing generator's safe-root, ownership, hash and no-extra-file validation
is reused. Three Python tests check repeatability, archive bytes/order and a full
tiny-lab validation. Blocking multi-disc contradictions still require trustworthy
catalogue evidence; filename-only fixture claims would be misleading.

## Remaining gaps, ranked by user impact

1. **Ownership:** shared launch-failure rendering still needs a unified reason/recovery journey (pages/native_workflows).
2. **Ownership:** first-use source setup and scan confirmation need end-to-end beginner QA (pages/mod).
3. **GUI:** MAME apply with a supplied plan still runs synchronously in its existing panel; move to the established worker before broader exposure.
4. **GUI:** bezel planning/apply still uses the existing synchronous path; large local packs need a bounded worker integration.
5. **Ownership:** manual friendly errors are implemented but not wired into the owned Game Details panel.
6. **Backend missing:** internal PDF rendering remains unavailable; preserve inspect-only/external-open distinction.
7. **Backend missing:** native CBR reading remains unavailable; do not silently invoke extraction tools.
8. **Ownership:** global History needs plain action names, recovery state and affected-game links throughout.
9. **Ownership:** emulator install/download failures need consistent retry and known-location recovery in the shared controller.
10. **Ownership:** source offline/authentication failures need direct provider configuration routes (RomM lane).
11. **GUI:** save inventory still needs a clearer jump from each unavailable root to the relevant emulator configuration.
12. **Backend missing:** multi-file save restore must wait for the active Save Vault backend lane, not a fake GUI button.
13. **Backend missing:** savestate compatibility/resume execution cannot be inferred from save-file discovery.
14. **Ownership:** source enable/disable/freshness explanations in DAT/provider setup need a coordinated pass.
15. **GUI:** durable conversion job failures need consistent output/retry/restart explanations across embedded panels.
16. **Ownership:** the shared Save hero counter overlaps its heading at 700x520; `ui/components.rs` is dirty elsewhere. Root breadcrumbs also clip at narrow widths. Coordinate fixes in the owned shared components/shell.
17. **Ownership:** global Escape/Back/modal ordering must be rechecked when concurrent shell changes land.
18. **GUI:** Wii U inspection still runs bounded local reads from presentation; cache/worker coordination belongs in a later focused pass.
19. **GUI:** mod history cannot prove emulator activation; a genuinely live enabled/load-order view needs supported evidence.
20. **GUI:** cheat refusal explanations inside the embedded legacy classifier panel still vary by device.
21. **GUI:** mod package progress has no wired cancellation; add it only if backend boundaries support truthful cancellation.
22. **Ownership:** MAME local import/full capture/directory verification is active elsewhere; GUI must await landed provider APIs.
23. **GUI:** small-window MAME report grids need further review with a large real-shaped synthetic report.
24. **Ownership:** organisation confirmation and receipt surfaces are owned by another lane.
25. **GUI:** keyboard focus semantics of remaining custom artwork/game cards need an application-wide audit.
26. **Human QA:** screen-reader traversal needs native assistive-technology testing beyond widget metadata.
27. **Human QA:** controller navigation cannot be claimed from keyboard-only automation.
28. **Human QA:** high-DPI scaling, long translated titles and contrast settings need physical-desktop review.
29. **GUI:** deterministic fixture seeding for authenticated/offline provider responses is not a network mock framework yet.
30. **Human QA:** Undo/conflict recovery journeys need supervised synthetic mutation walkthroughs across embedded tools, beyond unit coverage.

## Concurrent main reconciliation

During this pass another lane promoted `401af8c9`, changing Archive Inspector,
Bezel and Multi-disc. All 393 worktrees were rechecked; none held dirty ownership
of this pass's selected paths. The four candidate commits were rebased and the
three overlapping files reconciled deliberately: retain main's typed archive
failures/retry, typed bezel outcomes/refusal labels and all 19 multi-disc conflict
labels/tests; keep this pass's additional retry-loop/stale-result controls,
read-only guidance and layout/selection explanations. No duplicate outcome model
survives. Two improvements originally implemented here landed concurrently and
are excluded from this pass's 48-item count. Authoritative main was advanced by
the other lane, never by this task.

## Validation record

- 13 new Rust tests, plus 3 Python fixture tests.
- Focused GUI checks: 107 passed.
- Full GUI-v2 suite: 504 passed, 0 failed, 3 intentionally ignored (two report
  printers and a real-catalogue performance test; no real catalogue supplied).
- Three intermediate UI assertions failed. They passed on freshly rebuilt,
  untouched `401af8c9` main. The candidate fixed its own label/layout regressions;
  no baseline exception or owned-test edits were used to obtain the green suite.
- Tiny synthetic lab: 137 fixtures, validation has zero failures/warnings; CLI
  scan succeeded and catalogued 22 items. Eight scenarios are new in this pass.
- All source fixture hashes still matched after scan and the completed sampled GUI walkthrough.
- Existing warnings (unused helper/import/enum variant) remain; no unrelated fixes.
- Initial optimized GUI and CLI builds completed. A later compile/test attempt was
  terminated with exit 143 without a compiler diagnostic; successful reruns, not
  that interrupted attempt, are used for final validation.

Final full GUI-v2 rerun: 504 passed, 0 failed, 3 ignored (138.35 s).
The final offline/locked workspace check passed (40.26 s). Formatting, diff
checks, the three Python fixture tests and a fresh 393-worktree ownership scan
passed. No selected path had another dirty owner. The repository scope and GUI
root-boundary guards also passed.
Final optimized GUI build passed (8m 59s), using its own release target.
The final binary was reopened on the synthetic profile: the shortened platform
verification label and both actions fit at 700x520 and 1280x800. Normal window
closure exited successfully. The initial release CLI build and synthetic scan
also passed; no CLI source was changed. Walkthrough details follow below.

## Metrics

Counts describe changed call sites or user journeys, and overlap; they must not
be added together to inflate the 48-item improvement total.

| Measure | Count |
|---|---:|
| Raw diagnostics/internal identifiers removed from primary presentation | 11 |
| Recovery actions added (13 routes, clear save filters, multi-disc retry) | 15 |
| Retry/recheck journeys added or repaired | 4 |
| Important disabled states explained | 6 |
| Empty states clarified | 5 |
| Success states clarified | 3 |
| Surfaces with additional read-only/safety explanations | 10 |
| Undo/recovery visibility improvements | 3 |
| Direct navigation shortcuts | 13 |
| Specialist concepts clarified | 13 |
| Small-window issues addressed | 4 |
| Keyboard/accessibility focus improvements | 1 |
| Synthetic scenarios added | 8 |

The keyboard improvement is button semantics/focus rings for mod choices; this
pass changes no global Escape/Back dispatch. Existing keyboard/navigation/modal
regressions run in the full GUI-v2 suite. Controller and screen-reader usability
still require human QA. The old **Active stack** navigation label is retained
pending coordinated changes to the owned shared test; its content explicitly
avoids claiming enabled-state or load-order knowledge.

## Live walkthrough and limitations

The freshly optimized GUI ran on a dedicated Xvfb `:2`, never the user's desktop.
HOME, XDG config/data/cache and EmuWiz config/data all pointed under
`/tmp/emuwiz-ux-mega-qa`. It was checked with an empty profile, then with the
22-item synthetic catalogue. Screenshots were inspected at 1280x800 and 700x520;
Home was additionally checked at 1600x1000. Sampled journeys: Home, Games,
Mods/Cheats, Saves, Storage, Multi-disc, Platforms, Advanced/Archive Inspector,
and Settings. Other state variants were covered through the headless suite;
this is not a claim that every mutation or assistive-technology journey received
a human acceptance test.

Observed: Choose a game reaches Games; Escape from archive inspection returns to
Advanced; damaged 7z/RAR has a friendly error and collapsed details; narrow error
controls remain scroll-reachable; valid ZIP listing explicitly does not imply
verified content. The new workshop cards fit and stack on narrow layouts. Its
decorative header was replaced after the walkthrough showed it hiding controls.
The platform verification label was shortened after it was visibly truncated.

Remaining visible issue: the existing shared Save hero's decorative counter
overlaps its heading in the narrow window. The helper is actively owned, so it
was not edited; this is an explicit handoff gap, not a claim of a flawless
700x520 application. Some narrow screens still require substantial scrolling
below the owned shell/guidance.

QA teardown correction: the first `xdotool windowclose` destroyed the X window
and provoked a winit BadWindow panic. Subsequent teardown sent the normal
WM_DELETE_WINDOW protocol and the GUI exited successfully. No application or
backend workaround was introduced for the automation mistake.

All synthetic source hashes still matched after the walkthrough; no extracted
page files or unexpected corpus files appeared. No real game/save/profile was
used, no conversion/repair/restore was applied, and no account was connected.
Screenshots and logs stay outside Git under `/tmp/emuwiz-ux-*`.
