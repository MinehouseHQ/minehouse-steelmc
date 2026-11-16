use glam::{DVec3, Vec3};
use steel_registry::{REGISTRY, init_vanilla_registry, vanilla_blocks};

use super::{
    blocks::block_state,
    script::{Action, ActorKind, Gather, ShowScript, Stage, snap},
};

/// The meteor show of the Fabric mod without the cues that are not ported yet.
const METEOR: &str = r#"{
  "title": "Падение метеорита",
  "audience": { "gather": "teleport", "radius": 30 },
  "markers": { "impact": "0:13", "aftermath": "0:16" },
  "length": "0:45",
  "actors": {
    "cam": { "type": "camera" },
    "meteor": { "type": "block", "block": "minecraft:magma_block", "scale": 6, "bright": true, "viewRange": 8 },
    "sign": { "type": "text", "text": "&6Кратер", "background": "c0000000", "glow": "c040ff" }
  },
  "tracks": {
    "world": [
      { "at": "0:00", "do": "time", "to": 18000, "over": "6s" },
      { "at": "0:00", "do": "weather", "to": "thunder", "over": "6s" }
    ],
    "camera": [
      { "at": "0:00", "do": "cinematic" },
      { "at": "0:00", "do": "path", "orbit": { "center": [0, 10, 0], "radius": 40, "height": 25, "from": 200, "to": 260 }, "over": "8s" },
      { "at": "0:08", "do": "path", "points": [ { "pos": [34, 5, 30] }, { "pos": [22, 4, 20] } ], "look": [0, 18, 0], "over": "6s", "ease": "linear" },
      { "at": "0:12", "do": "shake", "strength": 0.3, "over": "2s" },
      { "at": "0:26", "do": "cinematic", "on": false }
    ],
    "actors": [
      { "at": "0:06", "do": "spawn", "actor": "meteor", "pos": [60, 120, -40] },
      { "at": "0:06", "do": "move", "actor": "meteor", "to": [0, 2, 0], "over": "8s", "ease": "inQuad" },
      { "at": "0:06", "do": "transform", "actor": "meteor", "rotate": [40, 120, 0], "over": "3s" },
      { "at": "0:14", "do": "transform", "actor": "meteor", "scale": 3, "over": "2s", "ease": "outCubic" }
    ],
    "fx": [
      { "at": "0:06", "do": "particles", "actor": "meteor", "id": "minecraft:flame", "count": 30, "spread": 2, "speed": 0.05, "over": "8s" },
      { "at": "0:01", "do": "sound", "id": "minecraft:ambient.cave", "volume": 1 },
      { "at": "0:02", "do": "sound", "id": "minehouse:show.voice", "source": "voice" }
    ],
    "text": [
      { "at": "0:00", "do": "title", "text": "&6Падение метеорита", "sub": "&7смотрите в небо", "stay": "4s" },
      { "at": "0:26", "do": "actionbar", "text": "&7Подойдите к кратеру" },
      { "at": "0:27", "do": "chat", "text": "Подробнее", "link": "https://minehouse.pro" }
    ]
  }
}"#;

fn show(text: &str) -> Result<ShowScript, String> {
    init_vanilla_registry();
    ShowScript::parse("probe", text, None)
}

#[test]
fn a_show_is_read_whole() {
    let show = show(METEOR).expect("the show loads");
    assert_eq!(show.title, "Падение метеорита");
    assert_eq!(show.gather, Gather::Teleport);
    assert!((show.gather_radius - 30.0).abs() < 1e-9);
    assert_eq!(show.length, 900);
    assert_eq!(show.markers.get("impact"), Some(&260));
    assert_eq!(show.stage, None);
    assert_eq!(show.countdown_name(), "Падение метеорита");

    let meteor = show
        .actors
        .iter()
        .find(|actor| actor.name == "meteor")
        .expect("the meteor");
    let magma = REGISTRY
        .blocks
        .get_default_state_id(&vanilla_blocks::MAGMA_BLOCK);
    assert!(matches!(meteor.kind, ActorKind::Block(state) if state == magma));
    assert_eq!(meteor.scale, Vec3::splat(6.0));
    assert!(meteor.bright);
    let sign = show
        .actors
        .iter()
        .find(|actor| actor.name == "sign")
        .expect("the sign");
    assert_eq!(sign.glow, Some(0x00c0_40ff));
    assert!(
        matches!(&sign.kind, ActorKind::Text { background, .. } if *background == 0xc000_0000_u32.cast_signed())
    );

    // Sorted by start; cues of the same tick keep the order of the file, track by track.
    let starts: Vec<u32> = show.cues.iter().map(|cue| cue.at).collect();
    assert!(starts.is_sorted());
    let first: Vec<&str> = show
        .cues
        .iter()
        .take_while(|cue| cue.at == 0)
        .map(|cue| match cue.action {
            Action::Time { .. } => "time",
            Action::Weather { .. } => "weather",
            Action::Cinematic { .. } => "cinematic",
            Action::Path { .. } => "path",
            Action::Title { .. } => "title",
            _ => "other",
        })
        .collect();
    assert_eq!(first, ["time", "weather", "cinematic", "path", "title"]);
    let path = show
        .cues
        .iter()
        .find(|cue| matches!(cue.action, Action::Path { .. }))
        .expect("a flight");
    assert_eq!(path.duration, 160);
}

#[test]
fn the_length_is_the_last_cue_and_two_seconds() {
    let show = show(
        r#"{"actors": {"cam": {"type": "camera"}},
            "cues": [{"at": "0:03", "do": "shake", "over": "2s"}, {"at": 1, "do": "cut", "pos": [0, 1, 0]}]}"#,
    )
    .expect("the show loads");
    assert_eq!(show.title, "probe");
    assert_eq!(show.length, 60 + 40 + 40);
    assert_eq!(show.gather_center, DVec3::ZERO);
}

#[test]
fn mistakes_name_the_place() {
    let cases = [
        (
            r#"{"cues": [{"at": 0, "do": "cut", "pos": [0, 0, 0]}]}"#,
            "нет актёра \"cam\"",
        ),
        (
            r#"{"actors": {"a": {"type": "block", "block": "minecraft:nope"}}}"#,
            "актёр \"a\": не понимаю блок",
        ),
        (
            r#"{"actors": {"a": {"type": "rig", "rig": "banker"}}}"#,
            "пока не перенесён",
        ),
        (
            r#"{"tracks": {"fx": [{"at": "0:02", "do": "meteors"}]}}"#,
            "tracks.fx[0] (\"meteors\" на 0:02): это действие пока не перенесено",
        ),
        (
            r#"{"cues": [{"at": 0, "do": "dance"}]}"#,
            "нет такого действия \"dance\"",
        ),
        (
            r#"{"cues": [{"at": 0, "do": "particles", "id": "minecraft:dust"}]}"#,
            "требуют параметров",
        ),
        (
            r#"{"cues": [{"at": 0, "do": "chat", "text": "x", "link": "http://a.b"}]}"#,
            "https://",
        ),
        (
            r#"{"actors": {"cam": {"type": "camera"}}, "cues": [{"do": "path", "path": "Fly Me"}]}"#,
            "только a-z",
        ),
        (r#"{"logic": {"nodes": [{"type": "start"}]}}"#, "logic"),
        ("[]", "ожидаю объект"),
    ];
    for (text, expected) in cases {
        let error = show(text).expect_err(text);
        assert!(error.contains(expected), "{error} has no {expected}");
    }
}

#[test]
fn a_show_turns_with_its_stage() {
    let text = r#"{
        "stageFacing": true,
        "audience": {"center": [0, 0, 10]},
        "actors": {"cam": {"type": "camera"}, "box": {"type": "block", "block": "minecraft:stone"}},
        "cues": [
            {"at": 0, "do": "cut", "pos": [2, 5, 10], "yaw": 30},
            {"at": 0, "do": "spawn", "actor": "box", "pos": [1, 0, 0], "scale": [1, 2, 3]},
            {"at": 0, "do": "transform", "actor": "box", "rotate": [10, 20, 30], "scale": [1, 2, 3]}
        ]
    }"#;
    let stage = |yaw| Stage {
        world: "minecraft:overworld".to_owned(),
        pos: DVec3::new(100.0, 64.0, 100.0),
        yaw,
    };
    init_vanilla_registry();
    let south = ShowScript::parse("probe", text, Some(stage(0))).expect("the show loads");
    let west = ShowScript::parse("probe", text, Some(stage(90))).expect("the show loads");

    assert_eq!(south.gather_center, DVec3::new(0.0, 0.0, 10.0));
    // Facing west, "ahead" (+Z of the script) is -X of the world.
    assert_eq!(west.gather_center, DVec3::new(-10.0, 0.0, 0.0));
    let cut = |show: &ShowScript| match show.cues.first().map(|cue| &cue.action) {
        Some(Action::Cut { pos, yaw, .. }) => (*pos, *yaw),
        other => panic!("not a cut: {other:?}"),
    };
    assert_eq!(cut(&south), (DVec3::new(2.0, 5.0, 10.0), 30.0));
    assert_eq!(cut(&west), (DVec3::new(-10.0, 5.0, 2.0), 120.0));
    // Sizes and angles are not places.
    for show in [&south, &west] {
        let sizes: Vec<Vec3> = show
            .cues
            .iter()
            .filter_map(|cue| match &cue.action {
                Action::Spawn { scale, .. } | Action::Transform { scale, .. } => *scale,
                _ => None,
            })
            .collect();
        assert_eq!(sizes, [Vec3::new(1.0, 2.0, 3.0); 2]);
    }
}

#[test]
fn yaw_snaps_to_quarter_turns() {
    assert_eq!(snap(0.0), 0);
    assert_eq!(snap(44.0), 0);
    assert_eq!(snap(46.0), 90);
    assert_eq!(snap(-90.0), 270);
    assert_eq!(snap(-179.0), 180);
    assert_eq!(snap(359.0), 0);
}

#[test]
fn block_states_take_properties() {
    init_vanilla_registry();
    let default = block_state("minecraft:oak_log").expect("a log");
    let lying = block_state("minecraft:oak_log[axis=x]").expect("a lying log");
    assert_ne!(default, lying);
    assert_eq!(
        block_state("minecraft:oak_log[ axis = y ]").expect("a standing log"),
        default
    );
    assert!(block_state("minecraft:oak_log[axis=q]").is_err());
    assert!(block_state("minecraft:oak_log[axis").is_err());
}
