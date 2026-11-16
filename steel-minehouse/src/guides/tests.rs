use glam::{Mat4, Quat, Vec3};

use crate::content::{CARD, MAGNIFIER};

use super::{
    session::{caption, step_ticks},
    stage::Element,
    *,
};

#[test]
fn every_guide_has_an_id_for_the_command_and_steps() {
    let mut seen = Vec::new();
    for guide in all() {
        assert!(
            guide
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'),
            "{}",
            guide.id
        );
        assert_ne!(guide.id, "close");
        assert!(!seen.contains(&guide.id), "{} twice", guide.id);
        seen.push(guide.id);
        assert_ne!(guide.steps.len(), 0);
        assert!(guide.width > 0 && guide.depth > 0);
    }
    assert!(seen.contains(&"bank-atm"));
    assert!(seen.contains(&"spawn-zones"));
    assert!(seen.contains(&"magnifier"));
    assert!(seen.contains(&"magic-herb"));
    assert!(seen.contains(&"magic-broth"));
    assert!(seen.contains(&"spawn-zones-auction"));
    assert!(seen.contains(&"bank-terminal"));
}

#[test]
fn the_items_of_a_guide_tell_of_it() {
    assert_eq!(
        lore_of(&CARD).as_deref(),
        Some("Shift + ПКМ в воздух: гайд «Банк: банкомат и карта»")
    );
    assert_eq!(
        lore_of(&MAGNIFIER).as_deref(),
        Some("Shift + ПКМ в воздух: гайд «Лупа»")
    );
}

#[test]
fn a_step_lasts_as_long_as_its_caption_reads() {
    assert_eq!(step_ticks(90, "Коротко"), 90);
    // 100 characters at 2.2 ticks each.
    assert_eq!(step_ticks(90, &"я".repeat(100)), 220);
}

#[test]
fn the_caption_names_the_guide_and_the_step() {
    let guide = by_id_any("bank-atm");
    let text = format!("{:?}", caption(guide, 1));
    assert!(text.contains("Банк: банкомат и карта  ·  2/4"), "{text}");
    assert!(text.contains("Положи алмазную руду"), "{text}");
}

fn by_id_any(id: &str) -> &'static Guide {
    all()
        .iter()
        .find(|guide| guide.id == id)
        .unwrap_or_else(|| panic!("no guide {id}"))
}

#[test]
fn the_list_runs_the_command_of_each_guide() {
    let guides: Vec<&Guide> = all().iter().collect();
    let text = format!("{:?}", list_text(&guides));
    assert!(text.contains("/guide bank-atm"), "{text}");
    assert!(text.contains("▶ Банк: терминал оплаты"), "{text}");
}

#[test]
fn a_matrix_goes_to_the_client_as_a_place_a_turn_and_a_size() {
    let turn = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.5);
    let matrix = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0))
        * Mat4::from_quat(turn)
        * Mat4::from_scale(Vec3::new(0.28, 0.56, 0.14));
    let mut element = Element::text(TextComponent::plain(""), 200, 0);
    element.transform(&matrix, 5);
    let display = element.display();
    let at = display.translation.get();
    assert!((Vec3::new(at.x, at.y, at.z) - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-5);
    let size = display.scale.get();
    assert!((Vec3::new(size.x, size.y, size.z) - Vec3::new(0.28, 0.56, 0.14)).length() < 1e-5);
    let left = display.left_rotation.get();
    let left = Quat::from_xyzw(left.x, left.y, left.z, left.w);
    assert!(left.dot(turn).abs() > 0.9999);
    assert_eq!(*display.transformation_interpolation_duration.get(), 5);
}
