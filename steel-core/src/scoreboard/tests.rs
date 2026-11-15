use std::{
    env::temp_dir,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use steel_protocol::packets::game::{NumberFormat, ObjectiveRenderType, TeamMethod};
use steel_registry::init_vanilla_registry;
use steel_utils::{
    locks::AsyncMutex,
    saved_data::{SavedDataManager, names as saved_data_names},
};
use text_components::TextComponent;
use tokio::fs;

use super::{
    CustomCriteria, DisplaySlot, DomainScoreboards, ObjectiveCriteria, PersistentScoreboard,
    ScoreHolder, Scoreboard, ScoreboardError, ScoreboardPacket, TeamColor, TeamProperty,
    sync::display_packet,
};

fn health_objective(scoreboard: &Scoreboard) -> super::ScoreboardObjective {
    scoreboard
        .add_objective(
            "health",
            ObjectiveCriteria::Custom(CustomCriteria::Health),
            TextComponent::plain("Health"),
            ObjectiveRenderType::Hearts,
            false,
            None,
        )
        .expect("objective should be added")
}

#[test]
fn new_scores_start_locked_and_keep_value_and_lock_apart() {
    let scoreboard = Scoreboard::new();
    let objective = scoreboard
        .add_dummy_objective("kills")
        .expect("objective should be added");
    let holder = ScoreHolder::new("Steve");

    scoreboard
        .set_score(&holder, &objective, 7)
        .expect("score should be writable");
    assert!(
        scoreboard
            .score_entry(&holder, &objective)
            .expect("score should exist")
            .is_locked()
    );
    scoreboard
        .set_score_locked(&holder, &objective, false)
        .expect("score lock should change");
    scoreboard
        .set_score(&holder, &objective, 9)
        .expect("score should remain writable");

    let entry = scoreboard
        .score_entry(&holder, &objective)
        .expect("score should exist");
    assert_eq!(entry.value(), 9);
    assert!(!entry.is_locked());
}

#[test]
fn read_only_objective_rejects_commands_but_not_criteria_tracking() {
    let scoreboard = Scoreboard::new();
    let objective = health_objective(&scoreboard);
    let holder = ScoreHolder::new("Steve");

    assert_eq!(
        scoreboard.set_score(&holder, &objective, 20),
        Err(ScoreboardError::ReadOnlyObjective("health".to_owned()))
    );
    scoreboard
        .set_score_forced(&holder, "health", 20)
        .expect("criteria tracking writes read-only scores");
    assert_eq!(scoreboard.score(&holder, &objective), Some(20));
}

#[test]
fn display_auto_update_copies_the_holder_display_name() {
    let scoreboard = Scoreboard::new();
    let objective = scoreboard
        .add_objective(
            "kills",
            ObjectiveCriteria::DUMMY,
            TextComponent::plain("Kills"),
            ObjectiveRenderType::Integer,
            true,
            None,
        )
        .expect("objective should be added");
    let holder = ScoreHolder::with_display_name("Steve", TextComponent::plain("Captain Steve"));

    scoreboard
        .set_score(&holder, &objective, 1)
        .expect("score should be writable");

    assert_eq!(
        scoreboard
            .score_entry(&holder, &objective)
            .and_then(|score| score.display().cloned()),
        Some(TextComponent::plain("Captain Steve"))
    );
}

#[test]
fn resetting_the_last_score_forgets_the_holder() {
    let scoreboard = Scoreboard::new();
    let kills = scoreboard
        .add_dummy_objective("kills")
        .expect("objective should be added");
    let deaths = scoreboard
        .add_dummy_objective("deaths")
        .expect("objective should be added");
    let holder = ScoreHolder::new("Steve");
    for objective in [&kills, &deaths] {
        scoreboard
            .set_score(&holder, objective, 1)
            .expect("score should be writable");
    }

    scoreboard.reset_score(&holder, &kills);
    assert_eq!(scoreboard.tracked_holders(), vec![holder.clone()]);
    scoreboard.reset_score(&holder, &deaths);
    assert_eq!(scoreboard.tracked_holders(), Vec::new());
}

#[test]
fn removing_an_objective_clears_its_slots_and_scores() {
    let scoreboard = Scoreboard::new();
    let objective = scoreboard
        .add_dummy_objective("kills")
        .expect("objective should be added");
    let holder = ScoreHolder::new("Steve");
    scoreboard
        .set_score(&holder, &objective, 3)
        .expect("score should be writable");
    scoreboard
        .set_display_objective(DisplaySlot::Sidebar, Some("kills"))
        .expect("objective should be displayed");

    scoreboard.remove_objective("kills");

    assert!(scoreboard.display_objective(DisplaySlot::Sidebar).is_none());
    assert!(scoreboard.holder_objectives(&holder).is_empty());
    assert!(scoreboard.state.read().tracked.is_empty());
}

#[test]
fn displayed_objectives_are_tracked_until_their_last_slot_clears() {
    let scoreboard = Scoreboard::new();
    scoreboard
        .add_dummy_objective("kills")
        .expect("objective should be added");
    for slot in [DisplaySlot::Sidebar, DisplaySlot::List] {
        scoreboard
            .set_display_objective(slot, Some("kills"))
            .expect("objective should be displayed");
    }

    let mut packets = Vec::new();
    scoreboard
        .state
        .write()
        .set_display_objective(DisplaySlot::Sidebar, None, &mut packets);
    assert_eq!(packets, vec![display_packet(DisplaySlot::Sidebar, None)]);
    assert!(scoreboard.state.read().tracked.contains("kills"));

    scoreboard
        .set_display_objective(DisplaySlot::List, None)
        .expect("slot should clear");
    assert!(scoreboard.state.read().tracked.is_empty());
}

#[test]
fn entire_scoreboard_sends_teams_then_each_displayed_objective_once() {
    let scoreboard = Scoreboard::new();
    let objective = scoreboard
        .add_dummy_objective("kills")
        .expect("objective should be added");
    scoreboard
        .set_score(&ScoreHolder::new("Steve"), &objective, 4)
        .expect("score should be writable");
    for slot in [DisplaySlot::List, DisplaySlot::Sidebar] {
        scoreboard
            .set_display_objective(slot, Some("kills"))
            .expect("objective should be displayed");
    }
    scoreboard.add_team("red").expect("team should be added");

    let packets = scoreboard.state.read().entire_scoreboard_packets();
    let kinds: Vec<_> = packets
        .iter()
        .map(|packet| match packet {
            ScoreboardPacket::Team(_) => "team",
            ScoreboardPacket::Objective(_) => "objective",
            ScoreboardPacket::Display(_) => "display",
            ScoreboardPacket::Score(_) => "score",
            ScoreboardPacket::Reset(_) => "reset",
        })
        .collect();
    assert_eq!(kinds, ["team", "objective", "display", "display", "score"]);
}

#[test]
fn team_assignment_replaces_prior_membership() {
    let scoreboard = Scoreboard::new();
    scoreboard
        .add_team("red")
        .expect("red team should be added");
    scoreboard
        .add_team("blue")
        .expect("blue team should be added");
    let holder = ScoreHolder::new("Steve");

    assert_eq!(scoreboard.add_holder_to_team(&holder, "red"), Ok(true));
    assert_eq!(scoreboard.add_holder_to_team(&holder, "red"), Ok(true));
    assert_eq!(scoreboard.add_holder_to_team(&holder, "blue"), Ok(true));

    assert_eq!(
        scoreboard.holder_team_name(&holder).as_deref(),
        Some("blue")
    );
    assert!(
        scoreboard
            .team("red")
            .expect("red team should exist")
            .players()
            .is_empty()
    );
}

#[test]
fn dead_non_player_entities_lose_scores_and_team() {
    let scoreboard = Scoreboard::new();
    let objective = scoreboard
        .add_dummy_objective("kills")
        .expect("objective should be added");
    scoreboard.add_team("red").expect("team should be added");
    let zombie = ScoreHolder::new("9b0c3d2e-0000-0000-0000-000000000001");
    let player = ScoreHolder::new("Steve");
    for holder in [&zombie, &player] {
        scoreboard
            .set_score(holder, &objective, 1)
            .expect("score should be writable");
        scoreboard
            .add_holder_to_team(holder, "red")
            .expect("holder should join");
    }

    scoreboard.entity_removed(&zombie, false, false);
    scoreboard.entity_removed(&player, true, false);

    assert_eq!(scoreboard.tracked_holders(), vec![player.clone()]);
    assert!(scoreboard.holder_team_name(&zombie).is_none());
    assert_eq!(scoreboard.holder_team_name(&player).as_deref(), Some("red"));
}

#[test]
fn team_packet_options_follow_the_flags() {
    let scoreboard = Scoreboard::new();
    scoreboard.add_team("red").expect("team should be added");
    scoreboard
        .modify_team("red", TeamProperty::AllowFriendlyFire(false))
        .expect("team should change");
    scoreboard
        .modify_team("red", TeamProperty::Color(Some(TeamColor::Red)))
        .expect("team should change");

    let packets = scoreboard.state.read().entire_scoreboard_packets();
    let Some(ScoreboardPacket::Team(packet)) = packets.first() else {
        panic!("team packet expected");
    };
    let TeamMethod::Add(parameters, _) = &packet.method else {
        panic!("add method expected");
    };
    assert_eq!(parameters.options, 2);
    assert_eq!(parameters.color, Some(TeamColor::Red.id()));
}

#[test]
fn packed_scoreboard_round_trips() {
    let _ = init_vanilla_registry();
    let scoreboard = Scoreboard::new();
    let objective = scoreboard
        .add_objective(
            "kills",
            ObjectiveCriteria::by_name("minecraft.custom:minecraft.jump")
                .expect("stat criteria should parse"),
            TextComponent::plain("Kills"),
            ObjectiveRenderType::Integer,
            true,
            Some(NumberFormat::Fixed(TextComponent::plain("-"))),
        )
        .expect("objective should be added");
    let holder = ScoreHolder::new("Steve");
    scoreboard
        .set_score(&holder, &objective, 5)
        .expect("score should be writable");
    scoreboard
        .set_display_objective(DisplaySlot::TeamSidebar(TeamColor::Gold), Some("kills"))
        .expect("objective should be displayed");
    scoreboard.add_team("red").expect("team should be added");
    scoreboard
        .modify_team(
            "red",
            TeamProperty::PlayerPrefix(TextComponent::plain("[R] ")),
        )
        .expect("team should change");
    scoreboard
        .add_holder_to_team(&holder, "red")
        .expect("holder should join");

    let packed = PersistentScoreboard::pack(&scoreboard.state.read());
    let toml = toml::to_string(&packed).expect("scoreboard should serialize");
    let parsed: PersistentScoreboard = toml::from_str(&toml).expect("scoreboard should parse");
    let restored = Scoreboard::from_state(parsed.unpack().expect("scoreboard should unpack"));

    assert_eq!(restored.objective("kills"), Some(objective.clone()));
    assert_eq!(
        restored.score_entry(&holder, &objective),
        scoreboard.score_entry(&holder, &objective)
    );
    assert_eq!(
        restored.display_objective(DisplaySlot::TeamSidebar(TeamColor::Gold)),
        Some(objective)
    );
    assert_eq!(restored.team("red"), scoreboard.team("red"));
    assert_eq!(restored.holder_team_name(&holder).as_deref(), Some("red"));
}

#[test]
fn first_steel_format_still_loads() {
    let legacy = r#"
teams = ["red"]

[objectives.kills]
read_only = false

[scores.Steve.kills]
value = 3
locked = false

[holder_teams]
Steve = "red"
"#;
    let parsed: PersistentScoreboard = toml::from_str(legacy).expect("legacy format should parse");
    let scoreboard = Scoreboard::from_state(parsed.unpack().expect("legacy format should unpack"));
    let objective = scoreboard
        .objective("kills")
        .expect("objective should load");
    let holder = ScoreHolder::new("Steve");

    assert_eq!(objective.criteria(), &ObjectiveCriteria::DUMMY);
    let score = scoreboard
        .score_entry(&holder, &objective)
        .expect("score should load");
    assert_eq!(score.value(), 3);
    assert!(!score.is_locked());
    assert_eq!(scoreboard.holder_team_name(&holder).as_deref(), Some("red"));
}

#[tokio::test]
async fn saved_scoreboard_becomes_clean_after_save() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_nanos();
    let path = temp_dir().join(format!("steel-scoreboard-{unique}"));
    let manager = SavedDataManager::new(Some(&path));
    let scoreboard = Scoreboard::new();
    let objective = scoreboard
        .add_dummy_objective("kills")
        .expect("objective should be added");
    let holder = ScoreHolder::new("Steve");
    scoreboard
        .set_score(&holder, &objective, 5)
        .expect("score should be writable");

    let (revision, state) = scoreboard
        .pending_save()
        .expect("scoreboard should be dirty");
    manager
        .save(saved_data_names::SCOREBOARD, &state)
        .await
        .expect("scoreboard should save");
    scoreboard.mark_saved(revision);
    assert!(scoreboard.pending_save().is_none());

    let persistent: PersistentScoreboard = manager
        .load_or_default(saved_data_names::SCOREBOARD)
        .await
        .expect("scoreboard should load");
    let restored = Scoreboard::from_state(persistent.unpack().expect("scoreboard should unpack"));
    let restored_objective = restored
        .objective("kills")
        .expect("objective should persist");
    assert_eq!(restored.score(&holder, &restored_objective), Some(5));

    fs::remove_dir_all(path)
        .await
        .expect("temporary scoreboard directory should be removed");
}

#[test]
fn mutation_after_snapshot_remains_dirty_when_snapshot_is_marked_saved() {
    let scoreboard = Scoreboard::new();
    scoreboard
        .add_dummy_objective("kills")
        .expect("objective should be added");
    let (revision, _) = scoreboard
        .pending_save()
        .expect("scoreboard should be dirty");

    scoreboard
        .add_dummy_objective("deaths")
        .expect("second objective should be added");
    scoreboard.mark_saved(revision);

    let (pending_revision, _) = scoreboard
        .pending_save()
        .expect("newer mutation should remain dirty");
    assert!(pending_revision > revision);
}

#[test]
fn domains_keep_independent_scoreboards() {
    let scoreboards = DomainScoreboards {
        scoreboards: [
            ("alpha".to_owned(), Arc::new(Scoreboard::new())),
            ("beta".to_owned(), Arc::new(Scoreboard::new())),
        ]
        .into_iter()
        .collect(),
        save_lock: AsyncMutex::new(()),
    };
    scoreboards
        .get("alpha")
        .expect("alpha scoreboard should exist")
        .add_dummy_objective("kills")
        .expect("alpha objective should be added");

    assert!(
        scoreboards
            .get("beta")
            .expect("beta scoreboard should exist")
            .objective("kills")
            .is_none()
    );
}
