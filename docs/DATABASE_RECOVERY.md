# Database recovery

## CURRENT BEHAVIOR

Recovery is copy-first and diagnostic-led. EmuWiz does not silently rewrite a
damaged catalogue during a read-only report. Run database-check/Doctor, stop
writers, preserve the database and any SQLite journal/WAL sidecars, and work
on a copy before attempting repair.

The catalogue is a cache of observed library state. If it is unavailable,
the source folders remain the underlying input; a fresh scan can rebuild
current observations. Persisted DAT identity, set verdicts, and verified facts
are valuable historical evidence, but stale or missing rows never authorize a
launch or apply.

Integrity findings include open/migration status, SQLite recovery indicators,
sidecars, and stable error categories. A read-only failure is reported rather
than “fixed” by creating directories, migrating, truncating, or deleting
files. Application-level scan runs left open by interruption are history to
interpret, not permission to edit migration files.

## An interrupted write ("rollback journal with a live header")

If EmuWiz, or the machine, stops in the middle of saving, SQLite leaves a
`library.sqlite3-journal` file holding the pages the cut-off write replaced.
EmuWiz refuses to open a library in that state: opening it would make SQLite
roll the journal back and delete it, and from the file alone EmuWiz cannot
tell a crashed writer from one that is still running somewhere it cannot see.
That refusal is deliberate and is not relaxed by anything below.

`emuwiz-cli database-recover-journal` is the supported way out:

1. **Prepare** (no arguments). It changes nothing in the library. It
   requires that no SQLite write lock is held, that the database is on a local
   filesystem (not network or FUSE storage), and that there is no WAL. It then
   copies the database and journal into
   `library.sqlite3.journal-recovery-<time>-<pid>/original/`, proves neither
   file changed during the copy, lets SQLite roll back a second copy in
   `recovered/`, and checks that copy (integrity, record relationships,
   schema). The report says whether the interrupted write had reached the
   database at all, or the journal was a leftover that restores nothing.
2. **Apply** (`--apply <recovery-folder> --confirm`). Only if the live
   database and journal are still byte for byte the preserved set, SQLite
   rolls the journal back on the library itself. The result must hash to the
   checked copy. No recovered file is ever copied over the library. If apply
   is interrupted, run it again: the journal is not changed until the rollback
   has finished.

The recovery folder (originals, recovered copy, receipt) is never deleted by
EmuWiz. Recovery never adds or upgrades saved evidence: whatever the cut-off
write had not committed is simply absent, and a DAT re-check lists it again.

Anything prepare cannot establish is refused with the reason, and the manual
sequence below still applies.

## Safe sequence

1. Stop concurrent EmuWiz writers and copy the database plus sidecars.
2. Run emuwiz-cli database-check against the copy.
3. Preserve the original if SQLite reports recovery or corruption.
4. Use SQLite-supported recovery tooling on the copy, or rebuild the
   catalogue by scanning the configured sources.
5. Re-audit DAT identity and verified facts where the rebuilt catalogue lacks
   them.

Applied migrations are immutable. Never replay or edit migration 0006 (the
old PS2 loose-ISO example). Every migration up to the current schema already
exists under `crates/archivefs-core/src/migrations/`; add new ones only at the end.
