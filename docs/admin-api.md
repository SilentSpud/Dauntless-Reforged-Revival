<!-- markdownlint-configure-file { "MD013": { "line_length": 120, "tables": false } } -->

# Admin API

A GraphQL API for the host's own tools, such as a Discord bot: kick, time out and ban players (and undo it), and
look up a player, the server and its hunts. It is the `admin` service (`rust/services/admin`). It listens on
`127.0.0.1:61003` only, wants its key on every request, and the gateway never forwards to it, so players can't reach
it. What it trusts and why is in [security/admin-api.md](security/admin-api.md).

## Running it

`reforged-host setup` makes its key and writes `.workspace/config/admin.env`. `reforged-host start` runs it after
the content server when the `admin` binary is in the release (or in `rust/target/`). In Docker it is the `admin`
service in `compose.yaml`. Nothing needs setting in `host.toml`.

What it needs, all written by setup:

| Variable | In | What |
| --- | --- | --- |
| `REFORGED_ADMIN_API_KEY` | `secrets.env`, `admin.env` | The key callers send as `Authorization: Bearer <key>` |
| `MYSTICPARADOX_MANAGER_OPERATOR_SECRET` | `secrets.env`, `admin.env`, `backend.env` | Signs the API's calls to the backend's `/admin/v1` routes |
| `MYSTICPARADOX_MANAGER_ADMIN=on` | `backend.env` | Lets the backend accept those signed calls |
| `REFORGED_ADMIN_KEY` | `secrets.env`, `admin.env`, `backend.env` | The backend's existing admin key, for the kick |
| `PORT`, `BIND_HOST` | `admin.env` | 61003, 127.0.0.1. Any non-loopback address stops the start |
| `BACKEND_URL`, `REFORGED_DIRECTOR_URL` | `admin.env` | `http://127.0.0.1:61000` and `:61001`. Loopback only |
| `REFORGED_ADMIN_API_TIMEOUTS_FILE` | `admin.env` | `.workspace/config/admin-timeouts.json`, which holds when each timeout ends |
| `REFORGED_MAX_HUNT_SERVERS`, `REFORGED_SERVER_MODE` | `admin.env` | From `host.toml`, for `huntStatus` and `serverStatus` |

A host moving from an older setup gets the new secrets on the next `setup` (or `update`). The secrets it already has
stay as they are.

## The schema

The full schema, with every field's description, is
[rust/services/admin/schema.graphql](../rust/services/admin/schema.graphql) (`admin schema` prints it; a test keeps
the file current). In short:

```graphql
type Query {
  player(username: String!): Player            # null when there is no such account
  serverStatus: ServerStatus!                  # name, mode, registration, version, commit, uptimeSeconds, onlineCount
  huntStatus: HuntStatus!                      # maxHuntServers, running, free, hunts { title, players { username } }
}

type Mutation {
  kick(username: String!): KickResult!
  timeout(username: String!, minutes: Int, until: String, reason: String!): Player!
  liftTimeout(username: String!): Player!
  ban(username: String!, reason: String!): Player!
  unban(username: String!, reason: String): Player!
}

type Player {
  userId: String!  username: String!  status: AccountStatus!    # ACTIVE, BANNED, DISABLED
  restriction: Restriction       # kind BAN | TIMEOUT | DISABLED, reason, since, by, until
  online: Boolean!  location: Location    # kind HUB | HUNT | TRAINING | TUTORIAL, instanceId, title
  admin: Boolean!  createdAt: String  lastLoginAt: String
}
```

How the moderation works:

- **kick** closes the player's chat connections and revokes their sign-in tokens (`POST /reforged/api/Kick` on the
  backend). They aren't banned and can log in again with their key. Nobody has yet checked what the 1.14.7 client
  does after losing its chat connection mid-hunt.
- **timeout** bans the player with the reason `Timed out until <time>: <reason>`, which the player sees in the
  launcher. The API records the end time and lifts the ban within 30 s of it, also after a restart. `minutes` or
  `until` (RFC 3339), not both, ending within a year. It replaces an earlier timeout. It is refused with
  `ALREADY_BANNED` for a player who is banned for good or disabled, since lifting it would undo that too.
- **ban** bans until someone unbans, and replaces a timeout. The backend audits it as `manager:<operator id>` and
  revokes the player's refresh tokens. A game that is running stops at its next request, because the backend checks
  bans on every call.
- **unban** lifts a ban or a timeout, and **liftTimeout** lifts only a timeout.
- A timeout whose ban changed in the meantime (lifted, or replaced from the backend's dashboard) is only forgotten
  when it runs out, never lifted.
- Accounts with the backend's `admin` role are refused (`PROTECTED_ACCOUNT`).
- Names follow the launcher's rule (3 to 16 letters, digits or `_`) and match regardless of letter case. Reasons are
  3 to 400 characters on one line.

Errors are GraphQL errors with a code in `extensions.code`: `BAD_INPUT`, `NOT_FOUND`, `PROTECTED_ACCOUNT`,
`ALREADY_BANNED`, `NOT_BANNED`, `NOT_TIMED_OUT`, `BACKEND_UNAVAILABLE` (the backend or Director didn't answer),
`BACKEND_REFUSED` (its message says when the secrets don't match: run `setup` and restart), `STATE_NOT_SAVED`. A
request refused before any GraphQL runs gets a 4xx with the same shape: 401 `UNAUTHENTICATED`, 403 `FORWARDED`,
400 `BAD_OPERATOR` or `BAD_REQUEST`, 415 `NOT_JSON`.

## Calling it

`POST http://127.0.0.1:61003/graphql`, with `Content-Type: application/json` and a body of
`{"query": ..., "variables": ...}`. The headers:

- `Authorization: Bearer <REFORGED_ADMIN_API_KEY>`
- `x-reforged-operator-id: <Discord user id>`: who the moderation is for, digits only. The backend's audit log records
  it as `manager:<id>`. Leave it out and it is `0`, the host.

From the server itself:

```bash
curl -s http://127.0.0.1:61003/graphql -H "Authorization: Bearer $REFORGED_ADMIN_API_KEY" \
  -H 'Content-Type: application/json' -d '{"query":"{ serverStatus { name onlineCount uptimeSeconds } }"}'
```

## A Discord bot calling it

The bot runs on the server itself, because the API is loopback only. A bot on another machine reaches it through an
SSH tunnel (`ssh -L 61003:127.0.0.1:61003 host`), never by changing `BIND_HOST`. Give the bot the key from
`secrets.env` in its own environment, and never put it in the bot's repository.

A discord.js sketch in the style of Dauntless Revived's `UndauntedDiscordBot`: `/status`, `/player`, `/hunts` for
everyone, and `/kick`, `/timeout`, `/ban`, `/unban` only for members who can ban in Discord.

```js
import { Client, Events, GatewayIntentBits, MessageFlags, PermissionFlagsBits, SlashCommandBuilder } from 'discord.js';

const API = 'http://127.0.0.1:61003/graphql';
const KEY = process.env.REFORGED_ADMIN_API_KEY;

async function admin(query, variables, operatorId) {
  const answer = await fetch(API, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${KEY}`, 'x-reforged-operator-id': operatorId },
    body: JSON.stringify({ query, variables }),
    signal: AbortSignal.timeout(15000),
  });
  const body = await answer.json();
  if (body.errors?.length) throw Object.assign(new Error(body.errors[0].message), { code: body.errors[0].extensions?.code });
  return body.data;
}

const name = o => o.setName('username').setDescription('Their launcher username').setRequired(true).setMinLength(3).setMaxLength(16);
const reason = o => o.setName('reason').setDescription('Shown to the player in the launcher').setRequired(true).setMinLength(3).setMaxLength(400);
const moderator = b => b.setDefaultMemberPermissions(PermissionFlagsBits.BanMembers);

export const commands = [
  new SlashCommandBuilder().setName('status').setDescription('Server status'),
  new SlashCommandBuilder().setName('hunts').setDescription('Running hunts'),
  new SlashCommandBuilder().setName('player').setDescription('Look up a player').addStringOption(name),
  moderator(new SlashCommandBuilder().setName('kick').setDescription('Drop a player now').addStringOption(name)),
  moderator(new SlashCommandBuilder().setName('timeout').setDescription('Ban for a while').addStringOption(name)
    .addIntegerOption(o => o.setName('minutes').setDescription('How long').setRequired(true).setMinValue(1).setMaxValue(525600))
    .addStringOption(reason)),
  moderator(new SlashCommandBuilder().setName('ban').setDescription('Ban until unbanned').addStringOption(name).addStringOption(reason)),
  moderator(new SlashCommandBuilder().setName('unban').setDescription('Lift a ban or timeout').addStringOption(name)),
].map(c => c.toJSON());

const PLAYER = `{ username status online location { title } restriction { kind reason until } }`;

const handlers = {
  status: async (_, op) => {
    const { serverStatus: s } = await admin(`{ serverStatus { name mode version onlineCount uptimeSeconds } }`, {}, op);
    return `**${s.name}** (${s.mode.toLowerCase()}, ${s.version}): ${s.onlineCount} online, up ${Math.floor(s.uptimeSeconds / 3600)} h`;
  },
  hunts: async (_, op) => {
    const { huntStatus: h } = await admin(`{ huntStatus { running free maxHuntServers hunts { title players { username } } } }`, {}, op);
    const lines = h.hunts.map(x => `• ${x.title}: ${x.players.map(p => p.username ?? '?').join(', ') || 'nobody yet'}`);
    return [`${h.running}/${h.maxHuntServers} hunts running, ${h.free} free`, ...lines].join('\n');
  },
  player: async (i, op) => {
    const { player: p } = await admin(`query($u: String!) { player(username: $u) ${PLAYER} }`, { u: i.options.getString('username') }, op);
    if (!p) return 'No such player.';
    const where = p.online ? (p.location ? `in ${p.location.title}` : 'online') : 'offline';
    const r = p.restriction;
    return `**${p.username}**: ${where}${r ? `, ${r.kind.toLowerCase()}${r.until ? ` until ${r.until}` : ''} (${r.reason})` : ''}`;
  },
  kick: async (i, op) => {
    const { kick } = await admin(`mutation($u: String!) { kick(username: $u) { username } }`, { u: i.options.getString('username') }, op);
    return `Kicked ${kick.username}.`;
  },
  timeout: async (i, op) => {
    const v = { u: i.options.getString('username'), m: i.options.getInteger('minutes'), r: i.options.getString('reason') };
    const { timeout } = await admin(`mutation($u: String!, $m: Int, $r: String!) { timeout(username: $u, minutes: $m, reason: $r) ${PLAYER} }`, v, op);
    return `${timeout.username} is timed out until ${timeout.restriction.until}.`;
  },
  ban: async (i, op) => {
    const v = { u: i.options.getString('username'), r: i.options.getString('reason') };
    const { ban } = await admin(`mutation($u: String!, $r: String!) { ban(username: $u, reason: $r) ${PLAYER} }`, v, op);
    return `${ban.username} is banned.`;
  },
  unban: async (i, op) => {
    const { unban } = await admin(`mutation($u: String!) { unban(username: $u) ${PLAYER} }`, { u: i.options.getString('username') }, op);
    return `${unban.username} can play again.`;
  },
};

const client = new Client({ intents: [GatewayIntentBits.Guilds] });
client.on(Events.InteractionCreate, async interaction => {
  const handler = interaction.isChatInputCommand() && handlers[interaction.commandName];
  if (!handler) return;
  await interaction.deferReply({ flags: MessageFlags.Ephemeral });
  try {
    await interaction.editReply({ content: await handler(interaction, interaction.user.id), allowedMentions: { parse: [] } });
  } catch (error) {
    // The API's messages are written for the host; NOT_FOUND, PROTECTED_ACCOUNT and the like read fine as they are
    await interaction.editReply(error.code === 'BACKEND_UNAVAILABLE' ? 'The server is not running.' : `Refused: ${error.message}`);
  }
});
client.login(process.env.DISCORD_TOKEN);
```

Register `commands` with `rest.put(Routes.applicationGuildCommands(appId, guildId), { body: commands })`, or the
create-or-patch pattern of `UndauntedDiscordBot/commands.mjs`. `setDefaultMemberPermissions` hides the moderation
commands from members who can't ban. A server admin can change that per role in Discord's integration settings,
and the API still never acts on an account with the backend's `admin` role.
