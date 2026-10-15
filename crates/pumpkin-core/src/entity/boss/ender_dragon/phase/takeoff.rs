use super::{EnderDragonPhase, Phase, navigate_to_next_path_node, wrap_node_index};
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Mutex;

/// The dragon leaves the podium once it is this far from it, vanilla `closerToCenterThan(.., 10)`.
const LEAVE_PODIUM_DIST: f64 = 10.0;

struct State {
    first_tick: bool,
    current_path: Option<Path>,
    target_location: Option<Vector3<f64>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            first_tick: true,
            current_path: None,
            target_location: None,
        }
    }
}

/// Leaving the podium, vanilla `DragonTakeoffPhase`.
#[derive(Default)]
pub struct TakeoffPhase {
    state: Mutex<State>,
}

impl TakeoffPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn find_new_target(dragon: &EnderDragonEntity, state: &mut State) {
        let current_node = dragon.find_closest_node();
        let look = dragon.head_look_vector();
        let target_node =
            dragon.find_closest_node_to(Vector3::new(-look.x * 40.0, 105.0, -look.z * 40.0)) as i32;
        let outer_ring = dragon.alive_crystals().is_some_and(|c| c > 0);
        let target_node = wrap_node_index(target_node, outer_ring);
        state.current_path = dragon.find_path(current_node, target_node, None);
        if let Some(path) = &mut state.current_path {
            path.advance();
        }
        navigate_to_next_path_node(&mut state.current_path, &mut state.target_location);
    }
}

impl Phase for TakeoffPhase {
    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        if !state.first_tick && state.current_path.is_some() {
            drop(state);
            let egg = dragon.podium_pos(true).to_centered_f64();
            if egg.squared_distance_to_vec(&dragon.position())
                >= LEAVE_PODIUM_DIST * LEAVE_PODIUM_DIST
            {
                dragon.set_phase(EnderDragonPhase::HoldingPattern);
            }
        } else {
            state.first_tick = false;
            Self::find_new_target(dragon, &mut state);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        *self.state() = State::default();
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        self.state().target_location
    }
}
