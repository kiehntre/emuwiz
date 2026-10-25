# EmuWiz UI/UX Design System Specification (v1.0)
**Codename:** Archival Retrograde
**Target Platform:** Linux Desktop (GNOME / KDE / Steam Deck / Living Room Big Picture / Sunshine & Moonlight 10-foot streaming)
**Reference Screens:** `Home / Gamer View`, `Cheats & Mods`, `Verify & DAT Coverage`, `Emulator Setup / Doctor`

---

## 1. Color Palette

The color system is built on a deep, cold-archival slate foundation with purposeful, restrained amber highlights and phosphorescent teal operational accents.

### Core Hex Tokens
| Role | Token Name | Hex Value | Usage & Guardrails |
| :--- | :--- | :--- | :--- |
| **Global Background** | `surface-lowest` | `#08100e` | Master canvas background, outer margins, letterbox areas. |
| **Surface Base** | `surface-base` | `#0d1513` | Top navigation chrome, footer telemetry bar, primary viewport container. |
| **Panel / Card Surface** | `surface-card` | `#161d1b` | Main cards, hero game container, technical breakdown boxes. |
| **Surface Raised / Active**| `surface-raised` | `#1e2825` | Hovered cards, active input pills, nested inset detail boxes. |
| **Border / Divider Normal** | `border-subtle` | `#23322d` | Standard 1px card outlines, horizontal rules, panel separators. |
| **Border Active / Focused**| `border-focus` | `#3b524a` | Focused elements, keyboard/gamepad tab stops. |
| **Primary Text** | `text-primary` | `#f1f5f9` (Slate-100) | Game titles, section headers, prominent values, key labels. |
| **Secondary Text** | `text-muted` | `#94a3b8` (Slate-400) | Descriptions, hardware specs, secondary labels, unselected pills. |
| **Subordinate / Meta Text**| `text-dim` | `#64748b` (Slate-500) | Monospace hashes, bus rates, slot IDs, breadcrumb slashes. |
| **Primary Accent (Warm Amber)** | `accent-amber` | `#f59e0b` (Amber-500) | **Restrained:** Primary `[A]` action buttons, active platform/card selection borders, critical warning badges. |
| **Primary Accent Dark** | `accent-amber-dark`| `#d97706` (Amber-600) | Active card badges, amber button hover states. |
| **Secondary Accent (Teal)** | `accent-teal` | `#03c6b2` | Healthy status pills, active tab indicator, TOSEC verified badges, healthy progress meters. |
| **Secondary Accent Glow** | `accent-teal-soft` | `rgba(3, 198, 178, 0.12)` | Subtle glow under selected nav tabs, healthy core badges. |
| **Warning / Attention** | `status-warning` | `#f59e0b` | Missing BIOS, unverified dumps, orphaned ROM paths. |
| **Error / Failed** | `status-error` | `#ef4444` (Red-500) | Missing required boot assets, checksum mismatches, corrupt files. |
| **Success / Verified** | `status-success` | `#10b981` (Emerald-500) | 100% DAT match, verified SHA-1, healthy JIT memory hook. |

> **Critical Rule:** Never flood panels with yellow/amber. Amber is strictly an "ignition" and "active focus" color. Teal and slate govern 90% of the UI chrome.

---

## 2. Typography Hierarchy

The type system blends geometric archival headers (`Space Grotesk`) with ultra-crisp, high-density monospace engineering labels (`JetBrains Mono` / `Share Tech Mono`).

| Level | Typeface | Size / Weight | Line Height | Case / Tracking | Example Usage |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Page / Screen Title** | Space Grotesk | 26px (1.625rem) / 700 Bold | 1.2 | Uppercase, tracking +0.05em | `COLLECTION INTEGRITY AUDIT & TOSEC/NO-INTRO RECONCILIATION` |
| **Game / Hero Title** | Space Grotesk | 34px (2.125rem) / 700 Bold | 1.15 | Title Case, tracking -0.01em | `Acheton and Kingdom of Hamil` |
| **Section Heading** | Space Grotesk | 18px (1.125rem) / 700 Bold | 1.3 | Uppercase, tracking +0.04em | `INDEXED PLATFORMS (6 ACTIVE TIERS)` |
| **Card Header / Title** | Space Grotesk | 15px (0.9375rem) / 600 SemiBold| 1.35 | Title Case / Mixed | `PCSX-ReARMed / DuckStation`, `Snes9x` |
| **Body / Description** | System Sans / Inter | 14px (0.875rem) / 400 Regular | 1.55 | Normal Case | Game synopsis, diagnostic explanations, mod summaries. |
| **System Labels & Pills**| JetBrains Mono | 11px (0.6875rem) / 600 SemiBold| 1.2 | Uppercase, tracking +0.08em | `ARM ARCHITECTURE // ACORN`, `ARM7TDMI // GBA` |
| **Technical Details / Meta**| JetBrains Mono| 12px (0.75rem) / 500 Medium | 1.4 | Monospace, tabular numerals | `0x0014B2`, `dc2b9bf8f43fa726e...`, `60 FPS • 4K HDR` |
| **Primary Button Label** | Space Grotesk | 14px (0.875rem) / 700 Bold | 1.0 | Uppercase, tracking +0.06em | `[A] LAUNCH GAME`, `[A] AUTO-FIX MISSING BIOS` |
| **Secondary Button Label**| Space Grotesk | 13px (0.8125rem) / 600 SemiBold| 1.0 | Uppercase, tracking +0.04em | `[X] SCAN FIRMWARE FOLDER`, `[Y] CHEATS & MODS` |

---

## 3. Spacing System

Based on a strict 4px/8px modular cadence optimized for consistent 16:9 widescreen layouts.

* **Page Margins (Outer Frame):**
  * Desktop standard: `24px` horizontal, `20px` vertical.
  * 10-foot / TV overscan buffer: `32px` minimum boundary padding.
* **Header / Navigation Bar:** `56px` height; internal padding `12px 24px`.
* **Platform Strip:** `40px` height; item gap `8px`.
* **Section Gap:** `24px` vertical spacing between distinct modules (e.g., hero panel to shelf).
* **Grid Card Gaps:** `16px` standard column/row gap.
* **Card Padding:**
  * Hero Game Showcase: `24px` internal padding.
  * Standard Doctor / Audit Cards: `20px` internal padding.
  * Shelf / Shelf Game Cards: `14px` padding around cover art and metadata block.
* **Button Spacing:**
  * Primary Action CTA: `14px 28px` padding; icon-to-label gap `10px`.
  * Secondary / Utility Buttons: `10px 18px` padding.
  * In-line Button Groups: `12px` gap between adjacent action triggers.

---

## 4. Card System

### A. Hero Game Card (Featured Title)
* **Structure:** Two-column split layout. Left: Archival typography, runtime target specs (`Arculator v2.1`, `3.5" DSDD Floppy`), and primary amber Launch CTA. Right: 3:4 physical box/cassette showcase with glowing platform badge and 5-star rating.
* **Surface:** `surface-card` (`#161d1b`) with 1px `border-subtle` (`#23322d`). Border radius: `6px`.

### B. Library Game Card (Shelf View)
* **Structure:** Vertical stack. Top: System/Year badge (`ACTIVE 1987`, `FLOPPY 1990`). Center: 4:3 or 3:4 archival cover art with subtle CRT inset border. Bottom: Game title, genre tag (`Text Adv`, `3D Sim`), and file size (`800KB`, `1.2MB`).
* **Active State:** Solid `1.5px` warm amber border (`#f59e0b`), amber year badge, amber-lit star ratings, and 4px ambient elevation glow.
* **Inactive State:** `1px` subtle slate border (`#23322d`), muted typography, neutral badge.

### C. Status Card (Diagnostic & Health Overview)
* **Structure:** Metric row with high-contrast numerical figures (e.g., `98.4% VERIFIED`, `7 CORES HEALTHY`).
* **Indicators:** Dual-color progress bars (`#03c6b2` for healthy, `#f59e0b` for pending/attention).
* **Corner Badges:** Round status seals (e.g., circular TOSEC seal, lock icon, chip icon).

### D. Warning / Problem Card (Core Doctor Action Required)
* **Structure:** High-visibility amber-toned card header badge (`ATTENTION REQUIRED`), accompanied by an explicit missing dependency label (`SCPH-5501.BIN Missing from /bios directory`).
* **Action:** Direct remediation button in amber (`DOWNLOAD LEGAL BIOS DUMP` or `RE-MAP PATH`) placed alongside impact level explanation (`Boot Failures on NTSC-U Titles`).

### E. Technical Details Card / Live Buffer Preview
* **Structure:** Inset terminal-style slate container (`#08100e`) with green/teal monospace readout (`VIDC1A CRT SCANNER // MODE 31 GRAPHICS`).
* **Hierarchy:** High-level status on top; raw register values and memory hook addresses below in 60% opacity monospace.

---

## 5. Button & Controller Action Hierarchy

Every button has both mouse-click styling and physical gamepad input cues (`[A]`, `[B]`, `[X]`, `[Y]`, `[L1/R1]`).

| Type | Visual Style | Gamepad Glyph | Use Case |
| :--- | :--- | :--- | :--- |
| **Primary Action** | Filled warm amber (`#f59e0b`), dark charcoal text (`#0d1513`), bold font, subtle amber drop-shadow. | `[A]` (Dark amber circle, high contrast) | `LAUNCH GAME`, `AUTO-FIX MISSING BIOS`, `RUN SMART VERIFY`. |
| **Secondary Action** | Dark slate surface (`#1e2825`), light slate text (`#f1f5f9`), 1px border (`#3b524a`). | `[X]` or `[Y]` (Muted grey circle) | `CHEATS & MODS`, `SCAN FIRMWARE FOLDER`, `SAVE STATES`. |
| **Quiet / Tertiary** | Ghost style, transparent background, light slate text with hover border. | Monospace key hint (`⌘K`, `L1/R1`) | Search trigger, system cycle, filter switches. |
| **Destructive Action** | Muted dark crimson container (`rgba(239, 68, 68, 0.15)`), red border (`#ef4444`), red text. | `[B]` or explicit label | `PURGE UNMATCHED ZERO-BYTE FILES`, `UNMOUNT ALL`. |

---

## 6. Status Language & Semantic States

Standardized system states across all diagnostics, scrapers, and verification screens:

1. **Ready / Pristine:**
   * *Badge:* Solid teal pill (`● 100% HEALTHY` / `PRISTINE VAULT`).
   * *Meaning:* BIOS present, SHA-1 matches TOSEC/No-Intro DAT standard, core runtime active.
2. **Needs Attention / Action Required:**
   * *Badge:* Warm amber pill (`● ATTENTION REQUIRED` / `1 REQUIRES BIOS FIRMWARE`).
   * *Meaning:* Game playable with glitched fallbacks, or missing optional BIOS / overlay assets.
3. **In Progress / Verifying:**
   * *Badge:* Glowing cyan animated pulse (`● LIVE I/O BUS // 2.8 GB/s`).
   * *Meaning:* Actively hashing, scanning mounts, or downloading patches.
4. **Failed / Problem States:**
   * *Badge:* Red pill (`● CRITICAL MISSING` / `CONFIG ERROR`).
   * *Meaning:* Path unreadable, permission denied, or corrupt ROM header.
5. **Not Confirmed / Unknown / Unmatched:**
   * *Badge:* Neutral slate pill (`○ 915 UNCATALOGUED TAPES` / `UNKNOWN REVISION`).
   * *Meaning:* File exists on disk but is not indexed in official No-Intro/TOSEC DATs.

---

## 7. Progress & Task Behavior

* **Short Tasks (< 2 Seconds):**
  * Inline spinning glyph (animated 16px segmented loader) within the button itself, transitioning directly to a checkmark (`✓`).
* **Long Tasks (> 2 Seconds, e.g., DAT Audit, Full Library Re-hash):**
  * High-visibility segmented dual-color progress bar (Height: `8px`, rounded `4px`).
  * Teal fill (`#03c6b2`) representing verified items; amber segment representing items needing action.
  * Real-time metadata label: `Re-indexing 19,376 files // Mounted from /mnt/usb_games`.
  * Safe cancel button: Always provide a secondary `[B] CANCEL SCAN` that flushes the write cache cleanly.
* **Completion State:**
  * Persistent summary banner showing exact counts: `882/882 Acorn Archimedes matched • Database sync complete`.

---

## 8. Artwork & Cover Presentation Rules

* **Aspect Ratios:**
  * Floppy / Boxed PC / Microcomputer: Standard `3:4` or `1:1` square media preview.
  * Console Cartridges (SNES, MegaDrive): `3:4` vertical poster ratio.
* **Scanline & CRT Bezel Polish:**
  * Artwork containers feature a 1px inner dark border (`rgba(0,0,0,0.6)`) and subtle vignette effect.
  * Artwork never uses aggressive neon drop shadows or generic SaaS blur backgrounds.
* **Fallback Imagery:**
  * When no cover art exists, display an archival blueprint placeholder: clean vector geometry (floppy disk, cartridge, or microchip icon) with platform-accurate monochrome linework and game title printed in monospace.

---

## 9. Responsive, Widescreen & 10-Foot TV Rules

* **Target Canvas:** Native 1920x1080 (1080p) and 2560x1440 (1440p) desktop.
* **Minimum Viewport:** `1100x720px` without horizontal scrolling (cards wrap into 3-column rows).
* **Sunshine / Moonlight / Couch Play Readability:**
  * Minimum UI body text: `12px` monospace; minimum primary heading: `18px`.
  * High-contrast text ratio: Slate-100 (`#f1f5f9`) on `#161d1b` delivers a **12.4:1 contrast ratio**, well above WCAG AAA standards.
  * Footer telemetry bar persistently surfaces gamepad navigation legends (`(A) Select / Launch`, `(B) Back`, `(X) Options`, `(Y) Quick Menu`, `(L1/R1) Cycle Systems`).

---

## 10. Technical-Details Hierarchy (The Subordination Rule)

EmuWiz is both a game launcher for players and an archival preservation suite for power users:

1. **Rule of Clean Presentation:** Beginner-facing, actionable information is always top-level and immediately scannable (e.g., `Play Game`, `7 Cores Healthy`, `98.4% Verified`).
2. **Subordinate Technical Evidence:** Deep technical evidence (SHA-1 hashes, memory allocation addresses, JIT flags, Bus timings) is never hidden in separate buried modals, but is placed directly beneath primary labels using:
   * Tabular monospace typography (`JetBrains Mono`).
   * Reduced optical weight (`text-dim` `#64748b` or `text-muted` `#94a3b8`).
   * Compact chip/pill containers (`bg-surface-raised border border-subtle`).
3. **Never Lose Data:** Users should never have to open a terminal to verify if a ROM header was trimmed or which BIOS revision is active.

---

## 11. Accessibility & Usability

* **Color Independence:** Status indicators always pair color with text or iconography (`● 100% HEALTHY [Checkmark]`, `▲ ATTENTION REQUIRED [Warning triangle]`).
* **Hit Targets:** Minimum interactive button target size is `44px` height on desktop and gamepad navigation.
* **Focus Rings:** Distinctive 2px warm amber or high-contrast teal outline on focused cards and buttons, ensuring effortless 10-foot gamepad navigation from 10 feet away.

---

## 12. The EmuWiz Personality: What Makes It Unique

EmuWiz rejects the three common traps of emulator frontends:
1. **Not a Generic AI Dashboard:** No rounded corporate bubble cards, no pastel gradients, no gratuitous whitespace, and no sterile SaaS metrics.
2. **Not an Illegible 90s Cheat Tool:** No pitch-black neon green hacker text, no unstyled tables, and no confusing hex-editor sprawl.
3. **Not a Locked-Down Mobile App:** It treats the user as an owner and archivist, respecting Linux file paths (`/mnt/storage/emuwiz/roms`), raw media formats (`.adf`, `.ssd`, `chd`), and verification standards (`TOSEC`, `No-Intro`, `Redump`).

**The EmuWiz Identity:** An authentic, physical-feeling **tactile archival computer deck** — combining the industrial dignity of 1980s microcomputers (Acorn Archimedes, Amiga) with modern, high-precision workstation ergonomics and 10-foot living room comfort.