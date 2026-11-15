//! Entity selection predicates shared by queries, after vanilla `EntitySelector`.

use steel_protocol::packets::game::TeamCollisionRule;

use super::Entity;

pub(crate) struct EntitySelector;

impl EntitySelector {
    /// Vanilla `pushableBy`: the team collision rules of both sides decide
    /// whether `pusher` pushes an entity. The id check stands in for the
    /// `except` entity of vanilla's `getEntities` call.
    pub(crate) fn pushable_by(pusher: &dyn Entity) -> impl Fn(&dyn Entity) -> bool {
        let pusher_id = pusher.id();
        let own_team = pusher.team();
        let own_rule = own_team.map_or(TeamCollisionRule::Always, |team| team.collision_rule());

        move |entity| {
            if own_rule == TeamCollisionRule::Never
                || entity.id() == pusher_id
                || entity.is_spectator()
                || !entity.is_pushable()
            {
                return false;
            }
            let their_team = entity.team();
            let their_rule =
                their_team.map_or(TeamCollisionRule::Always, |team| team.collision_rule());
            if their_rule == TeamCollisionRule::Never {
                return false;
            }
            let same_team = own_team.is_some_and(|team| team.is_allied_to(their_team.as_ref()));
            if (own_rule == TeamCollisionRule::PushOwnTeam
                || their_rule == TeamCollisionRule::PushOwnTeam)
                && same_team
            {
                return false;
            }
            own_rule != TeamCollisionRule::PushOtherTeams
                && their_rule != TeamCollisionRule::PushOtherTeams
                || same_team
        }
    }
}
