# GUI-v2 History & Undo - live (Sunshine) manual checklist

Do not auto-launch on `DISPLAY=:0`. Run these by hand. Every row of History is
a *record*; none of these checks should change a game file except an explicit,
confirmed Undo.

| # | Scenario | Expect |
|---|----------|--------|
| 1 | No history (fresh profile) | "No history yet", no rows, no raw ids |
| 2 | Successful rename | "Renamed N files", "Completed", UTC time, "Undo available", one primary button "Preview undo" |
| 3 | Failed rename | "Failed", "No file was changed...", "Nothing was changed, so there is nothing to undo.", handoff button (not Undo) |
| 4 | MAME reconstruction | "Rebuilt MAME set <name>", family MAME, handoff "Open MAME workflow" |
| 5 | Conversion | Not listed here (no journal source yet); the Conversion page owns its own record |
| 6 | Save restore | Not listed here (Saves & States owns its record) |
| 7 | Patch operation | Not listed here (patch flows own their receipts) |
| 8 | Undo available | "Preview undo" shows operation, affected target, what undo would do, receipt used, safety result; nothing changes |
| 9 | Output changed, so Undo unavailable | After editing a renamed file, press Preview undo: "Undo unavailable", "Undo is unavailable because the output has changed.", Blocked line; no Undo button |
| 10 | Already rolled-back operation | "Already undone", "Undone <time>", no Preview undo / Undo |
| 11 | Selected-game history | Open a game's History route: banner "Showing only history recorded for <title>", only exact-path receipts, "Show all history" |
| 12 | All History | Click "Show all history": every receipt is back |
| 13 | Filters / search | Undo available / Completed / Failed; operation-kind chips (only kinds present); search by operation, file or game. Clearing restores everything. History itself never changes |
| 14 | Advanced details | Expand: transaction id, journal folder, per-item paths, SHA-256, applied/undone unix times, provenance keys, raw errors |
| 15 | Narrow layout 1024x600 | No horizontal scroll; rows wrap; filter chips wrap; buttons reachable; "Show more" reachable |
| 16 | Large history | More than 50 rows: "Showing 50 of N." and "Show more"; scrolling stays smooth; opening the page runs no filesystem scan |
| 17 | Duplicate quarantine undo | Preview undo, then Undo, then confirm; the existing backend refuses with its own reason if anything changed |
| 18 | Painting is read-only | Leave the page open and idle: no files, journals, jobs or selected game change |

Final integration regressions: a same-count catalogue reload must refresh exact
receipt/game associations; reloaded receipt snapshots must refresh their facts
even when ID/state/item count are unchanged; completed undo must clear any old
preview and confirmation. Preview checks are preliminary (metadata, link target
or presence); the owning executor performs its full verification before mutation.

Validation on 2026-10-07 (base `ac37f78e2b59e53fdcdae5f44da8c1571accd04b`):

- `cargo test --offline --locked -p archivefs-gui --lib history_view:: --no-fail-fast -- --test-threads=1`: 21 passed.
- `cargo test --offline --locked -p archivefs-gui --lib history_page:: --no-fail-fast -- --test-threads=1`: 63 passed.
- `cargo test --offline --locked -p archivefs-gui --lib gui_v2:: --no-fail-fast -- --test-threads=1`: 761 passed, 8 ignored, no failures.
- Workspace all-targets check, workspace formatting check and diff check passed. Compiler warnings refer to untouched files.
- The initial parallel GUI-v2 run had one artwork assertion failure (760 passed, 8 ignored). That unchanged test passed both in isolation and in the complete serial rerun; its imagery helper has a ten-second deadline.
- An extra broad `history` name-filter run was cancelled after 242 passing tests because the unchanged legacy catalogue-cleanup test `tests::mounts_and_history::successful_missing_removal_records_one_activity_and_refreshes_without_resetting_view` stalled. Its isolated rerun timed out after 90 seconds. Its fixture invokes the legacy database-refresh worker; the cause remains unresolved. This diagnostic is not reported as passing.
- Sunshine/manual checks above have not been executed. Organisation integration and promotion remain separate steps.

Cargo used the dedicated History target directory
`/home/davedap/.cache/emuwiz-cargo-targets/emuwiz-gui-history-undo-current-29a82dbc27b9`.
