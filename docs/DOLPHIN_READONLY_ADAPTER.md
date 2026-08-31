# Dolphin inspection adapter and apply boundary

## CURRENT BEHAVIOR

The Dolphin inspection adapter discovers profiles and inventories
GameSettings INI files read-only. The external Gecko provider is a separate
source-validation and retrieval path; remote content is treated as inert
data, not executable code.

Apply requires a separately verified GameCube/Wii identity and any applicable
revision evidence. Selected Gecko records can be materialized into an exact
GameSettings destination and applied through the shared transaction engine
with confirmation, backup, journal, verification, and rollback. Existing
settings and unrelated entries are preserved by the supported installer.

Texture-pack and other mod workflows have separate coverage. A local package
can be inspected and planned, but unsupported formats, ambiguous identity,
unsafe paths, and external installers fail closed. Discovery and preview
never create Dolphin profiles or mutate files.

The filename READONLY_ADAPTER is historical: it identifies the inspection
boundary and remains compatible with existing references; it does not claim
that all current Dolphin flows are universally read-only.

See [SHARED_CHEAT_PREVIEW.md](SHARED_CHEAT_PREVIEW.md),
[SHARED_SAFE_APPLY_ROLLBACK.md](SHARED_SAFE_APPLY_ROLLBACK.md), and
[ADAPTER_SUPPORT_MATRIX.md](ADAPTER_SUPPORT_MATRIX.md).
