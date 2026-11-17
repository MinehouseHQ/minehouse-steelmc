//! Operator-only requests from vanilla `ServerGamePacketListenerImpl`: the
//! game rules screen, the difficulty lock and the F3 + I data queries.

use simdnbt::owned::NbtCompound;
use steel_protocol::packets::game::{
    CTagQuery, SBlockEntityTagQuery, SEntityTagQuery, SLockDifficulty, SSetGameRule,
};
use steel_utils::serial::OptionalNbt;

use crate::command::{player_can_change_difficulty, set_game_rules_from_client};
use crate::entity::Entity;
use crate::player::Player;

impl Player {
    /// Vanilla `handleSetGameRule`.
    pub fn handle_set_game_rule(&self, packet: SSetGameRule) {
        let entries: Vec<_> = packet
            .entries
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect();
        set_game_rules_from_client(self, &entries);
    }

    /// Vanilla `handleLockDifficulty`: a player who may change the difficulty
    /// locks or unlocks it for every world of their domain.
    pub fn handle_lock_difficulty(&self, packet: &SLockDifficulty) {
        let world = self.get_world();
        if !player_can_change_difficulty(self, &world) {
            return;
        }
        for world in self.server().worlds.worlds_in_domain(world.domain()) {
            world.set_difficulty_locked(packet.locked);
        }
    }

    /// Vanilla `handleEntityTagQuery`: a game master gets the entity's saved
    /// data (`saveWithoutId`).
    pub fn handle_entity_tag_query(&self, packet: &SEntityTagQuery) {
        if !self.is_operator() {
            return;
        }
        let Some(entity) = self.get_world().get_entity_by_id(packet.entity_id) else {
            return;
        };
        self.send_packet(CTagQuery {
            transaction_id: packet.transaction_id,
            tag: OptionalNbt(Some(entity_saved_data(entity.as_ref()))),
        });
    }

    /// Vanilla `handleBlockEntityTagQuery`: a game master gets the block
    /// entity's data without metadata, or an empty answer for a plain block.
    pub fn handle_block_entity_tag_query(&self, packet: &SBlockEntityTagQuery) {
        if !self.is_operator() {
            return;
        }
        let tag = self
            .get_world()
            .get_block_entity(packet.pos)
            .map(|block_entity| block_entity.save_without_metadata());
        self.send_packet(CTagQuery {
            transaction_id: packet.transaction_id,
            tag: OptionalNbt(tag),
        });
    }
}

/// Vanilla `Entity.saveWithoutId`: the command comparison data without the
/// player-only `SelectedItem` that `NbtPredicate` adds.
fn entity_saved_data(entity: &dyn Entity) -> NbtCompound {
    let mut nbt = entity.nbt_for_data_compare();
    if entity.as_player().is_some() {
        nbt.remove("SelectedItem");
    }
    nbt
}
