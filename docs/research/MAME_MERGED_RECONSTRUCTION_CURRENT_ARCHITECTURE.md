# MAME merged reconstruction: current architecture

This port is a projection over the current MAME evidence path. The historical
`dat::mame_normalizer` aggregate plan and its private repair journal are not the
authority for this feature.

| Reconstruction fact | Current authority |
| --- | --- |
| Parent/clone identity | Parsed MAME DAT `DatGameEntry.clone_of`, selected by the exact DAT revision |
| ROM ownership | Persisted `ArcadeJoinEvidence` and `ArcadeMemberEvidence` after checksum matching |
| Physical provenance | `mame_arcade_join_paths_for_dat` archive path plus evidence `current_name` |
| Catalogue freshness | SHA-bound join `dat_sha256`, version, and non-stale persisted audit query |
| Destination planning | `MameMergedReconstructionPlan`, with deterministic member ordering |
| Collision policy | Existing destination is always a hard plan refusal; no overwrite or guessed merge |
| Mutation boundary | Current staged-output and shared `rename_apply` transaction primitives |
| Recovery/undo | Shared transaction state/journal/reconcile/rollback only; no stronger promise is made |
| GUI | GUI-v2 Organisation area, using the typed plan as its preview model |

Filenames are display/provenance fields only. A member is actionable only when
the persisted observation carries a checksum identity that resolves to exactly
one required DAT ROM. Missing, duplicate, stale, conflicting, and incomplete
evidence remains blocked. Source archives are inputs and are never rewritten.

The planner currently accepts extracted source-set directories for the staged
writer. Packed source archive-member copying remains an explicit gap until the
current archive-member reader can provide a safe, identity-bound stream for the
same transaction boundary.
