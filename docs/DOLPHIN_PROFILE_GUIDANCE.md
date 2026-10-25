# Dolphin per-game profile guidance

Open a verified GameCube or Wii game in Blue Game Details. Under readiness,
expand **Dolphin per-game guidance** for an existing Dolphin profile. The same
panel appears on its Launch candidate. Emulator selection and launch commands
remain unchanged; guidance does not grant launch permission.

Enter the absolute path of a local recommendation JSON document and choose
**Inspect / recheck guidance**. Inspection runs on a worker. No document,
configuration, game, save or controller mapping is written. There are no
network requests or automatic imports. Apply is unavailable.

The document is an array of existing `GameProfileSetting` records, for example:

```json
[
  {
    "key": "Core.MMU",
    "proposed_value": "True",
    "evidence": {
      "provider": "Your reviewed source, with its provenance",
      "source_version": "1",
      "source_date": "2026-10-17",
      "platform": "GameCube",
      "game_identity": "GALE01",
      "game_identity_confidence": "exact",
      "emulator": "Dolphin",
      "emulator_version": "5.0",
      "profile": "dolphin:/absolute/path/to/the/selected/profile",
      "reason": "Explain the evidence for this recommendation"
    }
  }
]
```

This is a schema example, **not a recommendation for GALE01 or any other game**.
Supply the actual verified platform/game identity, exact profile ID shown in
EmuWiz, version and supporting evidence. A filename or title cannot establish
these facts. User-supplied provenance is displayed as a source claim; EmuWiz
does not authenticate it or independently validate setting values.

A missing, mismatched or unknown version never produces an applicable portable
recommendation. Existing discovery only knows versions supplied by its authorised
outer probe; this panel never executes Dolphin to invent missing version evidence.
Matching version text does not prove setting support. Unknown keys remain
unproven; host settings need local review. Controller/accessory and system-state
settings are excluded from portable recommendations. No game-compatibility or
playability verdict follows from a recommendation.

Local comparisons inspect only `GameSettings/<verified-ID>.ini` under the selected
profile's existing data root. They do not resolve global, inherited,
revision-specific or currently running Dolphin settings. Conflicts retain both
values and provenance. Missing files, unavailable files and parse warnings remain
distinct. There is no claim that this snapshot is Dolphin's complete effective
configuration. Bare keys and differently spelled keys are not guessed equivalent.

Documents and local INIs must be regular UTF-8 files reached without symlinks,
each at most 256 KiB; documents contain at most 256 records. The panel records
SHA-256 digests of inspected bytes and labels results as snapshots. Recheck after
changing files or installations. Neither a cached result nor its hashes authorise
an operation. Existing launch preflight still revalidates independently.
