//! Admin mode (the Fabric `AdminMode`). `/adminmode on [creative|spectator]` puts
//! everything the admin carries aside (inventory, ender chest, experience, effects, health,
//! food, game mode, place with its world) and leaves a clean creative or spectator player;
//! `/adminmode off` takes them back where they were and gives it all back. So admin work
//! never mixes with the admin's own survival game.
//!
//! The backup is a whole player data file, written to `adminmode/<uuid>.dat` in the data
//! directory before anything is cleared, so a crash or a restart in between loses nothing:
//! the file being there is what "in admin mode" means. On exit the file is moved to
//! `history/` (the last few per player are kept) instead of deleted, in case something goes
//! wrong after the restore. Every enter and exit is also appended to `log.txt` next to it.
//!
//! Needs `minehouse.adminmode`. Whoever is in admin mode can always leave it, even if they
//! lost the right meanwhile. Whatever was handed out in admin mode does not drop when the
//! admin dies.

use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
};

use glam::DVec3;
use rustc_hash::FxHashSet;
use steel_core::{
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, CommandSource, literal,
    },
    entity::{Entity as _, LivingEntity as _},
    inventory::container::Container as _,
    permission::{PermissionExpr, PermissionKey},
    player::{
        Player,
        death_observer::PlayerDeathObserver,
        experience::Experience,
        player_data::PersistentPlayerData,
        player_data_storage::{decode_player_data, encode_player_data},
        player_inventory::PlayerInventory,
    },
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
    world::World,
};
use steel_registry::item_stack::ItemStack;
use steel_utils::{Identifier, locks::SyncMutex, types::GameType};
use uuid::Uuid;

use crate::{config::data_path, moderation::format::render};

/// The directory with the backups, in the data directory.
const DIR: &str = "adminmode";
const HISTORY_DIR: &str = "history";
const LOG_FILE: &str = "log.txt";
const BACKUP_EXTENSION: &str = "dat";
/// How many used backups of one player are kept.
const HISTORY_PER_PLAYER: usize = 5;
/// Who may enter admin mode.
const PERMISSION: &str = "minehouse.adminmode";
/// Every how many ticks newly joined players are looked for.
const CHECK_EVERY_TICKS: u64 = 20;
const FULL_FOOD: i32 = 20;
const START_SATURATION: f32 = 5.0;

/// Players in admin mode: those with a backup file. Kept in memory so that the command
/// tree and a death do not ask the disk.
static ACTIVE: SyncMutex<Option<FxHashSet<Uuid>>> = SyncMutex::new(None);

fn is_active(id: Uuid) -> bool {
    ACTIVE.lock().as_ref().is_some_and(|ids| ids.contains(&id))
}

fn set_active(id: Uuid, active: bool) {
    let mut guard = ACTIVE.lock();
    let ids = guard.get_or_insert_with(FxHashSet::default);
    if active {
        ids.insert(id);
    } else {
        ids.remove(&id);
    }
}

fn dir() -> PathBuf {
    data_path(DIR)
}

fn backup_file(dir: &Path, id: Uuid) -> PathBuf {
    dir.join(format!("{id}.{BACKUP_EXTENSION}"))
}

/// The player a backup file name belongs to.
fn backup_owner(name: &str) -> Option<Uuid> {
    let id = name.strip_suffix(BACKUP_EXTENSION)?.strip_suffix('.')?;
    Uuid::from_str(id).ok()
}

/// The name a used backup gets in `history/`; `stamp` sorts by time.
fn history_name(id: Uuid, stamp: &str) -> String {
    format!("{id}-{stamp}.{BACKUP_EXTENSION}")
}

/// Which of the `history/` file `names` go, so that the newest `keep` of player `id` stay.
fn outdated(names: &[String], id: Uuid, keep: usize) -> Vec<String> {
    let prefix = format!("{id}-");
    let mut mine: Vec<&String> = names
        .iter()
        .filter(|name| name.starts_with(&prefix) && name.ends_with(BACKUP_EXTENSION))
        .collect();
    mine.sort();
    let extra = mine.len().saturating_sub(keep);
    mine.into_iter().take(extra).cloned().collect()
}

/// The mode's name in the log.
const fn mode_id(mode: GameType) -> &'static str {
    if matches!(mode, GameType::Spectator) {
        "spectator"
    } else {
        "creative"
    }
}

const fn mode_name(mode: GameType) -> &'static str {
    if matches!(mode, GameType::Spectator) {
        "наблюдатель"
    } else {
        "креатив"
    }
}

fn admin_permission() -> Option<PermissionExpr> {
    PermissionKey::parse(PERMISSION)
        .ok()
        .map(PermissionExpr::key)
}

fn is_admin(player: &Player) -> bool {
    admin_permission().is_some_and(|permission| player.has_permission(&permission))
}

fn place(player: &Player) -> String {
    let position = player.position();
    format!(
        "{} {:.1} {:.1} {:.1}",
        player.get_world().key,
        position.x,
        position.y,
        position.z
    )
}

fn log(dir: &Path, player: &Player, what: &str) {
    let line = format!(
        "{} {} ({}) {what}\n",
        chrono::Utc::now().to_rfc3339(),
        player.gameprofile.name(),
        player.gameprofile.id()
    );
    let written = fs::create_dir_all(dir).and_then(|()| {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(LOG_FILE))?
            .write_all(line.as_bytes())
    });
    if let Err(error) = written {
        log::warn!("MineHouse admin mode: could not append to {LOG_FILE}: {error}");
    }
}

/// Writes `bytes` to `file` through a temporary file next to it.
fn write_backup(file: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = file.with_extension("tmp");
    let mut out = fs::File::create(&temp)?;
    out.write_all(bytes)?;
    out.sync_all()?;
    fs::rename(&temp, file)
}

fn clear_carried(player: &Player) {
    {
        let mut inventory = player.inventory.lock();
        for slot in 0..PlayerInventory::CONTAINER_SIZE {
            inventory.set_item(slot, ItemStack::empty());
        }
    }
    player.experience.lock().clear();
}

fn remove_effects(player: &Player) {
    for effect in player.active_mob_effects() {
        player.remove_mob_effect(effect.effect());
    }
}

/// Backs everything up, then clears it. `Err` says what went wrong, for the player.
fn enter(player: &Player, mode: GameType) -> Result<(), &'static str> {
    let id = player.gameprofile.id();
    if is_active(id) {
        return Err("Ты уже в админ-режиме.");
    }
    player.close_container();
    let dir = dir();
    let file = backup_file(&dir, id);
    let saved = encode_player_data(&PersistentPlayerData::from_player(player))
        .and_then(|bytes| write_backup(&file, &bytes));
    if let Err(error) = saved {
        log::error!(
            "MineHouse admin mode: could not write the backup of {} to {}: {error}",
            player.gameprofile.name(),
            file.display()
        );
        return Err(
            "Не удалось сохранить бэкап, админ-режим не включён. Подробности в логе сервера.",
        );
    }
    set_active(id, true);

    // Only now, with the backup on disk, take things away.
    let at = place(player);
    clear_carried(player);
    {
        let mut chest = player.ender_chest_inventory.lock();
        for slot in 0..chest.get_container_size() {
            chest.set_item(slot, ItemStack::empty());
        }
    }
    remove_effects(player);
    player.set_health(player.get_max_health());
    {
        let mut food = player.food_data.lock();
        food.food_level = FULL_FOOD;
        food.saturation_level = START_SATURATION;
    }
    player.base().clear_fire();
    player.set_game_mode(mode);

    log(&dir, player, &format!("on {} at {at}", mode_id(mode)));
    log::info!(
        "MineHouse admin mode: {} entered ({}) at {at}",
        player.gameprofile.name(),
        mode_id(mode)
    );
    Ok(())
}

/// The world of `saved`, or the default one of the player's domain if it is gone.
fn saved_world(server: &Server, player: &Player, saved: &PersistentPlayerData) -> Arc<World> {
    let current = player.get_world();
    let found = server
        .worlds
        .worlds_in_domain(current.domain())
        .into_iter()
        .find(|world| world.key.to_string() == saved.world);
    found.unwrap_or_else(|| {
        log::warn!(
            "MineHouse admin mode: world {} of {}'s backup is gone, using the default one",
            saved.world,
            player.gameprofile.name()
        );
        server
            .worlds
            .default_world(current.domain())
            .map_or(current, Arc::clone)
    })
}

/// Gives back what `saved` holds and takes the player to where it was made.
fn restore(server: &Server, player: &Arc<Player>, saved: &PersistentPlayerData) {
    let world = saved_world(server, player, saved);
    {
        let mut inventory = player.inventory.lock();
        for slot in 0..PlayerInventory::CONTAINER_SIZE {
            inventory.set_item(slot, ItemStack::empty());
        }
        for slot in &saved.inventory {
            if let Ok(index) = usize::try_from(slot.slot)
                && index < PlayerInventory::CONTAINER_SIZE
            {
                inventory.set_item(index, slot.item.clone());
            }
        }
    }
    {
        let mut chest = player.ender_chest_inventory.lock();
        let size = chest.get_container_size();
        for slot in 0..size {
            chest.set_item(slot, ItemStack::empty());
        }
        for slot in &saved.ender_items {
            if let Ok(index) = usize::try_from(slot.slot)
                && index < size
            {
                chest.set_item(index, slot.item.clone());
            }
        }
    }
    *player.experience.lock() = Experience::from_parts(
        saved.experience_level,
        saved.experience_progress,
        saved.experience_total,
    )
    .with_enchantment_seed(saved.enchantment_seed);

    remove_effects(player);
    for effect in &saved.active_effects {
        player.add_mob_effect(effect.clone());
    }
    {
        let mut food = player.food_data.lock();
        food.food_level = saved.food_level;
        food.saturation_level = saved.food_saturation_level;
        food.exhaustion_level = saved.food_exhaustion_level;
        food.tick_timer = saved.food_tick_timer;
    }
    player.set_game_mode(GameType::from(saved.game_mode));

    player.base().stop_riding();
    let [x, y, z] = saved.pos;
    let [yaw, pitch] = saved.rotation;
    if !player.teleport_to_world(&world, DVec3::new(x, y, z), (yaw, pitch)) {
        log::warn!(
            "MineHouse admin mode: {} could not be taken back to {} {x:.1} {y:.1} {z:.1}",
            player.gameprofile.name(),
            saved.world
        );
    }
    player.set_health(saved.health.clamp(1.0, player.get_max_health()));
    player.base().reset_fall_distance();
    player.base().clear_fire();
    player.base().set_velocity(DVec3::ZERO);
}

/// Moves a used backup to `history/` and drops the oldest ones of the player.
fn archive(dir: &Path, id: Uuid, file: &Path) {
    let history = dir.join(HISTORY_DIR);
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let moved = fs::create_dir_all(&history)
        .and_then(|()| fs::rename(file, history.join(history_name(id, &stamp))));
    if let Err(error) = moved {
        log::error!(
            "MineHouse admin mode: could not move {} to history: {error}",
            file.display()
        );
        if let Err(error) = fs::remove_file(file) {
            log::error!(
                "MineHouse admin mode: could not delete {} either, after a restart the player is in admin mode again: {error}",
                file.display()
            );
        }
        return;
    }
    let names: Vec<String> = fs::read_dir(&history)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    for name in outdated(&names, id, HISTORY_PER_PLAYER) {
        if let Err(error) = fs::remove_file(history.join(&name)) {
            log::warn!("MineHouse admin mode: could not delete old backup {name}: {error}");
        }
    }
}

/// Gives everything back and returns the player to where admin mode started.
fn exit(server: &Server, player: &Arc<Player>) -> Result<(), String> {
    let id = player.gameprofile.id();
    if !is_active(id) {
        return Err("Ты не в админ-режиме.".to_owned());
    }
    let dir = dir();
    let file = backup_file(&dir, id);
    let saved = match fs::read(&file).and_then(|bytes| decode_player_data(&bytes)) {
        Ok(saved) => saved,
        Err(error) => {
            log::error!(
                "MineHouse admin mode: could not read the backup of {} from {}: {error}",
                player.gameprofile.name(),
                file.display()
            );
            return Err(format!(
                "Не удалось прочитать бэкап {id}.{BACKUP_EXTENSION}, ничего не изменено. Подробности в логе сервера."
            ));
        }
    };

    player.close_container();
    restore(server, player, &saved);
    set_active(id, false);
    archive(&dir, id, &file);
    let at = format!(
        "{} {:.1} {:.1} {:.1}",
        saved.world, saved.pos[0], saved.pos[1], saved.pos[2]
    );
    log(&dir, player, &format!("off, back at {at}"));
    log::info!(
        "MineHouse admin mode: {} left, back at {at}",
        player.gameprofile.name()
    );
    Ok(())
}

/// Things handed out in admin mode must not end up on the ground when the admin dies.
pub(super) struct AdminModeDeath;

/// The death observer of this module.
pub(super) static DEATH: AdminModeDeath = AdminModeDeath;

impl PlayerDeathObserver for AdminModeDeath {
    fn before_drops(&self, player: &Player) {
        if is_active(player.gameprofile.id()) {
            clear_carried(player);
        }
    }
}

/// Reminds those who join while in admin mode. A job of the server tick, never finished.
#[derive(Default)]
struct Watch {
    online: FxHashSet<Uuid>,
}

impl ServerJob for Watch {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        if !context.tick_count.is_multiple_of(CHECK_EVERY_TICKS) {
            return JobPoll::Pending;
        }
        let players = server.get_players();
        for player in &players {
            let id = player.gameprofile.id();
            if !self.online.contains(&id) && is_active(id) {
                player.send_message(&render(
                    "&6Ты в админ-режиме. &7Выйти и вернуть свои вещи: &f/adminmode off",
                    &[],
                ));
            }
        }
        self.online = players
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        JobPoll::Pending
    }
}

/// Finds who is in admin mode and starts the reminder.
pub(super) fn start(server: &Server) {
    let ids: FxHashSet<Uuid> = fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| backup_owner(entry.file_name().to_str()?))
        .collect();
    log::info!("MineHouse admin mode on: {} in it now", ids.len());
    *ACTIVE.lock() = Some(ids);
    server.jobs.spawn(Watch::default());
}

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    // Open to everyone as a command; who sees and runs it is decided by the tree itself.
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "adminmode"), tree).default_access(),
    )?;
    Ok(())
}

fn source_is_admin(source: CommandSource<'_>) -> bool {
    source.player().is_some_and(|player| is_admin(player))
}

fn source_in_mode(source: CommandSource<'_>) -> bool {
    source
        .player()
        .is_some_and(|player| is_active(player.gameprofile.id()))
}

fn tree() -> CommandNode {
    literal("adminmode")
        .requires(|source| source_is_admin(source) || source_in_mode(source))
        .executes(|context| toggle(*context))
        .then(
            literal("on")
                .requires(source_is_admin)
                .executes(|context| on(*context, GameType::Creative))
                .then(literal("creative").executes(|context| on(*context, GameType::Creative)))
                .then(literal("spectator").executes(|context| on(*context, GameType::Spectator))),
        )
        .then(literal("off").executes(|context| off(*context)))
}

fn player_of(context: CommandContext<'_>) -> Result<Arc<Player>, CommandError> {
    context
        .source()
        .player()
        .cloned()
        .ok_or_else(|| CommandError::from("Только для игроков"))
}

fn toggle(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player_of(context)?;
    if is_active(player.gameprofile.id()) {
        off(context)
    } else if is_admin(&player) {
        on(context, GameType::Creative)
    } else {
        Err(CommandError::from("Недостаточно прав"))
    }
}

fn on(context: CommandContext<'_>, mode: GameType) -> Result<i32, CommandError> {
    let player = player_of(context)?;
    enter(&player, mode).map_err(CommandError::from)?;
    context.source().send_success(
        &render(
            &format!(
                "&6Админ-режим включён ({}). &7Инвентарь, эндер-сундук, опыт и эффекты сохранены в бэкап, место запомнено. Выйти: &f/adminmode off",
                mode_name(mode)
            ),
            &[],
        ),
        true,
    );
    Ok(1)
}

fn off(context: CommandContext<'_>) -> Result<i32, CommandError> {
    let player = player_of(context)?;
    exit(context.source().server(), &player).map_err(|error| CommandError::from(error.as_str()))?;
    context.source().send_success(
        &render(
            "&aАдмин-режим выключен. &7Ты на прежнем месте, вещи, опыт и режим игры возвращены.",
            &[],
        ),
        true,
    );
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(last: u8) -> Uuid {
        Uuid::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, last])
    }

    #[test]
    fn a_backup_is_named_by_its_player() {
        let file = backup_file(Path::new("data/adminmode"), id(7));
        let name = file.file_name().and_then(|name| name.to_str());
        assert_eq!(name, Some("00000000-0000-0000-0000-000000000007.dat"));
        assert_eq!(name.and_then(backup_owner), Some(id(7)));
    }

    #[test]
    fn other_files_of_the_directory_are_nobodys_backup() {
        assert_eq!(backup_owner("log.txt"), None);
        assert_eq!(backup_owner("history"), None);
        assert_eq!(
            backup_owner("00000000-0000-0000-0000-000000000007.tmp"),
            None
        );
        assert_eq!(
            backup_owner("00000000-0000-0000-0000-000000000007dat"),
            None
        );
    }

    #[test]
    fn the_newest_backups_of_a_player_stay_in_history() {
        let mut names: Vec<String> = (10..17)
            .map(|second| history_name(id(1), &format!("20261115-1200{second}")))
            .collect();
        names.push(history_name(id(2), "20261115-110000"));
        names.push("notes.txt".to_owned());
        names.reverse();

        let gone = outdated(&names, id(1), 5);
        assert_eq!(
            gone,
            vec![
                history_name(id(1), "20261115-120010"),
                history_name(id(1), "20261115-120011"),
            ]
        );
        assert_eq!(outdated(&names, id(2), 5), Vec::<String>::new());
        assert_eq!(outdated(&names, id(3), 5), Vec::<String>::new());
    }

    #[test]
    fn the_modes_are_named_in_russian() {
        assert_eq!(mode_name(GameType::Creative), "креатив");
        assert_eq!(mode_name(GameType::Spectator), "наблюдатель");
    }
}
