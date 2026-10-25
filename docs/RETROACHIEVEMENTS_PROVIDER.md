# RetroAchievements Game Details provider

EmuWiz's RetroAchievements integration is optional, read-only, cache-first
metadata. It uses the official `API_GetGame` JSON contract; the endpoint is
authenticated with a user API key, so credentials are supplied by an explicit
provider configuration and are never written to the metadata cache or logs.

The provider retains a typed game ID, console ID, title, bounded achievement
summaries, points, icon reference, fetch time, and `RetroAchievements`
provenance. Cache publication is atomic and malformed/oversized responses are
refused. No HTML scraping, account-progress requests, polling, or launch/DAT
decisions are involved. Game matching must be established by a separate
strong identity/hash seam; title-only guesses are not accepted.

The cache lives at the EmuWiz data root under
`retroachievements/games.json`. Game Details renders a compact cached summary
or `Not linked` when no safe provider match is available. Network failure,
offline use, and an absent API key leave the rest of EmuWiz unaffected.

RetroAchievements documents game identification by console-specific hashes and
describes achievement sets as public game metadata:
[Game Identification](https://docs.retroachievements.org/developer-docs/game-identification.html)
and [How RA Works](https://docs.retroachievements.org/general/how-ra-works.html).
