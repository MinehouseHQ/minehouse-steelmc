//! Vanilla `DataAccessor` and its entity, block and storage providers.

use std::io::Cursor;
use std::sync::Arc;

use simdnbt::borrow::read_compound;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_utils::{BlockPos, Identifier, nbt::NbtPath, text::command_nbt_component, translations};
use text_components::TextComponent;

use super::super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    storage::CommandStorage,
};
use super::super::execute::{
    invalid_block_data_source, loaded_block_position, source_command_storage,
};
use crate::block_entity::SharedBlockEntity;
use crate::entity::SharedEntity;
use crate::world::World;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

/// Which of vanilla's `ALL_PROVIDERS` a node reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AccessorKind {
    Entity,
    Block,
    Storage,
}

impl AccessorKind {
    /// Vanilla's provider order.
    pub(super) const ALL: [Self; 3] = [Self::Entity, Self::Block, Self::Storage];

    const fn literal(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Block => "block",
            Self::Storage => "storage",
        }
    }

    /// Vanilla `ArgProvider.wrap`: `<builder> <kind> <argument> ...`.
    pub(super) fn wrap(
        self,
        builder: Builder,
        argument_name: &'static str,
        then: impl FnOnce(Builder) -> Builder,
    ) -> Builder {
        let argument_node = match self {
            Self::Entity => argument(argument_name, SteelArgumentType::entity()),
            Self::Block => argument(argument_name, SteelArgumentType::block_pos()),
            Self::Storage => argument(argument_name, SteelArgumentType::storage_key()),
        };
        builder.then(literal(self.literal()).then(then(argument_node)))
    }
}

/// A resolved vanilla `DataAccessor`.
pub(super) enum DataAccessor<'a> {
    Entity(SharedEntity),
    Block {
        block_entity: SharedBlockEntity,
        pos: BlockPos,
        world: Arc<World>,
    },
    Storage {
        storage: &'a CommandStorage,
        id: Identifier,
    },
}

impl<'a> DataAccessor<'a> {
    /// The provider's `access`: resolves the argument named `argument_name`.
    pub(super) fn access(
        context: &'a SteelCommandContext<CommandSource>,
        kind: AccessorKind,
        argument_name: &str,
    ) -> Result<Self, CommandSyntaxError> {
        match kind {
            AccessorKind::Entity => context.entity(argument_name).map(Self::Entity),
            AccessorKind::Block => {
                let pos = loaded_block_position(context, argument_name)?;
                let world = Arc::clone(context.source().world());
                let block_entity = world
                    .get_block_entity(pos)
                    .ok_or_else(invalid_block_data_source)?;
                Ok(Self::Block {
                    block_entity,
                    pos,
                    world,
                })
            }
            AccessorKind::Storage => Ok(Self::Storage {
                storage: source_command_storage(context)?,
                id: context.identifier(argument_name)?.clone(),
            }),
        }
    }

    /// Vanilla `getData`.
    pub(super) fn data(&self) -> NbtCompound {
        match self {
            Self::Entity(entity) => entity.nbt_for_data_compare(),
            Self::Block { block_entity, .. } => block_entity.save_with_full_metadata(),
            Self::Storage { storage, id } => storage.get(id),
        }
    }

    /// Vanilla `setData`.
    ///
    /// An entity loads through `apply_spawn_data`, Steel's `Entity.load` for a
    /// live entity, which keeps the UUID as vanilla restores it.
    pub(super) fn set_data(&self, data: NbtCompound) -> Result<(), CommandSyntaxError> {
        match self {
            Self::Entity(entity) => {
                if entity.as_player().is_some() {
                    return Err(CommandSyntaxError::dynamic(TextComponent::from(
                        &translations::COMMANDS_DATA_ENTITY_INVALID,
                    )));
                }
                let mut bytes = Vec::new();
                data.write(&mut bytes);
                let borrowed =
                    read_compound(&mut Cursor::new(bytes.as_slice())).map_err(|error| {
                        CommandSyntaxError::dynamic(format!("Failed to load entity data: {error}"))
                    })?;
                entity.apply_spawn_data((&borrowed).into());
                Ok(())
            }
            Self::Block {
                block_entity,
                pos,
                world,
            } => {
                // Vanilla's problem reporter logs a bad field and keeps going.
                if let Err(error) = block_entity.load_with_owned_components(&data) {
                    log::warn!("Failed to load block entity data at {pos:?}: {error}");
                }
                block_entity.set_changed();
                world.send_block_updated(*pos);
                Ok(())
            }
            Self::Storage { storage, id } => {
                storage.set(id.clone(), data);
                Ok(())
            }
        }
    }

    /// Vanilla `getModifiedSuccess`.
    pub(super) fn modified_success(&self) -> TextComponent {
        match self {
            Self::Entity(entity) => translations::COMMANDS_DATA_ENTITY_MODIFIED
                .message([entity.display_name()])
                .component(),
            Self::Block { pos, .. } => translations::COMMANDS_DATA_BLOCK_MODIFIED
                .message(block_coordinates(*pos))
                .component(),
            Self::Storage { id, .. } => translations::COMMANDS_DATA_STORAGE_MODIFIED
                .message([TextComponent::plain(id.to_string())])
                .component(),
        }
    }

    /// Vanilla `getPrintSuccess(Tag)`.
    pub(super) fn print_success(&self, data: &NbtTag) -> TextComponent {
        let pretty = command_nbt_component(data, false);
        match self {
            Self::Entity(entity) => translations::COMMANDS_DATA_ENTITY_QUERY
                .message([entity.display_name(), pretty])
                .component(),
            Self::Block { pos, .. } => {
                let [x, y, z] = block_coordinates(*pos);
                translations::COMMANDS_DATA_BLOCK_QUERY
                    .message([x, y, z, pretty])
                    .component()
            }
            Self::Storage { id, .. } => translations::COMMANDS_DATA_STORAGE_QUERY
                .message([TextComponent::plain(id.to_string()), pretty])
                .component(),
        }
    }

    /// Vanilla `getPrintSuccess(NbtPath, double, int)`.
    pub(super) fn print_path_success(
        &self,
        path: &NbtPath,
        scale: f64,
        value: i32,
    ) -> TextComponent {
        let path = TextComponent::plain(path.as_str().to_owned());
        let scale = TextComponent::plain(format!("{scale:.2}"));
        let value = TextComponent::plain(value.to_string());
        match self {
            Self::Entity(entity) => translations::COMMANDS_DATA_ENTITY_GET
                .message([path, entity.display_name(), scale, value])
                .component(),
            Self::Block { pos, .. } => {
                let [x, y, z] = block_coordinates(*pos);
                translations::COMMANDS_DATA_BLOCK_GET
                    .message([path, x, y, z, scale, value])
                    .component()
            }
            Self::Storage { id, .. } => translations::COMMANDS_DATA_STORAGE_GET
                .message([path, TextComponent::plain(id.to_string()), scale, value])
                .component(),
        }
    }
}

fn block_coordinates(pos: BlockPos) -> [TextComponent; 3] {
    [pos.x(), pos.y(), pos.z()].map(|value| TextComponent::plain(value.to_string()))
}
