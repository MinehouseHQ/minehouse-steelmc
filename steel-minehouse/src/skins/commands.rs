//! `/skin`, with the subcommands of `SkinRestorer` on prod:
//!
//! | Command | What it does |
//! | --- | --- |
//! | `/skin set mojang <ник> [игроки]` | the skin of a licensed account |
//! | `/skin set ely.by <ник> [игроки]` | the skin of an Ely.by account |
//! | `/skin set web classic\|slim "<ссылка>" [игроки]` | a PNG, signed by `MineSkin` |
//! | `/skin reset [игроки]` | the skin of one's own nickname again |
//! | `/skin clear [игроки]` | no skin: Steve or Alex |
//!
//! Anyone may change their own skin; the skin of others needs `minehouse.command.skin.others`
//! (the console may). The lookup runs on the Tokio runtime and the new skin is shown on a
//! later tick, to everyone at once.

use std::{future::Future, pin::Pin, sync::Arc};

use steel_core::{
    command::{
        CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
        CommandRegistrationError, CommandRegistry, CommandSource, DetachedCommandSource,
        SuspendedCommand, SuspendedCommandPoll, argument, literal,
    },
    permission::{PermissionExpr, PermissionKey},
    player::Player,
};
use steel_utils::Identifier;
use text_components::{Modifier, TextComponent, format::Color};
use tokio::{
    sync::{
        OnceCell,
        oneshot::{self, error::TryRecvError},
    },
    task::AbortHandle,
};
use uuid::Uuid;

use crate::tab;

use super::{
    Skins, auto_fetch, fetch, properties_with, providers,
    store::{ELY_BY, EMPTY, MOJANG, StoredSkin, WEB},
};

const OTHERS_PERMISSION: &str = "minehouse.command.skin.others";

/// A player a command changes the skin of.
#[derive(Clone)]
struct Target {
    uuid: Uuid,
    name: String,
}

/// What the lookup found for each target, shown on the tick.
struct Outcome {
    /// The new skin of each target that got one.
    skins: Vec<(Target, StoredSkin)>,
    /// Words for whoever ran the command, when something went wrong.
    failure: Option<String>,
}

/// A `/skin` waiting for Mojang, Ely.by or `MineSkin`.
struct Lookup {
    source: DetachedCommandSource,
    answer: oneshot::Receiver<Outcome>,
    task: AbortHandle,
    /// What to say once the skin is shown.
    done: String,
}

impl SuspendedCommand for Lookup {
    fn poll(&mut self) -> SuspendedCommandPoll {
        let outcome = match self.answer.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return SuspendedCommandPoll::Pending,
            Err(TryRecvError::Closed) => {
                return SuspendedCommandPoll::Ready(Err(CommandError::from(
                    "Команда прервана, попробуй ещё раз.",
                )));
            }
        };
        let source = self.source.source();
        let shown = show(source, &outcome.skins);
        if let Some(failure) = outcome.failure {
            return SuspendedCommandPoll::Ready(Err(CommandError::from(failure)));
        }
        source.send_success(
            &TextComponent::plain(self.done.clone()).color(Color::Green),
            false,
        );
        SuspendedCommandPoll::Ready(Ok(shown))
    }

    fn cancel(&mut self) {
        self.task.abort();
    }
}

/// Shows each found skin to everyone; returns how many players got one.
fn show(source: CommandSource<'_>, skins: &[(Target, StoredSkin)]) -> i32 {
    let online = source.server().get_players();
    let mut shown = 0;
    for (target, skin) in skins {
        let Some(player) = online
            .iter()
            .find(|player| player.gameprofile.id() == target.uuid)
        else {
            continue;
        };
        let properties = properties_with(&player.shown_profile(), skin.value.as_ref());
        match player.set_profile_properties(properties) {
            Ok(()) => shown += 1,
            Err(error) => log::warn!("Skins: the skin of {} does not fit: {error}", target.name),
        }
    }
    if shown > 0 {
        // Adding the tab entry again dropped its name and place in the list.
        tab::resend_names();
    }
    shown
}

impl Skins {
    /// Runs `work` for `targets` on the runtime; saves and shows what it finds.
    fn lookup(
        self: &Arc<Self>,
        source: CommandSource<'_>,
        targets: Vec<Target>,
        done: String,
        work: impl Fn(
            Target,
        )
            -> Pin<Box<dyn Future<Output = Result<Option<StoredSkin>, String>> + Send>>
        + Send
        + 'static,
    ) -> Lookup {
        let (sender, answer) = oneshot::channel();
        let skins = Arc::clone(self);
        let task = self.runtime.spawn(async move {
            let mut outcome = Outcome {
                skins: Vec::new(),
                failure: None,
            };
            for target in targets {
                match work(target.clone()).await {
                    Ok(Some(skin)) => {
                        skins.store.save(target.uuid, &skin);
                        outcome.skins.push((target, skin));
                    }
                    Ok(None) => {
                        skins.store.delete(target.uuid);
                        outcome
                            .skins
                            .push((target, StoredSkin::new(EMPTY, None, None, None)));
                    }
                    Err(failure) => {
                        outcome.failure = Some(failure);
                        break;
                    }
                }
            }
            let _ = sender.send(outcome);
        });
        Lookup {
            source: source.detach(),
            answer,
            task: task.abort_handle(),
            done,
        }
    }
}

pub(super) fn register(
    registry: &mut CommandRegistry,
    skins: &Arc<Skins>,
) -> Result<(), CommandRegistrationError> {
    let skins = Arc::clone(skins);
    registry.declare_permission(OTHERS_PERMISSION)?;
    registry.register(
        CommandRegistration::new(Identifier::new("minehouse", "skin"), move || tree(&skins))
            .default_access(),
    )?;
    Ok(())
}

/// `<node>` alone for oneself, or `<node> <игроки>`.
fn with_targets(
    node: CommandNode,
    run: impl for<'a> Fn(CommandContext<'a>, Vec<Target>) -> Result<Lookup, CommandError>
    + Send
    + Sync
    + Clone
    + 'static,
) -> CommandNode {
    let for_others = run.clone();
    node.executes_suspended(move |context| run(*context, own(context.source())?))
        .then(
            argument("targets", CommandArgument::players())
                .executes_suspended(move |context| for_others(*context, others(*context)?)),
        )
}

fn target_of(player: &Player) -> Target {
    Target {
        uuid: player.gameprofile.id(),
        name: player.gameprofile.name().to_owned(),
    }
}

fn own(source: CommandSource<'_>) -> Result<Vec<Target>, CommandError> {
    source
        .player()
        .map(|player| vec![target_of(player)])
        .ok_or_else(|| CommandError::from("Укажи игрока: /skin ... <ник>"))
}

fn others(context: CommandContext<'_>) -> Result<Vec<Target>, CommandError> {
    let players = context.players("targets")?;
    let source = context.source();
    let only_self = source.player().is_some_and(|me| {
        players
            .iter()
            .all(|player| player.gameprofile.id() == me.gameprofile.id())
    });
    let allowed = match source.player() {
        None => true,
        Some(player) => {
            only_self
                || PermissionKey::parse(OTHERS_PERMISSION)
                    .ok()
                    .map(PermissionExpr::key)
                    .is_some_and(|permission| player.has_permission(&permission))
        }
    };
    if !allowed {
        return Err(CommandError::from(
            "Менять скин другим игрокам могут только админы.",
        ));
    }
    Ok(players.iter().map(|player| target_of(player)).collect())
}

/// «Скин изменён», «Скин игрока Ник изменён» or «Скины 3 игроков изменены».
fn whose(targets: &[Target], source: CommandSource<'_>, one: &str, many: &str) -> String {
    let me = source.player().map(|player| player.gameprofile.id());
    match targets {
        [only] if Some(only.uuid) == me => format!("Скин {one}"),
        [only] => format!("Скин игрока {} {one}", only.name),
        all => format!("Скины игроков ({}) {many}", all.len()),
    }
}

fn tree(skins: &Arc<Skins>) -> CommandNode {
    let by_name = |provider: &'static str| {
        let skins = Arc::clone(skins);
        literal(provider).then(with_targets(
            argument("name", CommandArgument::word()),
            move |context, targets| set_by_name(&skins, context, targets, provider),
        ))
    };
    let web = |variant: &'static str| {
        let skins = Arc::clone(skins);
        literal(variant).then(with_targets(
            argument("url", CommandArgument::string()),
            move |context, targets| set_web(&skins, context, targets, variant == "slim"),
        ))
    };
    let resetting = Arc::clone(skins);
    let clearing = Arc::clone(skins);
    literal("skin")
        .then(
            literal("set")
                .then(by_name(MOJANG))
                .then(by_name(ELY_BY))
                .then(literal(WEB).then(web("classic")).then(web("slim"))),
        )
        .then(with_targets(literal("reset"), move |context, targets| {
            Ok(reset(&resetting, context, targets))
        }))
        .then(with_targets(literal("clear"), move |context, targets| {
            Ok(clear(&clearing, context, targets))
        }))
}

fn set_by_name(
    skins: &Arc<Skins>,
    context: CommandContext<'_>,
    targets: Vec<Target>,
    provider: &'static str,
) -> Result<Lookup, CommandError> {
    let name = context.string("name")?.to_owned();
    let source = context.source();
    let done = format!(
        "{}: {name} с {}.",
        whose(&targets, source, "изменён", "изменены"),
        site(provider)
    );
    Ok(skins.lookup(source, targets, done, move |_| {
        let name = name.clone();
        Box::pin(async move {
            match fetch(provider, &name).await? {
                Some(textures) => Ok(Some(StoredSkin::new(
                    provider,
                    Some(name),
                    None,
                    Some(textures),
                ))),
                None => Err(format!("На {} нет скина с ником {name}.", site(provider))),
            }
        })
    }))
}

fn site(provider: &str) -> &'static str {
    if provider == ELY_BY {
        "Ely.by"
    } else {
        "Mojang"
    }
}

fn set_web(
    skins: &Arc<Skins>,
    context: CommandContext<'_>,
    targets: Vec<Target>,
    slim: bool,
) -> Result<Lookup, CommandError> {
    let url = context.string("url")?.trim().to_owned();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(CommandError::from(
            "Нужна ссылка на png, которая начинается с https://",
        ));
    }
    let source = context.source();
    let done = format!(
        "{}: скин по ссылке.",
        whose(&targets, source, "изменён", "изменены")
    );
    // One upload serves every target.
    let signed: Arc<OnceCell<Result<StoredSkin, String>>> = Arc::new(OnceCell::new());
    Ok(skins.lookup(source, targets, done, move |target| {
        let url = url.clone();
        let signed = Arc::clone(&signed);
        Box::pin(async move {
            signed
                .get_or_init(|| async {
                    providers::mineskin(&url, slim, &target.name)
                        .await
                        .map(|textures| {
                            StoredSkin::new(
                                WEB,
                                Some(url.clone()),
                                Some(if slim { "slim" } else { "classic" }.to_owned()),
                                Some(textures),
                            )
                        })
                })
                .await
                .clone()
                .map(Some)
        })
    }))
}

fn reset(skins: &Arc<Skins>, context: CommandContext<'_>, targets: Vec<Target>) -> Lookup {
    let source = context.source();
    let done = format!(
        "{}: снова скин по нику.",
        whose(&targets, source, "сброшен", "сброшены")
    );
    skins.lookup(source, targets, done, |target| {
        Box::pin(async move { Ok(auto_fetch(&target.name).await) })
    })
}

fn clear(skins: &Arc<Skins>, context: CommandContext<'_>, targets: Vec<Target>) -> Lookup {
    let source = context.source();
    let done = format!("{}.", whose(&targets, source, "убран", "убраны"));
    skins.lookup(source, targets, done, |_| {
        Box::pin(async { Ok(Some(StoredSkin::new(EMPTY, None, None, None))) })
    })
}
