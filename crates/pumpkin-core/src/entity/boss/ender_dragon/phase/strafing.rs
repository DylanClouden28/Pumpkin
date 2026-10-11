use super::EnderDragonPhase;
use crate::entity::EntityBase;
use crate::entity::{
    boss::ender_dragon::{EnderDragonEntity, Vector3Ext, find_path},
    projectile::dragon_fireball,
};
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::Ordering;

pub struct StrafingPhase;

impl super::Phase for StrafingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Strafing
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        *dragon
            .fireball_charge
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
        *dragon
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    #[expect(clippy::too_many_lines)]
    fn tick(&self, dragon: &EnderDragonEntity) {
        let target_id = *dragon
            .target_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let world = dragon.mob_entity.living_entity.entity.world.load();
        let pos = dragon.mob_entity.living_entity.entity.pos.load();

        let player_target = if let Some(id) = target_id {
            world
                .players
                .load()
                .iter()
                .find(|p| p.gameprofile.id == id)
                .cloned()
        } else {
            None
        };

        let Some(player) = player_target else {
            dragon.set_phase(EnderDragonPhase::Circling);
            return;
        };

        let player_pos = player.get_entity().pos.load();
        let mut path = dragon
            .path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut target_location = dragon
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if path.is_empty() {
            let d2 = player_pos.x - pos.x;
            let d3 = player_pos.z - pos.z;
            let d4 = d2.hypot(d3);
            let d5 = (0.4 + d4 / 80.0 - 1.0).clamp(0.0, 10.0);
            *target_location = Some(Vector3::new(player_pos.x, player_pos.y + d5, player_pos.z));
        }

        let d11 = target_location
            .map(|loc| pos.distance_squared(loc))
            .unwrap_or(0.0);
        if !(100.0..=22500.0).contains(&d11)
            || dragon
                .mob_entity
                .living_entity
                .entity
                .horizontal_collision
                .load(Ordering::Relaxed)
        {
            if path.is_empty() {
                drop(path);
                let i = dragon.find_closest_node();
                let j = dragon.find_closest_node_to(player_pos);

                let mut path_lock = dragon
                    .path
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let nodes = dragon
                    .nodes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *path_lock = find_path(&nodes, i, j, None);
                drop(nodes);
                path = path_lock;
            }

            if let Some(next_node_idx) = path.first().copied() {
                path.remove(0);
                let nodes = dragon
                    .nodes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(node) = nodes[next_node_idx] {
                    let mut y_target = node.y + rand::random_range(0.0..20.0);
                    while y_target < node.y {
                        y_target = node.y + rand::random_range(0.0..20.0);
                    }
                    *target_location = Some(Vector3::new(node.x, y_target, node.z));
                }
            }
        }
        drop(path);
        drop(target_location);

        if player_pos.distance_squared(pos) < 4096.0 {
            let mut charge = dragon
                .fireball_charge
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            let aim_diff = player_pos - pos;
            let aim = if aim_diff.length_squared() > 1e-6 {
                aim_diff.normalize()
            } else {
                Vector3::new(0.0, 0.0, 0.0)
            };

            let yaw = dragon.mob_entity.living_entity.entity.yaw.load();
            let dir = Vector3::new(
                (yaw * (std::f32::consts::PI / 180.0)).sin() as f64,
                0.0,
                -(yaw * (std::f32::consts::PI / 180.0)).cos() as f64,
            );

            let dir_norm = if dir.length_squared() > 1e-6 {
                dir.normalize()
            } else {
                Vector3::new(0.0, 0.0, 0.0)
            };

            let dot = dir_norm.dot(&aim) as f32;
            let angle_degs = dot.acos().to_degrees() + 0.5;

            *charge += 1;

            if *charge >= 5 && angle_degs < 10.0 {
                *charge = 0;
                drop(charge);

                // Vanilla `DragonStrafePlayerPhase`: launch from just behind the head toward the
                // middle of the target.
                let head = dragon.parts[0].entity.pos.load();
                let yaw = dragon
                    .mob_entity
                    .living_entity
                    .entity
                    .yaw
                    .load()
                    .to_radians();
                let (view_x, view_z) = (-f64::from(yaw.sin()), f64::from(yaw.cos()));
                let start = Vector3::new(head.x - view_x, head.y + 0.5 + 0.5, head.z - view_z);
                let target_box = player.get_entity().bounding_box.load();
                let target_height = target_box.max.y - target_box.min.y;
                let direction = Vector3::new(
                    player_pos.x,
                    player_pos.y + target_height * 0.5,
                    player_pos.z,
                ) - start;
                let shooter = &dragon.mob_entity.living_entity.entity;
                if !shooter.is_silent() {
                    world.sync_world_event(
                        WorldEvent::SoundDragonFireball,
                        shooter.block_pos.load(),
                        0,
                    );
                }
                dragon_fireball::shoot(shooter, start, direction);

                dragon
                    .path
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clear();
                dragon.set_phase(EnderDragonPhase::Circling);
            }
        } else {
            let mut charge = dragon
                .fireball_charge
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *charge > 0 {
                *charge -= 1;
            }
        }
    }
}
