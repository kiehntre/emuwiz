# Virtual Boy backend support

EmuWiz recognises Virtual Boy platform context and the `.vb` and `.vboy`
cartridge-file extensions. Those extensions are family-level media evidence;
ordinary Virtual Boy ROMs do not provide a standard embedded title or release
identity that EmuWiz can safely invent.

Exact game identity therefore remains hash/DAT-driven. When the platform is
trusted by the scanner or catalogue and a direct ROM is readable, EmuWiz can
use its bounded full-file SHA-256 as the opaque identity key, through the same
existing loose-ROM identity path used by other cartridge platforms. A filename
or extension alone never becomes exact identity.

RetroArch candidate generation recognises the reviewed Beetle VB and Mednafen
VB core names from their own `.info` metadata. The normal launch planner still
requires a resolved platform, verified identity key, valid runnable content
path, and an installed matching core. If any of those are missing, planning
fails closed. No standalone Virtual Boy adapter is claimed.

This backend is read-only: it does not modify ROMs, download cores, alter
RetroArch configuration, or emulate/parse undocumented cartridge metadata.
