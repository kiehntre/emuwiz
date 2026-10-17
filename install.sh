#!/bin/sh
# EmuWiz installer.
#
# Installs emuwiz-cli and emuwiz for the current user only: no
# sudo, no system-wide changes, no edits to shell startup files. Safe to
# run more than once - re-running reinstalls the binaries and never
# touches an existing config.
#
# Works from two locations (detected automatically):
#   - an extracted release bundle (bin/emuwiz-cli and bin/emuwiz)
#   - a workspace checkout after `cargo build --workspace --release`
#     (target/release/emuwiz-cli / target/release/emuwiz)
#
# Linux only: safe asset publication requires Python 3, procfs, O_PATH
# (Linux 2.6.39+), and descriptor-relative hardlink support on the install
# filesystem. Missing support fails closed; there is no overwrite fallback.
# The starter config (written only when none exists) also needs Python's
# os.O_TMPFILE and ctypes, a configuration-directory filesystem that supports
# O_TMPFILE anonymous files and hardlinks, and a mounted /proc: the anonymous
# file is published with linkat() through /proc/self/fd. It is created with mode
# 0600, not the umask-dependent default earlier versions produced, and an
# existing config is never modified or re-permissioned. When any of that is
# unavailable the step refuses with an explicit message (no traceback, no named
# staging file, no fallback), assets already installed remain in place, and the
# run exits 1. See "Installer requirements and failure behaviour" in
# docs/RELEASE_ENGINEERING.md.
# Close native EmuWiz processes before upgrading: owned regular files update
# through held descriptors, non-atomically, just like the ownership manifest.
# Hardlinked files refuse changed-byte updates. A running EmuWiz is detected before
# anything is published (see preflight_binary_slot) and asks the user to close it.
#
# UPGRADE PRESERVATION
#
# Because that in-place write is not atomic, an owned binary whose bytes will
# change is first copied, before the first publication of the run, to
# $XDG_DATA_HOME/emuwiz-installer/previous/<slot>.<old sha256>: written under
# a fresh name, fsynced, re-hashed, then linked into place without replacing
# anything. A line naming the slot, old digest, new digest and destination is
# appended to emuwiz-installer/upgrade-pending and fsynced. publish refuses to
# write an owned binary unless that verified copy and journal line exist.
# The journal is removed only after the new manifest is written. A later run
# that finds it checks each destination: one back at the old digest is
# resolved; anything else (partly written, or new but unrecorded) stops the
# run before anything is read or published and prints the exact cp command
# that restores the kept copy. The installer never restores automatically and
# never deletes kept copies. Not enough free space (copy plus a 16 MiB margin,
# and room for a larger binary), no fsync, an unexpected file at the kept name
# or a shared "previous" directory refuse with nothing installed. Desktop
# entries and icons are regenerated from the bundle and are not kept.
#
# OWNERSHIP TRACKING
#
# Every path this script writes (binaries, legacy aliases, the desktop
# entry, hicolor icons) is recorded in a small manifest under
# $XDG_DATA_HOME/emuwiz-installer/manifest after a successful install. On
# every later run - reinstall or uninstall - that manifest is what proves a
# given path is still the exact thing this installer put there, rather than
# trusting the filename, its timestamp, or its inode alone. A path occupied
# by something else (a foreign file, a foreign symlink, a directory) is
# never silently overwritten or removed; it is left alone with a clear
# warning. See the "OWNERSHIP MODEL" comment block below for the full
# design.
set -eu

program_name="install.sh"
default_bin_dir="$HOME/.local/bin"
if [ -n "${XDG_DATA_HOME:-}" ]; then
    case "$XDG_DATA_HOME" in
        /*) data_home="$XDG_DATA_HOME" ;;
        *)
            printf '%s: warning: ignoring relative XDG_DATA_HOME; using %s/.local/share\n' \
                "$program_name" "$HOME" >&2
            data_home="$HOME/.local/share"
            ;;
    esac
else
    data_home="$HOME/.local/share"
fi
desktop_id="io.github.kiehntre.emuwiz"
desktop_file="$data_home/applications/$desktop_id.desktop"
# EmuWiz config directory. This mirrors the application's own resolution
# (archivefs-core app_dirs::config_dir) exactly:
#   1. EMUWIZ_CONFIG_HOME, when set, is the config directory itself;
#   2. else XDG_CONFIG_HOME, when set, gives $XDG_CONFIG_HOME/emuwiz;
#   3. else ~/.config/emuwiz, or the legacy ~/.config/archivefs when only
#      that exists. Any directory entry, including a dangling symlink,
#      counts as present, so a broken emuwiz entry never silently switches
#      the profile to the legacy one.
# Explicit and XDG roots never fall back to the legacy directory. A set but
# non-absolute override (the application refuses it too) leaves the config
# location unresolved: the binaries are still installed, no starter config
# is written, and the run exits non-zero with a clear message.
config_root=""
config_unresolved_reason=""
if [ "${EMUWIZ_CONFIG_HOME+set}" = set ]; then
    case "$EMUWIZ_CONFIG_HOME" in
        /*) config_root="$EMUWIZ_CONFIG_HOME" ;;
        *) config_unresolved_reason="EMUWIZ_CONFIG_HOME is not an absolute path" ;;
    esac
fi
if [ "${XDG_CONFIG_HOME+set}" = set ]; then
    case "$XDG_CONFIG_HOME" in
        /*)
            if [ "${EMUWIZ_CONFIG_HOME+set}" != set ]; then
                config_root="$XDG_CONFIG_HOME/emuwiz"
            fi
            ;;
        *) config_unresolved_reason="XDG_CONFIG_HOME is not an absolute path" ;;
    esac
fi
if [ "${EMUWIZ_CONFIG_HOME+set}" != set ] && [ "${XDG_CONFIG_HOME+set}" != set ]; then
    config_root="$HOME/.config/emuwiz"
    if [ ! -e "$config_root" ] && [ ! -L "$config_root" ] \
        && { [ -e "$HOME/.config/archivefs" ] || [ -L "$HOME/.config/archivefs" ]; }; then
        config_root="$HOME/.config/archivefs"
    fi
fi
if [ -n "$config_unresolved_reason" ]; then
    config_root=""
fi
config_dir="$config_root"
config_file="$config_dir/config.toml"
if [ -z "$config_root" ]; then
    config_dir="(unresolved configuration directory)"
    config_file="(unresolved configuration directory)/config.toml"
fi
config_step_failed=0

# --------------------------------------------------------------------------
# OWNERSHIP MODEL
#
# The manifest lives at $data_home/emuwiz-installer/manifest (never inside
# $data_home/emuwiz, which is the *application's* data directory, kept
# strictly separate so uninstall never has to reach into user data to clean
# up its own bookkeeping). It is a flat, line-oriented, non-executable text
# file - never sourced or eval'd as shell - with a fixed header (schema_version,
# bin_dir, data_home, record_count) followed by exactly record_count lines,
# one per tracked path, keyed by a fixed, closed set of slot names, and
# terminated by a literal "end" line:
#
#   bin-emuwiz-cli, bin-emuwiz                              (regular files)
#   alias-archivefs-cli, alias-emuwiz-gui, alias-archivefs-gui  (symlinks)
#   desktop                                                 (regular file)
#   icon-32, icon-64, icon-128, icon-256, icon-512          (regular files)
#
# A file slot line is "<slot> file <sha256-hex> <size-bytes>": the SHA-256
# digest of the installed file's exact byte content is the sole ownership
# proof - size is recorded only as supplemental metadata for diagnostics
# and is never consulted by any ownership decision. A symlink slot line is
# "<slot> symlink <target>": the target is the symlink's literal, raw
# readlink() value, which this installer only ever sets to one of two fixed
# values.
#
# CONTENT, not mtime, not inode. mtime was the original design and was
# rejected after a hostile review reproduced two real failures: `touch -r`
# from a genuinely owned file, or `cp -p`/`touch -d` to the installer's own
# recorded timestamp, let a byte-for-byte DIFFERENT foreign file pass the
# ownership check purely by forging the one signal being trusted. Inode
# number was tried before that and rejected too - empirical testing on
# plain ext4 showed a freed inode gets reused by the very next allocation
# with no attacker involved at all, so "the inode still matches" was
# already a false positive waiting to happen even without a forged
# timestamp. A cryptographic digest of the actual bytes has no equivalent
# forgery surface within this installer's threat model (no sudo, current
# user only, no chosen-prefix-collision attack against SHA-256 itself) -
# forging it requires reproducing the exact content, which is precisely
# what "provably the same file" means.
#
# Critically: install.sh NEVER reads a path out of the manifest. Every path
# it might create, verify or remove is always the one it independently
# computes right now from $bin_dir/$data_home/$desktop_id - the same way it
# always has. The manifest is consulted only as a slot-name-keyed lookup
# table of "kind + digest [+ size]", strictly validated against the shapes
# above on every read; anything that doesn't parse as one of those exact
# shapes invalidates the WHOLE manifest (see the parser rules below), never
# just the one bad line. A corrupted, truncated or hand-edited manifest can
# therefore never cause install.sh to touch a path other than the fixed set
# it was already going to consider - at worst it makes an owned path look
# unowned, which only ever means "ask again" (fail safe), never "delete
# something else instead".
#
# PARSER: fail-closed, not best-effort. A manifest is accepted only if, in
# one pass:
#   - exactly one schema_version line, whose value is the current schema
#   - exactly one bin_dir line, matching this invocation's resolved bin_dir
#   - exactly one data_home line, matching this invocation's data_home
#   - exactly one record_count line, a plain non-negative integer
#   - zero or more slot record lines, each for one of the fixed slot names
#     above, each appearing at most once (a second line for a slot already
#     seen invalidates the whole manifest - never "last one wins")
#   - a slot record's fields are validated strictly: a file slot's digest
#     must be exactly 64 lowercase hex characters and its size a plain
#     digit string; a symlink slot's target must be exactly one of the
#     finite set of targets this installer ever writes. Anything else
#     invalidates the whole manifest.
#   - no line of any other shape (an unrecognised key, a duplicate header,
#     anything appearing after the end marker) - the whole manifest is
#     invalidated, not just that line
#   - a single literal "end" line, exactly once, as the last line - nothing
#     may follow it, and a manifest is never accepted without one
#   - the number of slot record lines actually present exactly equals the
#     declared record_count
# Both record_count AND the "end" marker are required (belt and braces):
# either one alone already rules out "valid header followed by truncation"
# passing as complete, since a truncated file can supply at most one of
# "the declared count matches" or "the file has a terminator" but never a
# consistent forgery of both without simply being the real, untruncated
# file. A manifest that fails ANY of these checks is treated exactly like
# no manifest existing at all - every slot then falls back to the
# per-asset-type recognition rules below.
#
# Intentionally partial manifests ARE part of the design: a run that left
# some destinations foreign (no --replace-foreign) still writes a manifest
# recording only the slots it actually installed - record_count reflects
# that smaller number, not 11. A slot with no record in an otherwise valid,
# accepted manifest simply falls back to the same per-asset-type
# recognition rule as "no manifest at all", scoped to just that slot.
#
# No manifest, or a record for a bin_dir/data_home this run doesn't match,
# or no record for a given slot: that slot falls back to a per-asset-type
# recognition rule instead of blind trust - and the rule is only ever
# "recognise it" where that is actually provable, never "assume it because
# the name matches":
#   - binaries (bin-emuwiz-cli, bin-emuwiz): no recognition rule at all.
#     Binary *content* legitimately differs release to release, so there is
#     no byte pattern - or anything else - that safely proves an existing
#     file is this installer's own without a manifest entry to check
#     against. An unrecorded binary path is always foreign. The practical
#     effect: the first run of this installer version against an install
#     that predates it treats its own existing binaries as foreign and asks
#     for --replace-foreign once; every run after that has a manifest and
#     needs nothing special. A one-time migration step, not a permanent
#     regression - and the only honest answer once "adopt any executable
#     file at this name" is off the table as unprovable.
#   - aliases (alias-*): an existing symlink whose target is exactly the
#     one value this installer would itself write ("emuwiz-cli" or
#     "emuwiz") is adopted. This is close to proof, not a guess - a
#     coincidentally-matching relative symlink target at this exact path is
#     implausible, and unlike a binary's bytes, this target string is fixed
#     content this installer has always written verbatim.
#   - desktop entry: adopted only if it is byte-identical to what this run
#     would render, ignoring the "Exec=" line specifically (which
#     legitimately varies with --prefix across installs). Real proof, not a
#     heuristic.
#   - icons: adopted only if byte-identical to the exact approved source
#     asset for that size. Real proof.
# Anything that fails its recognition rule - including every case a
# directory or a foreign symlink occupies the path - is foreign: never
# overwritten or removed without the caller opting in (--replace-foreign
# for install; uninstall never overwrites/removes a foreign path at all).
#
# UPGRADE ORDERING: a legitimate reinstall over a newer release's binaries
# gates on the digest currently sitting at the destination - the OLD
# installed content - against the OLD recorded manifest digest, strictly
# BEFORE that destination is overwritten with the new bundled asset. Only
# once that gate passes does the copy happen, and only after the copy
# succeeds is the NEW digest computed and recorded. The new bundled asset's
# own digest never enters the ownership decision for what it is about to
# replace - comparing "what's already installed" against "what we're about
# to install" would make every version bump look foreign (the bytes always
# differ) and would also make an unrelated foreign file that happens to
# match some OTHER version's digest look owned. Both directions are wrong;
# only "does the CURRENT installed content match the LAST thing we
# ourselves recorded" is the right question.
#
# Publication remains nontransactional; no asset rollback is attempted. Each
# managed publication rechecks the bookkeeping binding. New metadata is linked
# into an absent name without clobbering; existing metadata is written through
# its validated descriptor (not a rename over a possibly replaced pathname).
# Descriptor writes are not atomic: interrupted writes may leave a truncated
# owned manifest, which fails closed on the next run. An external late change
# stops further publication; assets already published may remain unrecorded.
# The directory lock serializes cooperating installers and uninstallers. It
# does not prohibit another same-user process from editing these writable paths.
# Assets use the same pinning model: absent names are linked without clobbering
# from held staged inodes; verified regular files update through held inodes;
# fixed aliases are validated and retained. Each operation is relative to a
# pinned parent directory and checks its binding before and after publication.
# Asset records describe the held published bytes/target, never a fresh read
# that could adopt a substituted foreign object after publication.
#
# MANIFEST-DIRECTORY SAFETY: $data_home itself - wherever XDG_DATA_HOME
# resolves to, symlinked or not - is trusted exactly as much as it already
# was for every other asset this installer writes there (the desktop entry,
# the icons); a symlinked XDG_DATA_HOME pointing at a different disk is a
# completely ordinary, legitimate setup and nothing about ownership
# tracking treats it with any more suspicion than the rest of this script
# always has. The boundary that IS enforced is narrower and specific to
# this feature: the final "emuwiz-installer" path component, which this
# installer alone creates and fully owns, must be a real directory - never
# a symlink, and never anything else occupying that exact name. If it does
# not exist yet, install creates it fresh as a real directory, mode 0700.
# If something already occupies that name and is not a real directory,
# install refuses to proceed at all (there is nowhere left it could
# trustworthily record ownership of anything this run installs);
# uninstall, which never creates anything, simply treats it exactly like
# "no manifest" and never touches that path. Symmetrically, the manifest
# FILE itself is only ever written through its validated descriptor, or
# created without clobbering if absent. Its identity/content and the bookkeeping
# directory binding are checked before each managed publication. A pre-existing
# path there that did NOT load - a
# symlink, a directory, or simply a regular file whose content is not one
# of our own manifests - is treated exactly like any other foreign
# collision in this script: left untouched with a warning by default
# (the install is refused before any managed asset is published), or moved
# aside into a fresh backup - never silently
# written through or over - with --replace-foreign.
# --------------------------------------------------------------------------
manifest_schema_version=2
manifest_dir="$data_home/emuwiz-installer"
manifest_file="$manifest_dir/manifest"
manifest_loaded=0
manifest_input=""
replace_foreign=0

usage() {
    cat <<EOF
Usage: $program_name [--prefix PATH] [--replace-foreign] [--uninstall]
                  [--developer-unchecked-install] [--help]

Install EmuWiz (emuwiz-cli, emuwiz) for the current user.
No sudo is used and no shell startup files are modified.

Options:
  --prefix PATH      Install the binaries into PATH instead of the default
                      ($default_bin_dir). The directory is created if needed.
                      PATH should normally be an absolute path.
  --replace-foreign   Install only: when a binary, alias, desktop entry,
                      icon, or the ownership manifest itself is occupied by
                      something this installer did not put there, move it
                      aside into a freshly and securely created backup
                      directory next to it and install in its place. The
                      exact backup location is printed. Without this flag
                      such a path is left untouched and a warning is
                      printed; a foreign ownership manifest refuses the
                      install before any managed file is published.
  --uninstall         Remove EmuWiz binaries, its desktop entry and application
                      icons - but only the ones still provably EmuWiz-owned.
                      A path that was replaced by something else since it was
                      installed is left alone, with a warning. Your config at
                      $config_file is never touched.
  --developer-unchecked-install
                      Developer builds only: install binaries from a tree that
                      has no SHA256SUMS (a workspace target/release build or
                      an old bundle) without verifying them. Never needed or
                      honoured for an official release: a tree with SHA256SUMS
                      is always verified, and a tree with manifest.json but no
                      SHA256SUMS is always refused. Symlinked or non-regular
                      binaries are refused even in this mode, and a prominent
                      warning is printed. Without this flag such a tree is
                      refused with nothing installed.
  --help              Show this help and exit.

Without --uninstall, this script:
  1. Detects whether it is running from an extracted release bundle or a
     workspace checkout with release binaries already built, and fails
     with a clear message if neither is found. A release package's
     SHA256SUMS is checked in full first; any mismatch, unsafe path or
     symlink refuses with nothing installed or changed. A tree without
     SHA256SUMS is refused unless --developer-unchecked-install is given.
  2. Copies emuwiz-cli and emuwiz into the install directory
     and makes sure they are executable - unless that destination is
     occupied by something foreign (see --replace-foreign above).
  3. Installs one EmuWiz desktop launcher and its approved application icons
     below $data_home, subject to the same foreign-path protection.
     Before an EmuWiz-owned binary is upgraded, its exact previous bytes
     are kept, verified, under $manifest_dir/previous; an interrupted
     upgrade stops the next run with exact restore steps. Kept copies are
     never deleted automatically.
  4. Records what it just installed in a SHA-256-backed ownership manifest
     under $manifest_dir, so a later reinstall or uninstall can tell
     EmuWiz-owned paths apart from unrelated ones with the same name -
     never by filename, timestamp or inode alone.
  5. Creates $config_dir if it does not exist.
  6. Publishes config.toml.example as $config_file with an atomic,
     no-clobber link, only if nothing exists at that name (a dangling
     symlink counts as existing). An existing config is never overwritten
     or written through, even if it appears while the installer runs.
     The directory follows the application: EMUWIZ_CONFIG_HOME, else
     XDG_CONFIG_HOME/emuwiz, else ~/.config/emuwiz (or the legacy
     ~/.config/archivefs when only that exists).
  7. If a new config was just written and this script is running
     interactively, optionally prompts for one archive source folder to
     add via 'emuwiz-cli source add'. Leave blank to skip - source
     folders are never required at install time, and more can always be
     added later from the Sources page in the GUI or the CLI. Never
     offered for an existing config, and never offered for a
     non-interactive install.
  8. Checks whether ratarmount is on PATH and prints installation
     guidance if it is not (EmuWiz uses it to mount archives).
EOF
}

fail() {
    printf '%s: %s\n' "$program_name" "$*" >&2
    exit 1
}

warn() {
    printf '%s: warning: %s\n' "$program_name" "$*" >&2
}

# path_kind PATH - prints "symlink", "dir", "file" or "absent". Always
# checks -L first so a symlink is never misreported as whatever it points
# at (a symlink to a directory is "symlink", not "dir"; a dangling symlink
# is "symlink", not "absent").
path_kind() {
    if [ -L "$1" ]; then
        printf 'symlink\n'
    elif [ -d "$1" ]; then
        printf 'dir\n'
    elif [ -e "$1" ]; then
        printf 'file\n'
    else
        printf 'absent\n'
    fi
}

# manifest_dir_state - "dir" (safe to read from and, on install, write
# into), "absent" (nothing there yet - install may create it fresh) or
# "unsafe" (a symlink, a regular file, or anything else occupying the
# installer bookkeeping directory's path - never traversed, read through,
# or created-through). Computed once, before manifest_dir is used for
# anything at all.
manifest_dir_state() {
    case $(path_kind "$manifest_dir") in
        dir) printf 'dir\n' ;;
        absent) printf 'absent\n' ;;
        *) printf 'unsafe\n' ;;
    esac
}
manifest_dir_kind=$(manifest_dir_state)

# ensure_manifest_dir - called only from install preflight. Creates
# the bookkeeping directory fresh (mode 0700) if nothing occupies that name
# yet; refuses outright
# (fails the whole install - there is nowhere left it could trustworthily
# record anything) if something unsafe occupies that name. Deliberately
# plain mkdir, never mkdir -p: -p treats a name that already resolves to
# something (including through a symlink) as success, which is exactly the
# ambiguity this function exists to close.
ensure_manifest_dir() {
    case "$manifest_dir_kind" in
        dir)
            : # Tighten permissions only after opening and binding the directory.
            ;;
        absent)
            # $data_home itself is the same pre-existing, already-trusted
            # boundary every other asset this script writes relies on
            # (mkdir -p here is fine - it is not the hardened boundary,
            # manifest_dir's own final component below is). Only that last
            # component is ever created via a plain, non-p mkdir.
            mkdir -p -- "$data_home" ||
                fail "could not create the data directory: $data_home"
            mkdir -m 0700 -- "$manifest_dir" ||
                fail "could not create the installer bookkeeping directory: $manifest_dir"
            case $(path_kind "$manifest_dir") in
                dir) : ;;
                *) fail "installer bookkeeping directory did not come up safe after creation: $manifest_dir" ;;
            esac
            manifest_dir_kind=dir
            ;;
        unsafe)
            fail "installer bookkeeping directory exists but is not a plain directory (found a $(path_kind "$manifest_dir")): $manifest_dir - remove or rename it by hand, then re-run"
            ;;
    esac
}

# file_digest PATH - prints the SHA-256 digest of PATH's exact byte content
# as 64 lowercase hex characters. Tries sha256sum, then shasum -a 256, then
# openssl dgst -sha256 -r (all three emit "<hex>  <name>"-shaped output, so
# only the first field is ever taken). Prints nothing and returns non-zero
# if none of those tools is available or the read fails.
file_digest() {
    path=$1
    line=""
    if command -v sha256sum >/dev/null 2>&1; then
        line=$(sha256sum -- "$path" 2>/dev/null) || return 1
    elif command -v shasum >/dev/null 2>&1; then
        line=$(shasum -a 256 -- "$path" 2>/dev/null) || return 1
    elif command -v openssl >/dev/null 2>&1; then
        line=$(openssl dgst -sha256 -r -- "$path" 2>/dev/null) || return 1
    else
        return 1
    fi
    [ -n "$line" ] || return 1
    set -- $line
    printf '%s\n' "$1"
}

# validate_file_digest FP - true only for a string of exactly 64 lowercase
# hex characters and nothing else.
validate_file_digest() {
    fp=$1
    [ "${#fp}" -eq 64 ] || return 1
    case "$fp" in
        *[!0-9a-f]*) return 1 ;;
    esac
    return 0
}

# validate_size N - true only for a plain non-negative integer.
validate_size() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}

# validate_symlink_fp FP - true only for the exact, finite set of symlink
# targets this installer ever writes.
validate_symlink_fp() {
    case "$1" in
        emuwiz-cli|emuwiz) return 0 ;;
        *) return 1 ;;
    esac
}

# Manifest slot storage: one set of plain (non-indirect) variables per
# fixed slot name, populated by load_manifest and read directly by the gate
# functions below via manifest_get_slot's literal case match. No eval
# anywhere in the manifest read/write path. slot_*_size is supplemental
# only (never read by any gate function); kept for parity with the on-disk
# record and possible future diagnostics.
slot_bin_emuwiz_cli_kind=""; slot_bin_emuwiz_cli_fp=""; slot_bin_emuwiz_cli_size=""
slot_bin_emuwiz_kind="";     slot_bin_emuwiz_fp="";     slot_bin_emuwiz_size=""
slot_alias_archivefs_cli_kind=""; slot_alias_archivefs_cli_fp=""
slot_alias_emuwiz_gui_kind="";    slot_alias_emuwiz_gui_fp=""
slot_alias_archivefs_gui_kind=""; slot_alias_archivefs_gui_fp=""
slot_desktop_kind=""; slot_desktop_fp=""; slot_desktop_size=""
slot_icon_32_kind="";  slot_icon_32_fp="";  slot_icon_32_size=""
slot_icon_64_kind="";  slot_icon_64_fp="";  slot_icon_64_size=""
slot_icon_128_kind=""; slot_icon_128_fp=""; slot_icon_128_size=""
slot_icon_256_kind=""; slot_icon_256_fp=""; slot_icon_256_size=""
slot_icon_512_kind=""; slot_icon_512_fp=""; slot_icon_512_size=""

# store_file_slot SLOT DIGEST SIZE - records a file slot's digest/size, but
# only if this slot has not already been seen in this parse (a second
# record for the same slot invalidates the whole manifest via parse_ok,
# never "last one wins"). Literal case match only - no eval, no
# indirection.
store_file_slot() {
    case "$1" in
        bin-emuwiz-cli)
            [ -z "$slot_bin_emuwiz_cli_kind" ] || { parse_ok=0; return 0; }
            slot_bin_emuwiz_cli_kind=file; slot_bin_emuwiz_cli_fp=$2; slot_bin_emuwiz_cli_size=$3 ;;
        bin-emuwiz)
            [ -z "$slot_bin_emuwiz_kind" ] || { parse_ok=0; return 0; }
            slot_bin_emuwiz_kind=file; slot_bin_emuwiz_fp=$2; slot_bin_emuwiz_size=$3 ;;
        desktop)
            [ -z "$slot_desktop_kind" ] || { parse_ok=0; return 0; }
            slot_desktop_kind=file; slot_desktop_fp=$2; slot_desktop_size=$3 ;;
        icon-32)
            [ -z "$slot_icon_32_kind" ] || { parse_ok=0; return 0; }
            slot_icon_32_kind=file; slot_icon_32_fp=$2; slot_icon_32_size=$3 ;;
        icon-64)
            [ -z "$slot_icon_64_kind" ] || { parse_ok=0; return 0; }
            slot_icon_64_kind=file; slot_icon_64_fp=$2; slot_icon_64_size=$3 ;;
        icon-128)
            [ -z "$slot_icon_128_kind" ] || { parse_ok=0; return 0; }
            slot_icon_128_kind=file; slot_icon_128_fp=$2; slot_icon_128_size=$3 ;;
        icon-256)
            [ -z "$slot_icon_256_kind" ] || { parse_ok=0; return 0; }
            slot_icon_256_kind=file; slot_icon_256_fp=$2; slot_icon_256_size=$3 ;;
        icon-512)
            [ -z "$slot_icon_512_kind" ] || { parse_ok=0; return 0; }
            slot_icon_512_kind=file; slot_icon_512_fp=$2; slot_icon_512_size=$3 ;;
    esac
    records_seen=$((records_seen + 1))
}

# store_symlink_slot SLOT TARGET - same duplicate-rejection discipline as
# store_file_slot, for the three symlink alias slots.
store_symlink_slot() {
    case "$1" in
        alias-archivefs-cli)
            [ -z "$slot_alias_archivefs_cli_kind" ] || { parse_ok=0; return 0; }
            slot_alias_archivefs_cli_kind=symlink; slot_alias_archivefs_cli_fp=$2 ;;
        alias-emuwiz-gui)
            [ -z "$slot_alias_emuwiz_gui_kind" ] || { parse_ok=0; return 0; }
            slot_alias_emuwiz_gui_kind=symlink; slot_alias_emuwiz_gui_fp=$2 ;;
        alias-archivefs-gui)
            [ -z "$slot_alias_archivefs_gui_kind" ] || { parse_ok=0; return 0; }
            slot_alias_archivefs_gui_kind=symlink; slot_alias_archivefs_gui_fp=$2 ;;
    esac
    records_seen=$((records_seen + 1))
}

# load_manifest - populates manifest_loaded=1, and the slot_* variables
# above, only from a manifest that passes every rule in the "PARSER"
# section of the OWNERSHIP MODEL comment above: single, matching headers;
# a declared record_count that exactly equals the number of valid,
# non-duplicate slot records actually found; a mandatory "end" line with
# nothing after it; and no line of any other shape anywhere. Any failure
# leaves manifest_loaded=0 and every slot_* empty - callers then use the
# no-manifest recognition rules for everything, never a partially-trusted
# mix, and never infer completeness from merely reaching EOF.
load_manifest() {
    manifest_loaded=0
    [ "$manifest_dir_kind" = dir ] || return 0
    [ -f "$manifest_file" ] || return 0
    [ ! -L "$manifest_file" ] || return 0
    # Shell read can discard NUL bytes. Reject them before parsing text records.
    LC_ALL=C tr -d '\000' <"${manifest_input:-$manifest_file}" \
        | cmp -s - "${manifest_input:-$manifest_file}" || return 0

    manifest_version=""
    manifest_saved_bin_dir=""
    manifest_saved_data_home=""
    manifest_declared_count=""
    seen_schema=0
    seen_bindir=0
    seen_datahome=0
    seen_count=0
    seen_end=0
    records_seen=0
    parse_ok=1

    while [ "$parse_ok" -eq 1 ] && { IFS= read -r manifest_line || [ -n "$manifest_line" ]; }; do
        IFS=' ' read -r key rest <<EOF
$manifest_line
EOF
        # Nothing may follow "end" - not a record, not a header field, and
        # not a comment either. This guard runs before any dispatch at all,
        # so it applies uniformly to every line shape, not just the ones
        # that happened to have their own seen_end check.
        if [ "$seen_end" -eq 1 ]; then
            parse_ok=0
            continue
        fi
        case "$key" in
            '#'*)
                : # comments are allowed anywhere before "end" and never counted
                ;;
            schema_version)
                if [ "$seen_schema" -eq 1 ]; then parse_ok=0
                else seen_schema=1; manifest_version=$rest
                fi
                ;;
            bin_dir)
                if [ "$seen_bindir" -eq 1 ]; then parse_ok=0
                else seen_bindir=1; manifest_saved_bin_dir=$rest
                fi
                ;;
            data_home)
                if [ "$seen_datahome" -eq 1 ]; then parse_ok=0
                else seen_datahome=1; manifest_saved_data_home=$rest
                fi
                ;;
            record_count)
                if [ "$seen_count" -eq 1 ]; then parse_ok=0
                else seen_count=1; manifest_declared_count=$rest
                fi
                ;;
            end)
                [ -z "$rest" ] || parse_ok=0
                seen_end=1
                ;;
            bin-emuwiz-cli|bin-emuwiz|desktop|icon-32|icon-64|icon-128|icon-256|icon-512)
                fld_kind=""; fld_fp=""; fld_size=""
                IFS=' ' read -r fld_kind fld_fp fld_size <<EOF
$rest
EOF
                if [ "$fld_kind" = file ] && validate_file_digest "$fld_fp" && validate_size "$fld_size"; then
                    store_file_slot "$key" "$fld_fp" "$fld_size"
                else
                    parse_ok=0
                fi
                ;;
            alias-archivefs-cli|alias-emuwiz-gui|alias-archivefs-gui)
                fld_kind=""; fld_target=""
                IFS=' ' read -r fld_kind fld_target <<EOF
$rest
EOF
                if [ "$fld_kind" = symlink ] && validate_symlink_fp "$fld_target"; then
                    store_symlink_slot "$key" "$fld_target"
                else
                    parse_ok=0
                fi
                ;;
            *)
                parse_ok=0
                ;;
        esac
    done <"${manifest_input:-$manifest_file}"

    [ "$parse_ok" -eq 1 ] || return 0
    [ "$seen_schema" -eq 1 ] || return 0
    [ "$seen_bindir" -eq 1 ] || return 0
    [ "$seen_datahome" -eq 1 ] || return 0
    [ "$seen_count" -eq 1 ] || return 0
    [ "$seen_end" -eq 1 ] || return 0
    [ "$manifest_version" = "$manifest_schema_version" ] || return 0
    validate_size "$manifest_declared_count" || return 0
    [ "$manifest_declared_count" = "$records_seen" ] || return 0
    [ -n "$manifest_saved_bin_dir" ] || return 0
    [ "$manifest_saved_bin_dir" = "$bin_dir" ] || return 0
    [ -n "$manifest_saved_data_home" ] || return 0
    [ "$manifest_saved_data_home" = "$data_home" ] || return 0
    manifest_loaded=1
}

# manifest_get_slot SLOT - sets slot_kind/slot_fp to the manifest's record
# for SLOT (both empty if manifest_loaded=0 or SLOT has no record - which
# is the normal, supported shape of an intentionally partial manifest, not
# a parse failure). Maps SLOT to the corresponding slot_<name>_kind/_fp
# pair via a literal case match only - never eval, never indirection driven
# by file content.
manifest_get_slot() {
    slot_kind=""
    slot_fp=""
    [ "$manifest_loaded" -eq 1 ] || return 0
    case "$1" in
        bin-emuwiz-cli) slot_kind=$slot_bin_emuwiz_cli_kind; slot_fp=$slot_bin_emuwiz_cli_fp ;;
        bin-emuwiz) slot_kind=$slot_bin_emuwiz_kind; slot_fp=$slot_bin_emuwiz_fp ;;
        alias-archivefs-cli) slot_kind=$slot_alias_archivefs_cli_kind; slot_fp=$slot_alias_archivefs_cli_fp ;;
        alias-emuwiz-gui) slot_kind=$slot_alias_emuwiz_gui_kind; slot_fp=$slot_alias_emuwiz_gui_fp ;;
        alias-archivefs-gui) slot_kind=$slot_alias_archivefs_gui_kind; slot_fp=$slot_alias_archivefs_gui_fp ;;
        desktop) slot_kind=$slot_desktop_kind; slot_fp=$slot_desktop_fp ;;
        icon-32) slot_kind=$slot_icon_32_kind; slot_fp=$slot_icon_32_fp ;;
        icon-64) slot_kind=$slot_icon_64_kind; slot_fp=$slot_icon_64_fp ;;
        icon-128) slot_kind=$slot_icon_128_kind; slot_fp=$slot_icon_128_fp ;;
        icon-256) slot_kind=$slot_icon_256_kind; slot_fp=$slot_icon_256_fp ;;
        icon-512) slot_kind=$slot_icon_512_kind; slot_fp=$slot_icon_512_fp ;;
    esac
}

# desktop_matches_ignoring_exec FILE1 FILE2 - true if FILE1 and FILE2 are
# line-for-line identical except that any line starting "Exec=" in one is
# accepted against any line starting "Exec=" in the other, unconditionally.
# Used only to recognise a pre-manifest desktop entry this installer wrote
# for a *different* --prefix, without requiring the Exec value itself to
# match.
desktop_matches_ignoring_exec() {
    a=$1
    b=$2
    [ -f "$a" ] && [ ! -L "$a" ] || return 1
    [ -f "$b" ] && [ ! -L "$b" ] || return 1
    exec 3<"$a" || return 1
    exec 4<"$b" || { exec 3<&-; return 1; }
    matched=0
    while :; do
        line_a=""; have_a=0
        if IFS= read -r line_a <&3; then have_a=1
        elif [ -n "$line_a" ]; then have_a=1
        fi
        line_b=""; have_b=0
        if IFS= read -r line_b <&4; then have_b=1
        elif [ -n "$line_b" ]; then have_b=1
        fi
        if [ "$have_a" -eq 0 ] && [ "$have_b" -eq 0 ]; then
            matched=1
            break
        fi
        if [ "$have_a" -eq 0 ] || [ "$have_b" -eq 0 ]; then
            break
        fi
        case "$line_a" in
            Exec=*)
                case "$line_b" in
                    Exec=*) ;;
                    *) break ;;
                esac
                ;;
            *)
                [ "$line_a" = "$line_b" ] || break
                ;;
        esac
    done
    exec 3<&- 4<&-
    [ "$matched" -eq 1 ]
}

# gate_binary SLOT PATH - prints absent/owned/foreign for a regular EmuWiz
# binary destination. There is no pre-manifest recognition rule here at
# all, unlike aliases/desktop/icons below: a binary's content legitimately
# differs release to release, so there is no byte pattern - or any other
# signal - that safely proves a pre-existing file is this installer's own
# without a manifest record to check against. An unrecorded path is always
# foreign. The gate reads PATH's CURRENT content and compares it against
# the OLD recorded digest from the manifest loaded at the start of this
# run - always strictly before any new content is ever written to PATH, so
# a legitimate reinstall/upgrade is validated against what was actually
# there, never against the new asset this run is about to install.
gate_binary() {
    slot=$1
    path=$2
    case $(path_kind "$path") in
        absent) printf 'absent\n'; return 0 ;;
        symlink|dir) printf 'foreign\n'; return 0 ;;
    esac
    manifest_get_slot "$slot"
    if [ "$slot_kind" = file ]; then
        current_fp=$(file_digest "$path") || current_fp=""
        if [ -n "$current_fp" ] && [ "$current_fp" = "$slot_fp" ]; then
            printf 'owned\n'
            return 0
        fi
    fi
    printf 'foreign\n'
}

# gate_alias SLOT PATH EXPECTED_TARGET - prints absent/owned/foreign for a
# legacy symlink alias destination.
gate_alias() {
    slot=$1
    path=$2
    expected_target=$3
    case $(path_kind "$path") in
        absent) printf 'absent\n'; return 0 ;;
        dir) printf 'foreign\n'; return 0 ;;
        file) printf 'foreign\n'; return 0 ;;
    esac
    manifest_get_slot "$slot"
    if [ -n "$slot_kind" ]; then
        if [ "$slot_kind" = symlink ]; then
            current_target=$(readlink -- "$path" 2>/dev/null) || current_target=""
            if [ -n "$current_target" ] && [ "$current_target" = "$slot_fp" ]; then
                printf 'owned\n'
                return 0
            fi
        fi
        printf 'foreign\n'
        return 0
    fi
    current_target=$(readlink -- "$path" 2>/dev/null) || current_target=""
    if [ -n "$current_target" ] && [ "$current_target" = "$expected_target" ]; then
        printf 'owned\n'
    else
        printf 'foreign\n'
    fi
}

# gate_content SLOT PATH REFERENCE NORMALIZE - prints absent/owned/foreign
# for the desktop entry (NORMALIZE=exec, REFERENCE is the freshly-rendered
# candidate to compare against ignoring Exec=) or an icon (NORMALIZE=,
# REFERENCE is the exact approved source PNG for that size). If REFERENCE
# itself is unavailable (e.g. uninstalling without the original release
# bundle present), the pre-manifest recognition rule cannot run and an
# unrecorded path is conservatively reported foreign rather than skipped.
# Like gate_binary, the manifest comparison here always reads PATH's
# current (pre-overwrite) content against the OLD recorded digest.
gate_content() {
    slot=$1
    path=$2
    reference=$3
    normalize=$4
    case $(path_kind "$path") in
        absent) printf 'absent\n'; return 0 ;;
        symlink|dir) printf 'foreign\n'; return 0 ;;
    esac
    manifest_get_slot "$slot"
    if [ -n "$slot_kind" ]; then
        if [ "$slot_kind" = file ]; then
            current_fp=$(file_digest "$path") || current_fp=""
            if [ -n "$current_fp" ] && [ "$current_fp" = "$slot_fp" ]; then
                printf 'owned\n'
                return 0
            fi
        fi
        printf 'foreign\n'
        return 0
    fi
    if [ -z "$reference" ] || [ ! -f "$reference" ]; then
        printf 'foreign\n'
        return 0
    fi
    if [ "$normalize" = exec ]; then
        if desktop_matches_ignoring_exec "$path" "$reference"; then
            printf 'owned\n'
        else
            printf 'foreign\n'
        fi
    else
        if cmp -s -- "$path" "$reference"; then
            printf 'owned\n'
        else
            printf 'foreign\n'
        fi
    fi
}

# backup_foreign_path PATH - moves PATH (file, symlink, or directory) aside
# into a freshly, securely, exclusively created backup directory in the
# SAME PARENT as PATH (guaranteeing the same filesystem, so the move below
# is a plain rename, not a cross-device copy). mktemp -d's whole contract
# is that the name it returns did not exist a moment ago and cannot have
# been pre-planted by anything - there is no predictable name here for
# anything to race or pre-occupy, unlike a PID- or timestamp-derived name.
# The foreign object is then moved, under its own original basename, into
# that directory: since the directory was just created empty and exclusively
# by us, that destination is guaranteed not to exist yet either, so the
# move can never clobber anything (mv -n is added as a second, redundant
# guard rather than the only one). mv operates on PATH itself when PATH is
# a symlink - it is never dereferenced or followed. Sets BACKUP_PATH to the
# exact resulting location, which is the deterministic recovery
# instruction: the foreign object is never deleted, only moved, and stays
# exactly there - including if a later step in this same install run then
# fails, since nothing after this function ever touches BACKUP_PATH again.
backup_foreign_path() {
    path=$1
    parent=$(dirname -- "$path")
    base=$(basename -- "$path")
    backup_dir=$(mktemp -d -- "$parent/.emuwiz-foreign-backup.XXXXXXXX") ||
        fail "could not allocate a secure backup location next to: $path"
    chmod 0700 -- "$backup_dir" 2>/dev/null || true
    backup_target="$backup_dir/$base"
    mv -n -- "$path" "$backup_target" ||
        fail "could not move foreign path aside for backup (it has been left in place at: $path)"
    BACKUP_PATH="$backup_target"
}

# resolve_foreign_collision PATH - called only when a gate function reports
# "foreign". Without --replace-foreign: warns and returns 1 (caller must
# leave PATH alone). With --replace-foreign: backs PATH up via
# backup_foreign_path (never overwriting anything, never following a
# symlink) and returns 0 so the caller proceeds to install fresh.
resolve_foreign_collision() {
    path=$1
    if [ "$replace_foreign" -ne 1 ]; then
        warn "leaving foreign path untouched (not EmuWiz-owned): $path"
        warn "  re-run with --replace-foreign to move it aside and install here"
        skipped_any=1
        return 1
    fi
    backup_foreign_path "$path"
    warn "moved foreign path aside before installing: $path -> $BACKUP_PATH"
    return 0
}

# Linux descriptor paths keep reads/writes on the opened object even if its
# pathname is replaced. flock serializes cooperating install/uninstall runs;
# identity + digest checks also detect external replacements and in-place edits.
lock_manifest_dir() {
    command -v flock >/dev/null 2>&1 || fail "flock is required for safe installer bookkeeping"
    exec 8<"$manifest_dir"
    flock -n 8 || fail "another installer operation holds the bookkeeping lock: $manifest_dir"
    binding_dir=$(stat -Lc '%d:%i:%f' -- /proc/self/fd/8) || fail "cannot identify bookkeeping directory"
    binding_parent=$(stat -Lc '%d:%i:%f' -- "$data_home") || fail "cannot identify data directory"
    binding_location=$(CDPATH= cd -- "$manifest_dir" && pwd -P) || fail "cannot resolve bookkeeping directory"
    check_manifest_dir_binding
}

check_manifest_dir_binding() {
    [ "$(path_kind "$manifest_dir")" = dir ] \
        && [ "$(stat -c '%d:%i:%f' -- "$manifest_dir" 2>/dev/null)" = "$binding_dir" ] \
        && [ "$(stat -Lc '%d:%i:%f' -- "$data_home" 2>/dev/null)" = "$binding_parent" ] \
        && [ "$(CDPATH= cd -- "$manifest_dir" 2>/dev/null && pwd -P)" = "$binding_location" ] \
        || fail "installer bookkeeping location changed; refusing further publication: $manifest_dir; assets already published may remain unrecorded"
}

# Include ctime/mtime and size as well as device/inode/type. A digest binds the
# content used by the parser; the descriptor binds the object used by the writer.
manifest_object_state() {
    stat -c '%d:%i:%f:%s:%y:%z' -- "$manifest_file" 2>/dev/null
}
manifest_descriptor_state() {
    stat -Lc '%d:%i:%f:%s:%y:%z' -- /proc/self/fd/9 2>/dev/null
}

assert_manifest_binding() {
    check_manifest_dir_binding
    if [ "$binding_manifest" = absent ]; then
        [ "$(path_kind "$manifest_file")" = absent ] ||
            fail "ownership manifest appeared after inspection; refusing further publication: $manifest_file; assets already published may remain unrecorded"
    else
        [ "$(manifest_object_state)" = "$binding_manifest" ] ||
            fail "ownership manifest object changed; refusing further publication: $manifest_file; assets already published may remain unrecorded"
        if [ -n "$binding_digest" ]; then
            [ "$(manifest_descriptor_state)" = "$binding_manifest" ] \
                && [ "$(file_digest /proc/self/fd/9)" = "$binding_digest" ] ||
                fail "ownership manifest content changed; refusing further publication: $manifest_file; assets already published may remain unrecorded"
        fi
    fi
}

preflight_install_manifest() {
    # Refresh the early path inspection before using the bookkeeping directory.
    manifest_dir_kind=$(manifest_dir_state)
    ensure_manifest_dir
    lock_manifest_dir
    chmod 0700 /proc/self/fd/8 2>/dev/null || true
    binding_dir=$(stat -Lc '%d:%i:%f' -- /proc/self/fd/8) || fail "cannot identify bookkeeping directory"
    check_manifest_dir_binding
    # An unfinished earlier upgrade stops here, before anything is read or
    # published, with the exact restore steps (see UPGRADE PRESERVATION).
    asset_io pending "$binding_location" || exit 1
    check_manifest_dir_binding
    binding_manifest=absent
    binding_digest=""
    manifest_input=""
    manifest_loaded=0
    if [ -e "$manifest_file" ] || [ -L "$manifest_file" ]; then
        binding_manifest=$(manifest_object_state) || fail "cannot identify ownership manifest"
        if [ -f "$manifest_file" ] && [ ! -L "$manifest_file" ] && [ -r "$manifest_file" ]; then
            exec 9<"$manifest_file"
            [ "$(manifest_descriptor_state)" = "$binding_manifest" ] ||
                fail "ownership manifest changed while opening it"
            binding_digest=$(file_digest /proc/self/fd/9) || fail "cannot fingerprint ownership manifest"
            manifest_input=/proc/self/fd/9
            load_manifest
        fi
        assert_manifest_binding
        if [ "$manifest_loaded" -ne 1 ]; then
            if [ "$replace_foreign" -eq 1 ]; then
                backup_foreign_path "$manifest_file"
                warn "moved an unrecognised path at the ownership manifest location aside before installing: $manifest_file -> $BACKUP_PATH"
                exec 9<&-
                binding_manifest=absent
                binding_digest=""
            else
                fail "ownership manifest exists but is not provably EmuWiz-owned: $manifest_file - refusing to install managed files; re-run with --replace-foreign to move it aside"
            fi
        fi
    fi
    assert_manifest_binding
}

# The Python stdlib supplies openat/linkat/symlinkat and O_NOFOLLOW; sh cannot
# safely open a validated asset without following a substituted symlink.
# Keep the helper embedded so old and current bundle layouts remain self-contained.
asset_io() {
    python3 - "$@" <<'EMUWIZ_ASSET_PY'
import errno
import hashlib
import os
import shutil
import stat
import sys
import tempfile


def identity(st):
    return ":".join(str(value) for value in (
        st.st_dev, st.st_ino, st.st_mode, st.st_size, st.st_mtime_ns, st.st_ctime_ns))


def snapshot(name):
    try:
        return identity(os.stat(name, dir_fd=7, follow_symlinks=False))
    except FileNotFoundError:
        return "absent"


def check_parent(parent, location):
    # Mirror the bookkeeping binding: identity plus physical location. Walk
    # the canonical components without symlinks; final mutations use fd 7 only.
    if os.path.realpath(parent) != location or os.readlink("/proc/self/fd/7") != location:
        raise RuntimeError("destination parent changed during installation")
    fd = os.open("/", os.O_RDONLY | os.O_DIRECTORY)
    try:
        for component in location.split("/"):
            if component:
                child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                os.close(fd)
                fd = child
        held, current = os.fstat(7), os.fstat(fd)
        if (held.st_dev, held.st_ino) != (current.st_dev, current.st_ino):
            raise RuntimeError("destination parent changed during installation")
    finally:
        os.close(fd)


class Busy(Exception):
    """The owned inode is being executed, so it cannot accept an in-place update."""


def open_for_update(fd, name, location):
    # Opening the verified inode for write (no O_TRUNC/O_CREAT, no write) is the
    # exact operation the in-place update needs, and the kernel refuses it with
    # ETXTBSY precisely while the inode is executing. It changes no byte and no
    # inode field. The path comes from our own descriptor, never the name.
    try:
        return os.open(f"/proc/self/fd/{fd}", os.O_WRONLY)
    except OSError as error:
        if error.errno == errno.ETXTBSY:
            raise Busy(f"{location}/{name} is being executed") from error
        raise


def fingerprint(fd):
    digest = hashlib.sha256()
    with os.fdopen(os.dup(fd), "rb") as stream:
        stream.seek(0)
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def inspect(name, parent, location):
    check_parent(parent, location)
    try:
        fd = os.open(name, os.O_PATH | os.O_NOFOLLOW, dir_fd=7)
    except FileNotFoundError:
        print("absent -")
        return
    held = os.fstat(fd)
    expected = identity(held)
    digest = "-"
    if stat.S_ISREG(held.st_mode):
        try:
            readable = os.open(f"/proc/self/fd/{fd}", os.O_RDONLY)
        except PermissionError:
            pass  # The existing gate treats unreadable files as foreign.
        else:
            digest = fingerprint(readable)
            if identity(os.fstat(readable)) != expected:
                raise RuntimeError("destination changed during installation")
    if snapshot(name) != expected:
        raise RuntimeError("destination changed during installation")
    check_parent(parent, location)
    print(expected, digest)


def preflight(source, name, expected, digest, parent, location):
    """Fail early if a changed, owned binary cannot be updated in place.

    Binds to the same object publish() will update: the name is opened through
    the pinned parent with O_NOFOLLOW, then its identity and digest must still
    match what the gate verified. Identical bytes need no update, so a running
    binary is fine. Nothing is written.
    """
    check_parent(parent, location)
    with open(source, "rb") as incoming:
        new_digest = fingerprint(incoming.fileno())
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=7)
    held = os.fstat(fd)
    if not stat.S_ISREG(held.st_mode) or identity(held) != expected:
        raise RuntimeError("destination changed during installation")
    if fingerprint(fd) != digest:
        raise RuntimeError("destination changed during installation (content)")
    if digest != new_digest:
        os.close(open_for_update(fd, name, location))
    if snapshot(name) != expected or identity(os.fstat(fd)) != expected:
        raise RuntimeError("destination changed during installation")
    check_parent(parent, location)


def link_held(fd, name):
    # linkat AT_SYMLINK_FOLLOW follows only our proc descriptor to the held
    # inode (including an O_PATH-held symlink), never the destination name.
    os.link(f"/proc/self/fd/{fd}", name, dst_dir_fd=7, follow_symlinks=True)


def publish(kind, slot, source, name, expected, gate, digest, parent, location):
    if gate not in ("absent", "owned"):
        raise RuntimeError("foreign destination was not authorized")
    check_parent(parent, location)
    if snapshot(name) != expected:
        raise RuntimeError("destination changed during installation")
    if kind == "symlink":
        if gate == "owned":
            # Alias targets are fixed across releases. Hold the symlink itself;
            # never unlink/replace a name on the strength of an earlier check.
            fd = os.open(name, os.O_PATH | os.O_NOFOLLOW, dir_fd=7)
            if identity(os.fstat(fd)) != expected or os.readlink("", dir_fd=fd) != source:
                raise RuntimeError("destination changed during installation (alias target)")
        else:
            with tempfile.TemporaryDirectory(prefix=".emuwiz-alias.", dir="/proc/self/fd/7") as staging:
                candidate = staging + "/link"
                os.symlink(source, candidate)
                fd = os.open(candidate, os.O_PATH | os.O_NOFOLLOW)
                if os.readlink("", dir_fd=fd) != source:
                    raise RuntimeError("staged alias changed during installation")
                link_held(fd, name)  # EEXIST: never replace a destination
        published = identity(os.fstat(fd))
        if snapshot(name) != published or os.readlink("", dir_fd=fd) != source:
            raise RuntimeError("destination changed during installation")
        record = f"{slot} symlink {source}"
    else:
        staged = os.open(source, os.O_RDONLY | os.O_NOFOLLOW)
        staged_stat = os.fstat(staged)
        if not stat.S_ISREG(staged_stat.st_mode):
            raise RuntimeError("staged asset is not a regular file")
        new_digest = fingerprint(staged)
        if gate == "owned":
            fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=7)
            held = os.fstat(fd)
            if not stat.S_ISREG(held.st_mode) or identity(held) != expected:
                raise RuntimeError("destination changed during installation")
            old_digest = fingerprint(fd)
            if old_digest != digest:
                raise RuntimeError("destination changed during installation (content)")
            if old_digest != new_digest:
                # Like the manifest update: write the held inode, never rename
                # over a name. Fail closed for hardlinks and running executables.
                if held.st_nlink != 1:
                    raise RuntimeError("owned destination has hardlinks; refusing an in-place update")
                if slot in ("bin-emuwiz-cli", "bin-emuwiz"):
                    require_preserved(slot, old_digest)
                output = open_for_update(fd, name, location)
                check_parent(parent, location)
                if snapshot(name) != expected or identity(os.fstat(fd)) != expected:
                    raise RuntimeError("destination changed during installation")
                with os.fdopen(os.dup(staged), "rb") as input_stream, os.fdopen(output, "wb") as stream:
                    input_stream.seek(0)
                    shutil.copyfileobj(input_stream, stream)
                    stream.flush()
                    os.ftruncate(stream.fileno(), staged_stat.st_size)
                    os.fchmod(stream.fileno(), stat.S_IMODE(staged_stat.st_mode))
        else:
            link_held(staged, name)
            fd = staged
        published = os.fstat(fd)
        current = os.stat(name, dir_fd=7, follow_symlinks=False)
        if identity(current) != identity(published) or fingerprint(fd) != new_digest:
            raise RuntimeError("destination changed during installation")
        record = f"{slot} file {new_digest} {staged_stat.st_size}"
    check_parent(parent, location)
    # Record the bytes/target we published, never re-adopt a substituted pathname.
    print(record)


class Refused(Exception):
    """Upgrade safety could not be established; nothing was changed."""


# Room left over after a preserved copy or a growing binary, so keeping the
# previous version never fills a filesystem to the last block.
SPACE_MARGIN = 16 * 1024 * 1024
JOURNAL = "upgrade-pending"
PREVIOUS = "previous"


def fsync_dir(fd):
    try:
        os.fsync(fd)
    except OSError as error:
        raise Refused(f"cannot make the preserved copy durable on this filesystem ({error})") from error


def previous_dir(create):
    # fd 8 is the locked, bound bookkeeping directory. "previous" is created
    # by this installer only: a real directory, ours, not writable by others.
    if create:
        try:
            os.mkdir(PREVIOUS, 0o700, dir_fd=8)
            fsync_dir(8)
        except FileExistsError:
            pass
    try:
        fd = os.open(PREVIOUS, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=8)
    except OSError as error:
        if error.errno in (errno.ENOTDIR, errno.ELOOP):
            raise Refused(f"{PREVIOUS} in the installer bookkeeping directory is not a real directory") from error
        raise
    held = os.fstat(fd)
    if held.st_uid != os.getuid() or held.st_mode & 0o022:
        os.close(fd)
        raise Refused("the preserved-versions directory is not private to you")
    return fd


def preserved_matches(directory, name, digest):
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
    except FileNotFoundError:
        return False
    try:
        return stat.S_ISREG(os.fstat(fd).st_mode) and fingerprint(fd) == digest
    finally:
        os.close(fd)


def free_bytes(fd):
    info = os.fstatvfs(fd)
    return info.f_bavail * info.f_frsize


def preserve(source, name, expected, digest, parent, location, slot, bookkeeping):
    """Keep the exact installed bytes before an owned binary is changed.

    Runs after preflight and before the first publication. The copy is written
    to a fresh name, fsynced, re-hashed, then linked (never renamed over) to
    previous/<slot>.<old sha256> in the locked bookkeeping directory, and a
    journal line is appended and fsynced. Only then may publish() write the
    installed inode. Any doubt refuses with nothing installed.
    """
    check_parent(parent, location)
    with open(source, "rb") as incoming:
        new_digest = fingerprint(incoming.fileno())
        new_size = os.fstat(incoming.fileno()).st_size
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=7)
    held = os.fstat(fd)
    if not stat.S_ISREG(held.st_mode) or identity(held) != expected or fingerprint(fd) != digest:
        raise RuntimeError("destination changed during installation")
    if digest == new_digest:
        return
    if free_bytes(8) < held.st_size + SPACE_MARGIN:
        raise Refused(f"not enough free space in {bookkeeping} to keep the previous "
                      f"{location}/{name} ({held.st_size} bytes plus a safety margin)")
    if free_bytes(7) < max(0, new_size - held.st_size) + SPACE_MARGIN:
        raise Refused(f"not enough free space in {location} for the larger new {name}")
    directory = previous_dir(create=True)
    kept = f"{slot}.{digest}"
    try:
        if not preserved_matches(directory, kept, digest):
            try:
                os.stat(kept, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                pass
            else:
                raise Refused(f"{bookkeeping}/{PREVIOUS}/{kept} exists but is not the installed "
                              "version; move it away and run the installer again")
            partial = f".{kept}.partial-{os.getpid()}"
            out = os.open(partial, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                          stat.S_IMODE(held.st_mode) & 0o700, dir_fd=directory)
            try:
                with os.fdopen(os.dup(fd), "rb") as old, os.fdopen(os.dup(out), "wb") as copy:
                    old.seek(0)
                    shutil.copyfileobj(old, copy)
                    copy.flush()
                    os.fsync(copy.fileno())
                if fingerprint(out) != digest:
                    raise Refused("the preserved copy does not match the installed version")
                os.link(partial, kept, src_dir_fd=directory, dst_dir_fd=directory)
            except OSError as error:
                raise Refused(f"could not keep the previous version ({error})") from error
            finally:
                os.close(out)
                try:
                    os.unlink(partial, dir_fd=directory)
                except FileNotFoundError:
                    pass
            fsync_dir(directory)
        if identity(os.fstat(fd)) != expected or fingerprint(fd) != digest:
            raise RuntimeError("destination changed while keeping the previous version")
        line = os.fsencode(f"{slot} {digest} {new_digest} {location}/{name}\n")
        if b"\n" in os.fsencode(f"{location}/{name}"):
            raise Refused("the install path contains a line break")
        journal = os.open(JOURNAL, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600, dir_fd=8)
        try:
            if not stat.S_ISREG(os.fstat(journal).st_mode):
                raise Refused(f"{bookkeeping}/{JOURNAL} is not a regular file")
            os.write(journal, line)
            os.fsync(journal)
        finally:
            os.close(journal)
        fsync_dir(8)
    finally:
        os.close(directory)
    print(f"{bookkeeping}/{PREVIOUS}/{kept}")


def require_preserved(slot, digest):
    """publish() refuses an owned binary change without durable recovery evidence."""
    directory = previous_dir(create=False)
    try:
        if not preserved_matches(directory, f"{slot}.{digest}", digest):
            raise Refused(f"the preserved previous version for {slot} is missing or changed")
    finally:
        os.close(directory)
    entries = read_journal()
    if not any(entry[0] == slot and entry[1] == digest for entry in entries):
        raise Refused(f"the upgrade journal has no record for {slot}")


def read_journal():
    try:
        fd = os.open(JOURNAL, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=8)
    except FileNotFoundError:
        return []
    with os.fdopen(fd, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise Refused(f"{JOURNAL} is not a regular file")
        data = stream.read()
    if data and not data.endswith(b"\n"):
        raise Refused(f"{JOURNAL} ends with an incomplete line")
    entries = []
    for raw in os.fsdecode(data).split("\n")[:-1]:
        fields = raw.split(" ", 3)
        if (len(fields) != 4 or fields[0] not in ("bin-emuwiz-cli", "bin-emuwiz")
                or not all(len(value) == 64 and all(c in "0123456789abcdef" for c in value)
                           for value in fields[1:3])
                or not fields[3].startswith("/")):
            raise Refused(f"{JOURNAL} is not a complete upgrade journal")
        entries.append(fields)
    return entries


def current_digest(path):
    """Digest of a regular file, or a word naming what is there instead."""
    try:
        handle = os.open(path, os.O_PATH | os.O_NOFOLLOW)
    except FileNotFoundError:
        return "absent"
    except OSError:
        return "unreadable"
    try:
        mode = os.fstat(handle).st_mode
        if stat.S_ISLNK(mode):
            return "symlink"
        if not stat.S_ISREG(mode):
            return "not-a-file"
        try:
            fd = os.open(f"/proc/self/fd/{handle}", os.O_RDONLY | os.O_NONBLOCK)
        except OSError:
            return "unreadable"
        try:
            return fingerprint(fd)
        finally:
            os.close(fd)
    finally:
        os.close(handle)


def manifest_records(slot, digest):
    """True when the completed ownership manifest already records DIGEST for SLOT.

    The journal outlives the manifest only when a run stopped between writing
    the new manifest and removing the journal; the new bytes are then recorded
    and the upgrade is complete. The shell's strict parser still validates the
    whole manifest before anything else trusts it.
    """
    try:
        fd = os.open("manifest", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=8)
    except OSError:
        return False
    with os.fdopen(fd, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            return False
        lines = stream.read().split(b"\n")
    if lines[-2:] != [b"end", b""]:
        return False
    prefix = f"{slot} file {digest} ".encode()
    return any(line.startswith(prefix) for line in lines)


def pending(bookkeeping):
    """Check an earlier run's journal before anything else happens.

    Every journal line was written after its preserved copy was durable and
    before its binary was touched; the manifest still records the old digest
    until a run completes. A destination back at the old digest is resolved.
    Anything else stops this run with the exact manual restore steps; this
    installer never restores automatically.
    """
    try:
        entries = read_journal()
    except (Refused, UnicodeDecodeError) as error:
        raise Refused(f"an earlier upgrade left {bookkeeping}/{JOURNAL}, which cannot be read "
                      f"({error}); inspect it before running the installer again") from error
    if not entries:
        try:
            os.unlink(JOURNAL, dir_fd=8)
        except FileNotFoundError:
            pass
        return
    unresolved = []
    for slot, old, new, dest in entries:
        state = current_digest(dest)
        if state == old or (state == new and manifest_records(slot, new)):
            continue
        unresolved.append((slot, old, new, dest, state))
    if not unresolved:
        os.unlink(JOURNAL, dir_fd=8)
        fsync_dir(8)
        return
    directory = None
    try:
        directory = previous_dir(create=False)
    except (OSError, Refused):
        pass
    lines = ["install.sh: an earlier EmuWiz upgrade did not finish. This run changed nothing.",
             f"Journal: {bookkeeping}/{JOURNAL}"]
    commands = []
    for slot, old, new, dest, state in unresolved:
        what = {
            "absent": "is missing",
            new: "already holds the new version, which was never recorded",
            "symlink": "is now a symbolic link, which this installer never creates there",
            "not-a-file": "is no longer a regular file",
            "unreadable": "cannot be read",
        }.get(state, "holds neither the previous nor the new version (interrupted while writing)")
        kept = f"{bookkeeping}/{PREVIOUS}/{slot}.{old}"
        lines.append(f"  {dest} {what}.")
        if state in ("symlink", "not-a-file", "unreadable"):
            lines.append(f"  No restore command is given: copying over it could write somewhere else. "
                         f"Inspect it by hand; the previous version is at {kept} if it is still intact.")
        elif directory is not None and preserved_matches(directory, f"{slot}.{old}", old):
            lines.append(f"  The previous version is kept, verified, at {kept} (sha256 {old}).")
            commands.append((kept, dest, old))
        else:
            lines.append(f"  The preserved previous version {kept} is missing or changed, so it cannot be "
                         "restored from here. Check that path by hand; reinstall over it only with "
                         "--replace-foreign once you are sure.")
    if directory is not None:
        os.close(directory)
    if commands:
        lines.append("To restore the previous version, close EmuWiz and run:")
        for kept, dest, old in commands:
            lines.append(f"  cp -- {quote(kept)} {quote(dest)}")
            lines.append(f"  sha256sum -- {quote(dest)}    # must print {old}")
        lines.append("Then run the installer again; it will continue the upgrade from the restored version.")
    # Paths are written back as their exact bytes, so a restore command for a
    # non-UTF-8 prefix names the real directory rather than an escaped form.
    sys.stderr.flush()
    sys.stderr.buffer.write(os.fsencode("\n".join(lines) + "\n"))
    sys.stderr.buffer.flush()
    sys.exit(1)


def finish():
    """Make the upgrade durable, then retire its journal.

    The new binaries and the new manifest (fd 9) are fsynced before the
    journal is unlinked through fd 8, and the directory is fsynced after.
    """
    for slot, old, new, dest in read_journal():
        try:
            fd = os.open(dest, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        except FileNotFoundError:
            continue
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    os.fsync(9)
    try:
        os.unlink(JOURNAL, dir_fd=8)
    except FileNotFoundError:
        return
    fsync_dir(8)


def quote(value):
    return "'" + value.replace("'", "'\\''") + "'"


def main():
    if sys.platform != "linux" or not all(hasattr(os, flag) for flag in ("O_PATH", "O_NOFOLLOW", "O_DIRECTORY")):
        raise RuntimeError("Linux descriptor publication support is required")
    if not all(fn in os.supports_dir_fd for fn in (os.open, os.stat, os.readlink, os.symlink, os.link)):
        raise RuntimeError("Python descriptor-relative filesystem support is required")
    if sys.argv[1] == "inspect":
        inspect(*sys.argv[2:])
    elif sys.argv[1] == "preflight":
        preflight(*sys.argv[2:])
    elif sys.argv[1] in ("preserve", "pending", "finish"):
        try:
            {"preserve": preserve, "pending": pending, "finish": finish}[sys.argv[1]](*sys.argv[2:])
        except OSError as error:
            raise Refused(str(error)) from error
    else:
        publish(*sys.argv[2:])


if __name__ == "__main__":
    try:
        main()
    except Busy as error:
        # Not an error in the installer: EmuWiz is simply open. The preflight
        # runs before anything is published, so nothing has changed.
        where = "before anything was installed" if sys.argv[1] == "preflight" else \
            "after other files were already published; run the installer again"
        print("install.sh: EmuWiz is currently running and needs to be closed before it can be "
              f"updated. Close EmuWiz and try again. ({error}; stopped {where})", file=sys.stderr)
        sys.exit(1)
    except Refused as error:
        where = {
            "preserve": "no installed file was changed (any previous-version copy already kept by this run "
                        "stays under emuwiz-installer/previous)",
            "pending": "no installed file was changed",
            "finish": "the upgrade itself is complete and recorded; run the installer again to retire its journal",
        }.get(sys.argv[1], "files already published remain in place")
        print(f"install.sh: upgrade safety could not be established: {error}; {where}", file=sys.stderr)
        sys.exit(1)
    except (OSError, RuntimeError) as error:
        print(f"install.sh: destination changed during installation or safe publication unavailable: {error}; "
              "refusing further publication; assets already published may remain unrecorded", file=sys.stderr)
        sys.exit(1)
EMUWIZ_ASSET_PY
}

# Pin the asset's parent before its gate and stage in that same held directory.
# Symlinked XDG/prefix roots remain supported; retargeting them after this pin
# cannot redirect publication. fd 7 is separate from bookkeeping fds 8 and 9.
prepare_asset() {
    asset_dest=$1
    asset_parent=$(dirname -- "$asset_dest")
    asset_name=$(basename -- "$asset_dest")
    exec 7<"$asset_parent"
    asset_location=$(CDPATH= cd -- "$asset_parent" && pwd -P) || fail "cannot resolve asset parent"
    asset_bound="/proc/self/fd/7/$asset_name"
    asset_inspection=$(asset_io inspect "$asset_name" "$asset_parent" "$asset_location") || exit 1
    asset_state=${asset_inspection%% *}
    asset_digest=${asset_inspection#* }
}

publish_asset() {
    assert_manifest_binding
    asset_record=$(asset_io publish "$1" "$2" "$3" "$asset_name" "$asset_state" "$4" \
        "$asset_digest" "$asset_parent" "$asset_location") || exit 1
    assert_manifest_binding
    printf '%s\n' "$asset_record" >>"$manifest_records_tmp"
}

# --replace-foreign authorizes only the object encountered at the gate. Back
# it up through the pinned parent; a new collision must still fail closed.
resolve_asset_collision() {
    if [ "$replace_foreign" -ne 1 ]; then
        resolve_foreign_collision "$asset_dest"
        return 1
    fi
    backup_foreign_path "$asset_bound"
    BACKUP_PATH="$asset_location/${BACKUP_PATH#/proc/self/fd/7/}"
    warn "moved foreign path aside before installing: $asset_dest -> $BACKUP_PATH"
    asset_state=absent
}

# remove_if_owned SLOT KIND PATH [EXPECTED_OR_REFERENCE] [NORMALIZE] -
# removes PATH during --uninstall only if a gate function reports it
# "owned". Absent paths are silently skipped (nothing to warn about); a
# directory is always left alone with a warning. Foreign paths are left
# alone with a warning and never forced (--replace-foreign has no effect
# during --uninstall - there is nothing to "replace" when only removing).
remove_if_owned() {
    slot=$1
    kind=$2
    path=$3
    extra=${4:-}
    normalize=${5:-}

    case $(path_kind "$path") in
        absent) return 0 ;;
    esac

    case "$kind" in
        symlink) gate=$(gate_alias "$slot" "$path" "$extra") ;;
        file) gate=$(gate_content "$slot" "$path" "$extra" "$normalize") ;;
        binary) gate=$(gate_binary "$slot" "$path") ;;
    esac

    case "$gate" in
        owned)
            rm -f -- "$path" || fail "could not remove $path"
            printf 'Removed %s\n' "$path"
            removed_any=1
            ;;
        *)
            case $(path_kind "$path") in
                dir) warn "expected $slot at $path but found a directory; leaving it in place" ;;
                *) warn "leaving foreign path untouched (not EmuWiz-owned): $path" ;;
            esac
            left_foreign=1
            ;;
    esac
}

bin_dir="$default_bin_dir"
do_uninstall=0
developer_unchecked=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix)
            [ "$#" -ge 2 ] || fail "--prefix requires a PATH argument"
            bin_dir="$2"
            shift 2
            ;;
        --prefix=*)
            bin_dir="${1#--prefix=}"
            shift
            ;;
        --replace-foreign)
            replace_foreign=1
            shift
            ;;
        --uninstall)
            do_uninstall=1
            shift
            ;;
        --developer-unchecked-install)
            developer_unchecked=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            printf '%s: unknown argument: %s\n' "$program_name" "$1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

[ -n "$bin_dir" ] || fail "--prefix was given an empty PATH"

# Directory containing this script, resolved to an absolute path. Used to
# find the binaries (and config.toml.example) it was shipped alongside.
script_dir=$(dirname -- "$0")
script_dir=$(CDPATH= cd -- "$script_dir" && pwd) || fail "could not resolve the directory containing $program_name"

# desktop_template/branding_dir are resolved here (not just inside the
# install path below) because --uninstall's pre-manifest recognition rule
# for the desktop entry and icons needs the same reference content to
# compare against. Best-effort for uninstall: a missing bundle only means
# those two specific pre-manifest checks fall back to "foreign" (see
# gate_content) rather than failing the whole uninstall - a manifest from
# any successful install makes this irrelevant on every later run.
desktop_template="$script_dir/assets/linux/$desktop_id.desktop.in"
branding_dir="$script_dir/assets/branding"

if [ "$do_uninstall" -eq 1 ]; then
    # bin_dir must be an absolute, resolved path for manifest bin_dir
    # comparison to have any chance of matching what install recorded -
    # mirror the same resolution install uses, but tolerate a prefix that
    # does not exist (there may be nothing left to uninstall there at all).
    if [ -d "$bin_dir" ]; then
        bin_dir=$(CDPATH= cd -- "$bin_dir" && pwd -P) || fail "could not resolve install prefix: $bin_dir"
    fi

    manifest_dir_kind=$(manifest_dir_state)
    if [ "$manifest_dir_kind" = dir ]; then lock_manifest_dir; fi
    if [ "$manifest_dir_kind" = dir ] && { [ -e "$manifest_dir/upgrade-pending" ] || [ -L "$manifest_dir/upgrade-pending" ]; }; then
        fail "an earlier upgrade did not finish ($manifest_dir/upgrade-pending); nothing was removed. Run the installer (without --uninstall) to see the restore steps, then uninstall."
    fi
    load_manifest

    removed_any=0
    left_foreign=0

    remove_if_owned bin-emuwiz-cli binary "$bin_dir/emuwiz-cli"
    remove_if_owned bin-emuwiz binary "$bin_dir/emuwiz"
    remove_if_owned alias-archivefs-cli symlink "$bin_dir/archivefs-cli" emuwiz-cli
    remove_if_owned alias-emuwiz-gui symlink "$bin_dir/emuwiz-gui" emuwiz
    remove_if_owned alias-archivefs-gui symlink "$bin_dir/archivefs-gui" emuwiz

    desktop_reference=""
    if [ -f "$desktop_template" ]; then
        # Render what a legitimate desktop entry for the CURRENT bin_dir
        # would look like, purely so the pre-manifest gate can compare
        # against it (ignoring Exec=, since a manifest match already covers
        # the "same bin_dir as before" case without needing this at all).
        case "$bin_dir/emuwiz" in
            *'
'*) : ;;
            *)
                desktop_reference=$(mktemp -- "${TMPDIR:-/tmp}/.emuwiz-uninstall-ref.XXXXXX") || desktop_reference=""
                if [ -n "$desktop_reference" ]; then
                    desktop_exec_ref=$(printf '%s' "$bin_dir/emuwiz" | sed \
                        -e 's/\\/\\\\\\\\/g' \
                        -e 's/"/\\"/g' \
                        -e 's/`/\\`/g' \
                        -e 's/\$/\\$/g' \
                        -e 's/%/%%/g')
                    desktop_exec_ref="\"$desktop_exec_ref\""
                    while IFS= read -r line || [ -n "$line" ]; do
                        if [ "$line" = 'Exec=@EMUWIZ_EXEC@' ]; then
                            printf 'Exec=%s\n' "$desktop_exec_ref"
                        else
                            printf '%s\n' "$line"
                        fi
                    done <"$desktop_template" >"$desktop_reference" 2>/dev/null || desktop_reference=""
                fi
                ;;
        esac
    fi
    remove_if_owned desktop file "$desktop_file" "$desktop_reference" exec
    [ -z "$desktop_reference" ] || rm -f -- "$desktop_reference"

    for size in 32 64 128 256 512; do
        icon_reference="$branding_dir/emuwiz-logo-$size.png"
        [ -f "$icon_reference" ] || icon_reference=""
        remove_if_owned "icon-$size" file \
            "$data_home/icons/hicolor/${size}x${size}/apps/$desktop_id.png" \
            "$icon_reference"
    done

    if [ "$removed_any" -eq 0 ]; then
        printf 'Nothing to uninstall in %s\n' "$bin_dir"
    fi
    if [ "$left_foreign" -eq 1 ]; then
        printf 'Some paths were left in place because they are not provably EmuWiz-owned; see the warnings above.\n' >&2
    fi
    # Clean up the installer's own bookkeeping only once nothing was left
    # behind, and only through a manifest_dir confirmed to be a real
    # directory and a manifest_file confirmed to be a plain regular file -
    # if something was left foreign, or the bookkeeping path itself is not
    # safe to touch, keep whatever is there so a future run (or a human)
    # still has it to work from.
    if [ "$manifest_dir_kind" = dir ] && [ -d "$manifest_dir/previous" ] && [ ! -L "$manifest_dir/previous" ]; then
        printf 'Previous versions kept by earlier upgrades remain in %s; delete them yourself if you no longer need them.\n' "$manifest_dir/previous"
    fi
    if [ "$left_foreign" -eq 0 ] && [ "$manifest_dir_kind" = dir ] \
        && [ -f "$manifest_file" ] && [ ! -L "$manifest_file" ]; then
        rm -f -- "$manifest_file"
        rmdir -- "$manifest_dir" 2>/dev/null || true
    fi
    printf 'Your configuration at %s was left untouched.\n' "$config_file"
    exit 0
fi

# publish_starter_config SRC DIR DEST - never writes through or replaces DEST.
# Linux and Python are already required by safe asset publication. Create an
# anonymous O_TMPFILE regular inode in a pinned configuration directory, write
# only its descriptor, then publish with linkat(AT_SYMLINK_FOLLOW) through procfs.
# The kernel creates this inode without a staging name: existing regular files,
# FIFOs, sockets and symlinks cannot be opened as staging or block acquisition.
# No pathname is ever unlinked, including on failure or interruption. Closing
# our descriptor reclaims an unpublished inode, or leaves the published config
# intact. This eliminates the former ownership-check-to-unlink race entirely;
# it does not claim atomic ownership-checked pathname deletion.
# No named-file fallback: missing O_TMPFILE/filesystem support, procfs or linkat
# fails closed. The pinned directory may be renamed by another directory writer;
# publication remains in that directory, not a substituted pathname. Before
# success, verify the configured path still reaches that object and our inode.
# This observation cannot prevent a later external pathname or file change.
# States: none -> attempted/unproven -> owned regular -> published -> closed.
# Ownership is established by O_TMPFILE plus fstat, never an inherited descriptor.
# An unproven descriptor is closed only; no filesystem name needs cleanup.
#
# SIGNALS AND TRUTHFUL REPORTING. The Python worker's handlers for SIGINT, SIGTERM
# and SIGHUP only RECORD the signal; they never raise. An exception raised from a
# handler can fire between linkat() and the bookkeeping that follows it, or inside
# the descriptor cleanup, and then a file that WAS published is reported as a
# failure ("nothing was changed"). Instead the worker checks for a recorded signal
# only at checkpoints that precede linkat(), so every interruption is either
# before publication (nothing was published) or after it (the config is complete).
# Nothing is masked or blocked. The worker's exit status is the only channel back
# here, so each value is exact:
#     0 published                           2 destination exists (not touched)
#     3 published, then a signal arrived    4 a signal arrived and publication
#                                             could not be established
#     5 unsupported environment (nothing    6 refused before publication
#       attempted)                          7 published, configured path unverified
#     130 signal before publication
# Any other status (for example SIGKILL or an interpreter crash) proves nothing
# about the filesystem; it is reported as unconfirmed, never as "nothing changed".
# starter_config_changed: no (provably not published) | unknown (not provable).
starter_config_result=""
starter_config_changed=no
publish_starter_config() {
    psc_src=$1
    psc_dir=$2
    psc_dest=$3
    starter_config_changed=no
    # Reserve fd 6 before any traps or child creation; inherited fd 6 is never
    # evidence of staging ownership. The Python worker owns its own descriptors.
    exec 6>&-
    psc_pid=""
    psc_interrupted=no
    if [ -L "$psc_dir" ] && [ ! -d "$psc_dir" ]; then
        starter_config_result="configuration directory is a dangling symlink"
        return 1
    fi
    # Request 0700 for EVERY missing component. A default ACL replaces umask,
    # but the explicit mode clears its access mask, including named users/groups.
    # Existing components are only inspected, never chmod-ed or re-owned.
    if ! python3 - "$psc_dir" <<'PSC_MKDIR_PY'
import os
import stat
import sys

try:
    current = "/"
    for component in sys.argv[1].split("/"):
        if not component:
            continue
        current = os.path.join(current, component)
        try:
            os.mkdir(current, 0o700)
        except FileExistsError:
            if not os.path.isdir(current):
                raise NotADirectoryError(current)
        else:
            fd = os.open(current, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
            try:
                if stat.S_IMODE(os.fstat(fd).st_mode) & 0o777 != 0o700:
                    raise ValueError("new directory does not have private effective access: " + current)
            finally:
                os.close(fd)
except (OSError, ValueError) as error:
    print("install.sh: cannot create a private configuration directory: " + str(error), file=sys.stderr)
    sys.exit(1)
PSC_MKDIR_PY
    then
        starter_config_result="cannot create the configuration directory privately"
        return 1
    fi
    # A signal before pid assignment records interruption; immediately after
    # assignment we stop the worker too. Always reap it before exiting.
    trap 'psc_interrupted=yes; if [ -n "$psc_pid" ]; then kill -TERM "$psc_pid" 2>/dev/null || true; fi' INT TERM HUP
    # ACQUIRE-WINDOW: no staging descriptor has been acquired.
    if [ "$psc_interrupted" = yes ]; then
        trap - INT TERM HUP
        printf 'install.sh: installation interrupted before starter-config acquisition; no starter config was written; already installed assets remain.\n' >&2
        exit 130
    fi
    python3 - "$psc_src" "$psc_dir" "$psc_dest" 6>&- <<'PSC_PY' &
import errno
import os
import signal
import stat
import sys


def unsupported(reason):
    print("install.sh: cannot write the starter config: " + reason + ". Safe starter-config "
          "publication needs Linux, Python 3 with os.O_TMPFILE and ctypes, a filesystem that "
          "supports O_TMPFILE and hardlinks, and a mounted /proc. Nothing was attempted and no "
          "named staging fallback is used.", file=sys.stderr)
    sys.exit(5)


# CAPABILITY-CHECK: refuse explicitly, before any directory or descriptor is touched.
if not sys.platform.startswith("linux"):
    unsupported("this platform is " + sys.platform + ", not Linux")
for needed in ("O_TMPFILE", "O_DIRECTORY", "O_CLOEXEC"):
    if not hasattr(os, needed):
        unsupported("this Python has no os." + needed)
try:
    import ctypes
    linkat = ctypes.CDLL(None, use_errno=True).linkat
    linkat.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int,
                       ctypes.c_char_p, ctypes.c_int]
    linkat.restype = ctypes.c_int
except (ImportError, OSError, AttributeError) as error:
    unsupported("ctypes or linkat() is unavailable (" + str(error) + ")")


class Interrupted(Exception):
    pass


received = []


def record_signal(signum, frame):
    # Record only: see the SIGNALS note above publish_starter_config.
    received.append(signum)


for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
    signal.signal(sig, record_signal)


def checkpoint():
    # Only called before linkat(): a recorded signal then means "not published".
    if received:
        raise Interrupted()


def explain(error):
    code = getattr(error, "errno", None)
    name = errno.errorcode.get(code, "") if code else ""
    if code in (errno.EOPNOTSUPP, errno.EISDIR, errno.EINVAL) and state == "attempted/unproven":
        why = ("the filesystem holding the configuration directory does not support "
               "anonymous O_TMPFILE files (" + name + ")")
    elif code in (errno.ENOENT, errno.ENOSYS, errno.EACCES, errno.EPERM, errno.EXDEV,
                  errno.EMLINK) and state == "owned regular":
        why = ("descriptor linking through /proc/self/fd is unavailable or not permitted "
               "here: is /proc mounted, and does this filesystem allow hardlinks? (" + name + ")")
    else:
        why = str(error)
    print("install.sh: starter-config publication refused (" + state + "): " + why +
          ". No starter config was published.", file=sys.stderr)


def configured_directory(expected=None):
    """Observe each binding without following a substituted component.

    Existing symlinks are resolved explicitly through their O_PATH descriptors.
    On verification, compare every object before traversing it or its target.
    """
    current = None
    entry = None
    bindings = []
    pending = directory.split("/")
    links = 0
    try:
        current = os.open("/", os.O_PATH | os.O_DIRECTORY | os.O_CLOEXEC)
        root_info = os.fstat(current)
        root_binding = ("/", root_info.st_dev, root_info.st_ino, stat.S_IFMT(root_info.st_mode))
        if expected is not None and root_binding != expected[0]:
            raise ValueError("configuration root binding changed")
        bindings.append(root_binding)
        if expected is None:
            directory_refs.append(os.dup(current))
        while pending:
            name = pending.pop(0)
            if not name or name == ".":
                continue
            entry = os.open(name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=current)
            info = os.fstat(entry)
            binding = (name, info.st_dev, info.st_ino, stat.S_IFMT(info.st_mode))
            index = len(bindings)
            if expected is not None and (index >= len(expected) or binding != expected[index]):
                raise ValueError("configured directory binding changed at " + name)
            bindings.append(binding)
            if expected is None:
                # Keep the observed object alive: device/inode IDs cannot be
                # recycled into a substituted component during this check.
                directory_refs.append(os.dup(entry))
            if stat.S_ISLNK(info.st_mode):
                links += 1
                if links > 40:
                    raise OSError(errno.ELOOP, "too many configuration-directory symlinks")
                target = os.readlink("", dir_fd=entry)
                if target.startswith("/"):
                    root = os.dup(directory_refs[0])
                    os.close(current)
                    current = root
                pending = target.split("/") + pending
                os.close(entry)
                entry = None
            elif stat.S_ISDIR(info.st_mode):
                os.close(current)
                current = entry
                entry = None
            else:
                raise NotADirectoryError(name)
        if expected is not None and len(bindings) != len(expected):
            raise ValueError("configured directory resolution changed")
        result = current
        current = None
        return result, bindings
    finally:
        for fd in (entry, current):
            if fd is not None:
                os.close(fd)


def verify_configured_publication():
    checked, _ = configured_directory(directory_bindings)
    config_fd = None
    try:
        actual = os.fstat(checked)
        if (actual.st_dev, actual.st_ino) != (dir_info.st_dev, dir_info.st_ino):
            raise ValueError("configured path no longer reaches the pinned directory")
        # O_NONBLOCK avoids waiting on a substituted FIFO; O_NOFOLLOW retains
        # substituted symlinks. Opening for read proves effective path access.
        config_fd = os.open(os.path.basename(destination), os.O_RDONLY | os.O_NOFOLLOW |
                            os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=checked)
        actual = os.fstat(config_fd)
        owned = os.fstat(stage_fd)
        if not stat.S_ISREG(actual.st_mode) or (actual.st_dev, actual.st_ino) != (owned.st_dev, owned.st_ino):
            raise ValueError("configured file no longer resolves to the published inode")
    finally:
        if config_fd is not None:
            os.close(config_fd)
        os.close(checked)


src, directory, destination = sys.argv[1:]
state = "none"
stage_fd = None
directory_fd = None
published = False
reachable = False
directory_bindings = None
directory_refs = []
rc = 6
try:
    if os.path.normpath(os.path.dirname(destination)) != os.path.normpath(directory):
        raise ValueError("destination must be inside the configuration directory")
    checkpoint()
    directory_fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    state = "attempted/unproven"
    # DIRECTORY-CHECK: judged on the pinned descriptor (fstat), so the answer is
    # about the exact directory objects we will publish into, whatever its name
    # resolves to later. Never chmod: an unsafe directory is refused as found.
    dir_info = os.fstat(directory_fd)
    if not stat.S_ISDIR(dir_info.st_mode):
        raise ValueError("the configuration path is not a directory")
    if dir_info.st_uid != os.geteuid():
        raise ValueError("the configuration directory is owned by uid " + str(dir_info.st_uid) +
                         ", not by the installing user (uid " + str(os.geteuid()) +
                         "); its owner could replace config.toml. It was not changed. Use a "
                         "directory you own, or set EMUWIZ_CONFIG_HOME")
    if dir_info.st_mode & stat.S_IWOTH:
        raise ValueError("the configuration directory is writable by every user (mode " +
                         format(stat.S_IMODE(dir_info.st_mode), "04o") + "), so any user could "
                         "replace or pre-create config.toml. It was not changed; run "
                         "'chmod go-w' on it yourself, or set EMUWIZ_CONFIG_HOME to a private one")
    if dir_info.st_mode & stat.S_IWGRP:
        print("install.sh: warning: the configuration directory is group-writable (mode " +
              format(stat.S_IMODE(dir_info.st_mode), "04o") + "); members of its group can "
              "replace config.toml. It was left unchanged.", file=sys.stderr)
    observed, directory_bindings = configured_directory()
    try:
        observed_info = os.fstat(observed)
        if (observed_info.st_dev, observed_info.st_ino) != (dir_info.st_dev, dir_info.st_ino):
            raise ValueError("configuration directory changed while acquiring its bindings")
    finally:
        os.close(observed)
    # ANONYMOUS-OPEN-WINDOW: O_TMPFILE has no pre-existing pathname to follow.
    checkpoint()
    stage_fd = os.open(".", os.O_TMPFILE | os.O_RDWR | os.O_CLOEXEC, 0o600,
                       dir_fd=directory_fd)
    # ACQUIRED-WINDOW: even interruption before fstat only closes our new fd.
    info = os.fstat(stage_fd)
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 0:
        raise ValueError("anonymous staging ownership could not be established")
    state = "owned regular"
    # STAGE-WINDOW: bounded descriptor-only content write.
    checkpoint()
    with open(src, "rb") as source:
        while True:
            checkpoint()
            chunk = source.read(65536)
            if not chunk:
                break
            remaining = memoryview(chunk)
            while remaining:
                written = os.write(stage_fd, remaining)
                if written <= 0:
                    raise OSError("short staging write")
                remaining = remaining[written:]
    os.fsync(stage_fd)
    # PUBLISH-WINDOW: linkat is no-clobber, including dangling symlinks.
    checkpoint()
    while True:
        failed = linkat(-100, os.fsencode("/proc/self/fd/" + str(stage_fd)),
                        directory_fd, os.fsencode(os.path.basename(destination)), 0x400)
        failure = ctypes.get_errno() if failed else 0
        if not (failed and failure == errno.EINTR):
            break
    if failed:
        if failure == errno.EEXIST:
            rc = 2
        else:
            raise OSError(failure, "cannot publish anonymous starter config")
    else:
        published = True
        state = "published"
        # PUBLISHED-WINDOW: interruption leaves the complete published config.
        rc = 3 if received else 0
except Interrupted:
    rc = 130
except (OSError, ValueError, AttributeError) as error:
    explain(error)
    rc = 6
finally:
    # CLOSE-WINDOW: never unlink a path; only close descriptors we acquired.
    # Ground truth for "was it published?" is the inode we proved anonymous: it
    # has a name (st_nlink > 0) if and only if our linkat() succeeded.
    # An unproven descriptor (state "attempted/unproven") is never consulted: it may be
    # a foreign object that legitimately has a name.
    if not published and state == "owned regular":
        try:
            published = os.fstat(stage_fd).st_nlink > 0
        except OSError:
            rc = 4
    if published:
        try:
            verify_configured_publication()
            reachable = True
        except (OSError, ValueError, AttributeError) as error:
            print("install.sh: UNCERTAIN: the config was linked in the pinned directory, but "
                  "its configured pathname could not be verified (" + str(error) + "). "
                  "Already installed assets remain. Inspect " + destination + " and the original "
                  "directory before retrying; no path was replaced or removed.", file=sys.stderr)
            rc = 7
    for fd in (stage_fd, directory_fd, *directory_refs):
        if fd is not None:
            try:
                os.close(fd)
            except OSError:
                pass
            # CLOSE-LOOP-WINDOW: a signal here changes nothing about the outcome.
    state = "closed"
if published:
    rc = (3 if received else 0) if reachable else (4 if received else 7)
elif received and rc != 4:
    rc = 130
sys.exit(rc)
PSC_PY
    psc_pid=$!
    if [ "$psc_interrupted" = yes ]; then kill -TERM "$psc_pid" 2>/dev/null || true; fi
    psc_rc=0
    wait "$psc_pid" || psc_rc=$?
    if [ "$psc_interrupted" = yes ]; then
        # The first wait may have been cut short to run the trap. Collect the
        # worker's real status (127 means it was already reaped); never invent one.
        psc_rc2=0
        wait "$psc_pid" 2>/dev/null || psc_rc2=$?
        if [ "$psc_rc2" -ne 127 ]; then psc_rc=$psc_rc2; fi
    fi
    trap - INT TERM HUP
    psc_pid=""
    # An interrupted outcome is reported in exactly one place. A signal that reached
    # THIS shell still ends the installation whatever the worker reported; a signal
    # that reached only the worker shows up as status 3, 4 or 130.
    psc_outcome=""
    case "$psc_rc" in
        3) psc_outcome=published ;;
        4) psc_outcome=uncertain ;;
        7) if [ "$psc_interrupted" = yes ]; then psc_outcome=uncertain; fi ;;
        130) psc_outcome=unpublished ;;
        0) if [ "$psc_interrupted" = yes ]; then psc_outcome=published; fi ;;
        2|5|6) if [ "$psc_interrupted" = yes ]; then psc_outcome=unpublished; fi ;;
    esac
    if [ -n "$psc_outcome" ]; then
        case "$psc_outcome" in
            published)
                printf 'install.sh: installation interrupted; the starter config was published and is complete: %s; already installed assets remain and the remaining installation steps did not run.\n' "$psc_dest" >&2
                ;;
            uncertain)
                printf 'install.sh: installation interrupted; it could not be established whether the starter config was published, so inspect %s before retrying; already installed assets remain.\n' "$psc_dest" >&2
                ;;
            *)
                printf 'install.sh: installation interrupted before the starter config was published; nothing was written at %s; already installed assets remain.\n' "$psc_dest" >&2
                ;;
        esac
        exit 130
    fi
    case "$psc_rc" in
        0) starter_config_result=""; return 0 ;;
        2) starter_config_result="exists"; return 1 ;;
        7)
            starter_config_changed=unknown
            starter_config_result="UNCERTAIN: configured pathname could not be verified after publication"
            return 1
            ;;
        5)
            starter_config_result="unsupported environment, see the explanation above"
            return 1
            ;;
        6)
            starter_config_result="cannot publish the starter config safely, see the explanation above"
            return 1
            ;;
        *)
            # A status the worker never reports (SIGKILL, interpreter failure, stray
            # signal). It proves nothing about the filesystem.
            starter_config_changed=unknown
            if [ -e "$psc_dest" ] || [ -L "$psc_dest" ]; then
                starter_config_result="the starter-config worker ended unexpectedly (status $psc_rc) and a file is present at $psc_dest; it could not be established whether this installer published it"
            else
                starter_config_result="the starter-config worker ended unexpectedly (status $psc_rc); no file is present at $psc_dest"
            fi
            if [ "$psc_interrupted" = yes ]; then
                printf 'install.sh: installation interrupted; %s; already installed assets remain.\n' "$starter_config_result" >&2
                exit 130
            fi
            return 1
            ;;
    esac
}

# warn_exposed_existing_config - an existing configuration is never modified,
# replaced, re-owned or re-permissioned. If the directory holding it, or the file
# itself, is not owned by the installing user, or the directory is writable by every
# user, say so (stderr only; the exit status does not change).
warn_exposed_existing_config() {
    if [ -d "$config_dir" ]; then
        if [ ! -O "$config_dir" ]; then
            printf '%s: warning: %s is not owned by you; its owner can replace your configuration. It was left unchanged.\n' "$program_name" "$config_dir" >&2
        elif [ -n "$(find "$config_dir" -prune -perm -002 2>/dev/null)" ]; then
            printf '%s: warning: %s is writable by every user, so any user can replace your configuration. It was left unchanged; consider "chmod go-w" on it.\n' "$program_name" "$config_dir" >&2
        fi
    fi
    if [ -f "$config_file" ] && [ ! -L "$config_file" ] && [ ! -O "$config_file" ]; then
        printf '%s: warning: %s is not owned by you. It was left unchanged; check it before relying on it.\n' "$program_name" "$config_file" >&2
    fi
}

# Prefer an extracted release bundle (binaries in bin/); fall
# back to a workspace checkout with release binaries already built. The new
# EmuWiz names are preferred, with the legacy ArchiveFS names accepted so
# older release bundles still install.
if [ -f "$script_dir/bin/emuwiz-cli" ] || [ -f "$script_dir/bin/emuwiz" ]; then
    src_mode="release bundle (bin layout)"
    src_dir="$script_dir/bin"
elif [ -f "$script_dir/emuwiz-cli" ] || [ -f "$script_dir/emuwiz" ]; then
    src_mode="release bundle"
    src_dir="$script_dir"
elif [ -f "$script_dir/archivefs-cli" ] || [ -f "$script_dir/archivefs-gui" ]; then
    src_mode="release bundle (legacy names)"
    src_dir="$script_dir"
elif [ -f "$script_dir/target/release/emuwiz-cli" ] || [ -f "$script_dir/target/release/emuwiz" ]; then
    src_mode="workspace release build"
    src_dir="$script_dir/target/release"
elif [ -f "$script_dir/target/release/archivefs-cli" ] || [ -f "$script_dir/target/release/archivefs-gui" ]; then
    src_mode="workspace release build (legacy names)"
    src_dir="$script_dir/target/release"
else
    fail "could not find the emuwiz/emuwiz-cli binaries (or the legacy archivefs-cli/archivefs-gui) next to this script ($script_dir) or under target/release/. If you are in a workspace checkout, run 'cargo build --workspace --release' first."
fi

# Resolve the CLI and GUI sources, accepting either the new or the legacy
# name so old bundles install unchanged.
src_cli=""
for candidate in emuwiz-cli archivefs-cli; do
    if [ -f "$src_dir/$candidate" ]; then
        src_cli="$src_dir/$candidate"
        break
    fi
done
src_gui=""
for candidate in emuwiz archivefs-gui; do
    if [ -f "$src_dir/$candidate" ]; then
        src_gui="$src_dir/$candidate"
        break
    fi
done
missing=""
[ -n "$src_cli" ] || missing="$missing emuwiz-cli/archivefs-cli"
[ -n "$src_gui" ] || missing="$missing emuwiz/archivefs-gui"
if [ -n "$missing" ]; then
    fail "missing required binaries in $src_dir:$missing"
fi

[ -f "$desktop_template" ] || fail "desktop entry template is missing: $desktop_template"
for size in 32 64 128 256 512; do
    [ -f "$branding_dir/emuwiz-logo-$size.png" ] || \
        fail "approved application icon is missing: $branding_dir/emuwiz-logo-$size.png"
done

# RELEASE INTEGRITY. A packaged release ships SHA256SUMS covering every file,
# including install.sh and both binaries. When it is present every record is
# checked before anything is created or changed: strict record format, relative
# paths with no "..", no symlink in any component, regular files only, and
# matching digests. The two binaries are copied into a private snapshot while
# being hashed, and only those verified bytes are installed, so a binary
# changed in the extracted tree after the check cannot be published. A tree
# with manifest.json but no SHA256SUMS is a damaged package and is always
# refused. A tree with neither (workspace builds, legacy bundles) is refused
# unless --developer-unchecked-install is given; that mode skips only the
# digest comparison - binaries are still reached without following symlinks,
# must be regular files, are installed from the snapshot, and a prominent
# warning is printed. The flag never relaxes a tree that has SHA256SUMS or
# manifest.json. This detects damaged
# or modified packages; it is not a signature check (install.sh itself comes
# from the same tree).
release_check() {
    python3 - "$@" <<'EMUWIZ_RELEASE_PY'
import hashlib
import os
import re
import stat
import sys

root, snapshot, cli, gui, mode = sys.argv[1:6]


def refuse(message):
    what = "developer unchecked install refused" if mode == "unchecked" else "release integrity check failed"
    print(f"install.sh: {what}: {message}; nothing was installed or changed", file=sys.stderr)
    sys.exit(1)


def open_relative(base, relative):
    parts = relative.split("/")
    if relative.startswith("/") or any(part in ("", ".", "..") for part in parts):
        refuse(f"SHA256SUMS names an unsafe path: {relative!r}")
    fd = os.dup(base)
    try:
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        target = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
    except OSError as error:
        refuse(f"{relative} cannot be opened without following a link ({error.strerror})")
    finally:
        os.close(fd)
    if not stat.S_ISREG(os.fstat(target).st_mode):
        os.close(target)
        refuse(f"{relative} is not a regular file")
    return target


def snapshot_copy(fd, relative):
    name = "emuwiz-cli" if relative == cli else "emuwiz"
    return os.fdopen(os.open(os.path.join(snapshot, name),
                             os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o700), "wb")


if mode == "unchecked":
    # Developer mode skips only the digest comparison. The binaries are still
    # reached without following any symlink, must be regular files, and are
    # installed from the private snapshot taken here.
    base = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    for relative in (cli, gui):
        fd = open_relative(base, relative)
        try:
            with os.fdopen(fd, "rb") as stream, snapshot_copy(fd, relative) as copy:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    copy.write(chunk)
        except OSError as error:
            refuse(f"{relative} could not be read or copied ({error})")
    print(2)
    sys.exit(0)

try:
    base = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    sums = os.open("SHA256SUMS", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=base)
    if not stat.S_ISREG(os.fstat(sums).st_mode):
        refuse("SHA256SUMS is not a regular file")
    with os.fdopen(sums, "rb") as stream:
        text = stream.read().decode("utf-8")
except (OSError, UnicodeDecodeError) as error:
    refuse(f"SHA256SUMS cannot be read safely ({error})")

records = {}
for line in text.splitlines():
    match = re.fullmatch(r"([0-9a-f]{64})  ([^\r\n]+)", line)
    if not match or match.group(2) in records:
        refuse("SHA256SUMS contains an invalid or duplicate record")
    records[match.group(2)] = match.group(1)
for required in ("install.sh", cli, gui):
    if required not in records:
        refuse(f"SHA256SUMS does not cover {required}")

for relative, expected in sorted(records.items()):
    fd = open_relative(base, relative)
    copy = None
    if relative in (cli, gui):
        copy = snapshot_copy(fd, relative)
    digest = hashlib.sha256()
    try:
        with os.fdopen(fd, "rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
                if copy is not None:
                    copy.write(chunk)
        if copy is not None:
            copy.close()
    except OSError as error:
        refuse(f"{relative} could not be read or copied ({error})")
    if digest.hexdigest() != expected:
        refuse(f"{relative} does not match SHA256SUMS (the package is damaged or was modified)")
print(len(records))
EMUWIZ_RELEASE_PY
}

release_verified=0
if [ -e "$script_dir/SHA256SUMS" ] || [ -L "$script_dir/SHA256SUMS" ]; then
    release_mode=verified
elif [ -e "$script_dir/manifest.json" ] || [ -L "$script_dir/manifest.json" ]; then
    # A release manifest is never downgraded to a developer bundle.
    fail "this release package has manifest.json but no SHA256SUMS, so it is incomplete or damaged; nothing was installed or changed. Download and extract it again."
elif [ "$developer_unchecked" -eq 1 ]; then
    release_mode=unchecked
else
    fail "this $src_mode ships no SHA256SUMS, so its binaries cannot be verified; nothing was installed or changed. Official EmuWiz releases always include SHA256SUMS: download and extract the release again. For a local developer build only, re-run with --developer-unchecked-install."
fi
command -v python3 >/dev/null 2>&1 || fail "python3 is required to check the release"
release_snapshot=$(mktemp -d -- "${TMPDIR:-/tmp}/emuwiz-verified.XXXXXXXX") ||
    fail "could not create a private directory for the binaries to install"
trap 'rm -rf -- "$release_snapshot"' EXIT
verified_count=$(release_check "$script_dir" "$release_snapshot" \
    "${src_cli#"$script_dir"/}" "${src_gui#"$script_dir"/}" "$release_mode") || exit 1
src_cli="$release_snapshot/emuwiz-cli"
src_gui="$release_snapshot/emuwiz"
if [ "$release_mode" = verified ]; then
    release_verified=1
    printf 'Verified %s release files against SHA256SUMS.\n' "$verified_count"
else
    printf '%s: WARNING: DEVELOPER UNCHECKED INSTALL. --developer-unchecked-install was given and this %s has no SHA256SUMS, so the binaries being installed were NOT verified. Never use this for a downloaded release.\n' \
        "$program_name" "$src_mode" >&2
fi

mkdir -p -- "$bin_dir"
bin_dir=$(CDPATH= cd -- "$bin_dir" && pwd -P) || fail "could not resolve install prefix: $bin_dir"

# This must happen before copying or publishing any managed destination.  The
# prefix directory itself may be created above so its canonical path can be
# compared with a prior manifest; no managed file has been touched yet.
command -v python3 >/dev/null 2>&1 || fail "python3 is required for safe Linux asset publication"
preflight_install_manifest

# preflight_binary_slot SLOT SRC DEST - refuses, before anything is published,
# when an EmuWiz-owned binary whose bytes would change cannot be updated in place
# because it is running. Foreign or absent destinations need no in-place update.
preflight_binary_slot() {
    slot=$1
    src=$2
    dest=$3
    assert_manifest_binding
    prepare_asset "$dest"
    if [ "$(gate_binary "$slot" "$asset_bound")" = owned ]; then
        asset_io preflight "$src" "$asset_name" "$asset_state" "$asset_digest" \
            "$asset_parent" "$asset_location" || exit 1
    fi
    exec 7<&-
}

# Every changed owned binary is checked before the first publication, so a
# running GUI cannot leave the CLI published but unrecorded.
preflight_binary_slot bin-emuwiz-cli "$src_cli" "$bin_dir/emuwiz-cli"
preflight_binary_slot bin-emuwiz "$src_gui" "$bin_dir/emuwiz"

# preserve_binary_slot SLOT SRC DEST - keeps a durable, verified copy of an
# owned binary whose bytes are about to change, and journals it, before the
# first publication of this run. Absent, foreign and unchanged binaries need
# no copy. Any doubt (space, filesystem support, a changed destination)
# refuses here with nothing installed.
preserved_copies=""
preserve_binary_slot() {
    slot=$1
    src=$2
    dest=$3
    assert_manifest_binding
    prepare_asset "$dest"
    if [ "$(gate_binary "$slot" "$asset_bound")" = owned ]; then
        kept=$(asset_io preserve "$src" "$asset_name" "$asset_state" "$asset_digest" \
            "$asset_parent" "$asset_location" "$slot" "$binding_location") || exit 1
        if [ -n "$kept" ]; then
            preserved_copies="$preserved_copies  $dest -> $kept
"
        fi
    fi
    assert_manifest_binding
    exec 7<&-
}
preserve_binary_slot bin-emuwiz-cli "$src_cli" "$bin_dir/emuwiz-cli"
preserve_binary_slot bin-emuwiz "$src_gui" "$bin_dir/emuwiz"

manifest_records_tmp=$(mktemp -- "/proc/self/fd/8/.manifest-records.XXXXXX") ||
    fail "could not create a temporary file for the install manifest"

skipped_any=0

# install_binary_slot SLOT SRC DEST - copies SRC to DEST and marks it
# executable, unless DEST is occupied by something foreign (in which case
# it is skipped, or backed up first with --replace-foreign).
install_binary_slot() {
    slot=$1
    src=$2
    dest=$3
    assert_manifest_binding
    prepare_asset "$dest"
    gate=$(gate_binary "$slot" "$asset_bound")
    if [ "$gate" = foreign ]; then
        resolve_asset_collision || return 0
        gate=absent
    fi
    assert_manifest_binding
    binary_tmp=$(mktemp -- "/proc/self/fd/7/.emuwiz-binary.XXXXXX") || fail "could not stage binary"
    cp -- "$src" "$binary_tmp"
    chmod +x -- "$binary_tmp"
    publish_asset file "$slot" "$binary_tmp" "$gate"
    rm -f -- "$binary_tmp"
    exec 7<&-
}

# install_alias_slot SLOT TARGET DEST - creates the symlink DEST -> TARGET,
# unless DEST is occupied by something foreign.
install_alias_slot() {
    slot=$1
    target=$2
    dest=$3
    assert_manifest_binding
    prepare_asset "$dest"
    gate=$(gate_alias "$slot" "$asset_bound" "$target")
    if [ "$gate" = foreign ]; then
        resolve_asset_collision || return 0
        gate=absent
    fi
    publish_asset symlink "$slot" "$target" "$gate"
    exec 7<&-
}

install_binary_slot bin-emuwiz-cli "$src_cli" "$bin_dir/emuwiz-cli"
install_binary_slot bin-emuwiz "$src_gui" "$bin_dir/emuwiz"
# Legacy compatibility aliases: scripts and muscle memory that call
# `archivefs-cli` / `archivefs-gui` keep working.
install_alias_slot alias-archivefs-cli emuwiz-cli "$bin_dir/archivefs-cli"
install_alias_slot alias-emuwiz-gui emuwiz "$bin_dir/emuwiz-gui"
install_alias_slot alias-archivefs-gui emuwiz "$bin_dir/archivefs-gui"
printf 'Installed emuwiz-cli and emuwiz to %s (source: %s)\n' "$bin_dir" "$src_mode"
printf 'Aliases emuwiz-gui, archivefs-cli and archivefs-gui still work.\n'
if [ "$release_verified" -ne 1 ]; then
    printf 'Note: developer unchecked install - these binaries were not verified against a release checksum list.\n'
fi

# Desktop Entry Exec values have their own quoting rules. Reject line breaks,
# encode literal percent signs so they cannot become field codes, and escape
# the four characters that are special inside a double-quoted argument.
#
# Backslash is doubled to *four* backslashes, not two: a Desktop Entry
# string-type value (Exec included) is first run through the format's
# generic backslash-unescape pass (which recognises only \\, \s, \n, \t,
# \r) before the Exec-specific quoting rules below are ever applied to it.
# That generic pass collapses "\\" down to one "\" before an Exec parser
# sees it, so to have *one* backslash survive as the Exec-quoted escape
# sequence "\\" (which itself decodes to one literal backslash), the file
# must contain "\\\\". Two backslashes here would collapse to a single
# stray "\" that desktop-file-validate then folds into whatever character
# follows it, breaking quote tracking for the rest of the value - which is
# exactly what upstream desktop-file-utils' own parser documents in
# src/validate.c above handle_exec_key().
case "$bin_dir/emuwiz" in
    *'
'*) fail "the install prefix cannot contain a line break" ;;
esac
desktop_exec=$(printf '%s' "$bin_dir/emuwiz" | sed \
    -e 's/\\/\\\\\\\\/g' \
    -e 's/"/\\"/g' \
    -e 's/`/\\`/g' \
    -e 's/\$/\\$/g' \
    -e 's/%/%%/g')
desktop_exec="\"$desktop_exec\""

mkdir -p -- "$data_home/applications"
prepare_asset "$desktop_file"
# Render the candidate into a temp file first, before any ownership
# decision, so the "does the existing file match what we'd write" gate and
# the eventual publication always compares against and uses the exact
# same rendered bytes.
desktop_tmp=$(mktemp -- "/proc/self/fd/7/.$desktop_id.XXXXXX.desktop") ||
    fail "could not create a temporary file for the desktop entry"
while IFS= read -r line || [ -n "$line" ]; do
    if [ "$line" = 'Exec=@EMUWIZ_EXEC@' ]; then
        printf 'Exec=%s\n' "$desktop_exec"
    else
        printf '%s\n' "$line"
    fi
done <"$desktop_template" >"$desktop_tmp"
chmod 0644 "$desktop_tmp"
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$desktop_tmp" || {
        rm -f -- "$desktop_tmp"
        fail "rendered desktop entry failed desktop-file-validate"
    }
fi

assert_manifest_binding
desktop_gate=$(gate_content desktop "$asset_bound" "$desktop_tmp" exec)
desktop_installed=0
if [ "$desktop_gate" = foreign ]; then
    if resolve_asset_collision; then
        desktop_gate=absent
        desktop_installed=1
    fi
else
    desktop_installed=1
fi
if [ "$desktop_installed" -eq 1 ]; then
    publish_asset file desktop "$desktop_tmp" "$desktop_gate"
    rm -f -- "$desktop_tmp"
    printf 'Installed the EmuWiz desktop launcher below %s.\n' "$data_home"
else
    rm -f -- "$desktop_tmp"
fi
exec 7<&-

for size in 32 64 128 256 512; do
    icon_dir="$data_home/icons/hicolor/${size}x${size}/apps"
    mkdir -p -- "$icon_dir"
    icon_dest="$icon_dir/$desktop_id.png"
    icon_source="$branding_dir/emuwiz-logo-$size.png"
    assert_manifest_binding
    prepare_asset "$icon_dest"
    icon_gate=$(gate_content "icon-$size" "$asset_bound" "$icon_source" "")
    if [ "$icon_gate" = foreign ]; then
        resolve_asset_collision || continue
        icon_gate=absent
    fi
    icon_tmp=$(mktemp -- "/proc/self/fd/7/.$desktop_id.XXXXXX.png") ||
        fail "could not create a temporary file for the $size pixel icon"
    cp -- "$icon_source" "$icon_tmp"
    chmod 0644 "$icon_tmp"
    publish_asset file "icon-$size" "$icon_tmp" "$icon_gate"
    rm -f -- "$icon_tmp"
    exec 7<&-
done
printf 'Installed the EmuWiz application icons below %s.\n' "$data_home"

# Compose complete metadata in the pinned directory, then recheck the binding.
manifest_tmp=$(mktemp -- "/proc/self/fd/8/.manifest.XXXXXX") ||
    fail "could not create a temporary file for the install manifest"
record_count=$(wc -l <"$manifest_records_tmp" | tr -d '[:space:]')
{
    printf '# EmuWiz installer ownership manifest - machine-generated, do not hand-edit.\n'
    printf '# Regenerated in full on every successful install/reinstall.\n'
    printf 'schema_version %s\n' "$manifest_schema_version"
    printf 'bin_dir %s\n' "$bin_dir"
    printf 'data_home %s\n' "$data_home"
    printf 'record_count %s\n' "$record_count"
    cat -- "$manifest_records_tmp"
    printf 'end\n'
} >"$manifest_tmp"
rm -f -- "$manifest_records_tmp"
chmod 0600 "$manifest_tmp"
published_manifest_digest=$(file_digest "$manifest_tmp") || fail "cannot fingerprint new manifest"

# No pathname rename over an existing manifest: write only to the validated
# open inode. Recheck afterward too, reporting late replacement without touching
# its object. This write is deliberately non-atomic: interruption may truncate
# this owned manifest, which the strict parser rejects on the next run.
assert_manifest_binding
if [ "$binding_manifest" = absent ]; then
    # Atomic no-clobber creation also closes the check-to-create window.
    ln -T -- "$manifest_tmp" /proc/self/fd/8/manifest ||
        fail "ownership manifest creation collided; published assets may remain unrecorded: $manifest_file"
    exec 9<"$manifest_tmp"
else
    cat -- "$manifest_tmp" > /proc/self/fd/9 ||
        fail "could not update owned manifest; published assets may remain unrecorded"
    chmod 0600 /proc/self/fd/9
fi
binding_manifest=$(manifest_descriptor_state) || fail "cannot identify updated manifest"
binding_digest=$published_manifest_digest
assert_manifest_binding
rm -f -- "$manifest_tmp"
# The manifest now records the new digests, so the upgrade journal is done.
# The preserved copies themselves are never deleted by this installer.
if [ -e /proc/self/fd/8/upgrade-pending ] || [ -L /proc/self/fd/8/upgrade-pending ]; then
    asset_io finish || exit 1
fi
exec 9<&-
exec 8<&-
if [ -n "$preserved_copies" ]; then
    printf 'Kept the previous version of each upgraded binary:\n%s' "$preserved_copies"
    printf 'They are never removed automatically; delete them yourself once the new version works.\n'
fi

if [ "$skipped_any" -eq 1 ]; then
    printf '\n' >&2
    printf 'One or more destinations were left untouched because they are not\n' >&2
    printf 'provably EmuWiz-owned; see the warnings above. Re-run with the\n' >&2
    printf 'flag "--replace-foreign" to move them aside and install there anyway.\n' >&2
fi

wrote_new_config=0
if [ -z "$config_root" ]; then
    printf '%s: warning: %s; no starter config was written.\n' "$program_name" "$config_unresolved_reason" >&2
    printf 'Set it to an absolute path (or unset it) and create your config yourself.\n' >&2
    printf '%s: note: the binaries and desktop files were installed; only the starter config step was skipped.\n' "$program_name" >&2
    config_step_failed=1
elif [ -e "$config_file" ] || [ -L "$config_file" ]; then
    # -L as well as -e: a dangling symlink is an occupied name, and writing
    # through it would create a file at wherever it points.
    printf 'Existing config found at %s - leaving it untouched.\n' "$config_file"
    warn_exposed_existing_config
else
    src_config_example="$script_dir/config.toml.example"
    if [ -f "$src_config_example" ]; then
        if publish_starter_config "$src_config_example" "$config_dir" "$config_file"; then
            wrote_new_config=1
            printf 'Wrote a starter config to %s - edit mount_root before running doctor.\n' "$config_file"
            printf 'No source folders are configured yet; that is fine to run with.\n'
        else
            case "$starter_config_result" in
                exists)
                    printf 'Existing config found at %s - leaving it untouched.\n' "$config_file"
                    warn_exposed_existing_config
                    ;;
                *)
                    if [ "$starter_config_changed" = no ]; then
                        printf '%s: warning: could not write a starter config to %s (%s); nothing was changed there.\n' \
                            "$program_name" "$config_file" "$starter_config_result" >&2
                    else
                        # Not provable either way: never claim that nothing changed.
                        printf '%s: warning: the starter config step for %s did not finish cleanly (%s). Inspect that path before retrying.\n' \
                            "$program_name" "$config_file" "$starter_config_result" >&2
                    fi
                    printf '%s: note: the binaries, launchers and assets were already installed and remain in place; only the starter config step failed. To create the config yourself, copy config.toml.example to %s.\n' \
                        "$program_name" "$config_file" >&2
                    config_step_failed=1
                    ;;
            esac
        fi
    else
        printf '%s: warning: config.toml.example not found next to this script; no starter config was written.\n' "$program_name" >&2
        printf 'Create %s yourself before running emuwiz-cli.\n' "$config_file" >&2
    fi
fi

# Optional first source folder - only offered right after writing a brand
# new config (never for an existing one, which is left untouched above),
# and only when this script has an interactive terminal to prompt on
# (never for a piped/non-interactive install, e.g. `curl ... | sh`).
# Deliberately does not validate the path itself in shell: it is handed
# straight to the just-installed `emuwiz-cli source add`, the exact
# same validated, atomically-persisting function the GUI's Sources page
# and every other source-management entry point uses - no second,
# shell-side implementation of that validation. Skipping (blank input) is
# always safe; more sources can be added the same way, or from the
# Sources page in the GUI, at any time after this script exits.
if [ "$wrote_new_config" -eq 1 ] && [ -t 0 ] && [ -r /dev/tty ]; then
    printf '\n'
    printf 'Add your first archive source folder now? Leave blank to skip and add one\n'
    printf 'later (Sources page in the GUI, or: emuwiz-cli source add PATH).\n'
    printf 'Source folder path: '
    # shellcheck disable=SC2039 # `read -r` is supported by every /bin/sh this script targets.
    if IFS= read -r first_source_path < /dev/tty && [ -n "$first_source_path" ]; then
        if "$bin_dir/emuwiz-cli" source add "$first_source_path"; then
            printf 'Added. Scan it from the Sources page, or: emuwiz-cli source scan "%s"\n' "$first_source_path"
        else
            printf 'Could not add that source folder; you can retry later from the Sources page or: emuwiz-cli source add PATH\n' >&2
        fi
    else
        printf 'Skipped - add one later from the Sources page, or: emuwiz-cli source add PATH\n'
    fi
fi

if command -v ratarmount >/dev/null 2>&1; then
    printf 'ratarmount found: %s\n' "$(command -v ratarmount)"
else
    printf '\n' >&2
    printf 'WARNING: ratarmount was not found on PATH.\n' >&2
    printf 'EmuWiz uses ratarmount to mount archives read-only, and mounting\n' >&2
    printf 'will not work until it is installed.\n' >&2
    printf 'Install it with: pip install ratarmount\n' >&2
    printf '(see https://github.com/mxmlnkn/ratarmount for other options,\n' >&2
    printf 'including a portable AppImage build that needs no installation.)\n' >&2
fi

path_has_bin_dir=0
case ":$PATH:" in
    *":$bin_dir:"*) path_has_bin_dir=1 ;;
esac

printf '\n'
printf 'Next steps:\n'
printf '  1. Edit %s (mount_root). Source folders can be managed entirely\n' "$config_file"
printf '     from the app from now on - see the Sources page in the GUI, or\n'
printf '     emuwiz-cli sources / source add / source scan.\n'
if [ "$path_has_bin_dir" -eq 1 ]; then
    printf '  2. Run: emuwiz-cli doctor\n'
    printf '  3. Run: emuwiz-cli config-check\n'
    printf '  4. Launch the GUI: emuwiz\n'
else
    printf '  2. %s is not on your PATH yet. Add it yourself (this script\n' "$bin_dir"
    printf '     will not edit your shell startup files), for example by adding\n'
    printf '     this line to ~/.bashrc or ~/.zshrc:\n'
    printf '       export PATH="%s:$PATH"\n' "$bin_dir"
    printf '     Until then, use the full path:\n'
    printf '  3. Run: %s/emuwiz-cli doctor\n' "$bin_dir"
    printf '  4. Run: %s/emuwiz-cli config-check\n' "$bin_dir"
    printf '  5. Launch the GUI: %s/emuwiz\n' "$bin_dir"
fi

printf '\n'
printf 'To uninstall later: %s/install.sh --uninstall' "$script_dir"
if [ "$bin_dir" != "$default_bin_dir" ]; then
    printf ' --prefix %s' "$bin_dir"
fi
printf '\n'

if [ "$skipped_any" -eq 1 ] || [ "$config_step_failed" -eq 1 ]; then
    exit 1
fi
