//! "● LIVE" over the head of a player who streams on Twitch right now, above the name (the
//! Fabric `LiveOverhead`). Who is live comes from [`twitch`].
//!
//! The label is a text display that rides the player. Other players see it, the streamer
//! does not, and it is empty while the player is invisible, a spectator or sneaking, like
//! the name. A label without its player is removed, also one a stopped server left behind.

use std::{collections::hash_map::Entry, sync::Arc};

use rustc_hash::FxHashMap;
use steel_core::{
    entity::{
        Entity as _, RemovalReason, SharedEntity, entities::TextDisplayEntity, next_entity_id,
    },
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_registry::{entity_data::Vector3f, vanilla_entities};
use steel_utils::{WorldAabb, types::GameType};
use text_components::TextComponent;
use uuid::Uuid;

use crate::twitch;

/// The scoreboard tag of a label.
const LABEL_TAG: &str = "minehouse_live";
const CHECK_EVERY_TICKS: u64 = 20;
/// Every how many ticks labels left behind are looked for.
const SWEEP_EVERY_TICKS: u64 = 200;
/// How far around a player they are looked for, blocks.
const SWEEP_RADIUS: f64 = 32.0;
/// A passenger of a player is at the top of their box, and vanilla draws the name half a
/// block above it; this is one line higher.
const ABOVE_HEAD: f32 = 0.8;
const SCALE: f32 = 0.8;
/// Vanilla `Display.BillboardConstraints.CENTER`.
const BILLBOARD_CENTER: i8 = 3;
/// Vanilla `TextDisplay.FLAG_SHADOW`.
const FLAG_SHADOW: i8 = 1;
/// Vanilla `Brightness(15, 15).pack()`.
const FULL_BRIGHT: i32 = 15 << 4 | 15 << 20;
/// A quarter opaque black.
const BACKGROUND: i32 = 0x4000_0000;

/// The label of one player.
struct Label {
    display: Arc<TextDisplayEntity>,
    /// The player it rides: a respawned player is a new one.
    player: usize,
    /// Whether it shows the text now.
    shown: bool,
}

/// Whether the label of a player in this state shows its text.
const fn shows(invisible: bool, spectator: bool, sneaking: bool) -> bool {
    !(invisible || spectator || sneaking)
}

fn attach(player: &Arc<Player>) -> Option<Arc<TextDisplayEntity>> {
    let world = player.get_world();
    let display = Arc::new(TextDisplayEntity::new(
        &vanilla_entities::TEXT_DISPLAY,
        next_entity_id(),
        player.position(),
        Arc::downgrade(&world),
    ));
    display.edit(|data| {
        data.background_color.set(BACKGROUND);
        data.style_flags.set(FLAG_SHADOW);
        let display = &mut data.display;
        display.billboard_render_constraints.set(BILLBOARD_CENTER);
        display.brightness_override.set(FULL_BRIGHT);
        display.view_range.set(0.5);
        display.scale.set(Vector3f {
            x: SCALE,
            y: SCALE,
            z: SCALE,
        });
        display.translation.set(Vector3f {
            x: 0.0,
            y: ABOVE_HEAD,
            z: 0.0,
        });
    });
    display.hide_from(Some(player.gameprofile.id()));
    display.add_tag(LABEL_TAG.to_owned());
    let entity: SharedEntity = Arc::clone(&display) as SharedEntity;
    if world.try_add_entity(Arc::clone(&entity)).is_err() {
        return None;
    }
    let vehicle: SharedEntity = Arc::clone(player) as SharedEntity;
    if !entity.start_riding(&vehicle, true) {
        entity.set_removed(RemovalReason::Discarded);
        return None;
    }
    Some(display)
}

/// Keeps the labels over the live players.
#[derive(Default)]
struct LiveLabels {
    labels: FxHashMap<Uuid, Label>,
}

impl LiveLabels {
    fn check(&mut self, server: &Server) {
        let players = server.get_players();
        let text = twitch::overhead();
        // Labels of who left, respawned, stopped streaming or lost theirs.
        self.labels.retain(|uuid, label| {
            let stays = text.is_some()
                && twitch::is_live(*uuid)
                && !label.display.is_removed()
                && label.display.is_passenger()
                && players
                    .iter()
                    .any(|player| Arc::as_ptr(player).addr() == label.player);
            if !stays {
                label.display.set_removed(RemovalReason::Discarded);
            }
            stays
        });
        let Some(text) = text else {
            return;
        };
        for player in &players {
            let id = player.gameprofile.id();
            if !twitch::is_live(id) {
                continue;
            }
            let label = match self.labels.entry(id) {
                Entry::Occupied(label) => label.into_mut(),
                Entry::Vacant(free) => {
                    let Some(display) = attach(player) else {
                        continue;
                    };
                    free.insert(Label {
                        display,
                        player: Arc::as_ptr(player).addr(),
                        shown: false,
                    })
                }
            };
            let shown = shows(
                player.is_invisible(),
                player.game_mode() == GameType::Spectator,
                player.is_crouching(),
            );
            if shown != label.shown {
                label.shown = shown;
                label.display.set_text(if shown {
                    text.clone()
                } else {
                    TextComponent::plain("")
                });
            }
        }
    }

    /// Removes the labels near players that ride nobody: a stopped server saved them with
    /// their chunk.
    fn sweep(server: &Server) {
        for player in server.get_players() {
            let side = SWEEP_RADIUS * 2.0;
            let around = WorldAabb::of_size(player.position(), side, side, side);
            let left = player
                .get_world()
                .get_entities_in_aabb_matching(&around, |entity| {
                    !entity.is_passenger() && entity.tags().iter().any(|tag| tag == LABEL_TAG)
                });
            for label in left {
                label.set_removed(RemovalReason::Discarded);
            }
        }
    }
}

impl ServerJob for LiveLabels {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        if context.tick_count.is_multiple_of(CHECK_EVERY_TICKS) {
            self.check(&server);
        }
        if context.tick_count.is_multiple_of(SWEEP_EVERY_TICKS) {
            Self::sweep(&server);
        }
        JobPoll::Pending
    }
}

/// Starts keeping the labels.
pub(super) fn start(server: &Server) {
    server.jobs.spawn(LiveLabels::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_hides_with_the_name() {
        assert!(shows(false, false, false));
        assert!(!shows(true, false, false));
        assert!(!shows(false, true, false));
        assert!(!shows(false, false, true));
    }
}
