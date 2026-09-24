# Standalone patch-output durable recovery

Standalone IPS, BPS, UPS, PPF3, and xdelta/VCDIFF output now follows a
durable publication boundary. The selected source is read and hashed but is
never renamed, replaced, or deleted.

The operation writes a JSON journal beside the destination before creating
the output. It records source and patch identity, the same-filesystem
temporary path, expected output evidence, provenance path, and durable
checkpoints through `Prepared`, temporary verification, publication, published
verification, and `Completed`.

Patch application still uses the existing format-specific decoders. Bytes are
written to a same-directory temporary file, synced and hashed, then exposed
with a no-clobber hard-link publication. The final destination is hashed again
before completion. Existing destination and symlink paths are refused.

After a crash, `discover_pending_patch_outputs` and
`inspect_patch_output_operation` are read-only. They never resume or delete.
An explicit recovery plan revalidates its operation token immediately before
`resume_patch_output` or `rollback_patch_output`. A changed source, patch,
destination, temporary file, or unknown journal schema fails closed. Rollback
removes only an unchanged output proven to have been created by that journal.

Completed and rolled-back journals remain as history. Temporary artifacts are
removed only after the journal reaches a terminal state and only when they are
regular files at the journal-recorded path. No generic temporary-file cleanup
is performed.
