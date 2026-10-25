# GameHacking.org browser-assisted import

EmuWiz does not fetch or automate GameHacking.org when this flow is used.
The user opens the exact page in their own browser, then imports a saved HTML
page, copied page text, or a supported cheat export through the GUI.

Before writing anything, the bounded importer:

- rejects empty, oversized, unreadable, non-text, unrelated, and Cloudflare
  challenge content;
- checks the page/export's own GameHacking game and platform evidence against
  the selected local game's verified identity;
- refuses wrong-game, wrong-platform, mismatched Game ID, and PS2 serial
  imports;
- parses through the existing GameHacking provider parsers; and
- writes only the provider's existing cache key, with SHA-256 provenance and a
  copy of any replaced cache.

The browser opener accepts only a plain `https://gamehacking.org` URL and uses
the desktop handler with separate arguments. It never reads browser cookies,
credentials, or history; it never bypasses Cloudflare; and opening the page is
not an import.

GameCube accepts saved page HTML and Text exports. PlayStation 2 accepts the
PCSX2/PNACH export; a PS2 saved page is refused because that provider has no
page cache or page parser. Import is a cache/provider operation only: it does
not install or apply cheats. Existing preview/apply/rollback controls remain
separate and unchanged.
