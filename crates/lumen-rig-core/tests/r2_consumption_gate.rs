//! R2 consumption gate, re-homed to lumen's tree
//! (D-937; D-913 Option B).
//!
//! The gate's first home was torogao's tree
//! (`crates/torogao-cli/tests/r2_consumption_gate.rs`,
//! retired under D-918 Option A): it ran the torogao bridge
//! (`RigExporter`) over the cast repertoire and fed the
//! emitted documents to lumen's real loader/evaluator —
//! the estate's cross-lane consumption proof. D-913 ruled
//! the torogao→lumen code link retired, so the proof now
//! lives where the evaluator of record lives: the exported
//! documents cross the wall as DATA fixtures
//! (`fixtures/r2-consumption/`, frozen byte-exact from the
//! torogao exporter at the commit the fixture manifest
//! names), and this test runs lumen's own evaluator over
//! them. No torogao code is linked, in either direction.
//!
//! The four sub-checks are the retired gate's own:
//!
//! 1. **Re-stamped native.** The exported model with its
//!    `format` field set to Torogao's native token
//!    (`torogao-rig-model`) must be refused (LRF-105):
//!    lumen stays strict under the token ruling. The
//!    restamp changes exactly one byte range — the token
//!    string, asserted to occur exactly once.
//! 2. **As exported.** The bridge's own document, lumen
//!    token stamped by the writer itself, must parse —
//!    for Prima and Yui. Alma's export is refused
//!    **LRF-305** (keyform scale must be > 0 at deformer
//!    `rot_MilkStreamL_3106`): a real exporter/loader gap
//!    (record `0a91349`), encoded here as the EXPECTED
//!    outcome, green-honestly. If Alma's export ever
//!    parses — or refuses with a different code — this
//!    gate fails loudly: the gap moved, and the record
//!    must move with it. Unlike
//!    `fixtures/conformance/`, these fixtures are NOT
//!    repaired (no 1e-6 scale rewrite): the refusal is
//!    the claim.
//! 3. **Motion fidelity.** Every parsed motion curve is
//!    swept against the torogao engine sampler's values,
//!    frozen into the `*.samples-*.json` fixtures on the
//!    retired gate's own 0.37-frame grid. Bounds are the
//!    retired gate's: cubic < 2e-3 (PROGRAM.md Stage 1.0);
//!    linear/stepped inside the f32 envelope 4e-6 (the
//!    flat 1e-6 bound is a measured miss at these
//!    magnitudes — f32 quantization in the engine
//!    sampler, worst 2.001e-6 in the census — and the
//!    measured maxima are printed beside the bounds, not
//!    smoothed over).
//! 4. **Player path.** `MotionPlayer` contributions at
//!    t=0.25 equal `curve_value_at` at 0.25 (the section
//!    4.3 fade law over the same evaluator).
//!
//! Hermetic by construction: fixtures are in-tree, the
//! manifest's sha256 for every fixture is verified
//! before any check runs, and no env var or network is
//! involved — the improvement over the retired gate's
//! `TOROGAO_PHASE_F_FIXTURES` gating. The measured table
//! is printed (run with `-- --nocapture`) and transcribed
//! into the D-937 report.
//!
//! Fixture provenance: `fixtures/r2-consumption/README.md`
//! and `fixtures/r2-consumption/manifest.json`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use lumen_rig_core::{
    Interpolation, MotionPlayer, ParseError, curve_value_at, parse_model, parse_motion,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// PROGRAM.md Stage 1.0's cubic bound (the retired gate's
/// `CUBIC_DEVIATION_MAX`).
const CUBIC_DEVIATION_MAX: f64 = 2e-3;
/// PROGRAM.md Stage 1.0's linear/stepped flat bound —
/// printed for the record; a measured miss against the
/// f32 engine sampler (see the module note).
const PLAIN_DEVIATION_MAX: f64 = 1e-6;
/// The f32-quantization envelope the retired gate asserts
/// (`PLAIN_ENVELOPE_MAX`): twice the measured cast
/// maximum, so a real evaluator regression still fails by
/// orders of magnitude.
const PLAIN_ENVELOPE_MAX: f64 = 4e-6;

/// One cast member and the as-exported outcome the gate
/// expects for its model document.
struct Member {
    stem: &'static str,
    /// `None` parses; `Some(code)` must be refused with
    /// exactly this LRF code.
    refusal: Option<&'static str>,
    /// For a refused member, a fragment the refusal's
    /// rendered text must carry (the census-named locus).
    refusal_locus: Option<&'static str>,
}

const CAST: [Member; 3] = [
    Member {
        stem: "prima-repertoire",
        refusal: None,
        refusal_locus: None,
    },
    Member {
        stem: "yui-repertoire",
        refusal: None,
        refusal_locus: None,
    },
    Member {
        stem: "alma-repertoire",
        refusal: Some("LRF-305"),
        refusal_locus: Some("rot_MilkStreamL_3106"),
    },
];

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("r2-consumption")
}

fn fixture_bytes(relative: &str) -> Vec<u8> {
    let path = fixture_dir().join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("fixture {relative}: {error}"))
}

fn fixture_json(relative: &str) -> Value {
    serde_json::from_slice(&fixture_bytes(relative))
        .unwrap_or_else(|error| panic!("fixture {relative} parses as JSON: {error}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Verifies every fixture file against the manifest
/// (sha256 + exact file set) before any check runs, and
/// returns the manifest-listed file names.
fn verify_manifest() -> BTreeSet<String> {
    let manifest = fixture_json("manifest.json");
    assert_eq!(
        manifest["format"], "r2-consumption-fixtures",
        "manifest format tag"
    );
    let mut listed = BTreeSet::new();
    for entry in manifest["files"].as_array().expect("manifest lists files") {
        let path = entry["path"].as_str().expect("manifest entry names a path");
        let expected = entry["sha256"]
            .as_str()
            .expect("manifest entry carries a sha256");
        let actual = sha256_hex(&fixture_bytes(path));
        assert_eq!(
            actual, expected,
            "fixture {path} sha256 drifted from the manifest (re-freeze or re-measure; never edit silently)"
        );
        listed.insert(path.to_string());
    }
    let mut on_disk: BTreeSet<String> = BTreeSet::new();
    for entry in fs::read_dir(fixture_dir()).expect("fixture dir readable") {
        let name = entry.expect("fixture dir entry").file_name();
        let name = name.to_string_lossy().into_owned();
        if name != "manifest.json" && name != "README.md" {
            on_disk.insert(name);
        }
    }
    assert_eq!(
        listed, on_disk,
        "manifest file set and fixture dir contents differ"
    );
    listed
}

/// The boundary probe: the export's `format` token set to
/// Torogao's native token, by byte replacement — the
/// token string is asserted to occur exactly once in the
/// model document, so nothing else moves.
fn token_restamped_native(model_bytes: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(model_bytes).expect("model document is UTF-8");
    let token = "\"lumen-rig-model\"";
    assert_eq!(
        text.matches(token).count(),
        1,
        "the lumen format token occurs exactly once in the model document"
    );
    text.replace(token, "\"torogao-rig-model\"").into_bytes()
}

/// One motion's fidelity outcome against the frozen
/// engine sampler grid.
struct MotionFidelity {
    plain_max: f64,
    plain_worst_value: f64,
    cubic_max: f64,
    curves: usize,
    samples: usize,
}

/// Sweeps one parsed motion's curves against the frozen
/// engine samples for that motion, on the retired gate's
/// 0.37-frame grid (carried in the samples fixture).
fn motion_fidelity(
    stem: &str,
    performance: &str,
    motion: &lumen_rig_core::Motion,
) -> MotionFidelity {
    let samples_doc = fixture_json(&format!("{stem}.samples-{performance}.json"));
    assert_eq!(
        samples_doc["format"], "r2-engine-samples",
        "samples format tag"
    );
    assert_eq!(
        samples_doc["motion_id"], motion.id,
        "samples name the parsed motion"
    );
    let sample_curves = samples_doc["curves"]
        .as_array()
        .expect("samples carry curves");
    assert_eq!(
        sample_curves.len(),
        motion.curves.len(),
        "{stem}/{performance}: sample curve count vs parsed curve count"
    );
    let mut fidelity = MotionFidelity {
        plain_max: 0.0,
        plain_worst_value: 0.0,
        cubic_max: 0.0,
        curves: 0,
        samples: 0,
    };
    for curve in &motion.curves {
        let frozen = sample_curves
            .iter()
            .find(|entry| entry["parameter"] == curve.parameter)
            .unwrap_or_else(|| {
                panic!(
                    "{stem}/{performance}: no frozen samples for {}",
                    curve.parameter
                )
            });
        let cubic = curve
            .keyframes
            .iter()
            .any(|key| key.interpolation == Interpolation::Cubic);
        assert_eq!(
            frozen["cubic"].as_bool(),
            Some(cubic),
            "{stem}/{performance}: frozen cubic flag vs parsed curve {}",
            curve.parameter
        );
        for sample in frozen["samples"].as_array().expect("curve carries samples") {
            let time = sample[0].as_f64().expect("sample time");
            let engine = sample[1].as_f64().expect("sample engine value");
            let lumen = curve_value_at(&curve.keyframes, time);
            let deviation = (engine - lumen).abs();
            if cubic {
                fidelity.cubic_max = fidelity.cubic_max.max(deviation);
            } else if deviation > fidelity.plain_max {
                fidelity.plain_max = deviation;
                fidelity.plain_worst_value = engine;
            }
            fidelity.samples += 1;
        }
        fidelity.curves += 1;
    }
    fidelity
}

#[test]
fn r2_consumption_gate() {
    let listed = verify_manifest();
    let mut lines: Vec<String> = Vec::new();
    let mut breaches: Vec<String> = Vec::new();
    for member in &CAST {
        let stem = member.stem;
        let model_bytes = fixture_bytes(&format!("{stem}.model.json"));

        // (1) Re-stamped native: lumen's loader must refuse
        // the model on its format token (LRF-105).
        let restamped = token_restamped_native(&model_bytes);
        match parse_model(&restamped) {
            Err(error @ ParseError::UnsupportedFormat { .. }) => {
                assert_eq!(error.code(), "LRF-105");
                lines.push(format!(
                    "{stem}: native-restamped model refused LRF-105 (expected)"
                ));
            }
            Err(other) => {
                panic!("{stem}: native-restamped model refused unexpectedly: {other:?}")
            }
            Ok(_) => panic!("{stem}: native-restamped model parsed; lumen's token boundary moved"),
        }

        // (2) As exported: the bridge's own document.
        let model = match parse_model(&model_bytes) {
            Ok(model) => {
                assert!(
                    member.refusal.is_none(),
                    "{stem}: export parses but the gate expects refusal {} — the gap moved; update the record and this expectation together",
                    member.refusal.unwrap_or_default()
                );
                lines.push(format!("{stem}: as-exported model parses"));
                model
            }
            Err(error) => {
                let expected = member.refusal.unwrap_or_else(|| {
                    panic!(
                        "{stem}: as-exported model refused {} but the gate expects it to parse",
                        error.code()
                    )
                });
                assert_eq!(
                    error.code(),
                    expected,
                    "{stem}: as-exported refusal code moved"
                );
                if let Some(locus) = member.refusal_locus {
                    let rendered = format!("{error}");
                    assert!(
                        rendered.contains(locus),
                        "{stem}: refusal locus moved: expected '{locus}' in '{rendered}'"
                    );
                }
                lines.push(format!(
                    "{stem}: as-exported model refused {} (expected — the 0a91349 gap)",
                    error.code()
                ));
                continue;
            }
        };

        // (3) + (4): motions through the real loader and
        // evaluator; the player path over the same curves.
        let performances: Vec<String> = listed
            .iter()
            .filter_map(|name| {
                name.strip_prefix(&format!("{stem}.motion-"))
                    .and_then(|rest| rest.strip_suffix(".json"))
                    .map(str::to_string)
            })
            .collect();
        assert!(
            !performances.is_empty(),
            "{stem}: manifest lists no motion fixtures"
        );
        for performance in &performances {
            let motion_bytes = fixture_bytes(&format!("{stem}.motion-{performance}.json"));
            let motion = parse_motion(&motion_bytes, &model).unwrap_or_else(|error| {
                panic!(
                    "{stem}/{performance}: motion refused {} but the gate expects it to parse",
                    error.code()
                )
            });
            let fidelity = motion_fidelity(stem, performance, &motion);
            lines.push(format!(
                "{stem}/{performance}: {} curves, {} samples, plain max {:.3e} (worst at |v| {:.1}), cubic max {:.3e}",
                fidelity.curves,
                fidelity.samples,
                fidelity.plain_max,
                fidelity.plain_worst_value.abs(),
                fidelity.cubic_max
            ));
            if fidelity.plain_max >= PLAIN_ENVELOPE_MAX {
                breaches.push(format!(
                    "{stem}/{performance}: linear/stepped deviation {:.3e} at |v| {:.1} vs the f32 envelope {:.1e} (PROGRAM flat bound {:.1e})",
                    fidelity.plain_max,
                    fidelity.plain_worst_value.abs(),
                    PLAIN_ENVELOPE_MAX,
                    PLAIN_DEVIATION_MAX
                ));
            }
            if fidelity.cubic_max >= CUBIC_DEVIATION_MAX {
                breaches.push(format!(
                    "{stem}/{performance}: cubic deviation {:.3e} vs bound {:.1e}",
                    fidelity.cubic_max, CUBIC_DEVIATION_MAX
                ));
            }
            // The player path (the section 4.3 fade law
            // over the same evaluator).
            let mut player = MotionPlayer::new(&motion);
            player.start();
            player.advance(0.25);
            let contributions = player.evaluate();
            assert_eq!(contributions.len(), motion.curves.len());
            for contribution in &contributions {
                let curve = motion
                    .curves
                    .iter()
                    .find(|curve| curve.parameter == contribution.parameter)
                    .expect("contribution names a curve");
                let expected = curve_value_at(&curve.keyframes, 0.25);
                assert!(
                    (contribution.value - expected).abs() < 1e-9,
                    "player value for {} deviates from curve_value_at",
                    contribution.parameter
                );
            }
        }
    }
    for line in &lines {
        eprintln!("R2-GATE {line}");
    }
    assert!(
        breaches.is_empty(),
        "fidelity envelope breaches (full table above): {}",
        breaches.join("; ")
    );
}
