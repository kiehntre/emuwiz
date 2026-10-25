# PCSX2 inspection adapter and apply boundary

## CURRENT BEHAVIOR

The PCSX2 inspection adapter discovers local profiles and inventories PNACH
files without starting PCSX2 or interpreting patch directives. Discovery,
inventory, provider browsing, and preview are read-only.

Identity matching requires separately verified PS2 evidence, including the
serial and, where required by the provider, executable CRC. Filenames,
comments, and PNACH names are observations only. The adapter reports unsafe,
ambiguous, missing, or changed profiles and does not create a missing profile
as a side effect.

The separate PCSX2 install-plan path can materialize selected verified cheat
records and pass them to the shared apply/journal/rollback engine. It
revalidates the exact profile, identity, source, and destination and requires
explicit confirmation. It does not make arbitrary PNACH editing or directive
execution safe.

The filename READONLY_ADAPTER is historical: it names the inspection adapter
and its no-mutation inventory contract, not the complete current PCSX2
Cheats & Mods capability.

See [SHARED_CHEAT_PREVIEW.md](SHARED_CHEAT_PREVIEW.md) and
[SHARED_SAFE_APPLY_ROLLBACK.md](SHARED_SAFE_APPLY_ROLLBACK.md).
