//! The recorded player in their own replay. A client is never sent packets about itself, so
//! a recording of what it was sent has everybody but the player. This writes the missing
//! part: the player appears once the client has loaded the world (after joining, dying or
//! changing worlds) and is moved every tick they moved. What the players around are told
//! about them (the items held, the pose, the swings) the core adds itself.

use std::sync::Arc;

use glam::DVec3;
use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::{
    entity::{Entity as _, EntityTracker, SharedEntity},
    player::{Player, connection::NetworkConnection as _},
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_protocol::{
    packet_traits::{ClientPacket, EncodedPacket},
    packets::game::{CEntityPositionSync, CRotateHead, PositionPath, to_angle_byte},
    utils::ConnectionProtocol,
};
use steel_utils::locks::SyncMutex;
use uuid::Uuid;

/// The players being recorded.
pub(super) type Recorded = Arc<SyncMutex<FxHashSet<Uuid>>>;

/// Where a player was when the recording was last told.
#[derive(Clone, Copy, PartialEq)]
struct Place {
    position: DVec3,
    rotation: (f32, f32),
    head: i8,
    on_ground: bool,
}

impl Place {
    fn of(player: &Player) -> Self {
        Self {
            position: player.position(),
            rotation: player.rotation(),
            head: to_angle_byte(player.head_yaw()),
            on_ground: player.on_ground(),
        }
    }
}

/// What the recording of one player knows of them.
struct Shown {
    /// A player who died is a new entity.
    entity: i32,
    place: Place,
}

/// Writes the recorded players into their own recordings.
pub(super) struct OwnPlayers {
    recorded: Recorded,
    shown: FxHashMap<Uuid, Shown>,
}

fn record<P: ClientPacket>(player: &Player, packet: P) {
    let compression = player.connection.compression();
    if let Ok(packet) = EncodedPacket::from_bare(packet, compression, ConnectionProtocol::Play) {
        player.connection.record(&packet);
    }
}

impl OwnPlayers {
    pub(super) fn new(recorded: Recorded) -> Self {
        Self {
            recorded,
            shown: FxHashMap::default(),
        }
    }

    fn tick(&mut self, server: &Server) {
        let recorded = self.recorded.lock().clone();
        self.shown.retain(|player, _| recorded.contains(player));
        if recorded.is_empty() {
            return;
        }
        for player in server.get_players() {
            let id = player.gameprofile.id();
            if !recorded.contains(&id) {
                continue;
            }
            // Until the client has the world, it would drop the player with everything else.
            if !player.has_client_loaded() {
                self.shown.remove(&id);
                continue;
            }
            let place = Place::of(&player);
            let known = self
                .shown
                .get(&id)
                .filter(|shown| shown.entity == player.id());
            let Some(known) = known else {
                let entity: SharedEntity = Arc::<Player>::clone(&player);
                let compression = player.connection.compression();
                for packet in EntityTracker::spawn_packets(&entity, compression) {
                    player.connection.record(&packet);
                }
                let entity = player.id();
                self.shown.insert(id, Shown { entity, place });
                continue;
            };
            if known.place == place {
                continue;
            }
            let head_turned = known.place.head != place.head;
            // A rider is where the vehicle is: the replay has the vehicle and who sits in it.
            if player.vehicle().is_none() {
                record(
                    &player,
                    CEntityPositionSync {
                        entity_id: player.id(),
                        path: PositionPath::Linear(place.position),
                        y_rot: place.rotation.0,
                        x_rot: place.rotation.1,
                        on_ground: place.on_ground,
                    },
                );
            }
            if head_turned {
                record(
                    &player,
                    CRotateHead {
                        entity_id: player.id(),
                        head_y_rot: place.head,
                    },
                );
            }
            let entity = player.id();
            self.shown.insert(id, Shown { entity, place });
        }
    }
}

impl ServerJob for OwnPlayers {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        self.tick(&server);
        JobPoll::Pending
    }
}
