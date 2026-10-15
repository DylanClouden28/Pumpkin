use std::sync::atomic::{AtomicBool, Ordering};

use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{
        Entity, EntityBase,
        projectile::{
            ProjectileHit, ThrownItemEntity,
            fireball::{
                AIR_INERTIA, INITIAL_ACCELERATION_POWER, WATER_INERTIA, fireball_damage_source,
            },
        },
    },
    server::Server,
};

const GRAVITY: f64 = 0.0;

pub struct SmallFireballEntity {
    pub thrown: ThrownItemEntity,
}

impl SmallFireballEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner_id: None,
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            left_owner: AtomicBool::new(false),
            gravity: GRAVITY,
        };

        Self { thrown }
    }

    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, GRAVITY);
        let accel = INITIAL_ACCELERATION_POWER;
        thrown
            .entity
            .velocity
            .store(direction.normalize().multiply(accel, accel, accel));
        Self { thrown }
    }
}

impl EntityBase for SmallFireballEntity {
    fn get_owner_id(&self) -> Option<i32> {
        self.thrown.owner_id
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.thrown.process_hurting_tick(
            caller,
            INITIAL_ACCELERATION_POWER,
            AIR_INERTIA,
            WATER_INERTIA,
        );
    }

    fn get_entity(&self) -> &Entity {
        self.thrown.get_entity()
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }
    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_hit(&self, hit: ProjectileHit) {
        match hit {
            ProjectileHit::Entity {
                ref entity,
                hit_pos,
                ..
            } => {
                // Vanilla puts the target's fire back when the hit is rejected.
                let target = entity.get_entity();
                let remaining_fire_ticks = target.fire_ticks.load(Ordering::Relaxed);
                target.set_on_fire_for(5.0);
                let owner = self.thrown.get_owner();
                let (damage_type, cause) =
                    fireball_damage_source(self.get_entity(), owner.as_deref());
                if !entity.damage_with_context(
                    entity.as_ref(),
                    5.0,
                    damage_type,
                    Some(hit_pos),
                    Some(self.get_entity()),
                    Some(cause),
                ) {
                    target
                        .fire_ticks
                        .store(remaining_fire_ticks, Ordering::Relaxed);
                }
            }
            ProjectileHit::Block { pos, face, .. } => {
                // Try to place fire
                let block_to_place = match face {
                    pumpkin_data::BlockDirection::Up => pos.up(),
                    pumpkin_data::BlockDirection::Down => pos.down(),
                    pumpkin_data::BlockDirection::North => pos.north(),
                    pumpkin_data::BlockDirection::South => pos.south(),
                    pumpkin_data::BlockDirection::West => pos.west(),
                    pumpkin_data::BlockDirection::East => pos.east(),
                };
                let world = self.get_entity().world.load();
                let fire_state = pumpkin_data::Block::FIRE.default_state.id;
                world.set_block_state(
                    &block_to_place,
                    fire_state,
                    pumpkin_world::world::BlockFlags::NOTIFY_ALL,
                );
            }
        }
    }
}
