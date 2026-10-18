//! Ramp/toon material configuration as data (Phase 5).
//!
//! Contract summary:
//! - [`ToonConfig`] is the 3-band ramp configuration from the factory
//!   plan's T14 note: two ramp thresholds partition the lit value
//!   into shadow / mid / highlight bands, the shadow and highlight
//!   bands carry tints, and a rim-light strength is recorded for
//!   consumers that implement rim lighting.
//! - The configuration is data, not code: a build with the ramp
//!   option writes it into the GLB mesh `extras` under the key
//!   `lumen_toon`, and the Blender render proof
//!   (`blender/render_proof.py`) reads it back from the file to build
//!   its ramp node material. Nothing downstream hardcodes it.
//! - [`ToonConfig::validate`] fails closed: thresholds must be
//!   finite, ordered, and inside `[0.0, 1.0]`; the rim strength must
//!   be finite inside `[0.0, 1.0]`; both tints must be `#rrggbb`.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::FactoryError;
use crate::params::hex_to_rgb;

/// Default shadow-band threshold (T14 3-band ramp).
pub const DEFAULT_RAMP_LOW: f32 = 0.35;
/// Default highlight-band threshold (T14 3-band ramp).
pub const DEFAULT_RAMP_HIGH: f32 = 0.70;
/// Default rim-light strength.
pub const DEFAULT_RIM_STRENGTH: f32 = 0.35;
/// Default shadow tint (`#rrggbb`).
pub const DEFAULT_SHADOW_TINT: &str = "#2a2438";
/// Default highlight tint (`#rrggbb`).
pub const DEFAULT_HIGHLIGHT_TINT: &str = "#ffffff";
/// GLB mesh-extras key carrying the serialized configuration.
pub const TOON_EXTRAS_KEY: &str = "lumen_toon";

/// Ramp/toon material configuration (see module docs).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToonConfig {
    /// Tint applied to the highlight band (`#rrggbb`).
    pub highlight_tint: String,
    /// `[shadow, highlight]` band thresholds in lit-value space.
    pub ramp_thresholds: [f32; 2],
    /// Rim-light strength, `0.0..=1.0`.
    pub rim_strength: f32,
    /// Tint applied to the shadow band (`#rrggbb`).
    pub shadow_tint: String,
}

impl Default for ToonConfig {
    fn default() -> Self {
        Self {
            highlight_tint: DEFAULT_HIGHLIGHT_TINT.to_string(),
            ramp_thresholds: [DEFAULT_RAMP_LOW, DEFAULT_RAMP_HIGH],
            rim_strength: DEFAULT_RIM_STRENGTH,
            shadow_tint: DEFAULT_SHADOW_TINT.to_string(),
        }
    }
}

impl ToonConfig {
    /// Fail-closed validation (see module docs for the rules).
    pub fn validate(&self) -> Result<(), FactoryError> {
        let [low, high] = self.ramp_thresholds;
        if !low.is_finite() || !high.is_finite() {
            return Err(FactoryError::BadParam(format!(
                "ramp thresholds must be finite, got {:?}",
                self.ramp_thresholds
            )));
        }
        if !(0.0..=1.0).contains(&low) || !(0.0..=1.0).contains(&high) {
            return Err(FactoryError::BadParam(format!(
                "ramp thresholds must lie in [0.0, 1.0], got {:?}",
                self.ramp_thresholds
            )));
        }
        if low >= high {
            return Err(FactoryError::BadParam(format!(
                "ramp thresholds must be strictly increasing, got {:?}",
                self.ramp_thresholds
            )));
        }
        if !self.rim_strength.is_finite() || !(0.0..=1.0).contains(&self.rim_strength) {
            return Err(FactoryError::BadParam(format!(
                "rim strength must be finite in [0.0, 1.0], got {}",
                self.rim_strength
            )));
        }
        hex_to_rgb(&self.shadow_tint)
            .map_err(|e| FactoryError::BadParam(format!("shadow_tint: {e}")))?;
        hex_to_rgb(&self.highlight_tint)
            .map_err(|e| FactoryError::BadParam(format!("highlight_tint: {e}")))?;
        Ok(())
    }

    /// Serialize to the JSON value stored under [`TOON_EXTRAS_KEY`].
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Parse back from a JSON value (the inverse of [`Self::to_json`]),
    /// validating on the way in.
    pub fn from_json(value: &Value) -> Result<Self, FactoryError> {
        let config: Self = serde_json::from_value(value.clone())
            .map_err(|e| FactoryError::BadParam(format!("lumen_toon config: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Shadow tint as linear-ish `[r, g, b]` in `0.0..=1.0` (channel
    /// values are the hex bytes divided by 255; consumers doing color
    /// management convert as their pipeline requires).
    pub fn shadow_rgb(&self) -> Result<[f32; 3], FactoryError> {
        Ok(hex_to_rgb(&self.shadow_tint)?.map(|c| f32::from(c) / 255.0))
    }

    /// Highlight tint as `[r, g, b]` in `0.0..=1.0` (see
    /// [`Self::shadow_rgb`]).
    pub fn highlight_rgb(&self) -> Result<[f32; 3], FactoryError> {
        Ok(hex_to_rgb(&self.highlight_tint)?.map(|c| f32::from(c) / 255.0))
    }
}
