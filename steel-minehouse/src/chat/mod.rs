//! The chat: a port of the Fabric mod `minehouse-chat`.
//!
//! Local chat within a radius and global chat (`!text`, `/g`), private messages (`/msg`,
//! `/r`), `/ignore`, mention sounds, role badges, mutes (`/mute`, `/tempmute`, `/unmute`,
//! `/mutelist`) and admin announcements (`$$text`). Every text and option is in
//! `config/minehouse/chat.toml`, re-read by `/chat reload`.
//!
//! Like the mod, we deliver every chat message ourselves as a system message, so vanilla
//! never does and nothing here depends on signed chat.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::{
    command::CommandRegistry,
    entity::Entity,
    permission::{PermissionExpr, PermissionKey},
    player::Player,
    server::{Server, chat_interceptor::ChatInterceptor},
};
use steel_protocol::packets::game::{CSound, SoundSource};
use steel_registry::sound_event::SoundEventHolder;
use steel_utils::{Identifier, locks::SyncMutex};
use text_components::{
    Modifier, TextComponent,
    format::Color,
    interactivity::{ClickEvent, HoverEvent},
};
use uuid::Uuid;

use crate::{
    config,
    moderation::format::{format_duration, format_epoch_millis, render, render_parts},
    nitro,
};

mod commands;
pub mod settings;
pub mod store;

use settings::ChatConfig;
use store::{Ignores, Mute, Mutes};

/// Who may send `$$` announcements.
const ANNOUNCE_PERMISSION: &str = "minehouse.announce";
/// `{until}` and `{left}` of a mute without an end.
const FOREVER: &str = "навсегда";

/// The chat of one server.
pub struct Chat {
    config_file: PathBuf,
    config: SyncMutex<Arc<ChatConfig>>,
    ignores: Ignores,
    mutes: Mutes,
    /// Players who switched to global chat with `/g`.
    global_mode: SyncMutex<FxHashSet<Uuid>>,
    /// Whom `/r` answers.
    last_partner: SyncMutex<FxHashMap<Uuid, Uuid>>,
}

impl Chat {
    /// Loads the settings and adds the chat commands to `registry`. `None` when the config
    /// is broken: the server then keeps vanilla chat.
    #[must_use]
    pub fn register(registry: &mut CommandRegistry) -> Option<Arc<Self>> {
        let config_file = config::path("chat");
        let config = ChatConfig::load(&config_file)
            .inspect_err(|error| log::error!("Chat is off: {error}"))
            .ok()?;
        let chat = Arc::new(Self {
            config_file,
            config: SyncMutex::new(Arc::new(config)),
            ignores: Ignores::load(config::data_path("chat-ignores.json")),
            mutes: Mutes::load(config::data_path("chat-mutes.json")),
            global_mode: SyncMutex::new(FxHashSet::default()),
            last_partner: SyncMutex::new(FxHashMap::default()),
        });
        commands::register(registry, &chat)
            .inspect_err(|error| log::error!("Chat is off: {error}"))
            .ok()?;
        Some(chat)
    }

    /// Takes over the chat of `server`.
    pub fn start(self: &Arc<Self>, server: &Arc<Server>) {
        let interceptor: Arc<dyn ChatInterceptor> = Arc::<Self>::clone(self);
        if server.set_chat_interceptor(interceptor) {
            log::info!("MineHouse chat on: local, global, private messages, mutes");
        } else {
            log::error!("Another chat interceptor is already installed: vanilla chat stays");
        }
    }

    fn config(&self) -> Arc<ChatConfig> {
        Arc::clone(&self.config.lock())
    }

    /// Re-reads `config/minehouse/chat.toml`.
    fn reload(&self) -> Result<(), String> {
        let config = ChatConfig::load(&self.config_file)?;
        *self.config.lock() = Arc::new(config);
        Ok(())
    }

    /// If the player is muted, tells them so and returns `true`: the message must not go out.
    fn blocked_by_mute(&self, player: &Player) -> bool {
        let now = now_millis();
        let Some(mute) = self.mutes.get(player.gameprofile.id(), now) else {
            return false;
        };
        let key = if mute.until.is_some() {
            "mute.blocked_temp"
        } else {
            "mute.blocked"
        };
        player.send_message(&mute_text(&self.config(), key, &mute, now));
        true
    }

    // --- Channels ----------------------------------------------------------------------

    fn send(&self, server: &Server, sender: &Player, text: &str, global: bool) {
        if self.blocked_by_mute(sender) {
            return;
        }
        let config = self.config();
        let radius = config.number("local.radius", 100);
        let sender_id = sender.gameprofile.id();
        let line = channel_line(server, sender, text, global, &config);
        let mention_sound = config.text("mention.sound");

        let mut heard_by_others = false;
        for player in server.get_players() {
            let id = player.gameprofile.id();
            if id == sender_id {
                player.send_message(&line);
                continue;
            }
            if self.ignores.ignores(id, sender_id) || !(global || hears(sender, &player, radius)) {
                continue;
            }
            heard_by_others = true;
            player.send_message(&line);
            if mentions(text, player.gameprofile.name()) {
                play_sound(&player, mention_sound);
            }
        }
        log::info!(
            "[{}] {}: {text}",
            if global { "G" } else { "L" },
            sender.gameprofile.name()
        );

        let nobody = config.text("local.nobody");
        if !global && !heard_by_others && !nobody.is_empty() {
            sender.send_message(&render(nobody, &[("radius", &radius.to_string())]));
        }
    }

    /// An admin announcement (`$$text`): every player online gets it, ignores don't apply,
    /// it is framed with empty lines, plays a sound and can show a title.
    fn announce(&self, server: &Server, sender: &Player, text: &str) {
        if self.blocked_by_mute(sender) {
            return;
        }
        let config = self.config();
        let mut parts = player_parts(server, sender, &config);
        parts.push(("message", Some(TextComponent::plain(text.to_owned()))));
        let line = render_parts(config.text("announce.format"), &parts, &[]);
        let title_format = config.text("announce.title");
        let subtitle_format = config.text("announce.subtitle");
        let empty = TextComponent::plain("");

        for player in server.get_players() {
            player.send_message(&empty);
            player.send_message(&line);
            player.send_message(&empty);
            play_sound(&player, config.text("announce.sound"));
            if !title_format.is_empty() || !subtitle_format.is_empty() {
                player.send_title_times(10, 60, 20);
                player.send_subtitle(render_parts(subtitle_format, &parts, &[]));
                player.send_title(render_parts(title_format, &parts, &[]));
            }
        }
        log::info!("[ANNOUNCE] {}: {text}", sender.gameprofile.name());
    }

    fn set_global_mode(&self, player: &Player, global: bool) {
        let id = player.gameprofile.id();
        if global {
            self.global_mode.lock().insert(id);
        } else {
            self.global_mode.lock().remove(&id);
        }
        let key = if global { "mode.global" } else { "mode.local" };
        player.send_message(&render(self.config().text(key), &[]));
    }

    // --- Private messages --------------------------------------------------------------

    fn private_message(&self, server: &Server, sender: &Player, target: &Player, text: &str) {
        let config = self.config();
        let (sender_id, target_id) = (sender.gameprofile.id(), target.gameprofile.id());
        let target_name = target.gameprofile.name();
        let names = [("target", target_name)];
        if sender_id == target_id {
            sender.send_message(&render(config.text("pm.self"), &names));
            return;
        }
        if self.blocked_by_mute(sender) {
            return;
        }
        if self.ignores.ignores(target_id, sender_id) {
            sender.send_message(&render(config.text("pm.ignored"), &names));
            return;
        }

        let mut parts = player_parts(server, sender, &config);
        parts.push(("message", Some(TextComponent::plain(text.to_owned()))));
        parts.push(("target", Some(player_name(target_name, target_id, &config))));
        sender.send_message(&render_parts(config.text("pm.to"), &parts, &[]));
        target.send_message(&render_parts(config.text("pm.from"), &parts, &[]));
        play_sound(target, config.text("pm.sound"));

        {
            let mut last_partner = self.last_partner.lock();
            last_partner.insert(sender_id, target_id);
            last_partner.insert(target_id, sender_id);
        }
        log::info!(
            "[PM] {} -> {target_name}: {text}",
            sender.gameprofile.name()
        );
    }

    fn reply(&self, server: &Server, sender: &Player, text: &str) {
        let partner = self
            .last_partner
            .lock()
            .get(&sender.gameprofile.id())
            .copied();
        let config = self.config();
        let Some(partner) = partner else {
            sender.send_message(&render(config.text("pm.nobody_to_reply"), &[]));
            return;
        };
        let target = server
            .get_players()
            .into_iter()
            .find(|player| player.gameprofile.id() == partner);
        match target {
            Some(target) => self.private_message(server, sender, &target, text),
            None => sender.send_message(&render(config.text("pm.not_found"), &[("target", "?")])),
        }
    }

    // --- Ignore ------------------------------------------------------------------------

    fn toggle_ignore(&self, player: &Player, other: &Player) {
        let config = self.config();
        let names = [("target", other.gameprofile.name())];
        let (id, other_id) = (player.gameprofile.id(), other.gameprofile.id());
        if id == other_id {
            player.send_message(&render(config.text("pm.self"), &names));
            return;
        }
        let added = self.ignores.toggle(id, other_id, other.gameprofile.name());
        let key = if added {
            "ignore.added"
        } else {
            "ignore.removed"
        };
        player.send_message(&render(config.text(key), &names));
    }

    fn list_ignores(&self, player: &Player) {
        let config = self.config();
        let names = self.ignores.names(player.gameprofile.id());
        if names.is_empty() {
            player.send_message(&render(config.text("ignore.empty"), &[]));
        } else {
            let list = names.join(", ");
            player.send_message(&render(config.text("ignore.list"), &[("list", &list)]));
        }
    }
}

impl ChatInterceptor for Chat {
    fn player_chat(&self, server: &Arc<Server>, player: &Player, message: &str) -> bool {
        let config = self.config();
        let announce_prefix = config.text("announce.prefix");
        if !announce_prefix.is_empty()
            && let Some(text) = message.strip_prefix(announce_prefix)
            && may_announce(server, player)
        {
            let text = text.trim();
            if !text.is_empty() {
                self.announce(server, player, text);
            }
            return true;
        }

        let global_prefix = config.text("global.prefix");
        let (text, global) = match message.strip_prefix(global_prefix) {
            Some(text) if !global_prefix.is_empty() => (text.trim(), true),
            _ => (
                message.trim(),
                self.global_mode.lock().contains(&player.gameprofile.id()),
            ),
        };
        if !text.is_empty() {
            self.send(server, player, text, global);
        }
        true
    }

    fn allow_command_message(&self, player: &Player) -> bool {
        !self.blocked_by_mute(player)
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

fn may_announce(server: &Server, player: &Player) -> bool {
    PermissionKey::parse(ANNOUNCE_PERMISSION)
        .inspect_err(|error| log::error!("Bad permission {ANNOUNCE_PERMISSION}: {error}"))
        .is_ok_and(|key| {
            server
                .player_effective_permissions(player.gameprofile.id())
                .allows(&PermissionExpr::key(key))
        })
}

/// A config text about a mute: `{player}` `{reason}` `{by}` `{until}` `{left}`.
fn mute_text(config: &ChatConfig, key: &str, mute: &Mute, now: i64) -> TextComponent {
    let until = mute.until.map_or_else(
        || FOREVER.to_owned(),
        |until| format!("{} МСК", format_epoch_millis(until)),
    );
    let left = mute
        .seconds_left(now)
        .map_or_else(|| FOREVER.to_owned(), format_duration);
    render(
        config.text(key),
        &[
            ("player", &mute.name),
            ("reason", &mute.reason),
            ("by", &mute.by),
            ("until", &until),
            ("left", &left),
        ],
    )
}

/// One line of local or global chat as everyone sees it.
fn channel_line(
    server: &Server,
    sender: &Player,
    text: &str,
    global: bool,
    config: &ChatConfig,
) -> TextComponent {
    let channel = if global { "global" } else { "local" };
    let radius = config.number("local.radius", 100).to_string();
    let values = [("radius", radius.as_str())];
    let mut parts = player_parts(server, sender, config);
    parts.push(("message", Some(TextComponent::plain(text.to_owned()))));
    let line = render_parts(config.text(&format!("{channel}.format")), &parts, &values);
    let hover = config.text(&format!("{channel}.hover"));
    if hover.is_empty() {
        line
    } else {
        line.hover_event(HoverEvent::show_text(render(hover, &values)))
    }
}

/// Placeholders that describe the sender: `{name}`, `{prefix}` (role badge), `{group}` and
/// `{world}`.
fn player_parts(
    server: &Server,
    player: &Player,
    config: &ChatConfig,
) -> Vec<(&'static str, Option<TextComponent>)> {
    let groups = server
        .player_permission_state(player.gameprofile.id())
        .map_or_default(|state| state.groups().to_vec());
    let role = config
        .list("role.order")
        .find(|role| groups.iter().any(|group| group == role));
    let group = groups.first().map_or("default", String::as_str);
    let world = player.get_world();
    vec![
        (
            "name",
            Some(player_name(
                player.gameprofile.name(),
                player.gameprofile.id(),
                config,
            )),
        ),
        ("prefix", role.and_then(|role| role_badge(role, config))),
        ("group", Some(TextComponent::plain(group.to_owned()))),
        (
            "world",
            Some(TextComponent::plain(world_name(&world.key, config))),
        ),
    ]
}

/// The name of a world from the `world.<name>` keys; a world of another domain or
/// namespace is `world.<namespace>.<name>`.
fn world_name(key: &Identifier, config: &ChatConfig) -> String {
    let short = config.text(&format!("world.{}", key.path));
    let name = if short.is_empty() {
        config.text(&format!("world.{}.{}", key.namespace, key.path))
    } else {
        short
    };
    if name.is_empty() {
        key.path.to_string()
    } else {
        name.to_owned()
    }
}

/// The nickname: hovering shows a hint, a click starts `/msg`. In the player's `MineHouse`
/// Nitro colour if they have it.
fn player_name(name: &str, id: Uuid, config: &ChatConfig) -> TextComponent {
    let mut component = TextComponent::plain(name.to_owned())
        .click_event(ClickEvent::suggest_command(format!("/msg {name} ")));
    if let Some([red, green, blue]) = nitro::color(id) {
        component = component.color(Color::Rgb(red, green, blue));
    }
    let hover = config.text("name.hover");
    if hover.is_empty() {
        component
    } else {
        component.hover_event(HoverEvent::show_text(render(hover, &[])))
    }
}

/// The role badge plus a trailing space.
fn role_badge(role: &str, config: &ChatConfig) -> Option<TextComponent> {
    let badge = config.text(&format!("role.{role}.badge"));
    if badge.is_empty() {
        return None;
    }
    let mut component = render(badge, &[]);
    let hover = config.text(&format!("role.{role}.hover"));
    if !hover.is_empty() {
        component = component.hover_event(HoverEvent::show_text(render(hover, &[])));
    }
    Some(TextComponent::plain("").add_children(vec![component, TextComponent::plain(" ")]))
}

/// Local chat: the same world and, with a radius, close enough.
fn hears(sender: &Player, player: &Player, radius: u32) -> bool {
    if sender.get_world().key != player.get_world().key {
        return false;
    }
    let radius = f64::from(radius);
    radius <= 0.0 || sender.position().distance_squared(player.position()) <= radius * radius
}

/// Whether `text` has the nickname as a whole word, in any letter case.
fn mentions(text: &str, name: &str) -> bool {
    let is_word = |character: char| character.is_alphanumeric() || character == '_';
    let text = text.to_lowercase();
    let name = name.to_lowercase();
    if name.is_empty() {
        return false;
    }
    text.match_indices(&name).any(|(start, found)| {
        let before = text[..start].chars().next_back();
        let after = text[start + found.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// Plays the sound `key` (`block.bell.use`) to one player; an empty key is no sound.
fn play_sound(player: &Player, key: &str) {
    if key.is_empty() {
        return;
    }
    let Ok(sound_id) = key.parse::<Identifier>() else {
        return;
    };
    player.send_packet(CSound::new_holder(
        SoundEventHolder::Direct {
            sound_id,
            fixed_range: None,
        },
        SoundSource::Master,
        player.position(),
        1.0,
        1.0,
        rand::random(),
    ));
}

#[cfg(test)]
mod tests {
    use text_components::resolving::NoResolutor;

    use super::*;

    fn defaults() -> ChatConfig {
        ChatConfig::defaults().unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn mentions_need_the_whole_nickname() {
        assert!(mentions("привет, Steve!", "steve"));
        assert!(mentions("STEVE", "Steve"));
        assert!(mentions("steve_2 и steve", "Steve"));
        assert!(!mentions("steve_2", "Steve"));
        assert!(!mentions("mrsteve", "Steve"));
        assert!(!mentions("стивsteveстив", "Steve"));
        assert!(!mentions("привет", ""));
    }

    #[test]
    fn worlds_and_badges_come_from_the_config() {
        let config = defaults();
        let nether = Identifier::new("minecraft", "the_nether");
        assert_eq!(world_name(&nether, &config), "Незер");
        let paradise = Identifier::new("minehouse", "paradise");
        assert_eq!(world_name(&paradise, &config), "paradise");

        let badge = role_badge("admin", &config).map(|badge| badge.to_plain(&NoResolutor));
        assert_eq!(badge.as_deref(), Some("● "));
        assert!(role_badge("builder", &config).is_none());
    }

    #[test]
    fn mute_texts_say_how_long() {
        let config = defaults();
        let mute = Mute {
            name: "Alice".to_owned(),
            reason: "спам".to_owned(),
            by: "Server".to_owned(),
            since: 0,
            until: Some(1_794_063_600_000),
        };
        let now = 1_794_063_600_000 - 2 * 3_600_000;
        assert_eq!(
            mute_text(&config, "mute.blocked_temp", &mute, now).to_plain(&NoResolutor),
            "У тебя мут ещё 2 часа (до 07.11.2026 18:00 МСК), сообщение не отправлено. \
             Причина: спам"
        );
        let forever = Mute {
            until: None,
            ..mute
        };
        assert_eq!(
            mute_text(&config, "mute.announce", &forever, now).to_plain(&NoResolutor),
            "Alice больше не может писать в чат. Причина: спам"
        );
    }
}
