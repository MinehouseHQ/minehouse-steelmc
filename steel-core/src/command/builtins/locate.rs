//! Vanilla `/locate`: structures, biomes and points of interest.

use std::{sync::Arc, time::Instant};

use steel_registry::biome::BiomeRef;
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::{BlockPos, Identifier, translations};
use text_components::{
    Modifier, TextComponent,
    format::Color,
    interactivity::{ClickEvent, HoverEvent},
};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        BiomeOrTag, CommandResultSuspension, CommandResultSuspensionPoll, CommandSource,
        PoiTypeOrTag, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        StructureOrTagKey, argument, literal,
    },
    registration::CommandRegistration,
};
use crate::poi::OccupationStatus;
use crate::world::biome_search::{BiomeSearch, BiomeSearchPoll};
use crate::world::structure_search::{FoundStructure, StructureSearch, StructureSearchPoll};

const MAX_STRUCTURE_SEARCH_RADIUS: i32 = 100;
/// Vanilla `locateBiome`: search radius and sample spacing, in blocks.
const BIOME_SEARCH_RADIUS: i32 = 6400;
const BIOME_SAMPLE_HORIZONTAL: i32 = 32;
const BIOME_SAMPLE_VERTICAL: i32 = 64;
/// Vanilla `POI_SEARCH_RADIUS`, in blocks.
const POI_SEARCH_RADIUS: i32 = 256;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("locate"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("locate")
        .then(
            literal("structure").then(
                argument("structure", SteelArgumentType::structure_or_tag_key())
                    .executes_suspended(start_structure_search),
            ),
        )
        .then(
            literal("biome").then(
                argument("biome", SteelArgumentType::biome_or_tag())
                    .executes_suspended(start_biome_search),
            ),
        )
        .then(
            literal("poi")
                .then(argument("poi", SteelArgumentType::poi_type_or_tag()).executes(locate_poi)),
        )
}

/// Vanilla `locatePoi`: the closest point of interest of the type or tag
/// within [`POI_SEARCH_RADIUS`], in any occupancy.
///
/// Steel's point of interest storage holds the loaded chunks, so unlike
/// vanilla the search does not load the chunks in range.
fn locate_poi(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let query = context.poi_type_or_tag("poi")?;
    let origin = BlockPos::from(context.source().position());
    let started_at = Instant::now();
    let closest = context
        .source()
        .world()
        .poi_storage
        .lock()
        .find_all_closest_first(
            &|type_id| {
                REGISTRY
                    .poi_types
                    .by_id(type_id)
                    .is_some_and(|poi_type| query.matches(poi_type))
            },
            |_| true,
            origin,
            POI_SEARCH_RADIUS,
            OccupationStatus::Any,
        )
        .into_iter()
        .next();
    let Some((pos, type_id)) = closest else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_LOCATE_POI_NOT_FOUND
                .message([TextComponent::from(query.as_printable())])
                .component(),
        ));
    };
    let name = match query {
        PoiTypeOrTag::Type(_) => query.as_printable(),
        PoiTypeOrTag::Tag(_) => {
            let found = REGISTRY
                .poi_types
                .by_id(type_id)
                .map_or_else(String::new, |poi_type| poi_type.key.to_string());
            format!("{} ({found})", query.as_printable())
        }
    };
    let distance = horizontal_distance(origin, pos);
    context.source().send_success(
        &translations::COMMANDS_LOCATE_POI_SUCCESS
            .message([
                TextComponent::from(name.clone()),
                coordinates_component(pos, "~"),
                TextComponent::from(distance.to_string()),
            ])
            .component(),
        false,
    );
    tracing::info!(
        "Locating element {} took {} ms",
        name,
        started_at.elapsed().as_millis()
    );
    Ok(distance)
}

fn start_structure_search(
    context: &SteelCommandContext<CommandSource>,
) -> Result<LocateStructureSearch, CommandSyntaxError> {
    let query = context.structure_or_tag_key("structure")?;
    let Some(structures) = query.resolve() else {
        return Err(invalid_structure(query));
    };
    if structures.is_empty() {
        return Err(structure_not_found(query));
    }

    let world = context.source().world();
    let structure_keys = structures
        .iter()
        .map(|structure| structure.key.clone())
        .collect::<Vec<_>>();
    let origin = BlockPos::from(context.source().position());
    let Some(search) = StructureSearch::new(
        Arc::clone(world),
        &structure_keys,
        origin,
        MAX_STRUCTURE_SEARCH_RADIUS,
        false,
    ) else {
        return Err(structure_not_found(query));
    };

    Ok(LocateStructureSearch {
        source: context.source().clone(),
        query: query.clone(),
        search,
        origin,
        started_at: Instant::now(),
    })
}

struct LocateStructureSearch {
    source: CommandSource,
    query: StructureOrTagKey,
    search: StructureSearch,
    origin: BlockPos,
    started_at: Instant,
}

impl CommandResultSuspension for LocateStructureSearch {
    fn poll(&mut self) -> CommandResultSuspensionPoll {
        match self.search.poll() {
            StructureSearchPoll::Pending => CommandResultSuspensionPoll::Pending,
            StructureSearchPoll::Found(found) => self.success_result(&found),
            StructureSearchPoll::NotFound => {
                CommandResultSuspensionPoll::Ready(Err(structure_not_found(&self.query)))
            }
            StructureSearchPoll::Cancelled => Self::cancelled_result(),
        }
    }

    fn cancel(&mut self) {
        self.search.cancel();
    }
}

impl LocateStructureSearch {
    fn success_result(&self, found: &FoundStructure) -> CommandResultSuspensionPoll {
        let distance = horizontal_distance(self.origin, found.pos);
        let structure_name = self.query.found_name(&found.structure);
        self.source.send_success(
            &locate_success_component(structure_name.clone(), found.pos, distance),
            false,
        );
        tracing::info!(
            "Locating element {} took {} ms",
            structure_name,
            self.started_at.elapsed().as_millis()
        );
        CommandResultSuspensionPoll::Ready(Ok(distance))
    }

    fn cancelled_result() -> CommandResultSuspensionPoll {
        CommandResultSuspensionPoll::Ready(Err(CommandSyntaxError::dynamic(
            "Structure search was cancelled",
        )))
    }
}

fn start_biome_search(
    context: &SteelCommandContext<CommandSource>,
) -> Result<LocateBiomeSearch, CommandSyntaxError> {
    let query = context.biome_or_tag("biome")?.clone();
    let origin = BlockPos::from(context.source().position());
    let Some(search) = BiomeSearch::new(
        Arc::clone(context.source().world()),
        origin,
        BIOME_SEARCH_RADIUS,
        BIOME_SAMPLE_HORIZONTAL,
        BIOME_SAMPLE_VERTICAL,
        |biome| query.matches(biome),
    ) else {
        return Err(biome_not_found(&query));
    };
    Ok(LocateBiomeSearch {
        source: context.source().clone(),
        query,
        search,
        origin,
        started_at: Instant::now(),
    })
}

struct LocateBiomeSearch {
    source: CommandSource,
    query: BiomeOrTag,
    search: BiomeSearch,
    origin: BlockPos,
    started_at: Instant,
}

impl CommandResultSuspension for LocateBiomeSearch {
    fn poll(&mut self) -> CommandResultSuspensionPoll {
        match self.search.poll() {
            BiomeSearchPoll::Pending => CommandResultSuspensionPoll::Pending,
            BiomeSearchPoll::Found(pos, biome) => {
                let distance = distance_3d(self.origin, pos);
                let name = biome_found_name(&self.query, biome);
                self.source.send_success(
                    &translations::COMMANDS_LOCATE_BIOME_SUCCESS
                        .message([
                            TextComponent::from(name.clone()),
                            coordinates_component(pos, &pos.y().to_string()),
                            TextComponent::from(distance.to_string()),
                        ])
                        .component(),
                    false,
                );
                tracing::info!(
                    "Locating element {} took {} ms",
                    name,
                    self.started_at.elapsed().as_millis()
                );
                CommandResultSuspensionPoll::Ready(Ok(distance))
            }
            BiomeSearchPoll::NotFound => {
                CommandResultSuspensionPoll::Ready(Err(biome_not_found(&self.query)))
            }
        }
    }
}

/// Vanilla `ResourceOrTagArgument.Result.asPrintable`.
fn biome_printable(query: &BiomeOrTag) -> String {
    match query {
        BiomeOrTag::Biome(biome) => biome.key.to_string(),
        BiomeOrTag::Tag(tag) => format!("#{tag}"),
    }
}

/// Vanilla `showLocateResult`: a tag names the biome it matched.
fn biome_found_name(query: &BiomeOrTag, found: BiomeRef) -> String {
    match query {
        BiomeOrTag::Biome(_) => biome_printable(query),
        BiomeOrTag::Tag(_) => format!("{} ({})", biome_printable(query), found.key),
    }
}

fn biome_not_found(query: &BiomeOrTag) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_LOCATE_BIOME_NOT_FOUND
            .message([TextComponent::from(biome_printable(query))])
            .component(),
    )
}

/// Vanilla `Mth.floor(Mth.sqrt((float) sourcePos.distSqr(foundPos)))`.
fn distance_3d(a: BlockPos, b: BlockPos) -> i32 {
    let dx = f64::from(a.x()) - f64::from(b.x());
    let dy = f64::from(a.y()) - f64::from(b.y());
    let dz = f64::from(a.z()) - f64::from(b.z());
    let squared = (dx * dx + dy * dy + dz * dz) as f32;
    (f64::from(squared).sqrt() as f32).floor() as i32
}

fn invalid_structure(query: &StructureOrTagKey) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_LOCATE_STRUCTURE_INVALID
            .message([TextComponent::from(query.as_printable())])
            .component(),
    )
}

fn structure_not_found(query: &StructureOrTagKey) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_LOCATE_STRUCTURE_NOT_FOUND
            .message([TextComponent::from(query.as_printable())])
            .component(),
    )
}

fn horizontal_distance(a: BlockPos, b: BlockPos) -> i32 {
    let dx = b.0.x.wrapping_sub(a.0.x);
    let dz = b.0.z.wrapping_sub(a.0.z);
    let squared = dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz));
    (f64::from(squared as f32).sqrt() as f32).floor() as i32
}

fn locate_success_component(structure_name: String, pos: BlockPos, distance: i32) -> TextComponent {
    translations::COMMANDS_LOCATE_STRUCTURE_SUCCESS
        .message([
            TextComponent::from(structure_name),
            coordinates_component(pos, "~"),
            TextComponent::from(distance.to_string()),
        ])
        .component()
}

fn coordinates_component(pos: BlockPos, displayed_y: &str) -> TextComponent {
    TextComponent::plain("[")
        .add_child(
            translations::CHAT_COORDINATES
                .message([
                    TextComponent::from(pos.0.x.to_string()),
                    TextComponent::from(displayed_y.to_owned()),
                    TextComponent::from(pos.0.z.to_string()),
                ])
                .component(),
        )
        .add_child(TextComponent::plain("]"))
        .color(Color::Green)
        .hover_event(HoverEvent::show_text(
            &translations::CHAT_COORDINATES_TOOLTIP,
        ))
        .click_event(ClickEvent::suggest_command(format!(
            "/tp @s {} {} {}",
            pos.0.x, displayed_y, pos.0.z
        )))
}

#[cfg(test)]
mod tests {
    use super::super::create_dispatcher;
    use super::*;
    use crate::command::{
        brigadier::{CommandDispatcher, NodeId},
        execution::SteelCommandRuntime,
    };
    use steel_registry::init_vanilla_registry;

    type Dispatcher = CommandDispatcher<CommandSource, SteelCommandRuntime>;

    fn child(dispatcher: &Dispatcher, parent: NodeId, name: &str) -> NodeId {
        let Some(children) = dispatcher.children(parent) else {
            panic!("parent node should exist");
        };
        let Some(child) = children.iter().copied().find(|child| {
            dispatcher
                .node(*child)
                .is_some_and(|node| node.name() == name)
        }) else {
            panic!("child {name} should exist");
        };
        child
    }

    #[test]
    fn locate_graph_exposes_the_structure_biome_and_poi_branches() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let locate = child(&dispatcher, dispatcher.root(), "locate");
        let structure = child(&dispatcher, locate, "structure");
        let target = child(&dispatcher, structure, "structure");

        assert_eq!(
            dispatcher
                .node(target)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::structure_or_tag_key())
        );
        let Some(target_node) = dispatcher.node(target) else {
            panic!("locate structure argument should exist");
        };
        assert!(target_node.is_executable());
        assert!(dispatcher.children(target).is_some_and(<[_]>::is_empty));
        let biome = child(&dispatcher, child(&dispatcher, locate, "biome"), "biome");
        assert_eq!(
            dispatcher.node(biome).and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::biome_or_tag())
        );
        let poi = child(&dispatcher, child(&dispatcher, locate, "poi"), "poi");
        assert_eq!(
            dispatcher.node(poi).and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::poi_type_or_tag())
        );
        assert_eq!(dispatcher.children(locate).map(<[_]>::len), Some(3));
    }

    #[test]
    fn locate_coordinates_component_matches_vanilla_interactivity() {
        let component = coordinates_component(BlockPos::new(12, 0, -34), "~");

        assert_eq!(component.format.color, Some(Color::Green));
        assert!(matches!(
            component.interactions.click,
            Some(ClickEvent::SuggestCommand { ref command })
                if command.as_ref() == "/tp @s 12 ~ -34"
        ));
        assert!(matches!(
            component.interactions.hover,
            Some(HoverEvent::ShowText { .. })
        ));
    }

    #[test]
    fn horizontal_distance_matches_vanillas_wrapping_int_and_float_math() {
        assert_eq!(
            horizontal_distance(BlockPos::new(0, 0, 0), BlockPos::new(3, 100, 4)),
            5
        );
        assert_eq!(
            horizontal_distance(
                BlockPos::new(-30_000_000, 0, 0),
                BlockPos::new(30_000_000, 0, 0)
            ),
            36_907
        );
    }
}
