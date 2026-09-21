# EmuWiz upgrade preflight

`upgrade_preflight.py` is a read-only release gate for EmuWiz and ArchiveFS
installations. It reports the effective config/data roots, ArchiveFS fallback
roots, SQLite state, configuration format, journals, managed emulator
manifests, configured absolute paths, mount warnings, and high-value persistent
state.

It never opens SQLite writable, upgrades schemas, rewrites TOML/JSON, scans ROM
libraries, fetches provider data, touches journals, or copies files. A backup
manifest is an inventory only; it does not perform a backup.

Examples:

```sh
python3 scripts/qa/upgrade_preflight.py
python3 scripts/qa/upgrade_preflight.py --json /tmp/emuwiz-preflight.json
python3 scripts/qa/upgrade_preflight.py \
  --backup-manifest /tmp/emuwiz-backup-manifest.json
python3 scripts/qa/upgrade-preflight-selftest.sh
```

For fixture/testing roots, use `--config-root`, `--data-root`,
`--legacy-config-root`, and `--legacy-data-root`. Normal resolution follows
EmuWiz's documented rules: explicit `EMUWIZ_*_HOME` overrides, then absolute
XDG roots, then `~/.config`/`~/.local/share`, with EmuWiz preferred over
ArchiveFS at the directory level.

Exit codes are stable:

- `0` — `SAFE`
- `1` — `SAFE_WITH_WARNINGS`
- `2` — `BLOCKED`
- `3` — inspection/tool error or unreadable/invalid input

Schema 20 is the current target. Schema 19 and other recognized older schemas
are reported as upgrade-required. A newer schema is unsupported and blocks the
preflight. Meaningful state in both EmuWiz and ArchiveFS roots is a blocker;
the tool never merges them. Actionable or unknown transaction journals are also
upgrade blockers. Missing source paths beneath `/mnt` or `/media` report
`MOUNT MAY BE UNAVAILABLE — DO NOT RESCAN`.

The default backup manifest includes configuration, the catalogue, DAT state,
emulator bindings, transaction/recovery state, library views, provider
provenance, and managed install manifests when present. Thumbnails, scan
fingerprints, and provider cache bodies are excluded as rebuildable by default.
No secret values are printed or written to reports; secret-looking fields are
represented only as configured/not configured.

The tool intentionally does not repair paths, merge roots, upgrade databases,
resolve journals, adopt external emulator installations, or decide whether a
user should delete a cache. Those are explicit post-backup operator actions.
