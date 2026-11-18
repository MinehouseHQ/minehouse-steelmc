//! Vanilla's reloadable `predicate`, `context_int_provider` and
//! `context_float_provider` registries, one set per Steel domain: the built-in
//! vanilla entries with each selected data pack's files on top.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock};

use serde::Deserialize;
use serde_json::Value;
use steel_utils::Identifier;
use steel_utils::locks::SyncRwLock;

use super::{
    ContextFloatProvider, ContextIntProvider, LootCondition, LootElement, References, from_json,
};
use crate::command::function::{TagEntry, build_tags, load_tag_files};
use crate::datapack::DataPack;

/// One of the loot registries, or the tags of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum LootRegistryKind {
    /// `minecraft:predicate`.
    Predicate,
    /// Tags of `minecraft:predicate`.
    PredicateTag,
    /// `minecraft:context_int_provider`.
    IntProvider,
    /// Tags of `minecraft:context_int_provider`.
    IntProviderTag,
    /// `minecraft:context_float_provider`.
    FloatProvider,
    /// Tags of `minecraft:context_float_provider`.
    FloatProviderTag,
}

impl LootRegistryKind {
    /// The registry's id, as vanilla's error messages name it.
    pub(crate) const fn registry_name(self) -> &'static str {
        match self {
            Self::Predicate | Self::PredicateTag => "minecraft:predicate",
            Self::IntProvider | Self::IntProviderTag => "minecraft:context_int_provider",
            Self::FloatProvider | Self::FloatProviderTag => "minecraft:context_float_provider",
        }
    }

    /// The registry's directory in a data pack.
    const fn directory(self) -> &'static str {
        match self {
            Self::Predicate | Self::PredicateTag => "predicate",
            Self::IntProvider | Self::IntProviderTag => "context_int_provider",
            Self::FloatProvider | Self::FloatProviderTag => "context_float_provider",
        }
    }

    const fn is_tag(self) -> bool {
        matches!(
            self,
            Self::PredicateTag | Self::IntProviderTag | Self::FloatProviderTag
        )
    }

    /// The registry a tag kind lists entries of.
    const fn entries(self) -> Self {
        match self {
            Self::Predicate | Self::PredicateTag => Self::Predicate,
            Self::IntProvider | Self::IntProviderTag => Self::IntProvider,
            Self::FloatProvider | Self::FloatProviderTag => Self::FloatProvider,
        }
    }
}

/// One loot registry: its entries by id, and its tags as entry ids.
#[derive(Debug)]
pub(crate) struct LootRegistry<T> {
    entries: BTreeMap<Identifier, Arc<T>>,
    tags: BTreeMap<Identifier, Vec<Identifier>>,
}

impl<T> Default for LootRegistry<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            tags: BTreeMap::new(),
        }
    }
}

/// A domain's loot registries.
#[derive(Debug, Default)]
pub(crate) struct LootRegistries {
    predicates: LootRegistry<LootCondition>,
    ints: LootRegistry<ContextIntProvider>,
    floats: LootRegistry<ContextFloatProvider>,
}

/// Which of [`LootRegistries`]' registries holds `T`.
pub(crate) trait InRegistries: Sized {
    /// The registry holding values of this type.
    fn registry(registries: &LootRegistries) -> &LootRegistry<Self>;
    /// The same registry, mutably.
    fn registry_mut(registries: &mut LootRegistries) -> &mut LootRegistry<Self>;
}

impl InRegistries for LootCondition {
    fn registry(registries: &LootRegistries) -> &LootRegistry<Self> {
        &registries.predicates
    }

    fn registry_mut(registries: &mut LootRegistries) -> &mut LootRegistry<Self> {
        &mut registries.predicates
    }
}

impl InRegistries for ContextIntProvider {
    fn registry(registries: &LootRegistries) -> &LootRegistry<Self> {
        &registries.ints
    }

    fn registry_mut(registries: &mut LootRegistries) -> &mut LootRegistry<Self> {
        &mut registries.ints
    }
}

impl InRegistries for ContextFloatProvider {
    fn registry(registries: &LootRegistries) -> &LootRegistry<Self> {
        &registries.floats
    }

    fn registry_mut(registries: &mut LootRegistries) -> &mut LootRegistry<Self> {
        &mut registries.floats
    }
}

macro_rules! vanilla_entries {
    ($directory:literal: $($path:literal),* $(,)?) => {
        &[$((
            $path,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../steel-utils/build_assets/builtin_datapacks/minecraft/",
                $directory,
                "/",
                $path,
                ".json"
            )),
        )),*]
    };
}

/// Vanilla's built-in `predicate` entries (`LootPredicates.bootstrap`).
const VANILLA_PREDICATES: &[(&str, &str)] = vanilla_entries!["predicate":
    "block/fast_cooking",
    "tool/can_shear",
    "tool/can_silk_touch",
];

/// Vanilla's built-in `context_int_provider` entries.
const VANILLA_INT_PROVIDERS: &[(&str, &str)] = vanilla_entries!["context_int_provider":
    "brewing/uses_default",
    "compostable/always_add_one",
    "compostable/low",
    "compostable/low_medium",
    "compostable/medium",
    "compostable/medium_high",
    "cooking/fast_burn_time_reduction_factor",
    "cooking/normal_burn_time_reduction_factor",
    "cooking/time_bamboo",
    "cooking/time_blaze_rod",
    "cooking/time_boats",
    "cooking/time_coal",
    "cooking/time_coal_block",
    "cooking/time_dried_kelp_block",
    "cooking/time_dry_plants",
    "cooking/time_hanging_signs",
    "cooking/time_lava_bucket",
    "cooking/time_roots",
    "cooking/time_wood_blocks",
    "cooking/time_wood_items_extra_small",
    "cooking/time_wood_items_large",
    "cooking/time_wood_items_small",
    "cooking/time_wood_slabs",
    "cooking/time_wool",
    "cooking/time_wool_carpets",
    "cooking/time_wool_slabs",
];

/// Vanilla's built-in `context_float_provider` entries.
const VANILLA_FLOAT_PROVIDERS: &[(&str, &str)] = vanilla_entries!["context_float_provider":
    "brewing/speed_default",
    "cooking/fast_speed_multiplier",
    "cooking/normal_speed_multiplier",
    "cooking/speed_default",
];

/// The registries with only vanilla's built-in entries, for a domain whose
/// packs have not loaded and for tests.
static VANILLA: LazyLock<Arc<LootRegistries>> =
    LazyLock::new(|| Arc::new(LootRegistries::load(&[])));

impl LootRegistries {
    /// The registries with only vanilla's built-in entries.
    pub(crate) fn vanilla() -> Arc<Self> {
        Arc::clone(&VANILLA)
    }

    /// Vanilla `RegistryDataLoader` for the three registries: the built-in
    /// entries, then each pack's files in order, a later file replacing an
    /// earlier one with the same id, then the tags.
    ///
    /// A file that fails to parse is logged and left out. So is an entry
    /// that refers to a missing entry or tag, or to itself through other
    /// entries, as vanilla's validation reports them.
    pub(crate) fn load(packs: &[&DataPack]) -> Self {
        let mut files: BTreeMap<LootRegistryKind, BTreeMap<Identifier, Vec<u8>>> = BTreeMap::new();
        for (kind, vanilla) in [
            (LootRegistryKind::Predicate, VANILLA_PREDICATES),
            (LootRegistryKind::IntProvider, VANILLA_INT_PROVIDERS),
            (LootRegistryKind::FloatProvider, VANILLA_FLOAT_PROVIDERS),
        ] {
            let kind_files = files.entry(kind).or_default();
            for (path, contents) in vanilla {
                kind_files.insert(
                    Identifier::vanilla((*path).to_owned()),
                    contents.as_bytes().to_vec(),
                );
            }
            for pack in packs {
                for resource in pack.list_resources(kind.directory(), ".json") {
                    kind_files.insert(resource.id, resource.contents);
                }
            }
        }
        let mut registries = Self::default();
        decode_all::<LootCondition>(&mut registries, files.remove(&LootRegistryKind::Predicate));
        decode_all::<ContextIntProvider>(
            &mut registries,
            files.remove(&LootRegistryKind::IntProvider),
        );
        decode_all::<ContextFloatProvider>(
            &mut registries,
            files.remove(&LootRegistryKind::FloatProvider),
        );
        let tag_files = [
            LootRegistryKind::PredicateTag,
            LootRegistryKind::IntProviderTag,
            LootRegistryKind::FloatProviderTag,
        ]
        .map(|kind| {
            (
                kind,
                load_tag_files(packs, &format!("tags/{}", kind.directory())),
            )
        });
        registries.validate(&tag_files);
        registries
    }

    /// Builds the tags and drops invalid entries until nothing changes.
    fn validate(&mut self, tag_files: &[(LootRegistryKind, BTreeMap<Identifier, Vec<TagEntry>>)]) {
        loop {
            for (kind, files) in tag_files {
                self.build_tags(*kind, files);
            }
            let mut invalid = self.missing_reference_entries();
            if invalid.is_empty() {
                invalid = self.cyclic_entries();
            }
            if invalid.is_empty() {
                return;
            }
            for (kind, id) in invalid {
                self.remove(kind, &id);
            }
        }
    }

    fn build_tags(&mut self, kind: LootRegistryKind, files: &BTreeMap<Identifier, Vec<TagEntry>>) {
        let entries = kind.entries();
        let tags = build_tags(
            files,
            |id| self.contains(entries, id).then(|| id.clone()),
            PartialEq::eq,
        );
        match kind {
            LootRegistryKind::PredicateTag => self.predicates.tags = tags,
            LootRegistryKind::IntProviderTag => self.ints.tags = tags,
            _ => self.floats.tags = tags,
        }
    }

    /// Entries referring to an entry or tag the registries lack, each logged.
    fn missing_reference_entries(&self) -> Vec<(LootRegistryKind, Identifier)> {
        let mut invalid = Vec::new();
        for (kind, id, references) in self.all_references() {
            if let Some((missing_kind, missing)) = references
                .iter()
                .find(|(kind, id)| !self.contains(*kind, id))
            {
                log::error!(
                    "Failed to load {} {id}: missing {}element {missing} in {}",
                    kind.registry_name(),
                    if missing_kind.is_tag() { "tag " } else { "" },
                    missing_kind.registry_name()
                );
                invalid.push((kind, id));
            }
        }
        invalid
    }

    /// Entries that refer back to themselves, each logged.
    fn cyclic_entries(&self) -> Vec<(LootRegistryKind, Identifier)> {
        let graph = self
            .all_references()
            .into_iter()
            .map(|(kind, id, references)| {
                let edges = references
                    .into_iter()
                    .flat_map(|(kind, id)| self.expand_tag(kind, id))
                    .collect::<Vec<_>>();
                ((kind, id), edges)
            })
            .collect::<BTreeMap<_, _>>();
        let mut cyclic = BTreeSet::new();
        for start in graph.keys() {
            if reaches(&graph, start, start) {
                cyclic.insert(start.clone());
            }
        }
        for (kind, id) in &cyclic {
            log::error!(
                "Failed to load {} {id}: it refers to itself",
                kind.registry_name()
            );
        }
        cyclic.into_iter().collect()
    }

    /// The entries a reference stands for: itself, or a tag's members.
    fn expand_tag(
        &self,
        kind: LootRegistryKind,
        id: Identifier,
    ) -> Vec<(LootRegistryKind, Identifier)> {
        if !kind.is_tag() {
            return vec![(kind, id)];
        }
        let members = match kind {
            LootRegistryKind::PredicateTag => self.predicates.tags.get(&id),
            LootRegistryKind::IntProviderTag => self.ints.tags.get(&id),
            _ => self.floats.tags.get(&id),
        };
        members
            .into_iter()
            .flatten()
            .map(|member| (kind.entries(), member.clone()))
            .collect()
    }

    fn all_references(&self) -> Vec<(LootRegistryKind, Identifier, References)> {
        let mut all = Vec::new();
        collect_references(&self.predicates, &mut all);
        collect_references(&self.ints, &mut all);
        collect_references(&self.floats, &mut all);
        all
    }

    fn remove(&mut self, kind: LootRegistryKind, id: &Identifier) {
        match kind.entries() {
            LootRegistryKind::Predicate => {
                self.predicates.entries.remove(id);
            }
            LootRegistryKind::IntProvider => {
                self.ints.entries.remove(id);
            }
            _ => {
                self.floats.entries.remove(id);
            }
        }
    }

    /// Whether the registry (or tag set) `kind` has `id`.
    pub(crate) fn contains(&self, kind: LootRegistryKind, id: &Identifier) -> bool {
        match kind {
            LootRegistryKind::Predicate => self.predicates.entries.contains_key(id),
            LootRegistryKind::PredicateTag => self.predicates.tags.contains_key(id),
            LootRegistryKind::IntProvider => self.ints.entries.contains_key(id),
            LootRegistryKind::IntProviderTag => self.ints.tags.contains_key(id),
            LootRegistryKind::FloatProvider => self.floats.entries.contains_key(id),
            LootRegistryKind::FloatProviderTag => self.floats.tags.contains_key(id),
        }
    }

    /// The ids of the registry `kind`, sorted.
    pub(crate) fn ids(&self, kind: LootRegistryKind) -> Vec<Identifier> {
        match kind.entries() {
            LootRegistryKind::Predicate => self.predicates.entries.keys().cloned().collect(),
            LootRegistryKind::IntProvider => self.ints.entries.keys().cloned().collect(),
            _ => self.floats.entries.keys().cloned().collect(),
        }
    }

    /// The entry `id` of `T`'s registry.
    pub(crate) fn get<T: LootElement>(&self, id: &Identifier) -> Option<Arc<T>> {
        T::registry(self).entries.get(id).cloned()
    }

    /// The entries of `T`'s tag `id`, in tag order.
    pub(crate) fn tag<T: LootElement>(&self, id: &Identifier) -> Option<Vec<Arc<T>>> {
        let registry = T::registry(self);
        registry
            .tags
            .get(id)?
            .iter()
            .map(|member| registry.entries.get(member).cloned())
            .collect()
    }

    /// Vanilla's decode error for a value written in a command that refers
    /// to an entry or tag these registries lack.
    pub(crate) fn missing_reference(&self, value: &impl LootElement) -> Option<String> {
        let mut references = Vec::new();
        value.references(&mut references);
        references
            .into_iter()
            .find(|(kind, id)| !self.contains(*kind, id))
            .map(|(kind, id)| {
                format!(
                    "Missing {}element ResourceKey[{} / {id}]",
                    if kind.is_tag() { "tag " } else { "" },
                    kind.registry_name()
                )
            })
    }
}

fn decode_all<T: LootElement + for<'de> Deserialize<'de>>(
    registries: &mut LootRegistries,
    files: Option<BTreeMap<Identifier, Vec<u8>>>,
) {
    let registry = T::registry_mut(registries);
    for (id, contents) in files.into_iter().flatten() {
        let decoded = serde_json::from_slice::<Value>(&contents)
            .map_err(|error| error.to_string())
            .and_then(from_json::<T>);
        match decoded {
            Ok(value) => {
                registry.entries.insert(id, Arc::new(value));
            }
            Err(error) => log::error!("Failed to parse {} {id}: {error}", T::KIND.registry_name()),
        }
    }
}

fn collect_references<T: LootElement>(
    registry: &LootRegistry<T>,
    out: &mut Vec<(LootRegistryKind, Identifier, References)>,
) {
    for (id, value) in &registry.entries {
        let mut references = Vec::new();
        value.references(&mut references);
        out.push((T::KIND, id.clone(), references));
    }
}

/// Whether `target` can be reached from `from` along at least one edge.
fn reaches(
    graph: &BTreeMap<(LootRegistryKind, Identifier), Vec<(LootRegistryKind, Identifier)>>,
    from: &(LootRegistryKind, Identifier),
    target: &(LootRegistryKind, Identifier),
) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = graph.get(from).cloned().unwrap_or_default();
    while let Some(node) = stack.pop() {
        if &node == target {
            return true;
        }
        if seen.insert(node.clone())
            && let Some(edges) = graph.get(&node)
        {
            stack.extend(edges.iter().cloned());
        }
    }
    false
}

/// The loot registries of every Steel domain.
#[derive(Default)]
pub(crate) struct ServerLootRegistries {
    domains: SyncRwLock<BTreeMap<String, Arc<LootRegistries>>>,
}

impl ServerLootRegistries {
    /// The registries loaded for `domain`; vanilla's built-in entries before
    /// its packs load.
    pub(crate) fn get(&self, domain: &str) -> Arc<LootRegistries> {
        self.domains
            .read()
            .get(domain)
            .cloned()
            .unwrap_or_else(LootRegistries::vanilla)
    }

    /// Makes `registries` the domain's, replacing the last.
    pub(crate) fn replace(&self, domain: &str, registries: LootRegistries) {
        self.domains
            .write()
            .insert(domain.to_owned(), Arc::new(registries));
    }
}
