# Server preset

The game data every server set up from this project starts from, like the `vendor/*.json` files of
[Dauntless Revived](https://github.com/mixutin/dauntless-revived)'s metagame and deploy server.

`npm run setup` (in `legacy/ReforgedHost/`) copies these files into the server's game data wherever a file is
missing or still the shipped example. A host's own generated game data always takes precedence: the
preset never replaces it unless the host asks (`npm run preset -- --apply --force`, which keeps the old
file as `.bak`).

| File | What |
|---|---|
| `preset.json` | the manifest: the preset's name, the Dauntless build, the SHA-256 of the game executable the data was read from, the tools, and the SHA-256 of every file (a file that no longer matches is refused) |
| `backend/*.json` | the backend's game data (progression config, Slayer's Path, Reward Cache, storage policy) |
| `backend/raw/*.json` | the sources the backend's generators read (the progression config's) |
| `director/*.json` | the Director's hunt and matchmaker tables |

The preset here is "Freeza", read from the Dauntless 1.14.7 client whose executable's SHA-256 is in
`preset.json`.

Lady Luck's Store and the platinum store are never in a preset: they are built from captures of the
live store service that no license lets this project copy (see
[`paradox/backend/store-captures.json`](../paradox/backend/store-captures.json)), so `npm run gamedata` builds them
on each server, which needs the CatalogExporter export there. `npm run preset -- --create` leaves them
out, and a preset that lists them is refused.

Make or refresh the preset from your own game:

1. Play on your own server, inject CatalogExporter in Ramsgate and **Copy export** (launcher, Host page).
2. `cd legacy/ReforgedHost` and `npm run preset -- --create --generate` (the generators run on the export, then
   the result is written here). `npm run preset` shows what is in it.

Do not edit the files by hand: their hashes are in `preset.json`.

## What it holds, and publishing it

The files hold identifiers and tuning values read from an installed Dauntless 1.14.7 client for
interoperability (the item catalogue, stores, progression and hunt tables) and no game assets. They are
still data from Phoenix Labs' game: Mystic Paradox keeps its own copies out of its repository for that
reason, while Dauntless Revived ships comparable files with a provenance note in each. Committing a
preset to a public repository publishes it; keep it in a private repository, or as a game data pack
(`npm run gamedata -- --export`) in your own backups, if you would rather not.
