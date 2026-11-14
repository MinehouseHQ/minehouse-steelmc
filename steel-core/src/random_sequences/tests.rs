use std::env::temp_dir;
use std::time::{SystemTime, UNIX_EPOCH};

use steel_utils::Identifier;
use steel_utils::random::Random as _;
use steel_utils::random::xoroshiro::{Seed128, Xoroshiro};
use steel_utils::saved_data::SavedDataManager;
use steel_utils::saved_data::names as saved_data_names;
use tokio::fs;

use super::{PersistentRandomSequences, RandomSequences, SeedDefaults};

fn draws(sequences: &RandomSequences, key: &Identifier, seed: i64) -> Vec<i64> {
    (0..4)
        .map(|_| sequences.draw(key, seed, Xoroshiro::next_i64))
        .collect()
}

#[test]
fn a_new_sequence_follows_vanillas_seed_derivation() {
    let key = Identifier::vanilla_static("dice");
    let sequences = RandomSequences::new();
    let mut expected = Xoroshiro::from_seed_128(
        Seed128::upgrade_unmixed(12_345)
            .xor(Seed128::from_hash_of("minecraft:dice"))
            .mixed(),
    );
    let expected: Vec<_> = (0..4).map(|_| expected.next_i64()).collect();
    assert_eq!(draws(&sequences, &key, 12_345), expected);
    // The stream continues instead of restarting.
    assert_ne!(draws(&sequences, &key, 12_345), expected);
    assert_eq!(sequences.ids(), [key]);
}

#[test]
fn seed_settings_change_which_parts_feed_the_seed() {
    let dice = Identifier::vanilla_static("dice");
    let coin = Identifier::vanilla_static("coin");
    let sequences = RandomSequences::new();

    // Without the sequence id, every id with the same seed draws the same.
    sequences.set_seed_defaults(SeedDefaults {
        salt: 7,
        include_world_seed: true,
        include_sequence_id: false,
    });
    assert_eq!(draws(&sequences, &dice, 1), draws(&sequences, &coin, 1));

    // Without the world seed, only the salt is left.
    let world_free = SeedDefaults {
        salt: 7,
        include_world_seed: false,
        include_sequence_id: true,
    };
    sequences.reset_with(&dice, 1, world_free);
    let first_world = draws(&sequences, &dice, 1);
    sequences.reset_with(&dice, 999, world_free);
    assert_eq!(draws(&sequences, &dice, 999), first_world);
    let mut expected = Xoroshiro::from_seed_128(
        Seed128::upgrade_unmixed(7)
            .xor(Seed128::from_hash_of("minecraft:dice"))
            .mixed(),
    );
    assert_eq!(first_world[0], expected.next_i64());

    // `reset` restarts from the current defaults; `clear` forgets everything.
    sequences.reset(&dice, 1);
    let restarted = draws(&sequences, &dice, 1);
    sequences.reset(&dice, 1);
    assert_eq!(draws(&sequences, &dice, 1), restarted);
    assert_eq!(sequences.clear(), 2);
    assert_eq!(sequences.ids().len(), 0);
}

#[test]
fn only_draws_mark_the_sequences_dirty() {
    let key = Identifier::vanilla_static("dice");
    let sequences = RandomSequences::new();
    sequences.reset(&key, 0);
    sequences.set_seed_defaults(SeedDefaults {
        salt: 1,
        ..SeedDefaults::default()
    });
    assert!(sequences.pending_save().is_none());
    let _ = draws(&sequences, &key, 0);
    let (revision, persistent) = sequences.pending_save().expect("a draw dirties");
    assert_eq!(persistent.salt, 1);
    sequences.mark_saved(revision);
    assert!(sequences.pending_save().is_none());
}

#[tokio::test]
async fn sequences_resume_after_a_save_and_load() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_nanos();
    let path = temp_dir().join(format!("steel-random-sequences-{unique}"));
    let manager = SavedDataManager::new(Some(&path));

    let key = Identifier::from_steel("loot/chest");
    let sequences = RandomSequences::new();
    let defaults = SeedDefaults {
        salt: -3,
        include_world_seed: false,
        include_sequence_id: true,
    };
    sequences.set_seed_defaults(defaults);
    let _ = draws(&sequences, &key, 42);
    let (_, persistent) = sequences.pending_save().expect("draws dirty the data");
    manager
        .save(saved_data_names::RANDOM_SEQUENCES, &persistent)
        .await
        .expect("sequences should save");

    let loaded: PersistentRandomSequences = manager
        .load_or_default(saved_data_names::RANDOM_SEQUENCES)
        .await
        .expect("sequences should load");
    assert_eq!(loaded, persistent);
    let restored = RandomSequences::from_persistent(loaded).expect("saved sequences are valid");
    assert!(restored.pending_save().is_none());
    assert_eq!(restored.seed_defaults(), defaults);
    assert_eq!(draws(&restored, &key, 42), draws(&sequences, &key, 42));

    fs::remove_dir_all(path)
        .await
        .expect("temporary saved data directory should be removed");
}
