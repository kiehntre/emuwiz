# Save migration compatibility planner

EmuWiz now has a read-only, provider-neutral save migration planner.  It
accepts source and target observations and returns a `SaveMigrationPlan`; it
does not convert bytes, invoke tools, download dependencies, or write into an
emulator directory.

## Supported planning cases

The initial model names evidence for raw SRAM-family files, GBA raw saves,
SNES SRAM, Genesis/Mega Drive SRAM, PS1 memory-card images, Saturn backup
memory, PSP `SAVEDATA` directory contexts, and MiSTer-oriented raw saves.
RetroArch/core, standalone emulator, MiSTer, and original/raw targets can be
represented through the target emulator, core, platform, format, and
representation fields.

An exact game identity, compatible platform, exact format, and equivalent
representation are required before `DirectlyCompatible` is returned.  Raw
file sizes and memory-card image sizes are evidence; filename is never enough.

## Evidence rules

`FilenameOnly` produces `Ambiguous`, even when the extension looks familiar.
Missing game identity also prevents a direct-compatibility claim.  Stale
source evidence produces `StaleEvidence` before any compatibility conclusion.
Different game identities or platforms are `Unsupported`.

Container and wrapper identity are explicit.  A raw PS1 card is not silently
treated as a DuckStation card container, and Saturn backup-memory containers
are not interchangeable merely because their byte sizes match.  PSP
`SAVEDATA` is modeled as a directory context and is never treated as a raw
single-file save.

`ConversionAvailable` is reserved for a target that carries an explicit,
trusted `SaveMigrationConversionPath` for the exact source and target format
pair.  The planner does not invent adapters from platform similarity.

## Unsupported and uncertain cases

Unknown emulator wrappers, unproven byte-order/layout changes, unrecognized
containers, size changes without a proven adapter, and otherwise incomplete
format evidence produce `ConversionUnknown`, `ConversionRequired`, or
`Unsupported` according to the evidence available.  No folklore rule is
encoded as a conversion algorithm.

## Future conversion-engine boundary

A future conversion engine may consume a `ConversionAvailable` plan only after
its own reviewed adapter proves exact input identity, output representation,
and post-conversion verification.  That engine must be a separate capability;
this planner intentionally has no Apply API and cannot mutate a save.

## Research provenance

Save File Converter and similar external projects informed the distinction
between raw saves, emulator wrappers, memory-card containers, and uncertain
format families.  They are research references only; no external project,
runtime tool, network lookup, or download is a dependency of the planner.
