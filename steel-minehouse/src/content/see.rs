//! `/endersee <nick>` and `/offlinesee <nick>` (the rest of the Fabric `Invsee`; the core
//! `/invsee` already shows the inventory of a player who is online).
//!
//! `/endersee` opens a player's ender chest as a chest window, live if they are online.
//! For a player who is offline, it and `/offlinesee` (inventory with armor and offhand)
//! read the stored player data once, and every change is written back right away, so when
//! the player logs in they get exactly what the admin left. Admins looking at the same
//! offline player share one copy. When that player logs in the windows on them close;
//! when an online player leaves, the window on their ender chest closes.
//!
//! Every open and every changed slot goes to `invsee/log.txt` in the data directory and to
//! the server log. Needs `minehouse.invsee`.

use std::{
    fs,
    io::Write as _,
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use rustc_hash::FxHashMap;
use steel_core::{
    command::{
        CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
        CommandRegistrationError, CommandRegistry, CommandSource, DetachedCommandSource,
        SuspendedCommand, SuspendedCommandPoll, argument, literal,
    },
    inventory::prelude::*,
    permission::{PermissionExpr, PermissionKey},
    player::{
        connection::NetworkConnection as _,
        player_data::{PersistentPlayerData, PersistentSlot, next_capture_order},
        player_inventory::PlayerInventory,
    },
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_registry::vanilla_menu_types;
use steel_utils::{DowncastType, DowncastTypeKey, Identifier, locks::SyncMutex};
use tokio::{
    runtime::Handle,
    sync::oneshot::{self, error::TryRecvError},
    task::AbortHandle,
};
use uuid::Uuid;

use crate::{config::data_path, moderation::format::render};

const PERMISSION: &str = "minehouse.invsee";
const LOG_FILE: &str = "invsee/log.txt";
const ENDER_SLOTS: usize = 27;
/// Slots of the offline inventory window that hold nothing (it is a 9x5 chest).
const FILLER_SLOTS: usize = 4;
/// Every how many ticks offline copies are checked against who is online.
const CHECK_EVERY_TICKS: u64 = 5;

/// What a window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Inventory,
    EnderChest,
}

impl View {
    const fn what(self) -> &'static str {
        match self {
            Self::Inventory => "inventory",
            Self::EnderChest => "ender chest",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Inventory => "Инвентарь",
            Self::EnderChest => "Эндер-сундук",
        }
    }
}

/// An offline player being edited, shared by every window on them.
struct Offline {
    id: Uuid,
    name: String,
    domain: String,
    /// The stored data as it was read; the items are replaced on every save.
    data: SyncMutex<PersistentPlayerData>,
    inventory: Shared<SimpleContainer>,
    ender_chest: Shared<SimpleContainer>,
    /// The player logged in: their data was read again, this copy must not be written.
    closed: AtomicBool,
}

/// Offline players with open windows.
static OFFLINE: SyncMutex<Option<FxHashMap<Uuid, Weak<Offline>>>> = SyncMutex::new(None);
/// The Tokio runtime the player storage is used on.
static RUNTIME: OnceLock<Handle> = OnceLock::new();

fn stored(slots: &[PersistentSlot], size: usize) -> SimpleContainer {
    let mut container = SimpleContainer::new(size);
    for slot in slots {
        if let Ok(index) = usize::try_from(slot.slot)
            && index < size
        {
            container.set_item(index, slot.item.clone());
        }
    }
    container
}

fn slots_of(container: &dyn Container) -> Vec<PersistentSlot> {
    container
        .items()
        .iter()
        .enumerate()
        .filter(|(_, item)| !item.is_empty())
        .filter_map(|(slot, item)| {
            Some(PersistentSlot {
                slot: i8::try_from(slot).ok()?,
                item: item.clone(),
            })
        })
        .collect()
}

impl Offline {
    fn new(id: Uuid, name: String, domain: String, data: PersistentPlayerData) -> Self {
        Self {
            id,
            name,
            domain,
            inventory: stored(&data.inventory, PlayerInventory::CONTAINER_SIZE).into_shared(),
            ender_chest: stored(&data.ender_items, ENDER_SLOTS).into_shared(),
            data: SyncMutex::new(data),
            closed: AtomicBool::new(false),
        }
    }

    /// Writes the items into the stored player data, the rest of it as it was.
    fn save(&self, server: &Arc<Server>) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        let Some(runtime) = RUNTIME.get() else {
            return;
        };
        let data = {
            let mut data = self.data.lock();
            data.inventory = slots_of(&*self.inventory.lock());
            data.ender_items = slots_of(&*self.ender_chest.lock());
            data.capture_order = next_capture_order();
            data.clone()
        };
        let (server, domain, id, name) = (
            Arc::clone(server),
            self.domain.clone(),
            self.id,
            self.name.clone(),
        );
        runtime.spawn(async move {
            if let Err(error) = server
                .player_data_storage
                .save_domain_data(&domain, id, &data)
                .await
            {
                log::error!("MineHouse invsee: could not save the player data of {name}: {error}");
            }
        });
    }
}

fn session(id: Uuid) -> Option<Arc<Offline>> {
    OFFLINE
        .lock()
        .as_ref()
        .and_then(|sessions| sessions.get(&id))
        .and_then(Weak::upgrade)
}

/// The shared copy of an offline player, made from `data` unless there is one already.
fn session_for(id: Uuid, name: &str, domain: &str, data: PersistentPlayerData) -> Arc<Offline> {
    let mut guard = OFFLINE.lock();
    let sessions = guard.get_or_insert_with(FxHashMap::default);
    if let Some(open) = sessions.get(&id).and_then(Weak::upgrade) {
        return open;
    }
    let made = Arc::new(Offline::new(id, name.to_owned(), domain.to_owned(), data));
    sessions.insert(id, Arc::downgrade(&made));
    made
}

/// Closes the copies of players who logged in. A job of the server tick, never finished.
struct Watch;

impl ServerJob for Watch {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        if !context.tick_count.is_multiple_of(CHECK_EVERY_TICKS) {
            return JobPoll::Pending;
        }
        let mut guard = OFFLINE.lock();
        let Some(sessions) = guard.as_mut() else {
            return JobPoll::Pending;
        };
        sessions.retain(|_, session| session.strong_count() > 0);
        if sessions.is_empty() {
            return JobPoll::Pending;
        }
        for player in server.get_players() {
            if let Some(session) = sessions
                .remove(&player.gameprofile.id())
                .and_then(|session| session.upgrade())
            {
                session.closed.store(true, Ordering::Relaxed);
            }
        }
        JobPoll::Pending
    }
}

fn log(what: &str) {
    log::info!("MineHouse invsee: {what}");
    let file = data_path(LOG_FILE);
    let line = format!("{} {what}\n", chrono::Utc::now().to_rfc3339());
    let written = file
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file)?
                .write_all(line.as_bytes())
        });
    if let Err(error) = written {
        log::warn!("MineHouse invsee: could not append to {LOG_FILE}: {error}");
    }
}

fn describe(stack: &ItemStack) -> String {
    if stack.is_empty() {
        "empty".to_owned()
    } else {
        format!("{} {}", stack.count(), stack.item().key)
    }
}

fn slot_name(view: View, slot: usize) -> String {
    let note = match (view, slot) {
        (View::Inventory, 0..=8) => "hotbar",
        (View::Inventory, 36) => "feet",
        (View::Inventory, 37) => "legs",
        (View::Inventory, 38) => "chest",
        (View::Inventory, 39) => "head",
        (View::Inventory, PlayerInventory::SLOT_OFFHAND) => "offhand",
        (View::EnderChest | View::Inventory, _) => return slot.to_string(),
    };
    format!("{slot} ({note})")
}

/// The slots that differ between `before` and `after`, as lines for the log.
fn changes(view: View, before: &[ItemStack], after: &[ItemStack]) -> Vec<String> {
    before
        .iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (before, after))| !ItemStack::matches(before, after))
        .map(|(slot, (before, after))| {
            format!(
                "slot {}: {} -> {}",
                slot_name(view, slot),
                describe(before),
                describe(after)
            )
        })
        .collect()
}

/// Whose items a window shows.
enum Target {
    Online(Weak<Player>),
    Offline(Arc<Offline>),
}

struct SeeKind {
    view: View,
    target: Target,
    /// The admin and the target, as the log names them.
    admin: String,
    whom: String,
    watched: ContainerId,
    before: Option<Vec<ItemStack>>,
}

// SAFETY: This key is used for `SeeKind` alone.
unsafe impl DowncastType for SeeKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("minehouse:menu/see");
}

impl SeeKind {
    fn snapshot(&mut self, guard: &ContainerLockGuard) {
        if let Some(container) = guard.get(self.watched) {
            self.before = Some(container.items().to_vec());
        }
    }
}

impl MenuKind for SeeKind {
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _click: Click,
        _player: &Player,
    ) -> ClickOutcome {
        self.snapshot(guard);
        ClickOutcome::Fallthrough
    }

    fn on_drag(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _action: QuickCraft,
        _player: &Player,
    ) -> ClickOutcome {
        self.snapshot(guard);
        ClickOutcome::Fallthrough
    }

    fn slots_changed(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        let Some(before) = self.before.take() else {
            return;
        };
        let Some(container) = guard.get(self.watched) else {
            return;
        };
        let changed = changes(self.view, &before, container.items());
        if changed.is_empty() {
            return;
        }
        let offline = matches!(self.target, Target::Offline(_));
        for change in changed {
            log(&format!(
                "{} changed {} of {}{} {change}",
                self.admin,
                self.view.what(),
                self.whom,
                if offline { " [offline]" } else { "" }
            ));
        }
        if let Target::Offline(session) = &self.target
            && let Some(server) = player.try_server()
        {
            // The lock held here is the one `save` takes, so it runs once this click is over.
            let session = Arc::clone(session);
            server.jobs.spawn(SaveOnce {
                session: Some(session),
            });
        }
    }

    fn still_valid(&self, _behavior: &MenuBehavior, _player: &Player) -> bool {
        match &self.target {
            Target::Online(target) => target
                .upgrade()
                .is_some_and(|target| !target.connection.closed()),
            Target::Offline(session) => !session.closed.load(Ordering::Relaxed),
        }
    }
}

/// Saves an offline copy on the next server tick, outside the click that changed it.
struct SaveOnce {
    session: Option<Arc<Offline>>,
}

impl ServerJob for SaveOnce {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        if let (Some(session), Some(server)) = (self.session.take(), context.server()) {
            session.save(&server);
        }
        JobPoll::Finished
    }
}

const fn armor_slot(index: usize) -> Option<EquipmentSlot> {
    match index {
        36 => Some(EquipmentSlot::Feet),
        37 => Some(EquipmentSlot::Legs),
        38 => Some(EquipmentSlot::Chest),
        39 => Some(EquipmentSlot::Head),
        _ => None,
    }
}

fn names(admin: &Player) -> String {
    format!("{} ({})", admin.gameprofile.name(), admin.gameprofile.id())
}

/// A chest window over an ender chest, of a player or of an offline copy.
fn ender_menu(
    container_id: u8,
    admin: &Arc<Player>,
    (chest, watched): (ContainerRef, ContainerId),
    target: Target,
    whom: String,
) -> Menu {
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    let shown = builder.section_all(chest);
    let own = builder.player_inventory(&admin.inventory);
    builder.route(shown, own.all(), FillDirection::Backward);
    builder.route(own.all(), shown, FillDirection::Forward);
    builder.build(SeeKind {
        view: View::EnderChest,
        target,
        admin: names(admin),
        whom,
        watched,
        before: None,
    })
}

/// The inventory of an offline copy, laid out like the core `/invsee`: main, hotbar, armor
/// from the head down, offhand, and four slots that hold nothing.
fn inventory_menu(container_id: u8, admin: &Arc<Player>, session: Arc<Offline>) -> Menu {
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X5, container_id);
    let stored = ContainerRef::from(&session.inventory);
    let watched = ContainerId::from_arc(&session.inventory);
    let main = builder.section_at(&stored, PlayerInventory::MAIN, SectionKind::Normal);
    let hotbar = builder.section_at(&stored, PlayerInventory::HOTBAR, SectionKind::Normal);
    let armor = builder.section_at(
        &stored,
        PlayerInventory::ARMOR_TOP_DOWN,
        SectionKind::restricted(|index, item| {
            armor_slot(index).is_some_and(|slot| item.is_equippable_in_slot(slot))
        }),
    );
    let offhand = builder.section_at(
        &stored,
        [PlayerInventory::SLOT_OFFHAND],
        SectionKind::Normal,
    );
    builder.section_all_with(
        SimpleContainer::new(FILLER_SLOTS).into_shared(),
        SectionKind::Display,
    );
    let own = builder.player_inventory(&admin.inventory);
    builder.route(
        [main, hotbar, armor, offhand],
        own.all(),
        FillDirection::Backward,
    );
    builder.route(
        own.all(),
        [main, hotbar, armor, offhand],
        FillDirection::Forward,
    );
    let whom = format!("{} ({})", session.name, session.id);
    builder.build(SeeKind {
        view: View::Inventory,
        target: Target::Offline(session),
        admin: names(admin),
        whom,
        watched,
        before: None,
    })
}

fn open_offline(admin: &Arc<Player>, session: Arc<Offline>, view: View) {
    let title = format!("{} {} (не в сети)", view.title(), session.name);
    let whom = format!("{} ({})", session.name, session.id);
    log(&format!(
        "{} opened {} of {whom} [offline]",
        names(admin),
        view.what()
    ));
    let opener = Arc::clone(admin);
    admin.open_menu(render(&title, &[]), move |context| match view {
        View::Inventory => inventory_menu(context.container_id, &opener, session),
        View::EnderChest => {
            let chest = (
                ContainerRef::from(&session.ender_chest),
                ContainerId::from_arc(&session.ender_chest),
            );
            ender_menu(
                context.container_id,
                &opener,
                chest,
                Target::Offline(session),
                whom,
            )
        }
    });
}

fn open_online(admin: &Arc<Player>, target: &Arc<Player>) {
    let whom = names(target);
    log(&format!("{} opened ender chest of {whom}", names(admin)));
    let title = format!("{} {}", View::EnderChest.title(), target.gameprofile.name());
    let (opener, shown) = (Arc::clone(admin), Arc::clone(target));
    admin.open_menu(render(&title, &[]), move |context| {
        let chest = (
            ContainerRef::from(&shown.ender_chest_inventory),
            ContainerId::from_arc(&shown.ender_chest_inventory),
        );
        ender_menu(
            context.container_id,
            &opener,
            chest,
            Target::Online(Arc::downgrade(&shown)),
            whom,
        )
    });
}

/// What the storage answered: the stored data of the player, if there is any.
type Loaded = Result<Option<PersistentPlayerData>, String>;

/// A command waiting for the stored data of an offline player.
struct Load {
    source: DetachedCommandSource,
    answer: oneshot::Receiver<Loaded>,
    task: AbortHandle,
    view: View,
    id: Uuid,
    name: String,
    domain: String,
}

impl Load {
    fn finish(&self, loaded: Loaded) -> Result<i32, CommandError> {
        let source = self.source.source();
        let admin = source
            .player()
            .ok_or_else(|| CommandError::from("Только для игроков"))?;
        let data = loaded
            .map_err(|error| {
                log::error!(
                    "MineHouse invsee: could not read the player data of {}: {error}",
                    self.name
                );
                CommandError::from(format!(
                    "Не удалось прочитать данные игрока {}. Подробности в логе сервера.",
                    self.name
                ))
            })?
            .ok_or_else(|| {
                CommandError::from(format!(
                    "У игрока {} нет сохранённых данных на этом сервере.",
                    self.name
                ))
            })?;
        if online(source.server(), &self.name).is_some() {
            return Err(CommandError::from(format!(
                "{} уже в сети, открой ещё раз.",
                self.name
            )));
        }
        let session = session_for(self.id, &self.name, &self.domain, data);
        open_offline(admin, session, self.view);
        Ok(1)
    }
}

impl SuspendedCommand for Load {
    fn poll(&mut self) -> SuspendedCommandPoll {
        match self.answer.try_recv() {
            Ok(loaded) => SuspendedCommandPoll::Ready(self.finish(loaded)),
            Err(TryRecvError::Empty) => SuspendedCommandPoll::Pending,
            Err(TryRecvError::Closed) => SuspendedCommandPoll::Ready(Err(CommandError::from(
                "Команда прервана, попробуй ещё раз.",
            ))),
        }
    }

    fn cancel(&mut self) {
        self.task.abort();
    }
}

fn online(server: &Server, name: &str) -> Option<Arc<Player>> {
    server
        .get_players()
        .into_iter()
        .find(|player| player.gameprofile.name().eq_ignore_ascii_case(name))
}

/// Starts reading the stored data of `name`, or answers at once from an open copy.
fn load(source: CommandSource<'_>, name: &str, view: View) -> Result<Load, CommandError> {
    let admin = source
        .player()
        .ok_or_else(|| CommandError::from("Только для игроков"))?;
    let server = Arc::clone(source.server());
    let known = server.known_players();
    let Some(profile) = known.by_name(name) else {
        return Err(CommandError::from(format!(
            "Игрок {name} ни разу не заходил на сервер."
        )));
    };
    let (id, name) = (profile.uuid(), profile.last_known_name().to_owned());
    let domain = admin.get_world().domain().to_owned();
    let runtime = RUNTIME
        .get()
        .ok_or_else(|| CommandError::from("Данные игроков сейчас недоступны."))?;

    let (sender, answer) = oneshot::channel();
    let open = session(id).map(|session| session.data.lock().clone());
    let stored_in = domain.clone();
    let task = runtime.spawn(async move {
        let loaded = match open {
            Some(data) => Ok(Some(data)),
            None => server
                .player_data_storage
                .load_domain(&stored_in, id)
                .await
                .map_err(|error| error.to_string()),
        };
        // The command was cancelled: nobody waits for the answer.
        let _ = sender.send(loaded);
    });
    Ok(Load {
        source: source.detach(),
        answer,
        task: task.abort_handle(),
        view,
        id,
        name,
        domain,
    })
}

fn endersee(context: CommandContext<'_>) -> Result<Load, CommandError> {
    let name = context.string("player")?;
    let source = context.source();
    load(source, name, View::EnderChest)
}

/// `/endersee` on a player who is online needs no storage: it opens at once.
fn endersee_online(context: CommandContext<'_>) -> Option<Result<i32, CommandError>> {
    let name = context.string("player").ok()?;
    let source = context.source();
    let target = online(source.server(), name)?;
    Some(
        source
            .player()
            .ok_or_else(|| CommandError::from("Только для игроков"))
            .map(|admin| {
                open_online(admin, &target);
                1
            }),
    )
}

/// A command that is already answered.
struct Done(Option<Result<i32, CommandError>>);

impl SuspendedCommand for Done {
    fn poll(&mut self) -> SuspendedCommandPoll {
        SuspendedCommandPoll::Ready(
            self.0
                .take()
                .unwrap_or_else(|| Err(CommandError::from("Команда уже выполнена."))),
        )
    }

    fn cancel(&mut self) {}
}

/// Either kind of answer of the two commands.
enum Answer {
    Done(Done),
    Load(Box<Load>),
}

impl SuspendedCommand for Answer {
    fn poll(&mut self) -> SuspendedCommandPoll {
        match self {
            Self::Done(done) => done.poll(),
            Self::Load(load) => load.poll(),
        }
    }

    fn cancel(&mut self) {
        match self {
            Self::Done(done) => done.cancel(),
            Self::Load(load) => load.cancel(),
        }
    }
}

fn endersee_tree() -> CommandNode {
    literal("endersee").then(
        argument("player", CommandArgument::word()).executes_suspended(|context| {
            Ok(match endersee_online(*context) {
                Some(done) => Answer::Done(Done(Some(done))),
                None => Answer::Load(Box::new(endersee(*context)?)),
            })
        }),
    )
}

fn offlinesee_tree() -> CommandNode {
    literal("offlinesee").then(
        argument("player", CommandArgument::word()).executes_suspended(|context| {
            let name = context.string("player")?;
            let source = context.source();
            if online(source.server(), name).is_some() {
                return Err(CommandError::from(format!(
                    "{name} в сети: открой /invsee {name}."
                )));
            }
            load(source, name, View::Inventory)
        }),
    )
}

/// Starts the watch of offline copies.
pub(super) fn start(server: &Server) {
    server.jobs.spawn(Watch);
}

/// Registers the commands. Call inside the Tokio runtime.
pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    match Handle::try_current() {
        Ok(runtime) => drop(RUNTIME.set(runtime)),
        Err(error) => {
            log::warn!("MineHouse invsee: no Tokio runtime, offline players are off: {error}");
        }
    }
    let Some(permission) = PermissionKey::parse(PERMISSION)
        .ok()
        .map(PermissionExpr::key)
    else {
        log::warn!("MineHouse invsee: bad permission {PERMISSION}, the commands are off");
        return Ok(());
    };
    registry
        .register(
            CommandRegistration::new(Identifier::new("minehouse", "endersee"), endersee_tree)
                .permission(permission.clone()),
        )?
        .register(
            CommandRegistration::new(Identifier::new("minehouse", "offlinesee"), offlinesee_tree)
                .permission(permission),
        )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_items};

    use super::*;

    fn stack(count: i32) -> ItemStack {
        ItemStack::with_count(&vanilla_items::DIAMOND, count)
    }

    #[test]
    fn stored_slots_fill_a_container_and_come_back() {
        init_vanilla_registry();
        let slots = vec![
            PersistentSlot {
                slot: 0,
                item: stack(3),
            },
            PersistentSlot {
                slot: 40,
                item: stack(1),
            },
            PersistentSlot {
                slot: 99,
                item: stack(7),
            },
            PersistentSlot {
                slot: -1,
                item: stack(7),
            },
        ];
        let container = stored(&slots, PlayerInventory::CONTAINER_SIZE);
        assert_eq!(container.get_item(0).count(), 3);
        assert_eq!(container.get_item(40).count(), 1);

        let back = slots_of(&container);
        let listed: Vec<(i8, i32)> = back
            .iter()
            .map(|slot| (slot.slot, slot.item.count()))
            .collect();
        assert_eq!(listed, vec![(0, 3), (40, 1)]);
    }

    #[test]
    fn changed_slots_are_described_for_the_log() {
        init_vanilla_registry();
        let before = vec![stack(3), ItemStack::empty(), stack(5)];
        let after = vec![stack(3), stack(2), ItemStack::empty()];
        assert_eq!(
            changes(View::Inventory, &before, &after),
            vec![
                "slot 1 (hotbar): empty -> 2 minecraft:diamond",
                "slot 2 (hotbar): 5 minecraft:diamond -> empty",
            ]
        );
        assert_eq!(
            changes(View::Inventory, &before, &before),
            Vec::<String>::new()
        );
    }

    #[test]
    fn slots_are_named_by_what_they_are() {
        assert_eq!(slot_name(View::Inventory, 8), "8 (hotbar)");
        assert_eq!(slot_name(View::Inventory, 9), "9");
        assert_eq!(slot_name(View::Inventory, 39), "39 (head)");
        assert_eq!(slot_name(View::Inventory, 40), "40 (offhand)");
        assert_eq!(slot_name(View::EnderChest, 3), "3");
        assert_eq!(armor_slot(36), Some(EquipmentSlot::Feet));
        assert_eq!(armor_slot(40), None);
    }
}
