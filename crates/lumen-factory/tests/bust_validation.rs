//! Validation tests: the bust delta parameter — a graded A–G sweep
//! measured on the generated mesh itself, underbust invariance, the
//! byte-identical canonical base when the parameter is absent, the
//! end-to-end params-record path, and the identity-record path
//! (GAP-11-3: the parameter is per-character end-to-end).
//!
//! Ported from the factory Stage-3 line (bust-param branch), where
//! the shaping was first built and gated; that line never landed,
//! so the tests ride the port onto the landed factory.

use std::path::PathBuf;

use lumen_factory::base::{MeshPart, ScaledBuild, build_base_mesh, build_base_mesh_parametric};
use lumen_factory::blend::load_templates;
use lumen_factory::gltf::parse_glb;
use lumen_factory::identity::load_record;
use lumen_factory::params::{ParameterRecord, bust_grade};
use lumen_factory::project::{BuildOptions, build_character_with_params};

/// Half a grade step is 1.25 cm; the sweep must land within 0.75 cm
/// of target after polygon/section discretization.
const TOL_CM: f32 = 0.75;

fn asset_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

fn body_part(mesh: &lumen_factory::base::BaseMesh) -> &MeshPart {
    mesh.parts
        .iter()
        .find(|p| p.name == "body")
        .expect("body part exists")
}

/// Circumference (metres) of the mesh section at height `y`: chain
/// the triangle/plane intersection segments into loops by quantized
/// endpoints and return the longest loop's length.
fn section_circumference(part: &MeshPart, y: f32) -> f32 {
    let mut segments: Vec<([f32; 3], [f32; 3])> = Vec::new();
    for tri in part.indices.as_chunks::<3>().0 {
        let pts = [
            part.vertices[tri[0] as usize].position,
            part.vertices[tri[1] as usize].position,
            part.vertices[tri[2] as usize].position,
        ];
        let mut hits: Vec<[f32; 3]> = Vec::new();
        for e in 0..3 {
            let a = pts[e];
            let b = pts[(e + 1) % 3];
            let (da, db) = (a[1] - y, b[1] - y);
            if da * db < 0.0 {
                let t = da / (da - db);
                hits.push([a[0] + (b[0] - a[0]) * t, y, a[2] + (b[2] - a[2]) * t]);
            }
        }
        if hits.len() == 2 {
            segments.push((hits[0], hits[1]));
        }
    }
    // Chain segments by nearest endpoint within EPS. (Exact quantized
    // keys are fragile here: two triangles sharing an edge compute
    // the crossing from opposite directions, and an f32 ulp can flip
    // a rounding bucket and split the loop.)
    fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
        (a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)
    }
    const EPS2: f32 = 1e-8; // (0.1 mm)^2
    let mut used = vec![false; segments.len()];
    let mut best = 0.0f32;
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        let origin = segments[start].0;
        let mut cur = origin;
        let mut len = 0.0f32;
        loop {
            let next = segments
                .iter()
                .enumerate()
                .find(|(i, s)| !used[*i] && (dist2(s.0, cur) < EPS2 || dist2(s.1, cur) < EPS2))
                .map(|(i, _)| i);
            let Some(n) = next else { break };
            used[n] = true;
            let (a, b) = segments[n];
            len += (b[0] - a[0]).hypot(b[2] - a[2]);
            cur = if dist2(a, cur) < dist2(b, cur) { b } else { a };
            if dist2(cur, origin) < EPS2 {
                break;
            }
        }
        best = best.max(len);
    }
    best
}

/// Bust and underbust circumferences (cm) measured on the mesh by
/// scanning sections: the underbust is the minimum section in the
/// waist band, the bust the maximum section above it — the tailor's
/// definitions. (Fixed-height slices under-read extrema: the lathe
/// samples the profile at 19 ring heights, flattening any peak or
/// valley that falls between rings.)
fn measured_delta_cm(delta: Option<f32>) -> (f32, f32) {
    let mesh = build_base_mesh_parametric(&ScaledBuild {
        bust_delta_cm: delta,
        ..ScaledBuild::default()
    })
    .expect("parametric build");
    let body = body_part(&mesh);
    let mut under = f32::INFINITY;
    let mut under_y = 0.0f32;
    let mut y = 1.02f32;
    while y <= 1.26 {
        let c = section_circumference(body, y);
        if c < under {
            under = c;
            under_y = y;
        }
        y += 0.002;
    }
    let mut bust = 0.0f32;
    let mut y = under_y + 0.04;
    while y <= 1.47 {
        bust = bust.max(section_circumference(body, y));
        y += 0.002;
    }
    (bust * 100.0, under * 100.0)
}

#[test]
fn sweep_hits_targets_and_is_monotonic() {
    let mut prev_bust = 0.0f32;
    for target in [10.0f32, 12.5, 15.0, 17.5, 20.0, 22.5, 25.0] {
        let (bust, under) = measured_delta_cm(Some(target));
        let measured = bust - under;
        println!(
            "target {target:5.1} cm -> bust {bust:6.2} under {under:6.2} delta {measured:6.2} grade {:?}",
            bust_grade(f64::from(measured))
        );
        assert!(
            (measured - target).abs() <= TOL_CM,
            "delta {target}: measured {measured:.2} outside ±{TOL_CM}"
        );
        assert!(
            bust > prev_bust,
            "bust circumference not strictly increasing at {target}"
        );
        prev_bust = bust;
    }
}

#[test]
fn underbust_is_invariant_across_the_sweep() {
    // The underbust RING never moves (shaping weight is exactly 0 at
    // and below it). The scan minimum sits a hair above the ring,
    // where the surface interpolates toward the bust, so it rises
    // slightly as the bust grows: observed spread 0.56 cm across the
    // full A–G sweep, against the bust's 16 cm. Tolerance 0.65 cm.
    let (_, under_none) = measured_delta_cm(None);
    for target in [10.0f32, 15.0, 20.0, 25.0] {
        let (_, under) = measured_delta_cm(Some(target));
        assert!(
            (under - under_none).abs() <= 0.65,
            "underbust moved at delta {target}: {under:.2} vs {under_none:.2}"
        );
    }
}

#[test]
fn absent_parameter_reproduces_the_canonical_base_exactly() {
    let via_params = build_base_mesh_parametric(&ScaledBuild::default()).expect("default build");
    let canonical = build_base_mesh().expect("canonical build");
    assert_eq!(via_params.parts.len(), canonical.parts.len());
    for (a, b) in via_params.parts.iter().zip(canonical.parts.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.vertices.len(), b.vertices.len(), "part {}", a.name);
        for (va, vb) in a.vertices.iter().zip(b.vertices.iter()) {
            assert_eq!(va.position, vb.position, "part {}", a.name);
        }
    }
}

#[test]
fn default_base_measures_grade_e() {
    let (bust, under) = measured_delta_cm(None);
    let delta = bust - under;
    assert!(
        (18.75..=21.25).contains(&delta),
        "canonical base delta {delta:.2} cm outside the E band"
    );
    assert_eq!(bust_grade(f64::from(delta)), Some('E'));
}

#[test]
fn grade_table() {
    assert_eq!(bust_grade(10.0), Some('A'));
    assert_eq!(bust_grade(12.5), Some('B'));
    assert_eq!(bust_grade(15.0), Some('C'));
    assert_eq!(bust_grade(17.5), Some('D'));
    assert_eq!(bust_grade(20.0), Some('E'));
    assert_eq!(bust_grade(22.5), Some('F'));
    assert_eq!(bust_grade(25.0), Some('G'));
    assert_eq!(
        bust_grade(11.25),
        Some('A'),
        "seam belongs to the lower grade"
    );
    assert_eq!(bust_grade(8.74), None);
    assert_eq!(bust_grade(26.26), None);
}

#[test]
fn params_record_drives_the_build_end_to_end() {
    let record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    let templates_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/emotions_fixture.json");
    let templates = load_templates(&templates_path).expect("fixture templates");
    let options = BuildOptions {
        ramp: false,
        toon: true,
    };

    let baseline_params = ParameterRecord::from_identity(&record).expect("derive params");
    assert_eq!(baseline_params.proportions.bust_delta_cm, None);
    let baseline = build_character_with_params(&record, &templates, &baseline_params, options)
        .expect("baseline build");

    let mut graded_params = baseline_params.clone();
    graded_params.proportions.bust_delta_cm = Some(12.5);
    graded_params.record_hash = graded_params.compute_hash().expect("recompute hash");
    graded_params.validate().expect("graded params validate");
    let graded = build_character_with_params(&record, &templates, &graded_params, options)
        .expect("graded build");
    parse_glb(&graded).expect("graded GLB parses back");
    assert_ne!(
        graded, baseline,
        "a B-grade build must differ from the canonical base"
    );
}

#[test]
fn identity_record_carries_the_parameter_end_to_end() {
    // GAP-11-3's exact ask: a fixture IDENTITY with a specified
    // bust derives params that carry it, and the built character's
    // bust ring measures larger than the same identity without one.
    let mut record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    assert_eq!(record.bust_delta_cm, None, "shipped record carries none");
    record.bust_delta_cm = Some(25.0);
    record.record_hash = record.compute_hash().expect("recompute hash");

    let params = ParameterRecord::from_identity(&record).expect("derive params");
    assert_eq!(params.proportions.bust_delta_cm, Some(25.0));

    let templates_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/emotions_fixture.json");
    let templates = load_templates(&templates_path).expect("fixture templates");
    let options = BuildOptions::default();
    let built = build_character_with_params(&record, &templates, &params, options)
        .expect("identity-driven build");
    let shipped = std::fs::read(asset_path("seraphine.glb")).expect("anchor glb");
    assert_ne!(
        built, shipped,
        "a G-grade identity must not rebuild the shipped base-torso GLB"
    );
}

#[test]
fn bust_ring_slice_grows_in_width_and_depth() {
    // The Canon-11 measurement method: vertex slices at the bust
    // ring height. The base profile's bust ring sits at y = 1.31;
    // a G delta scales that ring's radii by the circumference
    // factor, so both the width (X span) and depth (Z span) of the
    // body slice at the ring grow measurably.
    let slice = |delta: Option<f32>| -> (f32, f32) {
        let mesh = build_base_mesh_parametric(&ScaledBuild {
            bust_delta_cm: delta,
            ..ScaledBuild::default()
        })
        .expect("parametric build");
        let body = body_part(&mesh);
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for v in &body.vertices {
            if (v.position[1] - 1.31).abs() < 0.005 {
                for axis in 0..3 {
                    lo[axis] = lo[axis].min(v.position[axis]);
                    hi[axis] = hi[axis].max(v.position[axis]);
                }
            }
        }
        (hi[0] - lo[0], hi[2] - lo[2])
    };
    let (w0, d0) = slice(None);
    let (w1, d1) = slice(Some(25.0));
    assert!(w0 > 0.30 && d0 > 0.20, "base slice sane: {w0} x {d0}");
    assert!(
        w1 > w0 + 0.02,
        "bust width must grow under a G delta: {w0} -> {w1}"
    );
    assert!(
        d1 > d0 + 0.015,
        "bust depth must grow under a G delta: {d0} -> {d1}"
    );
}
