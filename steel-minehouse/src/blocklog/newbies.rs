//! Newbie protection (the Fabric `Newbies` and `Property` of `MineHouse` Tweaks), our answer to
//! an open server without a whitelist: while a player has played less than `hours`, they may
//! not break blocks other players placed, open their containers, or use lava, fire and TNT.
//! An owner lets a friend in with `/trust <nick>`; an admin lifts it for good with
//! `/newbie release <nick>`. Settings in `config/minehouse/newbies.toml`.
//!
//! Who placed a block comes from the block log: the newest place or break at the spot. Lookups
//! run on the log's reader thread, so around every newbie online the owners of a whole cube
//! are fetched ahead ([`Prefetch`]); a spot outside every fetched cube is looked up alone and
//! refused until the answer is in. What players place and break while the server runs is
//! kept at once ([`Newbies::placed`]), since the log writes with a delay.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::{Arc, OnceLock},
};

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use steel_core::{
    behavior::blocks::{FireBlock, connected_direction},
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, CommandSource, literal,
    },
    entity::Entity as _,
    inventory::lock::ContainerRef,
    permission::{PermissionExpr, PermissionKey},
    player::Player,
    server::jobs::{JobPoll, ServerJob, ServerJobContext},
    world::World,
};
use steel_registry::{
    blocks::{
        block_state_ext::BlockStateExt as _,
        properties::{BlockStateProperties, ChestType},
    },
    item_stack::ItemStack,
    stat::vanilla_stat_types,
    vanilla_block_tags::BlockTag,
    vanilla_blocks, vanilla_custom_stats, vanilla_items,
};
use steel_utils::{BlockPos, Direction, Identifier, locks::SyncMutex, types::InteractionHand};

use super::store::{Actions, BREAK, Event, PLACE, Query, Reader, Store};
use crate::{
    config,
    moderation::{
        commands::{player as player_argument, word},
        format::render,
    },
};

/// Ticks in an hour.
const HOUR_TICKS: i64 = 72_000;
/// Cubes are `CUBE` wide and fetched with `MARGIN` around them.
const CUBE: i32 = 16;
const MARGIN: i32 = 16;
/// How far from a newbie the fetched owners must reach.
const REACH: i32 = 8;
const REGION_TTL_MS: i64 = 10 * 60_000;
const SPOT_TTL_MS: i64 = 5 * 60_000;
const RECENT_MS: i64 = 15 * 60_000;
const NOTICE_MS: i64 = 3_000;
/// Rows per page when fetching the owners of a region.
const REGION_PAGE: u32 = 100_000;
const MANAGE: &str = "minehouse.newbie.manage";
const BYPASS: &str = "minehouse.newbie.bypass";
const STATE_FILE: &str = "newbies.json";

/// `config/minehouse/newbies.toml`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NewbiesConfig {
    /// `false`: nobody is a newbie; `/trust` still works.
    pub enabled: bool,
    /// How long a newbie stays one, in hours of play.
    pub hours: u32,
    /// Newbies may not use lava buckets, flint and steel, fire charges and TNT.
    pub no_lava_fire_tnt: bool,
}

impl Default for NewbiesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hours: 6,
            no_lava_fire_tnt: true,
        }
    }
}

/// What is kept between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// Owner (lower case) → players (lower case) allowed to touch their blocks.
    trust: BTreeMap<String, BTreeSet<String>>,
    /// Players (lower case) released from the protection → who released them.
    released: BTreeMap<String, String>,
}

type Spot = (String, (i32, i32, i32));

/// Who owns a spot ("" = nobody), and when it was learned.
#[derive(Clone)]
struct Owner {
    player: String,
    at: i64,
}

/// Owners in a box, fetched at once.
struct Region {
    world: String,
    min: (i32, i32, i32),
    max: (i32, i32, i32),
    owners: FxHashMap<(i32, i32, i32), String>,
    at: i64,
}

impl Region {
    fn holds(&self, world: &str, (x, y, z): (i32, i32, i32), margin: i32) -> bool {
        self.world == world
            && x - margin >= self.min.0
            && x + margin <= self.max.0
            && y - margin >= self.min.1
            && y + margin <= self.max.1
            && z - margin >= self.min.2
            && z + margin <= self.max.2
    }
}

#[derive(Default)]
struct Known {
    regions: Vec<Region>,
    spots: FxHashMap<Spot, Owner>,
    recent: FxHashMap<Spot, Owner>,
    pending: FxHashSet<Spot>,
}

/// The newbie protection.
pub struct Newbies {
    config: NewbiesConfig,
    state: SyncMutex<State>,
    known: SyncMutex<Known>,
    noticed: SyncMutex<FxHashMap<uuid::Uuid, i64>>,
}

static NEWBIES: OnceLock<Arc<Newbies>> = OnceLock::new();

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn permission(key: &str) -> Option<PermissionExpr> {
    PermissionKey::parse(key).ok().map(PermissionExpr::key)
}

fn has(player: &Player, key: &str) -> bool {
    permission(key).is_some_and(|permission| player.has_permission(&permission))
}

fn lower(name: &str) -> String {
    name.to_lowercase()
}

const fn place(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x(), pos.y(), pos.z())
}

/// The owners in rows of the log, newest first: the newest place or break of a spot decides,
/// and a block placed by a player (not by a `#cause`, not rolled back) is theirs.
fn owners_of(rows: &[Event]) -> FxHashMap<(i32, i32, i32), String> {
    let mut seen = FxHashSet::default();
    let mut owners = FxHashMap::default();
    add_owners(rows, &mut seen, &mut owners);
    owners
}

/// [`owners_of`] for one page of many: `seen` carries the spots older pages already decided.
fn add_owners(
    rows: &[Event],
    seen: &mut FxHashSet<(i32, i32, i32)>,
    owners: &mut FxHashMap<(i32, i32, i32), String>,
) {
    for row in rows {
        if (row.action != PLACE && row.action != BREAK) || !seen.insert(row.pos) {
            continue;
        }
        if row.action == PLACE && !row.rolled_back && !row.actor.name.starts_with('#') {
            owners.insert(row.pos, row.actor.name.clone());
        }
    }
}

/// The spots whose owners decide over the container at `pos`: both halves of a double chest
/// (a newbie could otherwise put their own chest next to someone else's and open the pair through
/// their half), the spot alone for anything else.
fn container_spots(world: &World, pos: BlockPos) -> Vec<BlockPos> {
    let state = world.get_block_state(pos);
    if state
        .try_get_value(&BlockStateProperties::CHEST_TYPE)
        .is_some_and(|kind| kind != ChestType::Single)
    {
        let other = pos.relative(connected_direction(state));
        if world
            .get_block_state(other)
            .try_get_value(&BlockStateProperties::CHEST_TYPE)
            .is_some_and(|kind| kind != ChestType::Single)
        {
            return vec![pos, other];
        }
    }
    vec![pos]
}

/// Where a hopper or a hopper minecart used on the `face` of the block at `pos` would sit, when
/// there is a container right above it to pull from.
fn drains_from(
    stack: &ItemStack,
    world: &World,
    pos: BlockPos,
    face: Direction,
) -> Option<BlockPos> {
    let spot = if stack.is(&vanilla_items::HOPPER_MINECART) {
        // Minecarts go onto the rail that was clicked.
        if !world
            .get_block_state(pos)
            .get_block()
            .has_tag(&BlockTag::RAILS)
        {
            return None;
        }
        pos
    } else if stack.is(&vanilla_items::HOPPER) {
        if world.get_block_state(pos).is_replaceable() {
            pos
        } else {
            pos.relative(face)
        }
    } else {
        return None;
    };
    world
        .get_block_entity(spot.above())
        .and_then(ContainerRef::from_block_entity)
        .map(|_| spot)
}

fn is_hazard(stack: &ItemStack) -> bool {
    [
        &vanilla_items::LAVA_BUCKET,
        &vanilla_items::FLINT_AND_STEEL,
        &vanilla_items::FIRE_CHARGE,
        &vanilla_items::TNT,
        &vanilla_items::TNT_MINECART,
    ]
    .into_iter()
    .any(|item| stack.is(item))
}

/// Whether flint and steel or a fire charge used on the `face` of the block at `pos` only
/// lights a Nether portal: the block is obsidian and the fire would land in an empty cell
/// of a closed frame (vanilla or any shape), where it turns into portal at once.
fn lights_portal(stack: &ItemStack, world: &Arc<World>, pos: BlockPos, face: Direction) -> bool {
    if !(stack.is(&vanilla_items::FLINT_AND_STEEL) || stack.is(&vanilla_items::FIRE_CHARGE))
        || world.get_block_state(pos).get_block() != &vanilla_blocks::OBSIDIAN
    {
        return false;
    }
    let fire = pos.relative(face);
    world.get_block_state(fire).is_air() && FireBlock::is_portal(world, fire, face)
}

impl Newbies {
    /// Loads the settings and the state and adds `/trust`, `/untrust` and `/newbie`.
    pub(super) fn register(registry: &mut CommandRegistry) -> Option<Arc<Self>> {
        let config: NewbiesConfig = match config::load(&config::path("newbies")) {
            Ok(config) => config,
            Err(error) => {
                log::error!("MineHouse newbie protection is off: {error}");
                return None;
            }
        };
        let state = fs::read_to_string(config::data_path(STATE_FILE))
            .ok()
            .and_then(|text| match serde_json::from_str(&text) {
                Ok(state) => Some(state),
                Err(error) => {
                    log::error!("MineHouse newbies: bad {STATE_FILE}: {error}");
                    None
                }
            })
            .unwrap_or_default();
        let newbies = Arc::new(Self {
            config,
            state: SyncMutex::new(state),
            known: SyncMutex::new(Known::default()),
            noticed: SyncMutex::new(FxHashMap::default()),
        });
        let _ = NEWBIES.set(Arc::clone(&newbies));
        if let Err(error) = register_commands(registry) {
            log::error!("MineHouse newbies: commands are not registered: {error:?}");
        }
        log::info!(
            "MineHouse newbie protection {} ({} h)",
            if newbies.config.enabled { "on" } else { "off" },
            newbies.config.hours
        );
        Some(newbies)
    }

    fn save(&self) {
        let state = self.state.lock().clone();
        let text = match serde_json::to_string_pretty(&state) {
            Ok(text) => text,
            Err(error) => {
                log::error!("MineHouse newbies: {error}");
                return;
            }
        };
        let file = config::data_path(STATE_FILE);
        let tmp = file.with_extension("json.tmp");
        let written = file
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&tmp, text))
            .and_then(|()| fs::rename(&tmp, &file));
        if let Err(error) = written {
            log::error!(
                "MineHouse newbies: could not save {}: {error}",
                file.display()
            );
        }
    }

    fn played_ticks(player: &Player) -> i64 {
        i64::from(player.stat(vanilla_stat_types::CUSTOM.get(&vanilla_custom_stats::PLAY_TIME)))
    }

    /// Whether `player` is still a newbie.
    #[must_use]
    pub fn is_newbie(&self, player: &Player) -> bool {
        self.config.enabled
            && self.config.hours > 0
            && Self::played_ticks(player) < i64::from(self.config.hours) * HOUR_TICKS
            && !self
                .state
                .lock()
                .released
                .contains_key(&lower(player.gameprofile.name()))
            && !has(player, BYPASS)
    }

    fn time_left(&self, player: &Player) -> String {
        let ticks = i64::from(self.config.hours) * HOUR_TICKS - Self::played_ticks(player);
        let minutes = ((ticks + 1_199) / 1_200).max(1);
        if minutes >= 60 {
            format!("{} ч {} мин", minutes / 60, minutes % 60)
        } else {
            format!("{minutes} мин")
        }
    }

    fn trusts(&self, owner: &str, player: &str) -> bool {
        self.state
            .lock()
            .trust
            .get(&lower(owner))
            .is_some_and(|trusted| trusted.contains(&lower(player)))
    }

    /// A player placed a block: it is theirs from now on.
    pub(super) fn placed(&self, player: &Player, world: &World, pos: BlockPos) {
        self.known.lock().recent.insert(
            (world.key.to_string(), place(pos)),
            Owner {
                player: player.gameprofile.name().to_owned(),
                at: now_ms(),
            },
        );
    }

    /// A player broke a block: the spot is nobody's now.
    pub(super) fn broken(&self, world: &World, pos: BlockPos) {
        self.known.lock().recent.insert(
            (world.key.to_string(), place(pos)),
            Owner {
                player: String::new(),
                at: now_ms(),
            },
        );
    }

    /// Who owns the spot, or `None` while it is being looked up.
    fn owner(self: &Arc<Self>, store: &Store, world: &str, at: (i32, i32, i32)) -> Option<String> {
        let now = now_ms();
        let spot = (world.to_owned(), at);
        let mut known = self.known.lock();
        if let Some(owner) = known.recent.get(&spot)
            && now - owner.at < RECENT_MS
        {
            return Some(owner.player.clone());
        }
        if let Some(region) = known
            .regions
            .iter()
            .find(|region| now - region.at < REGION_TTL_MS && region.holds(world, at, 0))
        {
            return Some(region.owners.get(&at).cloned().unwrap_or_default());
        }
        if let Some(owner) = known.spots.get(&spot)
            && now - owner.at < SPOT_TTL_MS
        {
            return Some(owner.player.clone());
        }
        if known.pending.insert(spot.clone()) {
            drop(known);
            let newbies = Arc::clone(self);
            store.lookup(
                Query {
                    world: Some(world.to_owned()),
                    around: Some((at, 0)),
                    // Only places and breaks: a much-used chest has more clicks than a page holds.
                    blocks: Some(Actions::PlaceOrBreak),
                    limit: 1,
                    ..Query::default()
                },
                move |answer| {
                    let owners = answer.map_or_else(
                        |error| {
                            log::warn!("Newbie protection lookup failed: {error}");
                            FxHashMap::default()
                        },
                        |page| owners_of(&page.rows),
                    );
                    let mut known = newbies.known.lock();
                    known.pending.remove(&spot);
                    known.spots.insert(
                        spot.clone(),
                        Owner {
                            player: owners.get(&spot.1).cloned().unwrap_or_default(),
                            at: now_ms(),
                        },
                    );
                },
            );
        }
        None
    }

    /// Fetches the owners around `at` unless they are already fetched or on their way.
    fn fetch_region(self: &Arc<Self>, store: &Store, world: &str, at: (i32, i32, i32)) {
        let now = now_ms();
        let center = (
            at.0.div_euclid(CUBE) * CUBE + CUBE / 2,
            at.1.div_euclid(CUBE) * CUBE + CUBE / 2,
            at.2.div_euclid(CUBE) * CUBE + CUBE / 2,
        );
        let key = (format!("region:{world}"), center);
        {
            let mut known = self.known.lock();
            if known
                .regions
                .iter()
                .any(|region| now - region.at < REGION_TTL_MS && region.holds(world, at, REACH))
                || !known.pending.insert(key.clone())
            {
                return;
            }
        }
        Arc::clone(self).fetch_region_page(
            store.reader(),
            key,
            world.to_owned(),
            center,
            0,
            FxHashSet::default(),
            FxHashMap::default(),
        );
    }

    /// One page of [`Self::fetch_region`], newest first; asks for the next until one comes back
    /// short. One capped page would drop the oldest rows, and with them who placed the chests of
    /// a busy base long ago.
    #[expect(clippy::too_many_arguments, reason = "the state of one paged lookup")]
    fn fetch_region_page(
        self: Arc<Self>,
        store: Reader,
        key: Spot,
        world: String,
        center: (i32, i32, i32),
        offset: u32,
        mut seen: FxHashSet<(i32, i32, i32)>,
        mut owners: FxHashMap<(i32, i32, i32), String>,
    ) {
        let radius = CUBE / 2 + MARGIN;
        let next = store.clone();
        store.lookup(
            Query {
                world: Some(world.clone()),
                around: Some((center, radius)),
                blocks: Some(Actions::PlaceOrBreak),
                offset,
                limit: REGION_PAGE,
                ..Query::default()
            },
            move |answer| match answer {
                Ok(page) => {
                    add_owners(&page.rows, &mut seen, &mut owners);
                    if page.rows.len() >= REGION_PAGE as usize {
                        self.fetch_region_page(
                            next,
                            key,
                            world,
                            center,
                            offset + REGION_PAGE,
                            seen,
                            owners,
                        );
                        return;
                    }
                    let mut known = self.known.lock();
                    known.pending.remove(&key);
                    known.regions.push(Region {
                        world,
                        min: (center.0 - radius, center.1 - radius, center.2 - radius),
                        max: (center.0 + radius, center.1 + radius, center.2 + radius),
                        owners,
                        at: now_ms(),
                    });
                }
                Err(error) => {
                    self.known.lock().pending.remove(&key);
                    log::warn!("Newbie protection lookup failed: {error}");
                }
            },
        );
    }

    fn forget_old(&self) {
        let now = now_ms();
        let mut known = self.known.lock();
        known
            .regions
            .retain(|region| now - region.at < REGION_TTL_MS);
        known.spots.retain(|_, owner| now - owner.at < SPOT_TTL_MS);
        known.recent.retain(|_, owner| now - owner.at < RECENT_MS);
    }

    /// Whether to say something to `player` now, at most once every few seconds.
    fn may_notice(&self, player: &Player) -> bool {
        let now = now_ms();
        let mut noticed = self.noticed.lock();
        let last = noticed.entry(player.gameprofile.id()).or_insert(0);
        if now - *last < NOTICE_MS {
            return false;
        }
        *last = now;
        true
    }

    /// A newbie hit or used a block: whether to refuse it.
    pub(super) fn keeps_from(
        self: &Arc<Self>,
        store: &Store,
        player: &Arc<Player>,
        world: &Arc<World>,
        pos: BlockPos,
        hit: bool,
    ) -> bool {
        // Only containers and other blocks with a menu: doors, buttons and beds stay usable.
        if !hit
            && world
                .get_block_entity(pos)
                .and_then(ContainerRef::from_block_entity)
                .is_none()
        {
            return false;
        }
        if !self.is_newbie(player) {
            return false;
        }
        // A double chest opens both halves: each must be the player's to open.
        let spots = if hit {
            vec![pos]
        } else {
            container_spots(world, pos)
        };
        self.keeps_from_spots(store, player, world, &spots, hit)
    }

    /// A newbie is about to place a hopper or a hopper minecart: whether it would sit under
    /// someone else's container and empty it.
    pub(super) fn keeps_hopper_from(
        self: &Arc<Self>,
        store: &Store,
        player: &Arc<Player>,
        hand: InteractionHand,
        on: Option<(&Arc<World>, BlockPos, Direction)>,
    ) -> bool {
        let Some((world, pos, face)) = on else {
            return false;
        };
        let spot = {
            let inventory = player.inventory.lock();
            drains_from(inventory.get_item_in_hand(hand), world, pos, face)
        };
        let Some(spot) = spot else {
            return false;
        };
        if !self.is_newbie(player) {
            return false;
        }
        self.keeps_from_spots(
            store,
            player,
            world,
            &container_spots(world, spot.above()),
            false,
        )
    }

    /// Whether any of `spots` is someone else's (or still being looked up); tells the player whose.
    fn keeps_from_spots(
        self: &Arc<Self>,
        store: &Store,
        player: &Arc<Player>,
        world: &Arc<World>,
        spots: &[BlockPos],
        hit: bool,
    ) -> bool {
        let name = player.gameprofile.name();
        let key = world.key.to_string();
        let mut waiting = false;
        let mut foreign = None;
        for &spot in spots {
            match self.owner(store, &key, place(spot)) {
                // Being looked up: the answer is in before the next click.
                None => waiting = true,
                Some(owner) => {
                    if !(owner.is_empty()
                        || owner.eq_ignore_ascii_case(name)
                        || self.trusts(&owner, name))
                    {
                        foreign.get_or_insert(owner);
                    }
                }
            }
        }
        let Some(owner) = foreign else {
            return waiting;
        };
        if self.may_notice(player) {
            player.send_action_bar(render(
                &format!(
                    "&eЭто поставил {owner}. Новички {} ч не могут {} чужое (тебе осталось {}). Пустить тебя может хозяин: /trust {name}",
                    self.config.hours,
                    if hit { "ломать" } else { "открывать" },
                    self.time_left(player)
                ),
                &[],
            ));
        }
        true
    }

    /// A newbie is about to use an item: whether it is lava, fire or TNT, which they may not.
    /// Fire that only lights a Nether portal is allowed.
    pub(super) fn keeps_from_item(
        &self,
        player: &Arc<Player>,
        hand: InteractionHand,
        on: Option<(&Arc<World>, BlockPos, Direction)>,
    ) -> bool {
        if !self.config.no_lava_fire_tnt {
            return false;
        }
        {
            let inventory = player.inventory.lock();
            let stack = inventory.get_item_in_hand(hand);
            if !is_hazard(stack)
                || on.is_some_and(|(world, pos, face)| lights_portal(stack, world, pos, face))
            {
                return false;
            }
        }
        if !self.is_newbie(player) {
            return false;
        }
        if self.may_notice(player) {
            player.send_action_bar(render(
                &format!(
                    "&eЛаву, огонь и ТНТ можно будет использовать через {} игры. Так мы бережём постройки от гриферов.",
                    self.time_left(player)
                ),
                &[],
            ));
        }
        true
    }
}

/// Once a second: fetches the owners around every newbie who has none fetched yet.
pub(super) struct Prefetch {
    pub(super) newbies: Arc<Newbies>,
    pub(super) log: Arc<super::BlockLog>,
}

impl ServerJob for Prefetch {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        if !context.tick_count.is_multiple_of(20) {
            return JobPoll::Pending;
        }
        self.newbies.forget_old();
        for player in server.get_players() {
            if !self.newbies.is_newbie(&player) {
                continue;
            }
            let world = player.get_world().key.to_string();
            let pos = player.position();
            let at = (
                pos.x.floor() as i32,
                pos.y.floor() as i32,
                pos.z.floor() as i32,
            );
            self.newbies.fetch_region(&self.log.store, &world, at);
        }
        JobPoll::Pending
    }
}

// --- Commands --------------------------------------------------------------------------------

fn newbies() -> Result<&'static Arc<Newbies>, CommandError> {
    NEWBIES
        .get()
        .ok_or_else(|| CommandError::from("Защита новичков выключена"))
}

fn player_of(context: CommandContext<'_>) -> Result<Arc<Player>, CommandError> {
    context
        .source()
        .player()
        .cloned()
        .ok_or_else(|| CommandError::from("Только для игроков"))
}

fn may_manage(source: CommandSource<'_>) -> bool {
    source.player().is_none_or(|player| has(player, MANAGE))
}

fn register_commands(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "trust"), trust_tree)
            .default_access(),
    )?;
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "untrust"), untrust_tree)
            .default_access(),
    )?;
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "newbie"), newbie_tree)
            .default_access(),
    )?;
    Ok(())
}

fn trust_tree() -> CommandNode {
    literal("trust")
        .executes(|context| list_trusted(*context))
        .then(player_argument().executes(|context| set_trust(*context, true)))
}

fn untrust_tree() -> CommandNode {
    literal("untrust").then(player_argument().executes(|context| set_trust(*context, false)))
}

fn newbie_tree() -> CommandNode {
    literal("newbie")
        .executes(|context| about_self(*context))
        .then(
            literal("release")
                .requires(may_manage)
                .then(player_argument().executes(|context| release(*context, true))),
        )
        .then(
            literal("restrict")
                .requires(may_manage)
                .then(player_argument().executes(|context| release(*context, false))),
        )
        .then(
            literal("list")
                .requires(may_manage)
                .executes(|context| list_released(*context)),
        )
}

fn set_trust(context: CommandContext<'_>, trust: bool) -> Result<i32, CommandError> {
    let newbies = newbies()?;
    let player = player_of(context)?;
    let nick = lower(&word(context, "player")?);
    let owner = lower(player.gameprofile.name());
    if nick == owner {
        return Err(CommandError::from("Свои постройки ты и так можешь трогать"));
    }
    {
        let mut state = newbies.state.lock();
        if trust {
            state.trust.entry(owner).or_default().insert(nick.clone());
        } else if !state
            .trust
            .get_mut(&owner)
            .is_some_and(|trusted| trusted.remove(&nick))
        {
            return Err(CommandError::from(
                format!("{nick} и так не в твоём списке").as_str(),
            ));
        }
        state.trust.retain(|_, trusted| !trusted.is_empty());
    }
    newbies.save();
    let text = if trust {
        format!(
            "&a{nick} теперь может ломать и открывать то, что ты построил. &7Отменить: /untrust {nick}"
        )
    } else {
        format!("&7{nick} больше не может трогать твои постройки")
    };
    context.source().send_success(&render(&text, &[]), false);
    if trust
        && let Some(target) = context
            .source()
            .server()
            .get_players()
            .into_iter()
            .find(|online| online.gameprofile.name().eq_ignore_ascii_case(&nick))
    {
        target.send_message(&render(
            &format!(
                "&a{} пустил тебя к своим постройкам и сундукам.",
                player.gameprofile.name()
            ),
            &[],
        ));
    }
    Ok(1)
}

fn list_trusted(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let newbies = newbies()?;
    let player = player_of(context)?;
    let trusted = newbies
        .state
        .lock()
        .trust
        .get(&lower(player.gameprofile.name()))
        .map_or_else(String::new, |trusted| {
            trusted.iter().cloned().collect::<Vec<_>>().join(", ")
        });
    let text = if trusted.is_empty() {
        "&7Ты никого не пускал к своим постройкам. Пустить друга: &f/trust <ник>".to_owned()
    } else {
        format!("&7К твоим постройкам пущены: &f{trusted}&7. Убрать: &f/untrust <ник>")
    };
    context.source().send_success(&render(&text, &[]), false);
    Ok(1)
}

fn about_self(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let newbies = newbies()?;
    let player = player_of(context)?;
    let text = if newbies.is_newbie(&player) {
        format!(
            "&eТы новичок ещё {} игры: пока нельзя ломать и открывать чужое{}. &7Друг может пустить тебя к своим постройкам командой &f/trust {}",
            newbies.time_left(&player),
            if newbies.config.no_lava_fire_tnt {
                ", а также ставить лаву, огонь и ТНТ"
            } else {
                ""
            },
            player.gameprofile.name()
        )
    } else {
        "&aОграничений новичка у тебя нет.".to_owned()
    };
    context.source().send_success(&render(&text, &[]), false);
    Ok(1)
}

fn release(context: CommandContext<'_>, release: bool) -> Result<i32, CommandError> {
    let newbies = newbies()?;
    let nick = lower(&word(context, "player")?);
    let by = context.source().player().map_or_else(
        || "console".to_owned(),
        |player| player.gameprofile.name().to_owned(),
    );
    {
        let mut state = newbies.state.lock();
        if release {
            state.released.insert(nick.clone(), by.clone());
        } else if state.released.remove(&nick).is_none() {
            return Err(CommandError::from(
                format!("{nick} и так не освобождён").as_str(),
            ));
        }
    }
    newbies.save();
    log::info!(
        "Newbie protection: {by} {} {nick}",
        if release { "released" } else { "restricted" }
    );
    let text = if release {
        format!("&a{nick} больше не новичок: может ломать и открывать всё, как остальные")
    } else {
        format!(
            "&e{nick} снова новичок, пока не наиграет {} ч",
            newbies.config.hours
        )
    };
    context.source().send_success(&render(&text, &[]), true);
    if release
        && let Some(target) = context
            .source()
            .server()
            .get_players()
            .into_iter()
            .find(|online| online.gameprofile.name().eq_ignore_ascii_case(&nick))
    {
        target.send_message(&render("&aС тебя сняли ограничения новичка.", &[]));
    }
    Ok(1)
}

fn list_released(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let newbies = newbies()?;
    let released = newbies
        .state
        .lock()
        .released
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let text = if released.is_empty() {
        "&7Никто не освобождён вручную".to_owned()
    } else {
        format!("&7Освобождены вручную: &f{released}")
    };
    context.source().send_success(&render(&text, &[]), false);
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklog::store::{Actor, Subject};

    fn event(name: &str, pos: (i32, i32, i32), action: u8, rolled_back: bool) -> Event {
        Event {
            time: 0,
            actor: Actor {
                name: name.to_owned(),
                uuid: None,
            },
            world: "minecraft:overworld".to_owned(),
            pos,
            kind: "minecraft:stone".to_owned(),
            action,
            rolled_back,
            subject: Subject::Block {
                state: String::new(),
            },
        }
    }

    #[test]
    fn the_newest_place_or_break_decides_who_owns_a_spot() {
        // Newest first.
        let rows = [
            event("Bob", (0, 0, 0), BREAK, false),
            event("Alice", (0, 0, 0), PLACE, false),
            event("Alice", (1, 0, 0), PLACE, false),
            event("Carol", (2, 0, 0), PLACE, true),
            event("#show", (3, 0, 0), PLACE, false),
            event("Dave", (1, 0, 0), BREAK, false),
        ];
        let owners = owners_of(&rows);
        assert_eq!(owners.get(&(0, 0, 0)), None);
        assert_eq!(owners.get(&(1, 0, 0)).map(String::as_str), Some("Alice"));
        assert_eq!(owners.get(&(2, 0, 0)), None);
        assert_eq!(owners.get(&(3, 0, 0)), None);
    }

    #[test]
    fn older_pages_do_not_undo_what_newer_pages_decided() {
        let mut seen = FxHashSet::default();
        let mut owners = FxHashMap::default();
        // Page one, newest: Bob broke (0,0,0). Page two, older: Alice placed both spots.
        add_owners(
            &[event("Bob", (0, 0, 0), BREAK, false)],
            &mut seen,
            &mut owners,
        );
        add_owners(
            &[
                event("Alice", (0, 0, 0), PLACE, false),
                event("Alice", (5, 0, 0), PLACE, false),
            ],
            &mut seen,
            &mut owners,
        );
        assert_eq!(owners.get(&(0, 0, 0)), None);
        assert_eq!(owners.get(&(5, 0, 0)).map(String::as_str), Some("Alice"));
    }

    #[test]
    fn a_region_holds_what_is_inside_with_the_margin() {
        let region = Region {
            world: "w".to_owned(),
            min: (0, 0, 0),
            max: (40, 40, 40),
            owners: FxHashMap::default(),
            at: 0,
        };
        assert!(region.holds("w", (20, 20, 20), 8));
        assert!(!region.holds("w", (35, 20, 20), 8));
        assert!(!region.holds("x", (20, 20, 20), 0));
    }
}
