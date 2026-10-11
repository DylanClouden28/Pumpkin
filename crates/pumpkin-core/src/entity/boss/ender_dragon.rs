use crossbeam::atomic::AtomicCell;
use pumpkin_data::Block;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{atan2, cos, sin, wrap_degrees};
use pumpkin_world::{chunk::ChunkHeightmapType, world::BlockFlags};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::entity::{
    Entity, EntityBase,
    ai::{
        control::body_rotation_control::BodyRotationControl,
        pathfinder::{binary_heap::BinaryHeap, node::Node, path::Path},
        target_predicate::TargetPredicate,
    },
    experience_orb::ExperienceOrbEntity,
    living::{LivingEntity, can_be_seen_as_enemy},
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::plugin::api::events::entity::{
    entity_change_block::EntityChangeBlockEvent, entity_damage::EntityDamageEvent,
    entity_death::EntityDeathEvent,
};
use crate::world::{World, dragon_fight::DragonFight};

pub mod flight_history;
pub mod phase;

use flight_history::DragonFlightHistory;
use phase::{EnderDragonPhase, Phase, PhaseManager};

pub const NODE_COUNT: usize = 24;
const DEATH_TIMER_MAX: i32 = 200;
const FLAP_SPEED: f32 = 0.2;
const SITTING_ALLOWED_DAMAGE_PERCENTAGE: f32 = 0.25;
const CRYSTAL_HEAL_INTERVAL: i32 = 10;
const CRYSTAL_SEARCH_RANGE: f64 = 32.0;
const CRYSTAL_DESTROYED_DAMAGE: f32 = 10.0;
/// Width and height of the head, neck, body, three tail parts and two wings (`EnderDragon` constructor).
const PART_SIZES: [(f32, f32); 8] = [
    (1.0, 1.0),
    (3.0, 3.0),
    (5.0, 3.0),
    (2.0, 2.0),
    (2.0, 2.0),
    (2.0, 2.0),
    (4.0, 2.0),
    (4.0, 2.0),
];
const HEAD: usize = 0;
const NECK: usize = 1;
const BODY: usize = 2;
const TAIL1: usize = 3;
const WING1: usize = 6;
const WING2: usize = 7;
const WING_KNOCKBACK_DAMAGE: f32 = 5.0;
const HEAD_DAMAGE: f32 = 10.0;
const DRAGON_PHASE_KEY: &str = "DragonPhase";
const DRAGON_DEATH_TIME_KEY: &str = "DragonDeathTime";
const SITTING_DAMAGE_RECEIVED_KEY: &str = "sitting_damage_received";
pub const INNER_RING_START: usize = 12;
pub const CENTER_RING_START: usize = 20;
/// Lowest node height, vanilla `findClosestNode`.
const MIN_NODE_Y: i32 = 73;

const NODE_ADJACENCY: [u32; NODE_COUNT] = [
    6146, 8197, 8202, 16404, 32808, 32848, 65696, 131392, 131712, 263424, 526848, 525313, 1581057,
    3166214, 2138120, 6373424, 4358208, 12910976, 9044480, 9706496, 15216640, 13688832, 11763712,
    8257536,
];

pub struct EnderDragonPart {
    pub entity: Entity,
    pub dragon_uuid: uuid::Uuid,
}

impl EnderDragonPart {
    pub const fn new(entity: Entity, dragon_uuid: uuid::Uuid) -> Self {
        Self {
            entity,
            dragon_uuid,
        }
    }
}

impl EntityBase for EnderDragonPart {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn damage(&self, source: &dyn EntityBase, amount: f32, damage_type: DamageType) -> bool {
        let world = self.entity.world.load();
        if let Some(dragon_base) = world
            .entities
            .load()
            .iter()
            .find(|e| e.get_entity().entity_uuid == self.dragon_uuid)
        {
            return dragon_base.damage(source, amount, damage_type);
        }
        false
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    fn can_hit(&self) -> bool {
        true
    }
}

pub struct EnderDragonEntity {
    pub mob_entity: MobEntity,
    pub parts: Vec<Arc<EnderDragonPart>>,

    pub flight_history: Mutex<DragonFlightHistory>,
    body_rotation_control: Mutex<BodyRotationControl>,
    pub flap_time: AtomicCell<f32>,
    pub o_flap_time: AtomicCell<f32>,

    pub in_wall: AtomicCell<bool>,
    pub dragon_death_time: AtomicCell<i32>,
    pub sitting_damage_received: AtomicCell<f32>,

    pub fight_origin: AtomicCell<BlockPos>,

    pub phase_manager: PhaseManager,
    pub phase: AtomicCell<EnderDragonPhase>,
    pub yaw_rot_accel: AtomicCell<f32>,

    nodes: Mutex<Option<[Node; NODE_COUNT]>>,
    /// Entity id of the crystal healing the dragon, vanilla `nearestCrystal`.
    pub nearest_crystal: AtomicCell<Option<i32>>,
}

impl EnderDragonEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Ordering::Relaxed);
        let base_id = entity.entity_id;
        let dragon_uuid = entity.entity_uuid;
        let world = entity.world.load();

        let _ = Entity::reserve_ids(8);

        let mut parts = Vec::new();
        for i in 1..=8 {
            let part_entity = Entity::from_uuid_with_id(
                base_id + i,
                uuid::Uuid::new_v4(),
                world.clone(),
                entity.pos.load(),
                &EntityType::ENDER_DRAGON,
            );
            let (width, height) = PART_SIZES[i as usize - 1];
            let size = EntityDimensions::new(width, height, height * 0.85);
            part_entity.entity_dimension.store(size);
            let pos = part_entity.pos.load();
            part_entity
                .bounding_box
                .store(BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &size));
            let part = Arc::new(EnderDragonPart::new(part_entity, dragon_uuid));
            // TODO: world.add_entity_silent(part.clone() as Arc<dyn EntityBase>);
            parts.push(part);
        }

        Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            parts,
            flight_history: Mutex::new(DragonFlightHistory::default()),
            body_rotation_control: Mutex::new(BodyRotationControl::new()),
            flap_time: AtomicCell::new(0.0),
            o_flap_time: AtomicCell::new(0.0),
            in_wall: AtomicCell::new(false),
            dragon_death_time: AtomicCell::new(0),
            sitting_damage_received: AtomicCell::new(0.0),
            fight_origin: AtomicCell::new(BlockPos::new(0, 128, 0)),
            phase_manager: PhaseManager::default(),
            // Vanilla starts every dragon hovering; the fight switches it to the holding pattern.
            phase: AtomicCell::new(EnderDragonPhase::Hover),
            yaw_rot_accel: AtomicCell::new(0.0),
            nodes: Mutex::new(None),
            nearest_crystal: AtomicCell::new(None),
        })
    }

    pub fn set_fight_origin(&self, pos: BlockPos) {
        self.fight_origin.store(pos);
    }

    /// Vanilla `EnderDragonPhaseManager.setPhase`, which also syncs the phase to clients.
    pub fn set_phase(&self, phase_type: EnderDragonPhase) {
        let old = self.current_phase_type();
        if old == phase_type {
            return;
        }
        // Vanilla ends the old phase while it is still the current one.
        self.phase_manager.get_phase(old).end(self);
        self.phase.store(phase_type);
        self.mob_entity.living_entity.entity.set_synced_data(
            tracked_data::ender_dragon::DATA_PHASE,
            phase_type.network_id(),
        );
        self.phase_manager.get_phase(phase_type).begin(self);
    }

    #[must_use]
    pub fn current_phase_type(&self) -> EnderDragonPhase {
        self.phase.load()
    }

    #[must_use]
    pub fn current_phase(&self) -> &dyn Phase {
        self.phase_manager.get_phase(self.current_phase_type())
    }

    #[must_use]
    pub fn position(&self) -> Vector3<f64> {
        self.mob_entity.living_entity.entity.pos.load()
    }

    #[must_use]
    pub fn horizontal_speed(&self) -> f32 {
        let v = self.mob_entity.living_entity.entity.velocity.load();
        v.horizontal_length() as f32
    }

    /// Vanilla `horizontalCollision || verticalCollision`; both stay false without physics.
    #[must_use]
    pub fn collided(&self) -> bool {
        self.mob_entity
            .living_entity
            .entity
            .horizontal_collision
            .load(Ordering::Relaxed)
    }

    /// Crystals still standing, or `None` when this dragon isn't the fight's dragon.
    #[must_use]
    pub fn alive_crystals(&self) -> Option<i32> {
        self.with_fight(|fight, _| fight.alive_crystals())
    }

    /// Runs `f` on the fight if this is its dragon; the caller must not hold the fight lock.
    pub fn with_fight<T>(&self, f: impl FnOnce(&mut DragonFight, &Arc<World>) -> T) -> Option<T> {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let mut fight = world
            .dragon_fight
            .as_ref()?
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (fight.dragon_uuid == Some(entity.entity_uuid)).then(|| f(&mut fight, &world))
    }

    /// The top of the podium, vanilla `getHeightmapPos(.., getPodiumLocation(fightOrigin))`.
    #[must_use]
    pub fn podium_pos(&self, no_leaves: bool) -> BlockPos {
        let origin = self.fight_origin.load();
        let heightmap = if no_leaves {
            ChunkHeightmapType::MotionBlockingNoLeaves
        } else {
            ChunkHeightmapType::MotionBlocking
        };
        let world = self.mob_entity.living_entity.entity.world.load();
        // Vanilla `getHeightmapPos` is one above Pumpkin's heightmap height.
        let y = world.get_heightmap_height(heightmap, origin.0.x, origin.0.z) + 1;
        BlockPos::new(origin.0.x, y, origin.0.z)
    }

    #[must_use]
    pub fn podium_bottom_center(&self, no_leaves: bool) -> Vector3<f64> {
        self.podium_pos(no_leaves).to_f64()
    }

    #[must_use]
    pub fn head_pos(&self) -> Vector3<f64> {
        self.parts[HEAD].entity.pos.load()
    }

    /// Vanilla `head.getY(scale)`.
    #[must_use]
    pub fn head_y(&self, scale: f64) -> f64 {
        let head = &self.parts[HEAD].entity;
        head.pos.load().y + f64::from(head.height()) * scale
    }

    /// Vanilla `getViewVector(1.0F)` with the given pitch, which uses the head yaw.
    #[must_use]
    pub fn view_vector(&self, x_rot: f32) -> Vector3<f64> {
        let yaw = self.mob_entity.living_entity.entity.head_yaw.load();
        let f = x_rot.to_radians();
        let g = -yaw.to_radians();
        Vector3::new(
            f64::from(sin(g) * cos(f)),
            f64::from(-sin(f)),
            f64::from(cos(g) * cos(f)),
        )
    }

    /// Vanilla `getHeadLookVector`: tilted down while landing, taking off or perched.
    #[must_use]
    pub fn head_look_vector(&self) -> Vector3<f64> {
        let phase = self.current_phase_type();
        if matches!(phase, EnderDragonPhase::Landing | EnderDragonPhase::Takeoff) {
            let egg = self.podium_pos(true).to_centered_f64();
            let dist =
                ((egg.squared_distance_to_vec(&self.position()).sqrt() as f32) / 4.0).max(1.0);
            let y_offset = 6.0 / dist;
            self.view_vector(-y_offset * 1.5 * 5.0)
        } else if self.current_phase().is_sitting() {
            self.view_vector(-45.0)
        } else {
            self.view_vector(self.mob_entity.living_entity.entity.pitch.load())
        }
    }

    /// Angle in degrees between the dragon's facing and `aim`, plus vanilla's 0.5 degree offset.
    #[must_use]
    pub fn facing_angle_to(&self, aim: Vector3<f64>) -> f32 {
        let yaw = self.mob_entity.living_entity.entity.yaw.load().to_radians();
        let dir = Vector3::new(f64::from(sin(yaw)), 0.0, -f64::from(cos(yaw))).vanilla_normalize();
        let dot = dir.dot(&aim) as f32;
        (f64::from(dot).acos() * 180.0 / f64::from(std::f32::consts::PI)) as f32 + 0.5
    }

    #[must_use]
    pub fn has_line_of_sight(&self, player: &Player) -> bool {
        self.mob_entity
            .living_entity
            .entity
            .has_line_of_sight(player.get_entity())
    }

    /// Vanilla `getNearestPlayer(TargetingConditions.forCombat()..)`.
    #[must_use]
    pub fn nearest_combat_player(
        &self,
        pos: Vector3<f64>,
        range: Option<f64>,
        max_y_distance: Option<f64>,
        line_of_sight: bool,
    ) -> Option<Arc<Player>> {
        let world = self.mob_entity.living_entity.entity.world.load();
        let mut predicate = TargetPredicate::create_attackable();
        if let Some(range) = range {
            predicate = predicate.set_base_max_distance(range);
        }
        if !line_of_sight {
            predicate = predicate.ignore_visibility();
        }
        if let Some(max_y_distance) = max_y_distance {
            let dragon_y = self.position().y;
            predicate.set_predicate(move |target, _| {
                (target.entity.pos.load().y - dragon_y).abs() <= max_y_distance
            });
        }
        world.get_nearest_player(pos, f64::INFINITY, |player| {
            predicate.test(&world, Some(self), player.as_ref())
        })
    }

    /// Vanilla `EnderDragon.onCrystalDestroyed`.
    pub fn on_crystal_destroyed(
        &self,
        crystal: &dyn EntityBase,
        crystal_pos: Vector3<f64>,
        attacker: Option<Arc<Player>>,
    ) {
        let world = self.mob_entity.living_entity.entity.world.load();
        // Vanilla `CRYSTAL_DESTROY_TARGETING` has no targeter, so its range is never applied.
        let predicate = TargetPredicate::create_attackable();
        let player = attacker.or_else(|| {
            world.get_nearest_player(crystal_pos, f64::INFINITY, |player| {
                predicate.test(&world, None, player.as_ref())
            })
        });
        if self
            .nearest_crystal
            .compare_exchange(Some(crystal.get_entity().entity_id), None)
            .is_ok()
        {
            let cause = player.as_ref().map(|p| p.as_ref() as &dyn EntityBase);
            let damage_type = if cause.is_some() {
                DamageType::PLAYER_EXPLOSION
            } else {
                DamageType::EXPLOSION
            };
            self.hurt(
                HEAD,
                CRYSTAL_DESTROYED_DAMAGE,
                damage_type,
                None,
                Some(crystal),
                cause,
            );
        }
        self.current_phase()
            .on_crystal_destroyed(self, player.as_ref());
    }

    /// Vanilla `DragonHoldingPatternPhase.strafePlayer`.
    pub fn strafe_player(&self, player: &Arc<Player>) {
        self.set_phase(EnderDragonPhase::StrafePlayer);
        self.phase_manager.strafe_player.set_target(self, player);
    }

    fn with_nodes<T>(&self, f: impl FnOnce(&[Node; NODE_COUNT]) -> T) -> T {
        let mut nodes = self
            .nodes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let nodes = nodes.get_or_insert_with(|| self.create_nodes());
        f(nodes)
    }

    /// The node rings vanilla `findClosestNode` builds on first use.
    fn create_nodes(&self) -> [Node; NODE_COUNT] {
        let world = self.mob_entity.living_entity.entity.world.load();
        std::array::from_fn(|i| {
            let (radius, slices, index, y_adjustment) = if i < INNER_RING_START {
                (60.0, 12.0, i, 5)
            } else if i < CENTER_RING_START {
                (40.0, 8.0, i - INNER_RING_START, 15)
            } else {
                (20.0, 4.0, i - CENTER_RING_START, 5)
            };
            let step = (std::f64::consts::PI / slices) as f32;
            let angle = 2.0 * (-std::f32::consts::PI + step * index as f32);
            let x = (radius * cos(angle)).floor() as i32;
            let z = (radius * sin(angle)).floor() as i32;
            let height =
                world.get_heightmap_height(ChunkHeightmapType::MotionBlockingNoLeaves, x, z) + 1;
            Node::new(BlockPos::new(x, MIN_NODE_Y.max(height + y_adjustment), z))
        })
    }

    fn first_usable_node(&self) -> usize {
        if self.alive_crystals().unwrap_or(0) == 0 {
            INNER_RING_START
        } else {
            0
        }
    }

    #[must_use]
    pub fn find_closest_node(&self) -> usize {
        self.find_closest_node_to(self.position())
    }

    #[must_use]
    pub fn find_closest_node_to(&self, pos: Vector3<f64>) -> usize {
        let start = self.first_usable_node();
        let target = BlockPos::floored_v(pos);
        self.with_nodes(|nodes| {
            let mut closest_dist = 10000.0;
            let mut closest = 0;
            for (i, node) in nodes.iter().enumerate().skip(start) {
                let dist = node.distance_to_sqr_block_pos(target);
                if dist < closest_dist {
                    closest_dist = dist;
                    closest = i;
                }
            }
            closest
        })
    }

    /// Vanilla `findPath`: A* over the node graph, falling back to the node closest to `end`.
    #[must_use]
    pub fn find_path(
        &self,
        start: usize,
        end: usize,
        final_node: Option<BlockPos>,
    ) -> Option<Path> {
        let min_index = self.first_usable_node();
        self.with_nodes(|nodes| {
            // Vanilla resets each node's search state here; copies start clean.
            let mut nodes = *nodes;
            let to = nodes[end];
            nodes[start].h = nodes[start].distance_to_node(&to);
            nodes[start].f = nodes[start].h;
            let mut open_set = BinaryHeap::with_capacity(NODE_COUNT);
            open_set.insert(nodes[start]);
            let mut closest = start;

            let reconstruct = |nodes: &[Node; NODE_COUNT], last: usize| {
                let mut path = vec![nodes[last]];
                let mut node = nodes[last];
                while let Some(prev) = node
                    .came_from
                    .and_then(|pos| nodes.iter().find(|n| n.pos.0 == pos))
                {
                    path.push(*prev);
                    node = *prev;
                }
                path.reverse();
                if let Some(final_node) = final_node {
                    path.push(Node::new(final_node));
                }
                Path::new(path, final_node.unwrap_or(nodes[last].pos), true)
            };

            while let Some(popped) = open_set.pop() {
                // The heap holds copies whose `g` goes stale on `change_cost`, so use `nodes`.
                let Some(current) = nodes.iter().position(|n| n.pos == popped.pos) else {
                    break;
                };
                if current == end {
                    return Some(reconstruct(&nodes, end));
                }
                let open_node = nodes[current];
                if open_node.distance_to_node(&to) < nodes[closest].distance_to_node(&to) {
                    closest = current;
                }
                nodes[current].closed = true;
                for (next, adjacent) in nodes.iter_mut().enumerate().skip(min_index) {
                    if NODE_ADJACENCY[current] & (1 << next) == 0 || adjacent.closed {
                        continue;
                    }
                    let tentative = open_node.g + open_node.distance_to_node(adjacent);
                    let in_open_set = open_set.contains(adjacent);
                    if !in_open_set || tentative < adjacent.g {
                        adjacent.came_from = Some(open_node.pos.0);
                        adjacent.g = tentative;
                        adjacent.h = adjacent.distance_to_node(&to);
                        let cost = adjacent.g + adjacent.h;
                        if in_open_set {
                            open_set.change_cost(adjacent, cost);
                        } else {
                            adjacent.f = cost;
                            open_set.insert(*adjacent);
                        }
                    }
                }
            }

            if closest == start {
                return None;
            }
            Some(reconstruct(&nodes, closest))
        })
    }

    pub fn move_relative(&self, speed: f32, relative_movement: Vector3<f64>) {
        let yaw = self.mob_entity.living_entity.entity.yaw.load();
        let movement = Self::get_input_vector(relative_movement, speed, yaw);
        let entity = &self.mob_entity.living_entity.entity;
        let vel = entity.velocity.load();
        entity.velocity.store(vel + movement);
    }

    /// Vanilla `Entity.getInputVector`; `Entity::movement_input_to_velocity` rounds differently.
    fn get_input_vector(relative_movement: Vector3<f64>, speed: f32, yaw: f32) -> Vector3<f64> {
        let dist = relative_movement.length_squared();
        if dist < 1.0E-7 {
            return Vector3::new(0.0, 0.0, 0.0);
        }
        let vec = if dist > 1.0 {
            relative_movement.vanilla_normalize()
        } else {
            relative_movement
        } * (speed as f64);
        let sin = sin(yaw * (std::f32::consts::PI / 180.0)) as f64;
        let cos = cos(yaw * (std::f32::consts::PI / 180.0)) as f64;
        Vector3::new(vec.x * cos - vec.z * sin, vec.y, vec.z * cos + vec.x * sin)
    }

    /// The flight step of vanilla `EnderDragon.aiStep`.
    pub fn steer_toward(
        &self,
        pos: Vector3<f64>,
        target: Vector3<f64>,
        fly_speed: f32,
        turn_speed: f32,
    ) {
        let xdd = target.x - pos.x;
        let mut ydd = target.y - pos.y;
        let zdd = target.z - pos.z;
        let dist_sq = xdd * xdd + ydd * ydd + zdd * zdd;

        let horizontal_dist = (target - pos).horizontal_length();
        if horizontal_dist > 0.0 {
            ydd = (ydd / horizontal_dist).clamp(-(fly_speed as f64), fly_speed as f64);
        }

        let entity = &self.mob_entity.living_entity.entity;
        let mut vel = entity.velocity.load();
        vel.y += ydd * 0.01;
        entity.velocity.store(vel);

        let yaw = entity.yaw.load();
        let aim = (target - pos).vanilla_normalize();
        let dir = Vector3::new(
            f64::from(sin(yaw * (std::f32::consts::PI / 180.0))),
            vel.y,
            -f64::from(cos(yaw * (std::f32::consts::PI / 180.0))),
        )
        .vanilla_normalize();

        let dot = (dir.dot(&aim) as f32 + 0.5).max(0.0) / 1.5;
        if xdd.abs() > f64::from(1.0E-5f32) || zdd.abs() > f64::from(1.0E-5f32) {
            let y_rot_d =
                wrap_degrees(180.0 - atan2(xdd, zdd) as f32 * (180.0 / std::f32::consts::PI) - yaw)
                    .clamp(-50.0, 50.0);

            let y_rot_a = self.yaw_rot_accel.load() * 0.8 + y_rot_d * turn_speed;
            self.yaw_rot_accel.store(y_rot_a);
            entity.yaw.store(yaw + y_rot_a * 0.1);
        }

        let span = (2.0 / (dist_sq + 1.0)) as f32;
        self.move_relative(
            0.06 * (dot * span + (1.0 - span)),
            Vector3::new(0.0, 0.0, -1.0),
        );

        let motion = entity.velocity.load();
        if self.in_wall.load() {
            entity.move_entity(self, motion * f64::from(0.8f32));
        } else {
            entity.move_entity(self, motion);
        }

        let actual_vel = entity.velocity.load();
        let actual_dir = actual_vel.vanilla_normalize();
        let slide = 0.8 + 0.15 * (actual_dir.dot(&dir) + 1.0) / 2.0;
        entity.velocity.store(Vector3::new(
            actual_vel.x * slide,
            actual_vel.y * f64::from(0.91f32),
            actual_vel.z * slide,
        ));
    }

    fn update_flap_time(&self) {
        let sitting = self.current_phase().is_sitting();
        let old_flap = self.flap_time.load();
        self.o_flap_time.store(old_flap);
        let mut flap = old_flap
            + if sitting {
                0.1
            } else if self.in_wall.load() {
                FLAP_SPEED * 0.5
            } else {
                FLAP_SPEED
            };
        if flap > std::f32::consts::TAU {
            flap -= std::f32::consts::TAU;
        }
        self.flap_time.store(flap);
    }

    /// Vanilla `checkCrystals`.
    fn check_crystals(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let mut nearest = self.nearest_crystal.load();
        let mut heal = false;
        if let Some(id) = nearest {
            match world.get_entity_by_id(id) {
                Some(crystal) if !crystal.get_entity().is_removed() => {
                    let living = &self.mob_entity.living_entity;
                    heal = entity.age.load(Ordering::Relaxed) % CRYSTAL_HEAL_INTERVAL == 0
                        && living.health.load() < living.get_max_health();
                }
                _ => nearest = None,
            }
        }

        if rand::random_range(0..10) == 0 {
            let search = entity.bounding_box.load().expand(
                CRYSTAL_SEARCH_RANGE,
                CRYSTAL_SEARCH_RANGE,
                CRYSTAL_SEARCH_RANGE,
            );
            let pos = entity.pos.load();
            nearest = world
                .get_entities_at_box(&search)
                .into_iter()
                .filter(|e| e.get_entity().entity_type == &EntityType::END_CRYSTAL)
                .min_by(|a, b| {
                    let da = a.get_entity().pos.load().squared_distance_to_vec(&pos);
                    let db = b.get_entity().pos.load().squared_distance_to_vec(&pos);
                    da.total_cmp(&db)
                })
                .map(|e| e.get_entity().entity_id);
        }
        self.nearest_crystal.store(nearest);
        if heal {
            self.mob_entity.living_entity.heal(1.0);
        }
    }

    /// Vanilla `wasHurtRecently`: its 10 ticks count down before `aiStep`, Pumpkin's 20 tick hurt
    /// cooldown after it, so 20..=12 covers the same 9 ticks.
    fn was_hurt_recently(&self) -> bool {
        self.mob_entity
            .living_entity
            .hurt_cooldown
            .load(Ordering::Relaxed)
            > 11
    }

    /// Living entities in `area`, vanilla `EntitySelector.NO_CREATIVE_OR_SPECTATOR`.
    fn living_targets_in(&self, area: &BoundingBox) -> Vec<Arc<dyn EntityBase>> {
        let world = self.mob_entity.living_entity.entity.world.load();
        let own_id = self.mob_entity.living_entity.entity.entity_id;
        world
            .get_all_at_box(area)
            .into_iter()
            .filter(|e| e.get_entity().entity_id != own_id && e.get_living_entity().is_some())
            .filter(|e| {
                e.get_player()
                    .is_none_or(|p| !p.is_spectator() && !p.is_creative())
            })
            .collect()
    }

    /// Vanilla `EnderDragon.knockBack`.
    fn knock_back(&self, area: &BoundingBox) {
        let body = self.parts[BODY].entity.bounding_box.load();
        let xm = f64::midpoint(body.min.x, body.max.x);
        let zm = f64::midpoint(body.min.z, body.max.z);
        let sitting = self.current_phase().is_sitting();
        let dragon: &dyn EntityBase = self;
        for target in self.living_targets_in(area) {
            let entity = target.get_entity();
            let pos = entity.pos.load();
            let xd = pos.x - xm;
            let zd = pos.z - zm;
            let dd = (xd * xd + zd * zd).max(0.1);
            let push = Vector3::new(xd / dd * 4.0, f64::from(0.2f32), zd / dd * 4.0);
            // Vanilla pushes before the hit, so the hit's knockback acts on the pushed velocity.
            entity.velocity.store(entity.velocity.load() + push);
            let hurt = !sitting
                && target.get_living_entity().is_some_and(|living| {
                    living.last_attacked_time.load(Ordering::Relaxed)
                        < entity.age.load(Ordering::Relaxed) - 2
                })
                && dragon.do_teams_allow_damage(target.as_ref())
                && target.damage_with_context(
                    target.as_ref(),
                    WING_KNOCKBACK_DAMAGE,
                    DamageType::MOB_ATTACK,
                    Some(self.position()),
                    Some(self),
                    Some(self),
                );
            if target.get_player().is_none() || hurt {
                entity.velocity_dirty.store(true, Ordering::SeqCst);
            } else {
                // Only a landed hit sends a player its push (vanilla `markHurt`). Pumpkin doesn't
                // simulate players, so an unsent push would linger instead of decaying.
                entity.velocity.store(entity.velocity.load() - push);
            }
        }
    }

    /// Vanilla `EnderDragon.hurt(List)`.
    fn hurt_touching(&self, area: &BoundingBox) {
        let dragon: &dyn EntityBase = self;
        for target in self.living_targets_in(area) {
            if dragon.do_teams_allow_damage(target.as_ref()) {
                target.damage_with_context(
                    target.as_ref(),
                    HEAD_DAMAGE,
                    DamageType::MOB_ATTACK,
                    Some(self.position()),
                    Some(self),
                    Some(self),
                );
            }
        }
    }

    /// Vanilla `checkWalls`: returns whether a block the dragon can't break is in `area`.
    fn check_walls(&self, area: &BoundingBox) -> bool {
        let world = self.mob_entity.living_entity.entity.world.load();
        let griefing = world.level_info.load().game_rules.mob_griefing;
        let min = BlockPos::floored_v(area.min);
        let max = BlockPos::floored_v(area.max);
        let mut hit_wall = false;
        let mut destroyed = false;
        for x in min.0.x..=max.0.x {
            for y in min.0.y..=max.0.y {
                for z in min.0.z..=max.0.z {
                    let pos = BlockPos::new(x, y, z);
                    let (block, state) = world.get_block_and_state(&pos);
                    if state.is_air() || block.has_tag(&tag::Block::MINECRAFT_DRAGON_TRANSPARENT) {
                        continue;
                    }
                    if !griefing || block.has_tag(&tag::Block::MINECRAFT_DRAGON_IMMUNE) {
                        hit_wall = true;
                        continue;
                    }
                    // `removeBlock` leaves the fluid's legacy block behind.
                    if block == &Block::WATER || block == &Block::LAVA {
                        continue;
                    }
                    let (_, fluid_state) = world.get_fluid_and_fluid_state(&pos);
                    let new_state = fluid_state.block_state_id;
                    let mut event = EntityChangeBlockEvent::new(
                        self.mob_entity.living_entity.entity.entity_id,
                        pos,
                        format!("minecraft:{}", new_state.to_block().name),
                    );
                    if let Some(server) = world.server.upgrade() {
                        server.plugin_manager.fire_blocking(&server, &mut event);
                    }
                    if event.cancelled {
                        continue;
                    }
                    destroyed |= world
                        .set_block_state_if(&pos, new_state, BlockFlags::NOTIFY_ALL, |current| {
                            current != new_state
                        })
                        .is_some();
                }
            }
        }
        if destroyed {
            let random_pos = BlockPos::new(
                rand::random_range(min.0.x..=max.0.x),
                rand::random_range(min.0.y..=max.0.y),
                rand::random_range(min.0.z..=max.0.z),
            );
            world.sync_world_event(WorldEvent::ParticlesDragonBlockBreak, random_pos, 0);
        }
        hit_wall
    }

    /// Vanilla `EnderDragon.hurt(level, part, ..)`: only the head and neck take full damage.
    pub fn hurt(
        &self,
        part: usize,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        if self.current_phase_type() == EnderDragonPhase::Dying {
            return false;
        }
        let mut damage = self.current_phase().on_hurt(damage_type, amount);
        if part != HEAD && part != NECK {
            damage = damage / 4.0 + damage.min(1.0);
        }
        if damage < 0.01 {
            return false;
        }

        let from_player = cause.or(source).is_some_and(|e| e.get_player().is_some());
        if from_player
            || damage_type.has_tag(&tag::DamageType::MINECRAFT_ALWAYS_HURTS_ENDER_DRAGONS)
        {
            let living = &self.mob_entity.living_entity;
            let health_before = living.health.load();
            if living.damage_with_context(self, damage, damage_type, position, source, cause)
                && living.health.load() <= 0.0
            {
                // Vanilla `handleKillingBlow`, after the normal death ran: a flying dragon goes
                // back to 1 health to fly to the podium, a perched one keeps its phase.
                living.dead.store(false, Ordering::Relaxed);
                if !self.current_phase().is_sitting() {
                    living.set_health(1.0);
                    self.set_phase(EnderDragonPhase::Dying);
                }
            }
            if self.current_phase().is_sitting() {
                // Vanilla takes off once a perched dragon has lost a quarter of its health.
                let received =
                    self.sitting_damage_received.load() + health_before - living.health.load();
                self.sitting_damage_received.store(received);
                if received > SITTING_ALLOWED_DAMAGE_PERCENTAGE * living.get_max_health() {
                    self.sitting_damage_received.store(0.0);
                    self.set_phase(EnderDragonPhase::Takeoff);
                }
            }
        }
        // Vanilla also reports ignored damage and hits during the hurt cooldown as taken.
        true
    }

    /// Vanilla `EnderDragon.kill`: removes the dragon without the death sequence or experience.
    fn kill_now(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        for part in &self.parts {
            part.entity.remove();
        }
        entity.remove();
        entity
            .world
            .load()
            .emit_game_event(GameEvent::EntityDie.name(), entity.pos.load());
        self.update_fight();
        self.with_fight(|fight, world| fight.set_dragon_killed(world, entity.entity_uuid));
    }

    /// Vanilla `dragonFight.updateDragon(this)`.
    fn update_fight(&self) {
        let living = &self.mob_entity.living_entity;
        let custom_name = living.entity.custom_name.load();
        self.with_fight(|fight, world| {
            fight.update_dragon(
                world,
                living.entity.entity_uuid,
                living.health.load(),
                living.get_max_health(),
                custom_name.as_ref().as_ref(),
            );
        });
    }

    /// Vanilla `EnderDragon.tickDeath`.
    fn tick_death(&self) {
        self.update_fight();
        let death_time = self.dragon_death_time.fetch_add(1) + 1;

        // Vanilla's server-side death particles do nothing; clients draw them.
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let xp_count = if self
            .with_fight(|fight, _| !fight.has_previously_killed_dragon())
            .unwrap_or(false)
        {
            12000
        } else {
            500
        };
        let mob_drops = world.level_info.load().game_rules.mob_drops;
        if mob_drops && death_time > 150 && death_time % 5 == 0 {
            ExperienceOrbEntity::spawn(&world, entity.pos.load(), (xp_count as f32 * 0.08) as u32);
        }
        if death_time == 1 && !entity.is_silent() {
            world.sync_global_world_event(WorldEvent::SoundDragonDeath, entity.block_pos.load(), 0);
        }

        let death_move = Vector3::new(0.0, f64::from(0.1f32), 0.0);
        entity.move_entity(self, death_move);
        for part in &self.parts {
            part.entity.set_pos(part.entity.pos.load() + death_move);
        }

        if death_time >= DEATH_TIMER_MAX {
            if mob_drops {
                ExperienceOrbEntity::spawn(
                    &world,
                    entity.pos.load(),
                    (xp_count as f32 * 0.2) as u32,
                );
            }
            self.with_fight(|fight, world| fight.set_dragon_killed(world, entity.entity_uuid));
            for part in &self.parts {
                part.entity.remove();
            }
            entity.remove();
            world.emit_game_event(GameEvent::EntityDie.name(), entity.pos.load());
        }
    }

    /// Vanilla `tickPart`. Callers compute the offsets in `f32` like vanilla.
    fn tick_part(&self, part: usize, x: f64, y: f64, z: f64) {
        let pos = self.mob_entity.living_entity.entity.pos.load();
        self.parts[part]
            .entity
            .set_pos(Vector3::new(pos.x + x, pos.y + y, pos.z + z));
    }

    /// The body and wing placement of vanilla `aiStep`.
    fn tick_body_and_wings(&self) {
        let yaw = self.mob_entity.living_entity.entity.yaw.load();
        let rot1 = yaw * (std::f32::consts::PI / 180.0);
        let ss1 = sin(rot1);
        let cc1 = cos(rot1);

        self.tick_part(BODY, f64::from(ss1 * 0.5), 0.0, f64::from(-cc1 * 0.5));
        self.tick_part(WING1, f64::from(cc1 * 4.5), 2.0, f64::from(ss1 * 4.5));
        self.tick_part(WING2, f64::from(cc1 * -4.5), 2.0, f64::from(ss1 * -4.5));
    }

    /// The head, neck and tail placement of vanilla `aiStep`.
    fn tick_head_neck_and_tail(&self) {
        let history = self
            .flight_history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let p5 = history.get(5);
        let p10 = history.get(10);
        let p0 = history.get(0);

        let tilt = (p5.y - p10.y) as f32 * 10.0 * (std::f32::consts::PI / 180.0);
        let cc_tilt = cos(tilt);
        let ss_tilt = sin(tilt);

        let yaw = self.mob_entity.living_entity.entity.yaw.load();
        let rot1 = yaw * (std::f32::consts::PI / 180.0);
        let ss1 = sin(rot1);
        let cc1 = cos(rot1);

        // Vanilla `getHeadYOffset`.
        let head_y_offset = if self.current_phase().is_sitting() {
            -1.0
        } else {
            (p5.y - p0.y) as f32
        };

        let rot2 = yaw * (std::f32::consts::PI / 180.0) - self.yaw_rot_accel.load() * 0.01;
        let ss2 = sin(rot2);
        let cc2 = cos(rot2);

        self.tick_part(
            HEAD,
            f64::from(ss2 * 6.5 * cc_tilt),
            f64::from(head_y_offset + ss_tilt * 6.5),
            f64::from(-cc2 * 6.5 * cc_tilt),
        );
        self.tick_part(
            NECK,
            f64::from(ss2 * 5.5 * cc_tilt),
            f64::from(head_y_offset + ss_tilt * 5.5),
            f64::from(-cc2 * 5.5 * cc_tilt),
        );

        for i in 0..3 {
            let pi = history.get(12 + i * 2);
            let rot = yaw * (std::f32::consts::PI / 180.0)
                + wrap_degrees(pi.y_rot - p5.y_rot) * (std::f32::consts::PI / 180.0);
            let ss = sin(rot);
            let cc = cos(rot);
            let dd = (i + 1) as f32 * 2.0;

            self.tick_part(
                TAIL1 + i as usize,
                f64::from(-(ss1 * 1.5 + ss * dd) * cc_tilt),
                pi.y - p5.y - f64::from((dd + 1.5) * ss_tilt) + 1.5,
                f64::from((cc1 * 1.5 + cc * dd) * cc_tilt),
            );
        }
    }

    /// Vanilla `EnderDragon.aiStep`, plus `tickDeath`, which vanilla runs from `baseTick`.
    pub fn ai_step(&self) {
        let living = &self.mob_entity.living_entity;
        if living.entity.is_removed() {
            return;
        }
        living.entity.update_last_pos();
        if living.health.load() <= 0.0 {
            self.tick_death();
            return;
        }
        self.update_flap_time();
        self.check_crystals();

        let entity = &living.entity;
        entity.yaw.store(wrap_degrees(entity.yaw.load()));
        // Vanilla `isNoAi` keeps the crystal healing above and holds the wings still.
        if self.mob_entity.is_no_ai() {
            self.flap_time.store(0.5);
            return;
        }
        self.flight_history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record(entity.pos.load().y, entity.yaw.load());

        // Vanilla ticks the new phase straight away when the old one switched.
        let mut phase_type = self.current_phase_type();
        self.phase_manager
            .get_phase(phase_type)
            .do_server_tick(self);
        if self.current_phase_type() != phase_type {
            phase_type = self.current_phase_type();
            self.phase_manager
                .get_phase(phase_type)
                .do_server_tick(self);
        }

        // Vanilla steers with the phase it ticked last, even if that one switched again.
        let phase = self.phase_manager.get_phase(phase_type);
        if let Some(target) = phase.get_fly_target_location() {
            let pos = self.position();
            self.steer_toward(
                pos,
                target,
                phase.get_fly_speed(),
                phase.get_turn_speed(self),
            );
        }

        entity.body_yaw.store(entity.yaw.load());
        self.tick_body_and_wings();
        let part_box = |i: usize| self.parts[i].entity.bounding_box.load();
        // Vanilla attacks before moving the head and neck, so the bite uses last tick's boxes.
        if !self.was_hurt_recently() {
            for wing in [WING1, WING2] {
                self.knock_back(
                    &part_box(wing)
                        .expand(4.0, 2.0, 4.0)
                        .shift(Vector3::new(0.0, -2.0, 0.0)),
                );
            }
            self.hurt_touching(&part_box(HEAD).expand(1.0, 1.0, 1.0));
            self.hurt_touching(&part_box(NECK).expand(1.0, 1.0, 1.0));
        }
        self.tick_head_neck_and_tail();

        let in_wall = self.check_walls(&part_box(HEAD))
            | self.check_walls(&part_box(NECK))
            | self.check_walls(&part_box(BODY));
        self.in_wall.store(in_wall);
        self.update_fight();
    }
}

impl Mob for EnderDragonEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ai_step();
        // Vanilla `Mob.tickHeadTurn` runs after `aiStep` and drags the head yaw behind the body.
        self.body_rotation_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .client_tick(self);
    }

    /// Vanilla `EnderDragon.checkDespawn` is empty: the dragon never despawns.
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }

    /// Vanilla `EnderDragon.canAttack`.
    fn can_attack(&self, target: &dyn EntityBase) -> bool {
        can_be_seen_as_enemy(target)
    }

    /// Vanilla `EnderDragon.kill`, firing the same plugin events as the generic kill.
    fn mob_kill(&self, _caller: &dyn EntityBase) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let server = entity.world.load().server.upgrade();
        let mut damage_event =
            EntityDamageEvent::new(entity.entity_id, DamageType::GENERIC_KILL, f32::MAX);
        if let Some(server) = &server {
            server
                .plugin_manager
                .fire_blocking(server, &mut damage_event);
        }
        if damage_event.cancelled {
            return;
        }
        if damage_event.damage < living.health.load() {
            // A plugin made the kill nonlethal, so its damage lands as in the generic kill.
            living.damage_after_event(
                self,
                damage_event.damage,
                DamageType::GENERIC_KILL,
                None,
                None,
                None,
            );
            return;
        }
        if let Some(server) = &server {
            let mut death_event = EntityDeathEvent::new(entity.entity_id, 0);
            server
                .plugin_manager
                .fire_blocking(server, &mut death_event);
        }
        self.kill_now();
    }

    /// Vanilla `EnderDragon.hurtServer`: damage without a part hits the body.
    fn mob_damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        self.hurt(BODY, amount, damage_type, position, source, cause)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int(DRAGON_PHASE_KEY, self.current_phase_type().network_id());
        nbt.put_int(DRAGON_DEATH_TIME_KEY, self.dragon_death_time.load());
        nbt.put_float(
            SITTING_DAMAGE_RECEIVED_KEY,
            self.sitting_damage_received.load(),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        // Vanilla `EnderDragonPhase.getById` turns an unknown id into the holding pattern.
        if let Some(id) = nbt.get_int(DRAGON_PHASE_KEY) {
            self.set_phase(
                EnderDragonPhase::from_id(id).unwrap_or(EnderDragonPhase::HoldingPattern),
            );
        }
        self.dragon_death_time
            .store(nbt.get_int(DRAGON_DEATH_TIME_KEY).unwrap_or(0));
        self.sitting_damage_received
            .store(nbt.get_float(SITTING_DAMAGE_RECEIVED_KEY).unwrap_or(0.0));
    }
}
