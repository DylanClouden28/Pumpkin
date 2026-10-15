use super::{EnderDragonPhase, Phase, sitting_on_hurt};
use crate::entity::EntityBase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_data::damage::DamageType;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{atan2, wrap_degrees_f64};
use std::sync::atomic::{AtomicI32, Ordering};

const SITTING_SCANNING_IDLE_TICKS: i32 = 100;
const SITTING_ATTACK_Y_VIEW_RANGE: f64 = 10.0;
const SITTING_ATTACK_VIEW_RANGE: f64 = 20.0;
const SITTING_CHARGE_VIEW_RANGE: f64 = 150.0;

/// Perched and turning towards a nearby player, vanilla `DragonSittingScanningPhase`.
#[derive(Default)]
pub struct SittingScanningPhase {
    scanning_time: AtomicI32,
}

impl Phase for SittingScanningPhase {
    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let scanning_time = self.scanning_time.fetch_add(1, Ordering::Relaxed) + 1;
        let pos = dragon.position();
        let attack_target = dragon.nearest_combat_player(
            pos,
            Some(SITTING_ATTACK_VIEW_RANGE),
            Some(SITTING_ATTACK_Y_VIEW_RANGE),
            true,
        );

        if let Some(target) = attack_target {
            if scanning_time > 25 {
                dragon.set_phase(EnderDragonPhase::SittingAttacking);
                return;
            }
            let target_pos = target.get_entity().pos.load();
            let aim =
                Vector3::new(target_pos.x - pos.x, 0.0, target_pos.z - pos.z).vanilla_normalize();
            let angle = dragon.facing_angle_to(aim);
            if !(0.0..=10.0).contains(&angle) {
                let head = dragon.head_pos();
                let xd = target_pos.x - head.x;
                let zd = target_pos.z - head.z;
                let entity = &dragon.mob_entity.living_entity.entity;
                let yaw = entity.yaw.load();
                // Vanilla does this one in doubles.
                let y_rot_delta = wrap_degrees_f64(
                    180.0
                        - atan2(xd, zd) * 180.0 / f64::from(std::f32::consts::PI)
                        - f64::from(yaw),
                )
                .clamp(-100.0, 100.0) as f32;
                let rot_speed = Vector3::new(xd, 0.0, zd).horizontal_length() as f32 + 1.0;
                let dist = rot_speed.min(40.0);
                let y_rot_a =
                    dragon.yaw_rot_accel.load() * 0.8 + y_rot_delta * (0.7 / dist / rot_speed);
                dragon.yaw_rot_accel.store(y_rot_a);
                entity.yaw.store(yaw + y_rot_a);
            }
        } else if scanning_time >= SITTING_SCANNING_IDLE_TICKS {
            let charge_target =
                dragon.nearest_combat_player(pos, Some(SITTING_CHARGE_VIEW_RANGE), None, true);
            // Vanilla sets takeoff first, so its `begin` runs before the charge replaces it.
            dragon.set_phase(EnderDragonPhase::Takeoff);
            if let Some(target) = charge_target {
                dragon.set_phase(EnderDragonPhase::ChargingPlayer);
                dragon
                    .phase_manager
                    .charging_player
                    .set_target(target.get_entity().pos.load());
            }
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        self.scanning_time.store(0, Ordering::Relaxed);
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn on_hurt(&self, damage_type: DamageType, damage: f32) -> f32 {
        sitting_on_hurt(damage_type, damage)
    }
}
