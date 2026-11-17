//! Vanilla fish: cod, salmon, pufferfish and tropical fish.

mod cod;
mod pufferfish;
mod salmon;
mod tropical_fish;

pub use cod::CodEntity;
pub use pufferfish::PufferfishEntity;
pub use salmon::{SalmonEntity, SalmonVariant};
pub use tropical_fish::{
    COMMON_TROPICAL_FISH_VARIANTS, TropicalFishBase, TropicalFishEntity, TropicalFishPattern,
    TropicalFishVariant,
};

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::{CodEntity, PufferfishEntity, SalmonEntity, TropicalFishEntity};
    use crate::entity::Mob;

    #[test]
    fn fish_spawn_in_vanilla_cluster_sizes() {
        init_vanilla_registry();
        let at = DVec3::ZERO;
        let cod = CodEntity::new(&vanilla_entities::COD, 1, at, Weak::new());
        let salmon = SalmonEntity::new(&vanilla_entities::SALMON, 2, at, Weak::new());
        let pufferfish = PufferfishEntity::new(&vanilla_entities::PUFFERFISH, 3, at, Weak::new());
        let tropical =
            TropicalFishEntity::new(&vanilla_entities::TROPICAL_FISH, 4, at, Weak::new());

        assert_eq!(cod.max_spawn_cluster_size(), 8);
        assert_eq!(
            salmon.max_spawn_cluster_size(),
            5,
            "salmon schools are smaller"
        );
        assert_eq!(pufferfish.max_spawn_cluster_size(), 8);
        assert_eq!(tropical.max_spawn_cluster_size(), 8);
        assert!(
            !tropical.is_max_group_size_reached(1),
            "a fresh tropical fish schools"
        );
    }
}
