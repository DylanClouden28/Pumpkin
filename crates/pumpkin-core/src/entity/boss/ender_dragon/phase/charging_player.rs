use super::{EnderDragonPhase, Phase, TARGET_DIST_SQR_RANGE};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Mutex;

const CHARGE_RECOVERY_TIME: i32 = 10;

#[derive(Default)]
struct State {
    target_location: Option<Vector3<f64>>,
    time_since_charge: i32,
}

/// Diving at a player after taking off, vanilla `DragonChargePlayerPhase`.
#[derive(Default)]
pub struct ChargingPlayerPhase {
    state: Mutex<State>,
}

impl ChargingPlayerPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_target(&self, target: Vector3<f64>) {
        self.state().target_location = Some(target);
    }
}

impl Phase for ChargingPlayerPhase {
    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        let Some(target) = state.target_location else {
            drop(state);
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
            return;
        };
        // Vanilla `timeSinceCharge > 0 && timeSinceCharge++ >= 10` falls through when not done.
        if state.time_since_charge > 0 {
            let done = state.time_since_charge >= CHARGE_RECOVERY_TIME;
            state.time_since_charge += 1;
            if done {
                drop(state);
                dragon.set_phase(EnderDragonPhase::HoldingPattern);
                return;
            }
        }
        let dist = target.squared_distance_to_vec(&dragon.position());
        if !TARGET_DIST_SQR_RANGE.contains(&dist) || dragon.collided() {
            state.time_since_charge += 1;
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        *self.state() = State::default();
    }

    fn get_fly_speed(&self) -> f32 {
        3.0
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        self.state().target_location
    }
}
