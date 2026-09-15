# PS3 / Xbox 360 Device Deployment Architecture

Status: research only. No transport, credential store, console connection, or
remote write is implemented by this document.

## 1. Executive Summary

PS3 and Xbox 360 require separate adapters. Their remote services, identity
sources, path conventions, and safety guarantees differ.

PS3 deployments commonly use an FTP service supplied by webMAN MOD or
multiMAN in a CFW/HEN environment. webMAN also exposes HTTP commands, some of
which control the console or perform filesystem actions; HTTP must not become
EmuWiz's generic command channel. See the
[webMAN feature documentation](https://github.com/aldostools/webMAN-MOD/wiki/Features)
and [webMAN command documentation](https://github.com/aldostools/webMAN-MOD/wiki/Web-Commands).
FTP port 21 and HTTP port 80 are common defaults, but must be probed or
configured per device.

Xbox 360 deployments may use FTP when DashLaunch or another homebrew service
enables it, or XBDM when xbdm.xex is loaded on a JTAG/RGH environment. XBDM is
a debug-monitor protocol, not merely a file service. Common XBDM clients use
TCP 730 and UDP discovery 731, but endpoint and capability details must be
verified. See the [Xbox Debug Monitor reference](https://xboxdevwiki.net/Xbox_Debug_Monitor),
[XBDM client documentation](https://docs.rs/xbdm/latest/xbdm/), and
[DashLaunch settings](https://github.com/jrobiche/xbox360-aurora-developer-documentation/blob/main/docs/nova-0.7b.2r1622/schema_dashlaunch.md).

Recommended first implementation:

> PS3 read-only FTP capability probe plus bounded remote PARAM.SFO identity
> verification, tested entirely with a fake adapter.

No local catalogue match authorizes a remote write. Apply-time identity and
destination checks must be new observations.

## 2. PS3 Transport/Capability Model

webMAN MOD provides an FTP server and web server; its implementation exposes
ordinary operations such as read, store, size, listing, and rename, alongside
control extensions. multiMAN also has a built-in FTP server while running
([multiMAN overview](https://consolemods.org/wiki/PS3:MultiMAN)).
Rebug Toolbox is a local CFW management tool, not proof of a particular remote
transport.

Typical locations are only hints:

* installed content often uses /dev_hdd0/game/TITLE_ID/;
* extracted disc content may use a user-selected game directory;
* USB devices may appear as /dev_usb000 through /dev_usb007;
* /dev_bdvd may be a mounted disc view, not a stable install root;
* /dev_flash and /dev_blind are system areas and are outside the default Mods
  boundary.

The [webMAN setup documentation](https://github.com/aldostools/webMAN-MOD/wiki/Setup-Options)
describes these content locations and configurable scanning. It does not make
any particular path authoritative.

The PS3 adapter should report independently whether it can connect, list a
bounded reviewed directory, stat/read a bounded file, hash/read back bytes,
create an owned temporary file, upload, rename, backup, restore, or query
free space. FTP command availability does not prove atomic rename, durability,
link safety, or transactionality. Missing proof means unavailable.

PKG installation, RAP/license handling, package-manager commands, and system
area access are deferred. The first deployment boundary should cover only an
already-inspected loose-file operation relative to a remotely verified game
root.

## 3. Xbox 360 Transport/Capability Model

XBDM is associated with development kits and is commonly enabled on JTAG/RGH
systems using xbdm.xex and a plugin loader. It can expose file management as
well as debugging, memory, module, screenshot, and launch operations. EmuWiz
must allowlist only bounded file and identity operations. There must be no
generic XBDM command, memory, module, or XEX-launch API.

DashLaunch documents an optional ftpserv setting and commonly configured FTP
port 21. FTP availability is optional and depends on the dashboard and
plugins. An FTP handshake does not prove Xbox platform or JTAG/RGH state.

Extracted games may expose default.xex below an HDD/USB game directory. GOD
content, Title Updates, and DLC are distinct content classes and must not be
joined by folder-name folklore. A future adapter should read a caller-reviewed
XEX path and its bounded native header, not scan or reorganize content.

XBDM and FTP remain distinct transports even when served by one console. The
transport used for each read/write must be recorded in the receipt.

## 4. Adapter Boundary

Use separate typed boundaries:

* Ps3DeviceAdapter
* Xbox360DeviceAdapter

Each should return typed equivalents of DeviceIdentity, DeviceCapabilities,
RemoteGameIdentity, RemotePathResolution, DeploymentPreview, and
DeploymentReceipt.

Device identity contains platform evidence, transport, endpoint details,
capability probe, and session identifier. An IP address or hostname is only a
connection coordinate. Capabilities are an allowlist, not commands to try.
There is no arbitrary browse, recursive delete, shell execution, or generic
remote filesystem authority.

## 5. Device Identity

The minimum proof is a platform-specific handshake plus a bounded
capability/service probe. A debug name, IP, or remembered device profile is
not sufficient. MAC or hardware identifiers, if exposed, are sensitive and
should not be persisted by default.

A session ID or nonce should be locally generated for receipt correlation.
Firmware/environment facts may be recorded only when the protocol proves them.
Credentials are never part of catalogue records or receipts.

## 6. Remote Game Revalidation

Remote target identity is not authorized by local identity. At preview and
immediately before apply:

* PS3 reads bounded PARAM.SFO under a reviewed remote root and compares native
  TITLE_ID, with APP_VER as optional revision context.
* Xbox reads bounded default.xex or an explicitly reviewed XEX path and
  compares XEX Title ID and, when required, Media ID.

Missing or malformed native metadata is Unknown or Invalid. A verified
platform, Title ID, or required Media ID mismatch blocks before mutation.
Provider title strings, cached catalogue review, and local paths cannot
override a remote mismatch.

## 7. Destination Resolution

Resolution is:

    verified remote game root + safe relative provider intent
    = reviewed concrete remote destination

Absolute provider paths, traversal, guessed roots, device aliases, and
system-area targets are rejected. Link/reparse escape must be rejected when
the transport exposes link metadata. If FTP cannot prove that boundary, a
write requiring that proof is unavailable.

The review must display both declared relative intent and resolved destination.
It must never silently replace one with the other.

## 8. Pre-Write Verification

Before any future write:

1. The exact record and payload are user-selected.
2. Payload bytes were safely inspected and any supplied hash was verified.
3. Platform/session and capabilities were freshly probed.
4. Remote game identity matches the reviewed selection.
5. Destination remains confined to the reviewed root.
6. Current target size/hash/state matches the reviewed original.
7. Free space is known, or the operation is refused.
8. Backup, publish, verification, and rollback capabilities are available.
9. The user confirmed device, game, payload, destination, and replacement.

Failure is a pre-write refusal, not an implicit overrideable warning.

## 9. Backup Ownership

An owned backup must record transaction ID, device session and transport,
remote original path, original size/hash, backup remote path, backup
size/hash, and capture time.

If original bytes cannot be read, hashed, and copied to an
adapter-confirmed, operation-owned backup location, replacement is refused.
A backup is not merely “some copy somewhere.”

## 10. Transaction Receipts

Future receipts should extend existing EmuWiz transaction concepts with:

* transaction ID and operation status;
* device/session/transport and capability snapshot;
* catalogue provider record and payload identity;
* remote game identity;
* concrete destination and original identity;
* backup identity;
* written identity and rollback state.

Existing shared transaction ideas can provide deterministic operation IDs,
content digests, source/destination identity, backup metadata, and rollback
state. Remote fields must preserve degraded transport guarantees; an FTP
receipt must not imply atomicity.

## 11. Safe Write Sequence

Where the adapter proves the necessary capabilities:

1. Re-probe the device/session.
2. Re-read remote game identity and destination boundary.
3. Re-stat the current target and compare the reviewed original.
4. Upload to an EmuWiz-owned temporary path inside the reviewed boundary.
5. Verify temporary bytes by remote hash or bounded read-back hash.
6. Capture and verify the original backup.
7. Atomically rename/publish only where atomicity is proven.
8. Re-read and verify the final destination.
9. Persist the receipt only after successful verification.

Delete-then-upload is not atomic and should be refused in the safe V1 path.
Transfer success alone is not proof of final contents.

## 12. Read-Back Verification

Evidence ranking:

1. device-native cryptographic hash of the final path;
2. bounded read-back and local cryptographic hash;
3. verified size plus a transport integrity mechanism;
4. size only;
5. successful transfer response.

The last item cannot produce a successful deployment receipt.

## 13. Rollback / Uninstall

Rollback requires a matching receipt, a continuous or freshly re-established
device identity, an unchanged receipt-matching backup, and a destination that
still matches the EmuWiz-written identity. If the destination changed after
installation, stop and request review rather than overwriting it.

Uninstall may restore an owned original or delete an EmuWiz-created file only
when the receipt proves ownership. No broad delete or cleanup command exists.

## 14. Credential / Transport Security

Ordinary PS3 FTP/HTTP and Xbox FTP/XBDM should be treated as plaintext or
weakly authenticated LAN services unless a specific adapter proves otherwise.
webMAN remote-IP controls are access policy, not encryption. XBDM is a debug
surface and may have little meaningful authentication.

Future code must accept credentials only for the active session, never put
them in URLs/logs/receipts, never persist them in catalogue metadata, and use
OS secret storage only in a separately approved phase. Display a plaintext
LAN warning and do not support internet exposure or automatic port forwarding.

## 15. Failure Handling

| Failure | Required result |
|---|---|
| Connection lost mid-upload | Mark incomplete; clean only owned temporary data; never claim publish success. |
| Reboot or changed session | Re-probe and invalidate the prior preview. |
| Disk full | Stop before publish and leave no partial final file. |
| Target changed | Refuse publish; do not overwrite. |
| Backup succeeds, publish fails | Preserve incomplete receipt and owned cleanup/rollback state. |
| Publish succeeds, verification fails | Mark indeterminate; do not retry blindly. |
| Backup missing or differs | Block rollback. |
| Wrong console | Platform/identity mismatch blocks before mutation. |
| Link boundary unknowable | Refuse the operation requiring that proof. |
| Rename unproven | Refuse replacement in safe V1. |

## 16. Local Test Doubles

Use deterministic in-memory PS3 and Xbox fakes with immutable device identity,
configurable capabilities, bounded path/byte storage, stat/read, upload,
rename, hash/read-back, and failure injection. Simulate target replacement,
identity change, link escape, disk-full, connection loss, and missing
capabilities.

Core tests must prove zero writes on failed preconditions, exact receipt
ownership, path confinement, and rollback refusal after external change.
No sockets, credentials, real console, or live filesystem are needed.

## 17. Live Validation Plan

1. Read-only connection and capability probe.
2. Read-only remote game identity.
3. Write only in a dedicated throwaway directory.
4. Synthetic backup/publish/rollback in that directory.
5. Only then consider an explicitly reviewed user file.

Each stage is independently abortable and produces bounded evidence. A
read-only probe never automatically advances to a game-file write.

## 18. PS3 vs Xbox Comparison

| Capability | PS3 FTP/webMAN | Xbox FTP/XBDM | Evidence/safety caveat |
|---|---|---|---|
| Discovery | User endpoint; service probe | XBDM may provide discovery; FTP usually needs endpoint | IP/name is not stable identity. |
| Platform | FTP alone insufficient; PS3 service plus native SFO | XBDM handshake stronger; FTP alone insufficient | Require protocol and content evidence. |
| Game identity | Bounded PARAM.SFO Title ID/APP_VER | Bounded XEX Title ID/Media ID | Missing/malformed remains unknown/invalid. |
| Read | FTP read/list | XBDM read or FTP read | Read-back bytes are strongest practical proof. |
| Write | FTP store, variable guarantees | XBDM/FTP, variable guarantees | Capability must be explicit. |
| Rename | FTP behavior varies | XBDM/FTP behavior varies | Never assume atomicity. |
| Hash | webMAN-specific or local read-back | transport-specific or local read-back | Hash/read-back > size > transfer response. |
| Backup | bounded FTP copy/read | bounded adapter operation | Original and backup hashes required. |
| Rollback | receipt plus changed-target gate | same, with session changes | Never overwrite later user changes. |
| Authentication | service-dependent, often weak LAN trust | debug access often weak | No credential persistence; plaintext warning. |
| Encryption | ordinary FTP/HTTP unencrypted | FTP/XBDM not assumed encrypted | Trusted LAN only. |
| Special risk | HTTP commands can control/filesystem actions | XBDM can debug, read memory, and launch | Strict allowlists and separate adapters. |

## 19. Recommended First Implementation Slice

Implement only a PS3 read-only adapter capability probe and remote PARAM.SFO
identity verification:

* explicit endpoint and active session only;
* no saved credentials;
* bounded service handshake and file reads;
* reuse the existing PARAM.SFO parser and native identity projection;
* fake-adapter tests for capability absence, malformed SFO, Title ID match,
  mismatch, connection loss, and path confinement;
* typed output suitable for the existing catalogue review.

Do not add upload, rename, backup, rollback, HTTP command execution, package
installation, or Xbox support in this first slice.

## 20. Deferred Work

Credential management, network discovery, FTP/XBDM clients, package and Title
Update installation, SFB parsing, STFS/GOD/DLC joins, free-space guarantees,
atomic FTP replacement, recursive deployment, system-area writes, GUI
workflow, live-console testing, and degraded non-atomic replacement all remain
deferred.
