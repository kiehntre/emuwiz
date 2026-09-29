# Artwork startup and RomM connectivity

## What the interface shows

Every picture slot in Browse & Play (and every other artwork surface) is in
exactly one of these states:

| State | Words | Meaning |
|---|---|---|
| Index not ready | Preparing artwork… | The artwork index is being built in the background. Browse & Play stays usable. |
| Ready | (the picture) | Shown from the local cache or a local file. A cached RomM picture never needs RomM to be reachable. |
| Loading | Loading picture… | The index is ready and the picture is being fetched or decoded. |
| Provider unavailable | Picture unavailable / RomM cannot currently be reached. | A RomM picture is known but RomM cannot serve it now. **Not** the same as "no picture". |
| No picture anywhere | No picture yet | No provider has artwork for this game. |
| Failed, will retry | Picture temporarily unavailable / Will try again soon. | A load or decode failed; it is asked for again after 120 seconds while the game stays on screen. |

Technical detail (configured endpoint without any user-information part,
provider state, whether a cached image was used, retry timing, matching
reason) is under **Advanced details** for the selected game. Credentials are
never shown.

## Artwork index startup

The index is built on a background thread the first time any artwork-consuming
surface shows a picture after the catalogue is loaded. Browse & Play, Museum,
Artwork & Extras, Game Details and the showcase all use the same request, so
the index is built once and reused; nothing needs to visit another page first.

## RomM endpoint

EmuWiz uses the endpoint configured in **Sources & Providers** (stored in the
identity `config.json`). No host name is built in. The address must be an
`http`/`https` origin without a path that resolves to a loopback, private-LAN
or private container address. Failures are classified as: not configured,
switched off, DNS failure, connection refused/failed, timeout, TLS failure,
authentication failed, HTTP error, rate limited, or address refused by policy.

After a provider proves unreachable, further RomM requests fail fast for 60
seconds (no lookups, no sockets); then one probe is allowed. Pressing a retry
button probes immediately. Local, ES-DE, bundled and cached artwork are
unaffected.

## If a configured host name does not resolve

A host name such as `romm.<something>` only works where the local resolver (or
`/etc/hosts`) knows it. If RomM itself is running but its name does not
resolve on the machine running EmuWiz, either fix the name resolution or change
the address in Sources & Providers. Note that cached pictures are filed under
the configured origin; changing the address to a different origin makes new
requests use a new cache namespace.
