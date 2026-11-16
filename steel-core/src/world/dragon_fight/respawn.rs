//! Vanilla `DragonRespawnStage` and the respawn ritual: four End crystals
//! around the exit portal rebuild the towers and summon a new dragon.

use std::sync::Arc;

use glam::{DVec3, IVec3};
use serde::{Deserialize, Serialize};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::feature::ConfiguredFeatureRef;
use steel_registry::feature::data::{ConfiguredFeatureKind, EndSpikeConfiguration};
use steel_registry::{level_events, vanilla_blocks};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, Direction, Downcast as _, WorldAabb};

use super::portal::place_feature;
use super::{EnderDragonFight, crystals_in, spike_top_box, spikes};
use crate::entity::entities::EndCrystalEntity;
use crate::entity::{RemovalReason, SharedEntity};
use crate::world::World;
use crate::world::explosion::{ExplosionInteraction, ExplosionOptions};

/// The block the crystals beam at while the ritual runs.
const BEAM_CENTER: BlockPos = BlockPos(IVec3::new(0, 128, 0));
/// Ticks each tower takes to rebuild.
const PILLAR_INTERVAL: i32 = 40;
/// The half size of the cube cleared before a tower is rebuilt.
const PILLAR_CLEAR_RADIUS: i32 = 10;
/// The y a rebuilt tower's feature is placed at.
const PILLAR_PLACE_Y: i32 = 45;
const PILLAR_EXPLOSION_POWER: f32 = 5.0;
const CRYSTAL_EXPLOSION_POWER: f32 = 6.0;
/// How far from the portal's center each ritual crystal stands.
const RITUAL_CRYSTAL_DISTANCE: i32 = 3;

/// Vanilla `DragonRespawnStage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DragonRespawnStage {
    /// The crystals start beaming at the center.
    Start,
    /// The roar before the towers rise.
    PreparingToSummonPillars,
    /// The towers are rebuilt one by one.
    SummoningPillars,
    /// The crystals beam at the center again and the dragon appears.
    SummoningDragon,
    /// The ritual is over.
    End,
}

impl DragonRespawnStage {
    /// Vanilla `DragonRespawnStage.tick`.
    fn tick(
        self,
        world: &Arc<World>,
        fight: &EnderDragonFight,
        crystals: &[SharedEntity],
        time: i32,
    ) {
        match self {
            Self::Start => {
                set_beams(crystals, Some(BEAM_CENTER));
                fight.set_respawn_stage(world, Self::PreparingToSummonPillars);
            }
            Self::PreparingToSummonPillars => {
                if time < 100 {
                    if matches!(time, 0 | 50 | 51 | 52) || time >= 95 {
                        roar(world);
                    }
                } else {
                    fight.set_respawn_stage(world, Self::SummoningPillars);
                }
            }
            Self::SummoningPillars => summon_pillars(world, fight, crystals, time),
            Self::SummoningDragon => {
                if time >= 100 {
                    fight.set_respawn_stage(world, Self::End);
                    EnderDragonFight::reset_spike_crystals(world);
                    for crystal in crystals {
                        set_beam(crystal, None);
                        let mut options = ExplosionOptions::new(
                            crystal.position(),
                            CRYSTAL_EXPLOSION_POWER,
                            ExplosionInteraction::None,
                        );
                        options.source = Some(Arc::clone(crystal));
                        world.explode(options);
                        crystal.set_removed(RemovalReason::Discarded);
                    }
                } else if time >= 80 {
                    roar(world);
                } else if time == 0 {
                    set_beams(crystals, Some(BEAM_CENTER));
                } else if time < 5 {
                    roar(world);
                }
            }
            Self::End => {}
        }
    }
}

/// The `SUMMONING_PILLARS` stage: each tower gets a beam for 40 ticks, then
/// is cleared, blown up and rebuilt with a guarded crystal.
fn summon_pillars(
    world: &Arc<World>,
    fight: &EnderDragonFight,
    crystals: &[SharedEntity],
    time: i32,
) {
    let start_of_beam = time % PILLAR_INTERVAL == 0;
    let end_of_beam = time % PILLAR_INTERVAL == PILLAR_INTERVAL - 1;
    if !start_of_beam && !end_of_beam {
        return;
    }
    let spikes = spikes(world);
    let Some(spike) = usize::try_from(time / PILLAR_INTERVAL)
        .ok()
        .and_then(|index| spikes.get(index))
    else {
        if start_of_beam {
            fight.set_respawn_stage(world, DragonRespawnStage::SummoningDragon);
        }
        return;
    };
    if start_of_beam {
        let target = BlockPos::new(spike.center_x, spike.height + 1, spike.center_z);
        set_beams(crystals, Some(target));
        return;
    }
    for x in spike.center_x - PILLAR_CLEAR_RADIUS..=spike.center_x + PILLAR_CLEAR_RADIUS {
        for y in spike.height - PILLAR_CLEAR_RADIUS..=spike.height + PILLAR_CLEAR_RADIUS {
            for z in spike.center_z - PILLAR_CLEAR_RADIUS..=spike.center_z + PILLAR_CLEAR_RADIUS {
                world.remove_block(BlockPos::new(x, y, z), false);
            }
        }
    }
    world.explode(ExplosionOptions::new(
        DVec3::new(
            f64::from(spike.center_x) + 0.5,
            f64::from(spike.height),
            f64::from(spike.center_z) + 0.5,
        ),
        PILLAR_EXPLOSION_POWER,
        ExplosionInteraction::Block,
    ));
    let feature = ConfiguredFeatureRef::Inline(Box::new(ConfiguredFeatureKind::EndSpike(
        EndSpikeConfiguration {
            spikes: vec![spike.clone()],
            crystal_invulnerable: true,
            crystal_beam_target: Some(BEAM_CENTER.0),
        },
    )));
    place_feature(
        world,
        &feature,
        BlockPos::new(spike.center_x, PILLAR_PLACE_Y, spike.center_z),
    );
}

/// Vanilla `new AABB(pos)`: the unit box of a block.
fn block_box(pos: BlockPos) -> WorldAabb {
    let (x, y, z) = (f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
    WorldAabb::new(x, y, z, x + 1.0, y + 1.0, z + 1.0)
}

fn roar(world: &World) {
    world.level_event(
        level_events::ANIMATION_DRAGON_SUMMON_ROAR,
        BEAM_CENTER,
        0,
        None,
    );
}

fn set_beam(crystal: &SharedEntity, target: Option<BlockPos>) {
    if let Some(crystal) = crystal.downcast_ref::<EndCrystalEntity>() {
        crystal.set_beam_target(target);
    }
}

fn set_beams(crystals: &[SharedEntity], target: Option<BlockPos>) {
    for crystal in crystals {
        set_beam(crystal, target);
    }
}

impl EnderDragonFight {
    /// Runs one tick of the ritual; `false` when it was aborted because its
    /// crystals are gone, which ends the fight's tick.
    pub(super) fn tick_respawn(&self, world: &Arc<World>) -> bool {
        let uuids = self.state.lock().respawn_crystals.clone();
        let crystals: Vec<SharedEntity> = uuids
            .iter()
            .filter_map(|uuid| world.get_entity_by_uuid(uuid))
            .filter(|entity| entity.downcast_ref::<EndCrystalEntity>().is_some())
            .collect();
        if crystals.is_empty() {
            self.abort_respawn_sequence(world);
            return false;
        }
        let (stage, time) = {
            let mut fight = self.state.lock();
            let Some(current) = fight.respawn_stage else {
                return true;
            };
            let time = fight.respawn_time;
            fight.respawn_time += 1;
            (current, time)
        };
        stage.tick(world, self, &crystals, time);
        true
    }

    /// Vanilla `setRespawnStage`: the `END` stage ends the ritual with a new
    /// dragon.
    pub(super) fn set_respawn_stage(&self, world: &Arc<World>, stage: DragonRespawnStage) {
        {
            let mut fight = self.state.lock();
            if fight.respawn_stage.is_none() {
                log::error!("Dragon respawn isn't in progress, can't skip ahead in the animation.");
                return;
            }
            fight.respawn_time = 0;
            if stage != DragonRespawnStage::End {
                fight.respawn_stage = Some(stage);
                return;
            }
            fight.respawn_stage = None;
            fight.dragon_killed = false;
        }
        // TODO(advancement-triggers): vanilla fires `SUMMONED_ENTITY` for
        // every boss bar player once the new dragon spawns.
        self.create_new_dragon(world);
    }

    /// Vanilla `tryRespawn`: starts the ritual when the dragon is dead and
    /// a crystal stands on each side of the exit portal.
    pub fn try_respawn(&self, world: &Arc<World>) {
        let location = {
            let state = self.state.lock();
            if !state.dragon_killed || state.respawn_stage.is_some() {
                return;
            }
            state.exit_portal_location
        };
        let location = if let Some(location) = location {
            location
        } else {
            log::debug!("Tried to respawn, but need to find the portal first.");
            if self.find_exit_portal(world).is_none() {
                log::debug!("Couldn't find a portal, so we made one.");
                self.spawn_exit_portal(world, true);
            } else {
                log::debug!("Found the exit portal & saved its location for next time.");
            }
            let Some(location) = self.state.lock().exit_portal_location else {
                return;
            };
            location
        };

        let center = location.above();
        let mut crystals = Vec::new();
        for direction in Direction::HORIZONTAL {
            let pos = center.relative_n(direction, RITUAL_CRYSTAL_DISTANCE);
            let found = crystals_in(world, &block_box(pos));
            if found.is_empty() {
                return;
            }
            crystals.extend(found);
        }
        log::debug!("Found all crystals, respawning dragon.");
        self.respawn_dragon(world, &crystals);
    }

    /// Vanilla `respawnDragon`: tears down the exit portal and starts the
    /// ritual.
    fn respawn_dragon(&self, world: &Arc<World>, crystals: &[SharedEntity]) {
        {
            let state = self.state.lock();
            if !state.dragon_killed || state.respawn_stage.is_some() {
                return;
            }
        }
        let end_stone = vanilla_blocks::END_STONE.default_state();
        while let Some(portal) = self.find_exit_portal(world) {
            for x in 0..self.exit_portal_pattern.width() {
                for y in 0..self.exit_portal_pattern.height() {
                    for z in 0..self.exit_portal_pattern.depth() {
                        let pos = portal.block(x, y, z);
                        let block = world.get_block_state(pos).get_block();
                        if block == &vanilla_blocks::BEDROCK || block == &vanilla_blocks::END_PORTAL
                        {
                            world.set_block(pos, end_stone, UpdateFlags::UPDATE_ALL);
                        }
                    }
                }
            }
        }
        {
            let mut state = self.state.lock();
            state.respawn_stage = Some(DragonRespawnStage::Start);
            state.respawn_time = 0;
        }
        self.spawn_exit_portal(world, false);
        self.state.lock().respawn_crystals =
            crystals.iter().map(|crystal| crystal.uuid()).collect();
    }

    /// Vanilla `abortRespawnSequence`.
    pub(super) fn abort_respawn_sequence(&self, world: &Arc<World>) {
        log::debug!("Aborting respawn sequence");
        {
            let mut state = self.state.lock();
            state.respawn_stage = None;
            state.respawn_time = 0;
        }
        Self::reset_spike_crystals(world);
        self.spawn_exit_portal(world, true);
    }

    /// Vanilla `resetSpikeCrystals`: the tower crystals lose their guard and
    /// beam.
    pub fn reset_spike_crystals(world: &World) {
        for spike in spikes(world) {
            for crystal in crystals_in(world, &spike_top_box(&spike)) {
                crystal.set_invulnerable(false);
                set_beam(&crystal, None);
            }
        }
    }
}
