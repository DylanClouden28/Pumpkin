use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::entity::boss::ender_dragon::EnderDragonEntity;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use pumpkin_data::{
    damage::DamageType,
    tag::{self, Taggable},
};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

pub struct EndCrystalEntity {
    entity: Entity,
    beam_target: Mutex<Option<BlockPos>>,
    show_bottom: AtomicBool,
    invulnerable: AtomicBool,
}

impl EndCrystalEntity {
    #[must_use]
    pub const fn new(entity: Entity) -> Self {
        Self {
            entity,
            beam_target: Mutex::new(None),
            show_bottom: AtomicBool::new(false),
            invulnerable: AtomicBool::new(false),
        }
    }

    pub fn set_show_bottom(&self, show_bottom: bool) {
        self.show_bottom.store(show_bottom, Ordering::Relaxed);
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::end_crystal::SHOW_BOTTOM,
            show_bottom,
        );
    }

    pub fn show_bottom(&self) -> bool {
        self.show_bottom.load(Ordering::Relaxed)
    }

    pub fn set_beam_target(&self, beam_target: Option<BlockPos>) {
        *self
            .beam_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = beam_target;
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::end_crystal::BEAM_TARGET,
            beam_target,
        );
    }

    pub fn beam_target(&self) -> Option<BlockPos> {
        *self
            .beam_target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_invulnerable(&self, invulnerable: bool) {
        self.invulnerable.store(invulnerable, Ordering::Relaxed);
    }

    pub fn is_invulnerable(&self) -> bool {
        self.invulnerable.load(Ordering::Relaxed)
    }
}

impl EntityBase for EndCrystalEntity {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.is_invulnerable() {
            return false;
        }
        // Vanilla destroys a crystal once, so its own explosion doesn't hurt the dragon again.
        if self.entity.is_removed() {
            return true;
        }

        self.entity.remove();
        let world = self.entity.world.load();
        if !damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION) {
            let pos = self.entity.pos.load();
            world.explode(pos, 6.0, crate::world::ExplosionInteraction::Block);
        }

        let dragon_uuid = world.dragon_fight.as_ref().and_then(|fight_mutex| {
            fight_mutex
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .on_crystal_destroyed(&world, self.entity.entity_uuid)
        });
        if let Some(dragon_uuid) = dragon_uuid
            && let Some(dragon) = world.get_entity_by_uuid(dragon_uuid)
            && let Some(dragon) = dragon.cast_any().downcast_ref::<EnderDragonEntity>()
        {
            let attacker = cause
                .or(source)
                .and_then(|e| world.get_player_by_id(e.get_entity().entity_id));
            dragon.on_crystal_destroyed(self, self.entity.block_pos.load().0.to_f64(), attacker);
        }

        true
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
