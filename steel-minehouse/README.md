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
| `/co rollback <words>`, `/co rb` | Undoes what the words match: broken blocks come back, placed ones go, taken items return to the container, put ones leave it. Needs `t:` and `u:` or `r:`. |
| `/co restore <words>`, `/co rs` | Redoes what a rollback undid. Same words. |
| `/co status` | How much the log holds. |
| `/co purge <age>` | Deletes rows older than `90d` (30 days at least). |
| `/co import` | Imports `<data>/coreprotect-export`. |

Moving the history of CoreProtect: `src/blocklog/export.sql` turns a copy of its DuckDB
database into `blocks.csv`, `containers.csv` and `data_map.csv`; put them into
`<data>/coreprotect-export` and run `/co import`. It runs on its own thread, reports to the
log, and can be run again: it goes on after the last row it brought over. Nicknames like
`#tnt` are causes that are not players. Item components of old container rows are not
brought over, only the item and the amount.

A rollback reads its rows on the reader thread and changes the world on the tick, 1500
rows a tick, without neighbor updates. Rows in chunks that are not loaded are left for
another run, and so are items that have no container or no room; only what was really
changed is marked as rolled back. One run takes at most 500 000 rows.

Not there yet: blocks changed by anything but a player's hand (explosions, fire, fluids,
buckets), signs, chat and sessions. The core hook is `WorldLog`
(`steel-core/src/server/world_log.rs`).

## Bank

The Fabric `MineHouse Bank`, piece by piece. One diamond ore (plain or deepslate) is one АР. The
money lives on the site: every operation is a signed `POST /api/mc/bank/<action>` (the contract is
in minehouse-landing `bank-mod/README.md`), the server only moves ores in and out of inventories.

The base the ATM, the cards, the terminals and the spawn zones stand on:

- **Calls to the site** never block the tick: the answer comes back on a later tick. A call that
  fails (no secret, timeout, HTTP error) answers `{ok: false, error: "offline"}`.
- **The journal** `bank-journal.json` in the data directory (`MINEHOUSE_DATA_DIR`) keeps what is
  half done, so a crash or a lost answer never loses or doubles ores. It is rewritten (temp file
  and rename) after every change. Kinds: `deposit`, `delivered`, `vending`, `owed`.
- **On join** a player gets the items the bank owes them (`owed`), the ores of withdrawals that
  were paid but not handed out (`pending`, then `delivered`), and one line about what their
  terminals sold while they were away (`sales`). On leave the sales are marked seen.
- **Every 30 seconds** and on start, a `delivered` the site has not heard yet is sent again, and
  so is a `deposit` whose answer was lost.

**The ATM** (`minehouse-bank:atm`, given by admins) is a block of ours: the client sees a barrier
with an item display in it that carries the model, turned to face the player who placed it. The
display is put back on use if it is missing and removed with the block. A right click asks the
site for the account and opens a chest window:

- Without an account the window has one button, «Открыть счёт». It opens the personal account
  and hands out the card.
- With an account: the balance, five slots that take diamond ores and «Внести», «Снять АР» with
  an amount picker (up to `max_withdraw` and the balance), «Выдать карту», «Перевыпустить»,
  «Открыть ещё счёт» (the name is typed in an anvil window) and «Сменить счёт» when the player
  has more than one.
- A deposit takes the ores first and writes them to the journal as `deposit`; a refusal gives
  them back (or owes them if the player has left), no answer keeps them for the retry. Ores left
  in the slots when the window closes go back to the player.
- A withdrawal hands the ores out when the site has charged the account; if the player has left
  by then, they get them on the next join.
- The window closes when the player walks further than `use_distance` or the block is gone.

**The card** (`minehouse-bank:card`) is paper with the card id, its number and the owner in
`custom_data` (`minehouse_bank_card`). An ATM opened with a card in the hand opens that card's
account.

**The terminal** (`minehouse-bank:terminal`, «Купить терминал» in the ATM for `terminal_price`)
is a small block for a counter or a chest. The client sees a state of the plant carrier with our
model, dark until the terminal is locked, and a stone button without the pack. What it is set to
is in `bank-terminals.json` in the data directory, by world and position; the till's invoice is
in memory only. A text display above it (tag `minehouse_terminal`, saved with the chunk) says
what it sells or asks for and follows the stock twice a second while a player is within 24
blocks.

- A terminal that is not locked opens the setup window to anyone: the account (the player's own
  by default, another one typed in an anvil window and checked by the site), the mode, and for
  vending the goods (the stack in the hand: its count is one sale) and the price. «Сохранить и
  закрепить» tells the site and locks it. Only breaking the block undoes that, and the site is
  told who broke it.
- **A till**: the owner sets an amount, the label shows it for `invoice_seconds`, a buyer clicks
  and pays with a card from the inventory (the pay window lets them pick one of several).
- **A vending terminal** sells from the chest or barrel under it. The goods leave the chest into
  the journal (`vending`) before the money moves: paid, they go to the buyer (or wait for the
  next join); refused, they go back to the chest; no answer, the bank asks the site about the
  operation two minutes later and does one or the other. Goods that no longer fit the chest are
  dropped on it.
- The stock under a locked vending terminal (both halves of a double chest) is not broken by
  anyone but the owner and players with `minehouse.bank.admin`, and hoppers take nothing from
  it. Opening it is not limited, as on the Fabric server. These two checks are hooks in
  steel-core (`world::block_guard`).

**Transfers**: `/transfer` (every player, from anywhere, no card) opens a window with the
account the money leaves (a click goes through the player's accounts), the ATM's amount picker up
to the balance, and the recipient: a number typed in an anvil window that the site resolves
before the transfer is offered. The player's own account and a frozen one are refused there. The
owner of the receiving account is told if they are online. On the Fabric server the window was a
line of the server menu, which Steel does not have yet.

**Spawn zones** (`bank/zones/`): a shared circle in the middle of the spawn island and lots
around it that go to auction and are rented for 7 days. A zone only says whose land it is, nothing
is protected. The site keeps everything; the server asks for the list every 30 s.

- The action bar names the zone a player walks into (free, the tenant, or the auction with its
  bid) and says when they leave. Spectators see nothing.
- Every 20 s, and at once when it changes, the site is told which lot each player stands in: it
  takes a bid only from a player inside the zone.
- Chat announces an auction that was scheduled, one that opened and one that ended; the winner
  gets a line of their own.
- `/zones` (every player) lists the zones. A lot opens its card: «Сделать ставку» while its
  auction runs and the player stands in it, «Продлить на 7 дней» for the tenant in the last two
  days of the lease. `/zones bid` opens the bid window of the zone the player stands in. A bid
  comes from the personal account; the amount starts at the least bid and follows it while the
  window is open (open windows ask the site again every 3 s). The player whose bid was beaten is
  told if they are online.
- On join a tenant whose lease ends within two days is reminded to renew.
- Boss bars: one per announced or running auction, the soonest three, with a countdown to
  its start or end (red in the last ten minutes), redrawn every second. `/zones bossbar` hides
  them for one player; who hid them is kept in `zones-hidden-bars.txt` of the data directory.
- `/zones show [num]` (every player) draws the borders with dust particles for 30 seconds, in
  the color of the zone's status, at the player's height and within 64 blocks.
- `/zones map` (every player) gives «Карта зон спавна»: a filled map the server draws itself
  and sends only as packets while it is held (no saved map data): the terrain of the loaded
  chunks, the borders of the zones in the colors of their status, a banner with the number and
  the tenant or the auction over every zone, and the holder's marker. The picture is sent again
  when the zones change, the terrain is read again every two minutes.
- `/zones auction <num> [hours] [price] [delay minutes]` (`minehouse.zones.auction` or
  `minehouse.zones.admin`, and the console), `/zones delete <num>` and `/zones reload`
  (`minehouse.zones.admin`, and the console).
- Markup (`minehouse.zones.admin`): `/zones wand` gives the stick «Разметка зон». A right click
  on a block adds a corner, with Shift the last one is taken back, a left click tells what the
  draft holds and breaks nothing. While the stick is held the draft, its corners and every
  saved zone are drawn. `/zones height <min> <max>`, `/zones circle <radius>` (a circle around
  the admin), `/zones edit <num>` (a saved zone into the draft), `/zones clear`,
  `/zones save <num> [title]` and `/zones savecommon <num> [title]` send the draft to the site.

The Fabric server hands the shapes to the client mod, which draws lines and walls instead of
particles. Steel does not know which channels a client listens to, so everyone gets particles.

The zones are on the web map too (`bank/zones/bluemap.rs`, the Fabric `ZoneBlueMap`): the
marker set `minehouse-zones` in the same `markers.json` files as the metro (see
`content/map_markers.rs`), a shape for each zone in the color of its status. Its popup is the
card of the lot: the area, the rent, the tenant or the bid, when the auction ends (Moscow
time) and a button to the zone on the site. The set is rewritten when the site's list changes
and taken off while the zones are closed.

The banker (`bank/banker/`) explains how the bank works. `/bank banker` (`minehouse.bank.banker`)
puts him where the admin stands, facing the admin; `/bank banker <nick>` with the skin of a
player, `/bank banker url <link>` with the skin PNG at the link (signed through MineSkin), and
`/bank banker remove` takes away the nearest one within five blocks. He is not a world entity:
where the bankers stand is kept in `bank-bankers.json` of the data directory and every player
within 64 blocks is sent a mannequin of their own, so he takes no harm, can't be pushed and
turns his head to each player he talks to.

A right click starts the talk. The window is one action bar line of the fonts
`minehouse-bank:dialog*` of the server pack over a fog drawn as a title (`banker/dialog.json`
is the layout `tools/gen_dialog.py` of the Fabric mod makes with the fonts): the words type
out two characters a tick, Shift shows the page, turns it or takes the answer, the wheel
picks an answer (the hotbar slot comes right back), Space, an opened window or five blocks
away end the talk.

The guides of the ATM and of the terminal are in `src/bank/guides.rs` (see Guides below).

Not here yet: of the zones the guides; the banker's body animations (they were played by the
events mod on the Fabric server).

`config/minehouse/bank.toml`:

| Key | Default | |
| --- | --- | --- |
| `enabled` | `true` | Turns the bank off entirely |
| `api_url`, `server_id`, `api_secret` | empty | Empty: taken from `console.toml`; the bank signs with the same per-server secret as the console (env `MINEHOUSE_CONSOLE_SECRET`) |
| `http_timeout_ms` | `10000` | Timeout of one call |
| `terminal_price` | `5` | What the ATM charges for a payment terminal |
| `max_withdraw` | `640` | Most ores one withdrawal hands out |
| `invoice_seconds` | `120` | How long a till's invoice lives |
| `use_distance` | `6.0` | How far from a terminal or an ATM a player may use it |
| `zones` | `true` | Whether the spawn zones are open to players; closed, only `minehouse.zones.admin` opens `/zones` |

Differences from the Fabric mod: the journal is in the data directory instead of the world
folder, and its items are hex of their NBT, not SNBT, so a Fabric journal with items in it is not
read. Move a server with an empty journal. An entry whose items can't be read stays in the file
and is logged instead of being dropped. Cards and ATMs of the Fabric server are not carried over:
the ATM there had no display entity, the card was a registry item, and a terminal kept its
settings in a block entity (a terminal block found in the world is offered for setup again).

## Guides

`src/guides`: a port of the Fabric mod MineHouse Guides. A guide is a small scene of display
entities in front of one player, sent to them only, that shows a thing of ours step by step:
blocks and items drop in, move and glow, a caption over the scene says what happens, and four
buttons under it go back, start again, go on and close. The steps play on their own; a guide
closes when the player walks 8 blocks away, changes the world, dies, or a minute after its end.

| How | |
| --- | --- |
| `/guide` | The list; a click on a line opens the guide |
| `/guide <id>` | Opens a guide |
| `/guide close` | Closes the open one |
| Sneak + right click in the air with an item of a guide | Opens it; the item's tooltip says so |

A guide is a `Guide` with steps; a step is a caption, a least time and a script
`fn(&mut Stage)` that builds on what the steps before it left (to show step 3 the first two are
replayed instantly) and puts things off with `Stage::later`. `Stage` has blocks, stretched
blocks, items, labels and models of the server pack (`Stage::model`, for blocks of ours that
a block display can't show), each under an id to move, change, glow, pulse or remove later. A
module adds its guides to the list in `guides::all` (the bank: `bank-atm`, `bank-terminal`,
shown while the bank is on).

The book «Гайды MineHouse» (`src/guides/book.rs`) is a vanilla written book marked in its
custom data: a book and a sheet of paper make one on a workbench (the recipe
`minehouse-guides:guide_book`, in everybody's recipe book from the start). Its pages are put
together every time it is opened, so a book made long ago lists the guides of today: a first
page, the contents five guides a page (a line runs `/guide <id>`), and a last page about
recipes with a link to the wiki.

Not here yet: the server menu, the guides of the zones, of the magnifier, of the metro and of
the herbs.

## Our items

`src/content`: the items of our mods on a vanilla client. On the Fabric server they were
registry items that Polymer swapped for vanilla ones on the way to the client. Here an item
of ours is a vanilla item from the start: the base item with `item_name`, `lore`,
`max_stack_size`, an `item_model` and its id in `custom_data` (`minehouse_item`). The id and
the model are the ones of the Fabric mods (`minehouse-tweaks:magnifier`), so the resource
pack the players already have draws them.

| Command | What it does |
| --- | --- |
| `/mhgive <item> [count]` | Gives an item of ours to the player who asks. Permission `minehouse.command.mhgive`. |

Items so far:

- `magnifier`, «Лупа»: shows the history of a block from the block log to any player.
  Hitting a block shows who placed and broke it, using a container shows who put in and
  took out what, using another block shows the spot in front of the clicked face. The same
  click within a minute turns the page. The click itself does nothing to the world.

Recipes are in `src/content/recipes.rs`, under the keys of the Fabric mods
(`minehouse-tweaks:magnifier`: copper ingots around a glass pane, on a stick). They are added
through `steel_registry::extend_registry` before the registry is frozen; the log says at
start how many of them the registry holds.

Not there yet: turning the registry items of a Fabric world
(`minehouse-tweaks:magnifier` as an item id) into these on load, our blocks.

## Resource pack

Our items and blocks are drawn by a resource pack. On the Fabric server Polymer builds it
at startup; here it is built ahead of time from the same assets:

```sh
python3 steel-minehouse/pack/build_pack.py ../minehouse-landing minehouse-pack.zip
```

The script takes the assets of every mod that gives its assets to Polymer, merges the
files several mods add to (atlases, fonts, languages, `sounds.json`), and refuses to build
when two mods carry different versions of another file or when a model, texture or sound
of ours is referenced but missing. The zip is reproducible, and the script prints its
SHA-1. Put the address the pack is served at and that hash into `config/config.toml`:

```toml
[server.resource_pack]
url = "https://example.com/minehouse-pack.zip"
sha1 = "..."
required = true
```

## Our blocks

A block of ours is a real block of the registry under the id the Fabric mod gave it
(`minehouse-tweaks:cloud`), so a world saved by the Fabric server loads as it is. A vanilla
client does not have the block: the server sends it a vanilla state in its place, and the
pack draws that state with our model.

Full blocks borrow note block states. A client draws every note block alike, so a real
note block always goes out as its default state, and each of the other 1349 states is free
for a block of ours. `pack/client_blocks.json` says which state is whose. The server and
`build_pack.py` both read it: the script writes `blockstates/note_block.json` from it, and
a test checks that the file lists the note block properties of the game.

Plants borrow sugar cane states the same way: a sugar cane has no collision, is drawn with
cutouts and looks alike at each of its 16 ages, so a real one goes out at age 0 and the
other 15 are free. A carrier in `pack/client_blocks.json` is such a vanilla block; another
shape of block gets another carrier, added to the file and to `blocks::carriers`.

To add a full block: define the `Block` and its behavior in `src/content/`, register both
in `content/blocks.rs`, and give it a state in `pack/client_blocks.json`. A block whose own
states look different takes a note block state for each: the entry names the state of ours
under `when` (a metro station has one per line). `blocks::show_full_block` then tells
clients what to draw, and what the block breaks like: a note block crumbles like wood.

A block that needs a tool is added to the vanilla tag (`mineable/pickaxe`) on the server
only. Clients are sent the tags without our blocks, whose ids their game does not have.

An item of ours is a vanilla item with components, so using it would run the behavior of
the vanilla item (clouds would be placed as white wool). `content::use_behavior` names the
items that behave differently.

The blocks so far, each with its recipe: the cloud («Облачко»), the metro station («Станция
метро»), the metro junction («Стрелка метро») and the block of magic leaves («Блок магических
листьев», nine magic leaves and back). A station takes its name from a name tag
with a name (right click, up to 48 characters) and its line from a dye, which also turns
the sign on the block to the color of the line; an empty hand tells what the station is.
`content/stations.rs` keeps the names and lines in `metro-stations.json` of the data
directory, by world, with the line as the name of its dye (as the saved data
`minehouse-tweaks:metro_stations` of the Fabric mod has it, so a world can be carried over).
A station that is broken is forgotten.

`content/rail_graph.rs` is the metro network as carts drove it (the Fabric `RailGraph`).
Its nodes are the rails on station blocks and the junctions: a plain rail on a junction
block with track coming in from three or four sides. Any minecart learns the run of rails
between two nodes it drives, both ways, through the `MinecartObserver` hook of `steel-core`
(`set_minecart_observer`), which hears of every cart at the end of its tick. The graph is
kept in `metro-rail-graph.json` of the data directory and written at most every five
seconds; breaking a station or junction block forgets the runs through its rail.

`content/ride.rs` is riding (the Fabric `Stations` and `Routes`). A
cart that reaches a station rail stops there, with the bell and the name of the station as
a title for its rider. Where the network knows another station from there, the cart is
docked: pulled to the middle of the rail, deaf to pushes, until its rider sends it on with
`/metro <station>` (bare `/metro` lists the stations in reach; the start of a name is
enough) or drives off by pressing forward. A sent cart goes past every other station, the
junctions on its way are turned for it four blocks ahead (only the shape, with no neighbour
updates), and it plans again from where it is when it ends up off the way. Where no other
station is known yet, a cart with a rider counts down five seconds and leaves the way it
was going, or back at the end of the track, so that the line gets learned; an unpowered
powered rail on the station keeps it there. An empty cart never leaves by itself. The
observer also gets `before_tick` from `steel-core` for this.

`content/trains.rs` is trains (the Fabric `Trains` and `Consist`). Sneak and use a chain on
one cart, then on another within four blocks: they are coupled, up to twelve in a line,
never a ring. Sneak and use shears to uncouple a cart; the chains drop, as they do when a
cart is destroyed or a coupling is stretched past six blocks. Only the cart in front has
physics. The others are put on the way it went, a block and a half apart, so a train does
not stretch or derail cart by cart. A push on any cart goes to the whole train, and a train
pushed or sent backwards turns around: its rear cart drives. At a station the front cart
is the one that stops on the rail, everyone aboard sees the title, and `/metro` works from
any cart. Carts of one train go through each other, and so do carts on rails that drive at
each other or of which one carries a player; a cart or train with a player also goes
through mobs and players. Coupled carts roll like ridden ones. The couplings are kept in
`metro-couplings.json` of the data directory by the id of the cart (`content/couplings.rs`,
the Fabric `CartLinks`). `steel-core` asks the observer for all of this: `interact`,
`passes_through`, `follows`, `rolls_freely` and `removed`.

`content/speed.rs` is the speed (the Fabric `MinecartSpeed`). A cart with a player, or a
train with a player in any of its carts, goes up to 80 blocks a second on straight flat
track; an empty cart is the vanilla one. The server moves such a cart several times a tick,
each time less than a block, so it reads every rail it passes: it turns in curves, climbs
slopes, runs activator rails, stops at its station and is followed by the rail graph. On
straight flat track it does not lose speed to rolling, so one stretch of powered rails
carries it a long way, and every powered rail adds 6 blocks a second. It stops moving this
tick where the chunks ahead are not ticking. `steel-core` asks the observer for `max_speed`
and gives it `drive`, the move along the track to make as many times as it wants.

`content/station_menu.rs` is the station map (the Fabric `StationMenu`, with `MapView` in
`content/map_view.rs` and `Transfers` in `content/transfers.rs`). A rider whose cart (or
the front cart of their train) stands on a station sees the network known from there
hanging in front of them: the lines as strips of the line's color, the stations as dots,
forks where the color changes as white rings. The station looked at is signed, a click
sends the cart there (`/metro` still works), a held right button drags the map, and the
wheel zooms it; the hotbar slot the wheel moved is given back when the map closes. The map
is display entities sent to that player only, with one interaction entity over it for the
clicks; nothing is added to the world. `steel-core` has `PlayerInputObserver`
(`player/input_observer.rs`) for this: `interact` and `attack` for entity ids the server
does not know, and `carried_item` for the wheel.

A cart with a player in it, or any cart of a train that carries one, knocks empty plain
minecarts off the line as it passes (`trains::smash_empty`, the Fabric `Trains.smashEmpty`):
the cart left on the rails breaks into its item instead of stopping the ride. Carts of the
same train, carts with a passenger and chest, hopper and other carts are left alone.

`content/map_markers.rs` puts the metro on the web map and the site (the Fabric
`BlueMapMarkers`). It writes the stations, the lines between them and the transfers as the
`BlueMap` marker set `minehouse-metro`, in the files `BlueMap` serves:
`maps/<map>/live/markers.json` (`world`, `world_the_nether`, `world_the_end`) and, if the
directory has none of its own, a `settings.json` that lists the maps. The directory is
`MINEHOUSE_METRO_MAP_DIR`, or `metro-map` in the data directory; point the site's
`BLUEMAP_URL` at a web server that gives it out. Other marker sets in a `markers.json` are
kept. Other modules put sets of their own through `map_markers::put_extra` (the spawn zones do). The files are written by a thread of their own, at most every two seconds after a
station or the track changed. A line is the track the rail graph knows between two
stations, through the forks on the way (the Fabric mod draws the trips its trains
recorded).

Silent name tags (`content/silent_tags.rs`, the Fabric `SilentTags`): a name tag whose name
has one of the words «тихо», «тихий», «тихая», `silent`, `shh`, «шшш» as a whole word
("Бобик [тихо]") makes the mob silent with the vanilla Silent flag; any other tag gives its
voice back. Only what a tag did is undone (the entity tag `minehouse_silenced`). `steel-core`
has `NameTagObserver` (`behavior/items/name_tag.rs`) for this. The words are fixed here; the
Fabric mod reads them from its config.

Name tags on minecarts (`content/minecart_names.rs`, the Fabric `MinecartNames`): a named
name tag names any minecart with a right click, and the tag is used up (not in creative, and
not when the cart already has that name). The name shows while a player points at the cart.
A silent word in it makes the cart silent. Not ported: a cart placed from a minecart item
renamed "Тихо" on an anvil is not made silent.

Waxed and invisible item frames (`content/waxed_frames.rs`, the Fabric `WaxedFrames`):
honeycomb waxes an item frame (sneak to put the honeycomb in instead) and a sneaking axe
scrapes the wax off. A waxed frame is vanilla's fixed frame: its item cannot be turned or
taken and the frame cannot be broken; a right click on it goes to the block it hangs on, so
a frame on a chest opens the chest. Sneaking shears hide or show a frame that holds an
item, and a hidden frame shows again once its item is out. `steel-core` has
`ItemFrameObserver` (`display_ui/item_frame.rs`) and `player::game_mode::use_block` for this.

Waxed maps and smithing templates (`content/waxed_items.rs`, the Fabric `MapLock` and
`TemplateLock`): on the cartography table a filled map with honeycomb in the lower slot
gives the same map, waxed; on the smithing table a template with honeycomb in the middle
slot gives the same template, waxed (recipes `minehouse-tweaks:waxed_<template>`). A waxed
map is not copied, neither with an empty map on the cartography table nor in the crafting
grid, and a waxed template is in no shaped recipe, so it is not duplicated with diamonds.
Both still work as before: the map updates, zooms out and locks, the template trims and
upgrades. The mark is `custom_data.minehouse_waxed`, as in the Fabric mod, plus a lore
line. The core hook is `inventory::craft_observer::CraftObserver`.

Merging experience orbs (`content/xp_orbs.rs`, the Fabric `XpOrbs`): every 10 ticks an
orb folds the orbs within 2 blocks, of any value, into itself and becomes a single orb
worth all of them, so the pile of a farm is picked up in one touch instead of one orb
every 2 ticks. The total never changes; a sum above 32767 (an orb saves a short) is not
merged. The core hook is `ExperienceOrbObserver`, called from the orb's tick.

A newcomer's fireworks (`content/first_join.rs`, the Fabric `FirstJoin`): on a player's
very first join (their stats have never counted a `leave_game`) everyone gets one line in
chat, and 40 ticks later four waves of six rockets, 15 ticks apart, go up in a ring of 3
blocks around the newcomer. The rockets hurt no one (`FireworkRocketEntity::set_harmless`).
It runs as a job of the server tick (`server.jobs`), which also notices who came online.

Nether portals of any shape (`content/any_shape_portals.rs`, the Fabric `AnyShapePortals`):
fire inside a closed upright obsidian frame of any outline (a circle, a heart, an arch),
up to 21 x 21, fills the whole opening with portal. A portal block stands while each
neighbor in its plane is obsidian or portal, so a hole in the frame puts the portal out
block by block. Rectangles, linking and travel stay vanilla. The core asks through
`steel_core::PortalShapeObserver` only after vanilla's own rectangle search found nothing.

The AFK mark (`content/afk.rs`, the Fabric `AfkDisplay`): a player with no action for 5
minutes is put into the scoreboard team `minehouse_afk`, whose prefix «[AFK]» the client
draws over the head; on return they get back the team they were in. Checked every 10 ticks
by a job of the server tick from `Player::idle_time`. A marked player who is not in bed
does not count towards `playersSleepingPercentage` (`steel_core::world::SleepObserver`).
`MINEHOUSE_AFK_SECONDS` shortens the wait for tests on a local server.

Names over players' heads (`content/nametags.rs`, the Fabric `Nametags`): each player
chooses whether they see them. A player who has not chosen gets a vanilla dialog 40 ticks
after joining; `/nametags` toggles, `/nametags show|hide` sets, `/nametags menu` opens the
dialog again. For a player who hides names every team packet goes out with
`nametagVisibility=never` (`steel_core::scoreboard::TeamPacketObserver`; the scoreboard
itself does not change), and players in no team are put, on that client only, into the
private team `!minehouse_names`. The choices are in `nametags.properties` of the data
directory, in the format of the Fabric `minehouse-nametags.properties`. The dialog needs
the play `CShowDialog` and the click of its buttons, `PlayerInputObserver::custom_click`.

Fast leaf decay (`content/leaf_decay.rs`, the Fabric `LeafDecay`): leaves that find no log
on their scheduled tick wait 4 to 14 ticks and fall on the next one, if they still have
none, so a felled tree comes apart in a second or two. Persistent leaves and leaves by a
log are vanilla. The core hook is `behavior::blocks::vegetation::LeafDecayObserver`.

Mending an anvil (`content/anvil_repair.rs`, the Fabric `AnvilRepair`): a click on a chipped
or damaged anvil with a block of iron in hand undoes one step of damage and takes the block,
keeping the facing. The core hook is `PlayerInputObserver::use_on_block`, asked before the
block and the item.

Washing in a cauldron (`content/cauldron_conversion.rs`, the Fabric `CauldronConversion`): a
stack thrown into a cauldron of water changes all at once. Dirt, coarse dirt and rooted dirt
become mud and take no water; concrete powder becomes concrete of its color and takes one
level of water for a stack. The core hook is `ItemEntityObserver`, asked at the end of an
item's tick.

The closed End (`content/end_lock.rs`, the Fabric `EndLock`): with `closed = true` under
`[end]` in `config/minehouse/tweaks.toml` nobody can put an eye of ender into a portal frame,
and a player who still ends up in the End is sent out within a second, to where the exit
portal leads (the Fabric mod used the player's own respawn point). A spectator with
`minehouse.end.bypass` may stay. `message` is what the player is told. The setting is read at
start. The core gets `Player::leave_end`.

The whole recipe book (`content/all_recipes.rs`, the Fabric `RecipeBookAll`): on joining a
player's book is filled with every vanilla recipe a book can show, quietly, and sent again.
Recipes of ours still unlock the vanilla way.

Admin mode (`content/admin_mode.rs`, the Fabric `AdminMode`): `/adminmode on
[creative|spectator]` puts the admin's inventory, ender chest, experience, effects, health,
food, game mode and place aside and leaves a clean creative (or spectator) player;
`/adminmode off` gives it all back and returns them; the bare command toggles. The backup
is a whole player data file, `adminmode/<uuid>.dat` in the data directory, written before
anything is cleared; the file being there is what "in admin mode" means, so it survives a
restart, and the player is reminded on joining. Used backups move to `adminmode/history/`
(the last five per player stay) and every enter and exit goes to `adminmode/log.txt`. Needs
`minehouse.adminmode`; whoever is in admin mode may always leave. A death in admin mode
drops nothing. The player storage is not saved at once on exit: until the next autosave a
crash leaves the admin player in the save and the survival one in `history/`.

Other players' items (`content/see.rs`, the rest of the Fabric `Invsee`; the inventory of
a player who is online is the core `/invsee`): `/endersee <nick>` opens an ender chest,
live if the player is online, and `/offlinesee <nick>` the inventory, armor and offhand of
a player who is offline. For an offline player the stored data is read once and every
change is written back at once; admins on the same player share one copy, and the windows
close when the player logs in. Every open and every changed slot goes to `invsee/log.txt`
in the data directory and to the server log. Both need `minehouse.invsee`. Changes made
through the core `/invsee` are not logged.

The magic herb: the wild plant (`wild_magic_herb`, no item, drops 2–3 seeds), the seeds
(«Семена магического растения») and the crop they sow on farmland (`magic_herb`, four
stages, half as fast as a vanilla crop, in bloom drops 1–2 seeds and 1–2 leaves). The crop
is a `steel_core` `CropLike`, so light, farmland, trampling and bone meal are the vanilla
ones. The wild plant is not generated in the world yet: on the Fabric server it grows only
in the biomes of Paradise, which is not ported.

The brew (`content/cauldron.rs`): three buckets of milk into a cauldron, then ten magic
leaves with a lit campfire under it make five bowls of «Магический отвар». The cauldrons of
milk and broth are blocks of ours that need no pack: clients see powder snow and water
cauldrons (`blocks::show_as`). A bucket of milk is the one vanilla item that behaves
differently in `content::use_behavior`: on an empty cauldron it is poured. «Магическая
папироса» (`content/papirosa.rs`) is two leaves and paper, lit with a flint and steel in
the other hand. A consumed item of ours says how in `CustomItem::consume`; what it does
when finished is its `ItemBehavior::finish_using`, which a player's active use now takes
from the stack (`use_behavior_of`) and not from the vanilla item.

To a recipe an item of ours is not the vanilla item it is made of: `content::recipe_id`
tells them apart (`steel_registry::recipe::set_extension_item_id`), so vanilla ingredients
turn our items down (no book out of magic leaves, no bed out of clouds), and a recipe of
ours asks for one with `CustomItem::ingredient`. The client's recipe book still shows the
vanilla item. This covers recipes only: a furnace, a composter or a villager still sees
the vanilla item.
