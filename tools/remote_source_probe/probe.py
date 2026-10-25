#!/usr/bin/env python3
"""Bounded availability probe for a remote-backed archive source.

Proof-of-concept for docs/research/REMOTE_ARCHIVE_MOUNT_TORRENT_NZB.md
(sections 8 and 15). This is the one concrete gap this research found:
neither EmuWiz, decypharr, nor Altmount currently expose a *bounded*,
typed check of "is this file's backing data actually reachable right now,"
distinct from filesystem metadata (stat/realpath), which can succeed for
content whose remote backing has already gone away (see the aged-out
Real-Debrid case documented in this session's memory: stat/realpath
succeed, but `dd`/`head` hang for ~37s then return zero bytes).

This is deliberately small and standalone (no archivefs-core dependency):
it operates on any local path, remote-backed or not, which is exactly the
shape RatarmountBackend and every acquisition provider in this stack
already present to the OS. It does NOT read gigabytes to answer the
question - only a few bytes, under a hard wall-clock budget.

Usage:
    python3 probe.py /mnt/decypharr/__all__/SomeGame/game.zip
    python3 probe.py --budget-ms 2000 /mnt/psx-roms/some.chd

Exit code is 0 for Available/AvailableSlow, 1 for everything else, so this
composes into shell scripts and systemd health checks without extra
plumbing.
"""
from __future__ import annotations

import argparse
import enum
import os
import sys
import time
from dataclasses import dataclass


class Availability(enum.Enum):
    """States a remote-backed source can be in, independent of provider.

    These names are deliberately provider-agnostic (see the research doc's
    "generic source model" section) - a caller never needs to know whether
    the path underneath is decypharr/Real-Debrid, decypharr/AllDebrid, or
    an Altmount NZB import to interpret one of these.
    """

    AVAILABLE = "Available"
    AVAILABLE_SLOW = "AvailableSlow"
    UNAVAILABLE = "Unavailable"
    STALE_METADATA = "StaleMetadata"
    NEEDS_REACQUIRE = "NeedsReacquire"
    PARTIAL = "Partial"
    UNKNOWN = "Unknown"


@dataclass
class ProbeResult:
    path: str
    availability: Availability
    stat_ok: bool
    read_ok: bool
    elapsed_ms: float
    bytes_read: int
    detail: str

    def as_dict(self) -> dict:
        return {
            "path": self.path,
            "availability": self.availability.value,
            "stat_ok": self.stat_ok,
            "read_ok": self.read_ok,
            "elapsed_ms": round(self.elapsed_ms, 1),
            "bytes_read": self.bytes_read,
            "detail": self.detail,
        }


# Reads are capped hard - this probe's whole point is to never behave like
# the thing it is trying to detect (a multi-second-to-never hang pulling
# real bytes). PROBE_READ_BYTES is small enough to be free even over a
# genuinely slow remote link once the connection itself is live.
PROBE_READ_BYTES = 4096
# Threshold above which a successful-but-slow read is reported as
# AvailableSlow rather than Available - distinct from Unavailable, so a
# caller can choose to wait rather than immediately treat it as failed.
SLOW_THRESHOLD_MS = 1500.0


def _read_with_budget(path: str, budget_ms: float) -> tuple[bool, int, float, str]:
    """Attempt a small bounded read. Runs the actual read in a subprocess-free
    way using os-level non-blocking-ish polling: since a stalled FUSE read
    blocks the calling thread with no portable async read primitive in
    stdlib for regular files, this uses a short-lived child process so the
    parent can enforce the wall-clock budget by killing it - the read
    itself may block indefinitely in the kernel/FUSE layer (as directly
    observed against the aged-out decypharr case this session), but the
    prober never does.
    """
    import multiprocessing

    def _do_read(q: "multiprocessing.Queue") -> None:
        try:
            with open(path, "rb", buffering=0) as f:
                data = f.read(PROBE_READ_BYTES)
            q.put(("ok", len(data)))
        except Exception as exc:  # noqa: BLE001 - reporting, not handling
            q.put(("error", str(exc)))

    ctx = multiprocessing.get_context("fork")
    q: "multiprocessing.Queue" = ctx.Queue()
    proc = ctx.Process(target=_do_read, args=(q,))
    start = time.monotonic()
    proc.start()
    proc.join(timeout=budget_ms / 1000.0)
    elapsed_ms = (time.monotonic() - start) * 1000.0

    if proc.is_alive():
        proc.terminate()
        proc.join(timeout=1.0)
        if proc.is_alive():
            proc.kill()
            proc.join()
        return False, 0, elapsed_ms, f"read did not complete within {budget_ms:.0f}ms budget"

    if not q.empty():
        kind, payload = q.get()
        if kind == "ok":
            return True, int(payload), elapsed_ms, "read succeeded"
        return False, 0, elapsed_ms, f"read failed: {payload}"

    return False, 0, elapsed_ms, "probe process exited without reporting a result"


def probe(path: str, budget_ms: float = 5000.0) -> ProbeResult:
    """Bounded, typed availability check for one remote-backed path.

    Two independent checks, matching the exact failure signature this
    session found live: filesystem metadata (`stat`/`realpath`) can
    succeed while the actual backing data is gone - so a probe that only
    stats the path would have reported the aged-out Real-Debrid case as
    healthy. Only a real (small, bounded) read distinguishes them.
    """
    try:
        st = os.stat(path)
        stat_ok = True
        # A directory can never be "read" as bytes; report it as Available
        # purely on the metadata check succeeding - callers probing a
        # directory (e.g. a multi-file archive's mount root) want exactly
        # this, not a spurious read failure.
        if not os.path.isfile(path) or st.st_size == 0:
            return ProbeResult(path, Availability.AVAILABLE, True, True, 0.0, 0, "non-regular or empty file; metadata-only check")
    except OSError as exc:
        return ProbeResult(path, Availability.UNAVAILABLE, False, False, 0.0, 0, f"stat failed: {exc}")

    read_ok, bytes_read, elapsed_ms, detail = _read_with_budget(path, budget_ms)

    if read_ok and bytes_read > 0:
        availability = Availability.AVAILABLE_SLOW if elapsed_ms > SLOW_THRESHOLD_MS else Availability.AVAILABLE
        return ProbeResult(path, availability, stat_ok, True, elapsed_ms, bytes_read, detail)

    if read_ok and bytes_read == 0:
        # Read "succeeded" (no exception) but delivered nothing - the exact
        # decypharr symptom this session diagnosed for aged-out Real-Debrid
        # content: metadata is real, the byte stream is not.
        return ProbeResult(path, Availability.STALE_METADATA, stat_ok, False, elapsed_ms, 0, detail)

    # Read did not complete in budget, or errored outright.
    if elapsed_ms >= budget_ms:
        # Timed out rather than errored - could be transient provider
        # slowness (matches this doc's synthetic missing-7z-volume test,
        # which recovered cleanly once the volume came back) or a
        # permanently gone backend (the real decypharr case, ~37s to
        # nothing). A single probe cannot tell these apart - that is what
        # NeedsReacquire vs Unavailable is for at the caller's discretion
        # after repeated probes; this probe reports the more conservative
        # Unavailable and lets the caller apply its own retry policy.
        return ProbeResult(path, Availability.UNAVAILABLE, stat_ok, False, elapsed_ms, 0, detail)

    return ProbeResult(path, Availability.UNAVAILABLE, stat_ok, False, elapsed_ms, 0, detail)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("path", help="Path to probe (any remote-backed or local file)")
    parser.add_argument("--budget-ms", type=float, default=5000.0, help="Hard wall-clock budget for the read check (default 5000ms)")
    parser.add_argument("--json", action="store_true", help="Emit machine-readable JSON instead of a human summary")
    args = parser.parse_args()

    result = probe(args.path, budget_ms=args.budget_ms)

    if args.json:
        import json

        print(json.dumps(result.as_dict()))
    else:
        print(f"{result.path}")
        print(f"  availability : {result.availability.value}")
        print(f"  stat_ok      : {result.stat_ok}")
        print(f"  read_ok      : {result.read_ok}")
        print(f"  elapsed_ms   : {result.elapsed_ms:.1f}")
        print(f"  bytes_read   : {result.bytes_read}")
        print(f"  detail       : {result.detail}")

    return 0 if result.availability in (Availability.AVAILABLE, Availability.AVAILABLE_SLOW) else 1


if __name__ == "__main__":
    sys.exit(main())
