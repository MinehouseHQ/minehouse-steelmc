//! Vanilla `ServerPlayer` camera: a spectator watching the world through
//! another entity's eyes.

use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::{CSetCamera, RelativeMovement};

use super::Player;
use crate::entity::{Entity, LivingEntity as _, SharedEntity, change_entity_world};
use crate::portal::{TeleportPostTransition, TeleportTransition};
use crate::world::World;

impl Player {
    /// Vanilla `getCamera`: the watched entity, or `None` while the player
    /// looks through their own eyes.
    #[must_use]
    pub fn camera_entity(&self) -> Option<SharedEntity> {
        self.camera.lock().clone()
    }

    /// The network id of the entity the client renders from.
    #[must_use]
    pub fn camera_id(&self) -> i32 {
        self.camera
            .lock()
            .as_ref()
            .map_or_else(|| self.id(), |camera| camera.id())
    }

    /// Whether the player looks through their own eyes (vanilla
    /// `getCamera() == this`).
    #[must_use]
    pub fn is_own_camera(&self) -> bool {
        self.camera.lock().is_none()
    }

    /// Vanilla `setCamera`: watch through `camera`, or the player again for
    /// `None`. A change moves the player to the camera and tells the client.
    pub fn set_camera(&self, camera: Option<SharedEntity>) {
        let camera = camera.filter(|camera| camera.id() != self.id());
        let old_id = self.camera_id();
        let new_id = camera
            .as_ref()
            .map_or_else(|| self.id(), |camera| camera.id());
        self.camera.lock().clone_from(&camera);
        if old_id == new_id {
            return;
        }

        match &camera {
            Some(camera) => {
                if let Some(world) = camera.level() {
                    self.teleport_for_camera(&world, camera.position());
                }
            }
            None => self.teleport_for_camera(&self.get_world(), self.position()),
        }
        self.send_packet(CSetCamera { camera_id: new_id });
        self.movement.lock().reset_position(self.position());
    }

    /// Vanilla `ServerPlayer.teleportTo(level, x, y, z, Set.of(), yRot, xRot,
    /// false)`: keeps the camera, wakes a sleeper.
    fn teleport_for_camera(&self, world: &Arc<World>, position: DVec3) {
        if self.is_sleeping() {
            self.stop_sleeping();
        }
        let rotation = self.rotation();
        if Arc::ptr_eq(world, &self.get_world()) {
            if self.teleport(position, rotation.0, rotation.1).is_ok() {
                self.reset_flying_ticks();
            }
            return;
        }
        let transition = TeleportTransition {
            target_world: Arc::clone(world),
            position,
            rotation,
            velocity: DVec3::ZERO,
            relatives: RelativeMovement::NONE,
            portal_cooldown: 0,
            as_passenger: false,
            post_transition: TeleportPostTransition::do_nothing(),
        };
        let Some(player) = self.get_world().players.get_by_entity_id(self.id()) else {
            return;
        };
        if change_entity_world(player as SharedEntity, &transition).is_some() {
            self.reset_flying_ticks();
        }
    }

    /// Vanilla `ServerPlayer.tick` camera step: follow a live camera until the
    /// player sneaks, and return to the player once it is gone.
    pub(super) fn tick_camera(&self) {
        let Some(camera) = self.camera_entity() else {
            return;
        };
        if !camera.is_alive() {
            self.set_camera(None);
            return;
        }
        let (yaw, pitch) = camera.rotation();
        if self.try_set_position(camera.position()).is_ok() {
            self.set_rotation((yaw, pitch));
            self.set_old_position_to_current();
        }
        let world = self.get_world();
        if world.entity_manager().get_by_id(self.id()).is_some() {
            world.chunk_map.update_player_status(self);
        }
        if self.wants_to_stop_riding() {
            self.set_camera(None);
        }
    }
}
