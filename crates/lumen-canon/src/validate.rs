//! Structural and invariant validation of a parsed record (SPEC §3,
//! §5). Validation collects every issue it can find — a record with
//! three defects reports all three — and returns them sorted by
//! rendered text so reports are deterministic.

use crate::model::{is_sha256_hex, CanonRecord, CheckMethod, FORMAT};
use std::collections::BTreeSet;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValidationIssue {
    WrongFormat(String),
    MalformedId(String),
    IdLaneMismatch,
    VersionZero,
    NoAnchors,
    NoPrimaryAnchor,
    AnchorMissingAdoptionAct { role: String, field: &'static str },
    BadSha256 { owner: String, value: String },
    DuplicateInvariantName(String),
    InvariantFactEmpty(String),
    MachineInvariantWithoutMethod(String),
    DuplicateBandMetric(String),
    BandMethodEmpty(String),
    BandTargetNotFinite(String),
    BandToleranceInvalid(String),
    PaletteHexMalformed { role: String, hex: String },
    PaletteToleranceInvalid(String),
    ChainVersionAfterRecord { index: usize },
    ChainVersionRegresses { index: usize },
    MarkerEmpty,
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationIssue::WrongFormat(got) => {
                write!(f, "format is {got:?}, expected {FORMAT:?}")
            }
            ValidationIssue::MalformedId(id) => {
                write!(f, "id {id:?} is not of the form <lane>/<character>")
            }
            ValidationIssue::IdLaneMismatch => {
                write!(f, "id lane segment does not match origin_lane")
            }
            ValidationIssue::VersionZero => write!(f, "version must be >= 1"),
            ValidationIssue::NoAnchors => write!(f, "record has no anchors"),
            ValidationIssue::NoPrimaryAnchor => {
                write!(f, "record has no anchor with role \"primary\"")
            }
            ValidationIssue::AnchorMissingAdoptionAct { role, field } => write!(
                f,
                "anchor role {role:?} has empty {field}: anchors enter only by an adoption act"
            ),
            ValidationIssue::BadSha256 { owner, value } => {
                write!(
                    f,
                    "{owner} sha256 {value:?} is not a 64-char lowercase hex digest"
                )
            }
            ValidationIssue::DuplicateInvariantName(name) => {
                write!(f, "duplicate invariant name {name:?}")
            }
            ValidationIssue::InvariantFactEmpty(name) => {
                write!(f, "invariant {name:?} states no fact")
            }
            ValidationIssue::MachineInvariantWithoutMethod(name) => write!(
                f,
                "invariant {name:?} is machine-measured but names no measurement method"
            ),
            ValidationIssue::DuplicateBandMetric(metric) => {
                write!(f, "duplicate proportion band for metric {metric:?}")
            }
            ValidationIssue::BandMethodEmpty(metric) => {
                write!(f, "band {metric:?} names no measurement method")
            }
            ValidationIssue::BandTargetNotFinite(metric) => {
                write!(f, "band {metric:?} target is not finite")
            }
            ValidationIssue::BandToleranceInvalid(metric) => {
                write!(f, "band {metric:?} tolerance must be finite and > 0")
            }
            ValidationIssue::PaletteHexMalformed { role, hex } => {
                write!(f, "palette colour {role:?} hex {hex:?} is not #RRGGBB")
            }
            ValidationIssue::PaletteToleranceInvalid(role) => {
                write!(f, "palette colour {role:?} tolerance must be in [0, 1]")
            }
            ValidationIssue::ChainVersionAfterRecord { index } => write!(
                f,
                "keeper_chain[{index}] record_version exceeds the record version"
            ),
            ValidationIssue::ChainVersionRegresses { index } => write!(
                f,
                "keeper_chain[{index}] record_version regresses (chain is append-only)"
            ),
            ValidationIssue::MarkerEmpty => {
                write!(f, "rendering_markers contains an empty marker")
            }
        }
    }
}

fn is_hex_color(hex: &str) -> bool {
    hex.len() == 7 && hex.starts_with('#') && hex[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Validate a record structurally and against the §3 invariants.
/// Returns every issue found, sorted for determinism.
pub fn validate_record(record: &CanonRecord) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    if record.format != FORMAT {
        issues.push(ValidationIssue::WrongFormat(record.format.clone()));
    }

    // Id shape and lane agreement (SPEC §3: id is lane-scoped).
    match (record.id_lane(), record.character()) {
        (Some(id_lane), Some(_)) => {
            if id_lane != record.origin_lane {
                issues.push(ValidationIssue::IdLaneMismatch);
            }
        }
        _ => issues.push(ValidationIssue::MalformedId(record.id.clone())),
    }

    if record.version == 0 {
        issues.push(ValidationIssue::VersionZero);
    }

    // Anchors: present, one primary, hashes pinned, adoption acts
    // recorded (SPEC §3: an anchor enters only by an adoption act).
    if record.anchors.is_empty() {
        issues.push(ValidationIssue::NoAnchors);
    }
    if !record.anchors.iter().any(|a| a.role == "primary") {
        issues.push(ValidationIssue::NoPrimaryAnchor);
    }
    for anchor in &record.anchors {
        if !is_sha256_hex(&anchor.sha256) {
            issues.push(ValidationIssue::BadSha256 {
                owner: format!("anchor role {:?}", anchor.role),
                value: anchor.sha256.clone(),
            });
        }
        for (field, value) in [
            ("adopted_by", &anchor.adopted_by),
            ("adopted_at", &anchor.adopted_at),
            ("decision_ref", &anchor.decision_ref),
        ] {
            if value.trim().is_empty() {
                issues.push(ValidationIssue::AnchorMissingAdoptionAct {
                    role: anchor.role.clone(),
                    field,
                });
            }
        }
    }

    for refr in &record.authority_refs {
        if !is_sha256_hex(&refr.sha256) {
            issues.push(ValidationIssue::BadSha256 {
                owner: format!("authority_ref {:?}", refr.handle),
                value: refr.sha256.clone(),
            });
        }
    }

    // Invariants: named, unique, fact stated; the machine/eye-only
    // split is honest — a machine invariant must name its measurement.
    let mut seen = BTreeSet::new();
    for inv in &record.identity_invariants {
        if !seen.insert(inv.name.clone()) {
            issues.push(ValidationIssue::DuplicateInvariantName(inv.name.clone()));
        }
        if inv.fact.trim().is_empty() {
            issues.push(ValidationIssue::InvariantFactEmpty(inv.name.clone()));
        }
        if let CheckMethod::Machine { method } = &inv.check {
            if method.trim().is_empty() {
                issues.push(ValidationIssue::MachineInvariantWithoutMethod(
                    inv.name.clone(),
                ));
            }
        }
    }

    // Bands: unique metrics, finite targets, positive tolerances, a
    // named measurement method per band (SPEC §3).
    let mut seen = BTreeSet::new();
    for band in &record.style_fingerprint.proportion_bands {
        if !seen.insert(band.metric.clone()) {
            issues.push(ValidationIssue::DuplicateBandMetric(band.metric.clone()));
        }
        if band.method.trim().is_empty() {
            issues.push(ValidationIssue::BandMethodEmpty(band.metric.clone()));
        }
        if !band.target.is_finite() {
            issues.push(ValidationIssue::BandTargetNotFinite(band.metric.clone()));
        }
        if !band.tolerance.is_finite() || band.tolerance <= 0.0 {
            issues.push(ValidationIssue::BandToleranceInvalid(band.metric.clone()));
        }
    }

    // Register bands (D-312): same shape discipline as proportion
    // bands, metrics unique within the register class.
    let mut seen = BTreeSet::new();
    for band in &record.style_fingerprint.register_bands {
        if !seen.insert(band.metric.clone()) {
            issues.push(ValidationIssue::DuplicateBandMetric(band.metric.clone()));
        }
        if band.method.trim().is_empty() {
            issues.push(ValidationIssue::BandMethodEmpty(band.metric.clone()));
        }
        if !band.target.is_finite() {
            issues.push(ValidationIssue::BandTargetNotFinite(band.metric.clone()));
        }
        if !band.tolerance.is_finite() || band.tolerance <= 0.0 {
            issues.push(ValidationIssue::BandToleranceInvalid(band.metric.clone()));
        }
    }

    for colour in &record.style_fingerprint.palette {
        if !is_hex_color(&colour.hex) {
            issues.push(ValidationIssue::PaletteHexMalformed {
                role: colour.role.clone(),
                hex: colour.hex.clone(),
            });
        }
        if !colour.tolerance.is_finite() || !(0.0..=1.0).contains(&colour.tolerance) {
            issues.push(ValidationIssue::PaletteToleranceInvalid(
                colour.role.clone(),
            ));
        }
    }

    if record
        .style_fingerprint
        .rendering_markers
        .iter()
        .any(|m| m.trim().is_empty())
    {
        issues.push(ValidationIssue::MarkerEmpty);
    }

    // Keeper chain: entries are written against record versions that
    // exist and never regress (append-only, SPEC §3).
    let mut previous = 0u64;
    for (index, entry) in record.keeper_chain.iter().enumerate() {
        if !is_sha256_hex(&entry.sha256) {
            issues.push(ValidationIssue::BadSha256 {
                owner: format!("keeper_chain[{index}]"),
                value: entry.sha256.clone(),
            });
        }
        if entry.record_version > record.version {
            issues.push(ValidationIssue::ChainVersionAfterRecord { index });
        }
        if entry.record_version < previous {
            issues.push(ValidationIssue::ChainVersionRegresses { index });
        }
        previous = entry.record_version;
    }

    issues.sort();
    issues.dedup();
    issues
}
