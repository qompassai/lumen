//! The proportion-band gate (SPEC §6, §6.1). Measures an artifact's
//! measured proportions against the record's bands and returns PASS
//! or the typed refusal `drift_detected` with the full delta table.
//!
//! The gate is a floor, not a verdict: PASS releases an artifact to
//! the eye sheet, never to adoption (SPEC §6.1). A FAIL is equally
//! precise — every out-of-band metric is named with its delta, so a
//! refusal is evidence, not an opinion.
//!
//! Determinism: failures are sorted by metric name and the verdict
//! contains no timestamps or ambient state, so the same artifact
//! against the same record yields a byte-identical verdict (SPEC
//! §10 S1 determinism clause).

use crate::model::CanonRecord;
use crate::refusal::{BandFailure, Refusal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Measured proportions for one artifact, keyed by band metric name.
/// A `BTreeMap` keeps iteration (and therefore every derived report)
/// deterministic.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurements {
    pub measurements: BTreeMap<String, f64>,
    /// Provenance of the measurement set (which artifact, which
    /// method run) — carried so a verdict can be traced to its
    /// evidence, never used by the gate itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Measurements {
    pub fn get(&self, metric: &str) -> Option<f64> {
        self.measurements.get(metric).copied()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum GateVerdict {
    /// Every band measured inside its band. Releases the artifact to
    /// the eye sheet (SPEC §6.1) — never to adoption.
    Pass,
    Refused(Refusal),
}

/// Representation allowance for band-edge comparisons. Bands and
/// measurements are stated in decimal (two-decimal ratios in the
/// D-280 record) but compared in binary floating point, where e.g.
/// `0.17 - 0.16` exceeds `0.01` by ~1e-17. An excess at or below
/// this allowance counts as ON the edge (edges are inclusive); the
/// allowance is five-plus orders of magnitude below any real
/// measurement precision, so it cannot smuggle a real drift through.
pub const EDGE_EPSILON: f64 = 1e-9;

/// Run the band gate. Bands are evaluated in record order; the
/// refusal's failures are sorted by metric for byte-stability.
///
/// Band semantics:
/// - window band (default): pass iff `|measured - target| <= tolerance`
///   (edges inclusive, up to [`EDGE_EPSILON`]);
/// - floor band (`floor_only`): pass iff `measured >= target - tolerance`;
/// - a banded metric with no measurement FAILS (a floor gate cannot
///   pass what it did not measure);
/// - a non-finite measurement FAILS.
pub fn gate_bands(record: &CanonRecord, measured: &Measurements) -> GateVerdict {
    verdict_for(&record.style_fingerprint.proportion_bands, measured)
}

/// The register gate (D-312): the same band math over the record's
/// register bands. A record with no register bands yields no
/// failures here — callers that require register enforcement (the
/// lumen broker) refuse such records separately, never silently.
pub fn gate_register(record: &CanonRecord, measured: &Measurements) -> GateVerdict {
    verdict_for(&record.style_fingerprint.register_bands, measured)
}

/// Proportion + register in one verdict (the lumen broker's gate).
/// Failures from both classes merge and sort by metric, so a
/// register drift and a proportion drift refuse together with the
/// full delta table.
pub fn gate_all(record: &CanonRecord, measured: &Measurements) -> GateVerdict {
    let mut bands: Vec<&crate::model::ProportionBand> =
        record.style_fingerprint.proportion_bands.iter().collect();
    bands.extend(record.style_fingerprint.register_bands.iter());
    // verdict_for takes a slice of owned bands; clone is cheap and
    // keeps the single-band-loop the only band math in the crate.
    let owned: Vec<crate::model::ProportionBand> = bands.into_iter().cloned().collect();
    verdict_for(&owned, measured)
}

fn verdict_for(bands: &[crate::model::ProportionBand], measured: &Measurements) -> GateVerdict {
    let mut failures: Vec<BandFailure> = Vec::new();

    for band in bands {
        let value = measured.get(&band.metric);
        let failure = match value {
            None => Some(BandFailure {
                metric: band.metric.clone(),
                target: band.target,
                tolerance: band.tolerance,
                measured: None,
                delta: None,
                excess: None,
            }),
            Some(v) if !v.is_finite() => Some(BandFailure {
                metric: band.metric.clone(),
                target: band.target,
                tolerance: band.tolerance,
                measured: None,
                delta: None,
                excess: None,
            }),
            Some(v) => {
                let delta = v - band.target;
                let excess = if band.floor_only {
                    (band.target - band.tolerance) - v
                } else {
                    delta.abs() - band.tolerance
                };
                if excess > EDGE_EPSILON {
                    Some(BandFailure {
                        metric: band.metric.clone(),
                        target: band.target,
                        tolerance: band.tolerance,
                        measured: Some(v),
                        delta: Some(delta),
                        excess: Some(excess),
                    })
                } else {
                    None
                }
            }
        };
        if let Some(failure) = failure {
            failures.push(failure);
        }
    }

    if failures.is_empty() {
        GateVerdict::Pass
    } else {
        failures.sort_by(|a, b| a.metric.cmp(&b.metric));
        GateVerdict::Refused(Refusal::DriftDetected { failures })
    }
}
