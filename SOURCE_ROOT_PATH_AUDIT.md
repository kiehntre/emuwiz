# Source-root path audit

This is an inventory of persisted or serialized path-bearing fields found on
the clean base `78bd8fc`. It is a semantic audit, not an instruction to apply
any migration.

| Subsystem / field | Persistence | Semantics | Safe response to a source move | Stale handling / impact |
|---|---|---|---|---|
| Registered source folders / `SourceFolderRecord.path` | SQLite | `SOURCE_ROOT_ABSOLUTE` | Yes, only with an explicit source-folder adapter and existence proof | Database scan state becomes missing; library-wide impact |
| Archive/database discovered paths | SQLite | `SOURCE_ROOT_ABSOLUTE` | Re-scan/recompute; do not blindly rewrite identities | Missing records and unmatched launches |
| RomM `ExternalIdentityRecord.archivefs_path` | JSON identity cache | source-root-derived local absolute | Adapter may plan exact root rebases; provider paths remain external | Existing mapping planner reports stale/unmatched records |
| RomM provider path / media URL | JSON cache/config | `EXTERNAL_PROVIDER_PATH` | No | Provider identity is not a local filesystem path |
| DAT audit records (`archive_path`, `scan_root`, `technical_path`) | JSON/report/cache | source-root-derived for scan/archive paths; DAT artifact path external to library | Recompute from current catalogue/scan; no generic blind rebase | Audit is stale, identity records can be regenerated |
| DAT managed source artifact path | TOML/managed state | `USER_SELECTED_EXTERNAL` or managed-cache path | No / regenerate managed cache | DAT source selection must remain stable |
| DAT rename plans (`source_path`, `destination_path`) | JSON plan/journal | live source-root paths | Pending/live adapter may plan recovery; apply remains separate | Recovery must distinguish current source from historical evidence |
| Completed rename/repair history paths | JSON journal/history | `TRANSACTION_HISTORICAL` | Never | Preserve original paths as evidence |
| Library view `source_folders`, `target_path`, `source_folder_path` | JSON config/manifest | source-root paths for source links; destination paths are `DESTINATION_ROOT_ABSOLUTE` | Source links can be regenerated from current view; destinations are not source-rebased | Broken links/manifests; repair/regenerate is safer |
| Library view history `destination_root`, `manifest_path` | JSON history | destination or historical | No generic rebase | Preserve history; current view can be rebuilt |
| Playing-library archive paths / destination root | in-memory plan/export | source-root input plus destination root | Regenerate plan from current catalogue; destination is not source root | Stale plan must not move files |
| Canonical organisation / duplicate quarantine paths | plan/journal/database reports | source-root-derived source and destination-root destinations | Recompute plans; quarantine destinations require explicit destination adapter | Collision/quarantine evidence must not be guessed |
| Artwork/local media mappings | provider cache/config and generated local media | provider/external or cache-internal; local output may be destination-root | Regenerate/download or explicit media adapter | Missing artwork is recoverable; no ROM-root inference |
| Emulator executable/profile paths | config | `USER_SELECTED_EXTERNAL` | Never source-rebase | Launch configuration must not follow ROM library moves |
| Cheat/mod install and backup/history paths | config, cache, journals | game-relative installs may be source-relative; emulator config and backups are external/history | Only game-library-relative references may opt in; configs do not | Installed content can be reinstalled/regenerated; history stays historical |
| Mount state / source discovery paths | config/runtime state | source-root absolute or temporary | Manual review/reconcile mount; no blind rebase | Missing mount makes all dependent paths stale |

The DAT identity model is content/catalogue keyed; its persisted local paths are
observations (`archive_path`/technical paths), not durable identity. They should
be invalidated or recomputed from the current catalogue rather than treated as
proven migration candidates. No hashes are used by the migration contract.

RomM's existing `identity_source::romm::mapping_plan` remains specialized: it
uses canonical platform identity and folder discovery to repair provider
mapping configuration. It is not rewritten here because its semantics are
provider-specific and its tests already cover that behavior. The new generic
planner is suitable for a future adapter over individual local cache references
and supplies exact containment/suffix/existence proof for that adapter.

The core contract is intentionally read-only. No database/config/cache write,
filesystem move, GUI hook, background task, title matching, or hashing was
added.
