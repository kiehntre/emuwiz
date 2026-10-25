# Atari ST media in Blue

Inspected against origin/main `2039984d31666aa9aed8081a8a2422ce2ece7941`.

## Existing infrastructure and missing connection

The existing `disk_format/atari_st.rs` validates bounded raw ST/FAT12 geometry;
`disk_format/atari_stx.rs` validates bounded Pasti headers and track records.
Raw FAT12 geometry also occurs on DOS disks. It is not conclusive Atari ST
identity. Pasti structural evidence establishes the platform/representation,
not a catalogue release or playable sector payload.

Collection Discovery previously admitted arbitrary `.st` and `.stx` bytes under
an Atari ST source through its generic extension/folder path. A synthetic garbage
fixture reproduced that acceptance before the correction. Discovery now reuses
the bounded inspectors and platform detector. Raw ST requires confirmed source
corroboration; STX requires valid Pasti structure and consistent platform evidence.
Neither path verifies a release, decodes protected sectors or invents disk sets.

Game Details already showed these structural inspectors. Its canonical launch
identity bridge deliberately has no verified Atari ST title fact. Previously this
fell through to generic unknown-identity guidance without showing existing Hatari
profile/TOS observations. The new separate, read-only Atari ST readiness snapshot
connects these observations to Blue and Advanced details. It never enters the
executable LaunchPlan variant, persists identity or creates a verified title.

## Read-only readiness

Only structurally supported ST/STX files are inspected, with a 32 MiB maximum,
selection-bound background results and profile count bound. Existing Hatari
profile discovery, config interpretation, firmware guidance and preflight are
reused. Each profile is shown separately; multiple profiles do not select one for
execution. An opaque content hash identifies the inspected bytes only. TOS
filename/presence is reported as unverified, and per-game machine, TOS version
and region compatibility remain unknown without authoritative evidence.

The proposed command shows the exact executable, config and selected disk in
A. Missing TOS, unknown machine, ambiguous executable or unavailable profile
retain existing refusal reasons. Filename disk/side/count hints are explicitly
unverified: no additional disk is paired, ordered or marked complete.
MSA, DIM and IPF are outside this structural readiness slice even where the
standalone backend knows an extension. No new decoding/conversion is provided.

## Native launch remains blocked

The current native Hatari backend is insufficient to guarantee preservation of
original media/effective settings. Hatari loads system/user configuration before
processing arguments, and `--configfile` adds settings rather than replacing
all existing settings. `--disk-a` alone does not isolate inherited storage,
automatic drive-B insertion or writeback. A preview is therefore explicitly
labelled **not authorised for execution**, every candidate is blocked and Blue
exposes no Play action for this snapshot.

Upstream evidence (read-only source inspection):

- [startup configuration](https://github.com/hatari/hatari/blob/main/src/main.c)
- [additional config and disk options](https://github.com/hatari/hatari/blob/main/src/options.c)
- [automatic drive-B insertion and writeback](https://github.com/hatari/hatari/blob/main/src/floppy.c)

Completing native Launch requires an independently reviewed effective-runtime
preservation contract, including inherited storage and write paths, followed by
fresh preflight/execution binding. This change does not enable or redesign that
backend. No real emulator is invoked by its tests.
