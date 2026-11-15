//! Animated guides (a port of the Fabric mod `MineHouse` Guides): a small scene of display
//! entities in front of a player that shows a thing of ours step by step, with a caption and
//! buttons. A player opens a guide with `/guide <id>`, from the list `/guide` prints, or with
//! sneak + right click in the air holding one of its items.
//!
//! A module that has something to explain builds its guides (see `bank/guides.rs`) and adds
//! them to [`all`].

pub mod book;
mod session;
pub mod stage;

use std::sync::{Arc, LazyLock};

use rustc_hash::{FxHashMap, FxHashSet};
use steel_core::{
    behavior::{InteractionResult, ItemBehavior, UseItemContext},
    command::{
        CommandContext, CommandError, CommandNode, CommandRegistration, CommandRegistrationError,
        CommandRegistry, literal,
    },
    player::Player,
    server::{
        Server,
        jobs::{JobPoll, ServerJob, ServerJobContext},
    },
};
use steel_registry::item_stack::ItemStack;
use steel_utils::{Identifier, locks::SyncMutex, types::InteractionHand};
use text_components::{
    Modifier as _, TextComponent,
    format::Color,
    interactivity::{ClickEvent, HoverEvent},
};
use uuid::Uuid;

use self::{session::Session, stage::Stage};
use crate::{
    bank,
    content::{CustomItem, id_of},
};

/// One step of a guide: a caption and a script that builds or animates the scene. The
/// script says what comes later with [`Stage::later`]; the step lasts at least `ticks`, and
/// until everything it put off has happened.
///
/// A step may only build on what the steps before it left: to show step 3 the first two are
/// replayed instantly.
pub struct Step {
    /// The words over the scene.
    pub caption: &'static str,
    /// How long the step is shown at least, ticks.
    pub ticks: u32,
    /// What happens in the scene.
    pub script: fn(&mut Stage),
}

/// One guide: a small scene of `width` by `depth` cells played step by step.
pub struct Guide {
    /// What `/guide` takes: latin letters, digits, `_` and `-`.
    pub id: &'static str,
    /// The name in the list and over the scene.
    pub title: &'static str,
    /// The size of the scene, cells.
    pub width: u8,
    /// The size of the scene, cells.
    pub depth: u8,
    /// The steps, in order.
    pub steps: Vec<Step>,
    /// Sneak + right click in the air with one of these opens the guide.
    pub items: Vec<&'static CustomItem>,
    /// The guide is listed and opens only while this says so.
    pub visible: fn() -> bool,
}

impl Guide {
    /// A guide with no steps yet, always visible.
    #[must_use]
    pub fn new(id: &'static str, title: &'static str, width: u8, depth: u8) -> Self {
        Self {
            id,
            title,
            width,
            depth,
            steps: Vec::new(),
            items: Vec::new(),
            visible: || true,
        }
    }

    /// Adds a step.
    #[must_use]
    pub fn step(mut self, caption: &'static str, ticks: u32, script: fn(&mut Stage)) -> Self {
        self.steps.push(Step {
            caption,
            ticks,
            script,
        });
        self
    }

    /// Sneak + right click in the air with `item` opens the guide.
    #[must_use]
    pub fn opens_with(mut self, item: &'static CustomItem) -> Self {
        self.items.push(item);
        self
    }

    /// The guide is listed and opens only while `visible` says so.
    #[must_use]
    pub fn visible_when(mut self, visible: fn() -> bool) -> Self {
        self.visible = visible;
        self
    }
}

static GUIDES: LazyLock<Vec<Guide>> = LazyLock::new(|| {
    let mut guides = bank::guides::all();
    guides.retain(|guide| !guide.steps.is_empty());
    guides
});

/// Every guide, in the order of the list.
#[must_use]
pub fn all() -> &'static [Guide] {
    &GUIDES
}

fn visible() -> impl Iterator<Item = &'static Guide> {
    all().iter().filter(|guide| (guide.visible)())
}

/// The guide `id`, if there is one to show now.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static Guide> {
    visible().find(|guide| guide.id == id)
}

/// The guide the item `stack` opens, if there is one to show now.
#[must_use]
pub fn of_item(stack: &ItemStack) -> Option<&'static Guide> {
    let id = id_of(stack)?;
    visible().find(|guide| guide.items.iter().any(|item| item.id == id))
}

/// The line under the name of `item` that tells of its guide.
#[must_use]
pub fn lore_of(item: &CustomItem) -> Option<String> {
    all()
        .iter()
        .find(|guide| guide.items.iter().any(|other| other.id == item.id))
        .map(|guide| format!("Shift + ПКМ в воздух: гайд «{}»", guide.title))
}

static OPEN: LazyLock<SyncMutex<FxHashMap<Uuid, Session>>> =
    LazyLock::new(|| SyncMutex::new(FxHashMap::default()));

/// Opens a guide for a player, in place of the one they had open.
pub fn open(player: &Arc<Player>, guide: &'static Guide) {
    close(player);
    let session = Session::open(player, guide);
    OPEN.lock().insert(player.gameprofile.id(), session);
}

/// Closes the guide a player has open.
pub fn close(player: &Player) {
    let session = OPEN.lock().remove(&player.gameprofile.id());
    if let Some(session) = session {
        session.close();
    }
}

/// A click on the entity `id`: `true` if it was a button of the player's guide.
pub(crate) fn clicked(player: &Player, id: i32) -> bool {
    let uuid = player.gameprofile.id();
    let mut open = OPEN.lock();
    let Some(button) = open.get(&uuid).and_then(|session| session.button(id)) else {
        return false;
    };
    let stays = open
        .get_mut(&uuid)
        .is_some_and(|session| session.click(button));
    if !stays && let Some(session) = open.remove(&uuid) {
        session.close();
    }
    true
}

/// Sneak + right click in the air with an item that has a guide opens it.
pub(crate) fn use_item(context: &UseItemContext) -> InteractionResult {
    if context.hand != InteractionHand::MainHand || !context.player.last_client_input().shift() {
        return InteractionResult::Pass;
    }
    let Some(guide) = context.inv.with_item(|stack| of_item(stack)) else {
        return InteractionResult::Pass;
    };
    open(context.player, guide);
    InteractionResult::Success
}

/// An item of ours that does nothing but open its guide.
pub(crate) struct Opener;

impl ItemBehavior for Opener {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        use_item(context)
    }
}

/// Plays the open guides.
struct Tick;

impl ServerJob for Tick {
    fn poll(&mut self, context: &mut ServerJobContext) -> JobPoll {
        let Some(server) = context.server() else {
            return JobPoll::Finished;
        };
        let mut open = OPEN.lock();
        if open.is_empty() {
            return JobPoll::Pending;
        }
        let online: FxHashSet<Uuid> = server
            .get_players()
            .iter()
            .map(|player| player.gameprofile.id())
            .collect();
        // Who left has nothing to take away from.
        open.retain(|uuid, _| online.contains(uuid));
        let done: Vec<Uuid> = open
            .iter_mut()
            .filter_map(|(uuid, session)| (!session.tick()).then_some(*uuid))
            .collect();
        for uuid in done {
            if let Some(session) = open.remove(&uuid) {
                session.close();
            }
        }
        JobPoll::Pending
    }
}

/// Starts playing the guides. Call once the server is ready for players.
pub fn start(server: &Server) {
    server.jobs.spawn(Tick);
    log::info!("MineHouse guides on: {}", all().len());
}

/// `/guide`: the list, `/guide <id>`, `/guide close`.
pub fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "guide"), tree).default_access(),
    )?;
    Ok(())
}

fn tree() -> CommandNode {
    let root = literal("guide")
        .executes(|context| Ok(list(*context)))
        .then(literal("close").executes(|context| {
            close(&*player(*context)?);
            Ok(1)
        }));
    all().iter().fold(root, |root, guide| {
        root.then(literal(guide.id).executes(move |context| show(*context, guide)))
    })
}

fn player(context: CommandContext<'_>) -> Result<Arc<Player>, CommandError> {
    context
        .source()
        .player()
        .map(Arc::clone)
        .ok_or_else(|| CommandError::from("Гайды показываются только игроку."))
}

/// The list of the guides: a line of each opens it.
fn list_text(guides: &[&Guide]) -> TextComponent {
    guides.iter().fold(
        TextComponent::plain("Гайды (нажми, чтобы открыть):"),
        |text, guide| {
            let command = format!("/guide {}", guide.id);
            text.add_child(
                TextComponent::plain(format!("\n  ▶ {}", guide.title))
                    .color(Color::Aqua)
                    .click_event(ClickEvent::run_command(command.clone()))
                    .hover_event(HoverEvent::show_text(TextComponent::plain(command))),
            )
        },
    )
}

fn list(context: CommandContext<'_>) -> i32 {
    let guides: Vec<&Guide> = visible().collect();
    if guides.is_empty() {
        context
            .source()
            .send_success(&TextComponent::plain("Гайдов пока нет"), false);
        return 0;
    }
    context.source().send_success(&list_text(&guides), false);
    i32::try_from(guides.len()).unwrap_or(i32::MAX)
}

fn show(context: CommandContext<'_>, guide: &'static Guide) -> Result<i32, CommandError> {
    let player = player(context)?;
    if !(guide.visible)() {
        return Err(CommandError::from(format!(
            "Нет гайда «{}». Список: /guide",
            guide.id
        )));
    }
    open(&player, guide);
    Ok(1)
}

#[cfg(test)]
mod tests;
