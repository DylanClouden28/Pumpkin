use super::{
    EnderDragonPhase, Phase, TARGET_DIST_SQR_RANGE, navigate_to_next_path_node, wrap_node_index,
};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use crate::entity::mob::Mob;
use crate::entity::player::Player;
use pumpkin_util::math::vector3::Vector3;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
    current_path: Option<Path>,
    target_location: Option<Vector3<f64>>,
    clockwise: bool,
}

/// Circling the pillars, vanilla `DragonHoldingPatternPhase`.
#[derive(Default)]
pub struct HoldingPatternPhase {
    state: Mutex<State>,
}

enum Decision {
    Land,
    Strafe(Arc<Player>),
    Continue,
}

impl HoldingPatternPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Vanilla `findNewTarget`.
    fn find_new_target(&self, dragon: &EnderDragonEntity) {
        let path_done = self
            .state()
            .current_path
            .as_ref()
            .is_some_and(Path::is_done);
        let decision = if path_done {
            Self::decide(dragon)
        } else {
            Decision::Continue
        };
        match decision {
            Decision::Land => {
                dragon.set_phase(EnderDragonPhase::LandingApproach);
                return;
            }
            Decision::Strafe(player) => {
                dragon.strafe_player(&player);
                return;
            }
            Decision::Continue => {}
        }

        // Vanilla checks `aliveCrystals() >= 0` here, so it stays on the outer ring while a fight exists.
        let outer_ring = dragon.alive_crystals().is_some();
        let mut state = self.state();
        if state.current_path.as_ref().is_none_or(Path::is_done) {
            let current_node = dragon.find_closest_node();
            let mut target_node = current_node as i32;
            if rand::random_range(0..8) == 0 {
                state.clockwise = !state.clockwise;
                target_node += 6;
            }
            target_node += if state.clockwise { 1 } else { -1 };
            let target_node = wrap_node_index(target_node, outer_ring);
            state.current_path = dragon.find_path(current_node, target_node, None);
            if let Some(path) = &mut state.current_path {
                path.advance();
            }
        }
        let State {
            current_path,
            target_location,
            ..
        } = &mut *state;
        navigate_to_next_path_node(current_path, target_location);
    }

    fn decide(dragon: &EnderDragonEntity) -> Decision {
        let egg = dragon.podium_pos(true);
        let crystals = dragon.alive_crystals().unwrap_or(0);
        if rand::random_range(0..crystals + 3) == 0 {
            return Decision::Land;
        }

        let egg_center = egg.to_centered_f64();
        let player = dragon.nearest_combat_player(egg.0.to_f64(), None, None, false);
        let dist_sqr = player.as_ref().map_or(64.0, |p| {
            p.get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&egg_center)
                / 512.0
        });
        if let Some(player) = player
            && (rand::random_range(0..(dist_sqr + 2.0) as i32) == 0
                || rand::random_range(0..crystals + 2) == 0)
        {
            return Decision::Strafe(player);
        }
        Decision::Continue
    }
}

impl Phase for HoldingPatternPhase {
    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let dist = self
            .state()
            .target_location
            .map_or(0.0, |t| t.squared_distance_to_vec(&dragon.position()));
        if !TARGET_DIST_SQR_RANGE.contains(&dist) || dragon.collided() {
            self.find_new_target(dragon);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.current_path = None;
        state.target_location = None;
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        self.state().target_location
    }

    fn on_crystal_destroyed(&self, dragon: &EnderDragonEntity, player: Option<&Arc<Player>>) {
        if let Some(player) = player
            && dragon.can_attack(player.as_ref())
        {
            dragon.strafe_player(player);
        }
    }
}
