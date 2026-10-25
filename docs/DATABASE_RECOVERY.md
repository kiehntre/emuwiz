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

## Safe sequence

1. Stop concurrent EmuWiz writers and copy the database plus sidecars.
2. Run emuwiz-cli database-check against the copy.
3. Preserve the original if SQLite reports recovery or corruption.
4. Use SQLite-supported recovery tooling on the copy, or rebuild the
   catalogue by scanning the configured sources.
5. Re-audit DAT identity and verified facts where the rebuilt catalogue lacks
   them.

Applied migrations are immutable. Never replay or edit migration 0006 (the
old PS2 loose-ISO example), and do not invent a migration 0011.
