# Saturn component patch semantic reconciliation

Reviewed before implementation against authoritative main e306d7c293c179660800d62c23e3e6013e414861. Candidate commits: ffcbd909, 1f6ce22d. No wholesale cherry-pick.

| Candidate hunk / responsibility | Classification | Integration decision |
| --- | --- | --- |
| lib.rs export | still useful | Export the adapted backend. |
| CUE/layout, Saturn manifest/identity/readiness and standalone patch engines | already on main | Use canonical parser, manifest and patch engine; do not fork these models. |
| request, preview, disc ordinal checks | needs adaptation | Private immutable plan binds reviewed manifest, exact component, track, patch digest, source membership and matching disc ordinal. |
| caller-constructed readiness acceptance | unsafe / discard | Labels are not apply authority; verify current canonical manifest and exact reviewed component/patch relationship. |
| target_component | needs adaptation | Require an explicitly reviewed whole component and data track mapping. Refuse logical-track hashes treated as whole-component hashes, audio/mixed components and basename fallback. |
| validate_source / patch format checks | still useful, needs adaptation | Bound inputs before engine inspection; CUE only, complete native identity, IPS/BPS/UPS/PPF3 only. No SSP, CHD, xdelta or filesystem rebuild. |
| copy_source_set / tree_hash | needs adaptation | Complete bounded source directory copy, including extra members/directories; canonical content identities instead of basename-only fingerprints and unbounded reads. |
| preserve_manifest | still useful, needs adaptation | Preserve exact CUE bytes, component membership, relative mapping, all track facts and native System ID; allow only reviewed component content and corresponding logical data hash to change. Audio unchanged. |
| standalone output preparation | still useful | Reuse current-main build_standalone_patch_apply_plan and prepare_standalone_patch_output inside shared staging. Require same-length component output. |
| standalone_patch.rs record-growth changes | already on main / superseded | Current engine is authoritative; no engine changes. |
| standalone_patch.rs EOF grow-only change | unsafe / discard | Explicit IPS truncation remains authoritative; backend rejects layout-changing output rather than changing engine semantics. |
| private staging/ID, rename publication, independent receipt/state, recursive-delete rollback/cleanup | superseded / unsafe, discard | Sole publication/recovery contract: TreePatchPlan and tree::{prepare,publish,inspect,undo}. |
| fixture builders, IPS/BPS/UPS/PPF tests, identity/ordinal/source/patch/collision/rollback tests | test-only | Salvage synthetic formats and assertions, replace private transaction expectations with shared lifecycle; extend stale membership, audio/layout and explicit truncation tests. |

Size policy: dedicated extracted CUE source directory at most 1 GiB (bounded CD component set), patch at most 128 MiB, selected component at most the canonical engine's 512 MiB. Output must retain every component's length, so maximum staging is 1 GiB. Each shared plan allows actual combined input bytes (at most 1152 MiB), counting logical sizes including sparse files. No automatic 8 GiB allowance.

GUI integration is deferred: current Saturn GUI is manifest inspection, without component-patch apply/recovery ownership. No dedicated page or misleading apply-ready state will be added.

Implementation notes: native `CD-N/M` System ID evidence must match the reviewed disc ordinal. Only `ComponentBin` targeting a single data track is accepted; `FullRawImage`, logical-track, audio and shared multi-track component targets are deliberately refused. The canonical standalone apply plan is retained from review, with only its base path relocated into staging, so staged bytes cannot become a fresh source-identity baseline.

Final preservation audit: the manifest projection does not retain every CUE directive or raw-sector header byte. Adapter admission therefore allows only BINARY FILE, supported TRACK, INDEX 00/01, PREGAP/POSTGAP and benign REM COMMENT/GENRE or quoted TITLE declarations; the canonical parser still owns all layout/path resolution. It refuses other declarations and nonconsecutive track numbering. Raw MODE1/2352 and MODE2/2352 targets additionally preserve every sector's sync/address/mode (and XA subheader) bytes and must agree with the CUE mode. MODE1/2048 remains supported; no sector reconstruction or checksum engine is introduced.
