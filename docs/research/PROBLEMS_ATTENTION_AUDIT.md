> **STATUS NOTE (recovered 2026-10-03; recovery baseline main `d112b7f3`).**
> Recovered verbatim from branch `research/problems-attention-audit` (`e379d745`, 2026-09-30), audited against main `973228c9`.
> **All counts below (including 132,064 rows, 89,763 "need attention" and the 69,034 present-on-disk-but-flagged games) are a HISTORICAL MEASUREMENT of one real catalogue on one date. They are not the current live catalogue counts, and no new measurement was run for this recovery.**
>
> ## CURRENT STATUS: OPEN (code evidence at main `d112b7f3`)
> - The rule is unchanged: `Game::from_archive` in `crates/archivefs-gui/src/gui_v2/library.rs:118` sets `attention = archive.last_verified_missing_at.is_some() || last_known_health in {"missing","corrupt","damaged","error"}`. That feeds `Library.attention`, the Home/Browse counts and the "Needs attention" filter.
> - `ProblemSummary::from_library` in `crates/archivefs-gui/src/gui_v2/problems.rs:168` still uses a different, wider rule (adds `!absolute_path.is_file()`), so the Home count and Problems & Repair can still disagree.
> - Since the audit base, main added a read-only Missing games review (`gui_v2/missing_review.rs`, `database/forget_missing.rs`, commit `4124825f`) with a `MissingClassification` that includes `NotMissing`/`PossiblyMoved`. It does not change `Game::attention`.
> - A core repair for stale flags exists: `Database::apply_presence_reconciliation` (`crates/archivefs-core/src/database/catalogue_health.rs:403`) clears `last_verified_missing_at` for rows verified present. At this commit it has no non-test caller in the GUI or CLI (only `tests/catalogue_health_*.rs`), so it is not product-reachable.
> - Not fixed here. Whether the real catalogue still shows the stale population would need the same read-only measurement re-run.

---

*Historical document follows unchanged.*

# "Needs attention" and Problems & Repair audit

Audit only. No production code, GUI code or main was changed; nothing was pushed.

- **Main used:** `973228c9fb3fbfd1d3dfb4de1121159ebe2aa9ea` (local `main` == `origin/main`, tracked tree clean).
- **Data:** the real catalogue, `~/.local/share/archivefs/library.sqlite3`, opened read-only (132,064 rows).
- **Method:** source trace; read-only SQL and filesystem checks; a throwaway probe program outside the repo
  (`/tmp/attn-probe`, uses the app's own `canonical_identity_from_game_report`); the real release build of
  current main, driven on X11 with timings.

## 1. Headline

**The 89,763 is not "games that need attention". It is "games whose database row carries a
`last_verified_missing_at` timestamp".** Nothing else contributes. 69,034 of those files (77%) are on disk right now.
The stamp is an artefact of one scan on 2026-09-22.

Problems & Repair does not agree with Home. On the same catalogue it reports **132,064** "missing or has a saved
health problem" findings (every game), and under one timing it reports **0**, a false all-clear.

## 2. Where the number comes from

| Surface | Source | Condition |
|---|---|---|
| Home ("89763 need attention"), Browse & Play card and panel, Games filter, Check Games, Setup readiness list | `Game.attention` (`gui_v2/library.rs:118`), counted in `Library.attention` | `last_verified_missing_at IS NOT NULL` **or** `last_known_health` is exactly `missing`/`corrupt`/`damaged`/`error` |
| Problems & Repair | `ProblemSummary::from_library` (`gui_v2/problems.rs`) | file problem if `last_verified_missing_at` set **or** `!absolute_path.is_file()` **or** the same four health strings; otherwise identity problem if not verified |
| Game Details | launch readiness (`launch_readiness_summary.rs`, "Needs attention" = `Blocked`) | separate: launch planner, not `Game.attention` |
| Check Games | `backend.rs:355-375` | presence check that also accepts arcade set directories; then `attention`, then identified/unknown |

So **there is no single canonical state**: Home/Browse use one rule, Problems a wider one (adds `is_file()`), Check
Games a third (directory-aware), Game Details a fourth (readiness planner).

Notes on the conditions:
- `last_known_health` is `Pending` on **all 132,064 rows** (capital P). The four strings never match, so the health arm
  contributes 0. Health is never updated from `Pending`.
- The only real trigger is `last_verified_missing_at`.

## 3. Real counts, by cause

Presence is directory-aware (an arcade set is a directory). Categories are the four combinations the code itself
distinguishes; nothing here is invented.

| | Games | Meaning |
|---|---:|---|
| A. present, not flagged | 28,899 | all Arcade sets, under source folder 22 |
| B. present on disk **but flagged missing** | 69,034 | stale flag |
| C. absent, flagged missing | 20,729 | file is gone from its recorded path |
| D. absent, not flagged | 13,402 | all under source folder 5 (`/mnt/games/roms`), removed from config 2026-08-11; directory no longer exists |
| **Total** | **132,064** | |

Home's 89,763 = **B + C** (69,034 + 20,729).

Identity (canonical evidence, same function the GUI uses):
- Verified: **15,665**. Never checked (no identity report at all): **116,399**.
- Conflicting, or a report that is unresolved: **0**. There is no "identity conflict" population.

| | verified | never checked |
|---|---:|---:|
| A | 15,653 | 13,246 |
| B | 5 | 69,029 |
| C | 3 | 20,726 |
| D | 4 | 13,398 |

Absent rows (C + D = 34,131): **30,964** have a file with the same name present elsewhere in the catalogue (a
renamed or moved folder such as `gameboy`→`gb`, `NEC PC-8801`→`pc-8800-series`); only **3,167** have no same-named
file anywhere.

Other categories the prompt listed (missing emulator, BIOS, unsupported media, conversion needed, DAT mismatch, metadata,
artwork) **do not feed "Needs attention" at all** in the current code. They live in per-game launch readiness and Setup.

### Why B exists
Scan #197 (2026-09-22 16:06) reported `archives_seen = 28,899` against 69,675 in scan #196 and stamped **69,675 rows
missing in one pass**. Every scan since (#198-#206, through 2026-09-27) sees the same 28,899, all Arcade, although the
other folders under `/mnt/usbdrive/games` (zxs, snes, gb, gba, ...) exist and are readable (checked with stat). Earlier
scans (2026-08-26, 09-02, 09-03, 09-06) stamped a further ~20,000 rows when folders were renamed. **Why scans stopped
seeing the other systems is not determined here**: reproducing it needs a scan, which writes to the database. It is
a scanner/scope question, separate from the GUI.

## 4. Severity and actionability

| Group | Count | Class | Reason |
|---|---:|---|---|
| C: file gone, and the same-named file exists elsewhere | ~30,964 combined with D | **B** action needed | duplicate/stale row for a moved folder; needs cleanup, the game itself is fine |
| C+D: file gone, nowhere else | 3,167 | **A** broken now | cannot launch |
| D: removed source folder | 13,402 | **B** action needed / **D** informational | orphan rows of a source the user removed; still listed as browsable |
| B: present but flagged | 69,034 | **F** unknown / stale | data says missing; disk says present; a rescan resolves it |
| A/B present, never checked | 82,275 | **C** review / **D** informational | not verified yet; nothing is wrong |
| A verified | 15,653 | none | fine |
| Missing artwork/manual/metadata | n/a | **E** cosmetic | not in this bucket |

A coherent partition of all 132,064 (mutually exclusive):

| Group | Games |
|---|---:|
| File present, identity verified | 15,658 |
| File present, identity not yet checked | 82,275 (of which 69,029 also carry the stale "missing" flag) |
| File not found at its recorded path | 34,131 (of which 30,964 have the same file elsewhere) |

## 5. Double counting

- **Home:** one condition, so each game is counted once (89,763 unique = 89,763 reasons). Nothing is double counted, but
  it is one weak reason.
- **Problems & Repair:** `if file_problem { … } else if !identified { … }`, so a game yields **one** finding. Games with
  2+ findings: 0 by construction. Consequence: because every game currently hits the file rule, **the "Metadata and
  identity" category is empty**, and the 116,399 unverified games are hidden behind the file finding. If the file rule
  were fixed, up to ~116,000 identity warnings would appear at once.
- Overlap of triggers inside the file rule: flagged and file absent 20,729; flagged and present 69,034; not flagged and
  not a regular file 42,301 (28,899 arcade set directories + 13,402 orphan rows).
- **Arcade sets:** `is_file()` is false for a directory, so all **28,899 healthy, unflagged Arcade sets** are reported as
  "missing or has a saved health problem". Home does not count them; Check Games handles directories correctly. This
  is a mismatch, not a data problem.

## 6. Problems & Repair loading

Measured on the current-main release build, real catalogue, 1400x1000.

Startup queue (a **single worker thread, strictly FIFO**, `backend.rs:237`):
1. `EnvironmentCheck` ("Checking EmuWiz setup"): **14-25 s**
2. `Restore`, `LoadRepairHistory`
3. `Load` (library): ~1 s, but it only *starts* after step 1, so Home shows "Loading…" for the first 15-25 s.
4. anything the user opens meanwhile, queued behind them.

`BuildProblemSummary` itself: **~1 s** (Activity shows "Elapsed: 1 seconds"; cold-cache stat of 132,064 paths measured
at 4.3 s in the probe, 0.5 s warm). When the worker is free the page fills in under 2 s (screenshots
`/tmp/gf/qp_t2.png`).

### Why the spinner is long
It is **queueing, not the check**. Open Problems in the first ~25 s and the job waits behind the environment check
(observed: "3 active jobs · 1 running · 2 waiting", spinner, and a disabled "Checking saved evidence" button; nothing
partial is shown).

### A more serious defect: the false all-clear
`start_problem_summary` captures `self.library` **when it queues the job**, so the job can hold an *empty*
library. Observed twice: with the library still loading, or after the app restored Problems as the last page
(`gui-v2.json` stores the route):
1. the job is queued with the empty library, behind the 14-25 s environment check;
2. the library arrives, and clears the summary, but the job id is still set, so no new job starts;
3. the stale job finishes and stores an empty summary.

Result after 60 s: **"Nothing currently needs your attention. 0 actionable · 0 warnings · 0 total"** on a catalogue where
Home says 89,763. It stays until the library is reloaded or a duplicate scan finishes. Reproduced by launching with
Problems as the saved page; not reproduced when opening it after the library had loaded.

### Repeated work
The summary is discarded on every library load and every duplicate scan, and rebuilt on the next visit. It is not
cached across visits otherwise. The rebuild is cheap (~1 s), so this is secondary. Not measured: the cost of *drawing*
the resulting 132,064 findings (CPU was ~13% during the healthy run).

## 7. Is "Needs attention" a meaningful warning?

**No, not today.** It conflates:
- a stale scan record (69,034, files fine),
- genuinely gone files (34,131 at their recorded path, mostly moved folders),
- and, on Problems & Repair, healthy arcade sets (28,899) and orphan rows.

The 82,275 unverified games are a normal starting state, not a fault, but they are hidden behind the file finding.
The word "attention" is attached to the largest, least reliable bucket, and different pages give 89,763, 132,064 or 0.

### Suggested user-facing grouping (real numbers, not implemented)
| Group | Now |
|---|---:|
| Ready to play (file present, verified) | 15,658 |
| Not verified yet | 82,275 |
| Can't find the file | 34,131 (30,964 look like moved folders) |
| Out of date: needs a rescan (file present, marked missing) | 69,034, a subset of "not verified yet" |
| Actually broken (gone, and no copy anywhere) | 3,167 |

The headline should be the last row, or "can't find the file", not their sum.

## 8. Recommendation for the next GUI pass (not implemented)

1. **One shared definition** of file presence (directory-aware) and of the attention bucket, used by Home, Browse,
   Problems and Check Games.
2. **Split the bucket** as above; make "Not verified yet" a neutral state, not a warning.
3. **Treat "recorded missing but present on disk" as stale**, a "Recheck" action, not a problem; group missing rows that
   have a same-named file elsewhere as "moved".
4. **Problems & Repair:** never cache a summary built from an older library; show the known counts instantly and fill in
   detail; say "waiting for the setup check" rather than "checking evidence"; draw a summary with drill-down rather than
   132,000 rows.
5. Separate (not GUI): the environment check delays library load by 14-25 s at startup because both share one FIFO
   worker, and the scanner has seen only the Arcade folder since 2026-09-22.

## Reproducing

```
sqlite3 -readonly ~/.local/share/archivefs/library.sqlite3 \
  "select count(*) from archives where last_verified_missing_at is not null"      # 89763
# probe: /tmp/attn-probe (path dependency on archivefs-core; read-only; not committed)
```

Side effects: the GUI was launched several times for timing (it saves its own UI preferences, the last-visited page);
one run used an isolated copy of that file. No database, game, or cache file was written.
