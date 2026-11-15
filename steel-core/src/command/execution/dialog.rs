//! Vanilla `Dialog.DIRECT_CODEC` for dialogs written inline in a command.
//!
//! Steel never acts on an inline dialog: it only sends it to clients, which
//! decode it with the same codec. So the dialog is checked the way vanilla
//! decodes one, and the checked tag is sent instead of a re-encoded copy.
//! The error texts are Steel's own, not the ones of vanilla's codec library.

use simdnbt::owned::{NbtCompound, NbtTag};
use steel_registry::dialog::DialogRef;
use steel_registry::{REGISTRY, RegistryExt as _, TaggedRegistryExt as _};
use steel_utils::Identifier;
use steel_utils::nbt::{NbtNumeric as _, nbt_collection_values};
use text_components::TextComponent;
use text_components::content::Content;
use text_components::interactivity::{ClickEvent, Dialog, HoverEvent};

use super::item::{component_value_is_valid, read_component_value};
use super::text::validate_component_syntax;
use crate::command::function::{StringTemplate, is_valid_variable_name};

type Checked = Result<(), String>;

/// Vanilla `Holder<Dialog>` from the dialog argument.
#[derive(Clone, Debug)]
pub(crate) enum CommandDialog {
    /// A dialog of the registry.
    Reference(DialogRef),
    /// A checked dialog written inline.
    Direct(NbtTag),
}

/// Vanilla `Dialog.WIDTH_CODEC`.
const WIDTHS: (i32, i32) = (1, 1024);

/// Vanilla `Dialog.DIRECT_CODEC`: a dialog dispatched on its `type`.
pub(super) fn check_dialog(tag: &NbtTag) -> Checked {
    let map = compound(tag)?;
    match registry_name(required(map, "type")?)?.as_str() {
        "notice" => {
            check_common(map)?;
            optional(map, "action", check_action_button)
        }
        "confirmation" => {
            check_common(map)?;
            check_action_button(required(map, "yes")?)?;
            check_action_button(required(map, "no")?)
        }
        "multi_action" => {
            check_common(map)?;
            non_empty_list(required(map, "actions")?, check_action_button)?;
            check_button_list(map)
        }
        "server_links" => {
            check_common(map)?;
            check_button_list(map)?;
            optional(map, "button_width", |tag| int_range(tag, WIDTHS))
        }
        "dialog_list" => {
            check_common(map)?;
            check_dialog_holder_set(required(map, "dialogs")?)?;
            check_button_list(map)?;
            optional(map, "button_width", |tag| int_range(tag, WIDTHS))
        }
        other => Err(format!("Unknown dialog type: {other}")),
    }
}

/// Vanilla `Dialog.CODEC`: a registry id or an inline dialog.
fn check_dialog_holder(tag: &NbtTag) -> Checked {
    match identifier(tag) {
        Some(id) => check_dialog_id(&id),
        None => check_dialog(tag),
    }
}

/// A dialog holder's id must name a registry dialog.
fn check_dialog_id(id: &Identifier) -> Checked {
    REGISTRY
        .dialogs
        .by_key(id)
        .map(|_| ())
        .ok_or_else(|| format!("Failed to get element {id} from registry minecraft:dialog"))
}

/// Vanilla `Dialog.LIST_CODEC`: a `#tag`, or one or more dialog holders.
fn check_dialog_holder_set(tag: &NbtTag) -> Checked {
    if let NbtTag::String(value) = tag
        && let Some(tag_id) = value.to_str().strip_prefix('#')
    {
        let tag_id = tag_id
            .parse::<Identifier>()
            .map_err(|_| format!("Not a valid tag: #{tag_id}"))?;
        return REGISTRY
            .dialogs
            .get_tag(&tag_id)
            .map(|_| ())
            .ok_or_else(|| format!("Missing tag: '{tag_id}' in 'minecraft:dialog'"));
    }
    compact_list(tag, check_dialog_holder)
}

/// Vanilla `CommonDialogData.MAP_CODEC`.
fn check_common(map: &NbtCompound) -> Checked {
    check_component(required(map, "title")?)?;
    optional(map, "external_title", check_component)?;
    let pause = optional_bool(map, "pause", true)?;
    optional_bool(map, "can_close_with_escape", true)?;
    let after_action = match map.get("after_action") {
        Some(tag) => string(tag)?,
        None => "close".to_owned(),
    };
    let unpauses = match after_action.as_str() {
        "close" | "wait_for_response" => true,
        "none" => false,
        other => return Err(format!("Unknown element name:{other}")),
    };
    optional(map, "body", |tag| compact_list(tag, check_body))?;
    optional(map, "inputs", |tag| list(tag, check_input))?;
    if pause && !unpauses {
        return Err(
            "Dialogs that pause the game must use after_action values that unpause it after user action!"
                .to_owned(),
        );
    }
    Ok(())
}

/// The fields vanilla `ButtonListDialog`s share.
fn check_button_list(map: &NbtCompound) -> Checked {
    optional(map, "exit_action", check_action_button)?;
    optional(map, "columns", positive_int)
}

/// Vanilla `ActionButton.CODEC`.
fn check_action_button(tag: &NbtTag) -> Checked {
    let map = compound(tag)?;
    check_component(required(map, "label")?)?;
    optional(map, "tooltip", check_component)?;
    optional(map, "width", |tag| int_range(tag, WIDTHS))?;
    optional(map, "action", check_action)
}

/// Vanilla `Action.CODEC`, dispatched on its `type`: a click event the
/// server may send, or a dynamic action.
fn check_action(tag: &NbtTag) -> Checked {
    let map = compound(tag)?;
    match registry_name(required(map, "type")?)?.as_str() {
        "open_url" => check_url(required(map, "url")?),
        "run_command" | "suggest_command" => check_chat_string(required(map, "command")?),
        "change_page" => positive_int(required(map, "page")?),
        "copy_to_clipboard" => string(required(map, "value")?).map(drop),
        "show_dialog" => check_dialog_holder(required(map, "dialog")?),
        "custom" => {
            required_identifier(map, "id")?;
            Ok(())
        }
        "dynamic/run_command" => {
            let template = string(required(map, "template")?)?;
            StringTemplate::parse(&template)
                .map(drop)
                .map_err(|error| format!("Failed to parse template {template}: {error}"))
        }
        "dynamic/custom" => {
            required_identifier(map, "id")?;
            optional(map, "additions", |tag| compound(tag).map(drop))
        }
        other => Err(format!("Unknown action type: {other}")),
    }
}

/// Vanilla `DialogBody.DIALOG_BODY_CODEC`.
fn check_body(tag: &NbtTag) -> Checked {
    let map = compound(tag)?;
    match registry_name(required(map, "type")?)?.as_str() {
        "plain_message" => check_plain_message_map(map),
        "item" => {
            check_item_template(required(map, "item")?)?;
            optional(map, "description", check_plain_message)?;
            optional_bool(map, "show_decorations", true)?;
            optional_bool(map, "show_tooltip", true)?;
            optional(map, "width", |tag| int_range(tag, (1, 256)))?;
            optional(map, "height", |tag| int_range(tag, (1, 256)))
        }
        other => Err(format!("Unknown dialog body type: {other}")),
    }
}

/// Vanilla `PlainMessage.CODEC`: the message map, or just its contents.
fn check_plain_message(tag: &NbtTag) -> Checked {
    match tag {
        NbtTag::Compound(map) if check_plain_message_map(map).is_ok() => Ok(()),
        _ => check_component(tag),
    }
}

/// Vanilla `PlainMessage.MAP_CODEC`.
fn check_plain_message_map(map: &NbtCompound) -> Checked {
    check_component(required(map, "contents")?)?;
    optional(map, "width", |tag| int_range(tag, WIDTHS))
}

/// Vanilla `ItemStackTemplate.CODEC`: an item id, or the item, count and
/// component patch.
fn check_item_template(tag: &NbtTag) -> Checked {
    let Some(map) = as_map(tag) else {
        return check_item(tag);
    };
    check_item(required(map, "id")?)?;
    optional(map, "count", |tag| int_range(tag, (1, 99)))?;
    optional(map, "components", check_component_patch)
}

/// Vanilla `Item.CODEC`: any item but air.
fn check_item(tag: &NbtTag) -> Checked {
    let id = identifier(tag).ok_or("Not a valid item id")?;
    match REGISTRY.items.by_key(&id) {
        Some(item) if item.key == Identifier::vanilla_static("air") => {
            Err("Item must not be minecraft:air".to_owned())
        }
        Some(_) => Ok(()),
        None => Err(format!("Unknown registry key in item: {id}")),
    }
}

/// Vanilla `DataComponentPatch.CODEC`: persistent components by id, or
/// removals as `!id`.
fn check_component_patch(tag: &NbtTag) -> Checked {
    for (key, value) in compound(tag)?.iter() {
        let key = key.to_str();
        let (removed, id) = match key.strip_prefix('!') {
            Some(id) => (true, id),
            None => (false, key.as_ref()),
        };
        let id = id
            .parse::<Identifier>()
            .map_err(|_| format!("Not a valid component id: {key}"))?;
        let entry = REGISTRY
            .data_components
            .by_key(&id)
            .filter(|entry| entry.is_persistent())
            .ok_or_else(|| format!("No component with type: {id}"))?;
        if removed {
            compound(value)?;
            continue;
        }
        let data = read_component_value(entry, value)
            .ok_or_else(|| format!("Malformed '{id}' component"))?;
        if !component_value_is_valid(&id, &data) {
            return Err(format!("Malformed '{id}' component"));
        }
    }
    Ok(())
}

/// Vanilla `Input.CODEC`: a variable name and an input control dispatched
/// on its `type`.
fn check_input(tag: &NbtTag) -> Checked {
    let map = compound(tag)?;
    let key = string(required(map, "key")?)?;
    if !is_valid_variable_name(&key) {
        return Err(format!("{key} is not a valid input name"));
    }
    match registry_name(required(map, "type")?)?.as_str() {
        "boolean" => {
            check_component(required(map, "label")?)?;
            optional_bool(map, "initial", false)?;
            optional(map, "on_true", |tag| string(tag).map(drop))?;
            optional(map, "on_false", |tag| string(tag).map(drop))
        }
        "number_range" => check_number_range(map),
        "single_option" => check_single_option(map),
        "text" => check_text_input(map),
        other => Err(format!("Unknown input control type: {other}")),
    }
}

/// Vanilla `NumberRangeInput.MAP_CODEC` with its `RangeInfo`.
fn check_number_range(map: &NbtCompound) -> Checked {
    optional(map, "width", |tag| int_range(tag, WIDTHS))?;
    check_component(required(map, "label")?)?;
    optional(map, "label_format", |tag| string(tag).map(drop))?;
    let start = float(required(map, "start")?)?;
    let end = float(required(map, "end")?)?;
    optional(map, "step", |tag| {
        if float(tag)? > 0.0 {
            Ok(())
        } else {
            Err("Value must be positive".to_owned())
        }
    })?;
    if let Some(initial) = map.get("initial") {
        let initial = f64::from(float(initial)?);
        let (min, max) = (f64::from(start.min(end)), f64::from(start.max(end)));
        if initial < min || initial > max {
            return Err(format!(
                "Initial value {initial} is outside of range [{min}, {max}]"
            ));
        }
    }
    Ok(())
}

/// Vanilla `SingleOptionInput.MAP_CODEC`.
fn check_single_option(map: &NbtCompound) -> Checked {
    optional(map, "width", |tag| int_range(tag, WIDTHS))?;
    let options = required(map, "options")?;
    non_empty_list(options, check_option_entry)?;
    check_component(required(map, "label")?)?;
    optional_bool(map, "label_visible", true)?;
    let initial = nbt_collection_values(options)
        .unwrap_or_default()
        .iter()
        .filter(|entry| {
            as_map(entry)
                .is_some_and(|entry| optional_bool(entry, "initial", false).unwrap_or(false))
        })
        .count();
    if initial > 1 {
        return Err("Multiple initial values".to_owned());
    }
    Ok(())
}

/// Vanilla `SingleOptionInput.Entry.CODEC`: the entry map, or just its id.
fn check_option_entry(tag: &NbtTag) -> Checked {
    let Some(map) = as_map(tag) else {
        return string(tag).map(drop);
    };
    string(required(map, "id")?)?;
    optional(map, "display", check_component)?;
    optional_bool(map, "initial", false).map(drop)
}

/// Vanilla `TextInput.MAP_CODEC`.
fn check_text_input(map: &NbtCompound) -> Checked {
    optional(map, "width", |tag| int_range(tag, WIDTHS))?;
    check_component(required(map, "label")?)?;
    optional_bool(map, "label_visible", true)?;
    let initial = match map.get("initial") {
        Some(tag) => string(tag)?,
        None => String::new(),
    };
    let max_length = match map.get("max_length") {
        Some(tag) => {
            positive_int(tag)?;
            int(tag)?
        }
        None => 32,
    };
    optional(map, "multiline", |tag| {
        let multiline = compound(tag)?;
        optional(multiline, "max_lines", positive_int)?;
        optional(multiline, "height", |tag| int_range(tag, (1, 512)))
    })?;
    // Vanilla compares Java string lengths, in UTF-16 units.
    if initial.encode_utf16().count() > usize::try_from(max_length).unwrap_or(0) {
        return Err("Default text length exceeds allowed size".to_owned());
    }
    Ok(())
}

/// Vanilla `ComponentSerialization.CODEC`, as Steel's component argument
/// checks it, with the dialogs its click events show.
fn check_component(tag: &NbtTag) -> Checked {
    let component = TextComponent::try_from_nbt(tag).map_err(|error| error.to_string())?;
    validate_component_syntax(&component)?;
    check_component_dialogs(&component)
}

/// Checks the dialogs that `show_dialog` click events in `component` show.
fn check_component_dialogs(component: &TextComponent) -> Checked {
    if let Some(ClickEvent::ShowDialog { dialog }) = &component.interactions.click {
        match dialog {
            Dialog::Reference(id) => {
                let id = id
                    .parse::<Identifier>()
                    .map_err(|_| format!("Not a valid dialog id: {id}"))?;
                check_dialog_id(&id)?;
            }
            Dialog::Inline(dialog) => check_dialog(&dialog.clone().into_nbt())?,
        }
    }
    if let Content::Translate(message) = &component.content
        && let Some(arguments) = &message.args
    {
        for argument in arguments {
            check_component_dialogs(argument)?;
        }
    }
    for child in &component.children {
        check_component_dialogs(child)?;
    }
    match &component.interactions.hover {
        Some(
            HoverEvent::ShowText { value }
            | HoverEvent::ShowEntity {
                name: Some(value), ..
            },
        ) => check_component_dialogs(value),
        _ => Ok(()),
    }
}

/// Vanilla `ExtraCodecs.UNTRUSTED_URI`: an absolute `http` or `https` URI.
fn check_url(tag: &NbtTag) -> Checked {
    let url = string(tag)?;
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    match scheme.as_deref() {
        Some("http" | "https") => Ok(()),
        _ => Err(format!("Unsupported protocol in URL: {url}")),
    }
}

/// Vanilla `ExtraCodecs.CHAT_STRING`: no `§` and no control characters.
fn check_chat_string(tag: &NbtTag) -> Checked {
    let value = string(tag)?;
    if value
        .chars()
        .any(|character| character == '\u{a7}' || character < ' ' || character == '\u{7f}')
    {
        return Err(format!("Disallowed chat characters: '{value}'"));
    }
    Ok(())
}

/// Vanilla `ExtraCodecs.compactListCodec`: a list, or a single element.
fn compact_list(tag: &NbtTag, element: impl Fn(&NbtTag) -> Checked) -> Checked {
    match nbt_collection_values(tag) {
        Some(values) => values.iter().try_for_each(element),
        None => element(tag),
    }
}

/// Vanilla `Codec.listOf`.
fn list(tag: &NbtTag, element: impl Fn(&NbtTag) -> Checked) -> Checked {
    nbt_collection_values(tag)
        .ok_or_else(|| "Not a list".to_owned())?
        .iter()
        .try_for_each(element)
}

/// Vanilla `ExtraCodecs.nonEmptyList` of `Codec.listOf`.
fn non_empty_list(tag: &NbtTag, element: impl Fn(&NbtTag) -> Checked) -> Checked {
    if nbt_collection_values(tag).is_some_and(|values| values.is_empty()) {
        return Err("List must have contents".to_owned());
    }
    list(tag, element)
}

/// Vanilla `fieldOf`: the field must be present.
fn required<'a>(map: &'a NbtCompound, field: &str) -> Result<&'a NbtTag, String> {
    map.get(field).ok_or_else(|| format!("No key {field}"))
}

/// Vanilla `optionalFieldOf`: absent is fine, present must decode.
fn optional(map: &NbtCompound, field: &str, check: impl FnOnce(&NbtTag) -> Checked) -> Checked {
    map.get(field).map_or(Ok(()), check)
}

/// Vanilla `Codec.BOOL.optionalFieldOf(field, default)`.
fn optional_bool(map: &NbtCompound, field: &str, default: bool) -> Result<bool, String> {
    map.get(field).map_or(Ok(default), |tag| {
        tag.codec_bool().ok_or_else(|| "Not a boolean".to_owned())
    })
}

/// Vanilla `Identifier.CODEC.fieldOf(field)`.
fn required_identifier(map: &NbtCompound, field: &str) -> Result<Identifier, String> {
    let tag = required(map, field)?;
    identifier(tag).ok_or_else(|| format!("Not a valid resource location: {field}"))
}

/// The path of a registry name, which defaults to the `minecraft` namespace,
/// for the vanilla types a dispatch knows.
fn registry_name(tag: &NbtTag) -> Result<String, String> {
    let id = identifier(tag).ok_or("Not a valid registry name")?;
    if id.namespace != "minecraft" {
        return Err(format!("Unknown registry key: {id}"));
    }
    Ok(id.path.to_string())
}

fn identifier(tag: &NbtTag) -> Option<Identifier> {
    match tag {
        NbtTag::String(value) => value.to_str().parse().ok(),
        _ => None,
    }
}

fn compound(tag: &NbtTag) -> Result<&NbtCompound, String> {
    as_map(tag).ok_or_else(|| "Not a map".to_owned())
}

const fn as_map(tag: &NbtTag) -> Option<&NbtCompound> {
    match tag {
        NbtTag::Compound(map) => Some(map),
        _ => None,
    }
}

fn string(tag: &NbtTag) -> Result<String, String> {
    match tag {
        NbtTag::String(value) => Ok(value.to_str().into_owned()),
        _ => Err("Not a string".to_owned()),
    }
}

fn int(tag: &NbtTag) -> Result<i32, String> {
    tag.codec_i32().ok_or_else(|| "Not a number".to_owned())
}

fn float(tag: &NbtTag) -> Result<f32, String> {
    tag.codec_f32().ok_or_else(|| "Not a number".to_owned())
}

/// Vanilla `ExtraCodecs.intRange`.
fn int_range(tag: &NbtTag, (min, max): (i32, i32)) -> Checked {
    let value = int(tag)?;
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(format!("Value must be within range [{min};{max}]: {value}"))
    }
}

/// Vanilla `ExtraCodecs.POSITIVE_INT`.
fn positive_int(tag: &NbtTag) -> Checked {
    let value = int(tag)?;
    if value > 0 {
        Ok(())
    } else {
        Err(format!("Value must be positive: {value}"))
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use steel_utils::nbt::parse_snbt_argument;

    use super::check_dialog;

    fn check(snbt: &str) -> Result<(), String> {
        init_vanilla_registry();
        let (tag, _) = parse_snbt_argument(snbt).expect("test SNBT parses");
        check_dialog(&tag)
    }

    #[track_caller]
    fn accepts(snbt: &str) {
        if let Err(error) = check(snbt) {
            panic!("{snbt} should be a dialog: {error}");
        }
    }

    #[track_caller]
    fn rejects(snbt: &str) {
        assert!(check(snbt).is_err(), "{snbt} should not be a dialog");
    }

    #[test]
    fn dialogs_dispatch_on_type_with_common_data() {
        accepts(r#"{type:"notice",title:"Hi"}"#);
        accepts(
            r#"{type:"minecraft:notice",title:{text:"Hi"},body:{type:"plain_message",contents:"Plain"}}"#,
        );
        // A body is always a typed map, never a bare component.
        rejects(r#"{type:"notice",title:"Hi",body:"Plain"}"#);
        rejects(r#"{type:"notice"}"#);
        rejects(r#"{type:"poster",title:"Hi"}"#);
        rejects(r#"{type:"other:notice",title:"Hi"}"#);
        // Pausing dialogs must unpause after an action.
        rejects(r#"{type:"notice",title:"Hi",after_action:"none"}"#);
        accepts(r#"{type:"notice",title:"Hi",after_action:"none",pause:false}"#);
        rejects(r#"{type:"notice",title:"Hi",after_action:"stay"}"#);
        rejects(r#"{type:"confirmation",title:"Hi",yes:{label:"Yes"}}"#);
        accepts(r#"{type:"confirmation",title:"Hi",yes:{label:"Yes"},no:{label:"No"}}"#);
    }

    #[test]
    fn buttons_and_actions_follow_their_codecs() {
        rejects(r#"{type:"multi_action",title:"Hi",actions:[]}"#);
        accepts(
            r#"{type:"multi_action",title:"Hi",columns:3,actions:[
                {label:"Run",width:100,action:{type:"run_command",command:"say hi"}},
                {label:"Open",action:{type:"open_url",url:"https://example.com"}},
                {label:"Copy",action:{type:"copy_to_clipboard",value:"x"}},
                {label:"Ask",action:{type:"dynamic/run_command",template:"say $(name)"}},
                {label:"Send",action:{type:"dynamic/custom",id:"demo:send",additions:{a:1}}}
            ]}"#,
        );
        rejects(r#"{type:"multi_action",title:"Hi",columns:0,actions:[{label:"A"}]}"#);
        rejects(r#"{type:"notice",title:"Hi",action:{label:"A",width:2000}}"#);
        rejects(
            r#"{type:"notice",title:"Hi",action:{label:"A",action:{type:"open_url",url:"ftp://x"}}}"#,
        );
        rejects(
            r#"{type:"notice",title:"Hi",action:{label:"A",action:{type:"run_command",command:"§c"}}}"#,
        );
        rejects(
            r#"{type:"notice",title:"Hi",action:{label:"A",action:{type:"dynamic/run_command",template:"say"}}}"#,
        );
        rejects(r#"{type:"notice",title:"Hi",action:{label:"A",action:{type:"open_file"}}}"#);
    }

    #[test]
    fn dialog_lists_hold_tags_ids_and_inline_dialogs() {
        accepts(r##"{type:"dialog_list",title:"Hi",dialogs:"#minecraft:quick_actions"}"##);
        rejects(r##"{type:"dialog_list",title:"Hi",dialogs:"#minecraft:missing"}"##);
        accepts(r#"{type:"dialog_list",title:"Hi",dialogs:"minecraft:server_links"}"#);
        rejects(r#"{type:"dialog_list",title:"Hi",dialogs:"minecraft:missing"}"#);
        accepts(
            r#"{type:"dialog_list",title:"Hi",dialogs:["server_links",{type:"notice",title:"In"}]}"#,
        );
        rejects(r#"{type:"dialog_list",title:"Hi",dialogs:[{type:"notice"}]}"#);
        accepts(
            r#"{type:"notice",title:"Hi",action:{label:"A",action:{type:"show_dialog",dialog:{type:"notice",title:"B"}}}}"#,
        );
        rejects(
            r#"{type:"notice",title:"Hi",action:{label:"A",action:{type:"show_dialog",dialog:{type:"notice"}}}}"#,
        );
        // Dialogs shown from text are checked too.
        rejects(
            r#"{type:"notice",title:{text:"Hi",click_event:{action:"show_dialog",dialog:{type:"notice"}}}}"#,
        );
    }

    #[test]
    fn item_bodies_take_item_templates() {
        accepts(r#"{type:"notice",title:"Hi",body:{type:"item",item:"minecraft:diamond"}}"#);
        accepts(
            r#"{type:"notice",title:"Hi",body:[{type:"item",item:{id:"stone",count:5,components:{"minecraft:max_stack_size":10,"!minecraft:food":{}}},description:"Rock",width:32}]}"#,
        );
        rejects(r#"{type:"notice",title:"Hi",body:{type:"item",item:"minecraft:air"}}"#);
        rejects(r#"{type:"notice",title:"Hi",body:{type:"item",item:{id:"stone",count:100}}}"#);
        rejects(
            r#"{type:"notice",title:"Hi",body:{type:"item",item:{id:"stone",components:{"minecraft:max_stack_size":100}}}}"#,
        );
        rejects(r#"{type:"notice",title:"Hi",body:{type:"item",item:"stone",height:0}}"#);
    }

    #[test]
    fn inputs_check_their_keys_and_values() {
        accepts(
            r#"{type:"notice",title:"Hi",inputs:[
                {type:"text",key:"name",label:"Name",initial:"Alex",max_length:8,multiline:{max_lines:2,height:40}},
                {type:"boolean",key:"on",label:"On",initial:true},
                {type:"single_option",key:"pick",label:"Pick",options:["a",{id:"b",initial:true}]},
                {type:"number_range",key:"n",label:"N",start:0,end:10,step:1,initial:5}
            ]}"#,
        );
        rejects(r#"{type:"notice",title:"Hi",inputs:[{type:"boolean",key:"bad-key",label:"A"}]}"#);
        rejects(
            r#"{type:"notice",title:"Hi",inputs:[{type:"text",key:"t",label:"T",initial:"too long",max_length:3}]}"#,
        );
        rejects(
            r#"{type:"notice",title:"Hi",inputs:[{type:"single_option",key:"s",label:"S",options:[{id:"a",initial:true},{id:"b",initial:true}]}]}"#,
        );
        rejects(
            r#"{type:"notice",title:"Hi",inputs:[{type:"single_option",key:"s",label:"S",options:[]}]}"#,
        );
        rejects(
            r#"{type:"notice",title:"Hi",inputs:[{type:"number_range",key:"n",label:"N",start:0,end:10,initial:11}]}"#,
        );
        rejects(
            r#"{type:"notice",title:"Hi",inputs:[{type:"number_range",key:"n",label:"N",start:0,end:10,step:0}]}"#,
        );
    }
}
