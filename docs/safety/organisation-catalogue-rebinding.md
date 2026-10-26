# Organisation catalogue rebinding

Recovery #2 starts at `973e974840cd7715e859720d8f02c342210a1efd`, including
catalogue-health PR #158. The historical `emuwiz-canon-rename` branch was read
as a design reference; none of its commits were applied.

## Reproduction

A private TempDir held `games/Old.zip`, a SQLite catalogue and a rename journal.
Registering the source and upserting the file produced archive ID 1. The
original organisation transaction renamed it to `New.zip` and returned Applied.
`New.zip` existed and `Old.zip` did not, but the database lookup still found ID 1
at `Old.zip` and no ID at `New.zip`. The pre-fix Rust fixture passed with those
defect assertions (`catalogue_rebind_reproduction`, one test, zero failures).
The corrected Apply/Undo fixture now asserts the opposite association while
preserving the same archive ID and all non-path archive columns.

## Commit and recovery contract

Preview captures the source object identity and exact catalogue association.
Apply rejects missing/legacy proofs, changed object identity, changed catalogue
identity, stale catalogue size/mtime and a changed expected destination.
Destinations must be inside an enabled, registered source folder; this operation
never registers sources, creates a database, migrates a schema or scans a library.

Every move carries a sealed per-entry binding: database object, archive ID,
old/new source folder and exact path bytes, source root identities and owned
current cache/audit rows. Apply and Undo hold SQLite's writer lock across the
filesystem operation. The order is:

1. Revalidate catalogue associations and collisions.
2. Publish the sealed Applying/RollingBack intent.
3. Run the existing identity-checked, no-clobber filesystem operation and sync.
4. Commit the archive association, fingerprint key and owned audit paths in one
   SQLite transaction, preserving non-path metadata.
5. Publish Applied/RolledBack only after that commit succeeds.

A database update or commit failure after the filesystem operation returns an
error and leaves the journal entry in flight. Shared recovery first proves the
actual filesystem location and durability, then completes only the recorded
old/new catalogue association before promoting the entry. Either database
association is accepted for idempotent recovery; a third association, absent or
replaced database, source-root replacement, or any collision requires review.
Generic History Undo uses the same catalogue completion hooks.

Updates use archive ID plus expected association, and exact recorded audit IDs.
Contradictory or unassociated audit ownership is refused. Destination cache rows
are never deleted or overwritten. Existing names, identity facts, assignments,
provenance, observations and metadata remain intact. Scan-run discovery history,
import receipts and MAME member observations keep their observed paths rather
than acquiring authority at a new location. Existing import-history triggers
may mark the affected receipt touched. Linked-library creation keeps the original
catalogue association.

Advanced Protection uses a separate filename-pair/FD protocol that cannot carry
these catalogue checkpoints. Catalogue-bound organisation is refused before that
helper runs, and the compact authority decoder independently refuses this
binding. Standard Apply/Undo is the supported path. Existing unbound evaluation
fixtures retain their existing protocol.

This coordinates separate filesystem and SQLite commits; it does not claim a
single atomic commit across both systems. In-flight failures remain visible and
need the existing recovery/Undo workflow. No automatic renames or bulk repairs
are introduced.
