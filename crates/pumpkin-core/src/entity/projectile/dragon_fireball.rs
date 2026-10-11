use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::area_effect_cloud::{
    AreaEffectCloudEntity, DEFAULT_REAPPLICATION_DELAY, DEFAULT_WAIT_TIME, ParticleOptions,
};
use crate::entity::projectile::fireball::{INITIAL_ACCELERATION_POWER, apply_inertia};
use crate::entity::projectile::{ProjectileHit, ThrownItemEntity};
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::server::Server;

/// Vanilla `DragonFireball.SPLASH_RANGE`.
const SPLASH_RANGE: f64 = 4.0;
const CLOUD_RADIUS: f32 = 3.0;
const CLOUD_MAX_RADIUS: f32 = 7.0;
const CLOUD_DURATION: i32 = 600;
const CLOUD_POTION_DURATION_SCALE: f32 = 0.25;

/// The fireball the ender dragon shoots while strafing, vanilla `DragonFireball`.
pub struct DragonFireballEntity {
    pub thrown: ThrownItemEntity,
}

impl DragonFireballEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        Self {
            thrown: ThrownItemEntity {
                entity,
                owner_id: None,
                collides_with_projectiles: false,
                has_hit: AtomicBool::new(false),
                gravity: 0.0,
            },
        }
    }

    /// Vanilla `AbstractHurtingProjectile(type, owner, direction, level)` followed by `snapTo(pos)`.
    #[must_use]
    pub fn new_shot(
        entity: Entity,
        shooter: &Entity,
        pos: Vector3<f64>,
        direction: Vector3<f64>,
    ) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, 0.0);
        thrown.entity.pos.store(pos);
        let accel = INITIAL_ACCELERATION_POWER;
        thrown
            .entity
            .velocity
            .store(direction.normalize().multiply(accel, accel, accel));
        Self { thrown }
    }

    fn inside_owner(&self) -> bool {
        let entity = self.get_entity();
        let Some(owner) = self
            .thrown
            .owner_id
            .and_then(|id| entity.world.load().get_entity_by_id(id))
        else {
            return false;
        };
        owner
            .get_entity()
            .bounding_box
            .load()
            .intersects(&entity.bounding_box.load())
    }

    /// Vanilla `DragonFireball.onHit`. Pumpkin has already moved the fireball the full step, so
    /// `hit_pos` stands in for its position.
    fn splash(&self, hit_pos: Vector3<f64>) {
        let entity = self.get_entity();
        let world = entity.world.load();

        let search = BoundingBox::new_from_pos(
            hit_pos.x,
            hit_pos.y,
            hit_pos.z,
            &entity.entity_dimension.load(),
        )
        .expand(SPLASH_RANGE, 2.0, SPLASH_RANGE);
        let cloud_pos = world
            .get_all_at_box(&search)
            .into_iter()
            .filter(|e| e.get_living_entity().is_some() && !e.is_spectator())
            .map(|e| e.get_entity().pos.load())
            .find(|pos| pos.squared_distance_to_vec(&hit_pos) < SPLASH_RANGE * SPLASH_RANGE)
            .unwrap_or(hit_pos);

        let cloud = AreaEffectCloudEntity::create(
            Entity::new(world.clone(), cloud_pos, &EntityType::AREA_EFFECT_CLOUD),
            ItemStack::new(0, &Item::DRAGON_BREATH),
            vec![(&StatusEffect::INSTANT_DAMAGE, 1, 1, false, true, true)],
            CLOUD_DURATION,
            CLOUD_RADIUS,
            DEFAULT_REAPPLICATION_DELAY,
            DEFAULT_WAIT_TIME,
            0.0,
            0,
        );
        let owner = self
            .thrown
            .owner_id
            .and_then(|id| world.get_entity_by_id(id));
        cloud.set_owner(owner.as_ref().and_then(|owner| owner.get_living_entity()));
        cloud.set_custom_particle(Some(ParticleOptions::Power(Particle::DragonBreath, 1.0)));
        *cloud
            .radius_on_tick
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            (CLOUD_MAX_RADIUS - CLOUD_RADIUS) / CLOUD_DURATION as f32;
        *cloud
            .potion_duration_scale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = CLOUD_POTION_DURATION_SCALE;

        let data = if entity.is_silent() { -1 } else { 1 };
        world.sync_world_event(
            WorldEvent::ParticlesDragonFireballSplash,
            BlockPos::floored_v(hit_pos),
            data,
        );
        world.spawn_entity(cloud);
    }
}

impl EntityBase for DragonFireballEntity {
    fn get_owner_id(&self) -> Option<i32> {
        self.thrown.owner_id
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        let entity = self.get_entity();
        let velocity = apply_inertia(entity, INITIAL_ACCELERATION_POWER);

        // Pumpkin's hit test ignores vanilla `isPickable`, so skip hits inside the dragon.
        if self.inside_owner() {
            entity.update_last_pos();
            entity.set_pos(entity.pos.load().add(&velocity));
            return;
        }
        self.thrown.process_tick(caller);
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        // Vanilla only bursts when the hit isn't its own dragon.
        if let ProjectileHit::Entity { entity, .. } = &hit
            && Some(entity.get_entity().entity_id) == self.thrown.owner_id
        {
            return;
        }
        self.splash(hit.hit_pos());
    }
}

/// Spawns a dragon fireball like vanilla `DragonStrafePlayerPhase` does.
pub fn shoot(shooter: &Entity, pos: Vector3<f64>, direction: Vector3<f64>) {
    let world = shooter.world.load();
    let base = Entity::new(world.clone(), pos, &EntityType::DRAGON_FIREBALL);
    let fireball = DragonFireballEntity::new_shot(base, shooter, pos, direction);
    world.spawn_entity(Arc::new(fireball));
}
