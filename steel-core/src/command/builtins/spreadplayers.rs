//! Vanilla `SpreadPlayersCommand`.
//!
//! Vanilla reads the blocks under each candidate position straight from the
//! level, loading chunks as it goes. Steel's command thread can't load a chunk
//! in place, so the search suspends the command while it requests the chunk of
//! the next position to check, and picks up where it stopped once the chunk is
//! ready. The search itself runs the same steps in the same order.

use std::sync::Arc;

use glam::DVec3;
use rand::RngExt as _;
use rustc_hash::{FxHashMap, FxHashSet};
use steel_protocol::packets::game::RelativeMovement;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::{BlockPos, ChunkPos, Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandResultSuspension, CommandResultSuspensionPoll, CommandSource, SteelArgumentType,
        SteelCommandContext, SteelCommandRuntime, argument, literal,
    },
    registration::CommandRegistration,
};
use super::teleport::teleport_entity_to;
use crate::{
    chunk::{
        chunk_request::{ChunkRequestHandle, ChunkRequestState, ChunkTicketKind},
        status::ChunkStatus,
    },
    entity::SharedEntity,
    portal::{TeleportPostTransition, TeleportTransition},
    scoreboard::TeamRules,
    world::{LevelReader as _, World},
};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;
type Context = SteelCommandContext<CommandSource>;

/// Vanilla `MAX_ITERATION_COUNT`.
const MAX_ITERATION_COUNT: i32 = 10_000;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("spreadplayers"), |_| command())
}

fn command() -> Builder {
    let targets = |max_height_from_argument: bool| {
        argument("respectTeams", ArgumentType::bool()).then(
            argument("targets", SteelArgumentType::entities()).executes_suspended(
                move |context: &Context| {
                    let max_height = if max_height_from_argument {
                        context.integer("maxHeight")?
                    } else {
                        context.source().world().max_y_exclusive()
                    };
                    start_spread(context, max_height)
                },
            ),
        )
    };
    literal("spreadplayers").then(
        argument("center", SteelArgumentType::vec2(true)).then(
            argument("spreadDistance", ArgumentType::float(0.0, f32::MAX)).then(
                argument("maxRange", ArgumentType::float(1.0, f32::MAX))
                    .then(targets(false))
                    .then(
                        literal("under").then(
                            argument("maxHeight", ArgumentType::integer(i32::MIN, i32::MAX))
                                .then(targets(true)),
                        ),
                    ),
            ),
        ),
    )
}

/// Vanilla `spreadPlayers` up to the search, which [`Spread`] runs.
fn start_spread(context: &Context, max_height: i32) -> Result<Spread, CommandSyntaxError> {
    let source = context.source();
    let world = Arc::clone(source.world());
    let min_y = world.min_y();
    if max_height < min_y {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SPREADPLAYERS_FAILED_INVALID_HEIGHT
                .message([
                    TextComponent::plain(max_height.to_string()),
                    TextComponent::plain(min_y.to_string()),
                ])
                .component(),
        ));
    }
    let position = context.coordinates("center")?.position(source);
    // Vanilla's `Vec2` holds floats, and the bounds are float sums.
    let center = (position.x as f32, position.z as f32);
    let spread_distance = context.float("spreadDistance")?;
    let max_range = context.float("maxRange")?;
    let respect_teams = context.boolean("respectTeams")?;
    let targets = context.entities("targets")?;
    let bounds = Bounds {
        min_x: f64::from(center.0 - max_range),
        min_z: f64::from(center.1 - max_range),
        max_x: f64::from(center.0 + max_range),
        max_z: f64::from(center.1 + max_range),
    };
    let count = if respect_teams {
        number_of_teams(&targets)
    } else {
        targets.len()
    };
    let positions = (0..count).map(|_| bounds.random_position()).collect();
    Ok(Spread {
        source: source.clone(),
        world,
        targets,
        center,
        spread_distance: f64::from(spread_distance),
        bounds,
        max_height,
        respect_teams,
        positions,
        iteration: 0,
        has_collisions: true,
        min_distance: f64::from(f32::MAX),
        safety_index: None,
        chunks: FxHashMap::default(),
    })
}

/// Vanilla `getNumberOfTeams`: players count by team, every other entity
/// shares the no-team slot.
fn number_of_teams(targets: &[SharedEntity]) -> usize {
    let mut teams: Vec<Option<TeamRules>> = Vec::new();
    for target in targets {
        let team = if target.as_player().is_some() {
            target.team()
        } else {
            None
        };
        if !teams
            .iter()
            .any(|known| same_team(known.as_ref(), team.as_ref()))
        {
            teams.push(team);
        }
    }
    teams.len()
}

/// Vanilla's `Team` hash key equality, `null` included.
const fn same_team(first: Option<&TeamRules>, second: Option<&TeamRules>) -> bool {
    match (first, second) {
        (None, None) => true,
        (Some(first), Some(second)) => first.is_same_team(second),
        _ => false,
    }
}

#[derive(Clone, Copy)]
struct Bounds {
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
}

impl Bounds {
    /// Vanilla `Position.randomize`.
    fn random_position(&self) -> Position {
        Position {
            x: next_double(self.min_x, self.max_x),
            z: next_double(self.min_z, self.max_z),
        }
    }
}

/// Vanilla `Mth.nextDouble` on an unseeded random.
fn next_double(min: f64, max: f64) -> f64 {
    if min >= max {
        min
    } else {
        rand::rng().random::<f64>() * (max - min) + min
    }
}

/// Vanilla `SpreadPlayersCommand.Position`.
#[derive(Clone, Copy, Default, PartialEq)]
struct Position {
    x: f64,
    z: f64,
}

impl Position {
    fn dist(self, target: Self) -> f64 {
        let dx = self.x - target.x;
        let dz = self.z - target.z;
        (dx * dx + dz * dz).sqrt()
    }

    fn length(self) -> f64 {
        (self.x * self.x + self.z * self.z).sqrt()
    }

    /// Vanilla `clamp`: whether the position had to move into the bounds.
    fn clamp(&mut self, bounds: &Bounds) -> bool {
        let mut changed = false;
        if self.x < bounds.min_x {
            self.x = bounds.min_x;
            changed = true;
        } else if self.x > bounds.max_x {
            self.x = bounds.max_x;
            changed = true;
        }
        if self.z < bounds.min_z {
            self.z = bounds.min_z;
            changed = true;
        } else if self.z > bounds.max_z {
            self.z = bounds.max_z;
            changed = true;
        }
        changed
    }

    const fn block_x(self) -> i32 {
        self.x.floor() as i32
    }

    const fn block_z(self) -> i32 {
        self.z.floor() as i32
    }

    const fn chunk(self) -> ChunkPos {
        ChunkPos::new(self.block_x() >> 4, self.block_z() >> 4)
    }

    /// Vanilla `getSpawnY`: the top of the highest block with two air blocks
    /// above it, searching down from `max_height`.
    fn spawn_y(self, world: &World, max_height: i32) -> i32 {
        let (x, z) = (self.block_x(), self.block_z());
        let is_air = |y: i32| world.get_block_state(BlockPos::new(x, y, z)).is_air();
        let mut y = max_height + 1;
        let mut air_2_above = is_air(y);
        y -= 1;
        let mut air_1_above = is_air(y);
        while y > world.min_y() {
            y -= 1;
            let current_is_air = is_air(y);
            if !current_is_air && air_1_above && air_2_above {
                return y + 1;
            }
            air_2_above = air_1_above;
            air_1_above = current_is_air;
        }
        max_height + 1
    }

    /// Vanilla `isSafe`: below the height limit, on a block entities can
    /// teleport onto that is not a liquid.
    fn is_safe(self, world: &World, max_height: i32) -> bool {
        let y = self.spawn_y(world, max_height) - 1;
        let state = world.get_block_state(BlockPos::new(self.block_x(), y, self.block_z()));
        let block = state.get_block();
        y < max_height && !block.config.liquid && block.has_tag(&BlockTag::ENTITIES_CAN_TELEPORT_TO)
    }
}

/// The running search: vanilla `spreadPositions` and `setPlayerPositions`.
struct Spread {
    source: CommandSource,
    world: Arc<World>,
    targets: Vec<SharedEntity>,
    center: (f32, f32),
    spread_distance: f64,
    bounds: Bounds,
    max_height: i32,
    respect_teams: bool,
    positions: Vec<Position>,
    /// Vanilla's loop counter, `iteration`.
    iteration: i32,
    has_collisions: bool,
    min_distance: f64,
    /// The next position of the safety pass, while one is running.
    safety_index: Option<usize>,
    /// Chunk requests for the positions the safety pass reads.
    chunks: FxHashMap<ChunkPos, ChunkRequestHandle>,
}

/// Whether the search can go on or waits for a chunk.
enum ChunkReady {
    Ready,
    Pending,
    Cancelled,
}

impl Spread {
    /// One pass of vanilla's collision loop body, which moves every
    /// position away from its neighbors closer than the spread distance.
    fn collision_pass(&mut self) {
        self.has_collisions = false;
        self.min_distance = f64::from(f32::MAX);
        for index in 0..self.positions.len() {
            let mut position = self.positions[index];
            let mut neighbour_count = 0_i32;
            let mut average = Position::default();
            for (other_index, neighbour) in self.positions.iter().enumerate() {
                if index == other_index {
                    continue;
                }
                let dist = position.dist(*neighbour);
                self.min_distance = dist.min(self.min_distance);
                if dist < self.spread_distance {
                    neighbour_count += 1;
                    average.x += neighbour.x - position.x;
                    average.z += neighbour.z - position.z;
                }
            }
            if neighbour_count > 0 {
                average.x /= f64::from(neighbour_count);
                average.z /= f64::from(neighbour_count);
                let length = average.length();
                if length > 0.0 {
                    position.x -= average.x / length;
                    position.z -= average.z / length;
                } else {
                    position = self.bounds.random_position();
                }
                self.has_collisions = true;
            }
            if position.clamp(&self.bounds) {
                self.has_collisions = true;
            }
            self.positions[index] = position;
        }
    }

    /// Makes sure the chunk under `position` can be read.
    fn chunk_ready(&mut self, position: Position) -> ChunkReady {
        let chunk = position.chunk();
        if self
            .world
            .chunk_map
            .with_full_chunk(chunk, |_| ())
            .is_some()
        {
            return ChunkReady::Ready;
        }
        let request = self.chunks.entry(chunk).or_insert_with(|| {
            self.world
                .chunk_map
                .request_chunk(chunk, ChunkStatus::Full, ChunkTicketKind::Command)
        });
        match request.poll() {
            ChunkRequestState::Ready => ChunkReady::Ready,
            ChunkRequestState::Pending { .. } => ChunkReady::Pending,
            ChunkRequestState::Cancelled => ChunkReady::Cancelled,
        }
    }

    /// Vanilla's safety pass, run once a collision pass moved nothing:
    /// unsafe positions jump somewhere random. `None` while a chunk loads.
    fn safety_pass(&mut self) -> Option<Result<(), CommandSyntaxError>> {
        let mut index = self.safety_index.unwrap_or(0);
        while index < self.positions.len() {
            let position = self.positions[index];
            match self.chunk_ready(position) {
                ChunkReady::Ready => {}
                ChunkReady::Pending => {
                    self.safety_index = Some(index);
                    return None;
                }
                ChunkReady::Cancelled => {
                    return Some(Err(CommandSyntaxError::dynamic(
                        "Chunk loading was cancelled",
                    )));
                }
            }
            if !position.is_safe(&self.world, self.max_height) {
                self.positions[index] = self.bounds.random_position();
                self.has_collisions = true;
            }
            index += 1;
        }
        self.safety_index = None;
        // Keep only the requests for chunks the positions still stand in.
        let wanted: FxHashSet<ChunkPos> = self.positions.iter().map(|p| p.chunk()).collect();
        self.chunks.retain(|chunk, _| wanted.contains(chunk));
        Some(Ok(()))
    }

    /// Vanilla `spreadPositions`' loop. `None` while a chunk loads.
    fn spread_positions(&mut self) -> Option<Result<(), CommandSyntaxError>> {
        while self.iteration < MAX_ITERATION_COUNT && self.has_collisions {
            if self.safety_index.is_none() {
                self.collision_pass();
                if self.has_collisions {
                    self.iteration += 1;
                    continue;
                }
            }
            if let Err(error) = self.safety_pass()? {
                return Some(Err(error));
            }
            self.iteration += 1;
        }
        #[expect(
            clippy::float_cmp,
            reason = "vanilla checks for the untouched Float.MAX_VALUE sentinel"
        )]
        if self.min_distance == f64::from(f32::MAX) {
            self.min_distance = 0.0;
        }
        if self.iteration >= MAX_ITERATION_COUNT {
            let message = if self.respect_teams {
                &translations::COMMANDS_SPREADPLAYERS_FAILED_TEAMS
            } else {
                &translations::COMMANDS_SPREADPLAYERS_FAILED_ENTITIES
            };
            return Some(Err(CommandSyntaxError::dynamic(
                message
                    .message(self.summary_arguments(self.min_distance))
                    .component(),
            )));
        }
        Some(Ok(()))
    }

    /// The count, center and distance arguments of vanilla's messages.
    fn summary_arguments(&self, distance: f64) -> [TextComponent; 4] {
        [
            TextComponent::plain(self.positions.len().to_string()),
            TextComponent::plain(java_float_string(self.center.0)),
            TextComponent::plain(java_float_string(self.center.1)),
            TextComponent::plain(format!("{distance:.2}")),
        ]
    }

    /// Vanilla `setPlayerPositions`: teleports every target and returns the
    /// average distance to the closest other position.
    fn set_player_positions(&self) -> f64 {
        let mut average_distance = 0.0;
        let mut next_index = 0;
        let mut team_positions: Vec<(Option<TeamRules>, usize)> = Vec::new();
        for target in &self.targets {
            let index = if self.respect_teams {
                let team = if target.as_player().is_some() {
                    target.team()
                } else {
                    None
                };
                if let Some((_, index)) = team_positions
                    .iter()
                    .find(|(known, _)| same_team(known.as_ref(), team.as_ref()))
                {
                    *index
                } else {
                    team_positions.push((team, next_index));
                    next_index += 1;
                    next_index - 1
                }
            } else {
                next_index += 1;
                next_index - 1
            };
            let position = self.positions[index];
            let destination = DVec3::new(
                f64::from(position.block_x()) + 0.5,
                f64::from(position.spawn_y(&self.world, self.max_height)),
                f64::from(position.block_z()) + 0.5,
            );
            teleport_entity_to(
                target,
                &TeleportTransition {
                    target_world: Arc::clone(&self.world),
                    position: destination,
                    rotation: target.rotation(),
                    velocity: DVec3::ZERO,
                    relatives: RelativeMovement::NONE,
                    portal_cooldown: 0,
                    as_passenger: false,
                    post_transition: TeleportPostTransition::do_nothing(),
                },
            );
            let closest = self
                .positions
                .iter()
                .enumerate()
                .filter(|(other_index, _)| *other_index != index)
                .map(|(_, other)| position.dist(*other))
                .fold(f64::MAX, f64::min);
            average_distance += closest;
        }
        if self.targets.len() < 2 {
            0.0
        } else {
            average_distance / self.targets.len() as f64
        }
    }
}

impl CommandResultSuspension for Spread {
    fn poll(&mut self) -> CommandResultSuspensionPoll {
        match self.spread_positions() {
            None => CommandResultSuspensionPoll::Pending,
            Some(Err(error)) => CommandResultSuspensionPoll::Ready(Err(error)),
            Some(Ok(())) => {
                let distance = self.set_player_positions();
                self.chunks.clear();
                let message = if self.respect_teams {
                    &translations::COMMANDS_SPREADPLAYERS_SUCCESS_TEAMS
                } else {
                    &translations::COMMANDS_SPREADPLAYERS_SUCCESS_ENTITIES
                };
                self.source.send_success(
                    &message
                        .message(self.summary_arguments(distance))
                        .component(),
                    true,
                );
                CommandResultSuspensionPoll::Ready(Ok(
                    i32::try_from(self.positions.len()).unwrap_or(i32::MAX)
                ))
            }
        }
    }

    fn cancel(&mut self) {
        self.chunks.clear();
    }
}

/// Java's `Float.toString`, which vanilla's message arguments use for the
/// center: plain decimals between 10^-3 and 10^7, scientific otherwise.
fn java_float_string(value: f32) -> String {
    let magnitude = value.abs();
    if value == 0.0 || (1.0e-3..1.0e7).contains(&magnitude) || !value.is_finite() {
        let plain = format!("{value:?}");
        return match plain.as_str() {
            "inf" => "Infinity".to_owned(),
            "-inf" => "-Infinity".to_owned(),
            "NaN" => "NaN".to_owned(),
            _ => plain,
        };
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let mantissa = if mantissa.contains('.') {
        mantissa.to_owned()
    } else {
        format!("{mantissa}.0")
    };
    format!("{mantissa}E{exponent}")
}

#[cfg(test)]
mod tests {
    use super::java_float_string;

    #[test]
    fn floats_print_like_java() {
        assert_eq!(java_float_string(0.0), "0.0");
        assert_eq!(java_float_string(-12.5), "-12.5");
        assert_eq!(java_float_string(100.0), "100.0");
        assert_eq!(java_float_string(1.0e7), "1.0E7");
        assert_eq!(java_float_string(2.5e-4), "2.5E-4");
    }
}
