//! Vanilla `FetchProfileCommand`.

use std::borrow::Cow;

use simdnbt::ToNbtTag as _;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_registry::resolvable_profile::ResolvableProfile;
use steel_utils::nbt::to_canonical_snbt;
use steel_utils::{Identifier, translations};
use text_components::content::{Object, ObjectPlayer, PlayerProperties};
use text_components::{
    Modifier as _, TextComponent, format::Color, interactivity::ClickEvent,
    translation::Translation,
};
use uuid::Uuid;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        join_components, literal,
    },
    registration::CommandRegistration,
};
use crate::entity::Entity as _;

type Context = SteelCommandContext<CommandSource>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("fetchprofile"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("fetchprofile")
        .then(
            literal("name").then(argument("name", ArgumentType::greedy_string()).executes(
                |context| {
                    resolve_name(context, context.string("name")?.to_owned());
                    Ok(1)
                },
            )),
        )
        .then(literal("id").then(
            argument("id", SteelArgumentType::uuid()).executes(|context| {
                resolve_id(context, context.uuid("id")?);
                Ok(1)
            }),
        ))
        .then(
            literal("entity")
                .then(argument("entity", SteelArgumentType::entity()).executes(print_for_entity)),
        )
}

/// Vanilla `resolveName`: looks the name up off the tick and reports later.
fn resolve_name(context: &Context, name: String) {
    let source = context.source().clone();
    tokio::spawn(async move {
        let profile = source.server().fetch_profile_by_name(&name).await;
        let argument = TextComponent::plain(name);
        match profile {
            Some(profile) => report_resolved_profile(
                &source,
                &translations::COMMANDS_FETCHPROFILE_NAME_SUCCESS,
                argument,
                &ResolvableProfile::from(profile),
            ),
            None => source.send_failure(
                translations::COMMANDS_FETCHPROFILE_NAME_FAILURE
                    .message([argument])
                    .component(),
            ),
        }
    });
}

/// Vanilla `resolveId`.
fn resolve_id(context: &Context, id: Uuid) {
    let source = context.source().clone();
    tokio::spawn(async move {
        let profile = source.server().fetch_profile_by_id(id).await;
        let argument = TextComponent::plain(id.to_string());
        match profile {
            Some(profile) => report_resolved_profile(
                &source,
                &translations::COMMANDS_FETCHPROFILE_ID_SUCCESS,
                argument,
                &ResolvableProfile::from(profile),
            ),
            None => source.send_failure(
                translations::COMMANDS_FETCHPROFILE_ID_FAILURE
                    .message([argument])
                    .component(),
            ),
        }
    });
}

/// Vanilla `printForEntity`: only players carry a profile.
fn print_for_entity(context: &Context) -> Result<i32, CommandSyntaxError> {
    let entity = context.entity("entity")?;
    let Some(player) = entity.as_player() else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_FETCHPROFILE_NO_PROFILE
                .message([entity.display_name()])
                .component(),
        ));
    };
    report_resolved_profile(
        context.source(),
        &translations::COMMANDS_FETCHPROFILE_ENTITY_SUCCESS,
        player.display_name(),
        &ResolvableProfile::from(player.gameprofile.clone()),
    );
    Ok(1)
}

/// Vanilla `reportResolvedProfile`: the profile as SNBT behind buttons that
/// copy it, give its head, summon a mannequin or copy a head text component.
fn report_resolved_profile(
    source: &CommandSource,
    message: &Translation<2>,
    argument: TextComponent,
    profile: &ResolvableProfile,
) {
    let Some(encoded_profile) = to_canonical_snbt(&profile.clone().to_nbt_tag()) else {
        source.send_failure(serialize_failure("profile"));
        return;
    };
    let head = head_component(profile);
    let Some(encoded_head) = to_canonical_snbt(&head.to_codec_nbt()) else {
        source.send_failure(serialize_failure("component"));
        return;
    };
    let buttons = [
        TextComponent::from(&translations::COMMANDS_FETCHPROFILE_COPY_COMPONENT)
            .click_event(ClickEvent::copy_to_clipboard(encoded_profile.clone())),
        TextComponent::from(&translations::COMMANDS_FETCHPROFILE_GIVE_ITEM).click_event(
            ClickEvent::run_command(format!(
                "give @s minecraft:player_head[profile={encoded_profile}]"
            )),
        ),
        TextComponent::from(&translations::COMMANDS_FETCHPROFILE_SUMMON_MANNEQUIN).click_event(
            ClickEvent::run_command(format!(
                "summon minecraft:mannequin ~ ~ ~ {{profile:{encoded_profile}}}"
            )),
        ),
        translations::COMMANDS_FETCHPROFILE_COPY_TEXT
            .message([head.color(Color::White)])
            .component()
            .click_event(ClickEvent::copy_to_clipboard(encoded_head)),
    ]
    .map(|button| {
        translations::CHAT_SQUARE_BRACKETS
            .message([button.color(Color::Green)])
            .component()
    });
    let clickable = join_components(buttons, &TextComponent::plain(" "));
    source.send_success(&message.message([argument, clickable]).component(), false);
}

fn serialize_failure(what: &str) -> TextComponent {
    translations::COMMANDS_FETCHPROFILE_FAILED_TO_SERIALIZE
        .message([format!("Failed to encode {what}")])
        .component()
}

/// Vanilla `Component.object(new PlayerSprite(profile, true))`.
fn head_component(profile: &ResolvableProfile) -> TextComponent {
    let player = match profile.clone().to_nbt_tag() {
        NbtTag::Compound(compound) => object_player(&compound),
        _ => object_player(&NbtCompound::new()),
    };
    TextComponent::from(Object::Player {
        player,
        hat: true,
        fallback: None,
    })
}

/// The profile fields of a `ResolvableProfile.CODEC` compound.
fn object_player(compound: &NbtCompound) -> ObjectPlayer {
    let string = |key: &str| -> Option<Cow<'static, str>> {
        compound
            .string(key)
            .map(|value| Cow::Owned(value.to_str().into_owned()))
    };
    let id = compound
        .int_array("id")
        .and_then(|id| <[i32; 4]>::try_from(id).ok());
    let properties = compound
        .list("properties")
        .and_then(|list| list.compounds())
        .map_or_else(Vec::new, |properties| {
            properties
                .iter()
                .filter_map(|property| {
                    Some(PlayerProperties {
                        name: Cow::Owned(property.string("name")?.to_str().into_owned()),
                        value: Cow::Owned(property.string("value")?.to_str().into_owned()),
                        signature: property
                            .string("signature")
                            .map(|signature| Cow::Owned(signature.to_str().into_owned())),
                    })
                })
                .collect()
        });
    ObjectPlayer {
        name: string("name"),
        id,
        texture: string("texture"),
        cape: string("cape"),
        elytra: string("elytra"),
        model: None,
        properties,
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::game_profile::{GameProfile, ProfileProperty};
    use text_components::content::{Content, Object};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn head_component_carries_the_profile_with_its_hat() {
        let id = Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
        let property = ProfileProperty::new("textures".into(), "skin".into(), Some("sig".into()))
            .expect("valid property");
        let profile = GameProfile::new(id, "Alex".into(), vec![property]).expect("valid profile");
        let head = head_component(&ResolvableProfile::from(profile));
        let Content::Object(Object::Player { player, hat, .. }) = head.content else {
            panic!("expected a player sprite, got {head:?}");
        };
        assert!(hat);
        assert_eq!(player.name.as_deref(), Some("Alex"));
        assert_eq!(
            player.id,
            Some([0x0102_0304, 0x0506_0708, 0x090a_0b0c, 0x0d0e_0f10])
        );
        assert_eq!(player.properties.len(), 1);
        assert_eq!(player.properties[0].signature.as_deref(), Some("sig"));
    }
}
