//! The banker (the Fabric `Banker`): a mannequin that explains how the bank works. A right
//! click starts a talk; the window is drawn by [`hud`], the wheel picks an answer (the hotbar
//! slot comes right back), Shift goes on or answers, Space or five blocks away ends it.
//!
//! The Fabric banker was a summoned entity. Here he is shown with packets alone: where he
//! stands is kept in `bank-bankers.json` of the data directory and every player near gets a
//! mannequin of their own, so while he talks he looks at each player separately.
//!
//! His body is the rig of the Fabric banker, see [`puppet`]: he idles, looks up at whoever
//! comes and gestures while he talks.
//!
//! `/bank banker [nick]` puts him where the admin stands, `/bank banker url <link>` with the
//! skin at the link, `/bank banker remove` takes away the nearest one, and so does a hit of an
//! admin in creative mode (as killing him did on Fabric).

pub mod commands;
pub mod hud;
mod puppet;
mod rig;
mod skin;
pub mod talk;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use glam::DVec3;
use serde::{Deserialize, Serialize};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_core::{
    entity::{Entity as _, next_entity_id},
    player::Player,
    server::Server,
};
use steel_protocol::packets::game::{
    CAddEntity, CClearTitles, CRemoveEntities, CRotateHead, CSetActionBarText, CSetEntityData,
    CSetHeldSlot, CSetTitleText, CSetTitlesAnimation, CSound, SoundSource,
};
use steel_registry::{
    RegistryEntry as _, entity_data::DataValue, sound_event::SoundEvent, sound_events,
    vanilla_entities, vanilla_entity_data::MannequinEntityData,
};
use steel_utils::{
    Identifier,
    locks::SyncMutex,
    types::{GameType, InteractionHand},
};
use text_components::TextComponent;
use uuid::Uuid;

pub use self::skin::Skin;
use self::{
    puppet::{Doing, Puppet},
    talk::{Step, Talk},
};
use super::{Bank, bank};
use crate::config;

/// The file of the bankers in the data directory.
pub(crate) const FILE: &str = "bank-bankers.json";
/// The tag of the mannequin the Fabric mod summoned as the banker.
pub(crate) const TAG: &str = "minehouse_banker";
/// How far the player may step away while talking.
const REACH: f64 = 5.0;
/// From how far the banker is shown.
const VIEW: f64 = 64.0;
/// How often it is checked who is near.
const VIEW_TICKS: u64 = 10;
/// The fog stays for as long as the talk may last.
const FOG_TICKS: i32 = 20 * 60 * 60;
/// Vanilla `Entity.FLAG_INVISIBLE` of the shared flags.
const FLAG_INVISIBLE: i8 = 1 << 5;
/// His name over him.
const NAME: &str = "Банкир";

/// Where a banker stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Banker {
    world: String,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    skin: Skin,
}

impl Banker {
    const fn position(&self) -> DVec3 {
        DVec3::new(self.x, self.y, self.z)
    }

    /// The banker of a mannequin the Fabric mod summoned in `world`, as the world saved it.
    #[must_use]
    pub(crate) fn from_mannequin(world: &Identifier, saved: &NbtCompound) -> Option<Self> {
        let position = saved.list("Pos").and_then(NbtList::doubles)?;
        let [x, y, z] = position[..] else {
            return None;
        };
        let yaw = saved
            .list("Rotation")
            .and_then(NbtList::floats)
            .and_then(|rotation| rotation.first().copied())
            .unwrap_or(0.0);
        Some(Self {
            world: world.to_string(),
            x,
            y,
            z,
            yaw,
            skin: Skin::from_saved_profile(saved.get("profile")),
        })
    }
}

/// Adds `bankers` to the file of the bankers at `file`, skipping any already standing there.
/// Returns how many were added.
pub(crate) fn add_to_file(file: &Path, bankers: Vec<Banker>) -> io::Result<usize> {
    let mut saved = match fs::read_to_string(file) {
        Ok(text) => read(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let mut added = 0;
    for banker in bankers {
        if !saved
            .iter()
            .any(|other| other.world == banker.world && other.position() == banker.position())
        {
            saved.push(banker);
            added += 1;
        }
    }
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&saved).map_err(io::Error::other)?;
    fs::write(file, text)?;
    Ok(added)
}

struct Placed {
    banker: Banker,
    /// The id of his mannequin on the clients.
    entity: i32,
    /// His body, unless the rig can't be read: then the mannequin shows.
    puppet: Option<Puppet>,
}

impl Placed {
    fn new(banker: Banker) -> Self {
        Self {
            banker,
            entity: next_entity_id(),
            puppet: Puppet::new(0),
        }
    }

    /// The ids of every entity he is on the clients.
    fn ids(&self) -> Vec<i32> {
        let mut ids = vec![self.entity];
        ids.extend(self.puppet.iter().flat_map(Puppet::ids));
        ids
    }
}

struct Active {
    talk: Talk,
    /// The mannequin talked to.
    entity: i32,
    /// The hotbar slot held when the talk began.
    anchor: u8,
    shift: bool,
    jump: bool,
}

/// The bankers, who sees them and who talks to them.
pub struct Bankers {
    placed: SyncMutex<Vec<Placed>>,
    /// Per player, the mannequins they were shown and every entity sent with each.
    seen: SyncMutex<BTreeMap<Uuid, BTreeMap<i32, Vec<i32>>>>,
    talks: SyncMutex<BTreeMap<Uuid, Active>>,
    file: PathBuf,
}

/// A rotation in degrees as the byte of a packet.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "an angle wraps around the byte"
)]
fn angle(degrees: f32) -> i8 {
    ((degrees * 256.0 / 360.0).floor() as i32 & 0xFF) as u8 as i8
}

/// The yaw at which someone at `from` looks at `to`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "an angle in degrees fits a float"
)]
fn yaw_towards(from: DVec3, to: DVec3) -> f32 {
    ((to.z - from.z).atan2(to.x - from.x).to_degrees() - 90.0) as f32
}

fn read(text: &str) -> Vec<Banker> {
    serde_json::from_str(text).unwrap_or_else(|error| {
        log::warn!("Bank: {FILE} can't be read: {error}");
        Vec::new()
    })
}

/// The mannequin's data; it is invisible when the rig is his body.
fn data(banker: &Banker, rigged: bool) -> Option<Vec<DataValue>> {
    let mut data = MannequinEntityData::new();
    data.profile.set(banker.skin.profile());
    data.immovable.set(true);
    data.description
        .set(Some(Box::new(TextComponent::plain("Банк MineHouse"))));
    let base = &mut data.avatar.living_entity.base;
    base.custom_name
        .set(Some(Box::new(TextComponent::plain(NAME))));
    base.custom_name_visible.set(true);
    if rigged {
        base.shared_flags.set(FLAG_INVISIBLE);
    }
    data.pack_dirty()
}

fn sound(player: &Player, sound: &'static SoundEvent, volume: f32, pitch: f32) {
    player.send_packet(CSound::new(
        sound,
        SoundSource::Players,
        player.position(),
        volume,
        pitch,
        0,
    ));
}

fn close(player: &Player) {
    player.send_packet(CSetActionBarText::new(TextComponent::plain("")));
    player.send_packet(CClearTitles::new(true));
}

impl Bankers {
    /// Reads where the bankers stand.
    #[must_use]
    pub fn load() -> Self {
        let file = config::data_path(FILE);
        let placed = fs::read_to_string(&file)
            .map_or_default(|text| read(&text))
            .into_iter()
            .map(Placed::new)
            .collect();
        Self {
            placed: SyncMutex::new(placed),
            seen: SyncMutex::new(BTreeMap::new()),
            talks: SyncMutex::new(BTreeMap::new()),
            file,
        }
    }

    fn save(&self) {
        let bankers: Vec<Banker> = self
            .placed
            .lock()
            .iter()
            .map(|placed| placed.banker.clone())
            .collect();
        let text = serde_json::to_string_pretty(&bankers).unwrap_or_default();
        if let Some(parent) = self.file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(error) = fs::write(&self.file, text) {
            log::warn!("Bank: can't write {}: {error}", self.file.display());
        }
    }

    /// Puts a banker; those near see him within half a second.
    pub fn place(&self, banker: Banker) {
        self.placed.lock().push(Placed::new(banker));
        self.save();
    }

    /// Takes away the banker nearest to a place, within [`REACH`]; false if none is there.
    pub fn remove_near(&self, world: &str, at: DVec3) -> bool {
        let removed = {
            let mut placed = self.placed.lock();
            let nearest = placed
                .iter()
                .enumerate()
                .filter(|(_, placed)| placed.banker.world == world)
                .map(|(index, placed)| (index, placed.banker.position().distance(at)))
                .filter(|(_, distance)| *distance <= REACH)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            nearest.map(|(index, _)| placed.remove(index))
        };
        if removed.is_some() {
            self.save();
        }
        removed.is_some()
    }

    /// Takes away the banker whose mannequin is `entity`; false if it is no banker.
    fn remove_entity(&self, entity: i32) -> bool {
        let removed = {
            let mut placed = self.placed.lock();
            let before = placed.len();
            placed.retain(|placed| placed.entity != entity);
            placed.len() != before
        };
        if removed {
            self.save();
        }
        removed
    }

    fn is_banker(&self, entity: i32) -> bool {
        self.placed
            .lock()
            .iter()
            .any(|placed| placed.entity == entity)
    }

    pub(super) fn forget(&self, player: Uuid) {
        self.seen.lock().remove(&player);
        self.talks.lock().remove(&player);
    }

    fn start(&self, player: &Player, entity: i32) {
        let uuid = player.gameprofile.id();
        let mut talks = self.talks.lock();
        if talks.contains_key(&uuid) {
            return;
        }
        let input = player.last_client_input();
        let active = Active {
            talk: Talk::new(player.gameprofile.name()),
            entity,
            anchor: player.inventory.lock().get_selected_slot(),
            shift: input.shift(),
            jump: input.jump(),
        };
        player.send_packet(CSetTitlesAnimation {
            fade_in: 10,
            stay: FOG_TICKS,
            fade_out: 10,
        });
        player.send_packet(CSetTitleText::new(hud::fog()));
        sound(player, &sound_events::ENTITY_VILLAGER_TRADE, 0.6, 1.0);
        player.send_packet(CSetActionBarText::new(active.talk.render()));
        talks.insert(uuid, active);
    }

    /// Shows the bankers to those who came near and hides them from those who left.
    fn tick_view(&self, players: &[Arc<Player>]) {
        let placed = self.placed.lock();
        let mut seen = self.seen.lock();
        for player in players {
            let world = player.get_world().key.to_string();
            let position = player.position();
            // The client of a dead player forgets every entity when it respawns.
            let alive = player.is_alive();
            let near: BTreeSet<i32> = placed
                .iter()
                .filter(|placed| {
                    alive
                        && placed.banker.world == world
                        && placed.banker.position().distance(position) <= VIEW
                })
                .map(|placed| placed.entity)
                .collect();
            let shown = seen.entry(player.gameprofile.id()).or_default();
            let mut gone = Vec::new();
            shown.retain(|entity, ids| {
                let keep = near.contains(entity);
                if !keep {
                    gone.append(ids);
                }
                keep
            });
            if !gone.is_empty() {
                player.send_packet(CRemoveEntities { entity_ids: gone });
            }
            for placed in placed.iter().filter(|placed| near.contains(&placed.entity)) {
                if shown.contains_key(&placed.entity) {
                    continue;
                }
                let turn = angle(placed.banker.yaw);
                player.send_packet(CAddEntity {
                    id: placed.entity,
                    uuid: Uuid::new_v4(),
                    entity_type: vanilla_entities::MANNEQUIN.id() as i32,
                    position: placed.banker.position(),
                    velocity: DVec3::ZERO,
                    x_rot: 0,
                    y_rot: turn,
                    head_y_rot: turn,
                    data: 0,
                });
                if let Some(values) = data(&placed.banker, placed.puppet.is_some()) {
                    player.send_packet(CSetEntityData::new(placed.entity, values));
                }
                if let Some(puppet) = &placed.puppet {
                    puppet.spawn(player, placed.banker.position(), placed.banker.yaw, NAME);
                }
                shown.insert(placed.entity, placed.ids());
            }
        }
    }

    /// Moves the bankers' bodies: what each does depends on the talks to him.
    fn tick_rigs(&self, players: &[Arc<Player>], tick: u64) {
        let mut doing: BTreeMap<i32, Doing> = BTreeMap::new();
        for active in self.talks.lock().values() {
            let now = Doing::in_talk(active.talk.age, active.talk.typed());
            let was = doing.entry(active.entity).or_insert(Doing::Idle);
            *was = was.busier(now);
        }
        let mut placed = self.placed.lock();
        let seen = self.seen.lock();
        for placed in placed.iter_mut() {
            let entity = placed.entity;
            let yaw = placed.banker.yaw;
            let Some(puppet) = &mut placed.puppet else {
                continue;
            };
            let now = doing.get(&entity).copied().unwrap_or(Doing::Idle);
            let shapes = puppet.tick(now, tick, yaw);
            if shapes.is_empty() {
                continue;
            }
            for player in players {
                if !seen
                    .get(&player.gameprofile.id())
                    .is_some_and(|shown| shown.contains_key(&entity))
                {
                    continue;
                }
                for (id, values) in &shapes {
                    player.send_packet(CSetEntityData::new(*id, values.clone()));
                }
            }
        }
    }

    /// A tick of every talk.
    fn tick_talks(&self, players: &[Arc<Player>]) {
        let placed = self.placed.lock();
        self.talks.lock().retain(|uuid, active| {
            let Some(player) = players
                .iter()
                .find(|player| player.gameprofile.id() == *uuid)
            else {
                return false;
            };
            let banker = placed
                .iter()
                .find(|placed| placed.entity == active.entity)
                .map(|placed| &placed.banker)
                .filter(|banker| {
                    banker.world == player.get_world().key.to_string()
                        && banker.position().distance(player.position()) <= REACH
                        && !player.has_container_open()
                        && player.is_alive()
                });
            let Some(banker) = banker else {
                close(player);
                return false;
            };
            let talking = active.tick(player, banker.position());
            if !talking {
                close(player);
            }
            talking
        });
    }
}

impl Active {
    /// A tick of the talk; false when it is over.
    fn tick(&mut self, player: &Player, banker: DVec3) -> bool {
        self.talk.age += 1;
        player.send_packet(CRotateHead {
            entity_id: self.entity,
            head_y_rot: angle(yaw_towards(banker, player.position())),
        });
        let mut draw = false;

        let input = player.last_client_input();
        let (shift, jump) = (input.shift(), input.jump());
        let (pressed_shift, pressed_jump) = (shift && !self.shift, jump && !self.jump);
        self.shift = shift;
        self.jump = jump;
        if pressed_jump {
            return false;
        }
        if pressed_shift {
            match self.talk.advance() {
                Step::Ended => return false,
                Step::Turned => sound(player, &sound_events::ITEM_BOOK_PAGE_TURN, 0.7, 1.0),
                Step::Answered => sound(player, &sound_events::UI_BUTTON_CLICK, 0.4, 1.2),
                Step::Shown => {}
            }
            draw = true;
        }

        // The wheel moved the hotbar: the slot comes back and the choice moves instead.
        let slot = player.inventory.lock().get_selected_slot();
        if slot != self.anchor {
            let turned = (i32::from(slot) - i32::from(self.anchor) + 4).rem_euclid(9) - 4;
            player.inventory.lock().set_selected_slot(self.anchor);
            player.send_packet(CSetHeldSlot {
                slot: i32::from(self.anchor),
            });
            if self.talk.pick(turned > 0) {
                sound(player, &sound_events::UI_BUTTON_CLICK, 0.25, 1.8);
                draw = true;
            }
        }

        if self.talk.type_more() {
            if self.talk.age.is_multiple_of(3) {
                // The pitch wanders a little, as a voice does.
                #[expect(clippy::cast_precision_loss, reason = "a number below seven")]
                let pitch = 1.6 + (self.talk.age % 7) as f32 * 0.05;
                sound(player, &sound_events::BLOCK_NOTE_BLOCK_HAT, 0.15, pitch);
            }
            draw = true;
        }
        // Drawn again twice a second: the action bar must not fade and the arrow blinks.
        if draw || self.talk.age.is_multiple_of(10) {
            player.send_packet(CSetActionBarText::new(self.talk.render()));
        }
        true
    }
}

/// A click on an entity: true if it was a banker, and the click is his.
pub(crate) fn interact(player: &Player, entity: i32, hand: InteractionHand) -> bool {
    let Some(bank) = bank() else {
        return false;
    };
    if !bank.bankers.is_banker(entity) {
        return false;
    }
    if hand == InteractionHand::MainHand {
        bank.bankers.start(player, entity);
    }
    true
}

/// A hit on an entity: true if it was a banker. He takes no harm, but an admin in creative
/// mode takes him away, the way killing him did on Fabric.
pub(crate) fn attacked(player: &Player, entity: i32) -> bool {
    let Some(bank) = bank() else {
        return false;
    };
    if !bank.bankers.is_banker(entity) {
        return false;
    }
    if player.game_mode() == GameType::Creative
        && commands::may_place(player)
        && bank.bankers.remove_entity(entity)
    {
        super::gui::info(
            player,
            "Банкир убран. Поставить заново: /bank banker там, где он должен стоять.",
        );
    }
    true
}

impl Bank {
    /// The bankers' part of the server tick.
    pub(super) fn tick_bankers(&self, server: &Server, tick: u64) {
        let players = server.get_players();
        if tick.is_multiple_of(VIEW_TICKS) {
            self.bankers.tick_view(&players);
        }
        self.bankers.tick_rigs(&players, tick);
        self.bankers.tick_talks(&players);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_angle_wraps_around_the_byte() {
        assert_eq!(angle(0.0), 0);
        assert_eq!(angle(90.0), 64);
        assert_eq!(angle(180.0), -128);
        assert_eq!(angle(-90.0), -64);
        assert_eq!(angle(360.0), 0);
    }

    #[test]
    fn the_banker_turns_his_head_to_the_player() {
        let banker = DVec3::new(0.0, 64.0, 0.0);
        // South is yaw 0, west 90, north 180, east −90.
        assert!(yaw_towards(banker, DVec3::new(0.0, 64.0, 5.0)).abs() < 0.01);
        assert!((yaw_towards(banker, DVec3::new(-5.0, 64.0, 0.0)) - 90.0).abs() < 0.01);
        assert!((yaw_towards(banker, DVec3::new(5.0, 64.0, 0.0)) + 90.0).abs() < 0.01);
    }

    #[test]
    fn the_file_keeps_where_the_bankers_stand() {
        let banker = Banker {
            world: "minecraft:overworld".to_owned(),
            x: 10.5,
            y: 64.0,
            z: -3.5,
            yaw: 90.0,
            skin: Skin::Name {
                name: "Steve".to_owned(),
            },
        };
        let text = serde_json::to_string(&vec![banker.clone()]).unwrap_or_default();
        assert_eq!(read(&text), vec![banker]);
        assert_eq!(read("not json"), Vec::new());
    }

    #[test]
    fn the_mannequin_is_named_and_has_a_skin() {
        let banker = Banker {
            world: "minecraft:overworld".to_owned(),
            x: 0.0,
            y: 64.0,
            z: 0.0,
            yaw: 0.0,
            skin: Skin::default_skin(),
        };
        // The profile, the name and that it shows, the description and that he can't be pushed.
        assert_eq!(data(&banker, false).map(|values| values.len()), Some(5));
        // And invisible over a rig.
        assert_eq!(data(&banker, true).map(|values| values.len()), Some(6));
    }
}
