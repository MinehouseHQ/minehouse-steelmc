//! Vanilla `/attribute` (`AttributeCommand`): read and change a living
//! entity's attribute base values and modifiers.

use std::borrow::Cow;

use steel_registry::attribute::{AttributeModifierOperation, AttributeRef};
use steel_utils::{Identifier, java::double_to_string, translations};
use text_components::{TextComponent, translation::TranslatedMessage};

use crate::command::brigadier::{
    ArgumentType, CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder,
};
use crate::command::execution::{
    CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
    SteelSuggestionContext, argument, literal, suggest_resources,
};
use crate::command::registration::CommandRegistration;
use crate::entity::attribute::{AttributeMap, AttributeModifier};
use crate::entity::{LivingEntity, SharedEntity};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("attribute"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("attribute").then(
        argument("target", SteelArgumentType::entity()).then(
            argument("attribute", SteelArgumentType::attribute())
                .then(
                    literal("get")
                        .executes(|context| get_value(context, 1.0))
                        .then(
                            scale()
                                .executes(|context| get_value(context, context.double("scale")?)),
                        ),
                )
                .then(base())
                .then(modifier()),
        ),
    )
}

/// Vanilla `DoubleArgumentType.doubleArg()`.
fn double(name: &'static str) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    argument(name, ArgumentType::double(f64::MIN, f64::MAX))
}

fn scale() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    double("scale")
}

fn base() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("base")
        .then(literal("set").then(double("value").executes(set_base)))
        .then(
            literal("get")
                .executes(|context| get_base(context, 1.0))
                .then(scale().executes(|context| get_base(context, context.double("scale")?))),
        )
        .then(literal("reset").executes(reset_base))
}

fn modifier() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    let operation = |name: &'static str, operation: AttributeModifierOperation| {
        literal(name).executes(move |context| add_modifier(context, operation))
    };
    literal("modifier")
        .then(
            literal("add").then(
                argument("id", SteelArgumentType::identifier()).then(
                    double("value")
                        .then(operation("add_value", AttributeModifierOperation::AddValue))
                        .then(operation(
                            "add_multiplied_base",
                            AttributeModifierOperation::AddMultipliedBase,
                        ))
                        .then(operation(
                            "add_multiplied_total",
                            AttributeModifierOperation::AddMultipliedTotal,
                        )),
                ),
            ),
        )
        .then(
            literal("remove").then(
                argument("id", SteelArgumentType::identifier())
                    .suggests(suggest_modifiers)
                    .executes(remove_modifier),
            ),
        )
        .then(
            literal("value").then(
                literal("get").then(
                    argument("id", SteelArgumentType::identifier())
                        .suggests(suggest_modifiers)
                        .executes(|context| get_modifier(context, 1.0))
                        .then(
                            scale().executes(|context| {
                                get_modifier(context, context.double("scale")?)
                            }),
                        ),
                ),
            ),
        )
}

/// Vanilla `getAttributeModifiers`: the ids of every modifier on the
/// target's attribute.
fn suggest_modifiers(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let (Ok(target), Ok(attribute)) = (context.entity("target"), context.attribute("attribute"))
    else {
        return;
    };
    let Some(living) = target.as_living_entity() else {
        return;
    };
    let attributes = living.attributes().lock();
    let Some(instance) = attributes.get_instance(attribute) else {
        return;
    };
    suggest_resources(instance.modifiers().map(|modifier| &modifier.id), builder);
}

/// The target and attribute every branch starts from.
struct Target {
    entity: SharedEntity,
    attribute: AttributeRef,
}

impl Target {
    fn of(context: &SteelCommandContext<CommandSource>) -> Result<Self, CommandSyntaxError> {
        Ok(Self {
            entity: context.entity("target")?,
            attribute: context.attribute("attribute")?,
        })
    }

    /// Vanilla `getLivingEntity`.
    fn living(&self) -> Result<&dyn LivingEntity, CommandSyntaxError> {
        self.entity.as_living_entity().ok_or_else(|| {
            CommandSyntaxError::dynamic(
                translations::COMMANDS_ATTRIBUTE_FAILED_ENTITY
                    .message([self.entity.display_name()])
                    .component(),
            )
        })
    }

    /// Vanilla `getEntityWithAttribute` and `getAttributeInstance`: the
    /// target's attributes, when it has this one.
    fn with_attribute<T>(
        &self,
        action: impl FnOnce(&mut AttributeMap) -> Result<T, CommandSyntaxError>,
    ) -> Result<T, CommandSyntaxError> {
        let mut attributes = self.living()?.attributes().lock();
        if !attributes.has_attribute(self.attribute) {
            return Err(self.no_attribute());
        }
        action(&mut attributes)
    }

    fn no_attribute(&self) -> CommandSyntaxError {
        CommandSyntaxError::dynamic(
            translations::COMMANDS_ATTRIBUTE_FAILED_NO_ATTRIBUTE
                .message([self.entity.display_name(), self.description()])
                .component(),
        )
    }

    fn no_modifier(&self, id: &Identifier) -> CommandSyntaxError {
        CommandSyntaxError::dynamic(
            translations::COMMANDS_ATTRIBUTE_FAILED_NO_MODIFIER
                .message([
                    self.description(),
                    self.entity.display_name(),
                    TextComponent::plain(id.to_string()),
                ])
                .component(),
        )
    }

    /// Vanilla `getAttributeDescription`.
    const fn description(&self) -> TextComponent {
        TextComponent::translated(TranslatedMessage {
            key: Cow::Borrowed(self.attribute.translation_key),
            args: None,
            fallback: None,
        })
    }
}

/// Vanilla `(int)(result * scale)`: Java's saturating double to int cast.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Rust float to int casts saturate and map NaN to zero, like Java's"
)]
fn scaled(value: f64, scale: f64) -> i32 {
    (value * scale) as i32
}

fn get_value(
    context: &SteelCommandContext<CommandSource>,
    scale: f64,
) -> Result<i32, CommandSyntaxError> {
    let target = Target::of(context)?;
    let value = target.with_attribute(|attributes| {
        Ok(attributes.get_value(target.attribute).unwrap_or_default())
    })?;
    send(
        context,
        translations::COMMANDS_ATTRIBUTE_VALUE_GET_SUCCESS.message([
            target.description(),
            target.entity.display_name(),
            TextComponent::plain(double_to_string(value)),
        ]),
    );
    Ok(scaled(value, scale))
}

fn get_base(
    context: &SteelCommandContext<CommandSource>,
    scale: f64,
) -> Result<i32, CommandSyntaxError> {
    let target = Target::of(context)?;
    let value = target.with_attribute(|attributes| {
        Ok(attributes
            .get_base_value(target.attribute)
            .unwrap_or_default())
    })?;
    send(
        context,
        translations::COMMANDS_ATTRIBUTE_BASE_VALUE_GET_SUCCESS.message([
            target.description(),
            target.entity.display_name(),
            TextComponent::plain(double_to_string(value)),
        ]),
    );
    Ok(scaled(value, scale))
}

fn get_modifier(
    context: &SteelCommandContext<CommandSource>,
    scale: f64,
) -> Result<i32, CommandSyntaxError> {
    let target = Target::of(context)?;
    let id = context.identifier("id")?;
    let value = target.with_attribute(|attributes| {
        attributes
            .get_instance(target.attribute)
            .and_then(|instance| instance.modifier(id))
            .map(|modifier| modifier.amount)
            .ok_or_else(|| target.no_modifier(id))
    })?;
    send(
        context,
        translations::COMMANDS_ATTRIBUTE_MODIFIER_VALUE_GET_SUCCESS.message([
            TextComponent::plain(id.to_string()),
            target.description(),
            target.entity.display_name(),
            TextComponent::plain(double_to_string(value)),
        ]),
    );
    Ok(scaled(value, scale))
}

fn set_base(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let target = Target::of(context)?;
    let value = context.double("value")?;
    target.with_attribute(|attributes| {
        attributes.set_base_value(target.attribute, value);
        Ok(())
    })?;
    send(
        context,
        translations::COMMANDS_ATTRIBUTE_BASE_VALUE_SET_SUCCESS.message([
            target.description(),
            target.entity.display_name(),
            TextComponent::plain(double_to_string(value)),
        ]),
    );
    Ok(1)
}

fn reset_base(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let target = Target::of(context)?;
    let entity_type = target.entity.entity_type();
    let value = {
        let mut attributes = target.living()?.attributes().lock();
        if !attributes.reset_base_value(target.attribute, entity_type) {
            return Err(target.no_attribute());
        }
        attributes
            .get_base_value(target.attribute)
            .unwrap_or_default()
    };
    send(
        context,
        translations::COMMANDS_ATTRIBUTE_BASE_VALUE_RESET_SUCCESS.message([
            target.description(),
            target.entity.display_name(),
            TextComponent::plain(double_to_string(value)),
        ]),
    );
    Ok(1)
}

fn add_modifier(
    context: &SteelCommandContext<CommandSource>,
    operation: AttributeModifierOperation,
) -> Result<i32, CommandSyntaxError> {
    let target = Target::of(context)?;
    let id = context.identifier("id")?;
    let modifier = AttributeModifier {
        id: id.clone(),
        amount: context.double("value")?,
        operation,
    };
    target.with_attribute(|attributes| {
        if attributes.add_modifier(target.attribute, modifier, true) {
            Ok(())
        } else {
            Err(CommandSyntaxError::dynamic(
                translations::COMMANDS_ATTRIBUTE_FAILED_MODIFIER_ALREADY_PRESENT
                    .message([
                        TextComponent::plain(id.to_string()),
                        target.description(),
                        target.entity.display_name(),
                    ])
                    .component(),
            ))
        }
    })?;
    send(
        context,
        translations::COMMANDS_ATTRIBUTE_MODIFIER_ADD_SUCCESS.message([
            TextComponent::plain(id.to_string()),
            target.description(),
            target.entity.display_name(),
        ]),
    );
    Ok(1)
}

fn remove_modifier(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let target = Target::of(context)?;
    let id = context.identifier("id")?;
    target.with_attribute(|attributes| {
        if attributes.remove_modifier(target.attribute, id) {
            Ok(())
        } else {
            Err(target.no_modifier(id))
        }
    })?;
    send(
        context,
        translations::COMMANDS_ATTRIBUTE_MODIFIER_REMOVE_SUCCESS.message([
            TextComponent::plain(id.to_string()),
            target.description(),
            target.entity.display_name(),
        ]),
    );
    Ok(1)
}

fn send(context: &SteelCommandContext<CommandSource>, message: TranslatedMessage) {
    context.source().send_success(&message.component(), false);
}
