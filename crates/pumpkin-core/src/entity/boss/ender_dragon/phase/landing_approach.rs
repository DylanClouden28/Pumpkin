use super::{EnderDragonPhase, Phase, TARGET_DIST_SQR_RANGE, navigate_to_next_path_node};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Mutex;

#[derive(Default)]
struct State {
    current_path: Option<Path>,
    target_location: Option<Vector3<f64>>,
}

/// Flying down the node path to the podium, vanilla `DragonLandingApproachPhase`.
#[derive(Default)]
pub struct LandingApproachPhase {
    state: Mutex<State>,
}

impl LandingApproachPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn find_new_target(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        if state.current_path.as_ref().is_none_or(Path::is_done) {
            let current_node = dragon.find_closest_node();
            let egg = dragon.podium_pos(true);
            let target_node = dragon
                .nearest_combat_player(egg.0.to_f64(), None, None, false)
                .map_or_else(
                    || dragon.find_closest_node_to(Vector3::new(40.0, f64::from(egg.0.y), 0.0)),
                    |player| {
                        // Vanilla normalizes the raw position, so it lands opposite the player.
                        let p = player.get_entity().pos.load();
                        let aim = Vector3::new(p.x, 0.0, p.z).vanilla_normalize();
                        dragon.find_closest_node_to(Vector3::new(
                            -aim.x * 40.0,
                            105.0,
                            -aim.z * 40.0,
                        ))
                    },
                );
            state.current_path = dragon.find_path(current_node, target_node, Some(egg));
            if let Some(path) = &mut state.current_path {
                path.advance();
            }
        }
        let State {
            current_path,
            target_location,
        } = &mut *state;
        navigate_to_next_path_node(current_path, target_location);
        let done = state.current_path.as_ref().is_some_and(Path::is_done);
        drop(state);
        if done {
            dragon.set_phase(EnderDragonPhase::Landing);
        }
    }
}

impl Phase for LandingApproachPhase {
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
}
