//! Vanilla `PackRepository` with its `ServerPacksSource` and
//! `FolderRepositorySource`: the packs a domain can enable and the ones it has.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use steel_utils::locks::SyncMutex;
use steel_utils::translations;
use text_components::format::Color;
use text_components::interactivity::HoverEvent;
use text_components::translation::Translation;
use text_components::{Modifier as _, TextComponent};

use super::{DataPack, FeatureFlagSet, find_packs};
use crate::command::brigadier::escape_if_required;
use crate::level_data::DataPackConfig;

/// Vanilla's id for the built-in pack.
pub(crate) const VANILLA_PACK_ID: &str = "vanilla";

/// Vanilla `PackSource`: where a pack comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PackSource {
    /// `PackSource.BUILT_IN`: the vanilla pack.
    BuiltIn,
    /// `PackSource.FEATURE`: the bundled experimental feature packs.
    Feature,
    /// `PackSource.WORLD`: the level's `datapacks` folder.
    World,
}

impl PackSource {
    /// Vanilla `shouldAddAutomatically`.
    const fn should_add_automatically(self) -> bool {
        !matches!(self, Self::Feature)
    }

    /// Vanilla `decorate`: the name followed by the gray source.
    fn decorate(self, name: TextComponent) -> TextComponent {
        let source = match self {
            Self::BuiltIn => &translations::PACK_SOURCE_BUILTIN,
            Self::Feature => &translations::PACK_SOURCE_FEATURE,
            Self::World => &translations::PACK_SOURCE_WORLD,
        };
        translations::PACK_NAME_AND_SOURCE
            .message([name, source.msg().component()])
            .component()
            .color(Color::Gray)
    }
}

/// Vanilla `Pack.Position`: the end of the selection a pack goes to by default.
///
/// Vanilla's `insert` skips packs pinned to that end; no pack Steel knows is
/// pinned (vanilla's `fixedPosition`), so `Bottom` is the start and `Top` the
/// end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackPosition {
    Top,
    Bottom,
}

/// Vanilla `Pack`: one pack a domain can enable.
pub(crate) struct Pack {
    id: String,
    title: TextComponent,
    source: PackSource,
    description: TextComponent,
    requested_features: FeatureFlagSet,
    default_position: PackPosition,
    /// The files of a pack from the `datapacks` folder. The built-in packs
    /// carry no functions, which are all Steel loads from packs.
    files: Option<DataPack>,
}

impl Pack {
    /// Vanilla `getId`.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// Vanilla `getPackSource`.
    pub(crate) const fn source(&self) -> PackSource {
        self.source
    }

    /// Vanilla `getRequestedFeatures`.
    pub(crate) const fn requested_features(&self) -> &FeatureFlagSet {
        &self.requested_features
    }

    /// The pack's files, for packs from the `datapacks` folder.
    pub(crate) const fn files(&self) -> Option<&DataPack> {
        self.files.as_ref()
    }

    /// Vanilla `getChatLink`: the decorated id in square brackets, green when
    /// enabled, inserting the id and showing the title and description.
    pub(crate) fn chat_link(&self, enabled: bool) -> TextComponent {
        let mut hover = TextComponent::new();
        hover.children.push(self.title.clone());
        hover.children.push(TextComponent::plain("\n"));
        hover.children.push(self.description.clone());
        translations::CHAT_SQUARE_BRACKETS
            .message([self.source.decorate(TextComponent::plain(self.id.clone()))])
            .component()
            .color(if enabled { Color::Green } else { Color::Red })
            .insertion(escape_if_required(&self.id))
            .hover_event(HoverEvent::show_text(hover))
    }

    /// Vanilla `getDefaultPosition().insert(selected, this, ...)`.
    pub(crate) fn insert_at_default_position(self: &Arc<Self>, selected: &mut Vec<Arc<Self>>) {
        match self.default_position {
            PackPosition::Bottom => selected.insert(0, Arc::clone(self)),
            PackPosition::Top => selected.push(Arc::clone(self)),
        }
    }

    /// Vanilla `ServerPacksSource.createVanillaPack`.
    fn vanilla() -> Self {
        Self {
            id: VANILLA_PACK_ID.to_owned(),
            title: translations::DATA_PACK_VANILLA_NAME.msg().component(),
            source: PackSource::BuiltIn,
            description: translations::DATA_PACK_VANILLA_DESCRIPTION
                .msg()
                .component(),
            requested_features: FeatureFlagSet::vanilla(),
            default_position: PackPosition::Bottom,
            files: None,
        }
    }

    /// Vanilla `ServerPacksSource.createBuiltinPack` for a bundled feature pack.
    fn feature(id: &'static str, description: &Translation<0>) -> Self {
        Self {
            id: id.to_owned(),
            title: TextComponent::plain(id),
            source: PackSource::Feature,
            description: description.msg().component(),
            requested_features: FeatureFlagSet::of(&[id]),
            default_position: PackPosition::Top,
            files: None,
        }
    }

    /// Vanilla `FolderRepositorySource`'s pack for `files`.
    fn world(files: DataPack) -> Self {
        Self {
            id: files.id().to_owned(),
            title: TextComponent::plain(files.name().to_owned()),
            source: PackSource::World,
            description: files.metadata().description.clone(),
            requested_features: files.metadata().requested_features.clone(),
            default_position: PackPosition::Top,
            files: Some(files),
        }
    }
}

/// Vanilla `PackRepository` for one domain.
pub(crate) struct PackRepository {
    /// The level's `datapacks` folder; a world without a folder has only the
    /// built-in packs.
    directory: Option<PathBuf>,
    available: BTreeMap<String, Arc<Pack>>,
    selected: Vec<Arc<Pack>>,
}

impl PackRepository {
    /// A repository over the built-in packs and `directory`'s packs. Nothing is
    /// available until [`Self::reload`].
    pub(crate) const fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            available: BTreeMap::new(),
            selected: Vec::new(),
        }
    }

    /// Vanilla `reload`: finds the packs anew, keeping the selected ones that
    /// are still there.
    pub(crate) fn reload(&mut self) {
        let selected = self.selected_ids();
        self.available = self.discover_available();
        self.set_selected(&selected);
    }

    fn discover_available(&self) -> BTreeMap<String, Arc<Pack>> {
        let mut packs = vec![
            Pack::vanilla(),
            Pack::feature(
                "minecart_improvements",
                &translations::DATA_PACK_MINECART_IMPROVEMENTS_DESCRIPTION,
            ),
            Pack::feature(
                "redstone_experiments",
                &translations::DATA_PACK_REDSTONE_EXPERIMENTS_DESCRIPTION,
            ),
            Pack::feature(
                "trade_rebalance",
                &translations::DATA_PACK_TRADE_REBALANCE_DESCRIPTION,
            ),
        ];
        if let Some(directory) = &self.directory {
            packs.extend(find_packs(directory).into_iter().map(Pack::world));
        }
        packs
            .into_iter()
            .map(|pack| (pack.id.clone(), Arc::new(pack)))
            .collect()
    }

    /// Vanilla `setSelected`: the available packs of `ids`, in that order.
    pub(crate) fn set_selected(&mut self, ids: &[String]) {
        self.selected = ids
            .iter()
            .filter_map(|id| self.available.get(id).cloned())
            .collect();
    }

    /// Vanilla `getSelectedIds`.
    pub(crate) fn selected_ids(&self) -> Vec<String> {
        self.selected.iter().map(|pack| pack.id.clone()).collect()
    }

    /// Vanilla `getSelectedPacks`, from the bottom of the selection up.
    pub(crate) fn selected_packs(&self) -> &[Arc<Pack>] {
        &self.selected
    }

    /// Whether `pack` is selected.
    pub(crate) fn is_selected(&self, pack: &Pack) -> bool {
        self.selected.iter().any(|selected| selected.id == pack.id)
    }

    /// Vanilla `getAvailablePacks`, by id.
    pub(crate) fn available_packs(&self) -> impl Iterator<Item = &Arc<Pack>> {
        self.available.values()
    }

    /// Vanilla `getPack`.
    pub(crate) fn pack(&self, id: &str) -> Option<Arc<Pack>> {
        self.available.get(id).cloned()
    }

    /// Vanilla `MinecraftServer.configurePackRepository` for a world that is
    /// not new, outside safe mode: the saved selection, plus any new pack that
    /// adds itself, less the packs needing features `enabled_features` lacks.
    /// Returns the selection to save.
    ///
    /// Vanilla then forces on feature packs for enabled features no pack
    /// provides; Steel worlds enable only the vanilla flag, which no feature
    /// pack provides, so there is nothing to force.
    pub(crate) fn configure(
        &mut self,
        config: &DataPackConfig,
        enabled_features: &FeatureFlagSet,
    ) -> DataPackConfig {
        self.reload();
        let mut selected = Vec::<String>::new();
        for id in &config.enabled {
            if self.available.contains_key(id) {
                if !selected.contains(id) {
                    selected.push(id.clone());
                }
            } else {
                log::warn!("Missing data pack {id}");
            }
        }
        for pack in self.available.values() {
            if config.disabled.contains(&pack.id) {
                continue;
            }
            let features = &pack.requested_features;
            let is_selected = selected.contains(&pack.id);
            if !is_selected && pack.source.should_add_automatically() {
                if features.is_subset_of(enabled_features) {
                    log::info!("Found new data pack {}, loading it automatically", pack.id);
                    selected.push(pack.id.clone());
                } else {
                    log::info!(
                        "Found new data pack {}, but can't load it due to missing features {}",
                        pack.id,
                        features.print_missing(enabled_features)
                    );
                }
            }
            if is_selected && !features.is_subset_of(enabled_features) {
                log::warn!(
                    "Pack {} requires features {} that are not enabled for this world, disabling pack.",
                    pack.id,
                    features.print_missing(enabled_features)
                );
                selected.retain(|id| *id != pack.id);
            }
        }
        if selected.is_empty() {
            log::info!("No datapacks selected, forcing vanilla");
            selected.push(VANILLA_PACK_ID.to_owned());
        }
        self.set_selected(&selected);
        self.selection_config()
    }

    /// Vanilla `MinecraftServer.getSelectedPacks(repository, true)`: the
    /// selection, and every other available pack as disabled.
    pub(crate) fn selection_config(&self) -> DataPackConfig {
        DataPackConfig {
            enabled: self.selected_ids(),
            disabled: self
                .available
                .values()
                .filter(|pack| !self.is_selected(pack))
                .map(|pack| pack.id.clone())
                .collect(),
        }
    }
}

/// Every Steel domain's [`PackRepository`].
#[derive(Default)]
pub(crate) struct PackRepositories {
    domains: SyncMutex<BTreeMap<String, PackRepository>>,
}

impl PackRepositories {
    /// Replaces `domain`'s repository.
    pub(crate) fn insert(&self, domain: &str, repository: PackRepository) {
        self.domains.lock().insert(domain.to_owned(), repository);
    }

    /// Runs `f` on `domain`'s repository, if it has one.
    pub(crate) fn with<R>(
        &self,
        domain: &str,
        f: impl FnOnce(&mut PackRepository) -> R,
    ) -> Option<R> {
        self.domains.lock().get_mut(domain).map(f)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::{env, fs, process};

    use super::*;
    use crate::datapack::tests::PACK_MCMETA;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("steel-packs-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir
    }

    fn add_pack(dir: &Path, name: &str, mcmeta: &str) {
        fs::create_dir_all(dir.join(name)).expect("pack dir");
        fs::write(dir.join(name).join("pack.mcmeta"), mcmeta).expect("pack.mcmeta");
    }

    fn config(enabled: &[&str], disabled: &[&str]) -> DataPackConfig {
        DataPackConfig {
            enabled: enabled.iter().map(|id| (*id).to_owned()).collect(),
            disabled: disabled.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    #[test]
    fn a_new_world_enables_vanilla_and_its_world_packs_but_not_feature_packs() {
        let dir = temp_dir("new");
        add_pack(&dir, "b", PACK_MCMETA);
        add_pack(&dir, "a", PACK_MCMETA);
        let mut repository = PackRepository::new(Some(dir.clone()));
        let saved = repository.configure(&DataPackConfig::default(), &FeatureFlagSet::vanilla());
        assert_eq!(saved.enabled, ["vanilla", "file/a", "file/b"]);
        assert_eq!(
            saved.disabled,
            [
                "minecart_improvements",
                "redstone_experiments",
                "trade_rebalance"
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_saved_selection_keeps_its_order_and_disabled_packs_stay_off() {
        let dir = temp_dir("saved");
        add_pack(&dir, "a", PACK_MCMETA);
        add_pack(&dir, "b", PACK_MCMETA);
        add_pack(&dir, "c", PACK_MCMETA);
        let mut repository = PackRepository::new(Some(dir.clone()));
        let saved = repository.configure(
            &config(&["file/b", "vanilla", "file/gone"], &["file/a"]),
            &FeatureFlagSet::vanilla(),
        );
        assert_eq!(saved.enabled, ["file/b", "vanilla", "file/c"]);
        assert!(saved.disabled.contains(&"file/a".to_owned()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn packs_needing_missing_features_are_dropped_and_vanilla_is_forced_last() {
        let dir = temp_dir("features");
        add_pack(
            &dir,
            "trades",
            r#"{"pack":{"description":""},"features":{"enabled":["minecraft:trade_rebalance"]}}"#,
        );
        let mut repository = PackRepository::new(Some(dir.clone()));
        let saved = repository.configure(
            &config(&["file/trades", "trade_rebalance"], &["vanilla"]),
            &FeatureFlagSet::vanilla(),
        );
        assert_eq!(saved.enabled, ["vanilla"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_links_show_the_source_and_insert_the_escaped_id() {
        let dir = temp_dir("links");
        add_pack(&dir, "my pack", PACK_MCMETA);
        let mut repository = PackRepository::new(Some(dir.clone()));
        repository.reload();
        let pack = repository
            .pack("file/my pack")
            .expect("pack should be found");
        let link = serde_json::to_value(pack.chat_link(true)).expect("link should encode");
        assert_eq!(link["insertion"], "\"file/my pack\"");
        assert_eq!(link["color"], "green");
        assert_eq!(link["translate"], "chat.square_brackets");
        assert_eq!(link["with"][0]["translate"], "pack.nameAndSource");
        assert_eq!(link["with"][0]["with"][1]["translate"], "pack.source.world");
        let _ = fs::remove_dir_all(&dir);
    }
}
