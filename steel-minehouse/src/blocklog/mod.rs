//! The block log: who broke and placed which block, and who took what out of which chest.
//! Our replacement for `CoreProtect`, with its history brought over (see [`import`]).
//!
//! The server tick only queues events ([`steel_core::server::world_log::WorldLog`]); a writer
//! thread puts them on disk and a reader thread answers lookups, so neither a busy moment nor
//! a wide search holds the tick.

use std::{
    mem,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use rustc_hash::FxHashMap;
use simdnbt::owned::NbtTag;
use steel_core::{
    behavior::blocks::connected_direction,
    command::CommandRegistry,
    inventory::{lock::ContainerRef, menu::Menu},
    player::Player,
    server::{Server, world_log::WorldLog},
    world::World,
};
use steel_registry::{
    REGISTRY,
    blocks::{
        block_state_ext::BlockStateExt as _,
        properties::{BlockStateProperties, ChestType},
    },
    item_stack::ItemStack,
    vanilla_blocks,
};
use steel_utils::{BlockPos, BlockStateId, Direction, locks::SyncMutex, types::InteractionHand};
use text_components::TextComponent;
use uuid::Uuid;

mod commands;
mod format;
pub mod import;
mod magnifier;
mod params;
mod rollback;
mod settings;
pub mod store;

pub use settings::BlockLogConfig;
use store::{Actions, Actor, BREAK, Event, PLACE, PUT, Query, Store, Subject, TAKE};

use magnifier::Glance;

use crate::{config, content::MAGNIFIER, moderation::format::render};

/// What the log remembers about one asker between commands.
#[derive(Default)]
struct Session {
    /// Clicks show the history of the block instead of acting on it.
    inspecting: bool,
    /// The last lookup, for `/co page`.
    last: Option<Lookup>,
    /// What the magnifier showed last, and when it was last clicked.
    glance: Option<Glance>,
    clicked: i64,
    /// The block containers of the open menu, as they were when it opened.
    open: Vec<(BlockPos, Vec<ItemStack>)>,
}

/// A lookup with how its answer is shown.
#[derive(Clone)]
struct Lookup {
    title: String,
    query: Query,
    /// Whether rows carry their coordinates.
    with_place: bool,
}

/// The running block log.
pub struct BlockLog {
    config: BlockLogConfig,
    store: Store,
    sessions: SyncMutex<FxHashMap<Uuid, Session>>,
    importing: AtomicBool,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// `facing=north,lit=false`; empty for a block without properties.
fn state_text(state: BlockStateId) -> String {
    REGISTRY
        .blocks
        .properties(state)
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn actor(player: &Player) -> Actor {
    Actor {
        name: player.gameprofile.name().to_owned(),
        uuid: Some(player.gameprofile.id().to_string()),
    }
}

const fn place(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x(), pos.y(), pos.z())
}

/// Stacks counted by kind: equal items with equal components are one entry.
fn tally(items: &[ItemStack]) -> Vec<(ItemStack, i32)> {
    let mut counted: Vec<(ItemStack, i32)> = Vec::new();
    for stack in items.iter().filter(|stack| !stack.is_empty()) {
        match counted
            .iter_mut()
            .find(|(known, _)| ItemStack::is_same_item_same_components(known, stack))
        {
            Some((_, count)) => *count += stack.count(),
            None => counted.push((stack.clone(), stack.count())),
        }
    }
    counted
}

/// What went in (positive) and out (negative) between two views of a container.
fn changes(before: &[ItemStack], after: &[ItemStack]) -> Vec<(ItemStack, i32)> {
    let mut delta = tally(after);
    for (stack, count) in tally(before) {
        match delta
            .iter_mut()
            .find(|(known, _)| ItemStack::is_same_item_same_components(known, &stack))
        {
            Some((_, now)) => *now -= count,
            None => delta.push((stack, -count)),
        }
    }
    delta.retain(|(_, count)| *count != 0);
    delta
}

/// The stack as NBT, when it is more than a plain item.
fn stack_data(stack: &ItemStack) -> Option<Vec<u8>> {
    if stack.components_patch().is_empty() {
        return None;
    }
    let NbtTag::Compound(compound) = stack.to_nbt_tag_ref() else {
        return None;
    };
    let mut bytes = Vec::new();
    compound.write(&mut bytes);
    Some(bytes)
}

impl BlockLog {
    /// Loads the settings, opens the log and adds `/co`. `None` when the log is off.
    #[must_use]
    pub fn register(registry: &mut CommandRegistry) -> Option<Arc<Self>> {
        let config: BlockLogConfig = match config::load(&config::path("blocklog")) {
            Ok(config) => config,
            Err(error) => {
                log::error!("MineHouse block log is off: {error}");
                return None;
            }
        };
        if !config.enabled {
            log::info!("MineHouse block log is off in config/minehouse/blocklog.toml");
            return None;
        }
        let file = config::data_path("blocklog.db");
        let store = match Store::open(&file, config.keep_ms()) {
            Ok(store) => store,
            Err(error) => {
                log::error!("MineHouse block log is off: {}: {error}", file.display());
                return None;
            }
        };
        let log = Arc::new(Self {
            config,
            store,
            sessions: SyncMutex::new(FxHashMap::default()),
            importing: AtomicBool::new(false),
        });
        if let Err(error) = commands::register(registry, &log) {
            log::error!("MineHouse block log: /co is not registered: {error:?}");
        }
        Some(log)
    }

    /// Starts listening to the world.
    pub fn start(self: &Arc<Self>, server: &Arc<Server>) {
        if server.set_world_log(Arc::clone(self) as Arc<dyn WorldLog>) {
            log::info!(
                "MineHouse block log on (keeps {} days)",
                self.config.keep_days
            );
        } else {
            log::error!("Another world log is already installed: no block log");
        }
    }

    /// Writes what is queued and closes the log.
    pub fn stop(&self) {
        self.store.stop();
    }

    /// Turns the inspector of `player` on or off; says which it is now.
    fn toggle_inspector(&self, player: Uuid) -> bool {
        let mut sessions = self.sessions.lock();
        let session = sessions.entry(player).or_default();
        session.inspecting = !session.inspecting;
        session.inspecting
    }

    /// Runs `lookup` from row `offset` and gives the chat lines to `reply`, off the tick.
    fn ask(
        self: &Arc<Self>,
        asker: Uuid,
        lookup: Lookup,
        offset: u32,
        reply: impl FnOnce(Vec<TextComponent>) + Send + 'static,
    ) {
        let page_size = self.config.page_size.max(1);
        let mut query = lookup.query.clone();
        query.offset = offset;
        query.limit = page_size;
        let title = lookup.title.clone();
        let with_place = lookup.with_place;
        self.sessions.lock().entry(asker).or_default().last = Some(lookup);
        self.store.lookup(query, move |answer| {
            reply(match answer {
                Ok(page) => format::page(&title, &page, offset, page_size, now_ms(), with_place),
                Err(error) => {
                    log::warn!("Block log lookup failed: {error}");
                    vec![TextComponent::plain(format!("Журнал не ответил: {error}"))]
                }
            });
        });
    }
}

impl BlockLog {
    /// A click with the magnifier: the history of the block, or of what a container held.
    fn magnify(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        pos: BlockPos,
        face: Direction,
        hit: bool,
    ) {
        let now = now_ms();
        let container = !hit
            && world
                .get_block_entity(pos)
                .and_then(ContainerRef::from_block_entity)
                .is_some();
        // Using a block that holds nothing asks about the spot in front of the clicked face.
        let pos = if hit || container {
            pos
        } else {
            pos.relative(face)
        };
        let state = world.get_block_state(pos);
        let partner = (container
            && state.get_block() == &vanilla_blocks::CHEST
            && state.get_value(&BlockStateProperties::CHEST_TYPE) != ChestType::Single)
            .then(|| place(pos.relative(connected_direction(state))));
        let glance = {
            let mut sessions = self.sessions.lock();
            let session = sessions.entry(player.gameprofile.id()).or_default();
            if now - session.clicked < magnifier::COOLDOWN_MS {
                return;
            }
            session.clicked = now;
            let glance = Glance::next(
                session.glance.as_ref(),
                &world.key.to_string(),
                place(pos),
                container,
                now,
            );
            session.glance = Some(glance.clone());
            glance
        };
        let block = state.get_block().key.to_string();
        let player = Arc::clone(player);
        self.store
            .lookup(glance.query(partner), move |answer| match answer {
                Ok(page) => {
                    for line in magnifier::lines(&glance, &block, &page.rows, now_ms()) {
                        player.send_message(&line);
                    }
                }
                Err(error) => {
                    log::warn!("Magnifier lookup failed: {error}");
                    player.send_message(&render(
                        "&cЛупа: не удалось прочитать историю, попробуй ещё раз",
                        &[],
                    ));
                }
            });
    }
}

impl WorldLog for BlockLog {
    fn block_broken(
        &self,
        player: &Player,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) {
        self.store.record(Event {
            time: now_ms(),
            actor: actor(player),
            world: world.key.to_string(),
            pos: place(pos),
            kind: state.get_block().key.to_string(),
            action: BREAK,
            rolled_back: false,
            subject: Subject::Block {
                state: state_text(state),
            },
        });
    }

    fn block_placed(
        &self,
        player: &Player,
        world: &Arc<World>,
        pos: BlockPos,
        _replaced: BlockStateId,
        placed: BlockStateId,
    ) {
        self.store.record(Event {
            time: now_ms(),
            actor: actor(player),
            world: world.key.to_string(),
            pos: place(pos),
            kind: placed.get_block().key.to_string(),
            action: PLACE,
            rolled_back: false,
            subject: Subject::Block {
                state: state_text(placed),
            },
        });
    }

    fn block_clicked(
        &self,
        player: &Arc<Player>,
        world: &Arc<World>,
        pos: BlockPos,
        face: Direction,
        hit: bool,
    ) -> bool {
        let asker = player.gameprofile.id();
        if !self
            .sessions
            .lock()
            .get(&asker)
            .is_some_and(|session| session.inspecting)
        {
            let holds_magnifier = MAGNIFIER.is(player
                .inventory
                .lock()
                .get_item_in_hand(InteractionHand::MainHand));
            if holds_magnifier {
                self.magnify(player, world, pos, face, hit);
            }
            return holds_magnifier;
        }
        let (x, y, z) = place(pos);
        let lookup = Lookup {
            title: format!("{x} {y} {z}"),
            query: Query {
                world: Some(world.key.to_string()),
                around: Some(((x, y, z), 0)),
                // A hit asks about the block itself; a use also about what it holds.
                blocks: hit.then_some(Actions::Any),
                ..Query::default()
            },
            with_place: false,
        };
        let player = Arc::clone(player);
        // `self` is behind the `Arc` the server holds; the store only needs the reply.
        let page_size = self.config.page_size.max(1);
        let mut query = lookup.query.clone();
        query.limit = page_size;
        let title = lookup.title.clone();
        self.sessions.lock().entry(asker).or_default().last = Some(lookup);
        self.store.lookup(query, move |answer| match answer {
            Ok(page) => {
                for line in format::page(&title, &page, 0, page_size, now_ms(), false) {
                    player.send_message(&line);
                }
            }
            Err(error) => log::warn!("Block log lookup failed: {error}"),
        });
        true
    }

    fn menu_opened(&self, player: &Player, menu: &Menu) {
        let open = menu.block_containers();
        let mut sessions = self.sessions.lock();
        if open.is_empty() {
            // A view left over from a menu that never closed properly is not this menu's.
            if let Some(session) = sessions.get_mut(&player.gameprofile.id()) {
                session.open.clear();
            }
        } else {
            sessions.entry(player.gameprofile.id()).or_default().open = open;
        }
    }

    fn menu_closed(&self, player: &Player, menu: &Menu) {
        let before = match self.sessions.lock().get_mut(&player.gameprofile.id()) {
            Some(session) if !session.open.is_empty() => mem::take(&mut session.open),
            _ => return,
        };
        let after = menu.block_containers();
        let world = player.get_world().key.to_string();
        let time = now_ms();
        for (pos, was) in before {
            let Some((_, is)) = after.iter().find(|(place, _)| *place == pos) else {
                continue;
            };
            for (stack, count) in changes(&was, is) {
                self.store.record(Event {
                    time,
                    actor: actor(player),
                    world: world.clone(),
                    pos: place(pos),
                    kind: stack.item().key.to_string(),
                    action: if count > 0 { PUT } else { TAKE },
                    rolled_back: false,
                    subject: Subject::Items {
                        amount: count.abs(),
                        data: stack_data(&stack),
                    },
                });
            }
        }
    }
}

impl BlockLog {
    /// Whether an import is running; `true` claims the right to run one.
    fn claim_import(&self) -> bool {
        !self.importing.swap(true, Ordering::AcqRel)
    }

    fn release_import(&self) {
        self.importing.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_items};

    use super::*;

    #[test]
    fn container_changes_count_what_went_in_and_out() {
        init_vanilla_registry();
        let stack = |item, count| ItemStack::with_count(item, count);
        let before = [
            stack(&vanilla_items::DIAMOND, 10),
            stack(&vanilla_items::STONE, 64),
            ItemStack::empty(),
            stack(&vanilla_items::STONE, 6),
        ];
        // The same stone moved between slots; diamonds went out, dirt came in.
        let after = [
            stack(&vanilla_items::STONE, 70),
            stack(&vanilla_items::DIAMOND, 3),
            stack(&vanilla_items::DIRT, 2),
            ItemStack::empty(),
        ];
        let mut delta: Vec<(String, i32)> = changes(&before, &after)
            .into_iter()
            .map(|(stack, count)| (stack.item().key.to_string(), count))
            .collect();
        delta.sort();
        assert_eq!(
            delta,
            [
                ("minecraft:diamond".to_owned(), -7),
                ("minecraft:dirt".to_owned(), 2)
            ]
        );
        assert_eq!(changes(&before, &before).len(), 0);
    }
}
