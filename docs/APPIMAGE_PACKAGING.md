# EmuWiz AppImage packaging

Build the V1 x86_64 AppImage from a clean Linux worktree with an approved host
`appimagetool` 1.9.1, the pinned type-2 runtime 20251108, Rust/Cargo, Bash, `install`,
`sha256sum`, and `realpath`:

```sh
CARGO_BUILD_JOBS=2 packaging/appimage/build-appimage.sh \
  --appimagetool /absolute/path/to/appimagetool \
  --runtime-file /absolute/path/to/runtime-x86_64 \
  --output-dir "$PWD/dist"
```

The output is `EmuWiz-x86_64.AppImage` and its `.sha256` companion. The
existing tarball plus `install.sh` release path remains supported and is not
replaced.

## Packaging-tool provenance

The canonical x86_64 inputs are recorded in
[`packaging/appimage/tooling.lock`](../packaging/appimage/tooling.lock):

- `appimagetool` 1.9.1 from the official AppImage release, SHA-256
  `ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0`.
- type-2 runtime 20251108 from the official AppImage release, SHA-256
  `2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d`.

Both use immutable tagged release URLs, never `latest` or `continuous`. The
hashes were independently computed from the downloaded official assets. Print
the exact provenance used by the builder with:

```sh
packaging/appimage/build-appimage.sh --print-tooling
```

The preferred local cache is
`$XDG_CACHE_HOME/emuwiz/appimage-tools` (or
`$HOME/.cache/emuwiz/appimage-tools`) with `appimagetool-x86_64.AppImage` and
`runtime-x86_64`. The cache is not committed. Manual restoration uses the URLs
in `tooling.lock`; either cached file may be used automatically or supplied
explicitly with `--appimagetool`/`--runtime-file`. Supplied or cached files are refused unless their SHA-256
matches the lock file. Future pin changes require a deliberate lock-file
update, independent hash verification, and QA review.

The AppDir contains only `emuwiz`, the `emuwiz-cli` support companion, desktop
metadata, and EmuWiz icons. It intentionally contains no graphics/display
libraries, FUSE tools, `ratarmount`, `7z`, emulator binaries, or emulator
AppImages. EmuWiz therefore continues to use the host GPU/display stack and
the user's existing emulator and archive-mount tooling.

AppImage's own runtime mount is distinct from EmuWiz archive mounting:
`ratarmount`, FUSE kernel support, and `fusermount3`/an unmount fallback still
need to be present on the host for archive mounting. On systems without the
AppImage runtime's legacy FUSE compatibility library, use the supported
extract-and-run fallback:

```sh
APPIMAGE_EXTRACT_AND_RUN=1 ./EmuWiz-x86_64.AppImage
```

`AppRun` only executes the bundled GUI; it does not rewrite HOME, XDG paths,
PATH, or graphics-library paths. The package remains unsandboxed, so normal
access to arbitrary user-selected folders, `/mnt`, removable media, NAS mounts,
RomM/ES-DE destinations, and emulator configuration remains available. Normal
EmuWiz config/data resolution remains under the user's home directory (with
the existing legacy ArchiveFS compatibility fallback), never in AppDir.

The V1 update policy is manual download-and-replace. There is no updater,
zsync, AppImageUpdate integration, or background update service.

Validate a produced artifact without opening the GUI:

```sh
packaging/appimage/verify-appimage.sh \
  --checksum dist/EmuWiz-x86_64.AppImage.sha256 \
  dist/EmuWiz-x86_64.AppImage
```

The builder uses both `appimagetool` and its type-2 runtime as explicit host
build dependencies rather than downloading or vendoring executable packaging
inputs. It may use the documented cache or explicitly supplied paths, but never
a different or unpinned tool.
