use super::{EnderDragonPhase, Phase, sitting_on_hurt};
use crate::entity::Entity;
use crate::entity::area_effect_cloud::{
    AreaEffectCloudEntity, DEFAULT_REAPPLICATION_DELAY, DEFAULT_WAIT_TIME, ParticleOptions,
};
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Mutex;

const FLAME_DURATION: i32 = 200;
const SITTING_FLAME_ATTACKS_COUNT: i32 = 4;
const WARMUP_TIME: i32 = 10;
const FLAME_RADIUS: f32 = 5.0;
const FLAME_POTION_DURATION_SCALE: f32 = 0.25;

#[derive(Default)]
struct State {
    flame_ticks: i32,
    flame_count: i32,
    flame: Option<i32>,
}

/// Perched and breathing a cloud in front of its head, vanilla `DragonSittingFlamingPhase`.
#[derive(Default)]
pub struct SittingFlamingPhase {
    state: Mutex<State>,
}

impl SittingFlamingPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn reset_flame_count(&self) {
        self.state().flame_count = 0;
    }

    fn spawn_flame(dragon: &EnderDragonEntity) -> i32 {
        let entity = &dragon.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        let head = dragon.head_pos();
        let look = Vector3::new(head.x - pos.x, 0.0, head.z - pos.z).vanilla_normalize();
        let x = head.x + look.x * f64::from(FLAME_RADIUS) / 2.0;
        let z = head.z + look.z * f64::from(FLAME_RADIUS) / 2.0;
        let initial_y = dragon.head_y(0.5);
        let mut y = initial_y;
        while world.get_block_state(&BlockPos::floored(x, y, z)).is_air() {
            y -= 1.0;
            if y < 0.0 {
                y = initial_y;
                break;
            }
        }
        let y = y.floor() + 1.0;

        let cloud = AreaEffectCloudEntity::create(
            Entity::new(
                world.clone(),
                Vector3::new(x, y, z),
                &EntityType::AREA_EFFECT_CLOUD,
            ),
            ItemStack::new(0, &Item::DRAGON_BREATH),
            vec![(&StatusEffect::INSTANT_DAMAGE, 0, 0, false, true, true)],
            FLAME_DURATION,
            FLAME_RADIUS,
            DEFAULT_REAPPLICATION_DELAY,
            DEFAULT_WAIT_TIME,
            0.0,
            0,
        );
        cloud.set_owner(Some(&dragon.mob_entity.living_entity));
        cloud.set_custom_particle(Some(ParticleOptions::Power(Particle::DragonBreath, 1.0)));
        // `create` shrinks the cloud like a lingering potion; vanilla's flame keeps its radius.
        *cloud
            .radius_on_tick
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = 0.0;
        *cloud
            .potion_duration_scale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = FLAME_POTION_DURATION_SCALE;
        let id = cloud.entity.entity_id;
        world.spawn_entity(cloud);
        id
    }
}

impl Phase for SittingFlamingPhase {
    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.flame_ticks += 1;
        if state.flame_ticks >= FLAME_DURATION {
            let next = if state.flame_count >= SITTING_FLAME_ATTACKS_COUNT {
                EnderDragonPhase::Takeoff
            } else {
                EnderDragonPhase::SittingScanning
            };
            drop(state);
            dragon.set_phase(next);
        } else if state.flame_ticks == WARMUP_TIME {
            drop(state);
            let flame = Self::spawn_flame(dragon);
            self.state().flame = Some(flame);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.flame_ticks = 0;
        state.flame_count += 1;
    }

    fn end(&self, dragon: &EnderDragonEntity) {
        if let Some(id) = self.state().flame.take()
            && let Some(flame) = dragon
                .mob_entity
                .living_entity
                .entity
                .world
                .load()
                .get_entity_by_id(id)
        {
            flame.get_entity().remove();
        }
    }

    fn is_sitting(&self) -> bool {
        true
    }

    fn on_hurt(&self, damage_type: DamageType, damage: f32) -> f32 {
        sitting_on_hurt(damage_type, damage)
    }
}
