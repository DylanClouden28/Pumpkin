use super::{Phase, TARGET_DIST_SQR_RANGE};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Mutex;

/// Vanilla `DragonDeathPhase`; the death itself is `EnderDragonEntity::tick_death`.
#[derive(Default)]
pub struct DyingPhase {
    target_location: Mutex<Option<Vector3<f64>>>,
}

impl Phase for DyingPhase {
    fn begin(&self, _dragon: &EnderDragonEntity) {
        *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn get_fly_speed(&self) -> f32 {
        3.0
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let target = *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(|| dragon.podium_bottom_center(false));
        let dist = target.squared_distance_to_vec(&dragon.position());
        let living = &dragon.mob_entity.living_entity;
        if TARGET_DIST_SQR_RANGE.contains(&dist) && !dragon.collided() {
            living.set_health(1.0);
        } else {
            living.set_health(0.0);
        }
    }
}
