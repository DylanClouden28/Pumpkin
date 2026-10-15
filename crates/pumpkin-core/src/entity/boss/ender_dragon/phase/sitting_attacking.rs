use super::{EnderDragonPhase, Phase, sitting_on_hurt};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_data::damage::DamageType;
use std::sync::atomic::{AtomicI32, Ordering};

/// The growl lasts this long; the client plays it from the synced phase.
const ROAR_DURATION: i32 = 40;

/// Perched and roaring at a player, vanilla `DragonSittingAttackingPhase`.
#[derive(Default)]
pub struct SittingAttackingPhase {
    attacking_ticks: AtomicI32,
}

impl Phase for SittingAttackingPhase {
    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        if self.attacking_ticks.fetch_add(1, Ordering::Relaxed) >= ROAR_DURATION {
            dragon.set_phase(EnderDragonPhase::SittingFlaming);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        self.attacking_ticks.store(0, Ordering::Relaxed);
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn on_hurt(&self, damage_type: DamageType, damage: f32) -> f32 {
        sitting_on_hurt(damage_type, damage)
    }
}
