# Philips CD-i optical analysis (V6)

ArchiveFS already had the platform identity gate for Philips CD-i: an
ISO9660 Primary Volume Descriptor whose system identifier begins `CD-RTOS`.
V6 adds read-only structural corroboration through
`cdi_disc_evidence::observe_cdi`.

The observer consumes the shared `LogicalMedia` interface. It validates the
`CD001` Primary Volume Descriptor, 2048-byte logical geometry, both-endian
volume/root fields, bounded root directory and path-table locations, and
records the volume identifier and startup evidence. A CD-i boot-record
descriptor (`CD-I`) and/or a root `STARTUP`/`.APP` entry are reported as
launch-structure evidence; no content is executed.

Statuses are deliberately conservative: `Confirmed` means the exact
`CD-RTOS` identifier and a coherent ISO9660 structure were observed;
`NotCdi` is a valid generic ISO; malformed/truncated structures fail closed.
Startup evidence is not a platform substitute and does not infer a title,
region, player model, or product code.

The logical reader currently exposes cooked 2048-byte sectors, so the CD-i
observer reports `Logical2048Only`. Plain ISO images cannot prove raw-sector,
audio-track, session, or Mode 2 Form 2 fidelity. Existing CHD logical-media
routing can feed the same observer where its selected data track is
supported; specialist/multi-track CHDs remain an explicit backend limitation.
CUE/BIN and raw-sector handling continue to use the shared optical readers;
no second parser or CUE repair path is introduced here.

DAT/Redump identity remains an independent external corroboration layer.
Agreement and conflict must both be preserved; this module has no winner
selection. No filesystem repair, conversion, extraction, rename, or write
path exists. CAS/UEF/tape formats and later LaserDisc verification remain
outside this lane.
