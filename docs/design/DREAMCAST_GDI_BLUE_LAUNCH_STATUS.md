# Dreamcast GDI native launch readiness

Blue uses the existing Dreamcast IP.BIN identity inspector, strict GDI parser,
launch planner, firmware verifier and Flycast command/execution backend. It does
not convert, reconstruct or write game media, and does not expand DCP Apply.

The supported new action is a direct, structurally valid multi-track GDI through
native Linux Flycast using the inherited absolute XDG config/data roots. The
selected profile and executable must match exactly. A regular native ELF is
required; scripts, symlinks, AppImages, portable, explicit and Flatpak profiles
are refused. The installed executable is discovered through the existing
Flycast infrastructure; EmuWiz does not execute it to establish a version.

Read-only inspection runs in a selection-bound worker. Both Blue views expose
Refresh Dreamcast readiness. Existing identity conflicts and unknown identity
are preserved. The existing planner owns candidate preference and selection. The new optical
content projection authorizes only the stamped Flycast action; it cannot
unlock an unstamped RetroArch or other launch adapter.
A candidate needs strict Ready status, the existing recognized dc_boot.bin
firmware evidence, and a successful preflight. A BIOS name or file presence
alone cannot make the action ready. The native BIOS inspection root is corrected
to XDG_DATA_HOME/flycast; other installation types retain their existing models.

A preview lasts 30 seconds. It retains the parsed descriptor and every original
track reference plus device, inode, size, modification time, Linux change
time and a full-byte SHA256 freshness proof. Metadata alone is insufficient: a
rapid same-size edit with restored mtime reproduced identical ctime on the test
filesystem. Existing bounded identity hashing supplies the content proof. Original-path symlinks, missing files, ambiguous high-density tracks and
unsupported track layouts are refused. The native reader additionally requires
a descriptor below 16 KiB, at least three tracks in declaration order, and a
data/audio/data prefix with track 3 at LBA 45,000. Native-incompatible text forms
are refused; quoted short and Unicode filenames remain supported.
Settings, executable and boot ROM are
stamped too. Launch compares those facts before and after the existing fresh
Flycast preflight and immediately before handing the command to its executor.
Changed tracks are refused even when IP.BIN product-code identity is unchanged.
The whole GDI read budget uses the existing 2 GiB GD-ROM ceiling; profile files
have a separate 256 MiB budget. Reads and rechecks run only on workers. Slow
media can exhaust the 30-second lifetime and is refused rather than launched
with an expired preview. These checks do not lock media against another writer
during emulator runtime.

Product-code identity and SHA256 agreement with Preview are not trusted catalogue verification,
ROM completeness or proof that a game boots. Existing CHD inspection is unchanged;
this new Blue action supports GDI only. Configured nonempty Dreamcast.BiosPath
and unmodeled HLE selections are refused. An exactly empty native default BIOS
path remains supported. Flycast itself can select HLE through built-in game
rules; recognized firmware availability does not promise that it will execute.
A future user-authorized launch has Flycast's normal config/save behavior.
No real emulator launch is part of validation.

The bounded upstream contract was checked against Flycast commit
[6bb0fc3e18dac4b01694f488c7a71f5930e8a648](https://github.com/flyinghead/flycast/tree/6bb0fc3e18dac4b01694f488c7a71f5930e8a648):
`core/linux-dist/main.cpp`, `core/stdclass.cpp`, `core/cfg/cl.cpp`,
`core/cfg/ini.cpp`, `core/cfg/option.h`, `core/cfg/option.cpp`,
`core/oslib/oslib.cpp`, and `core/hw/flashrom/nvmem.cpp`.
