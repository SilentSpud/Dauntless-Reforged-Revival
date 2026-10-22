# The Reforged patches to Mystic Paradox

Dauntless Reforged Revival forks [Mystic Paradox](https://github.com/pranav158/Mystic-Paradox) by Pranav
Karande. The fork, [SilentSpud/Mystic-Paradox](https://github.com/SilentSpud/Mystic-Paradox), has a `reforged`
branch that starts at the commit Reforged used to pin and adds the 33 patches below as one commit each; that
branch is merged into this repository with its history at [`paradox/`](../paradox). Until then the host kit
downloaded the pinned commit and applied these patches as files under `upstream/`.

| | |
| --- | --- |
| Upstream repository | https://github.com/pranav158/Mystic-Paradox |
| Base commit | `00d3be8aa812e334aaa26e5149d2607acb744360` (docs: installing Dauntless 1.14.7 from Epic with Legendary) |
| Fork | https://github.com/SilentSpud/Mystic-Paradox, branch `reforged` at `24e60c8dd89c871f273b8f6b8753f2ee334b017f` |
| Game build | Dauntless `rel-1.14.7-Archon`, changelist 647472, Unreal Engine 4.26.2 |
| License | AGPL-3.0-only, with the attribution terms in [`paradox/ADDITIONAL_TERMS.md`](../paradox/ADDITIONAL_TERMS.md) |

Each patch below is the commit of the same name in `git log` (for example
`git log --grep=^0023- 24e60c8`). The paths in them are Paradox's: `ParadoxBackend` and `ParadoxDirector` are now
[`paradox/backend`](../paradox/backend) and [`paradox/director`](../paradox/director), `ParadoxRuntime` and
`tools/RuntimeLoader` are [`native/paradox-runtime`](../native/paradox-runtime) and
[`native/runtime-loader`](../native/runtime-loader). Later changes are ordinary commits to those folders; how
runtime patches from Mystic Paradox's own line are taken is in [paradox-runtime-patches.md](paradox-runtime-patches.md).

## Patches

### 0001: the runtime reads its backend address from the environment

`ParadoxRuntime/dllmain.cpp`. Upstream compiles one backend address into the DLL
(`kMpApiBase`, from a generated deployment header) and rewrites every official Dauntless URL to
it. With the patch the runtime reads `REFORGED_BACKEND_URL` (for example
`http://127.0.0.1:61100`) once at start-up:

- `http` or `https`, a host of letters, digits, dots and dashes, an optional port, nothing else;
  anything else is ignored and logged.
- Without it the compiled address is used, as upstream does (`MP_PUBLIC_HOST`, default
  `reforged.invalid` when no deployment header exists, so a build without one fails closed).
- The XMPP chat address follows it (`ws://` or `wss://`, the same host and port, `bUseSSL`
  matching), so chat also goes through the launcher's relay.

One runtime build therefore serves every server: the launcher points it at its local relay, and
the Director's game servers inherit `REFORGED_BACKEND_URL=http://127.0.0.1:61000` from the host kit.

### 0002: the backend speaks the launcher API

`ParadoxBackend`. New routes under `/reforged/api` modelled on Dauntless Revived's
`/undaunted/api` (see `src/routes/reforgedApi.ts` for the full contract):

- `POST Register` (username + invite code, in one MongoDB transaction) returns an account key
  `RRK_<48 hex>`; only its HMAC is stored, like every other API key.
- `GET GetUserInfo` (`x-reforged-user-api-key`), `GET ServerStatus` (players and instances for a
  registered player, a limited view otherwise; game build and game-data state for everyone),
  `GET RegistrationStatus`, `GET /dauntless-status`.
- Admin routes, accepted only from this machine with `x-reforged-admin-key` and never through the
  gateway: `CreateInvite`, `InviteCodes`, `InviteCode/:code`, `SetAccountStatus`.
- `AUTH_MODE=APIKEY`: the game's `client_credentials` grant without an exchange code gets the
  service token, and an ineligible (banned, disabled) account is refused with 403.
- `src/server.ts`: `BIND_HOST`, `REFORGED_HTTP_ONLY=true` (plain HTTP behind the gateway, which
  terminates TLS), `QOS_PORT=0` turns off the backend's own QoS listener (the launcher answers QoS).
- `src/reforgedApiRules.ts` holds the pure rules with their tests (`reforgedApiRules.test.ts`).

### 0003: the Director binds to one address and lists its instances

`ParadoxDirector`. `BIND_HOST` (default `127.0.0.1`, so only the backend on the same machine can
ask it for game servers) and `GET /api/instances` (the running hubs and hunts, for ServerStatus).

### 0004: a hub stays up when its phantom player's data cannot load

`ParadoxRuntime/dllmain.cpp`, the player-data `LoadFailed` hook. A 1.14.7 hub boots in listen-server
mode and spawns a local "phantom" player whose PlayerState has no net id, so its progression component
never loads ("the player state does not have a valid ID"). After the 60-second timeout and one retry the
game's terminal handler reloads the hub's map ("LoadMap: failed to Listen"), which disconnects every real
player about two minutes after each hub start, although their own loads all completed. On the server, a
load failure whose owner has no net id is now logged (`[PhantomLoadFailure]`) and dropped; real players
always carry one. `KEEP_PHANTOM_LOAD_FAILURE.flag` beside the game executable restores the original
handling.

### 0005: XP and objectives reported by the game server are stored

`ParadoxBackend`, `src/routes/progression.ts` and `src/controllers/progression.ts`. The 1.14.7 game
server sends its XP grants (`POST /progression/<user>/<track>/<amount>`: Slayer level, weapon, Hunt
Pass after a kill, a hunt or a collectible) without a grant id, so every grant was refused with 400
("missing or invalid authoritative grant id") and no XP was ever stored. For an authenticated game
server that sends no id, the backend now mints one, so each grant is applied once through the same
transactional ledger; an id that is present but invalid is still refused, and a player's own bearer
still cannot grant XP. Objective reports (`POST /progression/<user>`: daily collectibles, mastery and
achievement objectives) from a game server are now stored with the repository's max-guard
(`setObjectiveIfGreater`), so a stale report never lowers one; a player's own reports stay
diagnostics-only. The backend log shows `mode=gameserver authoritativeMutation=<n>` for them.

### 0006: a client no longer crashes travelling out of a hunt

`ParadoxRuntime/dllmain.cpp`, client only. Leaving a hunt (back to Ramsgate, or into a second hunt) crashed
the player's game in `UEngine::LoadMap` (`0x04271370`, at `0x04272D28`): "EXCEPTION_ACCESS_VIOLATION reading
address 0x20" in `0x0285E6F0`, the new world's `PersistentLevel` read through `GetOutermost` while it is null.
The fix is ported from Mystic Paradox's own (10 October 2026, upstream commit `c7869b6`): a map's previous
`UWorld` survives the trip because one pending-kill input component keeps the GUObjectArray `Async` item flag,
a garbage-collection keep flag, and with it its whole world. GC destroys the level but not the world, and when
the client travels to that map again `LoadMap` finds the world in memory and reuses it. A hook on `LoadMap`'s
entry now clears that stale `Async` bit on the pending-kill objects inside leftover worlds (pending-kill
`UWorld`s other than `GWorld`; never RootSet or AsyncLoading objects), before `LoadMap`'s own full purge, so the
purge frees them and the map loads fresh. The game's log says `[ZombieWorld] LoadMap: ...` when it does.
`KEEP_STUCK_ASYNC_FLAGS.flag` beside the game executable turns it into a report only.

### 0007: XP and reward multipliers for private test servers

`ParadoxBackend`, `src/testMultipliers.ts` (with `testMultipliers.test.ts`), `src/routes/progression.ts`,
`src/routes/inventory.ts` and a start-up log line in `src/server.ts`. A private server can multiply what its game
servers grant, so a tester reaches late-game content in a few hunts. `npm run multipliers -- --xp 5 --rewards 5`
(or setup's `--xp-multiplier` and `--reward-multiplier`) writes `REFORGED_XP_MULTIPLIER` and
`REFORGED_REWARD_MULTIPLIER` (1 to 100) into `backend.env`; the host kit refuses them in public mode, and the
backend uses them only when `REFORGED_SERVER_MODE=private` and `REFORGED_SERVER_HOST` is a Tailscale address
(100.64.0.0/10 or `*.ts.net`). Its log says `[TestMultiplier] ON: XP x5, rewards x5` or `[TestMultiplier] off (why)`.

- XP: every game-server grant on `POST /progression/<user>/<track>/<amount>` (Slayer level, weapons, Hunt Pass),
  capped at the route's maximum of 100000 per grant.
- Rewards: in a dedicated game server's `POST /inventory` whose `source` is `PlayerLoot`, `MISC` (part breaks),
  `patrol_chest`, `quest` or `BUNDLE_GATHER_*`, the stacked items added that are `CURRENCY_NOTES`,
  `CURRENCY_PJM_*`, `BREAK_*` or `GATHER_*`. Unlock tokens, cosmetics, equipment, prestige-gated rewards,
  challenge-funded season coins and anything a craft or a purchase removes keep their quantities. The game's own
  reward screen still shows the original numbers; the inventory and the levels hold the multiplied ones.

### 0008: players no longer wait a minute on the loading screen of a hunt

`ParadoxRuntime/dllmain.cpp`, server only. Joining a hunt or the Training Grounds kept every player on the
loading screen for about 60 seconds. With `CALL_HUNTPASS_QUERY.flag` the runtime issues the progression
configuration query (`GET /progression/config`) at the second "is the configuration loaded?" check. A hub
makes two such checks at boot, so its configuration is cached before anyone joins. A hunt or the Training
Grounds makes only one at boot, so the second was the first real player's own load: their progression
component started waiting, the configuration arrived a few milliseconds later, and nothing checked again
until the game's 60-second player-data timeout fired and its retry completed the load (the runtime log shows
`[HuntPassQuery] calling` at the join and `[LoadFailedDiag] fire #1` exactly 60 s later). On the server the
query is now issued at the first check, at boot, as a hub already does. The client keeps the original
timing.

### 0009: mastery XP is stored, and stored objectives are served as completed

`ParadoxBackend`, `src/controllers/progression.ts` and the progression repository (with a test in
`progressionHardening.gamedata.test.ts`). Mastery never gained XP:

- The 1.14.7 game server sends no grant for a `MasteryTrack_*`. When a mastery objective is completed it
  reports the points in its objective report (`POST /progression/<user>`) as `progress_tracks` deltas:
  +1 on the weapon's or the behemoth track and +1 on `MasteryTrack_PlayerLevel` per objective (captured from
  real hunts). Those were only logged. From a game server they are now added to the tracks, and a track
  gains at most the number of objectives this report completes for the first time (`completed_count` above
  the stored one), so a repeated report adds nothing. The log says `[MasteryXp] user=... MasteryTrack_...+1=<total>`.
- `GET /progression/objectives/<user>` answered `{objective_id, value, completed_count}`. The live service's
  row (Progression/GetObjectiveProgression.md in the endpoint captures) holds the count in `progress`, so the
  game server read every stored objective as not started and completed the same mastery objectives again in
  each hunt. The rows now carry `progress` (and `phx_account_id`, `created_date`, `last_modified_date`) as
  the capture does; `value` stays.

Mastery XP earned before this patch was never stored and is not restored; the objectives completed since
patch 0005 are stored and count as done.

### 0010: claiming a completed bounty works

`ParadoxBackend`, `src/bountyState.ts` (with `bountyState.test.ts`), `src/controllers/bounty.ts` and
`src/routes/system.ts`. Claiming a completed bounty on the board (U) makes the game server ask the backend to
delete it (`POST /bounty/delete/<user>`), and only a 200 lets it grant the reward and take the bounty off the
board. Upstream has no such route, so every claim got a 404, the game server logged
`UBountyComponent::OnClaimBountyComplete() - invalid http response` and the bounty stayed on the board, complete
and unclaimable. The new route (POST, and DELETE) removes the named bounties from the stored state, reading
`bounty_ids` (the field name in the 1.14.7 executable; no capture of the live body exists), `bounties` and
`delete_claimed_bounties`, and answers with the state like a save does. Season challenges are never deleted, so a
claimed challenge still cannot be re-seeded and pay its Cache Coins twice. Each call logs a
`[DraftMon] BOUNTY-DELETE` line with what was deleted and the raw body; a body that names nothing gets a 400.
The number skips 0009, which another pending change uses.

### 0011: a hunt client no longer crashes when its loading takes more than 3 seconds

`ParadoxRuntime/dllmain.cpp`, the client side of the `HasFinishedLoading` compatibility override. On a client
the override forces the local player controller's readiness to true as soon as the possession chain has been
stable since `ClientRestart` for 3 seconds. A hunt client is normally ready on its own about 1.5 seconds after
`ClientRestart`, so the override never fired; when the client was slower (a busy machine), it fired at 3 seconds,
the whole map ran `ReceiveGameplayStart` at once, including the adventure HUD, and the game crashed 20 ms later
with `EXCEPTION_ACCESS_VIOLATION reading address 0x248` at `Dauntless-Win64-Shipping.exe+0x1F6D6BE`
(`[ReadinessCompat] FORCE_TRUE scope=local-controller` right before `[ExceptionTrace]` in the client's
`mysticparadox_dll_port0.log`). The override now waits 20 seconds after `ClientRestart`, so a slow load finishes
on its own and only a load that is really stuck is forced.

### 0012: a hunt no longer spawns the player before their data has loaded

`ParadoxRuntime/dllmain.cpp`, both sides. 0011 did not stop the crash: in the next report the client's
readiness stayed false for the whole 20 seconds and the crash followed the forced `true` again. The load was not
slow, it was missing something:

- The client: the player controller's own readiness (`+0x1CB1360`) waits for the character's weapon
  (`AArchonCharacter` `+0x5F0`) and armour slots. The HUD it releases reads that weapon without a null check
  (`UHUDLegendaryAbilityWidget::NativeConstruct` `+0x1F7AA00` → `HandleWeaponEquipped` `+0x1F6D4F0`, which
  reads `weapon+0x248` at `+0x1F6D6BE`), so forcing readiness without a weapon always crashed. The override now
  also requires the weapon, and otherwise leaves the loading screen up.
- The server: the runtime answers `GameModeBase.PlayerCanRestart` with `true` for every controller, so any
  restart request spawns the pawn at once. In the three crashed hunts (16 Oct, all Snow Jen) the player was
  restarted about 1.5 seconds after joining, right after another progression component finished loading
  (`[ProgDiag] CompleteLoadInternal RUNS`), long before the player's own data had loaded, so the character
  reached the client without its weapon. In every other hunt the restart comes from `OnPlayerDataLoadComplete`
  (which calls `ServerRestartPlayer`), including the ones whose load needs the 60-second retry. In a hunt the
  runtime now answers `false` for a connected player until `OnPlayerDataLoadComplete` has run for that
  controller (`[RestartDataGate] holding restart ...` in the server log), and after 90 seconds it falls back to
  the old answer. The city and the Training Grounds keep the old behaviour; `DISABLE_RESTART_DATA_GATE.flag`
  turns the gate off. The same early restart happened before 0008, 20 to 30 seconds after joining; with the
  progression configuration available at boot (0008) it comes about 1.5 seconds after joining. 0008 stays.

### 0013: the log shows where a hunt's missing weapon got stuck

`ParadoxRuntime/dllmain.cpp`, client side, logging only. With 0012 the hunt no longer crashes, but on 16 Oct
every hunt with the newly crafted hammer (`ihammer_rock_00`) equipped stayed on the loading screen until the
server's 120-second "Loading timeout", while hunts with the war pike or the repeaters loaded. The server logs of
both kinds of hunt match line for line, so the weapon is lost on the client: `AArchonCharacter::Weapon`
(`+0x5F0`) is not replicated, and the client never sets it. While 0012's check holds the loading screen for a
missing weapon, the runtime now writes `[WeaponGap]` lines 3, 10, 30, 60 and 100 seconds after `ClientRestart`:
the character's equipment pointers, every weapon the client has (owner, network role, item id, the four
`AArchonEquipment` equip flags, and the weapon's replicated level, slot and parts, power table and meshes),
the other equipment the character owns, and the local loadout's active slot. It changes nothing in the game.

### 0014: the server log shows what happened to a hunt's weapon

`ParadoxRuntime/dllmain.cpp`, server and client, logging only. 0013's `[WeaponGap]` lines from a hammer hunt on
16 Oct show the hammer reaching the client already destroyed (`dying=1`, no owner) 4 seconds after
`ClientRestart`, with no other weapon arriving, while the armour and lantern arrived and equipped. A client
cannot destroy a replicated actor it does not own, so the server either destroyed the hammer or stopped
replicating it to that player. Neither the game's server log nor the runtime's recorded weapons. In hunts the
server now writes `[ServerWeaponGap]` lines 0.5, 1.5, 3, 5, 10 and 30 seconds after each connected player's
character appears: the character's equipment pointers and location, then every weapon and every piece of that
character's equipment, with its owner, equip flags, location and distance from the character, what it is
attached to, its cull distance, and its replication-graph state for that player's connection (the actor
channel, the cull distance the graph uses, and the frame the channel is due to close). The client's
`[WeaponGap]` item lines also gain the cull distance and attachment. `DISABLE_SERVER_WEAPON_GAP_LOG.flag` next
to the game exe turns the server lines off. It changes nothing in the game.

### 0015: a hammer reaches the player in a hunt

`ParadoxRuntime/dllmain.cpp`, server side. 0014's `[ServerWeaponGap]` lines from a hunt on a new account
(17 Oct, `WP_IH_BEGINNER`) show the hammer equipped and attached to the character, 150 units from it, while
that player's connection has no actor channel for it and has never replicated it, for the whole 30 seconds;
the armour and lantern, with the same cull distance, replicate every frame. The replication graph files a
dormant actor in the spatial grid as a static actor: `AddNetworkActor` sets `bWantsToBeDormant` from
`NetDormancy > DORM_Awake`, and the grid's `AddActor_Dormancy` then picks the actor's cells once, from where it
is at that moment, and never updates them. A weapon is registered before it is attached to its character, so a
dormant weapon stays filed at its spawn point and is only sent to a player whose view is within its cull
distance of that point; on a hunt map the player starts far from it. The runtime now files a weapon on the
spatial route as awake (`NetDormancy` `DORM_Awake`, `bWantsToBeDormant` 0, what the engine writes for an actor
that is awake from the start), so the grid moves it with the character. As a second net, each player's current
weapon that is still dormant and has never been sent to that player is woken with the engine's
`AActor::SetNetDormancy(DORM_Awake)`, which makes the grid move it itself. The server log gains a `[WeaponRoute]`
line per weapon registered (dormancy, flags, location, the route taken) and a `[WeaponDormancyWake]` line per
weapon woken, and the `[ServerWeaponGap]` item lines gain the dormancy and route flags.
`DISABLE_WEAPON_DORMANCY_FIX.flag` next to the game exe turns both changes off.

### 0016: an always-relevant weapon is no longer culled at the world origin

`ParadoxRuntime/dllmain.cpp`, server side. 0015 did not fix the hammer hunts, and its `[WeaponRoute]` line
(17 Oct, 03:43) shows why: the hammer is not dormant. Its class sets `bAlwaysRelevant` (`flags58=0x88`; the
armour and lantern are `0x80`), so it goes to the graph's global always-relevant node, registered at
`(0,0,0)`, where it is spawned. That node still applies the class cull distance (15000 units) and measures it
from the position the graph stores for the actor, which only the spatial grid updates; for a global actor it
stays at the origin. A hunt starts about 24000 units from the origin, so the hammer is culled for the player
and never sent; the city's start is close enough. The PlayerRole and PlayerState routes already clear the
cull distance for this reason. A weapon on the global route now gets the same correction when it is
registered, and a player's current weapon that is on that route and was never sent to them has its cull
distance cleared on the tick, in the graph and in each connection's copy (`[WeaponCullFix]` line). The
`[WeaponRoute]` line gains the cull change and the `[ServerWeaponGap]` item lines gain the graph's stored
position. `DISABLE_WEAPON_CULL_FIX.flag` next to the game exe turns this off. 0015 stays: it is correct for a
dormant weapon on the spatial route.

### 0017: new and existing characters can quick chat and quick emoji

`ParadoxBackend/src/controllers/starterManifest.ts`, `loadout.ts`. Both radial wheels read nine slots from
the persistent loadout (`quick_chats`, `emojis`), and the starter loadout saved all of them as `""`, so the
wheels opened empty and choosing a slot did nothing. Emojis are inventory items as well, and no account owned
one: the game's own `INITIAL_INVENTORY_BUNDLE` (catalog export) gives a new character `EJ_SYSTEM_TYPING` and
eight `EJ_PLAYER_*` emojis, which the starter manifest never granted. The game server shows the gap whenever a
loadout is checked before the inventory arrives (`ValidateLoadout - Unable to set valid Emoji -
EJ_PLAYER_BOSUN_JOY`, the first of the game's four default emojis).

- The nine emojis are added to the starter stacked items, so new characters own them and existing ones receive
  them on their next inventory read (the existing starter reconcile; `BOOTSTRAP_VERSION` becomes
  `starter-1.12-v5`).
- New characters start with both wheels filled: the eight player emojis (the game's four defaults first) and
  nine quick chats from `/Game/Gameplay/Backend/Catalogs/QuickChat/quickchat_text_table` (`qc_hello`,
  `qc_thankyou`, `qc_goodjob`, `qc_help`, `qc_follow_me`, `qc_onmyway`, `qc_yes`, `qc_no`, `qc_sorry`).
- When the loadout is read, a wheel whose slots are all empty gets those defaults; a wheel with any entry is
  the player's choice and is left alone. The game saves the persistent loadout back as usual, so the defaults
  are then stored and can be changed in the Identity menu.

`src/chatWheels.test.ts` covers the grant and the wheel defaults.

### 0018: the login queue's maintenance status

`ParadoxBackend/src/routes/loginQueue.ts`, mounted in `app.ts`. The 1.14.7 client asks
`GET https://login-queue-prod.steelyard.ca/maintenance/status` (no auth). Upstream has no such route, so the
backend answered 404 (`Unstubbed route GET /maintenance/status`). Found on the first run of the real 1.14.7
client against a Reforged server, while its login stopped at `CheckCrossPlayAndCrossProgression` ("An error
occurred while communicating with the game servers"). That stop turned out to have another cause:
Dauntless Revived's launcher leaves `[OnlineSubsystemPhoenix]` endpoints at `127.0.0.1:61000` in the
player's Game.ini, so the game's own login checks never left the PC (the Reforged launcher now removes them
before a 1.14.7 launch). The 404 was still a real unanswered request.

The route answers the captured contract, `{ "show-status": false }`
([EisigesEis/DauntlessEndpointDocumentation](https://github.com/EisigesEis/DauntlessEndpointDocumentation),
`Login/LoginQueue/MaintenanceStatus.md`). `src/maintenanceStatus.test.ts` covers it.

### 0019: the game servers stop wasting CPU on every frame and every reflected call

`ParadoxRuntime/dllmain.cpp`, server side. Ported from Mystic Paradox's own CPU fixes (marked
`[2026-10-10 1.14.7 CPU]` and `[2026-10-11 1.14.7 O3]`, upstream commit `c7869b6`, two days after the pin), with
the same flags to undo each one. The trigger was a report from a Debian 13 VPS (4 vCPU, 16 GB): about 40% CPU
with nobody online and over 80% with two players. The costs below are in every game server, Ramsgate and the
Training Grounds included, and grow with the players.

- **Every frame walked every object.** While a server listens, its tick hook kept `ExpectedPlayerCount` in
  sync by walking all of `GObjects` (about 208,000 objects in Ramsgate) with two `IsA` calls each, on every
  frame, with or without players. It now reads the world's own `AuthorityGameMode` (`GWorld+0x118`) and
  `GameState` (`GWorld+0x120`): same fields, same values, same log lines. A one-shot census (10 s after listen,
  and at the first `K2_PostLogin`) logs whether the two lookups agree. `LEGACY_EXPECTED_PLAYER_WALK.flag`
  beside the game executable brings the walk back.
- **Every reflected call built a name.** The server's `ProcessEvent` hook built `Function->GetFullName()` (an
  Outer-chain walk with string allocations) and then ran about 90 substring searches on it, for every call:
  tens of thousands a second on a hub with players. The name now comes from the per-UFunction cache the client
  hook already had (validated by index, name and outer), and each search is evaluated once per UFunction into
  a bit mask; a call site still runs its original test, only when its bit is set. `PE_FULLNAME_UNCACHED.flag`
  and `PE_CATEGORY_CACHE_OFF.flag` undo them.
- **Flag files were looked up on the disk.** `MpExeRelativeFlagPresent` resolved the executable's path and asked
  the file system on every call, several times a frame (and once per replicated actor added to the graph). Under
  Wine a file that does not exist costs a scan of the whole folder. Each answer is now kept for 4 s, so a flag
  created or deleted next to the executable still takes effect within 4 s. `UNCACHED_FLAG_FILES.flag` undoes it.
- The F7 object dump (a full walk, then a busy wait while the key is held) runs on clients only.

The log says `[FlagCache]`, `[PENameCache]`, `[PECategoryCache]` and `[ExpectedPlayerCount] lookup = ...`.

### 0020: a frame cap for the game servers

`ParadoxRuntime/dllmain.cpp`, server side, adapted from Dauntless Revived's `DR_SERVER_MAX_FPS` (its server DLL
caps the game thread at 30 frames per second). Nothing in this build reliably caps a game server's game thread,
so each one ran its frames as fast as the CPU allowed, players or not. `MpPaceServerFrame`, at the top of the tick
hook, waits for the rest of the frame's time (real elapsed time, not a fixed timestep: `DeltaTime` stays the real
frame time, so game time does not change):

- `REFORGED_SERVER_MAX_FPS`: frames per second (default 30, the configured `NetServerMaxTickRate`); 10-240, or
  0 for no cap.
- `REFORGED_SERVER_IDLE_FPS`: frames per second once the server has listened with no client connection for 15 s
  straight (default 0: the same as above). The first connection switches back to the full rate on the next
  frame; boot, the map load and the first 15 s after listening always run at the full rate.

The Director passes its environment to every game server, so the host kit writes both into `director.env`
(`npm run setup -- --server-fps 30 --idle-fps 10`). The log says `[FramePace]` at start and at each switch.

### 0021: the Training Grounds on demand, and quieter game-server logs

`ParadoxDirector/src/controllers/gameservers.ts`.

- `TRAINING_ON_DEMAND=1`: the Training Grounds are not started at boot (Dauntless Revived does the same with its
  Training Dojo). The first matchmaking request for them starts the hub and waits for it, as a hunt request waits
  for its hunt server; from then on it is supervised like Ramsgate. Saves a whole game process (about 1-1.5 GB of
  RAM and its CPU) while nobody trains. Without the setting the Director starts both hubs at boot, as before.
- `GAMESERVER_LOG_CMDS` replaces the game servers' `-LogCmds` value (default unchanged:
  `LogPhoenixCharacter VeryVerbose, LogOnline Verbose, LogLoad Off`). Letters, digits, underscores, spaces and
  commas only; anything else keeps the default (logged).

### 0022: a new hunt only when the memory for it is free

`ParadoxDirector/src/controllers/memoryAdmission.ts` (new, with `memoryAdmission.test.ts`) and
`gameservers.ts`, after Dauntless Revived's `MemoryAdmission`. `MAX_CONCURRENT_HUNT_SERVERS` alone does not know
how much memory is left, and on a small server the last hunts it allows can run the machine out of memory, when
the kernel kills whatever it picks (Ramsgate, MongoDB). With `GAMESERVER_MEMORY_GUARD=1` a hunt starts only when
the memory available now (Linux's `MemAvailable`; `os.freemem()` elsewhere), minus what hunts started in the last
`GAMESERVER_MEMORY_RESERVATION_SECONDS` (default 120) have yet to allocate, covers `GAMESERVER_HUNT_MEMORY_MB`
(default 1536) and leaves `GAMESERVER_MIN_FREE_MB` (default 1024) free. Otherwise the request is refused like a
full hunt cap, before a port or a process is taken (`[MemoryGuard] refusing a new hunt` in the Director's log);
running hunts and the hubs are never touched, and memory it cannot read never blocks a hunt. A failed start gives
its reservation back at once. Without `GAMESERVER_MEMORY_GUARD=1` nothing changes; the host kit turns it on.

The host kit also lets a server run up to 10 hunts (`--max-hunts 1-10`): up to 6 the game ports stay UDP
8770-8777, and each hunt above 6 takes one more port below 8770 (`PORT_RANGE_BEGIN` 8768 for 8, 8766 for 10).
Ramsgate (8777) and the Training Grounds (8776) do not move.

### 0023: supplies refilled at every game login

`ParadoxBackend`, `src/sessionSupplies.ts` and `src/routes/login.ts`. The host can name, per account,
stacked items (tonics, pylons, grenades...) that go back to a fixed quantity every time that account
starts a game session (`POST /login`). The list is the JSON file named by `REFORGED_SESSION_SUPPLIES_FILE`
(the host kit writes `.workspace/config/session-supplies.json` there), read again at every login, so
editing it needs no restart:

```json
{ "accounts": { "<user id>": { "QI_GRENADE_ICE": 1000, "QI_STAMINA_POTION_00": 1000 } } }
```

Quantities are set, not added (0 removes the item); every other item is left alone. A missing file or
an account it does not name changes nothing. Currencies (`CURRENCY_*`, kept in the wallet) and items the
storage policy keeps as instanced are refused, and a malformed file is logged (`[SessionSupplies]`)
without blocking the login.

### 0024: supplies without a stack cap

`ParadoxRuntime/dllmain.cpp`. A few tonics carry `MaxQuantity` 30 in the game's own catalog
(Granny's Heartbreaker `QI_11A_CRIT_POTION`, Granny's Antidote `QI_11A_POISON_CLEANSE` and three test
tonics), so a stack above 30 (from patch 0023, for example) is dropped from the player's inventory. The
runtime now sets `MaxQuantity` to 0 (no cap) on every item of the live `UArchonCatalog` tagged `supplies`
whose cap is above 1, in the player's game and in the game servers, from the game thread (the ProcessEvent
hooks, at most every 5 seconds); it logs `[SupplyCap]` once per change. Flares and other one-per-account
items (cap 1) are not touched. `KEEP_SUPPLY_STACK_CAPS.flag` beside the game executable keeps the game's
caps. It is a runtime patch: build the runtime again and publish it.

### 0025: Ramsgate chat, quick chat and emojis reach the City room

`ParadoxBackend/src/realtime/RoomService.ts`, new `realtime/cityRooms.ts`, `controllers/matchmaking.ts`,
`routes/matchmaking.ts`. In Ramsgate (and the dojo) the client sends typed lines, quick chats and emojis to
its city's XMPP room, `City-<gameSessionId>` (`UChatClient::SendRoomMessage` and `SendPlayerMessage`), with
the `gameSessionId` from the `/candidate/status` answer that sent it there. Upstream lets an account into a
City room only through the insecure development switch `REALTIME_XMPP_DEV_CITY_MUC` (any account, any City
room), which a production backend must keep off, so the host kit sets it to `false` and every City join was
refused: `[XMPP] MUC join denied (city muc disabled) room=City-...` (477 times in the host's backend log).
Hunt rooms have their own switch, on by default, which is why chat worked in hunts.

The backend now remembers, per account, the hub game session it last sent the account to (when
`/candidate/status` answers with a hub's `gameSessionId`), and lets that account into exactly
`City-<that id>`. Any other City room is still refused, now as `not sent to this city`. The development switch
keeps its meaning. After a backend restart the hubs get new ids, so a player already in Ramsgate chats again
after their next travel. `src/realtime/cityRooms.test.ts` covers it.

### 0026: the player's game logs what happens to its inventory and loadout

`ParadoxRuntime/dllmain.cpp`, the player's game only, logging only. Twice on 17 Oct the arsenal showed an empty
loadout ("Novo Equipamento", no armour, power 0) and the HUD its placeholder weapon XP (8,888 / 100,000) while
the character still wore its gear. Both times the game had just repaired an inventory state hash mismatch, once in
Ramsgate (a reward transaction landed between the server's and the client's inventory load) and once in a hunt
after a material pickup, and the inventory was fetched again. Nothing in the logs shows what the client did then.

The client now writes a `[ClientInventoryTrace #n]` line before and after every inventory or loadout RPC and
OnRep it runs (`ArchonInventory.Client*`, `HandleItemStateUpdates`, `ServerHandleInventoryHashMismatch`,
`ArchonInventory.OnRep*`, `ArchonLoadout.OnRep*` / `Client*`, `ArchonLoadoutSlotData.OnRep*`), at most 600
per game run, with the local loadout's active slot, slot count and how many of the five equipped items
(weapon, helmet, chest, arms, legs) the game's own getters resolve. When the arsenal empties again, the lines
around that moment in `mysticparadox_dll_port0.log` show whether the refresh dropped the equipped items.
`DISABLE_CLIENT_INVENTORY_TRACE.flag` beside the game executable turns it off. It is a runtime patch: build the
runtime again and publish it.

### 0027: the arsenal looks its items up again when the inventory arrives late

`ParadoxRuntime/dllmain.cpp`, the player's game only. The arsenal emptied again on 19 Oct in a hunt reached by
searching from another hunt. The 0026 trace showed the cause: in that hunt the loadout's `Inventory` reached the
client 1.2 s after its slots (`ArchonLoadout.OnRep_Inventory` came last instead of first). Each loadout item lives in
a `UArchonLoadoutItemSlot`, and only the CRC of its item id replicates (`FItemInstanceIdRepl::ItemInstanceID` is
`RepSkip`). The client turns the CRC back into the id in `UArchonLoadoutItemSlot::OnRep_ItemInstanceIdChanged`
(`+0x1BADCE0`) by looking in the slot's own `Inventory` (`+0x28`), and returns without doing anything while that
pointer is null. Nothing looks it up again later: the slot's `Inventory` has no OnRep, and
`AArchonLoadout::OnRep_Inventory` (`+0x1BAD7D0`) re-resolves only the loadout's own ids (emotes, banner, title),
not the slots'. The arsenal then shows empty slots and the HUD shows its placeholder weapon XP, while the server and
the character still have the gear. The same thing can happen after an inventory hash-mismatch refresh (the 17 Oct
cases).

About once a second, the client now looks through the local loadout's item slots. A slot qualifies when it has a
CRC but no item id and its inventory has finished loading. For each one, it runs the slot's own
`OnRep_ItemInstanceIdChanged` again, then calls `OnRep_GenericLoadoutChanged` on the loadout so the menus redraw. A
slot whose id is still unknown after three tries is left alone. Each try writes a `[LoadoutSlotRepair]` line to
`mysticparadox_dll_port0.log`, and slots still waiting for their inventory get one line per loadout.
`DISABLE_LOADOUT_SLOT_REPAIR.flag` beside the game executable turns it off. Correction to 0026: its
`equippedResolved` count is the number of slot objects that exist, not the number of items they resolve to. It
is a runtime patch: build the runtime again and publish it.

### 0028: several bounty tokens stay active

`ParadoxBackend/src/bountyState.ts`. Only one bounty token could stay active: with a second one drafted, entering a
hunt brought back only the last one. Drafting a token makes the server save `{ bounties: [the bounty just drafted],
draft_data }`. The backend read a save with one draft-data key as the component's whole list and deleted every
stored bounty of that group the save did not name, so each new token deleted the ones before it. The city server
still had them in memory, but the hunt server loads the board from the backend, so only the last one came back.
Token bounties (the base group) are now upserted, like Dauntless Revived's metagame does with every bounty save; a
token leaves the board only when the server claims or abandons it through `POST /bounty/delete` (0010). The daily
bounty and the season challenges keep the replace rule. Bounties deleted before this patch do not come back.

### 0029: cell fusion at the Middleman takes 30 minutes

`ParadoxRuntime/dllmain.cpp`, the game servers and the player's game. Each fusion recipe's `TimeRequired` comes
from the game's cell exchange data table (row struct `CellExchangeTableData`, held by
`UArchonGameInstance::CellExchangeTable`): the server builds a started fusion's end time from it
(`UArchonInventory::ExchangeCells`) and the Middleman's screen shows it. `MpSetCellFusionTime` sets every recipe to
30 minutes in memory, from both ProcessEvent hooks, every 2 seconds until the table is found and then every 30
seconds, and logs `[CellFusion]` with the old times. This is how Dauntless Revived does it (its `CellFusionTime.h`,
1 hour). `CELL_FUSION_MINUTES` (0 to 10080) sets another time; `CELL_FUSION_MINUTES=0` or
`KEEP_CELL_FUSION_TIME.flag` beside the game executable keeps the game's own times. The server decides the real
time; the player's game needs the new runtime only for the Middleman to show 30 minutes. Fusions started before the
update keep the end time they were given.

### 0030: Platinum offers are free

`ParadoxBackend/src/platinumStoreFree.ts`, `controllers/store.ts`, `routes/store.ts`. Every price in Platinum
(webstore, dyes, Hunt Pass rank skips and tracks, the Middleman's fusion slots 2 and 3) is listed as 0 by
`GET /product/skus/public` and nothing is debited for it at purchase. Prices in other currencies (Lady Luck's marks,
the Reward Cache's coins) are unchanged. The SKU data keeps its real prices, so `PLATINUM_STORE_FREE=0` in the
backend's environment brings them back. A one-time offer bought before this patch was recorded with its old price,
so buying it again answers 409 (already bought) instead of replaying the old purchase.

### 0031: broken Behemoth parts show as broken (server locale)

`ParadoxRuntime/dllmain.cpp`, server side. Ported from Dauntless Revived (`UndauntedInternalServer/ServerLocale.h`,
commit 9245d3f): they traced parts that stay attached on the player's screen after a rewarded break to servers
whose Windows culture writes decimals with a comma, which then misread decimal-point values. The game server now
serves its engine command line with `-CULTURE=en-US` (through the `FCommandLine::Get` hook, unless a `-culture=` is
already there) and sets the "C" locale on the game's CRT (`ucrtbase.dll`) and on the runtime's own before the engine
starts. The server log shows `[ServerLocale] windows_user_locale=... decimal_before='...' decimal_after='.'` and
`[CommandLine] appended -CULTURE=en-US`. `DISABLE_SERVER_LOCALE_FIX.flag` beside the game executable turns it off.
Only the game servers need the new runtime; the players' games are unchanged.

### 0032: weapon reforge resets the weapon and raises its reforge level

`ParadoxBackend`. A reforge on the game server (`UPlayerExperienceComponent::ServerPrestigeExperience`) takes 100
Aethersparks and then sends `DELETE /progression/<user>/ExperienceTrack_Weapon_<weapon>`. That route did not exist
(404), so the sparks were spent but the weapon was never reset and its reforge level never rose. The route now
exists, for game servers only: the weapon's experience track loses the XP of its full curve (XP above it stays)
and its confirmed ranks go back to 0, and the weapon's prestige track (`PrestigeTrack_Weapon_<weapon>`) moves up
one rank on its served curve, progress and confirmed rank together. A weapon that is not at its last rank is
refused with 409, so a repeated request does not count a second reforge. The backend log shows
`[Reforge] user=... track=... reset ... PrestigeTrack_Weapon_... rank=N`.

### 0033: the on-demand Training Grounds stop again once empty

`ParadoxDirector/src/controllers/hubOccupancy.ts` (new, with `hubOccupancy.test.ts`), `hubSupervisor.ts`
(`release`) and `gameservers.ts`. With `TRAINING_ON_DEMAND=1` (patch 0021) the Training Grounds started on the
first request and then ran for good, empty or not. `TRAINING_IDLE_STOP_MINUTES` (1-1440; unset or 0 keeps the
old behaviour) stops them once no player has been in them for that long, and the next request starts them again.

The Director does not see the game servers' connections, so it reads them from the runtime's own log: every
change of the remote-client count is a `[NetConnEdge] ... connCount <before> -> <after>` line in
`mysticparadox_dll_port8776.log` beside the game executable (`GAMESERVER_RUNTIME_LOG_DIR`). It reads only what
the current process wrote (from where the file ended when the hub started; under Wine the pid in the log is not
the Director's), and a count it has not read yet never stops anything. The idle time counts from the latest of
"the count went to 0", "the hub became ready" and "someone asked for the Training Grounds" (a player on the way
in has no connection yet). The watchdog checks once a minute and sends the process the SIGTERM a readiness
timeout sends; its exit releases the hub without a restart or a backoff (`[TrainingOnDemand] the Training
Grounds stopped`). A request during the stop waits for it, then starts a fresh one. Any other exit (a crash)
still restarts them as before.

## Store captures

[`paradox/backend/store-captures.json`](../paradox/backend/store-captures.json) pins a second source, read by
Mystic Paradox's store generators and not copied here: five captures of the live store service from
[EisigesEis/DauntlessEndpointDocumentation](https://github.com/EisigesEis/DauntlessEndpointDocumentation)
(Lady Luck's rotation, the platinum webstore and dyes, the Hunt Pass rank skips and passes), with the
commit and each file's SHA-256. `npm run gamedata` fetches that commit into `.workspace/captures/` and
writes the files to `paradox/DauntlessEndpointDocumentation/` (git-ignored), where
`scripts/enrich_ladyluck_store.ts` and `scripts/generate_platinum_store.ts` look for them. A file whose
bytes do not match its pinned hash is refused.
