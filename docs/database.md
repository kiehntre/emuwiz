# Database and persistence

## CURRENT BEHAVIOR

The SQLite catalogue is a durable, additive record of library observations and
identity evidence. It is not the live filesystem or mount authority, and it is
not a trust anchor for launch or apply. Read-only catalogue, report, and
database-check paths use an explicit read-only open and do not create a
database, parent directory, sidecar, or migration.

The committed migration chain currently ends at 0010:

| Migration | High-level ownership |
|---|---|
| 0001 | sources, archives, scan runs/observations, platform assignments |
| 0002 | user platform aliases |
| 0003 | per-source scan status/history |
| 0004 | scan skip and unchanged counters |
| 0005 | source-folder platform assignment |
| 0006 | persisted game identity reports |
| 0007 | full ingestion/discovery details for a scan run |
| 0008 | per-library-item DAT identity results and evidence snapshots |
| 0009 | DAT set-audit and dependency verdicts |
| 0010 | verified identity facts with file freshness snapshots |

The identity layers are intentionally distinct. Discovery details describe
what ingestion saw; identity reports explain structural inspection; DAT rows
persist release/hash results per source; set-audit rows persist set/dependency
outcomes; verified facts persist bounded direct-media facts such as serials,
IDs, or executable identity. A later read can mark cached facts stale when the
file snapshot changes.

## Ownership rules

Scans may update catalogue state through the migration-capable path. Read-only
views, Library/Playing Library reads, RomM mapping/reporting, Doctor, and
database-check must not migrate or repair the database as a side effect.
Launch and apply planning use fresh evidence checks rather than trusting a
stale cached row.

Applied migrations are immutable. Never edit or replay an old migration, and
never invent migration 0011 in documentation or code. Future schema changes
must append a new migration and preserve the forward-only chain. The stale
PS2 loose-ISO work is a useful example: migration 0006 is historical and must
not be edited to accommodate later identity behavior.

See [DATABASE_DESIGN.md](DATABASE_DESIGN.md) for design rationale and
[DATABASE_RECOVERY.md](DATABASE_RECOVERY.md) for copy-first recovery guidance.
