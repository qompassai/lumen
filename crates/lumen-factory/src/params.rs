//! Parameter records: token-IR-shaped canonical data that drives
//! the build (Phase 3, the REDox/T15 pattern — one schema, many
//! serializations).
//!
//! Contract summary:
//! - [`ParameterRecord`] (schema `lumen.factory-params`, version 1)
//!   is the canonical parameter data for one companion: proportion
//!   band scales plus the optional per-character frame fields (a
//!   bust delta and per-band width/depth scale sets, all absent =
//!   the canonical base frame), appearance palette assignments
//!   (`#rrggbb` strings, or `null` = take the color from the
//!   identity record), a hair length scalar, and a face eye-scale
//!   scalar. It serializes as
//!   canonical JSON (sorted keys, fixed-precision floats — the same
//!   discipline as `identity.rs`) and as TOML (the `toml` crate,
//!   pinned `=0.8.23`, the same pin as lumen main); both forms parse
//!   back to the same record. `record_hash` is the SHA-256 of the
//!   canonical JSON with the hash field itself empty, exactly like
//!   the identity record.
//! - [`ParameterRecord::validate`] fails closed: unknown fields are
//!   rejected at parse time (`deny_unknown_fields`), scales must be
//!   finite and inside [`SCALE_MIN`]..=[`SCALE_MAX`] (the Phase 2
//!   range owned by `identity.rs` — referenced here, not
//!   re-decided), colors must be well-formed `#rrggbb`, hair length
//!   must lie in `0.0..=1.0`, and eye scale in `0.8..=1.3`.
//! - [`ParameterRecord::from_identity`] derives a record from an
//!   [`IdentityRecord`]: proportions are the record's applied
//!   scales (the single source of truth,
//!   [`IdentityRecord::applied_scales`]), appearance comes from the
//!   palette anchors, hair length is the record's measured hair mass
//!   ([`IdentityRecord::hair_length`], projection v2), and eye scale
//!   defaults to [`DEFAULT_EYE_SCALE`]. Building Séraphine from her
//!   derived record reproduces `assets/seraphine.glb` byte-for-byte
//!   (the regression anchor, asserted in tests and in the real gate).
//! - [`ParameterRecord::from_identity_with_vrm_proportions`] is the
//!   same derivation with the proportion source swapped: the scales
//!   come from a VRM humanoid skeleton (the VRoid input path, see
//!   `vrm.rs`) through the same [`scales_for_ratios`] law, while
//!   appearance still comes from the identity record — a VRM carries
//!   no palette authority.

#![forbid(unsafe_code)]

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::identity::{BandScaleSet, IdentityRecord, SCALE_MAX, SCALE_MIN, scales_for_ratios};
use crate::{FactoryError, sha256_hex};

/// Schema identifier emitted in every record.
pub const PARAMS_SCHEMA: &str = "lumen.factory-params";
/// Schema version emitted in every record.
pub const PARAMS_VERSION: u32 = 1;

/// Eye scale applied by [`ParameterRecord::from_identity`].
pub const DEFAULT_EYE_SCALE: f64 = 1.0;
/// Minimum accepted hair length.
pub const HAIR_LENGTH_MIN: f64 = 0.0;
/// Maximum accepted hair length.
pub const HAIR_LENGTH_MAX: f64 = 1.0;
/// Minimum accepted eye scale.
pub const EYE_SCALE_MIN: f64 = 0.8;
/// Maximum accepted eye scale.
pub const EYE_SCALE_MAX: f64 = 1.3;
/// Minimum accepted bust delta (bust − underbust circumference),
/// cm — the f64 view of the range `base.rs` owns.
pub const BUST_DELTA_MIN_CM: f64 = crate::base::BUST_DELTA_MIN_CM as f64;
/// Maximum accepted bust delta (bust − underbust circumference), cm.
pub const BUST_DELTA_MAX_CM: f64 = crate::base::BUST_DELTA_MAX_CM as f64;

/// Proportion band scales (the applied scales of the projection).
///
/// Field order is load-bearing for the TOML serialization: the
/// scalar fields come first, the optional [`BandScaleSet`] tables
/// last (TOML emits values before tables).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProportionParams {
    /// Head band scale, finite in [SCALE_MIN, SCALE_MAX].
    pub head_scale: f64,
    /// Leg band scale, finite in [SCALE_MIN, SCALE_MAX].
    pub leg_scale: f64,
    /// Torso band scale, finite in [SCALE_MIN, SCALE_MAX].
    pub torso_scale: f64,
    /// Bust delta (bust − underbust circumference) in centimetres,
    /// finite in [BUST_DELTA_MIN_CM, BUST_DELTA_MAX_CM] when
    /// present. `None` reproduces the canonical base torso exactly;
    /// the field is omitted from serialization when `None`, so
    /// records written before this parameter existed keep
    /// identical bytes and hashes (GAP-11-3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bust_delta_cm: Option<f64>,
    /// Per-band depth (Z) scales, or `None` for the locked base
    /// depths (every band exactly 1.0); omitted from serialization
    /// when `None` (GAP-11-4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_scales: Option<BandScaleSet>,
    /// Per-band width (X) scales, or `None` for the locked base
    /// widths (every band exactly 1.0); omitted from serialization
    /// when `None` (GAP-11-4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_scales: Option<BandScaleSet>,
}

/// Named bust grade for a bust delta in centimetres, on the
/// Japanese-anchored scale (2.5 cm steps: A = 10, B = 12.5, C = 15,
/// D = 17.5, E = 20, F = 22.5, G = 25), each grade spanning ±1.25 cm
/// around its centre; a delta exactly on a seam belongs to the lower
/// grade. Returns `None` outside the graded band.
pub fn bust_grade(delta_cm: f64) -> Option<char> {
    const CENTRES: [(f64, char); 7] = [
        (10.0, 'A'),
        (12.5, 'B'),
        (15.0, 'C'),
        (17.5, 'D'),
        (20.0, 'E'),
        (22.5, 'F'),
        (25.0, 'G'),
    ];
    CENTRES
        .iter()
        .find(|(centre, _)| (delta_cm - centre).abs() <= 1.25)
        .map(|(_, grade)| *grade)
}

/// Appearance palette assignments. Each field is a `#rrggbb` color
/// or `null`, where `null` means "take this color from the identity
/// record" at build time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceParams {
    /// Eye (iris) color override.
    pub eyes: Option<String>,
    /// Hair color override.
    pub hair: Option<String>,
    /// Lip color override.
    pub lips: Option<String>,
    /// Outfit primary color override.
    pub outfit_primary: Option<String>,
    /// Outfit secondary color override.
    pub outfit_secondary: Option<String>,
    /// Skin color override.
    pub skin: Option<String>,
}

/// Hair parameters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HairParams {
    /// Hair length, finite in [HAIR_LENGTH_MIN, HAIR_LENGTH_MAX]: how
    /// far the hair tail reaches below the neck, as a fraction of the
    /// torso band (see `base::ScaledBuild::hair_length`).
    pub length: f64,
}

/// Face parameters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaceParams {
    /// Eye scale for texture painting, finite in
    /// [EYE_SCALE_MIN, EYE_SCALE_MAX].
    pub eye_scale: f64,
}

/// The canonical parameter record (schema `lumen.factory-params` v1).
///
/// Field order is load-bearing for the TOML serialization: scalar
/// fields come first (TOML emits values before tables), then the
/// table fields in alphabetical order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterRecord {
    /// Companion name (must match the identity record at build time).
    pub companion: String,
    /// SHA-256 of the canonical JSON with this field empty.
    pub record_hash: String,
    /// Schema identifier ([`PARAMS_SCHEMA`]).
    pub schema: String,
    /// Schema version ([`PARAMS_VERSION`]).
    pub version: u32,
    /// Appearance palette assignments.
    pub appearance: AppearanceParams,
    /// Face parameters.
    pub face: FaceParams,
    /// Hair parameters.
    pub hair: HairParams,
    /// Proportion band scales.
    pub proportions: ProportionParams,
}

/// Parse a `#rrggbb` color (either case) into channels. Fails closed
/// on any other shape.
pub fn hex_to_rgb(value: &str) -> Result<[u8; 3], FactoryError> {
    let digits = value.strip_prefix('#').ok_or_else(|| {
        FactoryError::BadParam(format!("color {value:?} is not #rrggbb (missing '#')"))
    })?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(FactoryError::BadParam(format!(
            "color {value:?} is not #rrggbb (six hex digits)"
        )));
    }
    let parse_pair = |pair: &str| -> Result<u8, FactoryError> {
        u8::from_str_radix(pair, 16)
            .map_err(|e| FactoryError::BadParam(format!("color {value:?} channel: {e}")))
    };
    Ok([
        parse_pair(&digits[0..2])?,
        parse_pair(&digits[2..4])?,
        parse_pair(&digits[4..6])?,
    ])
}

/// Write a `serde_json::Value` in canonical form: object keys
/// sorted, no insignificant whitespace, floats at fixed 12-decimal
/// precision (trailing zeros trimmed) — the same discipline as
/// `identity.rs`, so canonical bytes are a pure function of the
/// stored values and hashes survive a parse round trip.
fn canonical_write(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else if let Some(f) = n.as_f64() {
                let fixed = format!("{f:.12}");
                let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
                out.push_str(if trimmed.is_empty() || trimmed == "-" {
                    "0"
                } else {
                    trimmed
                });
            } else {
                out.push('0');
            }
        }
        serde_json::Value::String(s) => {
            out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()));
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                canonical_write(item, out);
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (idx, key) in keys.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string()));
                out.push(':');
                canonical_write(&map[*key], out);
            }
            out.push('}');
        }
    }
}

impl ParameterRecord {
    /// Derive a parameter record from an identity record:
    /// proportions from the record's applied scales, appearance from
    /// its palette anchors, hair length and eye scale at their
    /// defaults. The derived record carries a fresh `record_hash`.
    pub fn from_identity(record: &IdentityRecord) -> Result<Self, FactoryError> {
        Self::derive(record, record.applied_scales()?)
    }

    /// Derive a parameter record like [`Self::from_identity`], but
    /// take the body proportions from a VRM humanoid skeleton
    /// ([`crate::vrm::VrmProportions`], measured by
    /// [`crate::vrm::inspect_vrm`] — the VRoid input path) instead of
    /// the identity record's sprite measurements. Appearance, hair,
    /// and face parameters still come from the identity record, and
    /// the scales are computed by the same law as the identity path
    /// ([`scales_for_ratios`]), so both proportion sources produce
    /// records in one scale space. Fails closed on degenerate VRM
    /// ratios, exactly like the identity path.
    pub fn from_identity_with_vrm_proportions(
        record: &IdentityRecord,
        proportions: &crate::vrm::VrmProportions,
    ) -> Result<Self, FactoryError> {
        let scales = scales_for_ratios(
            f64::from(proportions.head_ratio),
            f64::from(proportions.torso_ratio),
            f64::from(proportions.leg_ratio),
        )?;
        Self::derive(record, scales)
    }

    fn derive(
        record: &IdentityRecord,
        (head, torso, leg): (f32, f32, f32),
    ) -> Result<Self, FactoryError> {
        let outfit = &record.palette.outfit;
        let mut derived = Self {
            appearance: AppearanceParams {
                eyes: record.palette.eye.as_ref().map(|c| c.hex.clone()),
                hair: Some(record.palette.hair.hex.clone()),
                lips: record.palette.lips.as_ref().map(|c| c.hex.clone()),
                outfit_primary: outfit.first().map(|c| c.hex.clone()),
                outfit_secondary: outfit.get(1).map(|c| c.hex.clone()),
                skin: Some(record.palette.skin.hex.clone()),
            },
            companion: record.companion.clone(),
            face: FaceParams {
                eye_scale: DEFAULT_EYE_SCALE,
            },
            hair: HairParams {
                length: record.hair_length()?,
            },
            proportions: ProportionParams {
                head_scale: head as f64,
                leg_scale: leg as f64,
                torso_scale: torso as f64,
                // The identity record's authored frame fields ride
                // through derivation unchanged (validated below);
                // an identity without them derives the locked base
                // frame, exactly as before they existed.
                bust_delta_cm: record.bust_delta_cm()?,
                depth_scales: record.depth_scales,
                width_scales: record.width_scales,
            },
            record_hash: String::new(),
            schema: PARAMS_SCHEMA.to_string(),
            version: PARAMS_VERSION,
        };
        derived.record_hash = derived.compute_hash()?;
        derived.validate()?;
        Ok(derived)
    }

    /// Validate the record's schema, ranges, and color shapes.
    /// Fails closed on the first violation; nothing is clamped.
    pub fn validate(&self) -> Result<(), FactoryError> {
        if self.schema != PARAMS_SCHEMA || self.version != PARAMS_VERSION {
            return Err(FactoryError::BadParam(format!(
                "unsupported params schema {} v{}",
                self.schema, self.version
            )));
        }
        if self.companion.is_empty() {
            return Err(FactoryError::BadParam(
                "params companion name is empty".to_string(),
            ));
        }
        let scale_range = SCALE_MIN as f64..=SCALE_MAX as f64;
        for (name, scale) in [
            ("head_scale", self.proportions.head_scale),
            ("torso_scale", self.proportions.torso_scale),
            ("leg_scale", self.proportions.leg_scale),
        ] {
            if !scale.is_finite() || !scale_range.contains(&scale) {
                return Err(FactoryError::BadParam(format!(
                    "{name} {scale} outside [{SCALE_MIN}, {SCALE_MAX}] (fail closed, not clamped)"
                )));
            }
        }
        if let Some(delta) = self
            .proportions
            .bust_delta_cm
            .filter(|d| !d.is_finite() || !(BUST_DELTA_MIN_CM..=BUST_DELTA_MAX_CM).contains(d))
        {
            return Err(FactoryError::BadParam(format!(
                "bust_delta_cm {delta} outside [{BUST_DELTA_MIN_CM}, {BUST_DELTA_MAX_CM}] (fail closed, not clamped)"
            )));
        }
        if let Some(set) = &self.proportions.width_scales {
            set.validate("proportions.width_scales")?;
        }
        if let Some(set) = &self.proportions.depth_scales {
            set.validate("proportions.depth_scales")?;
        }
        if !self.hair.length.is_finite()
            || !(HAIR_LENGTH_MIN..=HAIR_LENGTH_MAX).contains(&self.hair.length)
        {
            return Err(FactoryError::BadParam(format!(
                "hair length {} outside [{HAIR_LENGTH_MIN}, {HAIR_LENGTH_MAX}]",
                self.hair.length
            )));
        }
        if !self.face.eye_scale.is_finite()
            || !(EYE_SCALE_MIN..=EYE_SCALE_MAX).contains(&self.face.eye_scale)
        {
            return Err(FactoryError::BadParam(format!(
                "eye_scale {} outside [{EYE_SCALE_MIN}, {EYE_SCALE_MAX}]",
                self.face.eye_scale
            )));
        }
        for (field, value) in [
            ("eyes", &self.appearance.eyes),
            ("hair", &self.appearance.hair),
            ("lips", &self.appearance.lips),
            ("outfit_primary", &self.appearance.outfit_primary),
            ("outfit_secondary", &self.appearance.outfit_secondary),
            ("skin", &self.appearance.skin),
        ] {
            if let Some(hex) = value {
                hex_to_rgb(hex)
                    .map_err(|e| FactoryError::BadParam(format!("appearance.{field}: {e}")))?;
            }
        }
        Ok(())
    }

    /// The proportion scales as `(head, torso, leg)` f32, in the
    /// order `project.rs` consumes them.
    pub fn scales(&self) -> (f32, f32, f32) {
        (
            self.proportions.head_scale as f32,
            self.proportions.torso_scale as f32,
            self.proportions.leg_scale as f32,
        )
    }

    /// The per-band width scales `(head, torso, leg)` f32: the
    /// record's authored set, resolved, or exactly `(1.0, 1.0, 1.0)`
    /// when the record carries no set. Callers validate the record
    /// first (`build_character_with_params` does), so the set here
    /// is already known in-range.
    pub fn width_scales(&self) -> (f32, f32, f32) {
        match &self.proportions.width_scales {
            None => (1.0, 1.0, 1.0),
            Some(set) => set.resolved(),
        }
    }

    /// The per-band depth scales `(head, torso, leg)` f32; the
    /// depth counterpart of [`Self::width_scales`].
    pub fn depth_scales(&self) -> (f32, f32, f32) {
        match &self.proportions.depth_scales {
            None => (1.0, 1.0, 1.0),
            Some(set) => set.resolved(),
        }
    }

    /// Canonical JSON: sorted keys, no insignificant whitespace,
    /// fixed-precision floats (see [`canonical_write`]).
    pub fn canonical_json(&self) -> Result<String, FactoryError> {
        let value = serde_json::to_value(self)
            .map_err(|e| FactoryError::BadParam(format!("params serialize: {e}")))?;
        let mut out = String::new();
        canonical_write(&value, &mut out);
        Ok(out)
    }

    /// The hash this record should carry for its current contents.
    pub fn compute_hash(&self) -> Result<String, FactoryError> {
        let mut probe = self.clone();
        probe.record_hash = String::new();
        Ok(sha256_hex(probe.canonical_json()?.as_bytes()))
    }

    /// Verify `record_hash` matches the record contents.
    pub fn verify_hash(&self) -> Result<bool, FactoryError> {
        Ok(self.compute_hash()? == self.record_hash)
    }

    /// Serialize as TOML (the second serialization of the same
    /// schema). TOML has no `null`: `None` appearance fields are
    /// omitted, and parse back as `None`.
    pub fn to_toml(&self) -> Result<String, FactoryError> {
        toml::to_string(self)
            .map_err(|e| FactoryError::BadParam(format!("params toml serialize: {e}")))
    }

    /// Parse from TOML text, then validate and hash-verify.
    pub fn from_toml(text: &str) -> Result<Self, FactoryError> {
        let record: Self = toml::from_str(text)
            .map_err(|e| FactoryError::BadParam(format!("params toml parse: {e}")))?;
        record.validate()?;
        if !record.verify_hash()? {
            return Err(FactoryError::BadParam(
                "params record hash mismatch".to_string(),
            ));
        }
        Ok(record)
    }

    /// Parse from JSON text, then validate and hash-verify.
    pub fn from_json(text: &str) -> Result<Self, FactoryError> {
        let record: Self = serde_json::from_str(text)
            .map_err(|e| FactoryError::BadParam(format!("params json parse: {e}")))?;
        record.validate()?;
        if !record.verify_hash()? {
            return Err(FactoryError::BadParam(
                "params record hash mismatch".to_string(),
            ));
        }
        Ok(record)
    }

    /// Load a parameter record from a `.json` or `.toml` file
    /// (format chosen by extension), validated and hash-verified.
    pub fn load(path: &Path) -> Result<Self, FactoryError> {
        let text = std::fs::read_to_string(path)?;
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => Self::from_json(&text),
            Some("toml") => Self::from_toml(&text),
            other => Err(FactoryError::BadParam(format!(
                "params file extension {other:?} not recognized (expected .json or .toml)"
            ))),
        }
    }
}
