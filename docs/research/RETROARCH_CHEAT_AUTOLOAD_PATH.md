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
file.

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
platform-folder installs. Any future migration Apply must use an explicit
confirmation and the shared transaction journal.
