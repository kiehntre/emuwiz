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
  `reported` (a structured channel content cannot write to),
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
cause. Verdicts: `explained` (an established cause, no contradiction),
`partly_explained`, `conflicting`, `unknown`, `no_problem_observed` (which is
explicitly not a statement that the game works). Once something failed, only
a cause that can prevent a launch explains it; a problem that merely degrades
the game (no sound, saves not written) is still reported but does not turn an
unexplained crash into an explained one. An exit status that only relays a
request to stop (143 and the like) is a note, not a failure.

Settings are read per layer (main file, core, folder and game overrides). A
layer whose file does not exist is recorded as absent, which is an
observation. A layer that exists and could not be read, or changed while it
was being examined, holds every setting-based finding at `possible`, because
it may hold the value the emulator really uses.

Whoever supplies evidence is stated. What EmuWiz observed itself is
`measured`; an exit status or environment typed on the command line is
`reported`, its source says it was supplied, and the report carries a note.
A report restored from JSON is for display only: it cannot be compared with
the files it was made from and is never diagnosed again.

Categories: missing firmware, invalid or unsupported media, unsupported
emulator version, graphics initialisation, missing dependency, sandbox failure,
permission error, audio device, controller configuration, conflicting
modifications, crash, unexplained exit, and "stopped before launch" for planner
refusals outside those.

Compatibility limits come only from the compiled-in table
(`daedalus::limitations`), which is part of the rule-set digest. Its one entry
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
  values leak into the base settings file (see Repair below).

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
| `verdict` | `explained`, `partly_explained`, `conflicting`, `unknown` or `no_problem_observed`. |
| `causes` | Why. Each has `rule`, `rule_version`, `category`, `confidence` (`established`, `probable`, `possible`), `effect`, `title`, `explanation`, `supporting` and `contradicting` evidence ids, `limitations`, `next_checks`, `repairs` (pointers only; nothing is applied) and `claims`. |
| `symptoms` | What happened. Same shape, with `explains_cause: false`. `reported: true` marks one that restates what a program printed. |
| `conflicts` | Pairs of rules whose findings contradict each other. |
| `rejected` | Findings the engine refused, with the reason. |
| `coverage` | One entry per evidence area: `observed`, `stale`, `asserted` (statements that are not observations) and the reasons anything is `missing`. |
| `notes` | Limits that apply to the whole report. |
| `evidence` | `subject`, `truncated` and `items`; each item has `id`, `area`, `state`, `trust` (`measured`, `reported`, `untrusted_text`, `asserted`), `source`, `fingerprint` and a typed `fact`. Paths are sanitised for display. |

`diagnose-launch --rules --json` prints `engine_version`, `digest` and `rules`
(`id`, `version`, `category`, `summary`). New keys may be added without a
version change. Text fields are for people and are not stable.

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
