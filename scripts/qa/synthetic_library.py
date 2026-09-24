#!/usr/bin/env python3
"""Build, validate, scan, and remove EmuWiz's legal synthetic QA library."""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import shutil
import sqlite3
import stat
import struct
import subprocess
import sys
import tempfile
import time
import warnings
import zipfile
import zlib
import xml.etree.ElementTree as ET
from pathlib import Path, PurePosixPath
from typing import Any, Iterable

SCHEMA_VERSION = 1
GENERATOR_VERSION = "1.0.0"
OWNERSHIP_TOKEN = "emuwiz-synthetic-library-lab-v1"
DEFAULT_ROOT = Path(f"/tmp/emuwiz-synthetic-library-v{SCHEMA_VERSION}")
MARKER = ".emuwiz-synthetic-lab.json"
INCOMPLETE = ".emuwiz-synthetic-lab.incomplete.json"
LOCK = ".emuwiz-synthetic-lab.lock"
OPERATIONAL_FILES = {MARKER, "manifest.json", "MANIFEST.md", "report.json", "report.md"}
PROFILE_LEVEL = {"tiny": 0, "standard": 1, "full": 2}
PROFILE_CAP = {"tiny": 20 * 1024 * 1024, "standard": 100 * 1024 * 1024, "full": 500 * 1024 * 1024}

CLASSIFICATIONS = {
    "Playable", "Support", "Bios", "Firmware", "Device", "Artwork", "Metadata",
    "Cheat", "Mod", "Unknown", "Unsupported", "Malformed", "Ambiguous",
}
IDENTITIES = {"Verified", "StrongCandidate", "WeakCandidate", "Unknown", "Conflicting"}
HEALTH = {
    "Healthy", "Incomplete", "Malformed", "Unsupported", "MissingCompanion",
    "WrongExtension", "UnsafePath", "Duplicate", "NeedsReview",
}
EXPECTATIONS = {"ExactAssertion", "Invariant", "DiagnosticOnly", "ManualReviewExpected"}


class LabError(RuntimeError):
    pass


def canonical_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def deterministic_timestamp(seed: int) -> str:
    # Deliberately synthetic: manifests stay byte-identical for the same seed.
    return f"2000-01-{1 + (seed % 28):02d}T00:00:00Z"


def repository_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=Path(__file__).resolve().parents[2], text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def dangerous_roots() -> set[Path]:
    roots = {Path("/"), Path("/home"), Path("/mnt"), Path("/tmp")}
    home = os.environ.get("HOME")
    if home:
        roots.add(Path(home).resolve())
    return roots


def validate_output_root(path: Path, *, cleanup: bool = False) -> Path:
    if not path.is_absolute():
        raise LabError("output root must be an absolute path")
    if path.is_symlink():
        raise LabError("output root must not be a symlink")
    resolved = path.resolve(strict=False)
    if resolved in dangerous_roots():
        raise LabError(f"refusing dangerous output root: {resolved}")
    if resolved == Path("/tmp") or resolved.parent == Path("/"):
        raise LabError("output root must be a dedicated child directory")
    if cleanup and resolved.exists() and os.path.ismount(resolved):
        raise LabError("refusing to remove a mount point")
    return resolved


def safe_relative(value: str) -> str:
    posix = PurePosixPath(value)
    if not value or posix.is_absolute() or ".." in posix.parts or "" in posix.parts:
        raise LabError(f"unsafe fixture path: {value!r}")
    if "\\" in value:
        raise LabError(f"backslash is not allowed in fixture path: {value!r}")
    return posix.as_posix()


def payload(fixture_id: str, seed: int, version: str, size: int) -> bytes:
    result = bytearray()
    counter = 0
    while len(result) < size:
        result.extend(hashlib.sha256(f"{version}\0{seed}\0{fixture_id}\0{counter}".encode()).digest())
        counter += 1
    return bytes(result[:size])


def png_bytes(width: int, height: int, rgb: tuple[int, int, int]) -> bytes:
    signature = b"\x89PNG\r\n\x1a\n"
    def chunk(name: bytes, data: bytes) -> bytes:
        body = name + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    row = b"\x00" + bytes(rgb) * width
    raw = row * height
    return signature + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def zip_bytes(members: list[tuple[str, bytes]], *, truncate: bool = False) -> bytes:
    import io
    stream = io.BytesIO()
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", UserWarning)
        with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for name, data in members:
                info = zipfile.ZipInfo(name, date_time=(2000, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o100644 << 16
                archive.writestr(info, data)
    value = stream.getvalue()
    return value[:-11] if truncate else value


def zip_symlink_bytes(name: str, target: str) -> bytes:
    import io
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_STORED) as archive:
        info = zipfile.ZipInfo(name, date_time=(2000, 1, 1, 0, 0, 0))
        info.create_system = 3
        info.external_attr = (stat.S_IFLNK | 0o777) << 16
        archive.writestr(info, target.encode("utf-8"))
    return stream.getvalue()


class Builder:
    def __init__(self, root: Path, profile: str, seed: int, commit: str):
        self.root = root
        self.profile = profile
        self.level = PROFILE_LEVEL[profile]
        self.seed = seed
        self.commit = commit
        self.entries: list[dict[str, Any]] = []
        self.ids: set[str] = set()
        self.dependency_edges: list[dict[str, str]] = []

    def _base_entry(self, fixture_id: str, relative_path: str, kind: str, **expect: Any) -> dict[str, Any]:
        if fixture_id in self.ids:
            raise LabError(f"duplicate fixture id: {fixture_id}")
        self.ids.add(fixture_id)
        classification = expect.pop("classification", "Unknown")
        health = expect.pop("health", "Healthy")
        scenario = expect.pop("scenario", None)
        if scenario is None:
            scenario = {
                "Incomplete": "incomplete", "MissingCompanion": "incomplete",
                "Malformed": "malformed", "WrongExtension": "misnamed",
                "UnsafePath": "security", "Duplicate": "duplicate",
                "Unsupported": "unsupported", "NeedsReview": "needs-review",
            }.get(health)
        if scenario is None:
            scenario = {
                "Support": "support-material", "Bios": "support-material",
                "Firmware": "support-material", "Device": "support-material",
                "Artwork": "artwork", "Metadata": "metadata", "Cheat": "cheat",
                "Mod": "mod", "Unknown": "unknown",
            }.get(classification, "good")
        entry = {
            "fixture_id": fixture_id,
            "platform": expect.pop("platform", None),
            "scenario": scenario,
            "relative_path": relative_path,
            "kind": kind,
            "expectation_type": expect.pop("expectation_type", "ExactAssertion"),
            "expected_classification": classification,
            "expected_platform": expect.pop("expected_platform", None),
            "expected_identity_strength": expect.pop("identity", "Unknown"),
            "expected_playable": expect.pop("playable", False),
            "expected_support_only": expect.pop("support_only", False),
            "expected_archive_kind": expect.pop("archive_kind", None),
            "expected_media_set": expect.pop("media_set", None),
            "expected_warning": expect.pop("warning", None),
            "expected_error": expect.pop("error", None),
            "expected_health": health,
            "relationships": expect.pop("relationships", []),
            "notes": expect.pop("notes", "Synthetic fixture authored by EmuWiz; no commercial data."),
        }
        if expect:
            entry.update(expect)
        return entry

    def add_file(self, fixture_id: str, relative_path: str, data: bytes, **expect: Any) -> dict[str, Any]:
        relative_path = safe_relative(relative_path)
        path = self.root / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        entry = self._base_entry(fixture_id, relative_path, "file", **expect)
        entry.update({"sha256": sha256_bytes(data), "size": len(data), "expected_exists": True})
        self.entries.append(entry)
        return entry

    def add_text(self, fixture_id: str, relative_path: str, text: str, **expect: Any) -> dict[str, Any]:
        return self.add_file(fixture_id, relative_path, text.encode("utf-8"), **expect)

    def add_zip(self, fixture_id: str, relative_path: str, members: list[tuple[str, bytes]], **expect: Any) -> dict[str, Any]:
        expect.setdefault("archive_kind", "Zip")
        entry = self.add_file(fixture_id, relative_path, zip_bytes(members), **expect)
        entry["archive_members"] = [name for name, _ in members]
        return entry

    def add_symlink(self, fixture_id: str, relative_path: str, target: str, **expect: Any) -> dict[str, Any]:
        relative_path = safe_relative(relative_path)
        if Path(target).is_absolute():
            raise LabError("fixture symlink target must be relative")
        link = self.root / relative_path
        link.parent.mkdir(parents=True, exist_ok=True)
        os.symlink(target, link)
        entry = self._base_entry(fixture_id, relative_path, "symlink", **expect)
        entry.update({"sha256": sha256_bytes(target.encode()), "size": len(target.encode()), "link_target": target, "expected_exists": True})
        self.entries.append(entry)
        return entry

    def add_hardlink(self, fixture_id: str, relative_path: str, target_relative: str, **expect: Any) -> dict[str, Any]:
        relative_path = safe_relative(relative_path)
        target_relative = safe_relative(target_relative)
        link = self.root / relative_path
        target = self.root / target_relative
        link.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.link(target, link)
            kind = "hardlink"
            note = "Hardlink created; same inode and bytes as target."
        except OSError as error:
            shutil.copyfile(target, link)
            kind = "file"
            note = f"Filesystem rejected hardlink ({error}); deterministic byte-copy fallback."
        entry = self._base_entry(fixture_id, relative_path, kind, **expect)
        entry.update({"sha256": sha256_file(link), "size": link.stat().st_size, "link_target": target_relative, "expected_exists": True, "notes": note})
        self.entries.append(entry)
        return entry

    def add_virtual(self, fixture_id: str, relative_path: str, **expect: Any) -> dict[str, Any]:
        relative_path = safe_relative(relative_path)
        entry = self._base_entry(fixture_id, relative_path, "virtual", **expect)
        entry.update({"sha256": None, "size": 0, "expected_exists": False})
        self.entries.append(entry)
        return entry

    def bytes(self, fixture_id: str, size: int) -> bytes:
        return payload(fixture_id, self.seed, GENERATOR_VERSION, size)


PLATFORMS = [
    ("atari-st", "Atari ST", "st", "Atari ST"),
    ("amiga", "Amiga", "adf", "Amiga"),
    ("commodore-64", "Commodore 64", "prg", "Commodore 64"),
    ("zx-spectrum", "ZX Spectrum", "tap", "ZX Spectrum"),
    ("amstrad-cpc", "Amstrad CPC", "dsk", "Amstrad CPC"),
    ("bbc-micro", "BBC Micro", "ssd", "BBC Micro"),
    ("ms-dos", "DOS", "exe", "DOS"),
    ("scummvm", "ScummVM", "gen", "ScummVM"),
    ("nes", "Nintendo Entertainment System", "nes", "NES"),
    ("snes", "Super Nintendo Entertainment System", "sfc", "SNES"),
    ("mega-drive", "Mega Drive", "md", "Mega Drive"),
    ("master-system", "Master System", "sms", "Master System"),
    ("game-boy", "Game Boy", "gb", "Game Boy"),
    ("game-boy-color", "Game Boy Color", "gbc", "Game Boy Color"),
    ("game-boy-advance", "Game Boy Advance", "gba", "Game Boy Advance"),
    ("nintendo-64", "Nintendo 64", "z64", "Nintendo 64"),
    ("playstation", "PlayStation", "cue", "PSX"),
    ("playstation-2", "PlayStation 2", "iso", "PS2"),
    ("psp", "PSP", "iso", "PSP"),
    ("playstation-3", "PlayStation 3", "self", "PS3"),
    ("gamecube", "GameCube", "iso", "GameCube"),
    ("wii", "Wii", "iso", "Wii"),
    ("dreamcast", "Dreamcast", "gdi", "Dreamcast"),
    ("saturn", "Saturn", "bin", "Saturn"),
    ("xbox", "Xbox", "xbe", "Xbox"),
    ("xbox-360", "Microsoft Xbox 360", "xex", "Xbox 360"),
    ("arcade", "Arcade", "zip", "Arcade"),
    ("neo-geo", "Neo Geo", "zip", "Neo Geo"),
    ("pc-engine", "PC Engine", "pce", "PC Engine"),
    ("pc-engine-cd", "PC Engine CD", "cue", "PC Engine CD"),
    ("nintendo-ds", "Nintendo DS", "nds", "Nintendo DS"),
    ("nintendo-3ds", "Nintendo 3DS", "3ds", "Nintendo 3DS"),
    ("switch", "Switch", "nsp", "Switch"),
]


def platform_data(slug: str, fixture_id: str, builder: Builder) -> bytes:
    data = bytearray(builder.bytes(fixture_id, 32768))
    if slug == "atari-st":
        data = bytearray(builder.bytes(fixture_id, 737280)); data[11:13] = struct.pack("<H", 512); data[13] = 2; data[19:21] = struct.pack("<H", 1440); data[24:26] = struct.pack("<H", 9); data[26:28] = struct.pack("<H", 2); data[510:512] = b"\x55\xaa"
    elif slug == "amiga":
        data = bytearray(builder.bytes(fixture_id, 901120)); data[:4] = b"DOS\0"
    elif slug == "commodore-64": data[:2] = b"\x01\x08"
    elif slug == "zx-spectrum": data = bytearray(struct.pack("<H", 16) + builder.bytes(fixture_id, 16))
    elif slug == "amstrad-cpc": data[:34] = b"MV - CPCEMU Disk-File\r\nDisk-Info\r\n"
    elif slug == "ms-dos": data[:2] = b"MZ"
    elif slug == "scummvm": data = bytearray(b"Synthetic RESOURCE.GEN data; not a game asset.\n")
    elif slug == "nes": data = bytearray(b"NES\x1a" + bytes([1, 1, 0, 0]) + bytes(8) + builder.bytes(fixture_id, 24576))
    elif slug == "snes": data[0x7FC0:0x7FD5] = b"EMUWIZ SYNTHETIC SNES"
    elif slug == "mega-drive": data[0x100:0x104] = b"SEGA"
    elif slug == "master-system": data[0x7FF0:0x7FF8] = b"TMR SEGA"
    elif slug in {"game-boy", "game-boy-color"}: data[0x134:0x144] = b"EMUWIZ SYNTHETIC"; data[0x143] = 0x80 if slug == "game-boy-color" else 0
    elif slug == "game-boy-advance": data[0xA0:0xAC] = b"EMUWIZ LAB  "
    elif slug == "nintendo-64": data[:4] = b"\x80\x37\x12\x40"; data[0x20:0x34] = b"EMUWIZ SYNTH N64   "
    elif slug in {"playstation-2", "psp"}:
        data = bytearray(builder.bytes(fixture_id, 40 * 2048)); data[16*2048+1:16*2048+6] = b"CD001"
    elif slug == "playstation-3": data[:4] = b"SCE\0"
    elif slug == "gamecube": data[0x1C:0x20] = bytes.fromhex("C2339F3D")
    elif slug == "wii": data[0x18:0x1C] = bytes.fromhex("5D1C9EA3")
    elif slug == "saturn": data[:16] = b"SEGA SEGASATURN "
    elif slug == "xbox": data[:4] = b"XBEH"
    elif slug == "xbox-360": data[:4] = b"XEX2"
    elif slug == "nintendo-ds": data[:12] = b"EMUWIZ NDS  "; data[12:16] = b"EWZE"
    elif slug == "nintendo-3ds": data[0x100:0x104] = b"NCSD"
    return bytes(data)


def add_platforms(builder: Builder) -> None:
    tiny = {"atari-st", "nes", "mega-drive", "playstation", "arcade"}
    for slug, folder, extension, canonical in PLATFORMS:
        if builder.level == 0 and slug not in tiny:
            continue
        fixture_id = f"platform.{slug}.good"
        root = f"library/{folder}"
        if slug == "playstation":
            add_ps1_sets(builder, root)
            continue
        if slug == "dreamcast":
            add_dreamcast(builder, root)
            continue
        if slug == "pc-engine-cd":
            track = builder.bytes(fixture_id + ".track", 48 * 1024)
            builder.add_file(fixture_id + ".track", f"{root}/Synthetic CD.bin", track, platform=canonical, classification="Support", support_only=True, media_set="pc-engine-cd.synthetic")
            builder.add_text(fixture_id, f"{root}/Synthetic CD.cue", 'FILE "Synthetic CD.bin" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n', platform=canonical, expected_platform=canonical, classification="Playable", identity="StrongCandidate", playable=True, media_set="pc-engine-cd.synthetic")
            continue
        if slug == "scummvm":
            builder.add_file(fixture_id, f"{root}/Synthetic Quest/RESOURCE.GEN", platform_data(slug, fixture_id, builder), platform=canonical, expected_platform=canonical, classification="Playable", identity="StrongCandidate", playable=True, notes="Directory-layout fixture; ScummVM itself remains authoritative for exact game identity.")
            builder.add_text(fixture_id + ".config", f"{root}/Synthetic Quest/scummvm.ini", "[synthetic-quest]\ndescription=Synthetic Quest\npath=.\n", platform=canonical, classification="Metadata", support_only=True)
            continue
        if slug == "playstation-3":
            builder.add_file(fixture_id, f"{root}/Synthetic Title/PS3_GAME/USRDIR/EBOOT.BIN", platform_data(slug, fixture_id, builder), platform=canonical, expected_platform=canonical, classification="Playable", identity="StrongCandidate", playable=True, notes="Synthetic SELF-signature placeholder in a PS3_GAME layout; not executable software.")
            builder.add_text(fixture_id + ".sfo", f"{root}/Synthetic Title/PS3_GAME/PARAM.SFO.synthetic.json", '{"TITLE":"Synthetic PS3 Title","TITLE_ID":"EWZP00001"}\n', platform=canonical, classification="Metadata", support_only=True)
            continue
        if slug in {"arcade", "neo-geo"}:
            # Specialist extracted-set fixtures are built with their DAT below.
            continue
        path = f"{root}/Synthetic {folder}.{extension}"
        classification = "Unsupported" if slug == "switch" else "Playable"
        health = "Unsupported" if slug == "switch" else "Healthy"
        builder.add_file(fixture_id, path, platform_data(slug, fixture_id, builder), platform=canonical, expected_platform=canonical, classification=classification, identity="WeakCandidate" if slug == "switch" else "StrongCandidate", playable=slug != "switch", health=health, expectation_type="DiagnosticOnly" if slug == "switch" else "ExactAssertion", warning="No keys or encrypted Nintendo content; structural extension placeholder only." if slug == "switch" else None)
        if builder.level >= 2:
            builder.add_file(f"platform.{slug}.malformed", f"{root}/Malformed {folder}.{extension}", b"BAD\0", platform=canonical, expected_platform=canonical, classification="Malformed", identity="WeakCandidate", health="Malformed", expectation_type="DiagnosticOnly", error="Recognised name/extension with invalid or truncated structure.")
            builder.add_file(f"platform.{slug}.misnamed", f"{root}/Misnamed {folder}.wrong", platform_data(slug, fixture_id + ".misnamed", builder), platform=canonical, expected_platform=canonical, classification="Ambiguous", identity="WeakCandidate", playable=False, health="WrongExtension", expectation_type="ManualReviewExpected", warning="Content evidence and extension disagree or the parser has no safe content proof.")


def add_ps1_sets(builder: Builder, root: str) -> None:
    set_id = "ps1.synthetic-adventure.complete"
    for disc in (1, 2):
        bin_id = f"{set_id}.disc{disc}.bin"
        data = bytearray(builder.bytes(bin_id, 48 * 1024)); data[0x8008:0x8013] = b"PLAYSTATION"
        builder.add_file(bin_id, f"{root}/Synthetic Adventure (Disc {disc}).bin", bytes(data), platform="PSX", classification="Support", support_only=True, media_set=set_id, relationships=[f"{set_id}.disc{disc}.cue"])
        builder.add_text(f"{set_id}.disc{disc}.cue", f"{root}/Synthetic Adventure (Disc {disc}).cue", f'FILE "Synthetic Adventure (Disc {disc}).bin" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n', platform="PSX", expected_platform="PSX", classification="Playable", identity="StrongCandidate", playable=True, media_set=set_id, relationships=[bin_id])
    builder.add_text(set_id + ".m3u", f"{root}/Synthetic Adventure.m3u", "Synthetic Adventure (Disc 1).cue\nSynthetic Adventure (Disc 2).cue\n", platform="PSX", expected_platform="PSX", classification="Playable", identity="StrongCandidate", playable=True, media_set=set_id, relationships=[f"{set_id}.disc1.cue", f"{set_id}.disc2.cue"])
    if builder.level >= 1:
        builder.add_text("ps1.missing-disc.m3u", f"{root}/Incomplete/Adventure Missing Disc.m3u", "Disc 1.cue\nDisc 2.cue\n", platform="PSX", classification="Malformed", health="MissingCompanion", error="Disc 2 and Disc 1 companions are absent.", expectation_type="DiagnosticOnly")
        builder.add_text("ps1.escape.m3u", f"{root}/Unsafe/Escape.m3u", "../outside.cue\n", platform="PSX", classification="Malformed", health="UnsafePath", error="Playlist entry escapes its set directory.", expectation_type="Invariant")
        builder.add_text("ps1.duplicate-entry.m3u", f"{root}/Malformed/Duplicate Disc.m3u", "../Synthetic Adventure (Disc 1).cue\n../Synthetic Adventure (Disc 1).cue\n", platform="PSX", classification="Malformed", health="Malformed", warning="Duplicate playlist member.", expectation_type="DiagnosticOnly")
        builder.add_text("ps1.malformed-cue", f"{root}/Malformed/Broken.cue", "TRACK absolutely-not-a-cue\n", platform="PSX", classification="Malformed", health="Malformed", error="Malformed CUE syntax.", expectation_type="DiagnosticOnly")
        builder.add_text("ps1.missing-bin.cue", f"{root}/Incomplete/Missing Bin.cue", 'FILE "Missing Bin.bin" BINARY\n TRACK 01 MODE2/2352\n', platform="PSX", classification="Malformed", health="MissingCompanion", error="Referenced BIN does not exist.", expectation_type="Invariant")
        builder.add_text("ps1.extra-companion", f"{root}/Synthetic Adventure.nfo", "Unexpected but inert companion.\n", platform="PSX", classification="Metadata", support_only=True, warning="Unexpected extra companion; must not become a separate game.", expectation_type="Invariant")


def add_dreamcast(builder: Builder, root: str) -> None:
    track1 = builder.add_file("dreamcast.gdi.track1", f"{root}/Synthetic Dreamcast/track01.bin", builder.bytes("dreamcast.gdi.track1", 4096), platform="Dreamcast", classification="Support", support_only=True, media_set="dreamcast.gdi.good")
    track2 = builder.add_file("dreamcast.gdi.track2", f"{root}/Synthetic Dreamcast/track02.raw", builder.bytes("dreamcast.gdi.track2", 8192), platform="Dreamcast", classification="Support", support_only=True, media_set="dreamcast.gdi.good")
    builder.add_text("dreamcast.gdi.good", f"{root}/Synthetic Dreamcast/Synthetic Dreamcast.gdi", "2\n1 0 4 2352 track01.bin 0\n2 450 0 2352 track02.raw 0\n", platform="Dreamcast", expected_platform="Dreamcast", classification="Playable", identity="StrongCandidate", playable=True, media_set="dreamcast.gdi.good", relationships=[track1["fixture_id"], track2["fixture_id"]], notes="Minimal documented GDI descriptor with synthetic tracks.")
    if builder.level >= 1:
        builder.add_text("dreamcast.gdi.malformed", f"{root}/Malformed.gdi", "999999999\nnot tracks\n", platform="Dreamcast", classification="Malformed", health="Malformed", error="Declared track count is absurd and rows are invalid.", expectation_type="DiagnosticOnly")
        cdi = bytearray(builder.bytes("dreamcast.cdi.placeholder", 4096)); cdi[-8:] = b"CDI_SYN!"
        builder.add_file("dreamcast.cdi.placeholder", f"{root}/Synthetic Placeholder.cdi", bytes(cdi), platform="Dreamcast", expected_platform="Dreamcast", classification="Ambiguous", identity="WeakCandidate", health="NeedsReview", expectation_type="ManualReviewExpected", notes="Repository-owned synthetic CDI-like placeholder; not claimed DiscJuggler-valid.")
        builder.add_file("dreamcast.cdi.malformed", f"{root}/Malformed.cdi", b"CDI", platform="Dreamcast", classification="Malformed", health="Malformed", expectation_type="DiagnosticOnly")


def add_arcade(builder: Builder, arcade_sets: int) -> None:
    root = "library/Arcade"
    chips: dict[str, tuple[int, str, str]] = {}
    def chip(set_name: str, name: str, content_id: str, classification: str = "Support") -> None:
        data = builder.bytes(content_id, 64)
        entry = builder.add_file(content_id, f"{root}/{set_name}/{name}", data, platform="Arcade", classification=classification, support_only=True, identity="Verified", relationships=[f"mame.set.{set_name}"], notes="Synthetic chip member; must not be catalogued as a standalone game.", expectation_type="Invariant")
        chips[f"{set_name}/{name}"] = (len(data), f"{zlib.crc32(data) & 0xffffffff:08x}", hashlib.sha1(data).hexdigest())
    chip("emuwizparent", "parent-a.bin", "arcade.parent.a")
    chip("emuwizparent", "parent-b.bin", "arcade.parent.b")
    chip("emuwizclone", "clone-a.bin", "arcade.clone.a")
    chip("emuwizbios", "bios-a.bin", "arcade.bios.a", "Bios")
    chip("emuwizdevice", "device-a.bin", "arcade.device.a", "Device")
    chip("emuwizmissing", "present.bin", "arcade.missing.present")
    chip("emuwizextra", "expected.bin", "arcade.extra.expected")
    chip("emuwizextra", "surprise.bin", "arcade.extra.surprise")
    chip("emuwizmechanical", "mechanical.bin", "arcade.mechanical")
    machines = [
        ("emuwizparent", "", "", "yes", "no", [("parent-a.bin", None, "emuwizparent/parent-a.bin"), ("parent-b.bin", None, "emuwizparent/parent-b.bin")]),
        ("emuwizclone", ' cloneof="emuwizparent"', ' romof="emuwizparent"', "yes", "no", [("clone-a.bin", None, "emuwizclone/clone-a.bin"), ("parent-b.bin", "parent-b.bin", "emuwizparent/parent-b.bin")]),
        ("emuwizbios", ' isbios="yes"', "", "no", "no", [("bios-a.bin", None, "emuwizbios/bios-a.bin")]),
        ("emuwizdevice", ' isdevice="yes" runnable="no"', "", "no", "no", [("device-a.bin", None, "emuwizdevice/device-a.bin")]),
        ("emuwizmissing", ' romof="emuwizbios"', "", "yes", "no", [("present.bin", None, "emuwizmissing/present.bin"), ("absent.bin", None, None)]),
        ("emuwizextra", "", "", "yes", "no", [("expected.bin", None, "emuwizextra/expected.bin")]),
        ("emuwizmechanical", ' ismechanical="yes" runnable="no"', "", "no", "yes", [("mechanical.bin", None, "emuwizmechanical/mechanical.bin")]),
    ]
    lines = ['<?xml version="1.0"?>', '<mame build="emuwiz-synthetic-1">']
    for name, attrs, romof, runnable, mechanical, roms in machines:
        lines.append(f'  <machine name="{name}"{attrs}{romof}>')
        lines.append(f"    <description>EmuWiz Synthetic {name}</description>")
        for rom_name, merge, key in roms:
            if key is None:
                size, crc, sha = 64, "00000000", "0" * 40
            else:
                size, crc, sha = chips[key]
            merge_attr = f' merge="{merge}"' if merge else ""
            lines.append(f'    <rom name="{rom_name}" size="{size}" crc="{crc}" sha1="{sha}"{merge_attr}/>')
        if name == "emuwizmissing": lines.append('    <device_ref name="emuwizdevice"/>')
        lines.append("  </machine>")
    lines.append("</mame>")
    builder.add_text("dat.mame.synthetic", "metadata/dats/emuwiz-synthetic-mame.xml", "\n".join(lines) + "\n", platform="Arcade", classification="Metadata", identity="Verified", relationships=["mame.set.emuwizparent", "mame.set.emuwizclone"], notes="Entirely authored fake MAME-style XML; hashes refer only to generated bytes.")
    set_states = {
        "emuwizparent": ("Playable", "Healthy"), "emuwizclone": ("Playable", "Healthy"),
        "emuwizbios": ("Bios", "Healthy"), "emuwizdevice": ("Device", "Healthy"),
        "emuwizmissing": ("Malformed", "MissingCompanion"), "emuwizextra": ("Ambiguous", "NeedsReview"),
        "emuwizmechanical": ("Support", "Unsupported"),
    }
    for set_name, (classification, health) in set_states.items():
        builder.add_virtual(f"mame.set.{set_name}", f"expected/arcade/{set_name}.logical-set", platform="Arcade", scenario="logical-set", classification=classification, expected_platform="Arcade", identity="Verified", playable=classification == "Playable", support_only=classification in {"Bios", "Device", "Support"}, health=health, expectation_type="Invariant", notes="Logical extracted-set expectation; no on-disk placeholder is created.")
    builder.dependency_edges.extend([
        {"from": "emuwizclone", "to": "emuwizparent", "kind": "cloneof"},
        {"from": "emuwizclone", "to": "emuwizparent", "kind": "romof"},
        {"from": "emuwizmissing", "to": "emuwizbios", "kind": "bios"},
        {"from": "emuwizmissing", "to": "emuwizdevice", "kind": "device"},
    ])
    builder.add_file("fbneo.support.neogeo-bios", "library/Neo Geo/emuwizfbneo/neogeo-synthetic-bios.bin", builder.bytes("fbneo.support.neogeo-bios", 128), platform="Neo Geo", classification="Bios", support_only=True, identity="WeakCandidate", warning="Synthetic support member, not real Neo Geo firmware.")
    if arcade_sets:
        dat_lines = ['<?xml version="1.0"?>', '<mame build="emuwiz-scale-1">']
        for index in range(arcade_sets):
            set_name = f"scale{index:05d}"
            dat_lines.append(f'  <machine name="{set_name}"><description>Scale Set {index}</description>')
            for chip_index in range(3):
                fixture_id = f"arcade.scale.{index}.{chip_index}"
                data = builder.bytes(fixture_id, 16)
                name = f"chip{chip_index}.bin"
                builder.add_file(fixture_id, f"library/Arcade Scale/{set_name}/{name}", data, platform="Arcade", classification="Support", support_only=True, expectation_type="Invariant")
                dat_lines.append(f'    <rom name="{name}" size="16" crc="{zlib.crc32(data) & 0xffffffff:08x}" sha1="{hashlib.sha1(data).hexdigest()}"/>')
            dat_lines.append("  </machine>")
        dat_lines.append("</mame>")
        builder.add_text("dat.mame.scale", "metadata/dats/emuwiz-scale-mame.xml", "\n".join(dat_lines) + "\n", platform="Arcade", classification="Metadata", notes=f"Synthetic DAT for {arcade_sets} scale sets.")


def add_archives(builder: Builder) -> None:
    root = "library/Archives"
    builder.add_zip("archive.single", f"{root}/Single Game.zip", [("Synthetic Game.nes", b"NES\x1a" + builder.bytes("archive.single.member", 1024))], classification="Playable", identity="StrongCandidate", playable=True)
    builder.add_zip("archive.multi", f"{root}/Multi File.zip", [("game/data.bin", builder.bytes("archive.multi.a", 32)), ("game/index.dat", builder.bytes("archive.multi.b", 32))], classification="Playable", identity="WeakCandidate", playable=True)
    builder.add_zip("archive.wrapper", f"{root}/Wrapper.zip", [("Wrapper/Game/Synthetic.gba", builder.bytes("archive.wrapper", 256))], classification="Playable", identity="WeakCandidate", playable=True)
    builder.add_zip("archive.readme-only", f"{root}/Readme Only.zip", [("README.txt", b"Synthetic metadata only.\n")], classification="Metadata", support_only=True)
    builder.add_zip("archive.unsupported", f"{root}/Unsupported.zip", [("movie.xyzzy", builder.bytes("archive.unsupported", 64))], classification="Unsupported", health="Unsupported", expectation_type="DiagnosticOnly")
    builder.add_file("archive.truncated", f"{root}/Truncated.zip", zip_bytes([("game.bin", b"synthetic")], truncate=True), classification="Malformed", archive_kind="Zip", health="Malformed", error="ZIP central directory is truncated.", expectation_type="DiagnosticOnly")
    if builder.level >= 2:
        builder.add_zip("archive.nested-wrapper", f"{root}/Nested Wrapper.zip", [("one/two/three/Synthetic.sms", builder.bytes("archive.nested", 256))], classification="Playable", identity="WeakCandidate", playable=True)
        builder.add_zip("archive.duplicate-member", f"{root}/Duplicate Member.zip", [("same.bin", b"one"), ("same.bin", b"two")], classification="Malformed", health="NeedsReview", warning="Duplicate archive member name.", expectation_type="Invariant")
        builder.add_zip("archive.traversal", f"{root}/Traversal.zip", [("../escape.bin", b"inert traversal payload")], classification="Malformed", health="UnsafePath", error="Archive member attempts parent traversal; generator never extracts it.", expectation_type="Invariant")
        builder.add_zip("archive.absolute", f"{root}/Absolute.zip", [("/absolute/escape.bin", b"inert absolute-path payload")], classification="Malformed", health="UnsafePath", expectation_type="Invariant")
        builder.add_zip("archive.windows-drive", f"{root}/Windows Drive.zip", [("C:/escape.bin", b"inert drive-prefix payload")], classification="Malformed", health="UnsafePath", expectation_type="Invariant")
        archive_link = builder.add_file("archive.symlink-member", f"{root}/Symlink Member.zip", zip_symlink_bytes("safe-link", "synthetic-target.bin"), classification="Malformed", archive_kind="Zip", health="NeedsReview", warning="Archive contains an inert Unix symlink member; generator never extracts it.", expectation_type="Invariant")
        archive_link["archive_members"] = ["safe-link"]


def add_dat_fixtures(builder: Builder) -> None:
    data = builder.bytes("dat.nointro.member", 128)
    crc = f"{zlib.crc32(data) & 0xffffffff:08x}"
    builder.add_file("dat.nointro.member", "library/DAT Candidates/Synthetic Cartridge.nes", data, platform="NES", classification="Playable", identity="Verified", playable=True)
    builder.add_text("dat.logiqx.good", "metadata/dats/emuwiz-nointro.dat", f'<?xml version="1.0"?><datafile><header><name>EmuWiz Synthetic No-Intro</name><version>1</version></header><game name="Synthetic Cartridge"><description>Synthetic Cartridge</description><rom name="Synthetic Cartridge.nes" size="128" crc="{crc}" sha1="{hashlib.sha1(data).hexdigest()}" md5="{hashlib.md5(data).hexdigest()}"/></game></datafile>\n', classification="Metadata", identity="Verified")
    builder.add_text("dat.tosec.good", "metadata/dats/emuwiz-tosec.dat", f'<?xml version="1.0"?><datafile><header><name>EmuWiz Synthetic TOSEC</name><version>2026-09-21</version></header><game name="Synthetic Tape (1984)(EmuWiz)"><rom name="Synthetic Tape.tap" size="128" crc="{crc}"/></game></datafile>\n', classification="Metadata", identity="StrongCandidate")
    builder.add_text("dat.custom.good", "metadata/dats/emuwiz-custom.json", json.dumps({"schema": 1, "games": [{"name": "Synthetic Cartridge", "sha256": sha256_bytes(data), "platform": "NES"}]}, indent=2) + "\n", classification="Metadata", identity="Verified")
    builder.add_text("dat.malformed", "metadata/dats/malformed.xml", "<datafile><game><rom></datafile>\n", classification="Malformed", health="Malformed", expectation_type="DiagnosticOnly")
    builder.add_text("dat.wrong-ecosystem", "metadata/dats/wrong-ecosystem.xml", "<not-a-datafile ecosystem=\"unknown\"/>\n", classification="Unsupported", health="Unsupported", expectation_type="DiagnosticOnly")
    builder.add_text("dat.wrong-version", "metadata/dats/wrong-version.json", '{"schema":999999,"games":[]}\n', classification="Malformed", health="Unsupported", expectation_type="DiagnosticOnly")
    builder.add_text("dat.duplicate-games", "metadata/dats/duplicate-games.xml", '<datafile><game name="dup"/><game name="dup"/></datafile>\n', classification="Malformed", health="NeedsReview", expectation_type="DiagnosticOnly")
    builder.add_text("dat.conflicting-hash", "metadata/dats/conflicting-hash.xml", '<datafile><game name="conflict"><rom name="same.bin" sha1="0000000000000000000000000000000000000000"/><rom name="same.bin" sha1="1111111111111111111111111111111111111111"/></game></datafile>\n', classification="Ambiguous", identity="Conflicting", health="NeedsReview", expectation_type="ManualReviewExpected")


def add_bios_provider(builder: Builder) -> None:
    exact = builder.bytes("bios.exact", 512)
    wrong = builder.bytes("bios.wrong", 512)
    builder.add_file("bios.exact", "support/bios/emuwiz_test_bios.bin", exact, classification="Bios", identity="Verified", support_only=True, warning="SYNTHETIC / NOT A REAL BIOS")
    builder.add_file("bios.filename-only", "support/bios/scphSYNTH.bin", wrong, classification="Bios", identity="WeakCandidate", support_only=True, health="NeedsReview", warning="SYNTHETIC / HASH MISMATCH / NOT A REAL BIOS")
    builder.add_file("firmware.synthetic", "support/firmware/emuwiz_test_firmware.bin", builder.bytes("firmware.synthetic", 384), classification="Firmware", identity="Unknown", support_only=True, health="NeedsReview", warning="SYNTHETIC / NOT REAL VENDOR FIRMWARE")
    snapshot = {
        "schema_version": 1, "pinned_ref": "refs/tags/emuwiz-synthetic-v1",
        "entries": [
            {"id": "exact", "filename": "emuwiz_test_bios.bin", "sha256": sha256_bytes(exact), "required": True, "acquisition": "UserMustProvide", "license": "UnknownLicense"},
            {"id": "filename-only", "filename": "scphSYNTH.bin", "sha256": "f" * 64, "required": True, "acquisition": "BrowserHandoff", "license": "DoNotAutomate"},
            {"id": "optional", "filename": "optional_synthetic.bin", "required": False, "status": "Optional"},
            {"id": "not-required", "platform": "Synthetic No-BIOS", "status": "NotRequired"},
        ],
    }
    snapshot_bytes = canonical_json(snapshot)
    builder.add_file("retrobios.snapshot", "metadata/providers/retrobios-synthetic.json", snapshot_bytes, classification="Metadata", identity="Verified", notes="Offline fake RetroBIOS-style snapshot. No firmware payloads or URLs.")
    builder.add_text("retrobios.provenance", "metadata/providers/retrobios-provenance.json", json.dumps({"pinned_ref": snapshot["pinned_ref"], "snapshot_sha256": sha256_bytes(snapshot_bytes), "network": False}, indent=2) + "\n", classification="Metadata", identity="Verified")
    provider = {"schema_version": 1, "records": [
        {"id": "verified", "match": "Verified", "sha256": sha256_bytes(exact)},
        {"id": "likely", "match": "Likely", "title": "Synthetic Adventure"},
        {"id": "title", "match": "TitleOnly", "title": "Synthetic Quest"},
        {"id": "unknown", "match": "Unknown"},
        {"id": "conflict", "match": "Conflicting", "candidates": ["A", "B"]},
        {"id": "browser", "acquisition": "BrowserRequired", "host": "example.invalid"},
        {"id": "external", "acquisition": "ExternalHost", "host": "example.invalid"},
        {"id": "checksum", "acquisition": "checksum-backed", "sha256": sha256_bytes(exact)},
        {"id": "mismatch", "acquisition": "checksum-backed", "sha256": "0" * 64},
    ]}
    builder.add_text("provider.neutral", "metadata/providers/provider-neutral.json", json.dumps(provider, indent=2) + "\n", classification="Metadata")


def add_cheats_mods(builder: Builder) -> None:
    builder.add_text("cheat.retroarch.good", "cheats/retroarch/Synthetic Adventure.cht", 'cheats = 1\ncheat0_desc = "Harmless synthetic toggle"\ncheat0_code = "0000+00+00"\ncheat0_enable = false\n', classification="Cheat", identity="Verified")
    builder.add_text("cheat.retroarch.quirky", "cheats/retroarch/Quirky Syntax.cht", 'cheats=2\ncheat0_desc="spacing"\ncheat0_code="0000:00"\n# absent enable is intentional\ncheat1_desc = "quoted = value"\ncheat1_code = "0001+00+00"\n', classification="Cheat", identity="StrongCandidate")
    builder.add_text("cheat.retroarch.malformed", "cheats/retroarch/Malformed.cht", "cheats = absurd\ncheat999999_code\n", classification="Malformed", health="Malformed", expectation_type="DiagnosticOnly")
    builder.add_text("cheat.retroarch.huge-count", "cheats/retroarch/Huge Count Boundary.cht", "cheats = 999999999\ncheat0_desc = \"Bounded parser test\"\n", classification="Malformed", health="Malformed", warning="Absurd declared count in a tiny file; parser must reject without allocating proportionally.", expectation_type="Invariant")
    builder.add_text("cheat.pcsx2.good", "cheats/pcsx2/EWZ-00001.pnach", "gametitle=Synthetic PS2 Title [EWZ-00001]\npatch=0,EE,00100000,word,00000000\n", platform="PS2", classification="Cheat", identity="Verified")
    builder.add_text("cheat.pcsx2.wrong-id", "cheats/pcsx2/WRONG000.pnach", "gametitle=Wrong Synthetic Identity\npatch=1,EE,00100000,word,00000000\n", platform="PS2", classification="Cheat", identity="Conflicting", health="NeedsReview", expectation_type="ManualReviewExpected")
    builder.add_text("cheat.pcsx2.malformed", "cheats/pcsx2/Malformed.pnach", "patch=this,is,not,valid\n", platform="PS2", classification="Malformed", health="Malformed", expectation_type="DiagnosticOnly")
    builder.add_text("cheat.dolphin.gecko", "cheats/dolphin/EWZE01-gecko.ini", "[Gecko]\n$Synthetic Toggle\n00000000 00000000\n", platform="GameCube", classification="Cheat", identity="Verified")
    builder.add_text("cheat.dolphin.ar", "cheats/dolphin/EWZE01-ar.ini", "[ActionReplay]\n$Synthetic AR\n00000000 00000000\n", platform="GameCube", classification="Cheat", identity="Verified")
    builder.add_text("cheat.dolphin.onframe", "cheats/dolphin/EWZE01-onframe.ini", "[OnFrame]\n$Synthetic OnFrame\n0x00000000:dword:0x00000000\n", platform="GameCube", classification="Cheat", identity="StrongCandidate")
    builder.add_text("cheat.bsfree.synthetic", "cheats/bsfree/synthetic.json", '{"game_id":"EWZE01","codes":[{"name":"Synthetic","code":"00000000 00000000"}]}\n', classification="Cheat", identity="Verified")
    builder.add_text("cheat.gamehacking.malformed", "cheats/gamehacking/malformed.html", "<html><script>inert text only</script><div data-count=999999999></div></html>\n", classification="Malformed", health="Malformed", expectation_type="DiagnosticOnly")
    builder.add_text("cheat.xenia.patch", "cheats/xenia/EWZ00001.patch.toml", 'title_name = "Synthetic Xbox 360 Title"\ntitle_id = "EWZ00001"\n[[patch]]\nname = "Synthetic no-op"\n', platform="Xbox 360", classification="Cheat", identity="StrongCandidate")

    builder.add_zip("mod.archive.good", "mods/archive/safe-package.zip", [("textures/synthetic.png", png_bytes(2, 2, (20, 80, 160))), ("README.txt", b"Synthetic mod package.\n")], classification="Mod", identity="Verified")
    builder.add_zip("mod.archive.wrapper", "mods/archive/wrapper-package.zip", [("Synthetic Mod/textures/a.bin", b"texture")], classification="Mod", identity="StrongCandidate")
    builder.add_zip("mod.archive.traversal", "mods/archive/traversal-package.zip", [("../../escape.txt", b"inert")], classification="Malformed", health="UnsafePath", expectation_type="Invariant")
    builder.add_zip("mod.archive.script", "mods/archive/script-package.zip", [("install.sh", b"#!/bin/sh\n# inert; never executed\n")], classification="Malformed", health="Unsupported", warning="Executable/script member must be rejected.", expectation_type="Invariant")
    builder.add_zip("mod.archive.duplicate-destination", "mods/archive/duplicate-destination.zip", [("Textures/A.bin", b"one"), ("textures/a.bin", b"two")], classification="Ambiguous", health="NeedsReview", warning="Case-normalized destination collision.", expectation_type="Invariant")
    builder.add_file("mod.archive.unsupported", "mods/archive/unsupported-format.rar", b"SYNTHETIC INVALID RAR PLACEHOLDER\n", classification="Unsupported", archive_kind="Rar", health="Unsupported", warning="Not a RAR archive; no proprietary RAR creator is required.", expectation_type="DiagnosticOnly")
    ips = b"PATCH" + struct.pack(">I", 1)[1:] + struct.pack(">H", 1) + b"X" + b"EOF"
    builder.add_file("mod.patch.ips", "mods/patches/synthetic.ips", ips, classification="Mod", identity="Verified", relationships=["platform.nes.good"])
    builder.add_text("mod.patch.chain-compatible", "mods/patches/compatible-chain.json", '{"patches":["synthetic.ips","second-synthetic.ips"],"compatibility":"explicit"}\n', classification="Metadata", identity="Verified")
    builder.add_file("mod.patch.second", "mods/patches/second-synthetic.ips", b"PATCHEOF", classification="Mod", identity="StrongCandidate")
    builder.add_text("mod.patch.chain-incompatible", "mods/patches/incompatible-chain.json", '{"patches":["synthetic.ips","wrong-target.ips"],"compatibility":"conflict"}\n', classification="Ambiguous", identity="Conflicting", health="NeedsReview")
    builder.add_file("mod.patch.wrong-target", "mods/patches/wrong-target.ips", b"PATCHEOF", classification="Mod", identity="Conflicting", health="NeedsReview")
    builder.add_file("mod.patch.derived-output", "mods/patches/derived/Synthetic Patched.nes", builder.bytes("mod.patch.derived-output", 1024), classification="Playable", identity="StrongCandidate", playable=True, relationships=["mod.patch.ips"])
    builder.add_text("mod.patch.checksum-mismatch", "mods/patches/checksum-mismatch.json", '{"expected_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}\n', classification="Malformed", identity="Conflicting", health="NeedsReview")
    builder.add_text("mod.patch.ambiguous-target", "mods/patches/ambiguous-target.json", '{"title":"Synthetic Game","candidates":["A","B"]}\n', classification="Ambiguous", identity="Conflicting", health="NeedsReview")
    builder.add_file("mod.pcsx2.texture", "mods/emulator/PCSX2/textures/EWZ-00001/replacements/00000001.png", png_bytes(2, 2, (220, 20, 40)), platform="PS2", classification="Mod", identity="Verified")
    builder.add_file("mod.ppsspp.texture", "mods/emulator/PPSSPP/PSP/TEXTURES/EWZE00001/00000001.png", png_bytes(2, 2, (20, 220, 40)), platform="PSP", classification="Mod", identity="Verified")
    builder.add_text("mod.cemu.graphic-pack", "mods/emulator/Cemu/graphicPacks/Synthetic/rules.txt", "[Definition]\ntitleIds = 0005000012345678\nname = Synthetic Graphic Pack\nversion = 7\n", platform="Wii U", classification="Mod", identity="Verified")
    builder.add_file("mod.rpcs3.ordinary", "mods/emulator/RPCS3/dev_hdd0/game/EWZP00001/USRDIR/synthetic.bin", builder.bytes("mod.rpcs3.ordinary", 64), platform="PS3", classification="Mod", identity="Verified")


def add_artwork_metadata_frontends(builder: Builder) -> None:
    cover = png_bytes(8, 12, (50, 100, 180)); shot = png_bytes(12, 8, (20, 160, 90)); logo = png_bytes(10, 4, (190, 80, 30))
    builder.add_file("art.cover", "artwork/Synthetic Adventure/cover.png", cover, classification="Artwork", identity="Verified")
    builder.add_file("art.screenshot", "artwork/Synthetic Adventure/screenshot.png", shot, classification="Artwork", identity="Verified")
    builder.add_file("art.logo", "artwork/Synthetic Adventure/logo.png", logo, classification="Artwork", identity="Verified")
    builder.add_file("art.corrupt", "artwork/Synthetic Adventure/corrupt.png", b"\x89PNG\r\ntruncated", classification="Malformed", health="Malformed", expectation_type="DiagnosticOnly")
    builder.add_file("art.duplicate", "artwork/Duplicates/cover-copy.png", cover, classification="Artwork", identity="Verified", health="Duplicate", relationships=["art.cover"])
    builder.add_virtual("art.missing", "artwork/Synthetic Adventure/missing.png", classification="Artwork", health="MissingCompanion", expectation_type="DiagnosticOnly")
    if builder.level >= 2:
        fake_huge = b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR" + struct.pack(">IIBBBBB", 20000, 20000, 8, 2, 0, 0, 0)
        builder.add_file("art.oversized-dimensions", "artwork/Boundary/oversized-dimensions.png", fake_huge, classification="Malformed", health="Malformed", warning="Huge declared dimensions in a tiny invalid PNG; decoder must enforce limits.", expectation_type="Invariant")
    records = {
        "local": {"title": "Synthetic Adventure", "platform": "PSX", "serial": "EWZ-00001"},
        "romm": {"id": 1001, "name": "Synthetic Adventure", "platform_slug": "psx", "sha256": sha256_bytes(b"synthetic")},
        "launchbox": {"ApplicationPath": "Games/Synthetic Adventure.m3u", "Title": "Synthetic Adventure"},
        "screenscraper": {"gameid": "synthetic-1", "nom": "Synthetic Adventure", "persisted": True},
    }
    for name, record in records.items():
        builder.add_text(f"metadata.{name}", f"metadata/catalogues/{name}.json", json.dumps(record, indent=2) + "\n", classification="Metadata", identity="StrongCandidate")
    builder.add_text("metadata.esde", "frontends/es-de/gamelists/psx/gamelist.xml", '<?xml version="1.0"?><gameList><game><path>../../roms/psx/Synthetic Adventure.m3u</path><name>Synthetic Adventure</name><desc>Original synthetic fixture.</desc></game></gameList>\n', classification="Metadata", identity="Verified")
    builder.add_text("esde.unrelated", "frontends/es-de/gamelists/nes/gamelist.xml", '<?xml version="1.0"?><gameList><game><path>../../roms/nes/Unrelated.nes</path><name>Unrelated Existing Entry</name></game></gameList>\n', classification="Metadata", identity="StrongCandidate")
    builder.add_text("esde.malformed", "frontends/es-de/gamelists/broken/gamelist.xml", "<gameList><game></gameList>\n", classification="Malformed", health="Malformed", expectation_type="DiagnosticOnly")
    builder.add_virtual("esde.unmapped-platform", "frontends/es-de/roms/unmapped/Synthetic.rom", classification="Ambiguous", health="NeedsReview", expectation_type="ManualReviewExpected")
    builder.add_text("esde.recovery-record", "frontends/es-de/recovery/synthetic.json", '{"operation":"export","state":"Interrupted","destination":"gamelists/psx/gamelist.xml"}\n', classification="Metadata", health="NeedsReview")
    builder.add_text("romm.slug-map", "frontends/romm/reviewed-slug-map.json", '{"PSX":"psx","NES":"nes","ScummVM":"scummvm"}\n', classification="Metadata", identity="Verified")
    builder.add_text("romm.visibility.correct", "frontends/romm/projection/correct.json", '{"source":"library/PlayStation/Synthetic Adventure.m3u","destination":"roms/psx/Synthetic Adventure.m3u","visibility":"same-path"}\n', classification="Metadata")
    builder.add_text("romm.visibility.invalid", "frontends/romm/projection/invalid.json", '{"source":"library/NES/Synthetic.nes","destination":"/outside/roms/nes/Synthetic.nes","visibility":"invalid"}\n', classification="Malformed", health="UnsafePath", expectation_type="Invariant")
    builder.add_file("romm.occupied-destination", "frontends/romm/roms/psx/Occupied.m3u", b"existing synthetic destination\n", classification="Metadata", health="NeedsReview")
    builder.add_virtual("romm.duplicate-destination", "frontends/romm/expected/duplicate-destination", classification="Ambiguous", health="NeedsReview", expectation_type="Invariant")
    builder.add_text("romm.launcher", "frontends/romm/roms/psx/Synthetic Launcher.m3u", "Synthetic Launcher (Disc 1).cue\n", classification="Playable", playable=True, media_set="romm.launcher-set")
    builder.add_text("romm.launcher.companion", "frontends/romm/roms/psx/Synthetic Launcher (Disc 1).cue", 'FILE "Synthetic Launcher (Disc 1).bin" BINARY\n TRACK 01 MODE2/2352\n', classification="Support", support_only=True, media_set="romm.launcher-set")


def add_paths_duplicates_sources(builder: Builder) -> None:
    base = builder.add_file("duplicate.original", "library/Path Cases/Original.bin", builder.bytes("duplicate.bytes", 256), classification="Ambiguous", identity="WeakCandidate")
    builder.add_file("duplicate.other-name", "library/Path Cases/Exact Copy Different Name.rom", (builder.root / base["relative_path"]).read_bytes(), classification="Ambiguous", identity="Verified", health="Duplicate", relationships=[base["fixture_id"]])
    builder.add_file("duplicate.other-directory", "library/Duplicates/Original.bin", (builder.root / base["relative_path"]).read_bytes(), classification="Ambiguous", identity="Verified", health="Duplicate", relationships=[base["fixture_id"]])
    builder.add_hardlink("duplicate.hardlink", "library/Path Cases/Hardlink.bin", base["relative_path"], classification="Ambiguous", identity="Verified", health="Duplicate", relationships=[base["fixture_id"]])
    builder.add_symlink("duplicate.symlink", "library/Path Cases/Symlink.bin", "Original.bin", classification="Ambiguous", identity="Verified", health="Duplicate", relationships=[base["fixture_id"]])
    near = bytearray((builder.root / base["relative_path"]).read_bytes()); near[-1] ^= 1
    builder.add_file("duplicate.near", "library/Path Cases/Near Duplicate.bin", bytes(near), classification="Ambiguous", identity="WeakCandidate", warning="One byte differs; must not be grouped by exact hash.")
    builder.add_file("duplicate.same-name-different", "library/Other/Original.bin", builder.bytes("different.same-name", 256), classification="Ambiguous", identity="WeakCandidate", warning="Same filename, different hash.")
    builder.add_file("path.spaces", "library/Path Cases/Name With Spaces.NES", builder.bytes("path.spaces", 64), platform="NES", classification="Playable", playable=True)
    builder.add_file("path.apostrophe", "library/Path Cases/Developer's Game.nes", builder.bytes("path.apostrophe", 64), platform="NES", classification="Playable", playable=True)
    builder.add_file("path.parentheses", "library/Path Cases/Game (Europe) (v1.1).nes", builder.bytes("path.parentheses", 64), platform="NES", classification="Playable", playable=True)
    builder.add_file("path.unicode", "library/Path Cases/Synthétique 日本語.nes", builder.bytes("path.unicode", 64), platform="NES", classification="Playable", playable=True)
    builder.add_file("path.long", "library/Path Cases/" + "L" * 180 + ".nes", builder.bytes("path.long", 64), platform="NES", classification="Playable", playable=True)
    builder.add_file("path.uppercase", "library/Path Cases/UPPERCASE.NES", builder.bytes("path.uppercase", 64), platform="NES", classification="Playable", playable=True)
    builder.add_file("path.mixedcase", "library/Path Cases/MixedCase.NeS", builder.bytes("path.mixedcase", 64), platform="NES", classification="Playable", playable=True)
    builder.add_file("path.hidden", "library/Path Cases/.hidden.nes", builder.bytes("path.hidden", 64), platform="NES", classification="Unknown", health="NeedsReview", expectation_type="DiagnosticOnly")
    builder.add_file("path.dotdir", "library/Path Cases/.support/hidden-support.bin", builder.bytes("path.dotdir", 64), classification="Support", support_only=True)
    builder.add_file("path.depth", "library/Path Cases/" + "/".join(f"level-{n:02d}" for n in range(16)) + "/deep.nes", builder.bytes("path.depth", 64), platform="NES", classification="Playable", playable=True)
    builder.add_symlink("symlink.safe-directory", "library/Symlink Cases/safe-dir", "../Path Cases/.support", classification="Support", support_only=True)
    builder.add_symlink("symlink.broken", "library/Symlink Cases/broken", "missing-synthetic-target", classification="Unknown", health="MissingCompanion", expectation_type="Invariant")
    builder.add_symlink("symlink.loop-a", "library/Symlink Cases/loop-a", "loop-b", classification="Malformed", health="UnsafePath", expectation_type="Invariant")
    builder.add_symlink("symlink.loop-b", "library/Symlink Cases/loop-b", "loop-a", classification="Malformed", health="UnsafePath", expectation_type="Invariant")
    builder.add_symlink("symlink.duplicate-two", "library/Symlink Cases/original-two", "../Path Cases/Original.bin", classification="Ambiguous", health="Duplicate", relationships=[base["fixture_id"]])
    builder.add_file("corrupt.zero", "library/Corrupt/zero-byte.rom", b"", classification="Malformed", health="Malformed", expectation_type="DiagnosticOnly")
    builder.add_file("corrupt.invalid-utf8", "library/Corrupt/invalid-utf8.json", b"{\"text\":\"\xff\"}", classification="Malformed", health="Malformed", expectation_type="DiagnosticOnly")
    builder.add_text("corrupt.absurd-count", "library/Corrupt/absurd-count.json", '{"count":999999999999,"items":[]}\n', classification="Malformed", health="Malformed", expectation_type="Invariant")
    builder.add_file("corrupt.extension-content", "library/Corrupt/not-really-a-game.nes", b"plain text with a conflicting extension\n", platform="NES", classification="Malformed", health="WrongExtension", expectation_type="DiagnosticOnly")
    source_matrix = {"schema_version": 1, "sources": [
        {"id": "healthy", "path": "library/Nintendo Entertainment System", "expected": "Healthy"},
        {"id": "missing", "path": "library/Does Not Exist", "expected": "Missing"},
        {"id": "unreadable-simulated", "path": "source-scenarios/unreadable-simulated", "expected": "ManualReview", "note": "No chmod: fixture remains cleanup-safe."},
        {"id": "empty", "path": "source-scenarios/empty", "expected": "Empty"},
        {"id": "nested", "path": "library", "expected": "NestedPlatforms"},
        {"id": "mixed", "path": "library/Path Cases", "expected": "Mixed"},
        {"id": "bios-only", "path": "support/bios", "expected": "SupportOnly"},
        {"id": "artwork-only", "path": "artwork", "expected": "ArtworkOnly"},
        {"id": "arcade", "path": "library/Arcade", "expected": "ExtractedArcade"},
        {"id": "archives", "path": "library/Archives", "expected": "ArchiveHeavy"},
    ]}
    builder.add_text("sources.matrix", "source-scenarios/source-matrix.json", json.dumps(source_matrix, indent=2) + "\n", classification="Metadata")
    (builder.root / "source-scenarios/empty").mkdir(parents=True, exist_ok=True)
    (builder.root / "source-scenarios/unreadable-simulated").mkdir(parents=True, exist_ok=True)


def add_playing_library(builder: Builder) -> None:
    records = [
        {"id": "parent-us-good", "title": "Synthetic Fighter", "region": "USA", "language": "en", "dump": "VerifiedGood", "parent": True, "expected_elected": True},
        {"id": "clone-jp-good", "title": "Synthetic Fighter", "region": "Japan", "language": "ja", "dump": "VerifiedGood", "cloneof": "parent-us-good", "expected_elected": False},
        {"id": "parent-us-bad", "title": "Synthetic Fighter", "region": "USA", "dump": "Bad", "expected_elected": False},
        {"id": "variant-eu", "title": "Synthetic Fighter", "region": "Europe", "language": "en,fr,de", "version": "1.1", "dump": "Unknown", "expected_elected": False},
        {"id": "multidisc", "title": "Synthetic Adventure", "media_set": "ps1.synthetic-adventure.complete", "dump": "VerifiedGood", "expected_elected": True},
        {"id": "duplicate", "title": "Synthetic Fighter", "same_identity_as": "parent-us-good", "expected_elected": False},
        {"id": "ambiguous-a", "title": "Synthetic Mystery", "dump": "Unknown", "expected_elected": None},
        {"id": "ambiguous-b", "title": "Synthetic Mystery", "dump": "Unknown", "expected_elected": None},
    ]
    builder.add_text("playing-library.intent", "metadata/playing-library/election-intent.json", json.dumps({"schema_version": 1, "records": records, "note": "Scenario intent only; EmuWiz remains authoritative."}, indent=2) + "\n", classification="Metadata", identity="StrongCandidate", expectation_type="ManualReviewExpected")
    builder.add_file("playing-library.support", "metadata/playing-library/support-only.bin", builder.bytes("playing-library.support", 32), classification="Support", support_only=True)


def add_scale(builder: Builder, scale: int) -> None:
    for index in range(scale):
        fixture_id = f"scale.file.{index:05d}"
        builder.add_file(fixture_id, f"scale/general/{index // 1000:03d}/Synthetic {index:05d}.rom", builder.bytes(fixture_id, 16), classification="Unknown", identity="Unknown", expectation_type="DiagnosticOnly", notes="Tiny deterministic scale entry.")


def build_manifest(builder: Builder, generation_seconds: float) -> dict[str, Any]:
    platform_counts = collections.Counter(entry["platform"] or "Unspecified" for entry in builder.entries)
    scenario_counts = collections.Counter(entry["scenario"] for entry in builder.entries)
    return {
        "schema_version": SCHEMA_VERSION,
        "generator_version": GENERATOR_VERSION,
        "repository_commit": builder.commit,
        "seed": builder.seed,
        "profile": builder.profile,
        "created_at": deterministic_timestamp(builder.seed),
        "determinism_note": "created_at is synthetic and seed-derived; timing is stored only in report.json",
        "legal_guarantee": "All payloads are generated synthetic bytes or fixture-authored metadata; no commercial ROM, BIOS, firmware, key, save, artwork, or credential data is included.",
        "vocabulary": {
            "classification": sorted(CLASSIFICATIONS), "identity": sorted(IDENTITIES),
            "health": sorted(HEALTH), "expectation_type": sorted(EXPECTATIONS),
        },
        "dependency_edges": builder.dependency_edges,
        "counts": {"fixtures": len(builder.entries), "platforms": dict(sorted(platform_counts.items())), "scenarios": dict(sorted(scenario_counts.items()))},
        "fixtures": sorted(builder.entries, key=lambda item: item["fixture_id"]),
    }


def write_manifest_markdown(root: Path, manifest: dict[str, Any]) -> None:
    lines = ["# EmuWiz Synthetic Library Manifest", "", f"Schema: `{manifest['schema_version']}`  ", f"Generator: `{manifest['generator_version']}`  ", f"Profile: `{manifest['profile']}`  ", f"Seed: `{manifest['seed']}`", "", manifest["legal_guarantee"], "", "| Fixture | Platform | Scenario | Classification | Health | Path |", "|---|---|---|---|---|---|"]
    for item in manifest["fixtures"]:
        lines.append(f"| `{item['fixture_id']}` | {item['platform'] or '—'} | {item['scenario']} | {item['expected_classification']} | {item['expected_health']} | `{item['relative_path']}` |")
    (root / "MANIFEST.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def iter_tree(root: Path) -> Iterable[Path]:
    for directory, dirnames, filenames in os.walk(root, followlinks=False):
        directory_path = Path(directory)
        # os.walk reports symlinked dirs in dirnames; yield them and prevent descent.
        retained = []
        for name in dirnames:
            path = directory_path / name
            if path.is_symlink():
                yield path
            else:
                retained.append(name)
        dirnames[:] = retained
        for name in filenames:
            yield directory_path / name


def tree_stats(root: Path) -> dict[str, int]:
    files = directories = symlinks = total = 0
    for directory, dirnames, filenames in os.walk(root, followlinks=False):
        directories += 1
        for name in list(dirnames) + filenames:
            path = Path(directory) / name
            info = path.lstat()
            if stat.S_ISLNK(info.st_mode): symlinks += 1
            elif stat.S_ISREG(info.st_mode): files += 1; total += info.st_size
    return {"bytes": total, "files": files, "directories": directories, "symlinks": symlinks}


def report_document(root: Path, manifest: dict[str, Any], failures: list[str], warnings_list: list[str], generation_seconds: float, verification_seconds: float, scan: dict[str, Any] | None = None) -> dict[str, Any]:
    stats = tree_stats(root)
    report = {
        "schema_version": SCHEMA_VERSION, "profile": manifest["profile"], "seed": manifest["seed"],
        "generator_version": GENERATOR_VERSION, "repository_commit": manifest["repository_commit"],
        "fixture_count": len(manifest["fixtures"]), **stats,
        "platform_counts": manifest["counts"]["platforms"], "scenario_counts": manifest["counts"]["scenarios"],
        "validation_failures": failures, "warnings": warnings_list,
        "timings_seconds": {"generation": round(generation_seconds, 6), "manifest_verification": round(verification_seconds, 6)},
    }
    if scan is not None: report["scan"] = scan
    return report


def write_report(root: Path, report: dict[str, Any]) -> None:
    (root / "report.json").write_bytes(canonical_json(report))
    lines = ["# Synthetic Library Validation Report", "", f"Profile: `{report['profile']}`", f"Seed: `{report['seed']}`", f"Fixtures: `{report['fixture_count']}`", f"Bytes: `{report['bytes']}`", f"Files/directories/symlinks: `{report['files']}` / `{report['directories']}` / `{report['symlinks']}`", f"Generation: `{report['timings_seconds']['generation']:.6f}s`", f"Verification: `{report['timings_seconds']['manifest_verification']:.6f}s`", "", f"Failures: `{len(report['validation_failures'])}`"]
    for failure in report["validation_failures"]: lines.append(f"- {failure}")
    lines.extend(["", f"Warnings: `{len(report['warnings'])}`"])
    for warning in report["warnings"]: lines.append(f"- {warning}")
    if "scan" in report:
        lines.extend(["", "## Optional EmuWiz scan", "", "```json", json.dumps(report["scan"], indent=2, sort_keys=True), "```"])
    (root / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def load_owned_marker(root: Path, *, allow_incomplete: bool = False) -> dict[str, Any]:
    candidates = [root / MARKER]
    if allow_incomplete: candidates.append(root / INCOMPLETE)
    for path in candidates:
        if path.is_file() and not path.is_symlink():
            try: value = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as error: raise LabError(f"invalid ownership marker {path}: {error}") from error
            if value.get("ownership") != OWNERSHIP_TOKEN or value.get("schema_version") != SCHEMA_VERSION:
                raise LabError(f"ownership marker is not recognized: {path}")
            return value
    raise LabError(f"refusing unowned directory: missing {MARKER}")


def lock_is_active(root: Path) -> bool:
    pid_path = root / LOCK / "pid"
    try:
        pid = int(pid_path.read_text(encoding="ascii").strip())
    except (OSError, ValueError):
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def verify_lab(root: Path, *, write: bool = True) -> tuple[dict[str, Any], list[str], list[str], float]:
    started = time.monotonic(); failures: list[str] = []; warning_list: list[str] = []
    root = validate_output_root(root)
    if not root.is_dir(): raise LabError(f"lab root is not a directory: {root}")
    marker = load_owned_marker(root)
    if (root / INCOMPLETE).exists(): failures.append("incomplete generation marker is present")
    manifest_path = root / "manifest.json"
    try: manifest_bytes = manifest_path.read_bytes(); manifest = json.loads(manifest_bytes)
    except (OSError, json.JSONDecodeError) as error: raise LabError(f"cannot load manifest: {error}") from error
    if manifest.get("schema_version") != SCHEMA_VERSION: failures.append("unsupported manifest schema")
    if marker.get("manifest_sha256") != sha256_bytes(manifest_bytes): failures.append("ownership marker manifest_sha256 mismatch")
    if marker.get("fixture_count") != len(manifest.get("fixtures", [])): failures.append("ownership marker fixture_count mismatch")
    expected_paths: set[str] = set(OPERATIONAL_FILES)
    for item in manifest.get("fixtures", []):
        try: rel = safe_relative(item["relative_path"])
        except (KeyError, LabError) as error: failures.append(f"invalid fixture path: {error}"); continue
        if item.get("kind") == "virtual": continue
        expected_paths.add(rel); path = root / rel
        try: info = path.lstat()
        except FileNotFoundError: failures.append(f"missing fixture: {rel}"); continue
        if item.get("kind") == "symlink":
            if not stat.S_ISLNK(info.st_mode): failures.append(f"expected symlink: {rel}"); continue
            target = os.readlink(path)
            if sha256_bytes(target.encode()) != item.get("sha256"): failures.append(f"symlink target changed: {rel}")
            lexical = Path(os.path.normpath(str(path.parent / target)))
            try: lexical.relative_to(root)
            except ValueError: failures.append(f"symlink escapes root: {rel} -> {target}")
        elif not stat.S_ISREG(info.st_mode): failures.append(f"fixture is not a regular file: {rel}")
        else:
            if info.st_size != item.get("size"): failures.append(f"size mismatch: {rel}")
            if sha256_file(path) != item.get("sha256"): failures.append(f"sha256 mismatch: {rel}")
    for path in iter_tree(root):
        rel = path.relative_to(root).as_posix()
        if rel in {INCOMPLETE, LOCK}: continue
        if rel not in expected_paths: failures.append(f"unexpected file: {rel}")
    cap = PROFILE_CAP.get(manifest.get("profile"), 0)
    stats = tree_stats(root)
    if not cap: failures.append("unknown profile size cap")
    elif stats["bytes"] > cap: failures.append(f"profile exceeds size cap: {stats['bytes']} > {cap}")
    for item in manifest.get("fixtures", []):
        if item.get("expected_archive_kind") != "Zip": continue
        path = root / item["relative_path"]
        if item["fixture_id"] == "archive.truncated": continue
        try:
            with zipfile.ZipFile(path) as archive:
                if archive.namelist() != item.get("archive_members"): failures.append(f"archive member list mismatch: {item['relative_path']}")
        except zipfile.BadZipFile: failures.append(f"invalid expected ZIP: {item['relative_path']}")
    # The authored MAME DAT is part of the contract: every non-placeholder
    # digest must identify generated chip bytes. A zero digest is reserved for
    # the deliberate missing-member scenario.
    for dat_relative, arcade_relative in [
        ("metadata/dats/emuwiz-synthetic-mame.xml", "library/Arcade"),
        ("metadata/dats/emuwiz-scale-mame.xml", "library/Arcade Scale"),
    ]:
        dat_path = root / dat_relative
        if not dat_path.exists():
            continue
        try:
            document = ET.parse(dat_path)
            for machine in document.findall(".//machine"):
                machine_name = machine.attrib.get("name", "")
                parent = machine.attrib.get("cloneof")
                for rom in machine.findall("rom"):
                    expected_sha1 = rom.attrib.get("sha1", "")
                    if not expected_sha1 or set(expected_sha1) == {"0"}:
                        continue
                    owner = parent if rom.attrib.get("merge") and parent else machine_name
                    member = root / arcade_relative / owner / rom.attrib.get("name", "")
                    if not member.is_file():
                        failures.append(f"DAT member missing: {dat_relative}: {owner}/{rom.attrib.get('name', '')}")
                        continue
                    if hashlib.sha1(member.read_bytes()).hexdigest() != expected_sha1:
                        failures.append(f"DAT SHA-1 mismatch: {dat_relative}: {owner}/{rom.attrib.get('name', '')}")
        except (ET.ParseError, OSError) as error:
            failures.append(f"generated MAME DAT cannot be validated: {dat_relative}: {error}")
    elapsed = time.monotonic() - started
    if write:
        existing = {}
        report_path = root / "report.json"
        if report_path.exists():
            try: existing = json.loads(report_path.read_text())
            except (OSError, json.JSONDecodeError): pass
        report = report_document(root, manifest, failures, warning_list, existing.get("timings_seconds", {}).get("generation", 0.0), elapsed, existing.get("scan"))
        write_report(root, report)
    return manifest, failures, warning_list, elapsed


def prepare_root(root: Path, profile: str, seed: int, recreate: bool) -> None:
    root = validate_output_root(root)
    if root.exists():
        if not root.is_dir() or root.is_symlink(): raise LabError("existing output root is not a real directory")
        load_owned_marker(root, allow_incomplete=True)
        if (root / LOCK).exists():
            state = "active" if lock_is_active(root) else "stale"
            raise LabError(f"output root has a {state} generation lock; cleanup must be explicit")
        if not recreate: raise LabError("owned output root already exists; pass --recreate to replace it")
        safe_remove(root, yes=True)
    try: root.mkdir(parents=True, exist_ok=False)
    except FileExistsError as error: raise LabError("another generator created the output root concurrently") from error
    incomplete = {"ownership": OWNERSHIP_TOKEN, "schema_version": SCHEMA_VERSION, "generator_version": GENERATOR_VERSION, "profile": profile, "seed": seed, "state": "incomplete"}
    (root / INCOMPLETE).write_bytes(canonical_json(incomplete))
    try:
        os.mkdir(root / LOCK)
        (root / LOCK / "pid").write_text(f"{os.getpid()}\n", encoding="ascii")
    except FileExistsError as error: raise LabError("another generator holds the lab lock") from error


def build_lab(root: Path, profile: str, seed: int, scale: int, arcade_sets: int, recreate: bool) -> dict[str, Any]:
    if scale < 0 or arcade_sets < 0: raise LabError("scale counts must be non-negative")
    started = time.monotonic(); root = validate_output_root(root); prepare_root(root, profile, seed, recreate)
    try:
        builder = Builder(root, profile, seed, repository_commit())
        add_platforms(builder); add_arcade(builder, arcade_sets); add_archives(builder); add_dat_fixtures(builder)
        add_bios_provider(builder); add_cheats_mods(builder); add_artwork_metadata_frontends(builder)
        add_paths_duplicates_sources(builder); add_playing_library(builder); add_scale(builder, scale)
        generation_seconds = time.monotonic() - started
        manifest = build_manifest(builder, generation_seconds)
        manifest_bytes = canonical_json(manifest)
        (root / "manifest.json").write_bytes(manifest_bytes); write_manifest_markdown(root, manifest)
        marker = {"ownership": OWNERSHIP_TOKEN, "schema_version": SCHEMA_VERSION, "generator_version": GENERATOR_VERSION, "repository_commit": builder.commit, "seed": seed, "profile": profile, "created_at": deterministic_timestamp(seed), "fixture_count": len(builder.entries), "manifest_sha256": sha256_bytes(manifest_bytes)}
        marker_tmp = root / f"{MARKER}.tmp"
        marker_tmp.write_bytes(canonical_json(marker)); os.replace(marker_tmp, root / MARKER)
        (root / INCOMPLETE).unlink(); shutil.rmtree(root / LOCK)
        _, failures, warning_list, verify_seconds = verify_lab(root, write=False)
        if failures: raise LabError("generated corpus failed validation: " + "; ".join(failures))
        report = report_document(root, manifest, failures, warning_list, generation_seconds, verify_seconds)
        write_report(root, report)
        # Reports alter total byte count; refresh once so report values describe the final tree.
        report = report_document(root, manifest, failures, warning_list, generation_seconds, verify_seconds)
        write_report(root, report)
        return report
    except Exception:
        lock = root / LOCK
        if lock.is_dir(): shutil.rmtree(lock)
        raise


def safe_remove(root: Path, *, yes: bool) -> None:
    root = validate_output_root(root, cleanup=True)
    if not yes: raise LabError("cleanup requires --yes")
    if root.is_symlink() or not root.is_dir(): raise LabError("cleanup root must be a real directory")
    load_owned_marker(root, allow_incomplete=True)
    if lock_is_active(root): raise LabError("refusing to remove a lab while its generator process is active")
    shutil.rmtree(root)


def locate_cli(repo: Path) -> Path | None:
    explicit = os.environ.get("EMUWIZ_CLI")
    candidates = [Path(explicit)] if explicit else []
    candidates.extend([repo / "target/release/emuwiz-cli", repo / "target/debug/emuwiz-cli"])
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK): return candidate.resolve()
    found = shutil.which("emuwiz-cli")
    return Path(found).resolve() if found else None


def run_scan(root: Path) -> dict[str, Any]:
    root = validate_output_root(root); manifest, failures, _, _ = verify_lab(root, write=False)
    if failures: raise LabError("refusing scan of invalid lab: " + "; ".join(failures))
    repo = Path(__file__).resolve().parents[2]; cli = locate_cli(repo)
    if cli is None: raise LabError("no built emuwiz-cli found; set EMUWIZ_CLI to an existing binary (the harness never builds production code)")
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="emuwiz-synthetic-scan-") as temp:
        isolated = Path(temp); config_root = isolated / "config"; data_root = isolated / "data"; home = isolated / "home"; mount = isolated / "mount"
        config_root.mkdir(); data_root.mkdir(); home.mkdir(); mount.mkdir()
        (config_root / "config.toml").write_text(f'source_folders = ["{root / "library"}"]\nmount_root = "{mount}"\nratarmount_bin = "ratarmount"\n', encoding="utf-8")
        env = {**os.environ, "HOME": str(home), "XDG_CONFIG_HOME": str(isolated / "xdg-config"), "XDG_DATA_HOME": str(isolated / "xdg-data"), "EMUWIZ_CONFIG_HOME": str(config_root), "EMUWIZ_DATA_HOME": str(data_root)}
        scan = subprocess.run([str(cli), "library-scan", "--json"], env=env, text=True, capture_output=True)
        if scan.returncode != 0: raise LabError(f"EmuWiz scan failed: {scan.stderr.strip()}")
        listing = subprocess.run([str(cli), "library-list", "--json"], env=env, text=True, capture_output=True)
        if listing.returncode != 0: raise LabError(f"EmuWiz list failed: {listing.stderr.strip()}")
        try: scan_json = json.loads(scan.stdout); rows = json.loads(listing.stdout)
        except json.JSONDecodeError as error: raise LabError(f"EmuWiz CLI returned non-JSON output: {error}") from error
        database = data_root / "library.sqlite3"
        with sqlite3.connect(f"file:{database}?mode=ro", uri=True) as connection:
            quick_check = connection.execute("PRAGMA quick_check").fetchone()[0]
        row_list = rows if isinstance(rows, list) else rows.get("entries", [])
        row_paths = [str(item.get("path", "")) for item in row_list]
        expectation_checks = [
            {
                "name": "safe NES candidate is catalogued",
                "passed": any(path.endswith("/Nintendo Entertainment System/Synthetic Nintendo Entertainment System.nes") for path in row_paths),
                "expectation_type": "ExactAssertion",
            },
            {
                "name": "safe single-game ZIP is catalogued",
                "passed": any(path.endswith("/Archives/Single Game.zip") for path in row_paths),
                "expectation_type": "ExactAssertion",
            },
            {
                "name": "raw extracted Arcade chip members are not standalone catalogue rows",
                "passed": not any("/Arcade/" in path and path.endswith(".bin") for path in row_paths),
                "expectation_type": "Invariant",
            },
        ]
        failed_checks = [item["name"] for item in expectation_checks if not item["passed"]]
        if failed_checks:
            raise LabError("high-confidence scan expectation failed: " + "; ".join(failed_checks))
        result = {"status": "passed", "cli": str(cli), "catalogue_rows": len(row_list), "sqlite_quick_check": quick_check, "elapsed_seconds": round(time.monotonic() - started, 6), "scan_summary": scan_json, "expectation_scope": "High-confidence scanner output plus SQLite integrity; complex DiagnosticOnly and ManualReviewExpected fixtures remain non-brittle."}
        result["expectation_checks"] = expectation_checks
    report_path = root / "report.json"; report = json.loads(report_path.read_text()); report["scan"] = result; write_report(root, report)
    return result


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    sub = result.add_subparsers(dest="command", required=True)
    build = sub.add_parser("build"); build.add_argument("--profile", choices=PROFILE_LEVEL, default="standard"); build.add_argument("--output", type=Path, default=Path(os.environ.get("EMUWIZ_SYNTH_ROOT", DEFAULT_ROOT))); build.add_argument("--seed", type=int, default=1); build.add_argument("--scale", type=int, default=0); build.add_argument("--arcade-sets", type=int, default=0); build.add_argument("--allow-large-scale", action="store_true"); build.add_argument("--recreate", action="store_true"); build.add_argument("--verify", action="store_true", help="accepted for readability; every build is always verified")
    validate = sub.add_parser("validate"); validate.add_argument("root", nargs="?", type=Path); validate.add_argument("--output", type=Path)
    remove = sub.add_parser("remove"); remove.add_argument("root", nargs="?", type=Path); remove.add_argument("--output", type=Path); remove.add_argument("--yes", action="store_true")
    scan = sub.add_parser("scan"); scan.add_argument("root", nargs="?", type=Path); scan.add_argument("--output", type=Path)
    return result


def selected_root(args: argparse.Namespace) -> Path:
    value = getattr(args, "output", None) or getattr(args, "root", None) or os.environ.get("EMUWIZ_SYNTH_ROOT")
    if value is None: raise LabError("a lab root is required")
    return Path(value)


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "build":
            if not args.allow_large_scale and (args.scale > 50000 or args.arcade_sets > 50000): raise LabError("scale and arcade-sets are capped at 50,000; pass --allow-large-scale explicitly to exceed the cap")
            if args.allow_large_scale and (args.scale > 250000 or args.arcade_sets > 250000): raise LabError("absolute safety cap is 250,000")
            report = build_lab(args.output, args.profile, args.seed, args.scale, args.arcade_sets, args.recreate)
            print(json.dumps({"root": str(args.output), "profile": args.profile, "fixtures": report["fixture_count"], "bytes": report["bytes"], "generation_seconds": report["timings_seconds"]["generation"]}, sort_keys=True))
        elif args.command == "validate":
            _, failures, warnings_list, elapsed = verify_lab(selected_root(args), write=True)
            print(json.dumps({"failures": failures, "warnings": warnings_list, "verification_seconds": round(elapsed, 6)}, sort_keys=True))
            if failures: return 1
        elif args.command == "remove": safe_remove(selected_root(args), yes=args.yes); print(f"removed owned synthetic lab: {selected_root(args)}")
        elif args.command == "scan": print(json.dumps(run_scan(selected_root(args)), indent=2, sort_keys=True))
    except LabError as error:
        print(f"synthetic-library: {error}", file=sys.stderr); return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
