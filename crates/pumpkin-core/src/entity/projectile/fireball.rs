use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_util::math::atomic_f32::AtomicF32;
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{
        Entity, EntityBase,
        projectile::{ProjectileHit, ThrownItemEntity},
    },
    server::Server,
};

pub const MIN_CAMERA_DISTANCE_SQUARED: f32 = 12.25;
pub const INITIAL_ACCELERATION_POWER: f64 = 0.1;
pub const DEFLECTION_SCALE: f64 = 0.5;
pub const DEFAULT_EXPLOSION_POWER: f32 = 1.0;
pub const AIR_INERTIA: f32 = 0.95;
pub const WATER_INERTIA: f32 = 0.8;

/// Vanilla `DamageSources.fireball`: the damage type and cause of a fireball hit. An ownerless
/// fireball is its own cause.
#[must_use]
pub fn fireball_damage_source<'a>(
    fireball: &'a dyn EntityBase,
    owner: Option<&'a dyn EntityBase>,
) -> (DamageType, &'a dyn EntityBase) {
    owner.map_or((DamageType::UNATTRIBUTED_FIREBALL, fireball), |owner| {
        (DamageType::FIREBALL, owner)
    })
}

pub struct FireballEntity {
    pub thrown: ThrownItemEntity,
    pub item_stack: RwLock<ItemStack>,
    pub acceleration_power: AtomicU64,
    pub explosion_power: AtomicF32,
}

impl FireballEntity {
    #[must_use]
    pub fn new(entity: Entity) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner_id: None,
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            left_owner: AtomicBool::new(false),
            gravity: 0.0,
        };

        Self {
            thrown,
            item_stack: RwLock::new(Self::get_default_item()),
            acceleration_power: AtomicU64::new(INITIAL_ACCELERATION_POWER.to_bits()),
            explosion_power: AtomicF32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    #[must_use]
    pub fn new_shot(entity: Entity, shooter: &Entity, direction: Vector3<f64>) -> Self {
        let thrown = ThrownItemEntity::new(entity, shooter, 0.0);
        let accel = INITIAL_ACCELERATION_POWER;
        let vel = direction.normalize().multiply(accel, accel, accel);
        thrown.entity.velocity.store(vel);

        Self {
            thrown,
            item_stack: RwLock::new(Self::get_default_item()),
            acceleration_power: AtomicU64::new(accel.to_bits()),
            explosion_power: AtomicF32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    #[must_use]
    pub fn new_directional(
        entity: Entity,
        direction: Vector3<f64>,
        acceleration_power: f64,
    ) -> Self {
        let thrown = ThrownItemEntity {
            entity,
            owner_id: None,
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            left_owner: AtomicBool::new(false),
            gravity: 0.0,
        };
        let vel = direction.normalize().multiply(
            acceleration_power,
            acceleration_power,
            acceleration_power,
        );
        thrown.entity.velocity.store(vel);

        Self {
            thrown,
            item_stack: RwLock::new(Self::get_default_item()),
            acceleration_power: AtomicU64::new(acceleration_power.to_bits()),
            explosion_power: AtomicF32::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    #[must_use]
    pub fn get_default_item() -> ItemStack {
        ItemStack::new(1, &Item::FIRE_CHARGE)
    }

    pub fn get_item(&self) -> ItemStack {
        self.item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_item(&self, source: ItemStack) {
        let new_item = if source.item_count == 0 {
            Self::get_default_item()
        } else {
            let mut item = source;
            item.item_count = 1;
            item
        };
        *self
            .item_stack
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = new_item.clone();

        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::fireball::ITEM_STACK,
            ItemStackSerializer::from(new_item),
        );
    }

    pub fn get_acceleration_power(&self) -> f64 {
        f64::from_bits(self.acceleration_power.load(Ordering::Relaxed))
    }

    pub fn set_acceleration_power(&self, power: f64) {
        self.acceleration_power
            .store(power.to_bits(), Ordering::Relaxed);
    }

    pub fn get_explosion_power(&self) -> f32 {
        self.explosion_power.load(Ordering::Relaxed)
    }

    pub fn set_explosion_power(&self, power: f32) {
        self.explosion_power.store(power, Ordering::Relaxed);
    }

    pub fn on_deflection(&self, by_attack: bool) {
        if by_attack {
            self.set_acceleration_power(INITIAL_ACCELERATION_POWER);
        } else {
            let current = self.get_acceleration_power();
            self.set_acceleration_power(current * DEFLECTION_SCALE);
        }
    }

    pub fn should_render_at_sqr_distance(&self, distance_sqr: f64) -> bool {
        if self.get_entity().age.load(Ordering::Relaxed) < 2
            && distance_sqr < f64::from(MIN_CAMERA_DISTANCE_SQUARED)
        {
            false
        } else {
            let bb_size = self
                .get_entity()
                .bounding_box
                .load()
                .get_average_side_length()
                * 4.0;
            let size = if bb_size.is_nan() { 4.0 } else { bb_size } * 64.0;
            distance_sqr < size * size
        }
    }
}

impl EntityBase for FireballEntity {
    fn get_owner_id(&self) -> Option<i32> {
        self.thrown.owner_id
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_double("acceleration_power", self.get_acceleration_power());
        nbt.put_float("ExplosionPower", self.get_explosion_power());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        if let Some(accel) = nbt
            .get_double("acceleration_power")
            .or_else(|| nbt.get_double("power"))
        {
            self.set_acceleration_power(accel);
        }
        if let Some(exp) = nbt.get_float("ExplosionPower") {
            self.set_explosion_power(exp);
        }
    }

    fn init_data_tracker(&self) {
        let entity = self.get_entity();
        let stack = self
            .item_stack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        entity.set_synced_data(
            pumpkin_data::tracked_data::fireball::ITEM_STACK,
            ItemStackSerializer::from(stack.clone()),
        );
    }

    fn tick(&self, caller: &dyn EntityBase, _server: &Server) {
        self.thrown.process_hurting_tick(
            caller,
            self.get_acceleration_power(),
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
        let world = self.get_entity().world.load();
        let hit_pos = hit.hit_pos();

        if let ProjectileHit::Entity { ref entity, .. } = hit {
            let owner = self.thrown.get_owner();
            let (damage_type, cause) = fireball_damage_source(self.get_entity(), owner.as_deref());
            let _ = entity.damage_with_context(
                entity.as_ref(),
                6.0,
                damage_type,
                Some(hit_pos),
                Some(self.get_entity()),
                Some(cause),
            );
        }

        let power = self.get_explosion_power();
        world.explode(hit_pos, power, crate::world::ExplosionInteraction::Mob);
    }
}
