# GUI v2 whole-product UX pass

Base: `1571f7e56500d75b383c2aa5c34f632d7f487656`.
Branch: `feature/gui-v2-whole-product-ux-mega-pass`.

## Product map and ownership

Study covers the 38 top-level GUI-v2 Rust modules, their entry points, primary
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

## Findings to resolve

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

The implementation ledger and final validation results follow in later checkpoints.

## Improvement ledger

Each numbered item is a distinct user-facing problem, not a count of changed
strings. Related wording and controls are counted together.

### Beginner / first use
1. Empty Home offered general navigation → an explicit explanation and **Add my game folders** route.
2. Home attention totals required hunting → **Understand what needs attention** opens Problems.
3. Platform counts implied verification readiness → cards explicitly distinguish catalogued games from verification setup, with **Review checks**.

### Error recovery
4. Archive failures repeated raw diagnostics → accessible-file guidance, unchanged-file assurance, collapsed details and explicit **Retry inspection**.
5. Setup-file errors exposed raw failure text → retry instructions for file permissions/new export name, private diagnostics under Details.
6. Missing-game failures ended at an error → source-drive guidance, direct Sources/Activity routes and a fresh-plan **Check again**.
7. Equivalent-format failures had no recovery context → inspect History before another move, with direct History/Sources routes and retained technical cause.
8. Bezel preview failures exposed only the cause → explicit refresh/reselect guidance with the diagnostic retained under Details.

### Blocked / disabled states
9. An unresolved bezel could still sound applicable → no-match explanation directs users to local-folder selection and checking.
10. Bezel plan refusal could be hidden → a visible refusal explains that no apply is available, with evidence below.
11. Bezel Apply lacked a visible reason for its disabled state → unsupported plan / missing confirmation is explained beside the control.
12. Wii U key/readiness Rust variants dominated the summary → plain key and inspection limits distinguish missing, invalid, untested and unsupported states.

### Safety / consequences
13. Bezel apply results could all appear green → only a successful journal reports success; failed/partial results direct users to recovery, and old success cannot colour a later refusal.
14. Mod Activity offered cancellation without a connected cancel mechanism → no fake Cancel, and pending work states the limitation.
15. MAME introductory copy implied reconstruction always made a separate output → review output and replacement consequences before applying.
16. Wii U structural completeness could look like game readiness → explicit distinction between container checks, encrypted contents and proof that a game will run.

### Navigation / click reduction
17. Mods without a game required manually hunting the library → **Choose a game**.
18. Unconfirmed mod/game identity lacked a next step → **Check this game** routes with the current game ID.
19. Mod recovery was hard to find → **Open History & Undo** directly from the workshop.
20. Competing old/new setup evidence lacked a direct destination → **Review upgrade and recovery tools** opens Advanced.
21. A per-game save view could hide unassigned records → **Review all saves, including unassigned** opens the full inventory.

### Empty states
22. Save search/filter emptiness looked like missing saves → distinct no-match guidance and **Clear search and filters**.
23. Empty installed-mod records could imply no mods existed → explains that external/unmatched changes are not necessarily recorded here.

### Success / progress
24. Storage failure automatically retried every frame → a failure latch waits for deliberate **Check again**, with Activity details.
25. Old-library storage results could replace current evidence → stale responses are rejected and old figures removed when a new check begins.
26. Multi-disc failure automatically retried → explicit retry and Activity recovery, with no silent job loop.
27. Previous multi-disc results looked current during library changes → clearly marked as old evidence until the new check finishes.
28. Save refresh accepted redundant clicks and hid activity → disabled duplicate refresh and visible read-only progress.
29. Mod history read the filesystem inside rendering → one background history worker, retained prior evidence and visible progress/retry.
30. Partially unreadable mod history looked complete → visible incomplete-history warning and technical evidence.
31. Pending missing-game work could look idle → visible progress describing catalogue-only consequences.
32. Setup export success omitted location and coverage → exact destination and explicit distinction from a game/save backup.

### Emulator / BIOS
33. Missing firmware lacked context → BIOS is explained as console startup software, with existing setup action retained.
34. An undetected installed emulator could lead to another download → guidance to select the existing executable/profile in setup.
35. Stale launch checks were obscure → explains that old approval is not reused and asks for Recheck.

### Cheats
36. Recognised/installed cheats could imply in-game success → syntax, game version/region/emulator compatibility and runtime activity are explicitly distinct.

### Mods
37. Historical mod receipts were labelled a current enabled stack → **Recorded changes** explains installation/undo records are not live activation or load order.
38. Conflicts foregrounded opaque transaction IDs → explains overlapping destinations; IDs and paths remain under Details.

### MAME
39. **Export report** copied a placeholder → **Copy report details** copies actual available evidence; no report means no enabled copy action.
40. Set terminology lacked a beginner explanation → optional glossary covers parent/clone, support sets, merged/split/non-merged, software lists and matching the collection's version.

### Saves / states
41. Partial save discovery could look like a complete inventory → visible warning explains unavailable locations and where to reconfigure/recheck them.

### Manuals / documents
Friendly error helper wiring remains blocked by ownership of pages.rs. No parser,
renderer or viewer capability was added. New QA fixtures cover broken PDF/CBZ,
missing documents and natural CBZ page order; these are test data, not support claims.

### History / Undo
42. Missing-game Undo allowed repeat clicks during pending work → disabled until the operation completes.
43. Missing-game Undo said only “Restored” → explains restored catalogue entries, without claiming game files were recreated.

### Accessibility / small window
44. Forget-missing confirmation used a rigid window → viewport-bounded width and vertical scrolling keep consequences/actions reachable.
45. Mod lane wrapping produced mismatched cramped cards → equal-width desktop cards and full-width narrow cards.
46. Custom mod lane cards lacked an explicit button description/focus ring → egui button metadata and the existing theme focus ring.
47. Platform illustrations crowded actions at narrow widths → smaller artwork leaves room for wrapping primary actions.

### Consistency
48. Archive listing success could imply verified content → explicitly distinguishes a readable member list from complete, undamaged game verification.
49. Wii U issue Debug strings were primary warnings → concise recovery guidance with technical issue evidence collapsed by default.

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
16. **Ownership:** root small-window navigation and family submenus require layout changes in the owned shell.
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
