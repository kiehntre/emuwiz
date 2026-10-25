# Recovered read-only CLI inspection

Source: `feature/cli-release-polish` at `0f985d55`. Recovered onto current
main starting at `ec09d581`, without its unrelated save-state/status edits.

```text
emuwiz-cli emulators status [--json]
emuwiz-cli activity [list] [--json]
emuwiz-cli history [list] [--json]
```

Each command accepts `--help`; unknown, duplicate and mutation arguments fail
before inspection. Human output and JSON report unavailable/malformed evidence.
Diagnostic reports exit successfully even when evidence is unavailable, as
Doctor's existing reporting convention does; argument/serialization errors fail.
JSON is a versioned object containing problems and explicit incomplete coverage,
not the historical bare array that hid warnings from JSON consumers.

Emulator status reuses the existing inventory scanner with a probe-free entry
point and the pure lifecycle projection. It checks the existing seven native
PATH candidate families and managed manifests. It never starts an emulator,
Flatpak/package command, release query, installation or download. Versions can
come from existing local manifests; an unprobed executable's version stays
unknown. Flatpak/additional AppImage locations, configured selections, launch
readiness and update availability are explicitly not assessed. A known local
version is not advertised as up to date. Invalid managed records remain visible.
Managed inspection can hash installed executables using its existing reader;
this is probe-free inspection, not a promise of constant-time I/O.

Activity and History reuse `OperationRegistry` over rename, Library View,
shared cheat/mod and database receipts. Activity means recorded unsettled or
needs-attention operations, including failures; it is not a live process list.
Recovery information describes recorded evidence, not approval to perform an
action. These commands do not reconcile, repair or invoke an executor. Existing
history readers are best-effort (200-entry Library View/database limits and
512 shared journals); coverage limitations are visible in both formats. Missing
storage is distinct from unavailable storage. Root-resolution failures never
fall back to the working directory. Existing compact-journal discovery remains
with the canonical reader, including when the ordinary directory is absent.

Validation uses disposable subprocess HOME/data/config/cache/temp/PATH roots,
synthetic journals and managed manifests, and a fake emulator which would write
a marker if executed. State snapshots compare directory membership and file
bytes, inode, mode and modification time before/after repeated commands.
Access times may change when files are read. No production commands are run for
validation. Existing Doctor, Setup, Launch, recovery and discovery-with-probes
callers retain their behavior.

Retain the historical branch. Its three command capabilities are accounted for
by this recovery; its separate `saves.rs`, `save_state_orchestration.rs` and
`statuses_from_archive_index` changes are deliberately outside this task and
are not justification for deleting its unique history.
