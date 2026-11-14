# steel-minehouse

MineHouse features for our Steel fork: the survival mods from the Fabric server
(`P5ina/minehouse-landing`, `P5ina/minehouse-mods`), ported one by one. Everything
MineHouse-specific lives here; `steel-core` only gets small hooks.

Configs are in `config/minehouse/<module>.toml`, created with defaults on first start.

## Console

Port of `console-mod` (remote console without open ports). Polls
`POST /api/mc/console/poll` on the site, runs the queued commands as RCON and reports their
output, new log lines and the server state.

`config/minehouse/console.toml` has the fields of the Fabric `minehouse-console.json` in
snake_case. Environment overrides: `MINEHOUSE_CONSOLE_SECRET` (the site's
`MC_CONSOLE_SECRET_<ID>`), `MINEHOUSE_SERVER_ID`, `MINEHOUSE_SERVER_NAME`, `MINEHOUSE_API_URL`.
Without a secret the console stays off.

## Auth

Port of the login check of `mod/` (MineHouse Auth). A verified login waits in the
configuration phase while the site answers `POST /api/mc/check` for its nickname and IP:

- a trusted device, or a licensed login Gate verified with Mojang, goes straight in;
- a banned account, one outside Discord or one missing from this server's whitelist is
  disconnected with the site's text;
- anyone else sees the «Вход на MineHouse» dialog with a code and gets in once the site says
  the code was confirmed (`GET /api/mc/request/{id}`); «Отмена» or leaving cancels the request.

While the site is down, nickname + IP pairs confirmed in the last `offline_trust_days` still
get in (`auth-trusted.json` in `MINEHOUSE_DATA_DIR`, default `config/minehouse/data`). A
login is refused while the nickname is already playing, so nobody can kick a player by
joining under their name.

Steel has to sit behind Gate: Gate checks licensed nicknames and hands out the stable UUID
from the site, this module only reads the result from the forwarded profile.

`config/minehouse/auth.toml` has the fields of the Fabric `minehouse-auth.json` in
snake_case. Environment overrides: `MINEHOUSE_API_SECRET` (the site's `MC_API_SECRET`),
`MINEHOUSE_API_URL`, `MINEHOUSE_AUTH_SERVER_ID` (falls back to `MINEHOUSE_SERVER_ID`).
Without a secret only players from the local cache get in. `enabled = false` turns the check
off.

In `steel-core` this is the `LoginGate` hook (`server/login_gate.rs`); `steel-login` holds the
client, keeps it alive and refuses a client that finishes the configuration on its own.

Not ported yet: feature flags, Twitch.

## Moderation

`src/moderation/`: the ban, warn and whitelist commands of the Fabric mod `minehouse-auth`.
Bans, warns and the whitelist live on the site and hang on the Discord account, so they cover
every nickname of the player.

| Command | Permission |
| --- | --- |
| `/ban <ник> [причина]`, `/tempban <ник> <срок> [причина]`, `/unban <ник> [причина]`, `/banlist` | `minehouse.command.ban`, `.tempban`, `.unban`, `.banlist` |
| `/warn <ник> [причина]`, `/unwarn <ник>`, `/warns <ник>` | `minehouse.command.warn`, `.unwarn`, `.warns` |
| `/whitelist add\|remove <ник или Discord ID>` | `minehouse.command.whitelist` |

A length is `30m`, `12h`, `7d`, `2w`, `1mo` or a sum like `1d12h`. The console may run all of
them; players need the permission from a group in `config/groups.toml`.

A command calls the site from the Tokio runtime and finishes on a later tick
(`SuspendedCommand`), so a slow site never holds the tick. Every `sync_seconds` the module
also asks the site who of the online players was banned elsewhere or taken off this server's
whitelist and kicks them, and announces the bans made on the site or in Discord.

`config/minehouse/bans.toml` has the settings of the Fabric `minehouse-bans.properties` in
snake_case: the chat formats (`&` colors, `{player}`, `{reason}`, `{by}`, `{duration}`,
`{until}`, `{count}`, `{limit}`), `warn_limit`, `warn_ban_duration`, `warn_ban_reason`. It is
read again before every use. The commands are registered only while the login check is on:
they share its site client and server id.

`/ban`, `/tempban`, `/unban` and `/warn` take `--wipe` before the reason: the account also
starts over as a new player (`src/moderation/wipe.rs`). Whoever is online is kicked first, then
the data, statistics and advancements of every nickname, in every domain, move to
`wipes/<uuid>-<time>/` in the data directory (`PlayerDataStorage::archive_player` in
`steel-core`); nothing is deleted, so a reset can be undone by hand while the server is
stopped. The same happens for the resets asked for on the site, which the sync picks up under
the console id of the server (`MINEHOUSE_SERVER_ID`, else the id of the login check).
Scoreboard scores are not reset.

## Roles

`src/roles/`: Discord roles decide the permission groups, as `roleSync` of the Fabric mod did
with `LuckPerms`. `config/minehouse/roles.toml`:

```toml
enabled = true
sync_seconds = 60

[groups]
ADMIN = "admin"
"Модератор" = "moderator"
```

A key is a role name (case-insensitive) or a role ID, a value is a group of
`config/groups.toml` or several separated by commas. Players are checked a few seconds after
joining and then every `sync_seconds`. Only the groups listed here are added and removed;
every other group of a player stays. A player who left the Discord server loses all of them;
when the site does not answer, nothing changes. A listed group that does not exist is skipped
with a warning. The sync runs only while the login check is on.
