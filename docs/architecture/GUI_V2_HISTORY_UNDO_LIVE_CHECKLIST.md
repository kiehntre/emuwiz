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
