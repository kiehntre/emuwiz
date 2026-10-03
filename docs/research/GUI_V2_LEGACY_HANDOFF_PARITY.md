# GUI-v2 legacy handoff parity matrix

> **Recovered historical research — status against current main (`b66422c2`).**
> Source: branch `feature/gui-v2-legacy-handoff-reduction` at `02b6f117`. Recovered unchanged below this block except where marked `[refreshed]`.
> - **Historical design record; current GUI v2 is authoritative.** Audit basis `1f093cae`; the handoff inventory below is a point-in-time list and was not re-verified. Compare with [`GUI_V2_LEGACY_HANDOFF_AUDIT.md`](GUI_V2_LEGACY_HANDOFF_AUDIT.md) and `crates/archivefs-gui/src/gui_v2/legacy.rs`.
> - Rationale worth keeping: reduce handoffs to the older interface only where a native page exists; keep a fail-closed fallback for tasks without a native route; preserve the selected game and keep GUI v2 open while a fallback workflow runs.


Audit basis: `1f093cae591f270a64ddc86213a3e18d9bece0cc`.

“Handoff” here means a GUI-v2 action that opens the separate established
interface through `Command::Legacy`. Native pages that merely use an existing
typed core worker are not handoffs.

## Complete inventory

| # | GUI-v2 source | Existing-interface destination | Purpose | Native replacement | Safe to remove now | Decision |
|---:|---|---|---|---|---|---|
| 1 | Advanced tools page | Advanced/library specialist view | Mounts, media-set inspection, storage and journal details | Partial | No | Keep as “Open advanced tools”; archive inspection and normal organisation are native, but these specialist workflows are not. |
| 2 | Unsupported task fallback | Section-specific existing workflow | Safety-preserving escape for a task without a native v2 implementation | No/partial by task | No | Keep fail-closed fallback; no task is silently discarded or guessed into a native route. |
| 3 | Unsupported task fallback → advanced details | Advanced/library specialist view | Secondary escape when the task needs technical tools | Partial | No | Keep as “Open advanced tools”; retained only for unsupported administration/diagnostics. |
| 4 | Settings page advanced details | Existing application settings | Full settings/configuration surface | Partial | No | Keep as “Open advanced settings”; GUI-v2 settings currently cover navigation/preferences, not the complete application configuration. |

## Native parity with no remaining handoff

| Workflow | Native route/page | Result |
|---|---|---|
| RomM browsing | `Section::Romm` | Native platforms, games, filters, detail, provenance, offline cache. |
| RomM refresh/import | `Section::Romm` | Native cache refresh and bounded non-publishing import preview. |
| Artwork/metadata browsing | `Section::Artwork` and game artwork task | Native local-first artwork, provider evidence and selected-game context. |
| Bezel preview/apply | Artwork workflows | Native preview/apply controls with existing safe-apply policy. |
| History/undo | `Section::History` | Native transaction/history presentation and supported undo routes. |
| Selected-ROM evidence | Game detail and native evidence panels | Native evidence/provenance display without opening another window. |
| Source/provider browsing | `Section::Sources` | Native source/provider setup and read-only status surfaces. |

## Retained boundaries

RomM mapping administration, conflict resolution, stale diagnostics, advanced
provider diagnostics, full application settings, mounts, media/storage
inspection, and unsupported destructive or administrative operations remain
available through Advanced tools. They are intentionally not represented as
native parity claims.

The native route table remains exhaustive for the normal sidebar. The fallback
exists only for task routes that do not have an explicit native implementation;
it preserves the selected game and keeps GUI-v2 open while the established
workflow runs.
