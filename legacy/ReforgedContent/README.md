# Dauntless Reforged Revival content server

The content server, in the `ReforgedContent/` folder, behind the gateway. It hands the host's
**game runtime** (the Mystic Paradox DLL and its loader, built by the host) to registered players,
and serves the host's optional news and art pack for the launcher. It is based on Dauntless
Revived's `UndauntedContent` (the same key check, limits, news and art pack); where Dauntless
Revived serves the 1.4.4 game files, this serves the two runtime files. **The game itself is never
served**: every player installs their own Dauntless 1.14.7.

It has no database and no secrets of its own: it checks each account key by asking the backend.

## Endpoints

All are `GET` (and `HEAD`); anything else is `404` or `405`. There is no directory listing.

| Route | Key needed | Answer |
|---|---|---|
| `/content/v1/runtime/manifest` | no | `{"version", "build": {"name", "changelist"}, "files": [{"name", "size", "sha256"}]}`; `404 {"error":"no_runtime"}` before the host publishes one, `503 {"error":"runtime_invalid"}` when what is published does not check out |
| `/content/v1/runtime/files/MysticParadox.dll`, `.../winmm.dll` | yes (`x-reforged-user-api-key`) | the file. Only these two exact names |
| `/content/v1/branding` | no | `{"backgrounds": [{"url", "credit"}], "accent"}` from the host's art pack |
| `/content/v1/branding/<file>` | no | one of those images (jpg, png or webp) |
| `/content/v1/news` | no | `{"items": [{"date", "title", "body"}]}`, newest first; plain text |

The manifest is public on purpose: the launcher shows the files and their SHA-256 to the player
and asks them to accept that exact set before anything is installed. The files themselves need a
registered account's key (`401` without one, `503 auth_unavailable` when the backend cannot be
asked). At most 4 downloads per account and 48 in total.

## The runtime folder

`CONTENT_RUNTIME_DIR` (`.workspace/runtime/` in a ReforgedHost setup) holds `MysticParadox.dll`,
`winmm.dll` and `runtime.json`:

```json
{ "version": "20261014-1a2b3c4d", "build": { "changelist": 647472 },
  "files": [ { "name": "MysticParadox.dll", "size": 4718592, "sha256": "…" },
             { "name": "winmm.dll", "size": 233472, "sha256": "…" } ] }
```

ReforgedHost's `npm run publish-runtime` and the launcher's Host page write it. The server offers
the list only after both files on disk matched their size and SHA-256, re-checks the folder every
10 seconds, and picks up a newly published runtime without a restart. Players are asked again
before a new runtime is installed.

## Art pack and news

As in Dauntless Revived: every `.jpg`, `.jpeg`, `.png` and `.webp` in `CONTENT_BRANDING_DIR` is a
launcher background (in name order, at most 25 MB each, content checked against the extension),
with an optional `branding.json` for order, credits and an accent colour:

```json
{ "accent": "#c8a24a", "backgrounds": [ { "file": "ramsgate-dusk.jpg", "credit": "Screenshot by a friend" } ] }
```

`CONTENT_NEWS_FILE` is `{"items": [{"date": "2026-10-14", "title": "Friday hunts", "body": "Ramsgate at 20:00."}]}`,
re-read when it changes. Only share art you have the right to share.

## Setup and tests

```
npm ci
npm run build
npm start            # reads .env (see .env.example)
npm test             # unit tests and the handler on 127.0.0.1:62050-62051
```

## License

AGPL-3.0-only. Based on Dauntless Revived's `UndauntedContent` by mixutin and the Dauntless
Revived contributors.
