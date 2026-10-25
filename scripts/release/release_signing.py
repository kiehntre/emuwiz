"""Small, explicit detached-signature support for release manifests.

Private keys are imported only into a disposable temporary GnuPG home. They
are never copied into a release tree, archive, log, or generated metadata.
"""

from __future__ import annotations

import os
import pathlib
import re
import shutil
import stat
import subprocess
import tempfile
from typing import Any


class SigningError(Exception):
    """A signing or verification operation could not be completed safely."""


def available_signing_tools() -> list[str]:
    return [tool for tool in ("minisign", "signify", "cosign", "gpg") if shutil.which(tool)]


def _safe_file(path_value: pathlib.Path, label: str) -> pathlib.Path:
    path = pathlib.Path(path_value).expanduser()
    try:
        metadata = path.lstat()
    except FileNotFoundError as error:
        raise SigningError(f"{label} does not exist") from error
    if path.is_symlink() or not stat.S_ISREG(metadata.st_mode):
        raise SigningError(f"{label} must be a regular non-symlink file")
    absolute = path.absolute()
    resolved = path.resolve(strict=True)
    if resolved != absolute:
        raise SigningError(f"{label} must not resolve through a symlink")
    return resolved


def _safe_output(path_value: pathlib.Path, label: str) -> pathlib.Path:
    path = pathlib.Path(path_value).expanduser()
    if path.exists() or path.is_symlink():
        raise SigningError(f"refusing to replace existing {label}")
    if path.parent.is_symlink():
        raise SigningError(f"{label} parent is unsafe")
    parent = path.parent.resolve(strict=True)
    if parent != path.parent.absolute():
        raise SigningError(f"{label} parent is unsafe")
    return parent / path.name


def _gpg_version() -> str:
    executable = shutil.which("gpg")
    if executable is None:
        raise SigningError("gpg is not installed; provide a supported signing tool first")
    try:
        result = subprocess.run(
            [executable, "--version"],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise SigningError("could not inspect gpg version") from error
    return result.stdout.splitlines()[0] if result.stdout else "gpg"


def _run_gpg(home: pathlib.Path, arguments: list[str], *, text: bool = False) -> subprocess.CompletedProcess:
    executable = shutil.which("gpg")
    if executable is None:
        raise SigningError("gpg is not installed")
    try:
        return subprocess.run(
            [executable, "--batch", "--no-tty", "--homedir", str(home), *arguments],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=text,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise SigningError("gpg operation failed or timed out") from error


def _fingerprint(home: pathlib.Path) -> str:
    result = _run_gpg(home, ["--with-colons", "--list-secret-keys", "--keyid-format", "long"], text=True)
    if result.returncode != 0:
        raise SigningError("signing key has no usable secret key")
    for line in result.stdout.splitlines():
        fields = line.split(":")
        if len(fields) > 9 and fields[0] == "fpr":
            fingerprint = fields[9].strip().upper()
            if re.fullmatch(r"[0-9A-F]{16,64}", fingerprint):
                return fingerprint
    raise SigningError("signing key fingerprint could not be determined")


def inspect_signing_key(signing_key: pathlib.Path) -> dict[str, Any]:
    """Return public identity metadata without importing into the real keyring."""

    key = _safe_file(signing_key, "signing key")
    with tempfile.TemporaryDirectory(prefix="emuwiz-signing-inspect-") as temporary:
        home = pathlib.Path(temporary)
        os.chmod(home, 0o700)
        imported = _run_gpg(home, ["--import", str(key)])
        if imported.returncode != 0:
            raise SigningError("signing key could not be imported")
        return {
            "algorithm": "OpenPGP detached armored signature",
            "tool": "gpg",
            "tool_version": _gpg_version(),
            "key_fingerprint": _fingerprint(home),
            "detached": True,
            "archive_inclusion": "outside_reproducible_archive",
        }


def sign_detached_gpg(
    signed_file: pathlib.Path,
    signing_key: pathlib.Path,
    signature_file: pathlib.Path,
    public_key_file: pathlib.Path | None = None,
) -> dict[str, Any]:
    """Create an armored detached signature without retaining the private key."""

    signed = _safe_file(signed_file, "signed file")
    key = _safe_file(signing_key, "signing key")
    signature = _safe_output(signature_file, "signature file")
    public_key = _safe_output(public_key_file, "public key file") if public_key_file else None
    identity = inspect_signing_key(key)
    with tempfile.TemporaryDirectory(prefix="emuwiz-signing-") as temporary:
        home = pathlib.Path(temporary)
        os.chmod(home, 0o700)
        imported = _run_gpg(home, ["--import", str(key)])
        if imported.returncode != 0:
            raise SigningError("signing key could not be imported")
        fingerprint = identity["key_fingerprint"]
        staged_signature = home / "SHA256SUMS.asc"
        signed_result = _run_gpg(
            home,
            ["--armor", "--yes", "--detach-sign", "--local-user", fingerprint, "--output", str(staged_signature), str(signed)],
        )
        if signed_result.returncode != 0 or not staged_signature.is_file():
            raise SigningError("gpg could not create the detached signature")
        signature.write_bytes(staged_signature.read_bytes())
        os.chmod(signature, 0o644)
        if public_key is not None:
            exported = _run_gpg(home, ["--armor", "--export", fingerprint])
            if exported.returncode != 0 or not exported.stdout:
                raise SigningError("gpg could not export the public verification key")
            public_key.write_bytes(exported.stdout)
            os.chmod(public_key, 0o644)
    return {
        **identity,
        "signed_file": signed.name,
        "signature_file": signature.name,
        "public_key_file": public_key.name if public_key else None,
        "detached": True,
        "archive_inclusion": "outside_reproducible_archive",
    }


def verify_detached_gpg(
    signed_file: pathlib.Path,
    signature_file: pathlib.Path,
    public_key_file: pathlib.Path,
) -> bool:
    """Verify a detached signature using a disposable keyring."""

    signed = _safe_file(signed_file, "signed file")
    signature = _safe_file(signature_file, "signature file")
    public_key = _safe_file(public_key_file, "public key")
    with tempfile.TemporaryDirectory(prefix="emuwiz-signature-verify-") as temporary:
        home = pathlib.Path(temporary)
        os.chmod(home, 0o700)
        imported = _run_gpg(home, ["--import", str(public_key)])
        if imported.returncode != 0:
            return False
        verified = _run_gpg(home, ["--verify", str(signature), str(signed)])
        return verified.returncode == 0
