# Missing-record catalogue cleanup

This recovers `b6f455cf` and `c6931472`, not an imported-DAT lifecycle feature.
DAT versions, imports, identity projection and Unknown / Not checked are unchanged.

The existing Library missing-entry review and confirmation removes database rows
only. Its worker now carries the exact reviewed record's id, path, source and
relative path. CLI `library-remove-missing --path` resolves paths inside the
reserved SQLite write transaction, not into reusable ids before it. Explicit
`--id` keeps its existing meaning: the record holding that id at execution.

Historical Missing is insufficient. Cleanup requires an available, unchanged
bound source, current completed scan authority, unchanged source ownership,
consistent paths and confirmed live absence. It rechecks before deletion and
commit; inaccessible, I/O-error, replaced, ambiguous or returned paths refuse
the entire selection. Concurrent database writers cannot change the generation
while the reserved write transaction is open. Failed or suppressed deletion
rolls back instead of reporting a successful removal.

The existing confirmed-removal contract deletes the selected records and their
owned platform assignments, scan observations, library DAT identity snapshots,
verified identity facts and enrichment metadata. No automatic cleanup is added.
Persisted arcade set-audit evidence is separate historical evidence; if it still
references a selected record, the whole removal refuses and preserves it.
Unselected records and imported DAT inventories are retained. For receipt-backed
Undo, use the existing **Forget confirmed-missing** workflow instead; this older
explicit Remove Missing Entries operation does not acquire a new Undo contract.

Filesystem writers are not locked by SQLite. A file can reappear after the final
absence observation but before commit; no game file is ever removed or written,
and a later scan can recatalogue it. Delete counts and identity postconditions
are checked; this does not sandbox arbitrary malicious SQLite triggers or
exclude tools which bypass SQLite locking.
