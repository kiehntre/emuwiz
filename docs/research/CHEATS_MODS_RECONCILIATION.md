# Cheats & Mods reconciliation

The authoritative main line already contains the core transaction and
provenance machinery. The current surface includes RetroArch cheat discovery
and malformed-input tolerance; BSFree/GameCube/Wii classification; PCSX2
texture replacement; RPCS3 ordinary mod layering; Cemu graphic packs; PPSSPP
texture packs; provider-neutral mod discovery and provider-linked history; and
patch/fan-translation previews with recovery journals.

The GUI exposes discovery, candidate review, previews, explicit apply, history
and rollback where an adapter supports it. Compatibility is adapter-specific
and remains a readiness/result field rather than an identity claim. Source ROM
bytes are not modified by these workflows.

The newer GUI-v2 cheats/mods visual branch is superseded by the current
authoritative GUI architecture and is not replayed. No non-duplicative,
low-risk integration seam was found that is more important than preserving the
existing transaction boundaries. The next proven gap is a shared GUI summary
that joins adapter compatibility/readiness with the already-existing
provider/provenance and recovery projections; it should be implemented only
after the active GUI exposes those projections in one route.
