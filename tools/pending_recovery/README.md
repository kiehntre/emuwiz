# Pending recovery inspector

The inspector is read-only and emits a descriptive recovery view plus an
optional descriptive plan. It never calls resume or rollback APIs.

In addition to rename, shared transaction, cheat, database-restore, and
ES-DE records, it discovers bounded-depth `.emuwiz-patch-output-*.json`
journals and projects patch-output schema 1 into the same normalized states.
Source, patch, temporary output, destination, and provenance paths are checked
against the hashes recorded by the journal. Safe classifications mirror the
standalone patch recovery rules: verified temporary output can be a
`SAFE_RESUME_CANDIDATE`; verified published output with no pre-existing
destination can also be a `SAFE_ROLLBACK_CANDIDATE`; changed or ambiguous
evidence is `DO_NOT_TOUCH` or `REVIEW_REQUIRED`.
