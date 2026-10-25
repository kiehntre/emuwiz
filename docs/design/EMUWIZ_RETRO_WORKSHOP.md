# EMUWIZ Retro Workshop

Visual system and rollout guide for EmuWiz 0.8.2 and later.

This document establishes a shared world for the GUI. It is a presentation guide,
not a change to parser, identity, media, repair, emulator, or navigation authority.

## The visual language

EmuWiz is a warm retro-computing workshop: a place where a person can inspect,
organise, repair, and preserve a collection with confidence.

- **Environment:** deep charcoal and teal shell, warm off-white text, restrained
  cyan/teal technical glow, and amber/gold primary actions.
- **Signature:** a small retro rainbow accent used as a detail, underline, or trim;
  never as an all-screen neon effect.
- **Materials:** worn notebooks, paper labels, CRTs, tapes, floppy disks,
  cartridges, storage boxes, keyboards, tools, shelves, and filing systems.
- **Mood:** clever, nostalgic, reassuring, hands-on, and slightly magical.
- **Avoid:** corporate SaaS panels, grey engineering walls, gamer-RGB overload,
  toy-like cartoon styling, visual clutter, and decorative state claims.

The existing theme roles remain the implementation baseline: `TEAL`/`INFO` for
technical signal, `AMBER`/`ACCENT` for primary action and selected emphasis,
`SUCCESS` for ready state, and `DANGER` for a meaningful failure. Text must remain
readable against the dark shell at every viewport size.

## The golden rule

> **Artwork creates emotion. egui communicates truth.**

Artwork can contain mood, objects, branding, generic CRT glow, and decorative
motifs. It must not contain live authority such as health, counts, update status,
progress, repair results, DAT status, or provider availability. Those values stay
in native egui controls, labels, badges, and disclosures.

Poster art is therefore a background or bounded visual layer. Buttons, selected
items, progress, errors, status text, and waveform/status overlays are live egui.
A missing asset must fall back to a usable native layout; art must never be a
functional dependency.

## Page classification

Classification is a layout decision, not a mandate to repaint every page.

### Full hero poster

These workflows have a strong object metaphor and benefit from a cinematic entry
point. Each hero still flows into ordinary, scrollable content.

- **Tape Inspector** — cassette bench, CRT waveform; inspect and preserve.
- **Disc Conversion** — disk/media workbench; preserve and transform safely, using
  wording limited to the actual conversion backend.
- **Emulator Setup** — setup notebook and diagnostic CRT; guided setup, no guesswork.
- **Cheats & Mods** — tinkering bench, notes, and cartridges; preview changes safely.
- **Library Organisation** — catalogue boxes, shelves, and labels; chaos to order.
- **Problems & Repair** — repair notebook, tools, and system-check CRT; fix calmly.
- **Museum** — shelves, boxes, old hardware, and memories; rediscover and preserve.

Tape Inspector is the reference implementation. A full poster is appropriate only
when the page has a clear metaphor, a small set of real primary actions, and enough
content below the fold to justify the visual entry point.

### Light branded header

These pages benefit from a compact identity/header treatment while keeping status
and data density first:

- DAT Sources
- Sources / Discovery
- Health
- Doctor
- History
- Library Views
- Mounts

Light headers may use a small mascot, icon, flourish, or signal panel, but should
not consume the vertical space needed for source cards, findings, or history.

### Dense utility / no large art

Tables, import queues, raw evidence, settings, and technical review surfaces stay
compact and readable. They may use the colour roles, badges, paper-label accents,
and collapsed technical details without a large poster. A utility page earns a
hero only when its task remains immediately obvious at 1024x600.

## Shared hero pattern

The reusable shape is deliberately small rather than a single framework:

1. **Identity:** icon or mascot, title, and one-sentence purpose.
2. **Trust/state:** one live egui badge such as Read only, Safe, Ready, or Needs
   review, with text/icon as well as colour.
3. **Visual metaphor:** a bounded poster or transparent object asset, with a live
   signal/status overlay only where the backend provides truthful state.
4. **Action row:** two or three real egui actions, ordered primary, secondary,
   and informational. No baked or invisible fake buttons.
5. **Content flow:** normal page content starts immediately below and remains
   reachable without depending on hero interaction.

Existing `page_hero`, `page_hero_with_motif_size`, `hero_card`, `signal_panel`,
`status_badge`, `format_chip`, and `technical_details` components are the natural
building blocks. Tape-specific live waveform behaviour should remain page-owned;
only bounded layout, asset loading, fallback, and action-row primitives should be
shared.

## Responsive sizing

- **Wide:** use the full cinematic composition and let the poster span the content
  width without creating a fixed canvas.
- **Medium:** preserve the focal object, reduce height and decorative margins,
  and keep the title/action block stable.
- **Narrow:** crop decorative edges first, shrink the object, and stack the title,
  status, and actions when necessary. Primary actions must never be hidden or
  clipped. Filters and technical details wrap or collapse; content scrolls.

At roughly 1024x600, the first viewport should communicate purpose and expose the
primary action. The poster is expendable; the action and explanation are not.

## Mascot family

EmuWiz is an original, reusable character rather than a likeness of a real person:
an eccentric older retro-computing wizard with white hair and beard, round glasses,
dark teal clothing, a small rainbow trim, and a wand emitting cyan data-light.
Subtle circuit motifs are acceptable. The mascot guides the user and should not
dominate every page.

Planned transparent runtime assets:

- `emuwiz_mascot_neutral.png` — welcome and neutral states
- `emuwiz_mascot_teaching.png` — pointing toward the next action
- `emuwiz_mascot_inspecting.png` — analysis and evidence workflows
- `emuwiz_mascot_thinking.png` — pending or ambiguous state
- `emuwiz_mascot_repairing.png` — repair/review workflow
- `emuwiz_mascot_success.png` — completed safe operation

## Decorative asset family

Use these sparingly as transparent PNG/SVG-style assets, never as dynamic-state
authority:

- `emuwiz_magic_divider_long.png`
- `emuwiz_magic_divider_short.png`
- `emuwiz_magic_corner_flourish.png`
- `emuwiz_magic_sparkles.png`
- `emuwiz_rainbow_underline.png`
- `emuwiz_crt_glow.png`
- `emuwiz_data_particles.png`

One flourish should clarify hierarchy. Repeating every motif on every card turns
the workshop into clutter.

## Page moods

| Page | Metaphor | User promise |
| --- | --- | --- |
| Tape Inspector | Cassette bench and CRT waveform | Inspect and preserve |
| Disc Conversion | Disk/media workbench | Preserve and transform safely |
| Emulator Setup | Setup notebook and diagnostic CRT | Guided setup, no guesswork |
| Cheats & Mods | Tinkering bench and cartridges | Tweak safely, preview first |
| Library Organisation | Shelves, boxes, and labels | Turn chaos into order |
| Problems & Repair | Notebook, tools, and check CRT | Fix calmly and safely |
| Museum | Shelves and remembered hardware | Rediscover and preserve |

The metaphor must match real capability. For example, a DSK-looking poster cannot
claim DSK conversion if the current backend only supports CUE/BIN to CHD.

## Status and copy

Status remains native egui. Recommended labels include Read only, Safe, Verified,
Ready, Needs review, Local, Preserved, DAT matched, Update available, and Still
checking. Never make colour the only signal.

Primary copy is plain, short, friendly, and calm. Keep internal names, IDs, raw
paths, CLI flags, and enum values under `Technical details`.

| Avoid | Prefer |
| --- | --- |
| Execute reconciliation transaction | Preview changes |
| Provider resolution unavailable | Media source could not be checked |
| Mutate selected records | Apply reviewed changes |

The exact technical reason remains available when it helps diagnosis; it simply
does not lead the first reading of the page.

## Runtime asset convention

Production GUI assets are bundled under `crates/archivefs-gui/assets/` (or a later
canonical GUI asset directory). Runtime code must never load directly from
`docs/design/reference`; those files are design provenance.

Recommended names:

```text
emuwiz_hero_tape_inspector.png
emuwiz_hero_disc_conversion.png
emuwiz_hero_emulator_setup.png
emuwiz_hero_cheats_mods.png
emuwiz_hero_library_organisation.png
emuwiz_hero_problems_repair.png
emuwiz_hero_museum.png
```

The currently bundled Tape Inspector asset is
`emuwiz_tape_inspector_hero.png`; retain it as the compatibility name until a
deliberate asset migration is made. Do not duplicate or rename assets merely to
make names look uniform.

Images are decoded/uploaded once through the existing GUI texture conventions,
cached for the page lifetime where appropriate, and given a safe native fallback.
No network loading or per-frame file reads are permitted.

## Accessibility and small-window rules

- Maintain high text contrast and readable button labels.
- Keep artwork behind or beside copy; never let it obscure instructions.
- Keep egui controls visibly interactive and keyboard/focus reachable.
- Pair colour with text, icon, or shape for every important state.
- Let heroes stack, crop, or collapse at narrow widths rather than forcing a
  horizontal canvas.
- Keep live state and safety language outside the artwork so it remains available
  to scaling, theme, and accessibility systems.

## Controlled rollout

### Wave 1 — high-value trust workflows

- Finish Tape Inspector poster polish and keep it as the visual reference.
- Apply the poster pattern to Emulator Setup once its install/update truth is
  stable.
- Apply the pattern to Problems & Repair with safety and preview language leading.

### Wave 2 — collection workflows

- Library Organisation: shelves, labels, and destination cards.
- Cheats & Mods: a restrained tinkering bench with preview-first actions.
- Museum: warm archival shelves, with live cover/count/status content in egui.

### Wave 3 — capability-dependent and compact surfaces

- Disc Conversion only after the art and copy match the real backend capability.
- Light branded headers for DAT Sources, Health, Sources, and related technical
  pages.
- Review whether the compact utility surfaces need a small mascot or flourish;
  do not enlarge them by default.

Roll out one workflow at a time, with wide, medium, and narrow screenshots plus
keyboard/readability checks before moving to the next page.

## Non-goals

This guide does not authorize a global theme rewrite, navigation change, backend
semantic change, identity/election change, dynamic artwork generation, or mass
page repaint. Future implementation passes must preserve each page's existing
authority and truthful capability.
