---
name: Archival Retrograde
colors:
  surface: '#0d1513'
  surface-dim: '#0d1513'
  surface-bright: '#333b38'
  surface-container-lowest: '#08100e'
  surface-container-low: '#161d1b'
  surface-container: '#1a211f'
  surface-container-high: '#242c29'
  surface-container-highest: '#2f3634'
  on-surface: '#dce4e0'
  on-surface-variant: '#bacac6'
  inverse-surface: '#dce4e0'
  inverse-on-surface: '#2a3230'
  outline: '#859490'
  outline-variant: '#3c4a47'
  surface-tint: '#3cddc8'
  primary: '#44e2cd'
  on-primary: '#003731'
  primary-container: '#03c6b2'
  on-primary-container: '#004c44'
  inverse-primary: '#006b5f'
  secondary: '#9dd1c6'
  on-secondary: '#003731'
  secondary-container: '#1a4f47'
  on-secondary-container: '#8cbfb5'
  tertiary: '#ffbf85'
  on-tertiary: '#4b2800'
  tertiary-container: '#ef9f4e'
  on-tertiary-container: '#663800'
  error: '#ffb4ab'
  on-error: '#690005'
  error-container: '#93000a'
  on-error-container: '#ffdad6'
  primary-fixed: '#62fae4'
  primary-fixed-dim: '#3cddc8'
  on-primary-fixed: '#00201c'
  on-primary-fixed-variant: '#005047'
  secondary-fixed: '#b8ede2'
  secondary-fixed-dim: '#9dd1c6'
  on-secondary-fixed: '#00201c'
  on-secondary-fixed-variant: '#1a4f47'
  tertiary-fixed: '#ffdcc0'
  tertiary-fixed-dim: '#ffb875'
  on-tertiary-fixed: '#2d1600'
  on-tertiary-fixed-variant: '#6b3b00'
  background: '#0d1513'
  on-background: '#dce4e0'
  surface-variant: '#2f3634'
typography:
  display-tv:
    fontFamily: spaceGrotesk
    fontSize: 48px
    fontWeight: '700'
    lineHeight: 56px
    letterSpacing: -0.02em
  headline-xl:
    fontFamily: spaceGrotesk
    fontSize: 36px
    fontWeight: '600'
    lineHeight: 44px
    letterSpacing: -0.015em
  headline-xl-mobile:
    fontFamily: spaceGrotesk
    fontSize: 28px
    fontWeight: '600'
    lineHeight: 36px
    letterSpacing: -0.01em
  headline-lg:
    fontFamily: spaceGrotesk
    fontSize: 28px
    fontWeight: '600'
    lineHeight: 36px
    letterSpacing: -0.01em
  headline-lg-mobile:
    fontFamily: spaceGrotesk
    fontSize: 22px
    fontWeight: '600'
    lineHeight: 30px
    letterSpacing: 0em
  headline-sm:
    fontFamily: spaceGrotesk
    fontSize: 20px
    fontWeight: '500'
    lineHeight: 28px
    letterSpacing: 0em
  body-lg:
    fontFamily: inter
    fontSize: 16px
    fontWeight: '400'
    lineHeight: 24px
  body-md:
    fontFamily: inter
    fontSize: 14px
    fontWeight: '400'
    lineHeight: 20px
  body-sm:
    fontFamily: inter
    fontSize: 12px
    fontWeight: '400'
    lineHeight: 18px
  spec-code-lg:
    fontFamily: jetbrainsMono
    fontSize: 13px
    fontWeight: '500'
    lineHeight: 18px
    letterSpacing: 0.02em
  spec-code-sm:
    fontFamily: jetbrainsMono
    fontSize: 11px
    fontWeight: '500'
    lineHeight: 16px
    letterSpacing: 0.04em
  label-caps:
    fontFamily: jetbrainsMono
    fontSize: 10px
    fontWeight: '600'
    lineHeight: 14px
    letterSpacing: 0.08em
rounded:
  sm: 0.125rem
  DEFAULT: 0.25rem
  md: 0.375rem
  lg: 0.5rem
  xl: 0.75rem
  full: 9999px
spacing:
  unit-2xs: 0.125rem
  unit-xs: 0.25rem
  unit-sm: 0.5rem
  unit-md: 0.75rem
  unit-lg: 1rem
  unit-xl: 1.5rem
  unit-2xl: 2rem
  unit-3xl: 3rem
  gutter-desktop: 1.5rem
  gutter-tv: 2.5rem
  margin-edge: 2rem
---

## Brand & Style

The design system embodies the calculated precision of high-end vintage audio rack gear, industrial hardware BIOS interfaces (such as Analogue OS and Pioneer LaserActive), and scholarly media archiving software. It is engineered for discerning video game preservationists, ROM collectors, and vintage hardware connoisseurs.

The visual direction deliberately rejects hyperactive esports tropes, aggressive RGB neon glow effects, and gamified SaaS conventions. Instead, it projects quiet competence, institutional stewardship, tactile permanence, and the measured cadence of vintage computing instruments.

Key aesthetics:
- **Precision Instrument Architecture:** Structured panels, hairline surface dividers, calibrated status LED dots, and monospaced technical metadata that treat game binaries as cultural artifacts rather than disposable files.
- **Physical Media Ergonomics:** Media cards evoke the physical weight and proportion of actual archival units—jewel cases, Famicom cassettes, PC-Engine HuCards, and magnetic disks.
- **Ten-Foot Legibility:** Spatial navigation designed for effortless readability from a distance via gamepads and remote controls, without compromising information density on high-DPI desktop setups.

## Colors

The palette is founded upon deep obsidian and graphite slate tones, referencing matte powder-coated rack equipment and CRT bezel plastics. Accent colors evoke precision calibrated test displays and cool phosphor screens.

### Palette Roles
- **Base Canvas (`#101418`):** Ultra-deep ink obsidian for structural viewports and full-screen backgrounds.
- **Surface Elevation 1 (`#191c21`):** Primary panel container, shelf views, and sidebars.
- **Surface Elevation 2 (`#1d2025`):** Secondary cards, active list rows, and inspect drawers.
- **Surface Elevation 3 (`#272a2f`):** Elevated overlays, modal dialogs, and focused controller targets.
- **Borders & Dividers (`#3c4a46` / `#859490`):** 1px structural hairline rules establishing rigid grid discipline.
- **Primary Accent (`#03c6b2` - Phosphor Teal):** Active system states, selected tabs, focused cursor halos, and system verification ticks.
- **Secondary Accent (`#4e8077` - Cerulean Slate):** Metadata telemetry, active core selection, and emulator engine designations.
- **Tertiary Accent (`#ffac5a` - Vintage Amber):** Region tags, dump revision indicators, and catalog indexing labels.
- **Verified Green (`#10b981`):** Redump, No-Intro, and TOSEC bit-perfect verification hashes.
- **Alert Amber (`#f59e0b`):** Missing BIOS or unverified header flags (deployed sparingly).
- **Foreground Typography (`#e1e2e9` / `#bacac5`):** Warm parchment white and muted slate text.

## Typography

The type system blends authoritative mid-century mechanical geometric forms with contemporary technical data structures.

- **Headlines & Platform Titles (`Space Grotesk`):** Chosen for its machined, technical character reminiscent of industrial hardware faceplates and microcomputer manuals.
- **Interface & Narrative Body (`Inter`):** Clean, neutral, and structurally transparent, ensuring long-form game summaries and release notes remain effortless to read.
- **Hardware Specs, Hashes & Cataloging (`JetBrains Mono`):** Dedicated to technical metadata: CRC32/SHA-1 checksums, memory mapping types, save formats, video clock rates, and emulator core performance metrics.

Typographic hierarchy requires uppercase styling with slight letter-spacing for system tags (`label-caps`) to mirror stamped equipment labels.

## Layout & Spacing

The layout is built on a rigid 4px base increment grouped into an adaptable multi-tier grid structure:

- **Desktop Library Mode:** 12-column dynamic grid with a fixed 280px left inspector panel, 24px gutters, and fluid center-stage library display.
- **10-Foot Lean-Back Mode (TV / Sunshine / Moonlight):** 6-column fixed aspect-ratio viewport with 40px gutters and generous 48px outer edge safe areas to accommodate TV display overscan and wide viewing angles.
- **Mobile / Handheld Mode (e.g., Steam Deck, Odin):** Single-column stacked or compact 2-to-3 item horizontal rail with a collapsible sheet drawer.

Spatial relationships emphasize information hierarchy: tight pairings (4px–8px) between metadata titles and monospaced values, with generous separation (24px–32px) between distinct platform carousels.

## Elevation & Depth

Visual depth is achieved through structural layering and hardware-inspired bevels rather than blurred shadows.

- **Stacking Architecture:**
  - Base canvas sits at `#101418`.
  - Content containers and shelves rest on `#191c21`.
  - Interactive cards sit on `#1d2025`.
  - Floating dialogs and popovers utilize `#272a2f`.
- **Beveled Structural Borders:** Every card and panel employs a crisp 1px border (`#3c4a46`). Raised containers incorporate an interior top highlight (`1px inset rgba(255, 255, 255, 0.04)`) to reproduce the chamfered edge of machined chassis equipment.
- **Focus States:** High-visibility double rings for controller and keyboard navigation: a 2px interior border using `#03c6b2` surrounded by a 4px semi-transparent boundary (`rgba(3, 198, 178, 0.2)`). Blur drops are kept minimal and clinical.

## Shapes

The design system employs a soft, machined geometric language (`roundedness: 1`). Radii are tightly controlled to mirror vintage microcomputers and audio chassis:

- **4px (`rounded-sm`):** System tags, cartridge metadata badges, technical chips, and form input controls.
- **8px (`rounded-md`):** Box art cards, emulator core preference tiles, and media preview frames.
- **12px (`rounded-lg`):** Main application panels, modal viewports, and system configuration trays.
- **Pill shapes (`rounded-full`) are strictly limited** to circular LED verification lights and hardware controller glyphs.

## Components

### Buttons & Trigger Controls
- **Primary Action (e.g., "Run Core", "Launch Game"):** Solid `#03c6b2` background, `#101418` bold text, 4px corner radius, with a subtle 1px top edge shine. Active focus introduces a crisp `#03c6b2` exterior halo.
- **Secondary Utility (e.g., "Manage Save States", "Scrape Metadata"):** Translucent `#1d2025` fill, `#3c4a46` border, `#e1e2e9` text, switching to `#272a2f` on hover/focus.
- **Icon Action:** Square 36x36px with centered monochrome vectors, enclosed in a 1px border.

### Media & Collection Cards
- **Cartridge / Disc Formats:** Enforces real-world physical ratios:
  - 3:4 for standard Famicom/NES and Western jewel cases.
  - 1:1 for Game Boy cartridges and optical jewel packaging.
  - 4:3 for arcade system board captures.
- **Header Stamp:** Subtle debossed top strip indicating ROM medium: `[RAW DUMP]`, `[CHIP 8MB]`, or `[CD-ROM XA]`.
- **Status Indicator:** 6px circular dot in the card corner (Emerald for verified dump, Slate for standard, Amber for patched/homebrew).

### Badges & Metadata Chips
- Monospaced typography (`spec-code-sm`).
- Rendered in a high-contrast dark box: `#191c21` background, 1px border matching the tag category (e.g., `#03c6b2` for verified ROMs, `#ffac5a` for region designations, `#4e8077` for core revisions).

### Data Tables & Specification Lists
- Split-column layout with fixed left labels in `label-caps` (`#859490`) and right values in `spec-code-lg` (`#e1e2e9`).
- Horizontal row separators rendered as 1px dashed lines (`#3c4a46`).

### Input Fields & Search Bars
- Recessed `#101418` background with `#3c4a46` outline.
- Monospaced placeholder text in `#859490`. Focused state transitions the border to `#03c6b2` without transition bounce.

### Controller Focus Overlay (10-Foot Experience)
- Every actionable card features an explicit non-shifting outline state.
- Accompanying audio-visual cue: persistent HUD footer displaying gamepad legend buttons (A: Select, X: Core Config, Y: Quick Save, Menu: Options) using authentic physical button colors.