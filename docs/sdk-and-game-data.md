# The SDK, the runtime and the game data

A Dauntless 1.14.7 server needs three things made from **your own copy of the game**. None of
them is in this repository or shared by anyone: they are made on the host's PC and stay in the
git-ignored `.workspace\` folder.

| What | Made with | Used by |
|---|---|---|
| The **SDK**: C++ headers and sources describing the game's classes | Dumper-7, injected into your running game | building the runtime and CatalogExporter |
| The **runtime**: `MysticParadox.dll` and its loader `winmm.dll` | Visual Studio (C++) and Rust | every player's game, and the server's game servers |
| The **game data**: item catalog, stores, Slayer's Path, hunt tables | CatalogExporter, injected into your running game, and Mystic Paradox's generators | the backend and the Director |

The steps below are those of Mystic Paradox's own guides
([GENERATING_SDK.md](../paradox/docs/GENERATING_SDK.md),
[GENERATING_GAME_DATA.md](../paradox/docs/GENERATING_GAME_DATA.md); their paths are Mystic Paradox's:
`ParadoxRuntime` and `tools\` are `native\` here, `ParadoxBackend` and `ParadoxDirector` are `paradox\backend`
and `paradox\director`),
with the Reforged tools doing the file handling. The launcher's **Host tools** page
(Settings > Launcher > Advanced > Host tools, then choose your Dauntless Reforged Revival folder
as the workspace) shows each step's state and has a button for most of them. Run
`npm run setup` (see [hosting.md](hosting.md)) first: it creates the workspace.

## 1. The SDK (Dumper-7)

The SDK must come from exactly the 1.14.7 build (`rel-1.14.7-Archon`, CL 647472, UE 4.26.2); the
launcher refuses to dump a game it found to be another build.

1. Build [Dumper-7](https://github.com/Encryqed/Dumper-7) (Visual Studio, Release x64) and note
   where its DLL is.
2. Host tools page, **Build** tab: **Choose Dumper-7 DLL…**, then **Start game and dump**. The launcher
   sets the runtime loader aside (so the game starts unmodified), starts Dauntless, waits for its
   window and loads Dumper-7 into it. Wait in the main menu until `C:\Dumper-7\<game>\CppSDK\`
   appears and is complete (a minute or two), then close the game; the loader is put back.
3. **Import SDK** copies the newest dump into `native\paradox-runtime\`:
   `SDK.hpp`, `SDK\` (with the `*_functions.cpp` files), `Assertions.inl`, `NameCollisions.inl`,
   `PropertyFixup.hpp`, `UnrealContainers.hpp`, `UtfN.hpp`.

By hand: inject Dumper-7 into your running 1.14.7 game with any injector, and copy the whole
`CppSDK` folder's contents into `native\paradox-runtime\`. A header-only or partial copy is not enough.

## 2. The runtime

Host tools page, **Build** tab, or by hand from `native\`:

```powershell
cd paradox-runtime; .\_build.bat               # -> native\paradox-runtime\x64\Release\MysticParadox.dll
cd ..\runtime-loader; cargo build --release    # -> native\runtime-loader\target\release\winmm.dll
cd ..\catalog-exporter; .\_build.bat           # -> native\catalog-exporter\x64\Release\CatalogExporter.dll
```

- The projects ask for the v145 toolset (Visual Studio 2026). The Host tools page passes v143 to
  MSBuild on Visual Studio 2022; by hand, add `/p:PlatformToolset=v143` or retarget the projects.
- No `deployment_config.generated.h` is needed: with the Reforged patch the runtime gets the
  backend address from the launcher (`REFORGED_BACKEND_URL`) and, on the server, from the Director.
  One build works for every server.
- One DLL serves the player's game and the dedicated servers (it detects `-server`).
- Delete any old `MystPaxInternalServer.dll` next to the game: the loader still prefers it when it
  is newer.

Then publish it:

```powershell
cd D:\Reforged\legacy\ReforgedHost
npm run publish-runtime
```

This copies both files to `.workspace\runtime\` with `runtime.json` (names, sizes, SHA-256),
which the content server offers to players, and next to the server's game executable for the game
servers (stop the server first if it is running: Windows will not replace a loaded DLL). The
launcher's **Publish** button does the first half.

## 3. The game data

The backend reads seven files from `paradox\backend\game-data\` and the Director seven from
`paradox\director\game-data\`. Without them the services do not start.

### For a first start-up test: the examples

```powershell
npm run gamedata -- --examples
```

copies Mystic Paradox's `*.example.json` files where a file is missing. The services start and you
can test joining, but **it is not playable**: no real items, hunts or stores. The launcher warns
players that the server runs on example data.

### For a playable server: export from your game

1. Build CatalogExporter (step 2).
2. Start the server (**Start server** on the Host page's Server tab, or `npm run start`), make
   yourself an invite, join it with the launcher and press PLAY: the exporter needs the running
   1.14.7 game.
3. Host tools page, **Game data** tab: **Inject CatalogExporter**. The launcher writes `export_flags.txt` next
   to the game (catalog, Slayer's Path, hunts, the store's runtime data and the progression tables)
   and loads the exporter. While it runs (up to two minutes), go where each export needs you to be:
   - the catalog and the progression tables: the main menu or Ramsgate;
   - the Slayer's Path: open the Slayer's Path screen;
   - the hunts: in Ramsgate, open the Hunt/Map screen;
   - the store: in Ramsgate, Journal > Challenges > Reward Cache.
   The exporter writes `Items_Analysis\` next to the game; check its `export_manifest_1_14_7.json`
   and `catalog_export_status.txt`. Flags are read once per injection: inject again for a missing part.
4. **Generate game data** (Game data tab) copies `Items_Analysis\` into `paradox\`,
   where the generators read it, and runs step 5 for you.
5. Generate:

   ```powershell
   npm run gamedata
   ```

   This runs, in order: the progression config (from the progression export, below), the Director's
   hunt tables, the Slayer's Path graph and definitions, the Reward Cache store, the Lady Luck store
   and the Platinum store with the storage policy (both with the store captures, below). Each step that
   cannot run yet says why. A step that fails does not stop the others: the failed ones are listed at
   the end, with the error, and the full output is in `.workspace\logs\gamedata.log`.

### The progression config

`progression_config.json` holds the progression tracks: the player and weapon levels
(`ExperienceTrack_*`), the mastery and prestige tracks, and the Hunt Pass. The 1.14.7 client needs
them to load a player: with the example file, Ramsgate never finishes loading your character and
drops you after a minute ("Loading PlayerData Failed" in its log, "the game server could not be
reached" in the game).

Mystic Paradox builds it from a captured server response. Without one, `npm run gamedata` builds the
source from the progression tables of your own game, which CatalogExporter exports into
`Items_Analysis\progression_1_14_7\`: every track with its per-rank XP costs and rewards, plus the
active Hunt Pass (`season43`). The game's Hunt Pass rows hold no tiers, so the pass gets 50 ranks of
100 XP and no rewards. Each weapon's prestige track (`PrestigeTrack_Weapon_*`), which the export has no
rank table for, gets the ranks of that weapon's experience track and no rewards. A captured response in
`paradox\backend\game-data\raw\progression_config.source.json` (a progression-config response with
`payload.paths`) is always used instead.

To rebuild only this file after a new progression export, without touching the other game data:

```powershell
npm run gamedata -- --progression
```

### The store captures

The Lady Luck offers and the platinum store's prices, Hunt Pass rank skips and passes are not in the
game client, so no export can produce them. Mystic Paradox builds them from captures of the live store
service, published by EisigesEis in
[DauntlessEndpointDocumentation](https://github.com/EisigesEis/DauntlessEndpointDocumentation).
`npm run gamedata` downloads the five files it needs, at the commit and with the SHA-256 hashes in
[`paradox/backend/store-captures.json`](../paradox/backend/store-captures.json), into
`paradox\DauntlessEndpointDocumentation\` (it needs git and the internet once). Then:

- Lady Luck's Store is the live rotation, kept to the items your catalogue has (38 of its 43 offers
  in 1.14.7: the five gold cell cores are not in the game), with their live prices in Steel and
  Gilded Marks.
- The platinum store gets the captured prices, the 100 Hunt Pass rank skips and the passes, besides
  the reconstructed offers.

If the download fails, the step says so and the other steps still run: Lady Luck then keeps the
example (no offers) and the platinum store has only reconstructed offers. A captured Lady Luck response
of your own, in `paradox\backend\game-data\raw\ladyluck_store.source.json` (an array of store
entries), is always used instead of the live rotation. `npm run gamedata -- --example-sources` uses
the examples for the files whose source is missing, so the services can start.

### Check

```powershell
npm run doctor
```

shows the state of both folders (real, example or missing). Restart the server after generating:
the services read the data at start-up.

### Save it once: the game data pack

Once the server plays well, save everything `npm run gamedata` made into one file:

```powershell
npm run gamedata -- --export
```

It writes `.workspace\gamedata-packs\reforged-gamedata-1.14.7-<date>.json.gz` (or pass a path:
`-- --export D:\Backups\gamedata.json.gz`): the backend's and the Director's game data and the
sources in `game-data\raw`, without the examples. On your next setup, a new PC or a clean
workspace, put it back instead of dumping the SDK, exporting with CatalogExporter and generating:

```powershell
npm run gamedata -- --import D:\Backups\gamedata.json.gz
```

or **Import game data pack** on the launcher's Host page (Game data tab, where **Save pack** makes one). The pack is checked first (the
Dauntless build it is for and each file's hash); a file it replaces is kept beside it as
`<name>.<time>.bak`. You still build the runtime from an SDK, since the runtime is compiled from it.

### The project's server preset

Like Dauntless Revived's bundled `vendor/*.json`, the project can carry the game data its servers start
from, in [`preset/`](../preset) at the repository root. Make it from your own game:

```powershell
npm run preset -- --create --generate
```

`--generate` runs the generators on your CatalogExporter export first (leave it out to take the game data
already generated). The files go to `preset/` with `preset.json`: the preset's name (`--name`, default your
server's), the Dauntless build, the SHA-256 of your game executable, the tools, and each file's SHA-256.
`npm run preset` lists it.

Lady Luck's Store and the platinum store are left out: they are built from the store captures (above),
which the project does not copy, so each server builds them with `npm run gamedata`.

From then on `npm run setup` copies the preset into the game data wherever a file is missing or still the
example, on this PC and in every other checkout that has `preset/` (commit it to ship it with the project).
A host's own generated game data takes precedence and is never replaced, unless
`npm run preset -- --apply --force` (the replaced file is kept as `.bak`). A preset file that no longer
matches its hash is refused. See [`preset/README.md`](../preset/README.md) for what committing it to a
public repository means.

## Keep it to yourself

The SDK, the exports and the game data are derived from Phoenix Labs' game. Do not commit, upload
or share them; this repository's `.gitignore` and Mystic Paradox's keep them out of Git. Share only
your server, through invites. The same goes for a game data pack: keep it with your own backups (an
external drive, a private folder), not in a public repository or release. A server preset you commit
is published with the repository; that is a choice for the project's owner (see the preset's README).
