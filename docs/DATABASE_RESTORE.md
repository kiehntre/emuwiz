# Verified database restore

Database restore is an explicit recovery operation. Normal startup and
read-only catalogue loading never restore a database.

## What the user sees

History lists every retained pre-upgrade backup. `Review restore` prepares a
plan from the selected backup; the plan shows the backup's schema and, when it
is older than this build, that a staging copy will be upgraded first. Typing
`RESTORE DATABASE` enables `Restore database`. A completed restore shows
`Review rollback`, which puts the pre-restore database back from the emergency
backup after the same confirmation. A restore that was interrupted (crash,
kill, power loss) shows `Reconcile interrupted restore`, which records what is
actually on disk without changing any database file. The History page is a
separate EmuWiz process from Blue, so the protections below are all
cross-process.

## Writer exclusion

A restore must never lose a commit that SQLite acknowledged to another
connection, in this process or another. The executor therefore takes, before
it creates or changes anything, and holds until it has finished or rolled
back:

* the database **upgrade lock** (`<db>.upgrade.lock`, `flock`): no other
  EmuWiz process can open, create, upgrade or restore the database meanwhile
  (opens wait up to ten seconds, then refuse);
* a **`BEGIN IMMEDIATE` transaction** on the live file, i.e. SQLite's
  RESERVED lock: no connection anywhere can begin or commit a write. Nothing
  is written through this transaction, so it creates no rollback journal.
  Readers are tolerated; they keep the inode they opened.

A writer that already holds a transaction makes the restore **refuse** within
about two seconds; the restore never waits a writer out. A writer that was
between transactions gets `SQLITE_BUSY` on its next write and, after the
exchange, `SQLITE_READONLY_DBMOVED`: loud failures, never silent loss. In the
GUI the restore is additionally blocked while this window's catalogue is
loading, scanning, or running a source, alias, platform, missing-removal or
library-view action. The plan's freshness (size, mtime, SHA-256) is re-checked
under the lock, so a plan reviewed before a commit is refused as stale.

## Sidecars

The catalogue runs in DELETE journal mode. Before SQLite may open the live
file the executor inspects its sidecars with `symlink_metadata`:

* `-wal` or `-shm` present: refuse (a WAL belongs to the pathname and would be
  carried onto the wrong database);
* `-journal` with a live header while SQLite's RESERVED/PENDING lock is held:
  refuse as **busy**, a writer is mid-transaction;
* `-journal` with a live header and no visible lock: refuse with **"database
  activity cannot be determined"**. A crashed writer and a writer EmuWiz
  cannot see (another locking VFS, a network filesystem, an unreadable lock
  table) look identical from outside, so no claim of a crash is made and the
  journal is left untouched for the copy-first recovery procedure. EmuWiz
  never rolls a hot journal back silently.

The lock is read from `/proc/locks`, never with `fcntl` on a descriptor of
the database: on POSIX, closing any descriptor a process holds on a file
releases every record lock that process has on it, so a probe that opened
and closed the live database would drop this process's own SQLite locks
(the exclusion's, or a running scan's). For the same reason the executor
hashes the live database through one handle held open for the whole
exclusion, and every entry point that reads the live file with plain I/O
first refuses if this process holds any lock on it. The normal open path uses
the same classification, so a live writer is reported as "busy" there too.

## Staged upgrade of older backups

Every backup EmuWiz retains is a pre-upgrade backup, so it is usually at an
older schema. The selected backup is only ever opened read-only. It is copied
with SQLite's online-backup API into a staging file beside the live database,
verified at its own schema, migrated there through the same migration chain
the protected upgrade uses, fsynced and verified again (`quick_check`, schema,
required tables). A failed migration removes the staging copy and fails the
restore with the original untouched and the backup unchanged. Disk space for
the emergency backup, the staging copy and its migration journals is checked
first.

## Order of operations and the durable record

Every receipt (`<db>.restore-<plan id>.json`) is written atomically: temporary
file, fsync, rename, directory fsync. The receipt names the emergency backup
and the staging file before either exists, and records the staging file's
SHA-256 and inode before the exchange, so an interruption at any point can be
classified afterwards.

1. receipt `Validating`;
2. exclusion (above), then plan validation under the lock, including
   `quick_check` of the live database;
3. emergency backup of the live database (online backup, fsync, verify),
   receipt `Emergency backup created`;
4. staging copy (and upgrade), receipt `Restore staged` with hash and inode;
5. final sidecar check, `renameat2(RENAME_EXCHANGE)` of staging and live
   paths, directory fsync, receipt `Restore applied`;
6. verification of the restored file (inode and hash match the staged copy,
   `quick_check`, schema, tables); on failure the superseded original, still
   at the staging path, is hash-checked and exchanged back (`Rolled back`), or
   if that fails too the receipt names both the original's path and the
   emergency backup (`Rollback failed`);
7. the superseded original is unlinked (its content is in the verified
   emergency backup), receipt `Completed`.

Any failure before step 5 finalises the receipt as `Failed` with
`original_intact = true`.

## Recovery after restart

`interrupted_restore_receipts` lists receipts that never reached a terminal
state. `assess_interrupted_restore` compares the live file's hash with the two
hashes the receipt recorded:

* equal to the pre-restore hash: the exchange never happened, the original is
  intact; leftover emergency/staging files are listed, never deleted;
* equal to the staged hash and verifies: the replacement is in place; the
  superseded original is reported if it still sits at the staging path;
* otherwise: manual review.

`finalise_interrupted_restore` repeats the assessment under the exclusion and
only rewrites the receipt (`Failed` with the original intact, or `Completed`).
History shows the assessment for every interrupted receipt.

## Rollback

`review_database_rollback` checks, read-only, that the emergency backup exists
with its recorded hash and that the live database is still the restored
replacement. `rollback_database_restore` then takes the exclusion, stages the
emergency backup, exchanges, verifies, and records `Rolled back`; a failed
verification exchanges the restored database back.

## Limitations

* Readers in other processes keep their consistent view of the old inode
  until they reopen; the GUI reloads its own catalogue and says a restart may
  be needed elsewhere. Writers in other processes fail loudly rather than
  being paused.
* The restore runs on the History window's thread, so a very large database
  makes that window unresponsive for the duration (hashing is streamed, not
  loaded into memory).
* The emergency backup is recorded in the restore receipt, not under a backup
  ownership receipt, so it is offered through `Review rollback` rather than as
  a History backup entry.
* `renameat2(RENAME_EXCHANGE)` is Linux-only; other platforms refuse before
  any change.
