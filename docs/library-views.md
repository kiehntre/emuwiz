# Library organisation and projections

## CURRENT BEHAVIOR

EmuWiz has several related but distinct outputs:

- **Library View:** a named, managed local symlink layout over catalogue items.
  Preview plans changes; apply/repair creates or fixes only manifest-owned
  symlinks; remove is bounded by that manifest.
- **Playing Library / 1G1R:** evidence-backed planning and selection of one
  preferred playable entry per release/group. Its plan can be previewed and,
  where the operation is approved, applied through the shared transaction
  and history model.
- **RomM projection:** local-path-aware mapping/import/reporting against RomM.
  It is a projection of local identity and does not make RomM the identity
  authority.
- **ES-DE export:** a launch-facing metadata/path projection. It does not
  rewrite source media.

All of these consume identity evidence. Unknown or weakly identified items
remain visible as unresolved or skipped rather than being silently renamed.

## Managed Library Views

Views are configured with a destination root, source/platform filters, and the
current supported layout. Planning is read-only and classifies create,
already-correct, repair, stale removal, collision, and safety-skip outcomes.
Apply and repair create directories/symlinks only within the approved root and
only for manifest-owned paths. They never move, copy, extract, mount, or edit a
source archive.

The exact CLI surface is view list, preview, apply, repair, and remove; use
the command help for current flags. The legacy ArchiveFS config paths remain
compatibility paths.

## Historical design context

The original Stage-1 symlink view is still supported, but it is not the whole
current product. Playing Library/1G1R, RomM, and ES-DE have separate plans,
projection rules, and write boundaries.
