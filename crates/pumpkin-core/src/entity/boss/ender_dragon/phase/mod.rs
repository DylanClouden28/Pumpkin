use crate::entity::ai::pathfinder::path::Path;
use crate::entity::boss::ender_dragon::{CENTER_RING_START, EnderDragonEntity, INNER_RING_START};
use crate::entity::player::Player;
use pumpkin_data::damage::DamageType;
use pumpkin_util::math::vector3::Vector3;
use std::ops::RangeInclusive;
use std::sync::Arc;

mod charging_player;
mod dying;
mod holding_pattern;
mod hover;
mod landing;
mod landing_approach;
mod sitting_attacking;
mod sitting_flaming;
mod sitting_scanning;
mod strafe_player;
mod takeoff;

pub use charging_player::ChargingPlayerPhase;
pub use dying::DyingPhase;
pub use holding_pattern::HoldingPatternPhase;
pub use hover::HoverPhase;
pub use landing::LandingPhase;
pub use landing_approach::LandingApproachPhase;
pub use sitting_attacking::SittingAttackingPhase;
pub use sitting_flaming::SittingFlamingPhase;
pub use sitting_scanning::SittingScanningPhase;
pub use strafe_player::StrafePlayerPhase;
pub use takeoff::TakeoffPhase;

/// Vanilla `DragonPhaseInstance`.
pub trait Phase: Send + Sync {
    fn do_server_tick(&self, dragon: &EnderDragonEntity);
    fn begin(&self, _dragon: &EnderDragonEntity) {}
    fn end(&self, _dragon: &EnderDragonEntity) {}
    fn is_sitting(&self) -> bool {
        false
    }
    fn get_fly_speed(&self) -> f32 {
        0.6
    }
    fn get_turn_speed(&self, dragon: &EnderDragonEntity) -> f32 {
        let rot_speed = dragon.horizontal_speed() + 1.0;
        let dist = rot_speed.min(40.0);
        0.7 / dist / rot_speed
    }
    fn get_fly_target_location(&self) -> Option<Vector3<f64>> {
        None
    }
    fn on_hurt(&self, _damage_type: DamageType, damage: f32) -> f32 {
        damage
    }
    fn on_crystal_destroyed(&self, _dragon: &EnderDragonEntity, _player: Option<&Arc<Player>>) {}
}

/// Vanilla `EnderDragonPhase` ids, which the client reads from the dragon's phase data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnderDragonPhase {
    HoldingPattern = 0,
    StrafePlayer = 1,
    LandingApproach = 2,
    Landing = 3,
    Takeoff = 4,
    SittingFlaming = 5,
    SittingScanning = 6,
    SittingAttacking = 7,
    ChargingPlayer = 8,
    Dying = 9,
    Hover = 10,
}

impl EnderDragonPhase {
    #[must_use]
    pub const fn network_id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn from_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => Self::HoldingPattern,
            1 => Self::StrafePlayer,
            2 => Self::LandingApproach,
            3 => Self::Landing,
            4 => Self::Takeoff,
            5 => Self::SittingFlaming,
            6 => Self::SittingScanning,
            7 => Self::SittingAttacking,
            8 => Self::ChargingPlayer,
            9 => Self::Dying,
            10 => Self::Hover,
            _ => return None,
        })
    }
}

/// Vanilla's squared distances within which a phase keeps its fly target.
pub const TARGET_DIST_SQR_RANGE: RangeInclusive<f64> = 100.0..=22500.0;

/// Holds one instance of every phase, vanilla `EnderDragonPhaseManager`.
#[derive(Default)]
pub struct PhaseManager {
    pub holding_pattern: HoldingPatternPhase,
    pub strafe_player: StrafePlayerPhase,
    pub landing_approach: LandingApproachPhase,
    pub landing: LandingPhase,
    pub takeoff: TakeoffPhase,
    pub sitting_flaming: SittingFlamingPhase,
    pub sitting_scanning: SittingScanningPhase,
    pub sitting_attacking: SittingAttackingPhase,
    pub charging_player: ChargingPlayerPhase,
    pub dying: DyingPhase,
    pub hover: HoverPhase,
}

impl PhaseManager {
    #[must_use]
    pub fn get_phase(&self, phase: EnderDragonPhase) -> &dyn Phase {
        match phase {
            EnderDragonPhase::HoldingPattern => &self.holding_pattern,
            EnderDragonPhase::StrafePlayer => &self.strafe_player,
            EnderDragonPhase::LandingApproach => &self.landing_approach,
            EnderDragonPhase::Landing => &self.landing,
            EnderDragonPhase::Takeoff => &self.takeoff,
            EnderDragonPhase::SittingFlaming => &self.sitting_flaming,
            EnderDragonPhase::SittingScanning => &self.sitting_scanning,
            EnderDragonPhase::SittingAttacking => &self.sitting_attacking,
            EnderDragonPhase::ChargingPlayer => &self.charging_player,
            EnderDragonPhase::Dying => &self.dying,
            EnderDragonPhase::Hover => &self.hover,
        }
    }
}

/// Vanilla `AbstractDragonSittingPhase.onHurt`; tridents are arrows in vanilla.
#[must_use]
pub fn sitting_on_hurt(damage_type: DamageType, damage: f32) -> f32 {
    if damage_type == DamageType::ARROW
        || damage_type == DamageType::TRIDENT
        || damage_type == DamageType::WIND_CHARGE
    {
        0.0
    } else {
        damage
    }
}

/// Vanilla `navigateToNextPathNode`.
pub fn navigate_to_next_path_node(
    path: &mut Option<Path>,
    target_location: &mut Option<Vector3<f64>>,
) {
    if let Some(path) = path
        && let Some(current) = path.get_next_node_pos()
    {
        path.advance();
        *target_location = Some(Vector3::new(
            f64::from(current.0.x),
            random_node_y(current.0.y),
            f64::from(current.0.z),
        ));
    }
}

/// Vanilla `node.y + nextFloat() * 20` in float precision; its retry loop never repeats.
#[must_use]
pub fn random_node_y(node_y: i32) -> f64 {
    f64::from(node_y as f32 + rand::random::<f32>() * 20.0)
}

/// Vanilla wraps a node index into the outer ring (0..12) or the inner ring (12..20).
#[must_use]
pub const fn wrap_node_index(index: i32, outer_ring: bool) -> usize {
    let inner_start = INNER_RING_START as i32;
    if outer_ring {
        index.rem_euclid(inner_start) as usize
    } else {
        ((index - inner_start).rem_euclid(CENTER_RING_START as i32 - inner_start) + inner_start)
            as usize
    }
}
