# Save & State CLI

`emuwiz-cli saves` is a read-only projection of the existing emulator profile
discovery and persistent-state inventory. It does not restore, copy, convert,
delete, move, or rewrite emulator configuration.

## Commands

```text
emuwiz-cli saves summary
emuwiz-cli saves list
emuwiz-cli saves list --emulator pcsx2
emuwiz-cli saves list --type savestate
emuwiz-cli saves inspect state-0000
emuwiz-cli saves --json
```

`--json` returns stable provider-neutral records including the record ID,
emulator, installation binding, state type, portability class, provenance,
identity evidence, warnings, and summary counts. Human list output omits full
paths unless `--verbose` is supplied; inspect output is intentionally detailed.

The orchestration consumes only paths already established by the existing
profile discovery adapters. A missing path is reported as `unavailable`, never
as an empty save directory. Multiple profiles are not combined: without one
unambiguous profile, that emulator contributes no guessed records.

System containers such as RPCS3 `dev_hdd0` remain opaque records unless the
existing inventory adapter proves contained game identity. Savestates retain
their emulator-bound portability classification.

The command is suitable for offline use. It does not query providers or start
emulators, and inventory hashing remains bounded by the core inventory limits.
