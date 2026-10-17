# Native Discovery import Undo

Confirmed bounded Apply writes its operation receipt, exact inserted archive IDs,
raw Unix paths, source ownership, full-row archive/assignment hashes, native
candidate and content/fingerprint evidence, catalogue provenance and outcomes in
its existing SQLite `BEGIN IMMEDIATE` transaction. A result is successful only
after `COMMIT`. No filesystem journal is added. SQLite owns atomicity and crash
recovery; the existing `OperationRecord` adapter and Blue History project the
same authoritative database receipt.

Preview is a consistent read transaction. Confirm reserves the existing writer
transaction and reloads the immutable receipt. Its private preview binds the
operation, receipt digest and database object for 30 minutes. The executor checks
the recorded schema version/history and exact reviewed schema definitions,
including triggers. Unknown columns, tables or relationships require review.
SQLite's `SQLITE_FCNTL_HAS_MOVED` additionally checks the connection's actual open
file against its current pathname; metadata checks alone cannot bind an open
connection. The adapter requires durable journalling and FULL synchronous mode.

Only the recorded platform-assignment rows and archive rows can be deleted,
explicitly, with exact affected-count checks. No cascade is authorised. A later
archive, assignment, DAT, enrichment, scan, path-reference or relationship change
marks irreversible per-record touch evidence. This protects edits reverted to
old values and delete/reinsert attempts. Opaque topology changes conservatively
invalidate every untouched batch. Unrelated record edits remain compatible with
Undo. Preexisting path-based Discovery/fingerprint history is retained unchanged.
New unknown dependency shapes block the whole operation; partial Undo is not
supported. No old import is given invented ownership evidence.

Receipts and ownership rows survive Undo, cannot be updated/deleted/replaced, and
also reserve imported integer IDs permanently. Shared archive insertion skips
these tombstones, preventing delayed verification jobs from attaching evidence
to a new record using an undone ID. Explicit insert/rekey attempts are refused.

History distinguishes inserted rows, retained preexisting comparison records,
actual candidate refusals during Apply, and other context exclusions (which can
include deliberately unselected candidates). Each outcome retains its reason.
The Blue journey is Results → Preview Undo → confirmation → committed result.
Database work runs on workers; exact affected IDs are paged. A worker's missing
reply is an unknown outcome, resolved by reloading durable History.

An interrupted SQLite transaction either rolls back or commits its records and
receipt together. A hot-journal/read-only recovery refusal goes to the existing
copy-first database recovery workflow; this feature does not bypass that policy.
Repeated Undo reports already undone without deletion. Validation refusals and
rolled-back attempts remove nothing and can be previewed afresh. A commit error
is reported as unconfirmed and requires durable History/recovery to establish
its outcome before retrying. Undo never opens ROM files and works
when media is offline. It does not change paths, saves, artwork or emulator
configuration, and never restores a whole database backup.

Production imports remain paused pending implementation tests and independent
preservation review. Native Spectrum pilot preparation is separate and read-only;
there is no authorisation here to execute a production batch or upgrade a
production database silently.

SQLite reference: [file-control semantics](https://www.sqlite.org/c3ref/c_fcntl_begin_atomic_write.html#sqlitefcntlhassmoved).
