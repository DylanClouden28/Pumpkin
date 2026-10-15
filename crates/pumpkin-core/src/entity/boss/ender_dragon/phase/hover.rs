use super::Phase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Mutex;

/// Holding still where it is, vanilla `DragonHoverPhase`. Dragons outside a fight stay here.
#[derive(Default)]
pub struct HoverPhase {
    target_location: Mutex<Option<Vector3<f64>>>,
}

impl Phase for HoverPhase {
    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        self.target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(|| dragon.position());
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn get_fly_speed(&self) -> f32 {
        1.0
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
