use super::{EnderDragonPhase, Phase};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Mutex;

/// The last stretch onto the podium, vanilla `DragonLandingPhase`.
#[derive(Default)]
pub struct LandingPhase {
    target_location: Mutex<Option<Vector3<f64>>>,
}

impl Phase for LandingPhase {
    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let target = *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(|| dragon.podium_bottom_center(true));
        if target.squared_distance_to_vec(&dragon.position()) < 1.0 {
            dragon.phase_manager.sitting_flaming.reset_flame_count();
            dragon.set_phase(EnderDragonPhase::SittingScanning);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn get_fly_speed(&self) -> f32 {
        1.5
    }

    fn get_turn_speed(&self, dragon: &EnderDragonEntity) -> f32 {
        let rot_speed = dragon.horizontal_speed() + 1.0;
        rot_speed.min(40.0) / rot_speed
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
