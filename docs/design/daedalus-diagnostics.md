# Daedalus: launch diagnostics and reversible repair

Status: slice 1 (evidence model, rule engine, RetroArch adapter, read-only CLI)
is implemented. Repair, isolated experiments, the Blue view and the optional
AI layer are designed here and not yet built.

## Why

EmuWiz decides before Play whether a game can be launched. It had nothing to
say once a launch went wrong:

- the launcher keeps the exit status and the first 64 KiB of stderr, shows
  them as text, and guesses a cause from the rendered error string
  (`docs/GUI_V2_LAUNCH_DIAGNOSIS.md` records that gap);
- no emulator log is read, no signal is decoded, no duration is kept;
- nothing is known about graphics APIs;
- no code can change an emulator's own settings file.

Daedalus adds the missing layer without replacing the readiness planner,
Doctor or the launch executors.

## Contract

1. Every conclusion names the observations that support it and what limits
   confidence in it.
2. A failure no rule explains is reported as unexplained. No rule infers a
   cause from an unspecific error message.
3. Emulator output is untrusted text. It is matched against fixed signatures
   and otherwise only shown, sanitised and bounded. On its own it supports
   nothing above "possible".
4. The same evidence and rule set always give the same report; the report
   records the rule-set digest.
5. Diagnosis is read-only and grants nothing. A repair is a separate,
   previewed, consented, backed-up, undoable operation.
6. A process that started, or exited with status 0, has not been shown to
   load or run a game.

## What exists and is reused

| Need | Existing code |
|---|---|
| Launch blockers | `launch/readiness.rs`, `ready_to_play.rs` |
| Exit status and stderr | `launch/process_spawn.rs` (`ProcessExitReport`) |
| File identity | `launch/process_spawn.rs` (`CapturedFileIdentity`) |
| RetroArch settings grammar | `emulator_environment/retroarch.rs` |
| Firmware state | `launch::FirmwareReadiness` |
| Journalled, undoable file changes | `patch_manager/shared_transaction.rs` |
| Disposable RetroArch profile | `launch/cheat_runtime_retroarch.rs` |

Not reused, deliberately: the GUI's substring classification of blocker names
and of rendered error text.

## Evidence (`archivefs_core::daedalus::evidence`)

An `Evidence` item is one observation:

- `area`: one of thirteen (`game_identity`, `media_integrity`,
  `emulator_executable`, `firmware`, `graphics`, `emulator_configuration`,
  `launch_environment`, `launch_readiness`, `process_exit`, `log_events`,
  `filesystem`, `modifications`, `known_limitations`);
- `state`: `observed`, `unavailable`, `not_collected` or `stale`;
- `trust`: `measured` (EmuWiz read it from the operating system or a file),
  `reported` (a structured channel content cannot write to), `supplied`
  (stated by whoever asked for the diagnosis; a premise, never firm),
  `untrusted_text` (logs, stderr) or `asserted` (a person or an AI said so);
- `fingerprint`: identity and, for settings files, SHA-256 of the exact bytes
  read, so a later change is detected (`EvidenceSet::refresh_freshness`);
- `fact`: a typed value.

`EvidenceSet::coverage()` lists every area, including those with nothing
observed. A set holds at most 2,000 items and says when it was truncated.

## Rules (`archivefs_core::daedalus::rules`)

A rule is a pure function with an identifier, a version and a one-line
statement of what it concludes from what. `emuwiz-cli diagnose-launch --rules`
prints them. The engine applies limits no rule can opt out of:

- support must be observed and current; stale, unavailable and asserted items
  are not support, and asserted items are not shown to rules, do not count as
  coverage and cannot contradict anything;
- a finding (cause or symptom) resting on untrusted text needs a firm
  observation other than the process exit to rise above `possible`;
- a cause named from only an exit status and an unspecific message is
  rejected and listed under `rejected`;
- contradicting evidence, or a contradicting finding, holds a finding at
  `possible` and makes the verdict `conflicting`;
- evidence holding more than one process exit is not interpreted.

The built-in rules go further than the engine requires: a cause drawn from
what the emulator printed stays at `possible` even when a firm fact
accompanies it, unless that fact is a check of the very thing claimed. A
readable game file, a readable core, or a setting that says "vulkan" is true
of every healthy setup and corroborates nothing. So the Vulkan diagnosis is
`possible` from the log, `probable` when a probe finds Vulkan unusable on the
host, and `established` only when a probe finds it unusable in the launch
environment itself.

A report separates **symptoms** (what happened) from **causes** (why).
Symptoms are how the process ended and what a program said outright: an
explicit error line becomes a *reported* symptom ("RetroArch reported that it
could not set up Vulkan"), established as a report and never, by itself, a
cause. A reported symptom carries `confidence: established` and
`reported: true`: the confidence is in the fact that it was printed, never in
a cause, so a reader that filters on confidence must read `reported` and
`explains_cause` with it.

What happened to the launch is stated apart from why. `launch` is `failed`
(the process could not be started, crashed, was killed or ended with a
failure status), `failure_reported` (the process was not seen to fail, and a
program stated an error that stops a game from starting: a report, not an
observation), `no_failure_observed` (not a statement that a game ran) or
`not_observed`. A launch that did not fail can still have a serious problem,
for example a save that could not be written.

Verdicts are about why: `explained` (an established cause, no contradiction),
`partly_explained`, `conflicting`, `unknown` (a problem was observed or
reported and nothing found explains it), `no_problem_observed` (which is
explicitly not a statement that the game works). Once something failed, or
the evidence mixes several runs so that a failure cannot be ruled out, only a
cause that can prevent a launch explains it; a problem that merely degrades
the game (no sound, saves not written) is still reported but does not turn an
unexplained crash into an explained one.

An exit status of 128 plus a stop signal (143 and the like) is read as a
relayed request to stop only when a launcher is recorded between EmuWiz and
the emulator; it is then a note, not a failure. With no launcher the program
chose that status itself: it is a failure, and the other reading is offered
as no more than possible. A stop signal delivered to the watched process is
not a failure and is stated in a note.

Settings are read per layer (main file, core, folder and game overrides). A
layer whose file does not exist is recorded as absent, which is an
observation: it is listed in the report (a mistyped path looks the same) and
goes out of date if a file appears there. A layer that exists and could not
be read, or changed while it was being examined, holds every setting-based
finding at `possible`, because it may hold the value the emulator really
uses. Each override layer nobody examined is named as a limit of the finding.

Whoever supplies evidence is stated. What EmuWiz observed itself is
`measured`. An exit status or environment typed on the command line is
`supplied`: a rule may reason from it, but nothing that rests on supplied
facts alone, cause or symptom, is more than `possible`, its source says it
was supplied, and the report carries a note. A path typed on the command
line is looked at by EmuWiz, so what is there is `measured`; that the launch
used it in that role is the caller's statement. A finding whose only firm
support is such a path says so in its limits and is held at `probable`, and
the report carries a note. The same holds when the only firm support beside a
stated fact is that the launch failed: the finding stays `possible`. When how
the launch ended was itself stated, the `Launch` line of the text report is
labelled as supplied; in JSON, `launch` follows the `process_exit` item, whose
`trust` is then `supplied`. A collector that is not told otherwise records everything as supplied.
A report restored from JSON is for display: its fingerprints can no longer be
compared with the files it was made from, and say so instead of opening
whatever their text names.

Categories: missing firmware, invalid or unsupported media, unsupported
emulator version, graphics initialisation, missing dependency, sandbox failure,
permission error, audio device, controller configuration, conflicting
modifications, crash, unexplained exit, and "stopped before launch" for planner
refusals outside those.

Compatibility limits come only from the compiled-in table
(`daedalus::limitations`), which is part of the rule-set digest. The digest
covers every rule (id, version, category, summary), every signature, the
table of reported errors with their effects, the signal table and the
compatibility table with its summaries. Its one entry
restates the OpenRCT2 minimum already enforced in `launch/openrct2.rs`.

### Rules without a production collector yet

`controller.player_one_unbound`, `modifications.conflict`,
`media.integrity_failed`, `emulator.version_unsupported` and the graphics
probe inputs of `retroarch.video.vulkan_unavailable` are tested against typed
facts but nothing in production produces those facts yet. Until a collector
exists the corresponding area reads "not collected" in every report.

## Signatures

Log signatures are literal, line-anchored patterns captured from real output
(`crates/archivefs-core/tests/fixtures/daedalus/README.md` lists each capture).
RetroArch 1.21.0 facts that shaped the rules:

- it logs to stderr, and without `--verbose` a failed start prints one
  `Fatal error received in` line or nothing;
- a `-L` path that does not exist is ignored and RetroArch exits with status 0
  after drawing a blank frame;
- "Could not read content file" is printed for a missing file and for an
  unreadable one alike;
- "Failed to initialize audio driver" is printed on healthy runs of cores that
  produce no sound;
- the first occurrence of a repeated settings key wins;
- `--appendconfig` overrides `video_driver` for that launch, but appended
  values leak into the base settings file (see Repair below);
- `--verbose` is not written back to the settings file, on a clean exit, a
  SIGTERM or a SIGKILL (checked run by run; a regression test repeats it
  against the real program when one is available);
- a battery save that cannot be written is reported
  (`[ERROR] [SRAM]: Failed to save SRAM.`) **only with `--verbose`**; without
  it nothing at all is printed, and the process exits with status 0 either
  way. RetroArch then keeps a recovery copy of the save data in its settings
  folder and says so. A successful save is reported as `Saved successfully`;
- the content-free cores used for these captures do not support save states,
  so no real save-state failure has been captured.

## Command line

```sh
emuwiz-cli diagnose-launch --emulator retroarch --exit-code 1 \
    --settings ~/.config/retroarch/retroarch.cfg --stderr-file captured-stderr.txt
emuwiz-cli diagnose-launch --rules
```

Read-only: it reads the files it is pointed at (settings and logs without
following a final symbolic link, logs up to 4 MiB), looks at the paths it is
given, and prints a report. It opens no database and starts no emulator.
`--help` prints help and does nothing else.

### JSON output

`diagnose-launch --json` prints one object:

| Key | Meaning |
|---|---|
| `schema_version` | `1`. Raised when a key is removed or changes meaning. |
| `rule_set` | `engine_version` and the `digest` of the rules, signatures and compatibility table used. |
| `subject` | `emulator` and `game`, as supplied. |
| `launch` | What happened to the launch, apart from why: `failed`, `failure_reported`, `no_failure_observed` or `not_observed`. |
| `verdict` | `explained`, `partly_explained`, `conflicting`, `unknown` or `no_problem_observed`. |
| `causes` | Why. Each has `rule`, `rule_version`, `category`, `confidence` (`established`, `probable`, `possible`), `effect`, `title`, `explanation`, `supporting` and `contradicting` evidence ids, `limitations`, `next_checks`, `repairs` (pointers only; nothing is applied) and `claims`. |
| `symptoms` | What happened. Same shape, with `explains_cause: false`. `reported: true` marks one that restates what a program printed; its `confidence` is then `established` as a report only, never as a cause. |
| `conflicts` | Pairs of rules whose findings contradict each other. |
| `rejected` | Findings the engine refused, with the reason. |
| `coverage` | One entry per evidence area: `observed`, `stale`, `asserted` (statements that are not observations) and the reasons anything is `missing`. |
| `notes` | Limits that apply to the whole report. |
| `evidence` | `subject`, `truncated` and `items`; each item has `id`, `area`, `state`, `trust` (`measured`, `reported`, `supplied`, `untrusted_text`, `asserted`), `source`, `fingerprint` and a typed `fact`. Paths are sanitised for display. |

`diagnose-launch --rules --json` prints `engine_version`, `digest` and `rules`
(`id`, `version`, `category`, `summary`). New keys may be added without a
version change. Text fields are for people and are not stable.

## Capture in Blue launches (`launch::retroarch_launch_diagnostics`)

A RetroArch launch from Blue asks for detailed logging (`--verbose`, placed
before `-L`) and keeps a bounded capture of what RetroArch prints, standard
output and standard error through one pipe: the first 64 KiB and the last
64 KiB, with one marker line where output was left out. Lines are kept in the
order RetroArch wrote them to the pipe, which for a program that buffers its
standard output can be later than it produced them. The capture lives in
memory. It is best-effort: if the pipe cannot be set up the launch still asks
for detailed logging, and keeps what every other launch keeps (the first
64 KiB of standard error; standard output is discarded).
`EMUWIZ_RETROARCH_QUIET_LAUNCH=1` switches both the flag and the wider capture
off. No settings file is changed and no retry with other settings is made.

A managed RetroArch started this way ignores a broken output pipe, so it
finishes writing its saves even if EmuWiz ends while the game is running.

The capture is for diagnosis and is never shown whole. Where Blue shows the
output of a failed RetroArch launch (the technical-details box), it shows the
last 40 lines only, with recognised private details removed and every
character made visible (`redact::output_tail_for_display`). A launch that did
not fail shows no output at all, and its capture is dropped as soon as Blue
has seen that it ended well. When Blue starts calling
`session::diagnose_finished_launch`, that call must come before the capture is
dropped, or the capture must be kept until the save check has run: a launch
that ended well can still carry RetroArch's report of a failed save, and that
output is the only evidence of it.

## After a launch (`daedalus::session`, `daedalus::notice`)

`session::diagnose_finished_launch` is the one call a launcher makes, from a
worker thread, once the process has ended: it collects evidence (recorded as
observed by EmuWiz, not supplied), diagnoses, assesses saves, records the
launch when it is one to keep, and returns a `LaunchNotice`. It starts
nothing and changes no emulator setting or game file; a history that is
missing, unusable or switched off changes nothing else about the result.

`notice::launch_notice` decides how loud the result is, as plain values the
interface only draws:

| Level | When | Shown as |
|---|---|---|
| `LaunchFailed` | the process failed, or a program reported an error that stops a game from starting | prominent; the headline says which of the two. A save problem found beside it is a separate part of the notice (`save_warning`), with its own next step |
| `SaveWriteFailed` | the launch did not fail and the emulator said a save could not be written | prominent |
| `SavesAtRisk` | nothing reported; the save folder is missing or not writable | a warning |
| `Quiet` | nothing to report | no banner; at most one quiet line about what save checking covered |

A problem that only degrades a game that ran (sound that could not start) is
not shown on Play; it is in Game Doctor. Nothing says that saves were written.
When the evidence disagrees or does not show why a launch failed, a cause that
is listed says how it stands ("One possible cause: ...") and is not the
headline.
The text is built from a redacted copy of the report.

Blue does not call these yet: the Play notice, the Game Doctor history and its
controls are the next slice. Until then the history is reachable through
`emuwiz-cli launch-history`.

## Saves (`archivefs_core::daedalus::saves`)

Whether a save was written is assessed apart from what happened to the
launch. A game can run, exit with status 0 and still lose a save.

| Status | Meaning |
|---|---|
| `write_failed` | The emulator stated that a save could not be written. `confirmed_by_check` when EmuWiz's own look at the save folder agrees, `reported_by_emulator` when nothing measured says why. Shown prominently; kept in history. |
| `at_risk` | Nothing was reported, but EmuWiz found the save folder missing or not writable. Shown; not kept on its own. |
| `no_failure_detected` | The monitoring that was available saw no failed write. Only the mechanisms listed as monitored were watched. |
| `monitoring_unavailable` | Nothing was watching: no provider for this emulator, no output, output without detailed logging, or output that was cut. |

Coverage is a table, not an inference. `saves::PROVIDERS` lists, per
emulator, the save mechanisms whose failed writes are recognised from a real
captured message. Today: RetroArch, in-game (battery) saves, which is also how
most of its cores keep memory cards. A mechanism counts as monitored for a
launch only when its provider's output was detailed and complete. That an
emulator is RetroArch does not make its save states monitored, and a clean
exit is never evidence that a save was written. No wording says saves were
written; the most that is said is that the emulator reported writing one.

Not monitored yet, tracked as later slices: save states, files a core or
emulator writes by itself (memory-card files), and every standalone emulator
(PCSX2, DuckStation, Flycast, Atari800 and the rest). For those Play shows a
quiet line ("Save check: not available for this emulator") and Game Doctor
says what that means. Adding a row needs a real capture of that emulator
failing to write that kind of save.

Nothing here changes a save path, a permission or a file. Next steps are
things for a person to check.

## History (`archivefs_core::daedalus::history`)

A private, bounded record so that Game Doctor still has something to show
after Blue restarts. Two kinds of launch are kept and no other: one that
failed, and one that did not fail but whose emulator stated that a save could
not be written. Ordinary launches are never recorded.

**What a record holds** is built field by field from fixed lists: when; the
game's library id and platform (not its title: a title is free text, so a
record names the game by its id and whatever shows the record looks the title
up in the library); the emulator, core and version as short
names of a fixed shape; how the process ended, as numbers; what happened to
the launch; the verdict and, per finding, the rule, category, confidence and
effect; kinds of reported error with counts; driver settings whose value is
on a fixed list; graphics-check results; missing libraries whose name is on a
fixed list; the save status and which mechanisms were monitored; sizes and
counts of the output examined.

**What a record never holds**: emulator output in whole or in part, file or
folder paths, user names, network or web addresses, environment values, or
any free text from a log, a settings file or a diagnosis. This is not
redaction. Redaction cannot be shown to catch every secret, so text that
could carry one is not filtered on the way in; it is not copied at all, and a
value that is not on a list is stored as "not kept". The wording shown for a
record is regenerated from the built-in tables.

**Limits**: the newest 20 records, none older than 14 days, at most 16 KiB
each and 512 KiB in total. What is past a limit is removed, oldest first,
each time a finished launch is offered to the store (recorded or not,
recording on or off), before a record is added, and when Blue lists the
history. The read-only views, including `emuwiz-cli launch-history list` and
`show`, remove nothing and leave out what is past a limit. So a record past
14 days can stay on disk until the next launch, and is not shown in the
meantime. Repeats of the same save warning for the same game are one record
with a count.

**The files**: one folder (`launch-diagnostics`, mode 0700) in EmuWiz's data
directory, refused unless it is a real directory owned by this user with no
access for others; one file per record (mode 0600) under a name this module
generates; written to a temporary name, flushed, and renamed without
replacing anything. A file that is too large, does not parse, holds a field or
a value a record never holds, or does not match its own name is never shown
and is removed at the next clean-up. Something under a record's name that is
not a regular file (a link, a folder) is never shown and never removed: only
plain files are ever removed. Saving is best-effort and returns its failures
as values; it cannot affect a launch.

**Controls**: list, show, delete one, clear all, and switch recording off
(a marker file, so the switch survives clearing). On the command line:
`emuwiz-cli launch-history [list|show <id>|status]` only read (a store that
does not exist yet is left absent), `delete <id>`, `clear`, `off` and `on`
change only the history folder; `--json` on `list` and `show` prints the
records as stored.

## Detailed export (`archivefs_core::daedalus::export`)

The detail of a launch, what the emulator printed and the evidence behind
each finding, exists only in memory after the launch. It leaves the program
only when a person asks: `ExportPreview::prepare` builds the exact text,
which opens with a privacy warning; nothing can be written without
`confirmed_by_person`; the only thing that can then happen is one new file
(mode 0600) in a folder the caller names, never replacing anything. Nothing
is sent anywhere.

The text has recognised private details removed (`daedalus::redact`: the home
folder, the user name in paths, values labelled as secrets, logins in web
addresses). That is a short list of literal rules and **not** a guarantee: an
unlabelled token, an unusual credential, another person's name or a network
address passes through. The warning says so, a test holds the claim to that
scope, and this is why redacted text is shown and exported but never stored.

## Repair (next slice; owner policy of 2026-11-14)

Scope order: **this launch only**, then **this game** (a per-game override,
only where the emulator applies the setting from one), then **all games** (the
main settings file, with explicit confirmation that every game using it is
affected). An override layer that an emulator does not honour for a setting is
never offered.

"This launch only" for RetroArch means a disposable profile: a scratch settings
file seeded read-only from the real one, with the repaired key changed and
`config_save_on_exit = "false"`, passed with `--config`. It does **not** mean
`--appendconfig` on top of the real settings file: RetroArch writes appended
values back into the base file by default, even when it is killed
(`docs/research/RETROARCH_APPENDCONFIG_PERSISTENCE_TEST.md`), so that would be
a silent permanent change. The disposable-profile mechanism already exists for
cheat launches (`launch/cheat_runtime_retroarch.rs`) and is reused.

Every repair:

1. names the emulator, its version, the file, the keys and the scope;
2. is previewed as an exact before/after of the file, with unrelated lines,
   comments and line endings preserved byte for byte;
3. is bound to a digest of the plan and of the file as previewed, and is
   refused if the file changed, is a symbolic link, is not owned by the user,
   is in an unsupported format, or the emulator is running;
4. is applied through the existing shared transaction (backup, exchange
   publish, journal, interrupted-apply recovery);
5. is undone only while the file still matches the recorded post-repair
   digest;
6. is not called a fix until an isolated test launch shows the outcome
   changed.

Production settings are not written until an independent Tier A review has
passed and the owner enables it. Development and tests use disposable profiles
only. New transactional capability is coordinated with Ouroboros, not built
here.

## Isolated experiments and outcomes (next slice)

An experiment starts the emulator against a disposable copy of its settings
with one difference, in a controlled environment, with a time limit, and
records exit, log and a captured frame. Outcome levels are claimed only from
their own evidence: configuration validated, process started, game loaded
(core loaded, content ran, a non-blank frame), gameplay observed (frames
changing after load, or the user confirming it), performance measured, repair
confirmed, inconclusive or failed.

## Optional local AI (last slice)

An OpenAI-compatible endpoint on the loopback interface (llama.cpp, Ollama and
Open-WebUI all provide one). The model receives the structured report and
sanitised excerpts as data, and may return hypotheses, further safe checks and
plain-language explanations that cite evidence ids. Its output enters the
evidence set as `asserted`, which the engine never counts. It cannot run
commands, change settings or approve a repair, and everything works without it.

## Coordination

- Leviathan (DAT evidence, schema 27) and Ouroboros (transaction engine,
  schema 29+): Daedalus edits none of their files and plans no migration.
- Shared registration files (`lib.rs`, CLI `main.rs`, `gui_v2/mod.rs`,
  `routes.rs`) receive registration lines only (a module line, a dispatch
  arm, a help line), never logic.
