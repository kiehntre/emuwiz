# Blue DAT selector performance and verification journey

Baseline: fresh `origin/main` at `c651590645f5d2408ab13c40cea2266862ced8ad` (2026-10-17).

## Diagnosis and measurements

The shared picker laid out every catalogue and searched the entire inventory again for each row's eligibility: quadratic work on the GUI thread. The local platform setup laid out every local row and repeatedly checked file existence. The Blue DAT host additionally constructed the full DAT page view a second time merely to count sources. Inventory loading itself already uses a worker; no per-frame SQL bottleneck was identified in these selector paths.

Bounded headless egui measurements used the same unoptimized low-debug build, 1100×720 viewport, 456 and 5,000 synthetic catalogues, one warm-up and seven measured frames per sample. Local fixtures used distinct tiny disposable files, without imports or bulk audits. These are warmed CPU layout/render timings, not GPU frame times, remote-stream latency or release-build promises.

| Surface | Catalogues | Before median ms | After median ms | After maximum ms |
| --- | ---: | ---: | ---: | ---: |
| Shared picker | 456 | 72.364 | 6.397 | 6.512 |
| Shared picker | 5,000 | 1096.707 | 7.291 | 8.044 |
| Local platform setup list | 456 | 26.550 | 4.958 | 5.925 |
| Local platform setup list | 5,000 | 304.901 | 9.870 | 11.628 |

The local-list sample includes all groups rather than measuring an empty compatible filter. Initial cache creation and inventory loading are outside the warmed render medians. Ignored `selector_profile` tests retain the bounded measurement harness. Search tests exercise 5,000 rows and platform/provider/name terms together, A–Z, groups, paging and stable typed selection.

The full `DatSourcesPageState::view` projection is measured separately: 6.254/49.169 ms before and 6.350/50.243 ms after at 456/5,000 rows. The host no longer calls it twice. **One projection remains**, including filesystem/source-policy work in `crates/archivefs-gui/src/dat_sources_page.rs`. That file is actively owned by the managed-DAT agent in `/home/davedap/emuwiz-managed-dat-ps1-20261017`. Moving its work to an invalidated worker snapshot needs coordinated integration there; this change deliberately leaves that ownership boundary intact. Consequently the complete 5,000-source page can still exceed a 16 ms frame budget. No production GUI profiling or application replacement was performed.

## Selector behavior

Both lists reuse a presentation-only cached index and draw at most 40 rows per page. They search catalogue name, recorded platform and existing provider provenance; an alphabetical filter and explicit readiness groups bound navigation. Compatible ready rows sort first. Disabled, stale/needs-checking, unknown and other-platform/unsupported rows remain separately discoverable. A missing platform assignment is not compatibility evidence. Unrecognised explicit assignments are incompatible, not unassigned.

Local unknown-platform sources can be explicitly selected for the existing native compatibility preview. This only changes transient selection, never source platform identity. The old “Use for Amiga” assignment action is absent from this setup list. Fresh existence checks remain on the selected preview candidate; cached display metadata cannot authorize verification. Typed references survive filtering, and filtering never selects a source. No trust, content resolution, verification persistence, audit engine or recovery logic changes are included.

## Verification journey and safeguards

Game Details → Review Identity explains that installed catalogue counts do not represent checked games. DAT Setup retains the reviewed canonical platform when routing back to Check Games. Not checked/Not compared means no saved comparison; Audit required means existing native Preview and explicit confirmation are still needed. Ready catalogue metadata is distinct from compatible data, explicit selection, content evidence and a saved Verified match.

Existing persisted No match remains a completed comparison without a match. No usable evidence can reflect unavailable evidence or unsupported format; the page no longer asserts every such game is unreadable. No new persisted unsupported state is invented: introducing a distinct durable result would overlap the managed agent's identity/verification ownership. Existing Preview refusals remain authoritative. Existing native tests cover stale/unavailable/disabled sources, incompatible population, changed files, approval and verification boundaries. No catalogue installation, filtering or UI navigation manufactures Verified status.

## Isolation and validation

Implementation stays in selector presentation, local simple-list UI and small Blue journey/navigation hooks. Managed-DAT identity, verification, source resolution, database and persistence files are untouched. The owner's running Blue process, production recovery state and pinned Cargo target are untouched. Tests run with fresh HOME/XDG/application roots and disposable fixtures in this worktree's own Cargo target.
