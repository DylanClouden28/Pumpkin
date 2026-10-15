use super::{
    EnderDragonPhase, Phase, TARGET_DIST_SQR_RANGE, navigate_to_next_path_node, wrap_node_index,
};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use crate::entity::player::Player;
use crate::entity::projectile::dragon_fireball;
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::{Arc, Mutex};

const FIREBALL_CHARGE_AMOUNT: i32 = 5;
const MAX_FIREBALL_DIST_SQR: f64 = 4096.0;

#[derive(Default)]
struct State {
    fireball_charge: i32,
    current_path: Option<Path>,
    target_location: Option<Vector3<f64>>,
    attack_target: Option<Arc<Player>>,
    holding_pattern_clockwise: bool,
}

/// Flying at a player and shooting a fireball, vanilla `DragonStrafePlayerPhase`.
#[derive(Default)]
pub struct StrafePlayerPhase {
    state: Mutex<State>,
}

fn strafe_height_offset(horizontal_dist: f64) -> f64 {
    (f64::from(0.4f32) + horizontal_dist / 80.0 - 1.0).min(10.0)
}

impl StrafePlayerPhase {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Vanilla `setTarget`.
    pub fn set_target(&self, dragon: &EnderDragonEntity, target: &Arc<Player>) {
        let target_pos = target.get_entity().pos.load();
        let current_node = dragon.find_closest_node();
        let target_node = dragon.find_closest_node_to(target_pos);
        let final_x = target_pos.x.floor() as i32;
        let final_z = target_pos.z.floor() as i32;
        let dragon_pos = dragon.position();
        let dist = Vector3::new(
            f64::from(final_x) - dragon_pos.x,
            0.0,
            f64::from(final_z) - dragon_pos.z,
        )
        .horizontal_length();
        let final_y = (target_pos.y + strafe_height_offset(dist)).floor() as i32;
        let path = dragon.find_path(
            current_node,
            target_node,
            Some(BlockPos::new(final_x, final_y, final_z)),
        );

        let mut state = self.state();
        state.attack_target = Some(target.clone());
        state.current_path = path;
        if let Some(path) = &mut state.current_path {
            path.advance();
            let State {
                current_path,
                target_location,
                ..
            } = &mut *state;
            navigate_to_next_path_node(current_path, target_location);
        }
    }

    fn find_new_target(dragon: &EnderDragonEntity, state: &mut State, outer_ring: bool) {
        if state.current_path.as_ref().is_none_or(Path::is_done) {
            let current_node = dragon.find_closest_node();
            let mut target_node = current_node as i32;
            if rand::random_range(0..8) == 0 {
                state.holding_pattern_clockwise = !state.holding_pattern_clockwise;
                target_node += 6;
            }
            target_node += if state.holding_pattern_clockwise {
                1
            } else {
                -1
            };
            let target_node = wrap_node_index(target_node, outer_ring);
            state.current_path = dragon.find_path(current_node, target_node, None);
            if let Some(path) = &mut state.current_path {
                path.advance();
            }
        }
        navigate_to_next_path_node(&mut state.current_path, &mut state.target_location);
    }

    /// Returns where and in which direction to fire once charged and facing the target.
    fn tick_fireball(
        dragon: &EnderDragonEntity,
        state: &mut State,
        target: &Arc<Player>,
    ) -> Option<(Vector3<f64>, Vector3<f64>)> {
        let dragon_pos = dragon.position();
        let target_pos = target.get_entity().pos.load();
        if target_pos.squared_distance_to_vec(&dragon_pos) >= MAX_FIREBALL_DIST_SQR
            || !dragon.has_line_of_sight(target)
        {
            if state.fireball_charge > 0 {
                state.fireball_charge -= 1;
            }
            return None;
        }

        state.fireball_charge += 1;
        let aim = Vector3::new(
            target_pos.x - dragon_pos.x,
            0.0,
            target_pos.z - dragon_pos.z,
        )
        .vanilla_normalize();
        let angle = dragon.facing_angle_to(aim);
        if state.fireball_charge < FIREBALL_CHARGE_AMOUNT || !(0.0..10.0).contains(&angle) {
            return None;
        }

        let view = dragon.view_vector(dragon.mob_entity.living_entity.entity.pitch.load());
        let head = dragon.head_pos();
        let start = Vector3::new(head.x - view.x, dragon.head_y(0.5) + 0.5, head.z - view.z);
        let target_box = target.get_entity().bounding_box.load();
        let target_mid = target_pos.y + (target_box.max.y - target_box.min.y) * 0.5;
        let direction = Vector3::new(target_pos.x, target_mid, target_pos.z) - start;
        state.fireball_charge = 0;
        if let Some(path) = &mut state.current_path {
            while !path.is_done() {
                path.advance();
            }
        }
        Some((start, direction))
    }
}

impl Phase for StrafePlayerPhase {
    fn do_server_tick(&self, dragon: &EnderDragonEntity) {
        let outer_ring = dragon.alive_crystals().is_some_and(|c| c > 0);
        let mut state = self.state();
        let Some(target) = state.attack_target.clone() else {
            drop(state);
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
            return;
        };

        if state.current_path.as_ref().is_some_and(Path::is_done) {
            let target_pos = target.get_entity().pos.load();
            let dragon_pos = dragon.position();
            let dist = (target_pos - dragon_pos).horizontal_length();
            state.target_location = Some(Vector3::new(
                target_pos.x,
                target_pos.y + strafe_height_offset(dist),
                target_pos.z,
            ));
        }

        let dist = state
            .target_location
            .map_or(0.0, |t| t.squared_distance_to_vec(&dragon.position()));
        if !TARGET_DIST_SQR_RANGE.contains(&dist) {
            Self::find_new_target(dragon, &mut state, outer_ring);
        }

        if let Some((start, direction)) = Self::tick_fireball(dragon, &mut state, &target) {
            drop(state);
            let shooter = &dragon.mob_entity.living_entity.entity;
            if !shooter.is_silent() {
                shooter.world.load().sync_world_event(
                    WorldEvent::SoundDragonFireball,
                    shooter.block_pos.load(),
                    0,
                );
            }
            // Vanilla normalizes here and again in `assignDirectionalMovement`.
            dragon_fireball::shoot(shooter, start, direction.vanilla_normalize());
            dragon.set_phase(EnderDragonPhase::HoldingPattern);
        }
    }

    fn begin(&self, _dragon: &EnderDragonEntity) {
        let mut state = self.state();
        state.fireball_charge = 0;
        state.target_location = None;
        state.current_path = None;
        state.attack_target = None;
    }

    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        self.state().target_location
    }
}
