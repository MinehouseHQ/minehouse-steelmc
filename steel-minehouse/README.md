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
kept. The files are written by a thread of their own, at most every two seconds after a
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
