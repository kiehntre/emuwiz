# RetroArch Cheat Auto-Load Path

## Finding

RetroArch's cheat manager derives the automatic cheat location from the
selected core's libretro `library_name` and the loaded content basename. The
relevant source path is:

```text
<cheat_database_path>/<library_name>/<content-basename>.cht
```

RetroArch obtains the core directory from `core_get_system_info()` rather than
from a frontend display label or an emulator executable filename. Its content
name is derived from the runloop cheat-file/content basename with compression
extensions removed. Cheat loading is initiated by the normal cheat command
path, and `apply_cheats_after_load` controls whether enabled cheats are applied
after content loading.

Sources:

- [RetroArch cheat_manager.c](https://github.com/libretro/RetroArch/blob/master/command.c)
- [RetroArch command API](https://github.com/libretro/RetroArch/blob/master/command.h)

EmuWiz therefore treats the core `library_name` as an exact identity. A
selected RetroArch install now fails closed unless that core is known. It
writes the content file beneath that exact core directory and records the
resolved destination through the existing shared preview/transaction path.

## Destination and safety model

`CheatDestinationRequest` carries the exact `retroarch_core` when the GUI is
placing a RetroArch cheat. `retroarch_core_required` prevents an unknown core
from falling back to EmuWiz's older platform-folder layout. Core and content
names must already be safe single path components; they are never sanitized.
The existing destination safety checks reject traversal, symlink escapes,
unsafe parents, and destinations outside the selected profile's cheat root.

The old `<root>/<platform>/<name>.cht` layout remains available to non-
RetroArch/legacy callers so existing manual workflows are not silently
rewritten. The read-only migration preview hashes the old file, calculates the
new core/content destination, reports conflicts, and never moves or copies the
file during preview.

## Content cases

Archives, CUE/ BIN content, CHD content, and playlists use the existing
EmuWiz content-basename resolver. Auto-load is claimed only when that resolver
produces a stable basename and the selected core is exact. A multi-disc
playlist is represented by its playlist/content identity; EmuWiz does not
duplicate definitions for individual discs without evidence that RetroArch
uses those names for the selected core. Unknown/contentless bindings remain
preview-only or are refused by the existing identity/readiness checks.

## Loadability wording

The existing loadability model distinguishes a verified core/content path from
an expected path, disabled global cheat application, and the older platform
path requiring manual loading. It does not claim that a cheat executed in a
running game. RetroArch's restart/reload requirement remains visible in the
existing routing result.

## Scope

This change fixes destination planning and selected GUI installation. It does
not mutate `retroarch.cfg`, download databases, or automatically migrate old
platform-folder installs. Explicit backend migration now connects that preview
to the existing shared transaction; the GUI remains unchanged.

## Reviewed migration apply

This completes the migration-Apply gap documented with the original auto-load
path fix (`c914fff8`, 2026-09-27). The backend flow is:

1. `preview_retroarch_cheat_migration` reads the explicitly selected legacy
   platform/content file and proposed exact core/content destination. Both reads
   use the canonical bounded filesystem reader (8 MiB each); unreadable or unsafe
   entries are errors, not an assumed missing destination.
2. `RetroArchCheatMigrationPreview::build_transaction_plan` accepts the existing
   `CheatDestinationRequest`, verified `PreviewIdentity`, and explicit profile ID.
   It requires an exact core and content basename, matching canonical legacy
   platform, nonempty verified catalogue identity and profile, and unchanged
   reviewed source/destination hashes. It never infers identity from a filename,
   silently changes a name, or falls back to the catalogue title. The existing
   CHT parser must accept the complete document with no unresolved document
   warnings or unselectable entries. Unknown unsafe material remains refused.
3. The returned sealed `SharedTransactionPlan` uses the original legacy file as
   its source; there is no regenerated CHT or private alternate staging system.
   The caller reviews this plan and supplies its exact plan ID in the existing
   `SharedApplyConfirmation` before calling `execute_shared_apply`. Dry-run or
   absent approval writes nothing. The shared engine checks current profile/game
   context, source digest, destination state and safe paths, stages/verifies the
   copy, publishes atomically and writes its existing durable journal.
4. `preview_shared_rollback` / `execute_shared_rollback` undo the new copy through
   that journal. Changed user output refuses rollback; repeated rollback is
   safely unavailable. The legacy file stays intact throughout apply and undo.

The operation copies bytes exactly, including enable flags, comments and field
ordering. It does not delete the old install or claim that a cheat ran in an
emulator. An identical existing destination produces the shared no-op action;
a different destination is refused even if a caller would otherwise approve
replacement. Source/destination changes require another review. Plan context
and approval are bound by the existing shared plan digest; canonical transaction
TOCTOU and platform publication guarantees remain unchanged.

Remaining limits: a caller must supply previously verified applicability and
core/profile/content evidence. This layer proves safe placement of those bytes,
not core compatibility, effect correctness or running-emulator isolation. It
neither edits global RetroArch settings nor triggers a reload. Malformed legacy
files need separate review; no automatic repair, bulk migration, deletion,
external command, dependency, DB migration or GUI control is added.

Synthetic tests exercise read-only planning, confirmed apply and durable undo,
byte preservation, repeated undo, no-op/conflicting targets, stale sources and
destinations, identity/profile/core/content refusal, malformed documents,
symlinks and sparse files above the 8 MiB bound. Shared transaction regression
tests cover interrupted publication and failed verification without publication.
