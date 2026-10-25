# EmuWiz mod package conflict preview

## Scope

This feature is local-only and read-only until the user explicitly confirms an
existing shared transaction plan. It does not download packages, contact mod
sites, execute installers or scripts, patch binaries, or alter the selected
game during inspection.

## Existing safety primitives reused

`archive_mod_package` already performs bounded folder/ZIP inspection, hashes
payload files, refuses traversal, rooted paths, symlink/special entries,
malformed ZIPs and unsafe destinations, and records whether a destination is
missing, regular, a directory, special, or unavailable. Its
`build_archive_mod_package_transaction_plan` rechecks the package fingerprint
before handing the operation to the shared backup/history/rollback machinery.
The manifest-based `mod_package` path remains unchanged.

The new `mod_package_preview` module is a presentation projection over those
facts. It adds stable typed readiness, risk, entry, and conflict records and
compares multiple inspected packages without choosing load order.

## Package formats

Local directories and ZIP files are supported by the existing bounded
inspector. No new archive dependency was added. 7z/RAR are not claimed. A
directory containing a manifest continues through the established manifest
workflow; the new GUI action is for an ordinary folder/ZIP payload preview.

## Target and preservation policy

The selected game’s verified identity is authoritative. Names and filenames
cannot make a package apply-ready. A replacement is surfaced as
`ModVsOriginal`, and the shared transaction is responsible for the backup.
Without an original hash, the preview does not claim that the current file is
unmodified. Source package bytes are never changed and the plan retains a
fingerprint for stale-plan detection.

## Conflicts and scripts

The projection reports `ModVsModSameContent` and
`ModVsModDifferentContent`, plus case-only collisions. Different content does
not produce an implicit winner; explicit ordering remains the responsibility
of the existing mod-stack model. Installer/script extensions are displayed as
`ExecutableInstaller` warnings and are never executed.

Unsafe paths, unknown destinations, and unproven targets are typed refusals or
review states. Preview removal does not delete or move the selected package.

## GUI

GUI-v2 Mods → Available packages now offers a local ZIP/folder preview in
addition to the existing manifest package flow. It shows target, count,
per-file Add/Replace/Conflict/Refuse operations, warnings, destination root,
and the no-execution/source-unchanged guarantees. Apply remains behind the
existing explicit confirmation and shared transaction/undo flow.

## Limits

This pass does not add an installer, binary patcher, downloader, archive format
decoder, automatic mod ordering policy, or an original-file database. Exact
“already modified” classification requires a trusted original hash supplied by
a future platform adapter; an ordinary package with an existing destination is
therefore conservatively presented as a replacement requiring backup/review.
