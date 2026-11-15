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

## Maintenance and restart

`src/downtime/`: `/maintenance` and `/restart` of the Fabric mod `minehouse-tweaks`.

| Command | What it does |
| --- | --- |
| `/maintenance` | Shows the mode or the countdown to it. |
| `/maintenance on [время] [причина]` | Closes the server: everyone without `minehouse.maintenance.bypass` is kicked and refused at login, the server list shows a notice. With a time (`15m`, `1h30m`, `10с`) it counts down first. |
| `/maintenance cancel`, `/maintenance off` | Stops the countdown; opens the server again. |
| `/restart`, `/restart now` | Kicks everyone and stops the server as `/stop` does; the host starts it again. |
| `/restart <время> [причина]`, `/restart status`, `/restart cancel` | The same after a countdown with a bossbar, chat warnings, titles and a sound. A bare number is seconds, a day at most. |

Permissions are `minehouse.command.maintenance` and `minehouse.command.restart`; the console
may run both, so a deploy can send `restart 5m`. One countdown runs at a time, a new one
replaces the old. The mode is kept in `maintenance.toml` in the data directory and survives
restarts and image updates. The texts are in `config/minehouse/downtime.toml` (`&` colors,
`{time}`, `{reason}`), read again before every use.

Maintenance refuses a login before the login check asks the site: both sit behind one login
gate (`MinehouseGate` in `lib.rs`). `steel-core` got `Server::set_status_motd` for the notice
in the server list and `Server::player_effective_permissions` for the bypass of a player who
is still logging in.

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

## Chat

`src/chat/`: the Fabric mod `minehouse-chat`.

| What | How |
| --- | --- |
| Local chat | A plain message reaches players of the same world within `local.radius` blocks. |
| Global chat | `!text`, `/g text`, or `/g` to switch the channel (`/l` switches back). |
| Private messages | `/msg <ник> <текст>` (`/tell`, `/w`, `/m`), `/r <текст>` answers the last partner. |
| Ignore | `/ignore <ник>` toggles, `/ignore` lists. Applies to chat and private messages. |
| Mentions | A nickname in a message plays a sound to that player. |
| Role badges | `{prefix}`: the first group of `role.order` the player is in. |
| Announcements | `$$text` from a player with `minehouse.announce`: everyone gets it, framed, with a sound. |
| Mutes | `/mute <ник> [причина]`, `/tempmute <ник> <срок> [причина]`, `/unmute <ник>`, `/mutelist`. A muted player can't use chat, private messages, `/me` or `/say`. |

The channel and private message commands are open to every player; the mute commands and
`/chat reload` need `minehouse.command.<name>`. Our `/msg`, `/tell` and `/w` replace the
vanilla ones, which stay reachable as `/minecraft:msg`.

Every text and option is in `config/minehouse/chat.toml`, flat keys such as `local.format`;
the commented defaults are `src/chat/default.toml`. A key missing from the file keeps its
default, an empty value switches it off, `/chat reload` re-reads the file. Mutes and ignore
lists are `chat-mutes.json` and `chat-ignores.json` in the data directory; the mute file has
the shape of the mod's `minehouse-chat-mutes.json`, so it can be copied from the old server.

Like the mod, chat lines go out as system messages, so nothing depends on signed chat. The
core hook is `ChatInterceptor` (`steel-core/src/server/chat_interceptor.rs`). Not ported:
the `{lp_prefix}` and `{lp_suffix}` placeholders of `LuckPerms`.

## Tab list

`src/tab/`: what Styled Player List did on the Fabric server. `config/minehouse/tab.toml`
holds the header and footer lines (`{title}`, `{online}`, `{max}`, `{ping}`, `{tps}`; the
title is bold with a color gradient), the look of a nickname and `[[styles]]`: the first
style whose `group` a player is in (and none of `not_groups`, and not a nickname of `except`)
replaces the look and sets `order`, where higher is higher in the list. The defaults are our
style: a red dot and the top of the list for `admin`.

Header and footer go out once a second with the player's own ping; names and order when
someone joins or leaves or a player's style changes. The core hook is `TabListProvider`
(`steel-core/src/server/tab_list.rs`), which replaces the built-in header and footer.

## Voice chat

Proximity voice for clients with the [Plasmo Voice](https://github.com/plasmoapp/plasmo-voice)
mod, version 2: the server speaks its protocol itself, so players keep the mod they have.
On the Fabric server this was the Plasmo Voice server mod.

Settings: `config/minehouse/voice.toml`, off by default. The names follow
`plasmovoice/server/config.toml`.

```toml
enabled = true
# Clients keep their volumes per server id: take the one of the Fabric server.
server_id = "72a2a425-e7fe-4db8-9ac1-ecd979766d0c"
host = "0.0.0.0"
port = 24454          # UDP: open it next to the game port
public_host = ""      # empty: the address the client joined the game with
public_port = 0       # 0: port
distances = [8, 16, 32]
default_distance = 16
```

How it works: the client answers a question on the plugin channel `plasmo:voice/v2` with an
RSA key, gets the UDP address and a secret, and its first datagram opens the connection. The
sound is opus under an AES key that every client gets encrypted with its own RSA key; the
server never decodes it and passes each datagram on to the players within earshot in the
same world (the chosen distance plus `max_extra_audio_broadcast_distance`, at most twice
the distance). A client silent for `keep_alive_timeout_ms` is dropped and asked again.

Not ported: groups and other addons, `/vmute` and the other commands, hiding vanished
players. The core hook is `PluginChannelHandler`
(`steel-core/src/server/plugin_channels.rs`); answers go out as `CPluginMessage`.

The packet formats follow the `protocol` module of Plasmo Voice (LGPL-3.0, by Apehum).

## Block log

Who broke and placed which block, and who put what into which chest and took it out. On the
Fabric server this was CoreProtect; its history comes over with `/co import`.

The server tick only queues an event. One thread writes the queue to
`<data>/blocklog.db` (SQLite) in batches, another answers lookups from its own read-only
connection, so neither a busy moment nor a search over the whole history holds the tick. A
lookup first waits for the queue to reach the disk, so it sees what happened a moment ago.

Settings: `config/minehouse/blocklog.toml`, on by default.

```toml
enabled = true
keep_days = 90      # older rows are deleted; 0 keeps everything
page_size = 8
near_radius = 5     # /co near
max_radius = 100    # the widest r:

[import_worlds]     # world names of the imported history that differ from ours
# "world_nether" = "minecraft:the_nether"
```

`/co` (alias `/blocklog`) is for the console and players with `minehouse.command.co`:

| Command | What it does |
| --- | --- |
| `/co inspect`, `/co i` | Inspector on or off. Hitting a block shows its history, using it also shows what went in and out. The click itself does nothing. |
| `/co lookup <words>`, `/co l` | `u:Nick,Nick2` `t:3d` `r:10` (`r:#global`) `b:chest,tnt` `a:block`, `+block`, `-block`, `container`, `+container`, `-container`. Without `r:` the whole history is searched. |
| `/co near` | Everything within `near_radius` blocks. |
| `/co page <n>`, `/co p` | Another page of the last answer. |
| `/co status` | How much the log holds. |
| `/co purge <age>` | Deletes rows older than `90d` (30 days at least). |
| `/co import` | Imports `<data>/coreprotect-export`. |

Moving the history of CoreProtect: `src/blocklog/export.sql` turns a copy of its DuckDB
database into `blocks.csv`, `containers.csv` and `data_map.csv`; put them into
`<data>/coreprotect-export` and run `/co import`. It runs on its own thread, reports to the
log, and can be run again: it goes on after the last row it brought over. Nicknames like
`#tnt` are causes that are not players. Item components of old container rows are not
brought over, only the item and the amount.

Not there yet: rollback and restore (the rows already keep the block state and the item
NBT they need), blocks changed by anything but a player's hand (explosions, fire, fluids,
buckets), signs, chat and sessions. The core hook is `WorldLog`
(`steel-core/src/server/world_log.rs`).
