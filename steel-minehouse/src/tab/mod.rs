//! The tab list: header, footer, names and order. On the Fabric server this was the mod
//! Styled Player List with our style; here it is `config/minehouse/tab.toml`.
//!
//! Once a second every player gets the header and footer with their own ping. Names and
//! the order go out when someone joins or leaves, or when a style of a player changes
//! (a new permission group).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use steel_core::{
    player::{Player, connection::NetworkConnection as _},
    server::{Server, tab_list::TabListProvider},
};
use steel_protocol::packets::game::{
    CPlayerInfoUpdate, CTabList, PlayerInfoAction, PlayerInfoEntry,
};
use steel_utils::locks::SyncMutex;
use text_components::{Modifier, TextComponent, format::Color};
use uuid::Uuid;

use crate::{config, moderation::format::render_parts, nitro};

/// `config/minehouse/tab.toml`. Texts take `&` colors and `&#rrggbb`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TabConfig {
    /// `false` keeps the built-in tab list of Steel.
    pub enabled: bool,
    /// `{title}` of the header: bold, colored from the first to the second `#rrggbb` of
    /// `title_gradient`.
    pub title: String,
    /// Two `#rrggbb` colors.
    pub title_gradient: Vec<String>,
    /// Lines above the players. `{title}`, `{online}`, `{max}`, `{ping}`, `{tps}`.
    pub header: Vec<String>,
    /// Lines below the players, same placeholders.
    pub footer: Vec<String>,
    /// How a nickname looks: `{name}`.
    pub name: String,
    /// The first style whose conditions a player meets replaces `name` and sets the order.
    pub styles: Vec<TabStyle>,
}

/// A look and a place in the list for the players of a permission group.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TabStyle {
    /// The player is in this group of `config/groups.toml`.
    pub group: String,
    /// And in none of these.
    pub not_groups: Vec<String>,
    /// And is none of these nicknames.
    pub except: Vec<String>,
    /// How the nickname looks: `{name}`.
    pub name: String,
    /// Higher is higher in the list; everyone else has 0.
    pub order: i32,
}

impl Default for TabConfig {
    fn default() -> Self {
        let lines = |lines: &[&str]| lines.iter().map(|&line| line.to_owned()).collect();
        Self {
            enabled: true,
            title: "  ⌂ MINEHOUSE  ".to_owned(),
            title_gradient: lines(&["#d8b4fe", "#a855f7"]),
            header: lines(&["", "{title}", "&#8b7aa8vanilla+ сервер", ""]),
            footer: lines(&[
                "",
                "&#8b7aa8Онлайн &f{online}&8/&#8b7aa8{max}",
                "    &#8b7aa8Пинг &f{ping}&#8b7aa8 мс   &8·&#8b7aa8   TPS &f{tps}    ",
                "",
                "&#a855f7minehouse.pro",
                "",
            ]),
            name: "&f{name}".to_owned(),
            styles: vec![
                TabStyle {
                    group: "bot".to_owned(),
                    not_groups: vec!["admin".to_owned()],
                    except: Vec::new(),
                    name: "&#7dd3fc🤖 &f{name}".to_owned(),
                    order: -10,
                },
                TabStyle {
                    group: "admin".to_owned(),
                    not_groups: Vec::new(),
                    except: vec!["HouseClaw".to_owned()],
                    name: "&#ff5f5f● &f{name}".to_owned(),
                    order: 100,
                },
            ],
        }
    }
}

impl TabConfig {
    /// The style of a player: its index, or `None` for the plain look.
    fn style_of(&self, name: &str, groups: &[String]) -> Option<usize> {
        let in_group = |group: &String| groups.contains(group);
        self.styles.iter().position(|style| {
            in_group(&style.group)
                && !style.not_groups.iter().any(in_group)
                && !style
                    .except
                    .iter()
                    .any(|except| except.eq_ignore_ascii_case(name))
        })
    }

    /// The nickname in the list and its order. `color` is the `MineHouse` Nitro colour.
    fn entry(
        &self,
        name: &str,
        style: Option<usize>,
        color: Option<[u8; 3]>,
    ) -> (TextComponent, i32) {
        let style = style.and_then(|index| self.styles.get(index));
        let format = style.map_or(self.name.as_str(), |style| style.name.as_str());
        let order = style.map_or(0, |style| style.order);
        let name = TextComponent::plain(name.to_owned());
        let name = Some(match color {
            Some([red, green, blue]) => name.color(Color::Rgb(red, green, blue)),
            None => name,
        });
        (render_parts(format, &[("name", name)], &[]), order)
    }

    fn title(&self) -> TextComponent {
        let colors = match self.title_gradient.as_slice() {
            [from, to] => parse_hex(from).zip(parse_hex(to)),
            _ => None,
        };
        match colors {
            Some((from, to)) => gradient(&self.title, from, to),
            None => TextComponent::plain(self.title.clone()).bold(true),
        }
    }

    fn lines(&self, lines: &[String], values: &[(&str, &str)]) -> TextComponent {
        let parts = [("title", Some(self.title()))];
        render_parts(&lines.join("\n"), &parts, values)
    }
}

fn parse_hex(color: &str) -> Option<[u8; 3]> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let [_, red, green, blue] = u32::from_str_radix(hex, 16).ok()?.to_be_bytes();
    Some([red, green, blue])
}

/// Bold `text`, each character a step from `from` to `to`.
fn gradient(text: &str, from: [u8; 3], to: [u8; 3]) -> TextComponent {
    let steps = text.chars().count().saturating_sub(1).max(1);
    let mix = |from: u8, to: u8, step: usize| {
        let (from, to) = (usize::from(from), usize::from(to));
        let value = (from * (steps - step) + to * step) / steps;
        u8::try_from(value).unwrap_or(u8::MAX)
    };
    let children = text
        .chars()
        .enumerate()
        .map(|(step, character)| {
            let [red, green, blue] = [0, 1, 2].map(|channel| mix(from[channel], to[channel], step));
            TextComponent::plain(character.to_string())
                .color(Color::Rgb(red, green, blue))
                .bold(true)
        })
        .collect();
    TextComponent::plain("").add_children(children)
}

/// A player, their style and their Nitro colour.
type Look = (Uuid, Option<usize>, Option<[u8; 3]>);

/// The tab list of one server.
pub struct Tab {
    config: TabConfig,
    /// Who was in the list with which style and Nitro colour when names last went out.
    sent: SyncMutex<Vec<Look>>,
}

impl Tab {
    /// Loads the settings and takes over the tab list of `server`.
    pub fn start(server: &Arc<Server>) {
        let config: TabConfig = match config::load(&config::path("tab")) {
            Ok(config) => config,
            Err(error) => {
                log::error!("MineHouse tab list is off: {error}");
                return;
            }
        };
        if !config.enabled {
            log::info!("MineHouse tab list is off in config/minehouse/tab.toml");
            return;
        }
        let tab = Arc::new(Self {
            config,
            sent: SyncMutex::new(Vec::new()),
        });
        if server.set_tab_list_provider(tab) {
            log::info!("MineHouse tab list on");
        } else {
            log::error!("Another tab list provider is already installed");
        }
    }

    fn send_header_footer(&self, server: &Server, players: &[Arc<Player>]) {
        let online = players.len().to_string();
        let max = server.max_players().to_string();
        let tps = format!("{:.1}", server.tick_rate_manager.read().get_tps());
        for player in players {
            let ping = player.connection.latency().to_string();
            let values = [
                ("online", online.as_str()),
                ("max", max.as_str()),
                ("tps", tps.as_str()),
                ("ping", ping.as_str()),
            ];
            let header = self.config.lines(&self.config.header, &values);
            let footer = self.config.lines(&self.config.footer, &values);
            player.send_packet(CTabList::new(&header, &footer, &**player));
        }
    }

    fn send_names(&self, server: &Server, players: &[Arc<Player>]) {
        let mut styles: Vec<Look> = players
            .iter()
            .map(|player| {
                let id = player.gameprofile.id();
                let groups = server
                    .player_permission_state(id)
                    .map_or_default(|state| state.groups().to_vec());
                (
                    id,
                    self.config.style_of(player.gameprofile.name(), &groups),
                    nitro::color(id),
                )
            })
            .collect();
        styles.sort_unstable();
        {
            let mut sent = self.sent.lock();
            if *sent == styles {
                return;
            }
            sent.clone_from(&styles);
        }

        let entries: Vec<PlayerInfoEntry> = players
            .iter()
            .map(|player| {
                let id = player.gameprofile.id();
                let (style, color) = styles
                    .iter()
                    .find(|(styled, _, _)| *styled == id)
                    .map_or((None, None), |(_, style, color)| (*style, *color));
                let (name, order) = self.config.entry(player.gameprofile.name(), style, color);
                PlayerInfoEntry::new(id)
                    .with_display_name(Some(name))
                    .with_list_order(order)
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        server.broadcast_to_online(CPlayerInfoUpdate {
            actions: PlayerInfoAction::UpdateDisplayName as u8
                | PlayerInfoAction::UpdateListOrder as u8,
            entries,
        });
    }
}

impl TabListProvider for Tab {
    fn refresh(&self, server: &Server) {
        let players = server.get_players();
        self.send_header_footer(server, &players);
        self.send_names(server, &players);
    }
}

#[cfg(test)]
mod tests {
    use text_components::resolving::NoResolutor;

    use super::*;

    fn groups(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    #[test]
    fn styles_follow_groups_and_exceptions() {
        let config = TabConfig::default();
        assert_eq!(config.style_of("Steve", &groups(&["default"])), None);
        assert_eq!(config.style_of("Steve", &groups(&["admin"])), Some(1));
        assert_eq!(config.style_of("houseclaw", &groups(&["admin"])), None);
        assert_eq!(config.style_of("Bot", &groups(&["bot"])), Some(0));
        assert_eq!(config.style_of("Bot", &groups(&["bot", "admin"])), Some(1));

        let (name, order) = config.entry("Steve", Some(1), None);
        assert_eq!(name.to_plain(&NoResolutor), "● Steve");
        assert_eq!(order, 100);
        let (name, order) = config.entry("Alex", None, None);
        assert_eq!(name.to_plain(&NoResolutor), "Alex");
        assert_eq!(order, 0);
        // Nitro: the same text, the nickname in its colour.
        let (name, _) = config.entry("Alex", None, Some([0xa8, 0x55, 0xf7]));
        assert_eq!(name.to_plain(&NoResolutor), "Alex");
        let nick = &name.children[0].children[0];
        assert_eq!(nick.to_plain(&NoResolutor), "Alex");
        assert_eq!(
            format!("{:?}", nick.format.color),
            "Some(Rgb(168, 85, 247))"
        );
    }

    #[test]
    fn header_and_footer_fill_in() {
        let config = TabConfig::default();
        let values = [
            ("online", "7"),
            ("max", "100"),
            ("ping", "42"),
            ("tps", "20.0"),
        ];
        assert_eq!(
            config.lines(&config.header, &values).to_plain(&NoResolutor),
            "\n  ⌂ MINEHOUSE  \nvanilla+ сервер\n"
        );
        assert_eq!(
            config.lines(&config.footer, &values).to_plain(&NoResolutor),
            "\nОнлайн 7/100\n    Пинг 42 мс   ·   TPS 20.0    \n\nminehouse.pro\n"
        );
    }

    #[test]
    fn gradients_run_from_one_color_to_the_other() {
        assert_eq!(parse_hex("#d8b4fe"), Some([0xd8, 0xb4, 0xfe]));
        assert_eq!(parse_hex("d8b4fe"), None);
        assert_eq!(parse_hex("#zzzzzz"), None);
        let text = gradient("abc", [0, 0, 0], [200, 100, 50]);
        assert_eq!(text.to_plain(&NoResolutor), "abc");
        let colors: Vec<String> = text
            .children
            .iter()
            .map(|child| format!("{:?}", child.format.color))
            .collect();
        assert_eq!(
            colors,
            [
                "Some(Rgb(0, 0, 0))",
                "Some(Rgb(100, 50, 25))",
                "Some(Rgb(200, 100, 50))"
            ]
        );
    }
}
