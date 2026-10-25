# Verified database restore

Database restore is an explicit recovery operation. Normal startup and
read-only catalogue loading never restore a database.

The GUI owns a cached read-only snapshot; scan and repair workers own short-
lived `Database` connections. The restore review is therefore available only
when the GUI database state is idle. Before replacement, all EmuWiz database
workers must be stopped. A separately running process holding the database
still requires a controlled restart before it can see the replacement.

The executor captures a restore plan containing the live path, selected backup
SHA-256, schema, and live freshness (size, modification time, and SHA-256).
Execution refuses a stale plan. It validates the selected backup read-only,
including SQLite `quick_check` and required catalogue tables, then creates and
verifies a new SQLite online-backup of the current live database.

The selected backup is copied into a same-directory SQLite staging file and
validated again. On Linux, `renameat2(RENAME_EXCHANGE)` atomically exchanges
the staged file and the live file. Existing `-wal` or `-shm` sidecars block the
operation because a sidecar belongs to the pathname and must not be attached
to a different database. The old live inode remains at the staging path until
post-restore verification succeeds; on verification failure it is exchanged
back, while the emergency backup remains as an independent recovery source.

The GUI uses `Review restore` and requires the exact phrase `RESTORE DATABASE`.
After a successful replacement it reloads the catalogue and tells the user
that a restart may be required for other processes. Rollback uses the verified
emergency backup only when the restored live database still matches the
recorded post-restore identity.
