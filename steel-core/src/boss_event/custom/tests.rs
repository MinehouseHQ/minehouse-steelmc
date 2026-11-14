use std::env::temp_dir;
use std::slice;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use steel_protocol::packets::game::{BossBarColor, BossBarFlags, BossBarOverlay};
use steel_utils::saved_data::SavedDataManager;
use steel_utils::saved_data::names as saved_data_names;
use steel_utils::{Identifier, translations};
use text_components::format::Color;
use text_components::interactivity::HoverEvent;
use text_components::{Modifier as _, TextComponent};
use tokio::fs;

use super::super::tests::{ADD, REMOVE, UPDATE_PROGRESS, operations, recording_player};
use super::{CustomBossEvents, PersistentCustomBossEvents, progress};

#[test]
fn progress_divides_and_clamps_like_vanilla() {
    assert_eq!(progress(0, 100), 0.0);
    assert_eq!(progress(5, 10), 0.5);
    assert_eq!(progress(20, 10), 1.0);
    assert_eq!(progress(-3, 10), 0.0);
    // `/execute store` can set a zero max: a positive value overflows to a
    // full bar and zero over zero stays NaN, as Java's `Math.min` keeps it.
    assert_eq!(progress(3, 0), 1.0);
    assert!(progress(0, 0).is_nan());
}

#[test]
fn a_new_bar_is_empty_white_and_counts_as_a_change() {
    let events = CustomBossEvents::new();
    assert!(events.pending_save().expect("pack").is_none());
    let bar = events.create(
        Identifier::from_steel("fight"),
        TextComponent::plain("Fight"),
    );
    assert_eq!(bar.value(), 0);
    assert_eq!(bar.max(), 100);
    assert!(bar.event().progress().abs() < f32::EPSILON);
    assert_eq!(bar.event().color(), BossBarColor::White);
    assert_eq!(bar.event().overlay(), BossBarOverlay::Progress);
    assert!(bar.event().is_visible());
    assert!(events.pending_save().expect("pack").is_some());
    assert_eq!(
        bar.display_name(),
        translations::CHAT_SQUARE_BRACKETS
            .message([TextComponent::plain("Fight")])
            .component()
            .color(Color::White)
            .hover_event(HoverEvent::show_text(TextComponent::plain("steel:fight")))
            .insertion("steel:fight")
    );
}

#[test]
fn set_players_shows_the_bar_to_exactly_the_targets() {
    let events = CustomBossEvents::new();
    let bar = events.create(Identifier::from_steel("vote"), TextComponent::plain("Vote"));
    let (alice, alice_packets) = recording_player("BarAlice", 9201);
    let (bob, bob_packets) = recording_player("BarBob", 9202);
    alice_packets.lock().clear();
    bob_packets.lock().clear();

    assert!(bar.set_players(&[Arc::clone(&alice), Arc::clone(&bob)]));
    assert!(!bar.set_players(&[Arc::clone(&bob), Arc::clone(&alice)]));
    assert_eq!(operations(&alice_packets, bar.event().id()), [ADD]);
    assert_eq!(operations(&bob_packets, bar.event().id()), [ADD]);

    assert!(bar.set_players(slice::from_ref(&bob)));
    assert_eq!(operations(&alice_packets, bar.event().id()), [REMOVE]);
    assert_eq!(operations(&bob_packets, bar.event().id()), [0; 0]);
    assert_eq!(bar.event().players().len(), 1);

    // An empty bar stays empty when only its max changes: nothing is sent.
    bar.set_max(4);
    assert_eq!(operations(&bob_packets, bar.event().id()), [0; 0]);
    bar.set_value(1);
    assert_eq!(
        operations(&bob_packets, bar.event().id()),
        [UPDATE_PROGRESS]
    );
    assert!((bar.event().progress() - 0.25).abs() < f32::EPSILON);
}

#[test]
fn a_disconnected_player_keeps_the_bar_for_the_next_join() {
    let events = CustomBossEvents::new();
    let bar = events.create(
        Identifier::from_steel("event"),
        TextComponent::plain("Event"),
    );
    let (player, packets) = recording_player("BarReturner", 9203);
    bar.add_player(&player);
    let (stranger, stranger_packets) = recording_player("BarStranger", 9204);
    packets.lock().clear();
    stranger_packets.lock().clear();

    events.on_player_disconnect(&player);
    assert_eq!(operations(&packets, bar.event().id()), [REMOVE]);
    assert_eq!(bar.event().players().len(), 0);

    events.on_player_connect(&player);
    events.on_player_connect(&stranger);
    assert_eq!(operations(&packets, bar.event().id()), [ADD]);
    assert_eq!(operations(&stranger_packets, bar.event().id()), [0; 0]);
    assert_eq!(bar.event().players().len(), 1);
}

#[tokio::test]
async fn bars_round_trip_through_saved_data() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_nanos();
    let path = temp_dir().join(format!("steel-custom-boss-events-{unique}"));
    let manager = SavedDataManager::new(Some(&path));

    let events = CustomBossEvents::new();
    let id = Identifier::from_steel("siege");
    let name = TextComponent::plain("Siege").color(Color::Red);
    let bar = events.create(id.clone(), name.clone());
    let (player, _packets) = recording_player("BarSaver", 9205);
    bar.add_player(&player);
    bar.set_max(20);
    bar.set_value(15);
    bar.set_color(BossBarColor::Red);
    bar.set_overlay(BossBarOverlay::Notched10);
    bar.set_visible(false);
    bar.event().set_flags(BossBarFlags {
        darken_screen: true,
        play_boss_music: false,
        create_world_fog: true,
    });

    let (revision, persistent) = events
        .pending_save()
        .expect("bars should pack")
        .expect("a new bar is unsaved");
    manager
        .save(saved_data_names::CUSTOM_BOSS_EVENTS, &persistent)
        .await
        .expect("bars should save");
    events.mark_saved(revision);
    assert!(events.pending_save().expect("pack").is_none());

    let loaded: PersistentCustomBossEvents = manager
        .load_or_default(saved_data_names::CUSTOM_BOSS_EVENTS)
        .await
        .expect("bars should load");
    assert_eq!(loaded, persistent);
    let restored = CustomBossEvents::from_persistent(loaded).expect("saved bars are valid");
    assert!(restored.pending_save().expect("pack").is_none());
    let restored_bar = restored.get(&id).expect("the bar is restored");
    assert_eq!(restored_bar.event().name(), name);
    assert_eq!(restored_bar.value(), 15);
    assert_eq!(restored_bar.max(), 20);
    assert!((restored_bar.event().progress() - 0.75).abs() < f32::EPSILON);
    assert_eq!(restored_bar.event().color(), BossBarColor::Red);
    assert_eq!(restored_bar.event().overlay(), BossBarOverlay::Notched10);
    assert!(!restored_bar.event().is_visible());
    assert_eq!(
        restored_bar.event().flags(),
        BossBarFlags {
            darken_screen: true,
            play_boss_music: false,
            create_world_fog: true,
        }
    );
    // The saved player sees the bar again when they join.
    restored.on_player_connect(&player);
    assert_eq!(restored_bar.event().players().len(), 1);

    fs::remove_dir_all(path)
        .await
        .expect("temporary saved data directory should be removed");
}
