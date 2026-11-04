//! The effect player: effect timelines evaluated on the
//! fixed step. A key fires exactly once, at its absolute
//! step; an instance's particles are a pure function of
//! the key's seed, the particle index, and the instance
//! age (counter-based splitmix64 randomness — no RNG
//! state, so replays and restores are exact).
//!
//! Runtime law (SPEC section 4.2): strobe keys are capped
//! at [`FLASH_MAX_PER_SECOND`] in any one-second window.
//! The cap is enforced when a timeline loads — a timeline
//! that would break it is refused (LE-106), never clipped
//! at runtime.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::EngineError;
use crate::record::{EffectKind, EffectRecord};

/// Maximum strobe (flash) events in any one-second
/// window. Photosensitivity law, adopted as a runtime
/// invariant.
pub const FLASH_MAX_PER_SECOND: usize = 3;

/// An effect spawn event, emitted on the step a key fires.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EffectEvent {
    /// Timeline identifier.
    pub effect_id: String,
    /// Key index within the timeline.
    pub key_index: usize,
    /// Particle pattern.
    pub kind: EffectKind,
    /// Particles in the instance.
    pub count: u32,
    /// Instance seed.
    pub seed: u64,
    /// Emitter position (scene units).
    pub at: [f64; 2],
}

/// One step of splitmix64: returns the advanced state and
/// the mixed output. Counter-based and platform-exact
/// (wrapping u64 arithmetic only).
pub fn splitmix64(state: u64) -> (u64, u64) {
    let next = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = next;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (next, z ^ (z >> 31))
}

/// A uniform draw in [0, 1) derived from `(seed, index)`.
fn unit(seed: u64, index: u32) -> f64 {
    let (_, mixed) = splitmix64(seed ^ (u64::from(index) << 32) ^ 0xA076_1D64_78BD_642F);
    (mixed >> 11) as f64 / (1u64 << 53) as f64
}

/// The position of particle `index` of an instance at age
/// `age_steps` — a pure function; hosts and gates compute
/// the same positions from the same event. Motion is
/// deliberately simple (radial for Burst, circular for
/// Ring, Archimedean for Spiral): the player's contract
/// is determinism, not artistry.
pub fn particle_position(kind: EffectKind, seed: u64, index: u32, age_steps: u64) -> [f64; 2] {
    let angle = unit(seed, index) * std::f64::consts::TAU;
    let age = age_steps as f64;
    match kind {
        EffectKind::Burst => {
            let speed = 0.5 + unit(seed ^ 0x51_7C_C1_B7, index);
            [angle.cos() * speed * age, angle.sin() * speed * age]
        }
        EffectKind::Ring => {
            let radius = 2.0 * age;
            [angle.cos() * radius, angle.sin() * radius]
        }
        EffectKind::Spiral => {
            let turned = angle + age * 0.1;
            let radius = 0.5 * age;
            [turned.cos() * radius, turned.sin() * radius]
        }
    }
}

/// The effect player state: the timelines plus the set of
/// keys already fired (for state records).
#[derive(Clone, Debug)]
pub struct EffectPlayer {
    records: Vec<EffectRecord>,
    fired: BTreeSet<(usize, usize)>,
}

impl EffectPlayer {
    /// Build from records, enforcing the flash law at the
    /// scene's step `rate`: in any window of `rate`
    /// consecutive steps, at most
    /// [`FLASH_MAX_PER_SECOND`] strobe keys may fire.
    pub fn new(records: &[EffectRecord], rate: u32) -> Result<Self, EngineError> {
        for record in records {
            if record.life_steps == 0 {
                return Err(EngineError::Effect(format!(
                    "effect {}: life_steps must be at least 1",
                    record.id
                )));
            }
            for key in &record.keys {
                if key.count == 0 {
                    return Err(EngineError::Effect(format!(
                        "effect {}: a key spawns zero particles",
                        record.id
                    )));
                }
            }
        }
        // Flash law over absolute fire steps.
        let mut strobes: Vec<u64> = records
            .iter()
            .flat_map(|r| {
                r.keys
                    .iter()
                    .filter(|k| k.strobe)
                    .map(move |k| r.start_step + k.at_step)
            })
            .collect();
        strobes.sort_unstable();
        let window = u64::from(rate.max(1));
        for (i, &start) in strobes.iter().enumerate() {
            let in_window = strobes[i..]
                .iter()
                .take_while(|&&s| s < start + window)
                .count();
            if in_window > FLASH_MAX_PER_SECOND {
                return Err(EngineError::Effect(format!(
                    "flash law: {in_window} strobes in the second starting at step {start} \
                     (cap {FLASH_MAX_PER_SECOND})"
                )));
            }
        }
        Ok(Self {
            records: records.to_vec(),
            fired: BTreeSet::new(),
        })
    }

    /// Fire the keys scheduled at absolute step `step`.
    pub fn step(&mut self, step: u64) -> Vec<EffectEvent> {
        let mut events = Vec::new();
        for (ri, record) in self.records.iter().enumerate() {
            if step < record.start_step {
                continue;
            }
            let local = step - record.start_step;
            for (ki, key) in record.keys.iter().enumerate() {
                if key.at_step == local && !self.fired.contains(&(ri, ki)) {
                    self.fired.insert((ri, ki));
                    events.push(EffectEvent {
                        effect_id: record.id.clone(),
                        key_index: ki,
                        kind: key.kind,
                        count: key.count,
                        seed: key.seed,
                        at: [key.x, key.y],
                    });
                }
            }
        }
        events
    }

    /// Particles live at absolute step `step` across all
    /// timelines (instances whose age is within their
    /// life).
    pub fn live_particles(&self, step: u64) -> u64 {
        let mut total = 0u64;
        for record in &self.records {
            if step < record.start_step {
                continue;
            }
            let local = step - record.start_step;
            for key in &record.keys {
                if key.at_step <= local && local - key.at_step < record.life_steps {
                    total += u64::from(key.count);
                }
            }
        }
        total
    }

    /// Fired key identifiers ("effect_id:key_index") in
    /// sorted order, for the state record.
    pub fn fired_ids(&self) -> Vec<String> {
        self.fired
            .iter()
            .map(|(ri, ki)| format!("{}:{}", self.records[*ri].id, ki))
            .collect()
    }

    /// Restore the fired set from state-record identifiers.
    /// Unknown identifiers are refused (LE-106).
    pub fn restore_fired(&mut self, ids: &[String]) -> Result<(), EngineError> {
        let mut set = BTreeSet::new();
        for id in ids {
            let (effect_id, key_index) = id
                .rsplit_once(':')
                .ok_or_else(|| EngineError::Effect(format!("bad fired id '{id}'")))?;
            let key_index: usize = key_index
                .parse()
                .map_err(|_| EngineError::Effect(format!("bad fired id '{id}'")))?;
            let ri = self
                .records
                .iter()
                .position(|r| r.id == effect_id)
                .ok_or_else(|| EngineError::Effect(format!("unknown effect '{effect_id}'")))?;
            if key_index >= self.records[ri].keys.len() {
                return Err(EngineError::Effect(format!(
                    "fired key {key_index} beyond effect '{effect_id}'"
                )));
            }
            set.insert((ri, key_index));
        }
        self.fired = set;
        Ok(())
    }
}
