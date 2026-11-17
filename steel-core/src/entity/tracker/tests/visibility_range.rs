use super::*;
use std::hash::Hash;

#[test]
fn client_tracking_range_is_converted_to_blocks() {
    let range = EntityTrackingRange::from_client_chunk_range(4);

    assert!((range.visible_radius(10) - 64.0).abs() < f64::EPSILON);
}

#[test]
fn zero_client_tracking_range_disables_tracking() {
    let range = EntityTrackingRange::from_client_chunk_range(0);

    assert!(range.is_disabled());
}

#[test]
fn tracking_distance_uses_horizontal_circle() {
    let range = EntityTrackingRange::from_client_chunk_range(4);
    let entity_pos = DVec3::ZERO;

    assert!(is_within_tracking_distance(
        entity_pos,
        DVec3::new(64.0, 300.0, 0.0),
        range,
        8,
    ));
    assert!(!is_within_tracking_distance(
        entity_pos,
        DVec3::new(64.0, 0.0, 64.0),
        range,
        8,
    ));
    assert!(!is_within_tracking_distance(
        entity_pos,
        DVec3::new(64.1, 0.0, 0.0),
        range,
        8,
    ));
}

#[test]
fn tracking_distance_is_capped_by_player_view_distance() {
    let range = EntityTrackingRange::from_client_chunk_range(10);
    let entity_pos = DVec3::ZERO;

    assert!(is_within_tracking_distance(
        entity_pos,
        DVec3::new(32.0, 0.0, 0.0),
        range,
        2,
    ));
    assert!(!is_within_tracking_distance(
        entity_pos,
        DVec3::new(32.1, 0.0, 0.0),
        range,
        2,
    ));
}

#[test]
fn vehicle_effective_tracking_range_uses_widest_passenger_range() {
    init_vanilla_registry();

    let vehicle_typed = PairingTestEntity::new_with_type(1, &vanilla_entities::ITEM, Vec::new());
    let passenger_typed =
        PairingTestEntity::new_with_type(2, &vanilla_entities::PLAYER, Vec::new());
    assert!(
        passenger_typed.entity_type().client_tracking_range
            > vehicle_typed.entity_type().client_tracking_range
    );

    let passenger: SharedEntity = passenger_typed;
    vehicle_typed.add_passenger(&passenger);
    let vehicle: SharedEntity = vehicle_typed;
    let base_range =
        EntityTrackingRange::from_client_chunk_range(vehicle.entity_type().client_tracking_range);

    let effective = effective_tracking_range(vehicle.as_ref(), base_range);

    assert_eq!(
        effective.block_radius.to_bits(),
        (f64::from(passenger.entity_type().client_tracking_range) * BLOCKS_PER_CHUNK).to_bits()
    );
}

fn indexed_ids<K: Eq + Hash>(index: &scc::HashMap<K, FxHashSet<i32>>, key: &K) -> Vec<i32> {
    let mut ids = index
        .read_sync(key, |_, ids| ids.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    ids.sort_unstable();
    ids
}

#[test]
fn player_refresh_indexes_follow_chunk_moves_and_removal() {
    init_vanilla_registry();

    let tracker = EntityTracker::new();
    let entity = PairingTestEntity::shared(Vec::new());
    let entity_id = entity.id();
    let start = ChunkPos::from_entity_pos(entity.position());
    let far = ChunkPos::new(start.0.x + 40, start.0.y);
    let player_id = 7;

    track_entity_for_player(&tracker, &entity, player_id);
    assert_eq!(
        indexed_ids(&tracker.chunk_entities, &start),
        vec![entity_id]
    );
    assert_eq!(
        indexed_ids(&tracker.player_entities, &player_id),
        vec![entity_id]
    );

    // No player tracks the far chunk, so the move drops the pairing and both indexes follow.
    tracker.on_entity_section_change(entity_id, start, far, |_| Vec::new(), |_| None);
    assert_eq!(
        indexed_ids(&tracker.chunk_entities, &start),
        Vec::<i32>::new()
    );
    assert_eq!(indexed_ids(&tracker.chunk_entities, &far), vec![entity_id]);
    assert_eq!(
        indexed_ids(&tracker.player_entities, &player_id),
        Vec::<i32>::new()
    );

    tracker.remove(entity_id, |_| None);
    assert_eq!(tracker.chunk_entities.len(), 0);
    assert_eq!(tracker.player_entities.len(), 0);
}
