# Blue standalone Flycast cheats

Scope: existing native format, local-file inspection and entry review only.
Apply is intentionally disabled under the mission's safe fallback. This is not
completed installation support or an in-game effectiveness claim.

## Journey

Choose a Dreamcast game in Blue Cheats & Mods, explicitly select standalone
Flycast if routing is ambiguous, then choose a local native `.cht` file. The
panel uses the existing bounded parser. Source enable state is displayed;
selection and intended enable state start off. Enter the product code declared
by the source, select supported entries, and preview them. A complete verified
Dreamcast identity must refer to this exact game path. Conflicting, incomplete,
missing or merely candidate product codes refuse review. Product-code equality
is not source authentication and does not establish disc/revision applicability.

The panel projects the same `LaunchPlan` and `selected_candidate` that Blue
Launch displays, including its sole-eligible/remembered choice. It refuses to
name a Flycast profile when Launch is ambiguous, blocked, another emulator,
or different/unresolved content. This is a readiness projection, not proof
that Flycast can launch or consume a cheat file. No Launch preference is saved.
Flycast has no shared AUTO/PREFERRED/FORCED policy implementation; that policy
is available for DuckStation and must not be attributed to Flycast.

Source bytes are reread before entry review; missing or changed sources refuse.
Changing identity, Launch selection, declared product code, selected entries or
intended state invalidates review. The exact write list is empty. Apply is
visibly disabled and there is no install or Undo action in this module. Local
inspection creates no transaction or History record. Existing historical
transactions remain accessible in History & Logs.

## Format and profiles

Native sectionless `cheats` / `cheatN_*` fields are inspected. Direct set
operations with native `memory_search_size` 3/4/5 represent 8/16/32 bits
(1/2/4 bytes). The previous backend incorrectly used 0/1/2. Other operations
remain opaque. Parser diagnostics block supported-entry review. No external
CodeBreaker/GameShark decoder, RetroArch conversion or download is added.

Existing profile discovery recognizes native XDG, Flatpak user, portable and
explicit layouts. The panel shows only the profile selected by Launch when
provable. **No Flycast version or profile layout is approved for installation**
by this integration. Native source semantics do not prove runtime compatibility.

## Exact installation blockers

Upstream `core/cheats.cpp` loads `config::loadStr("cheats", gameId)`; newer
source also searches `Dreamcast.CheatPath` by content filename. It does not
establish the legacy backend's assumed `data/cheats/<product-code>.cht`
association. Loading a file also saves its mapping into `emu.cfg`. See
[Flycast source](https://github.com/flyinghead/flycast/blob/master/core/cheats.cpp)
and the earlier [runtime feasibility investigation](FLYCAST_CHEAT_RUNTIME_FEASIBILITY.md).

Independent baseline review additionally found that the legacy planner can
ignore duplicate/truncation warnings, allow opaque operations, trust imported
field maps and reuse stored context as current context at Apply. This change
does not expose that writer to Blue. It corrects only the confirmed native
width encoding. Enabling installation needs proven mapping/path ownership,
complete supported-subset validation, independent game/disc/source/profile
freshness checks, and independently reviewed preservation guarantees. This is
larger than safe GUI wiring and is not guessed here.

Shared transaction source/destination checks, durable History and guarded Undo
already exist. Their tests can verify file operations on disposable profiles;
such tests do not establish Flycast consumption. No R2 or journal code changes.

## Validation contract

Use synthetic identity reports, local native records, disposable source/profile
files and hand-built Launch plans. Test sole/remembered/ambiguous choices,
blocked/other/unresolved selections, identity conflicts, native widths, invalid
and opaque formats, duplicate diagnostics, missing/changed sources, selection
invalidation, default-off choices, small-window rendering and byte preservation.
Run existing Flycast backend Apply/readback/History/Undo tests only in temporary
roots. Never run an emulator against production profiles for these checks.

## Executed validation

The new native-width regression failed against the original implementation,
then passed after correction. Focused suites passed: core Flycast 58, routing
29, resolver 31, shared transactions 32; GUI Cheats & Mods 148, History 21,
resolver 37, Launch selection 40 (396 passing checks in these focused runs).
One existing real-machine resolver probe was intentionally ignored. The nine
new panel tests also passed separately and are included in the GUI suite.

Cargo commands ran through `scripts/cargo-iso --low-debug`. A test runner
supplied private HOME, XDG configuration/data/state/cache and temporary roots
to each test process, with recorded runner invocations. No production emulator
profile or game media was used for writes. In-game effectiveness was not tested.
Baseline regression, focused logs, isolation runner and review evidence are
retained under `emuwiz-audit-ledger/2026-10-17-flycast-blue-cheats` outside the
repository. Independent final review is required before promotion.
