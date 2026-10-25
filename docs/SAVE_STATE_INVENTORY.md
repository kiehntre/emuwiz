# Provider-neutral save and state inventory

`archivefs_core::persistent_state_inventory::inventory_persistent_state` is a
read-only projection of emulator persistent state. The caller supplies the
effective path discovered from an emulator/profile configuration; the module
does not invent XDG, Flatpak, portable, or home-directory defaults.

The result distinguishes `NativeSave`, `MemoryCard`, `SaveState`,
`NandOrVirtualDisk`, `ConfigBoundState`, `CloudManaged`, and `Unknown`, and
classifies portability as `SafeToCopy`, `CopyWithMetadata`, `VersionBound`,
`EmulatorBound`, `NeedsReview`, or `DoNotTouch`.

PS1/PS2 card paths compose the existing bounded memory-card inspector. The
card remains a container record; individual entries are projected only when
that parser provides an observation. RPCS3 `dev_hdd0`, Cemu `mlc01`, Dolphin
Wii NAND, and xemu HDD/NAND roots remain one opaque system-container record.

All records retain the selected installation, version, profile, firmware
context, effective-path origin, bounded SHA-256 evidence where practical, and
existing `IdentityEvidence` values. Filename-derived IDs remain candidate
review evidence and never become exact identity. Savestates are always
reported as emulator-bound with a version warning.

The API performs no copy, move, restore, conversion, deletion, cleanup, or
cloud operation. Missing roots and unsafe symlink roots are reported as
diagnostics without creating anything.
