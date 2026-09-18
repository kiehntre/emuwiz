# Managed provider snapshot lifecycle

Managed identity sources use a provider-neutral lifecycle around provider-owned
parsers:

`check metadata -> stage bounded bytes -> validate -> preview -> explicitly activate`.

The snapshot store owns source provenance, content-addressed immutable bytes,
atomic metadata, activation history, locking, and rollback. A provider owns
only the interpretation of staged bytes and returns a validation report. The
store never executes provider content, rewrites an active snapshot during a
check, or mass-revalidates identity results.

The active pointer is separate from immutable snapshot records. An activation
returns a `NeedsRecheck` signal when the content hash changes; callers may use
that signal to mark existing identity evidence stale without changing it. The
active snapshot remains usable offline, and a failed check, fetch, or validation
leaves it untouched.
