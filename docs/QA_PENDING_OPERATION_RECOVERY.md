# Pending-operation recovery inspector

`scripts/qa/pending-recovery.py` is a bounded, headless, read-only
view of EmuWiz transaction evidence. It answers whether known journals describe
an interrupted operation, which filesystem paths a later recovery action could
touch, and whether exact current evidence is sufficient to call rollback a
candidate. It never invokes an apply, resume, rollback, cleanup, or migration
function.

The inspector exists for diagnosis and recovery planning. It is not a recovery
engine. In particular, `SAFE_ROLLBACK_CANDIDATE` means only that all evidence
the supported journal format records currently agrees: expected applied output
is still present, every required backup exists with its recorded SHA-256, paths
remain under their recorded roots, and no symlink redirection was found. The
real transaction engine must revalidate those facts under its mutation lock and
require explicit approval before changing anything.

## Usage

Inspect the standard XDG/EmuWiz locations:

```sh
python3 scripts/qa/pending-recovery.py --redact-home
```

Inspect explicit disposable or restored roots:

```sh
python3 scripts/qa/pending-recovery.py \
  --data-root /absolute/path/to/data \
  --config-root /absolute/path/to/config \
  --es-de-root /absolute/path/to/ES-DE
```

Include settled history and create machine-readable, descriptive output:

```sh
python3 scripts/qa/pending-recovery.py \
  --include-history \
  --verbose \
  --json /tmp/emuwiz-recovery-report.json \
  --emit-plan /tmp/emuwiz-recovery-plan.json
```

JSON and plan files are the only writes the program can perform. They require
an explicit absolute path, use create-new semantics (never overwrite), require
an existing parent directory, and are refused beneath the inspected data or
config or ES-DE roots. Without either option, execution performs no writes.
The existing `scripts/qa/pending-recovery-inspector.sh` wrapper is retained as
an equivalent convenience entry point.

Root discovery accepts `--data-root` and `--config-root` first, then
`EMUWIZ_DATA_HOME` / `EMUWIZ_CONFIG_HOME`, then XDG data/config homes. The
preferred application directory is `emuwiz`; an existing legacy `archivefs`
directory is used only when the preferred directory does not exist. ES-DE
inspection defaults only to the documented `$HOME/ES-DE/gamelists` tree and
looks exactly one system-directory level deep. Repeatable `--es-de-root`
arguments support explicit/portable profiles without broad home-directory
crawling. The tool never scans ROM libraries, caches, or arbitrary directories.

## Supported evidence

Only direct journal files in known roots are considered:

| Family | Known data-root directory | Treatment |
|---|---|---|
| Rename / organisation | `rename-transactions` | Planned/apply/rollback checkpoints, identities, operations, created directories, and exact-resume envelope presence |
| Playing Library links | `rename-transactions` | Recognized from symlink/hardlink operations; exact link or inode evidence is inspected |
| Duplicate quarantine | `rename-transactions` | Recognized from `.emuwiz-quarantine` destinations; reverse-move proof uses recorded identity |
| Shared mod transactions | `shared-cheat-history` | Durable `*.pending.json` entry checkpoints, old final-only schema-1 history, and completed `*.rollback.json` receipts |
| Cheat install history | `cheat-install-runs` | Completed runs are history; an incomplete final-result journal is review-only |
| Cheat rollback history | `cheat-rollback-runs` | Completed runs are history; an incomplete final-result journal is review-only |
| Recovery visibility sidecar | `rename-transactions/recovery-history-state` | History/display state only; never recovery authority |
| Database restore | Direct child `library.sqlite3.restore-*.json` | Current plan, live/selected/emergency SHA-256 evidence, apply/rollback checkpoints, and WAL/SHM blockers |
| Library View history | `library_views/history` | Final-only RomM, ES-DE, and generic publication history; never reinterpreted as resumable intent |
| ES-DE gamelist publication | `<ES-DE>/gamelists/<system>/gamelist.xml.es-de-publish-recovery.json` | Exact prior content and current gamelist evidence, discovered only under bounded known roots |

Shared transaction context preserves the adapter/provider identity when the
journal contains it, including local archive packages, PCSX2, PPSSPP, Cemu,
RPCS3, and Xenia shared-transaction users. Old schema-1 shared journals are
treated as completed history, not reinterpreted as interrupted intent. Such an
old final-only record cannot reveal a crash that happened before that record
was written.

Unknown schema versions, malformed paths, corrupted JSON, path traversal,
unsafe symlink components, missing backups, and digest divergence fail closed.
Journals are capped at 16 MiB (large rename batches legitimately exceed 2 MiB).
Referenced regular files are hashed only when an
identity comparison is useful, with a 16 MiB per-file default and a bounded
total inspection budget. `--max-hash-bytes` can lower or raise the per-file
limit; a skipped hash cannot authorize recovery.

An ES-DE recovery record stores exact prior content but not the expected newly
published content hash. The inspector can prove a rollback candidate when the
gamelist already matches that exact prior state (or both prior and current are
absent). A divergent current gamelist is `REVIEW_REQUIRED`, because it could be
a later user change rather than the interrupted publication output.

## Normalized states and categories

Subsystem states map to:

- `Completed`
- `RolledBack`
- `Applying`
- `ApplyFailed`
- `RollingBack`
- `RollbackFailed`
- `NeedsReview`
- `UnsafeToResume`
- `UnknownSchema`
- `Unreadable`

The report separates `PENDING`, `RECOVERABLE`, `REVIEW_REQUIRED`, and
`COMPLETED_HISTORY`. Completed history is hidden unless `--include-history` is
used. Every record retains its original state and schema/format label.

Suggested statuses are deliberately narrower than the normalized state:

- `SAFE_ROLLBACK_CANDIDATE`: exact current destination and backup evidence
  supports asking the real engine for an explicitly approved rollback.
- `SAFE_RESUME_CANDIDATE`: currently limited to a database restore whose
  selected backup, unchanged live database, optional emergency backup, and
  sidecar state all match the durable approved plan.
- `REVIEW_REQUIRED`: evidence has diverged or is incomplete.
- `DO_NOT_TOUCH`: the journal/schema/path cannot be safely interpreted.
- `NO_ACTION`: settled history.

Rename exact-resume envelopes are reported as evidence, but are not labelled
safe to resume because this standalone invocation lacks the caller's current
plan generation/digest and does not duplicate the core engine's locked
preflight. Shared-mod recovery follows the core policy and never claims resume
is safe. Every candidate still requires the owning engine to revalidate under
its lock and obtain explicit approval.

## Exit codes

| Code | Meaning |
|---:|---|
| 0 | No pending operations were found |
| 1 | Pending operations exist, with no `REVIEW_REQUIRED` / `DO_NOT_TOUCH` result |
| 2 | At least one operation requires review or must not be touched |
| 3 | The inspection itself failed, or every discovered journal was unreadable |

A corrupted journal does not stop other known journals from being inspected.
When usable evidence remains, the result is surfaced as `DO_NOT_TOUCH` and the
overall exit code is 2; code 3 is reserved for a run that cannot continue
meaningfully.

## Plan semantics and privacy

`--emit-plan` writes evidence and one of only five recommendations:
`SAFE_ROLLBACK_CANDIDATE`, `SAFE_RESUME_CANDIDATE`, `REVIEW_REQUIRED`,
`DO_NOT_TOUCH`, or `NO_ACTION`.
The plan contains no executable commands and cannot be fed back to this tool to
perform an action. Paths are included because exact path identity is part of
recovery. `--redact-home` replaces the current home prefix with `$HOME` in the
human-readable report; structured JSON intentionally retains exact paths for
machine inspection. Journal contents unrelated to recovery are not printed,
and configuration files are never dumped, so provider credentials and tokens
are outside the inspection surface.

## Self-tests

```sh
python3 -m unittest -v tools/pending_recovery/test_inspector.py
```

The disposable suite covers completed, applying, apply-failed, rolling-back,
rollback-failed, exact safe rollback, missing/changed backup, changed
destination, future schema, corrupt JSON, parent symlink, duplicate quarantine,
partial shared-mod checkpoints, exact-resume evidence, Playing Library links,
recovery sidecars, database restore resume/rollback evidence, ES-DE exact and
divergent recovery, and an empty root. It snapshots fixture content, metadata,
and inode identities before and after inspection to prove all inspected trees
are unchanged.

## Deliberate limitations

- The inspector does not call production Rust or acquire transaction locks.
- It cannot reconstruct an operation for which no intent journal exists.
- Completed legacy shared journals and cheat result journals are history, not
  durable per-step intent; pre-journal crashes remain unknowable from them.
- Standalone patch output, arbitrary receipts, provider caches, and
  undocumented/newer journal formats are not guessed at. Unknown schemas are
  always `DO_NOT_TOUCH`.
- A large file whose digest cannot be checked within configured bounds cannot
  contribute to a safe rollback verdict.
- No automatic recovery is offered. Ambiguous destructive state must be
  reviewed with the owning transaction engine and fails closed here.
