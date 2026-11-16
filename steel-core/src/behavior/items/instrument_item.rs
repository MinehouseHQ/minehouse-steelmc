//! Vanilla `InstrumentItem`: the goat horn.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::INSTRUMENT;
use steel_registry::instrument::InstrumentValue;
use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_game_events;
use steel_utils::types::InteractionHand;

use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::behavior::{ItemBehavior, ItemUseAnimation};
use crate::entity::{Entity, LivingEntity};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::Player;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `InstrumentItem`.
#[item_behavior]
pub struct InstrumentItem;

/// The stack's instrument, vanilla `InstrumentItem.getInstrument`.
fn instrument(stack: &ItemStack) -> Option<InstrumentValue> {
    stack
        .get(INSTRUMENT)
        .map(|component| component.value().clone())
}

/// Vanilla `Mth.floor(useDuration * 20.0F)`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "vanilla floors the float tick count"
)]
fn duration_ticks(instrument: &InstrumentValue) -> i32 {
    (instrument.use_duration() * 20.0).floor() as i32
}

impl ItemBehavior for InstrumentItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let stack = context.inv.with_item(|stack| stack.clone());
        let Some(instrument) = instrument(&stack) else {
            return InteractionResult::Fail;
        };
        let player = context.player;
        player.start_using_item(context.hand);
        play(context.world, player, &instrument);
        let durability_damage = instrument.durability_damage();
        if durability_damage > 0 {
            let infinite = player.has_infinite_materials();
            let broke = context
                .inv
                .with_item(|stack| stack.hurt_and_break(durability_damage, infinite));
            if broke {
                let slot = match context.hand {
                    InteractionHand::MainHand => EquipmentSlot::MainHand,
                    InteractionHand::OffHand => EquipmentSlot::OffHand,
                };
                player.on_equipped_item_broken(stack.item(), slot);
            }
        }
        if instrument.use_duration() > 0.0 {
            player.add_item_cooldown(&stack, duration_ticks(&instrument));
        }
        player.award_stat(&vanilla_stat_types::ITEM_USED, stack.item());
        InteractionResult::Consume
    }

    fn get_use_duration(&self, stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        instrument(stack).map_or(0, |instrument| duration_ticks(&instrument))
    }

    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::TootHorn
    }
}

/// Vanilla `InstrumentItem.play`: everyone but the player hears the horn
/// follow them; the player's client plays it locally.
fn play(world: &Arc<World>, player: &Player, instrument: &InstrumentValue) {
    let volume = instrument.range() / 16.0;
    world.play_sound_from_entity(
        instrument.sound_event(),
        SoundSource::Records,
        player.id(),
        player.position(),
        volume,
        1.0,
        Some(player.id()),
    );
    world.game_event_at(
        &vanilla_game_events::INSTRUMENT_PLAY,
        player.position(),
        &GameEventContext::new(Some(player), None),
    );
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_items};
    use steel_utils::types::GameType;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::player::game_mode;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};

    #[test]
    fn goat_horn_toots_and_goes_on_cooldown() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("goat_horn_use");
        let world = &fixture.world;
        let player = TestPlayerBuilder::new(Arc::clone(world), "Horn", 61).build();
        player.set_game_mode(GameType::Survival);
        let horn = ItemStack::new(&vanilla_items::GOAT_HORN);
        player.inventory.lock().set_selected_item(horn.clone());

        let result = game_mode::use_item(&player, world, InteractionHand::MainHand);

        assert_eq!(result, InteractionResult::Consume);
        assert_eq!(
            player.active_item_use_hand(),
            Some(InteractionHand::MainHand)
        );
        assert!(player.is_item_on_cooldown(&horn));
        let instrument = instrument(&horn).expect("goat horns default to an instrument");
        assert_eq!(
            InstrumentItem.get_use_duration(&horn, player.as_ref()),
            duration_ticks(&instrument)
        );
        assert_eq!(duration_ticks(&instrument), 140);
    }
}
