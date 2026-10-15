use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, PoisonError};

use crate::{
    entity::{Entity, EntityBase, mob::neutral::find_by_uuid},
    item::potion::PotionContents,
    server::Server,
};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{
    DataComponentImpl, PotionContentsImpl, PotionDurationScaleImpl, StatusEffectInstance,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::particle::Particle;
use pumpkin_data::potion::Effect;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data::area_effect_cloud::{PARTICLE, RADIUS, WAITING};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::MetadataSerializer;
use pumpkin_protocol::ser::{NetworkWriteExt, WritingError};
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::version::JavaMinecraftVersion;
use uuid::Uuid;

type EffectEntry = (&'static StatusEffect, i32, u8, bool, bool, bool);
use pumpkin_data::item_stack::ItemStack;

const TIME_BETWEEN_APPLICATIONS: i32 = 5;
const MAX_RADIUS: f32 = 32.0;
const DEFAULT_POTION_DURATION_SCALE: f32 = 1.0;
const MINIMAL_RADIUS: f32 = 0.5;
const DEFAULT_RADIUS: f32 = 3.0;
pub const INFINITE_DURATION: i32 = -1;
pub const DEFAULT_WAIT_TIME: i32 = 20;
pub const DEFAULT_REAPPLICATION_DELAY: i32 = 20;

/// Vanilla `ParticleOptions`, limited to the two kinds a cloud uses: the potion coloured
/// `ColorParticleOption` and the `PowerParticleOption` of dragon breath.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ParticleOptions {
    Color(Particle, i32),
    Power(Particle, f32),
}

impl ParticleOptions {
    const fn particle(self) -> Particle {
        match self {
            Self::Color(particle, _) | Self::Power(particle, _) => particle,
        }
    }

    /// Vanilla `ParticleTypes.CODEC`; other option types are not modelled and read as `None`.
    fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        let name = nbt.get_string("type")?;
        let particle = Particle::from_name(name.strip_prefix("minecraft:").unwrap_or(name))?;
        match particle {
            Particle::EntityEffect | Particle::TintedLeaves | Particle::Flash => {
                Some(Self::Color(particle, nbt.get_int("color")?))
            }
            Particle::DragonBreath => {
                Some(Self::Power(particle, nbt.get_float("power").unwrap_or(1.0)))
            }
            _ => None,
        }
    }

    fn to_nbt(self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.put_string("type", format!("minecraft:{}", self.particle().to_name()));
        match self {
            Self::Color(_, color) => nbt.put_int("color", color),
            Self::Power(_, power) => nbt.put_float("power", power),
        }
        nbt
    }
}

impl MetadataSerializer for ParticleOptions {
    fn write_metadata(
        &self,
        writer: &mut impl std::io::Write,
        _version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        writer.write_var_int(&VarInt(i32::from(self.particle().to_id())))?;
        match *self {
            Self::Color(_, color) => writer.write_i32_be(color),
            Self::Power(_, power) => writer.write_f32_be(power),
        }
    }
}

/// The effect cloud entity that is spawned where a lingering potion lands.
pub struct AreaEffectCloudEntity {
    pub entity: Entity,
    /// Stored potion item stack (may be empty) to read effects from.
    pub item_stack: Mutex<ItemStack>,
    /// Active potion effects as tuples: (`StatusEffect`, `duration_ticks`, `amplifier`, `ambient`, `show_particles`, `show_icon`)
    pub effects: Mutex<Vec<EffectEntry>>,
    pub radius: Mutex<f32>,
    pub duration: Mutex<i32>,
    /// ticks between reapplications to the same entity
    pub reapplication_delay: Mutex<i32>,
    /// map of `entity_id` -> age at which that entity can be affected again
    pub reapplication_map: Mutex<HashMap<i32, i32>>,
    /// linear radius change per tick
    pub radius_on_tick: Mutex<f32>,
    /// radius change when an entity is affected
    pub radius_on_use: Mutex<f32>,
    /// duration change (ticks) when an entity is affected
    pub duration_on_use: Mutex<i32>,
    /// ticks to wait before the cloud becomes active and applies effects (grace period)
    pub wait_time: Mutex<i32>,
    pub potion_duration_scale: Mutex<f32>,
    owner: AtomicCell<Option<Uuid>>,
    custom_particle: Mutex<Option<ParticleOptions>>,
}

impl AreaEffectCloudEntity {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(entity: Entity) -> Arc<dyn EntityBase> {
        entity.no_physics.store(true, Relaxed);
        let cloud = Self {
            entity,
            item_stack: Mutex::new(ItemStack::new(0, &pumpkin_data::item::Item::GLASS_BOTTLE)),
            effects: Mutex::new(Vec::new()),
            radius: Mutex::new(DEFAULT_RADIUS),
            duration: Mutex::new(INFINITE_DURATION),
            reapplication_delay: Mutex::new(DEFAULT_REAPPLICATION_DELAY),
            reapplication_map: Mutex::new(HashMap::new()),
            radius_on_tick: Mutex::new(0.0),
            radius_on_use: Mutex::new(0.0),
            duration_on_use: Mutex::new(0),
            wait_time: Mutex::new(DEFAULT_WAIT_TIME),
            potion_duration_scale: Mutex::new(DEFAULT_POTION_DURATION_SCALE),
            owner: AtomicCell::new(None),
            custom_particle: Mutex::new(None),
        };

        Arc::new(cloud)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        entity: Entity,
        item_stack: ItemStack,
        effects_in: Vec<EffectEntry>,
        duration_in: i32,
        radius_in: f32,
        reapplication_delay_in: i32,
        wait_time_in: i32,
        radius_on_use_in: f32,
        duration_on_use_in: i32,
    ) -> Arc<Self> {
        entity.no_physics.store(true, Relaxed);
        // Vanilla `applyComponentsFromItemStack`: lingering potions carry their duration scale.
        let potion_duration_scale = item_stack
            .get_data_component::<PotionDurationScaleImpl>()
            .map_or(DEFAULT_POTION_DURATION_SCALE, |scale| scale.scale);
        let cloud = Self {
            entity,
            item_stack: Mutex::new(item_stack),
            effects: Mutex::new(effects_in),
            radius: Mutex::new(radius_in),
            duration: Mutex::new(duration_in),
            reapplication_delay: Mutex::new(reapplication_delay_in),
            reapplication_map: Mutex::new(HashMap::new()),
            radius_on_tick: Mutex::new(-radius_in / (duration_in as f32).max(1.0)),
            radius_on_use: Mutex::new(radius_on_use_in),
            duration_on_use: Mutex::new(duration_on_use_in),
            wait_time: Mutex::new(wait_time_in),
            potion_duration_scale: Mutex::new(potion_duration_scale),
            owner: AtomicCell::new(None),
            custom_particle: Mutex::new(None),
        };
        cloud.set_radius(radius_in);

        Arc::new(cloud)
    }

    #[must_use]
    pub fn get_radius(&self) -> f32 {
        *self.radius.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Vanilla `setRadius`; the box follows the radius like vanilla `refreshDimensions`.
    pub fn set_radius(&self, radius: f32) {
        let radius = radius.clamp(0.0, MAX_RADIUS);
        *self.radius.lock().unwrap_or_else(PoisonError::into_inner) = radius;
        self.entity.set_synced_data(RADIUS, radius);

        let dimensions = EntityDimensions {
            width: radius * 2.0,
            ..Entity::type_dimensions(self.entity.entity_type)
        };
        let pos = self.entity.pos.load();
        self.entity.entity_dimension.store(dimensions);
        self.entity
            .bounding_box
            .store(BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &dimensions));
    }

    pub fn set_owner(&self, owner: Option<&crate::entity::living::LivingEntity>) {
        self.owner
            .store(owner.map(|owner| owner.entity.entity_uuid));
    }

    #[must_use]
    pub fn get_owner(&self) -> Option<Arc<dyn EntityBase>> {
        let world = self.entity.world.load();
        find_by_uuid(&world, self.owner.load()?).filter(|owner| owner.get_living_entity().is_some())
    }

    pub fn set_custom_particle(&self, custom_particle: Option<ParticleOptions>) {
        *self
            .custom_particle
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = custom_particle;
        self.update_particle();
    }

    fn update_particle(&self) {
        let custom_particle = *self
            .custom_particle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let particle = custom_particle
            .unwrap_or_else(|| ParticleOptions::Color(Particle::EntityEffect, self.potion_color()));
        self.entity.set_synced_data(PARTICLE, particle);
    }

    /// The potion colour as opaque ARGB.
    fn potion_color(&self) -> i32 {
        let custom_color = self
            .item_stack
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_data_component::<PotionContentsImpl>()
            .and_then(|contents| contents.custom_color);
        let color = custom_color.unwrap_or_else(|| {
            let effects = self.effects.lock().unwrap_or_else(PoisonError::into_inner);
            if effects.is_empty() {
                // default water-like color
                return 0x385dc6;
            }
            let mut r_sum = 0.0f32;
            let mut g_sum = 0.0f32;
            let mut b_sum = 0.0f32;
            for (eff, ..) in &*effects {
                let c = eff.color;
                r_sum += ((c >> 16) & 0xFF) as f32;
                g_sum += ((c >> 8) & 0xFF) as f32;
                b_sum += (c & 0xFF) as f32;
            }
            let count = effects.len() as f32;
            let r = (r_sum / count) as i32;
            let g = (g_sum / count) as i32;
            let b = (b_sum / count) as i32;
            (r << 16) | (g << 8) | b
        });
        (0xFFi32 << 24) | color
    }

    /// Vanilla `potionContents`: the potion's own contents, or the effects added directly.
    fn potion_contents(&self) -> Option<PotionContentsImpl> {
        let stack = self
            .item_stack
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(contents) = stack.get_data_component::<PotionContentsImpl>() {
            return Some(contents.clone());
        }
        drop(stack);
        let effects = self.effects.lock().unwrap_or_else(PoisonError::into_inner);
        (!effects.is_empty()).then(|| PotionContentsImpl {
            potion_id: None,
            custom_color: None,
            custom_effects: effects
                .iter()
                .map(
                    |&(effect, duration, amplifier, ambient, show_particles, show_icon)| {
                        StatusEffectInstance {
                            effect_id: effect.minecraft_name.into(),
                            amplifier: i32::from(amplifier),
                            duration,
                            ambient,
                            show_particles,
                            show_icon,
                        }
                    },
                )
                .collect(),
            custom_name: None,
        })
    }
}

/// Vanilla `HealOrHarmMobEffect.applyInstantaneousEffect`.
fn apply_instantaneous_effect(
    effect: &'static StatusEffect,
    source: &dyn EntityBase,
    owner: Option<&dyn EntityBase>,
    target: &dyn EntityBase,
    amplifier: u8,
    scale: f64,
) {
    let Some(living) = target.get_living_entity() else {
        return;
    };
    let is_harm = effect == &StatusEffect::INSTANT_DAMAGE;
    let inverted = target
        .get_entity()
        .entity_type
        .has_tag(&tag::EntityType::MINECRAFT_INVERTED_HEALING_AND_HARM);
    if is_harm == inverted {
        let amount = (scale * f64::from(4i32.wrapping_shl(u32::from(amplifier))) + 0.5) as i32;
        // Pumpkin's `heal` rejects what vanilla's ignores.
        if amount > 0 {
            living.heal(amount as f32);
        }
    } else {
        let amount = (scale * f64::from(6i32.wrapping_shl(u32::from(amplifier))) + 0.5) as i32;
        target.damage_with_context(
            target,
            amount as f32,
            DamageType::INDIRECT_MAGIC,
            Some(source.get_entity().pos.load()),
            Some(source),
            owner,
        );
    }
}

impl EntityBase for AreaEffectCloudEntity {
    fn init_data_tracker(&self) {
        self.update_particle();
    }

    #[allow(clippy::too_many_lines)]
    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {
        let age = self.entity.age.load(Relaxed);
        let wait_time = *self
            .wait_time
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let duration = *self.duration.lock().unwrap_or_else(PoisonError::into_inner);
        if duration != INFINITE_DURATION && age - wait_time >= duration {
            self.entity.remove();
            return;
        }

        let should_wait = age < wait_time;
        self.entity.set_synced_data(WAITING, should_wait);
        if should_wait {
            return;
        }

        let mut radius = self.get_radius();
        let radius_per_tick = *self
            .radius_on_tick
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if radius_per_tick != 0.0 {
            radius += radius_per_tick;
            if radius < MINIMAL_RADIUS {
                self.entity.remove();
                return;
            }
            self.set_radius(radius);
        }

        if age % TIME_BETWEEN_APPLICATIONS != 0 {
            return;
        }

        let effects = self
            .effects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        {
            let mut map = self
                .reapplication_map
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            map.retain(|_, until| age < *until);
            if effects.is_empty() {
                map.clear();
                return;
            }
        }

        let duration_scale = *self
            .potion_duration_scale
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let delay = *self
            .reapplication_delay
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let radius_on_use = *self
            .radius_on_use
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let duration_on_use = *self
            .duration_on_use
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let pos = self.entity.pos.load();
        let owner = self.get_owner();
        let world = self.entity.world.load();

        for cand in world.get_all_at_box(&self.entity.bounding_box.load()) {
            let Some(living) = cand.get_living_entity() else {
                continue;
            };
            let ent_id = cand.get_entity().entity_id;
            // Vanilla `isAffectedByPotions`; spectators are left out by the entity query.
            if cand.is_spectator()
                || living.health.load() <= 0.0
                || self
                    .reapplication_map
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains_key(&ent_id)
            {
                continue;
            }

            let pos_e = cand.get_entity().pos.load();
            let dx = pos_e.x - pos.x;
            let dz = pos_e.z - pos.z;
            if dx * dx + dz * dz > f64::from(radius * radius) {
                continue;
            }

            self.reapplication_map
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(ent_id, age + delay);

            for &(effect, duration, amplifier, ambient, show_particles, show_icon) in &effects {
                if effect == &StatusEffect::INSTANT_DAMAGE
                    || effect == &StatusEffect::INSTANT_HEALTH
                {
                    apply_instantaneous_effect(
                        effect,
                        self,
                        owner.as_deref(),
                        cand.as_ref(),
                        amplifier,
                        0.5,
                    );
                } else {
                    // Vanilla `MobEffectInstance.withScaledDuration`.
                    let duration = if duration == INFINITE_DURATION || duration == 0 {
                        duration
                    } else {
                        ((duration as f32 * duration_scale).floor() as i32).max(1)
                    };
                    living.add_effect(Effect {
                        effect_type: effect,
                        duration,
                        amplifier,
                        ambient,
                        show_particles,
                        show_icon,
                        blend: false,
                    });
                }
            }

            if radius_on_use != 0.0 {
                radius += radius_on_use;
                if radius < MINIMAL_RADIUS {
                    self.entity.remove();
                    return;
                }
                self.set_radius(radius);
            }

            if duration_on_use != 0 {
                let mut duration_lock =
                    self.duration.lock().unwrap_or_else(PoisonError::into_inner);
                if *duration_lock != INFINITE_DURATION {
                    *duration_lock += duration_on_use;
                    if *duration_lock <= 0 {
                        drop(duration_lock);
                        self.entity.remove();
                        return;
                    }
                }
            }
        }
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Age", self.entity.age.load(Relaxed));
        nbt.put_int(
            "Duration",
            *self.duration.lock().unwrap_or_else(PoisonError::into_inner),
        );
        nbt.put_int(
            "WaitTime",
            *self
                .wait_time
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        nbt.put_int(
            "ReapplicationDelay",
            *self
                .reapplication_delay
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        nbt.put_int(
            "DurationOnUse",
            *self
                .duration_on_use
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        nbt.put_float(
            "RadiusOnUse",
            *self
                .radius_on_use
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        nbt.put_float(
            "RadiusPerTick",
            *self
                .radius_on_tick
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        nbt.put_float("Radius", self.get_radius());
        let custom_particle = *self
            .custom_particle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(particle) = custom_particle {
            nbt.put_compound("custom_particle", particle.to_nbt());
        }
        if let Some(owner) = self.owner.load() {
            nbt.put_uuid("Owner", owner);
        }
        if let Some(contents) = self.potion_contents() {
            nbt.put("potion_contents", contents.write_data());
        }
        let duration_scale = *self
            .potion_duration_scale
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if duration_scale != DEFAULT_POTION_DURATION_SCALE {
            nbt.put_float("potion_duration_scale", duration_scale);
        }
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        self.entity
            .age
            .store(nbt.get_int("Age").unwrap_or(0), Relaxed);
        *self.duration.lock().unwrap_or_else(PoisonError::into_inner) =
            nbt.get_int("Duration").unwrap_or(INFINITE_DURATION);
        *self
            .wait_time
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            nbt.get_int("WaitTime").unwrap_or(DEFAULT_WAIT_TIME);
        *self
            .reapplication_delay
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = nbt
            .get_int("ReapplicationDelay")
            .unwrap_or(DEFAULT_REAPPLICATION_DELAY);
        *self
            .duration_on_use
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = nbt.get_int("DurationOnUse").unwrap_or(0);
        *self
            .radius_on_use
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = nbt.get_float("RadiusOnUse").unwrap_or(0.0);
        *self
            .radius_on_tick
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            nbt.get_float("RadiusPerTick").unwrap_or(0.0);
        self.set_radius(nbt.get_float("Radius").unwrap_or(DEFAULT_RADIUS));
        self.owner.store(nbt.get_uuid("Owner"));
        self.set_custom_particle(
            nbt.get_compound("custom_particle")
                .and_then(ParticleOptions::from_nbt),
        );

        let mut stack = ItemStack::new(0, &pumpkin_data::item::Item::GLASS_BOTTLE);
        if let Some(contents) = nbt
            .get("potion_contents")
            .and_then(PotionContentsImpl::read_data)
        {
            stack.set_data_component(contents);
        }
        *self.effects.lock().unwrap_or_else(PoisonError::into_inner) =
            PotionContents::read_potion_effects(&stack);
        *self
            .item_stack
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = stack;
        *self
            .potion_duration_scale
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = nbt
            .get_float("potion_duration_scale")
            .unwrap_or(DEFAULT_POTION_DURATION_SCALE);
        self.update_particle();
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&crate::entity::living::LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
