//! Our creative tabs, the way the Fabric server shows them: through Polymer on the client.
//!
//! The pack has Polymer, and Polymer on the client draws the creative tabs a Polymer server
//! defines (`MineHouse`, «Банк», «Рай» with the magic herb). Steel has no Polymer, so it
//! speaks the part of its protocol the tabs need, on plugin channels in the play state:
//!
//! 1. On join the server says `polymer:hello`; a client with Polymer answers with its
//!    `polymer:handshake`, a vanilla client ignores it and gets nothing more.
//! 2. The server answers with its own handshake and `polymer:metadata`, whose
//!    `polymer:minecraft_protocol` turns the client's Polymer on.
//! 3. The client sends its metadata back; by then it has ours, and the tabs go out: define,
//!    contents, and the update that rebuilds the creative screen.
//!
//! Every clientbound payload starts with its version as a `VarInt`, as Polymer's versioned
//! codecs write it. The formats are Polymer 0.18.6+26.3 (`S2CPackets`, `HandshakePayload`,
//! `MetadataPayload`, `PolymerCreativeTab*S2CPayload`).

use std::{collections::BTreeMap, sync::Arc};

use steel_core::{
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
        plugin_channels::PluginChannelHandler,
    },
};
use steel_protocol::packets::common::CPluginMessage;
use steel_registry::{item_stack::ItemStack, packets::CURRENT_MC_PROTOCOL};
use steel_utils::{Identifier, codec::VarInt, serial::WriteTo as _};
use text_components::TextComponent;
use uuid::Uuid;

use super::{ATM, BROTH, CLOUD, CustomItem, JUNCTION, LEAF, LEAF_BLOCK, MAGNIFIER, PAPIROSA};
use super::{SEEDS, STATION, TERMINAL};

/// A creative tab of ours.
pub struct Tab {
    /// The id the Fabric mod gave it.
    pub id: &'static str,
    /// The title.
    pub name: &'static str,
    /// The item on the tab.
    pub icon: &'static CustomItem,
    /// What is in it, in order.
    pub items: &'static [&'static CustomItem],
}

/// The tabs, as the Fabric mods have them, with the items Steel has.
pub static TABS: &[Tab] = &[
    Tab {
        id: "minehouse-tweaks:items",
        name: "MineHouse",
        icon: &STATION,
        items: &[&STATION, &JUNCTION, &MAGNIFIER],
    },
    Tab {
        id: "minehouse-bank:items",
        name: "Банк",
        icon: &ATM,
        items: &[&ATM, &TERMINAL],
    },
    Tab {
        id: "minehouse-paradise:items",
        name: "Рай",
        icon: &CLOUD,
        // The magic herb is part of Paradise since Paradise 0.32.0: no tab of its own.
        items: &[&CLOUD, &SEEDS, &LEAF, &LEAF_BLOCK, &BROTH, &PAPIROSA],
    },
];

const HELLO: &str = "polymer:hello";
const HANDSHAKE: &str = "polymer:handshake";
const METADATA: &str = "polymer:metadata";
const TAB_DEFINE: &str = "polymer:sync/creative_tab/define";
const TAB_CLEAR: &str = "polymer:sync/creative_tab/contents/clear";
const TAB_ADD: &str = "polymer:sync/creative_tab/contents/add";
const TAB_APPLY: &str = "polymer:sync/creative_tab/apply_update";

/// The version of Polymer's own packets (hello, handshake, metadata).
const NET_VERSION: i32 = 2;
/// The version of the creative tab packets.
const TAB_VERSION: i32 = 15;
/// The Polymer version we say we are; the client only needs it to be there.
const POLYMER_VERSION: &str = "0.18.6+26.3";
/// The metadata key that tells the client's Polymer what game it is talking to.
const PROTOCOL_KEY: &str = "polymer:minecraft_protocol";
/// `Mode.INSERT_END` of a contents entry.
const INSERT_END: i32 = 2;
/// The NBT id of an int tag.
const NBT_INT: u8 = 3;

/// A payload under construction.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn versioned(version: i32) -> Self {
        let mut out = Self::default();
        out.var_int(version);
        out
    }

    fn var_int(&mut self, value: i32) {
        // Writing into a `Vec` does not fail.
        let _ = VarInt(value).write(&mut self.0);
    }

    fn string(&mut self, value: &str) {
        self.var_int(i32::try_from(value.len()).unwrap_or(i32::MAX));
        self.0.extend_from_slice(value.as_bytes());
    }

    fn count(&mut self, value: usize) {
        self.var_int(i32::try_from(value).unwrap_or(i32::MAX));
    }

    fn stack(&mut self, stack: &ItemStack) {
        let _ = stack.write(&mut self.0);
    }

    fn component(&mut self, text: &TextComponent) {
        let _ = text.write(&mut self.0);
    }
}

/// `polymer:hello`: nothing but the version.
fn hello() -> Vec<u8> {
    Out::versioned(NET_VERSION).0
}

/// Our handshake: the Polymer version and the serverbound packets we take, with versions.
fn handshake() -> Vec<u8> {
    let mut out = Out::versioned(NET_VERSION);
    out.string(POLYMER_VERSION);
    let accepted = [HANDSHAKE, METADATA];
    out.count(accepted.len());
    for id in accepted {
        out.string(id);
        out.count(1);
        out.var_int(NET_VERSION);
    }
    out.0
}

/// Our metadata: the game protocol, without which the client's Polymer stays off.
fn metadata() -> Vec<u8> {
    let mut out = Out::versioned(NET_VERSION);
    out.count(1);
    out.string(PROTOCOL_KEY);
    out.0.push(NBT_INT);
    out.0.extend_from_slice(&CURRENT_MC_PROTOCOL.to_be_bytes());
    out.0
}

fn tab_define(tab: &Tab) -> Vec<u8> {
    let mut out = Out::versioned(TAB_VERSION);
    out.string(tab.id);
    out.component(&TextComponent::plain(tab.name));
    out.stack(&tab.icon.stack(1));
    out.0
}

fn tab_clear(tab: &Tab) -> Vec<u8> {
    let mut out = Out::versioned(TAB_VERSION);
    out.string(tab.id);
    out.0
}

/// The tab's items, once in the tab and once in the creative search.
fn tab_add(tab: &Tab) -> Vec<u8> {
    let mut out = Out::versioned(TAB_VERSION);
    out.string(tab.id);
    for _ in ["main", "search"] {
        out.count(1);
        out.var_int(INSERT_END);
        out.count(tab.items.len());
        for item in tab.items {
            out.stack(&item.stack(1));
        }
    }
    out.0
}

fn tab_apply() -> Vec<u8> {
    Out::versioned(TAB_VERSION).0
}

fn send(player: &Player, channel: &'static str, data: Vec<u8>) {
    let (namespace, path) = channel.split_once(':').unwrap_or(("polymer", channel));
    player.send_packet(CPluginMessage::new(
        Identifier::new_static(namespace, path),
        data.into(),
    ));
}

/// Answers Polymer clients.
struct PolymerTabs;

impl PluginChannelHandler for PolymerTabs {
    fn plugin_message(
        &self,
        _server: &Arc<Server>,
        player: &Player,
        channel: &Identifier,
        _data: &[u8],
    ) {
        let channel = channel.to_string();
        if channel == HANDSHAKE {
            send(player, HANDSHAKE, handshake());
            send(player, METADATA, metadata());
        } else if channel == METADATA {
            for tab in TABS {
                send(player, TAB_DEFINE, tab_define(tab));
                send(player, TAB_CLEAR, tab_clear(tab));
                send(player, TAB_ADD, tab_add(tab));
            }
            send(player, TAB_APPLY, tab_apply());
        }
    }
}

/// Says hello to every player who comes in, once per connection.
#[derive(Default)]
struct Greeter {
    greeted: BTreeMap<Uuid, usize>,
}

impl ServerJob for Greeter {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        let mut now = BTreeMap::new();
        for player in server.get_players() {
            let id = player.gameprofile.id();
            // A rejoin is another `Player`: it gets its own hello.
            let connection = Arc::as_ptr(&player) as usize;
            if self.greeted.get(&id) != Some(&connection) {
                send(&player, HELLO, hello());
            }
            now.insert(id, connection);
        }
        self.greeted = now;
        JobPoll::Pending
    }
}

/// Starts greeting players and answering their Polymer.
pub(super) fn start(server: &Server) {
    server.add_plugin_channel_handler(Arc::new(PolymerTabs));
    server.jobs.spawn(Greeter::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{CARD, ITEMS};

    #[test]
    fn every_item_but_the_card_is_in_a_tab() {
        for item in ITEMS {
            let in_tabs = TABS
                .iter()
                .filter(|tab| tab.items.iter().any(|it| it.id == item.id))
                .count();
            let expected = usize::from(item.id != CARD.id);
            assert_eq!(in_tabs, expected, "{} is in {in_tabs} tabs", item.id);
        }
    }

    #[test]
    fn hello_is_its_version_alone() {
        assert_eq!(hello(), vec![2]);
    }

    #[test]
    fn metadata_carries_the_protocol_as_an_int_tag() {
        let data = metadata();
        let mut expected = vec![2, 1, 26];
        expected.extend_from_slice(PROTOCOL_KEY.as_bytes());
        expected.push(3);
        expected.extend_from_slice(&CURRENT_MC_PROTOCOL.to_be_bytes());
        assert_eq!(data, expected);
    }

    #[test]
    fn handshake_lists_what_we_take() {
        let data = handshake();
        let mut expected = vec![2, 11];
        expected.extend_from_slice(b"0.18.6+26.3");
        expected.push(2);
        for id in [HANDSHAKE, METADATA] {
            expected.push(u8::try_from(id.len()).unwrap_or_default());
            expected.extend_from_slice(id.as_bytes());
            expected.extend_from_slice(&[1, 2]);
        }
        assert_eq!(data, expected);
    }

    #[test]
    fn channels_are_identifiers() {
        for channel in [
            HELLO, HANDSHAKE, METADATA, TAB_DEFINE, TAB_CLEAR, TAB_ADD, TAB_APPLY,
        ] {
            assert_eq!(
                channel.parse::<Identifier>().map(|id| id.to_string()).ok(),
                Some(channel.to_owned())
            );
        }
        for tab in TABS {
            assert!(tab.id.parse::<Identifier>().is_ok(), "{}", tab.id);
        }
    }
}
