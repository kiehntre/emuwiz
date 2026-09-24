# Emulator lifecycle health

`archivefs_core::emulator_lifecycle` is a read-only composition of existing
EmuWiz evidence. It keeps exact launch bindings, selected and unselected
installations, local health, update authority, update status, provenance, and
existing launch readiness together. It does not install, adopt, update, or
rewrite emulator configuration.

## Trust and ownership

Inventory, emulator profiles, managed-install manifests, adapter readiness,
and update metadata remain authoritative in their existing domains. Lifecycle
health only joins those results. A Flatpak app is represented by its exact app
ID, not a pretend executable path. A managed AppImage is trusted only through
the existing manifest/hash validation. An external AppImage, PATH executable,
or familiar filename is user-managed or unknown until stronger evidence proves
otherwise.

Flatpak and system-package providers are read-only. They report who owns an
installation; they never run `flatpak update`, `apt`, `dnf`, `pacman`, `sudo`,
or another package mutation. Offline status affects remote freshness only:
local existence, selected binding, ownership, managed health, and existing
readiness remain usable.

## Multiple installations

All viable candidates are retained. An explicit existing selection is matched
against the exact binding. If it disappears, the projection reports a stale
selection/broken state and does not switch to another candidate. With multiple
candidates and no explicit selection, the state is `MultipleInstallations`.

## Supported provider identities

The reviewed Flatpak identity table covers RetroArch, PCSX2, Dolphin, Flycast,
xemu, PPSSPP, DuckStation, RPCS3, and Cemu. The provider-neutral ID list also
leaves room for MAME, FBNeo, Hatari, FS-UAE, Vita3K, Ryujinx, and Xenia. The
latter do not acquire an invented update authority: Ryujinx remains external
with no official updater, and Xenia remains `DoNotAutomate` at the future
provider layer unless official Linux evidence is established.

The all-emulator inspection is intentionally bounded to existing inventory,
managed roots, configured selections, reviewed Flatpak IDs, and exact package
ownership queries. It never recursively scans a home directory or system
tree.
