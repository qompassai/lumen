//! Host runner for the contact-pair probe (ActLab
//! Tool 2, D-943): loads the probe spec's rig records
//! (SHA-256 verified, then imported through the
//! evaluator of record's INP1 importer), validates the
//! timeline against the rigs' own parameter tables
//! before any evaluation, then drives the evaluator
//! across the timeline, solves the declared contacts
//! through the engine's contact seam, and measures
//! every declared pair per step.
//!
//! Nothing here evaluates a rig beyond what
//! lumen-rig-core's `PuppetEval` does, and nothing here
//! tunes or re-stages: the spec is the whole
//! choreography, as data.

use std::path::Path;

use lumen_engine_core::contact::{
    endpoint_world, evaluate_frame_with_geometry, ContactDef, ContactEndpoint, EvaluatedMesh,
    FrameInput, Placement, Scene,
};
use lumen_engine_core::probe::{
    band_box, box_distance, AnchorSelector, PairAccumulator, ProbeAnchor, ProbeCheckpointRead,
    ProbeContact, ProbePlacement, ProbeReport, ProbeRigReport, ProbeSpec, ProbeStepRead,
    RigParameterTable,
};
use lumen_engine_core::{
    sample_pivot_turn, sample_pose_track, sample_step_in_out, ResolvedPoseKey,
};
use lumen_rig_core::puppet::PuppetNodeKind;
use lumen_rig_core::{import_inp, Model, PuppetEval, PuppetStage, RgbaImage as TexImage};
use sha2::{Digest, Sha256};

use crate::instruments::{is_leg_part, point_triangle_signed};
use crate::staging::{bridge_model, harvest, measure_bounds, set_pose, settle, Bounds};

type HostResult<T> = Result<T, String>;

fn err<T>(message: impl Into<String>) -> HostResult<T> {
    Err(message.into())
}

/// Maximum `.inx` bytes the probe will read (the
/// importer's own bound is smaller; this caps the read
/// itself).
const INX_BYTES_MAX: u64 = 268_435_456;

/// A loaded rig: everything the run needs per rig.
struct LoadedRig {
    report: ProbeRigReport,
    table: RigParameterTable,
    stage: PuppetStage,
    textures: Vec<TexImage>,
    model: Model,
    bounds: Bounds,
}

/// Load, hash-verify, and import one rig record;
/// measure its bounds on a settled scratch evaluator.
fn load_rig(
    spec_rig: &lumen_engine_core::probe::ProbeRig,
    base: &Path,
    settle_steps: u32,
    dt: f32,
) -> HostResult<LoadedRig> {
    let path = base.join(&spec_rig.inx);
    let metadata = std::fs::metadata(&path)
        .map_err(|e| format!("rig {}: stat {}: {e}", spec_rig.id, path.display()))?;
    if metadata.len() > INX_BYTES_MAX {
        return err(format!(
            "rig {}: record is {} bytes, over the {INX_BYTES_MAX}-byte bound",
            spec_rig.id,
            metadata.len()
        ));
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("rig {}: read {}: {e}", spec_rig.id, path.display()))?;
    let digest = Sha256::digest(&bytes);
    let actual = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != spec_rig.sha256 {
        return err(format!(
            "PROBE-HASH-MISMATCH: rig {}: spec expects sha256 {}, record hashes to {}",
            spec_rig.id, spec_rig.sha256, actual
        ));
    }
    let imported = import_inp(&bytes, &spec_rig.id)
        .map_err(|e| format!("rig {}: INP import refused: {e}", spec_rig.id))?;
    let mut textures = Vec::with_capacity(imported.textures.len());
    for tex in &imported.textures {
        let img = image::load_from_memory(&tex.bytes)
            .map_err(|e| format!("rig {}: texture {} decodes: {e}", spec_rig.id, tex.source))?
            .to_rgba8();
        textures.push(TexImage {
            width: img.width() as usize,
            height: img.height() as usize,
            rgba: img.into_raw(),
        });
    }
    // The legality table is the stage's own parameter
    // data: name + x-channel range per parameter (pose
    // values are x-channel sets, as in every scene the
    // seam has staged).
    let table = RigParameterTable {
        rig_id: spec_rig.id.clone(),
        parameters: imported
            .stage
            .parameters
            .iter()
            .map(|p| (p.id.clone(), p.min[0], p.max[0]))
            .collect(),
    };
    let parts = imported
        .stage
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, PuppetNodeKind::Part))
        .count();
    let report = ProbeRigReport {
        id: spec_rig.id.clone(),
        inx: spec_rig.inx.clone(),
        sha256: actual,
        parameters: imported.stage.parameters.len(),
        parts,
    };
    let model = bridge_model(&imported.stage, &format!("bridge-{}", spec_rig.id));
    let mut scratch = PuppetEval::new(&imported.stage, &textures)
        .map_err(|e| format!("rig {}: eval builds: {e}", spec_rig.id))?;
    settle(&mut scratch, settle_steps, dt);
    let bounds = measure_bounds(&mut scratch, &model, dt);
    Ok(LoadedRig {
        report,
        table,
        stage: imported.stage,
        textures,
        model,
        bounds,
    })
}

/// Convert a spec anchor to the seam's endpoint form.
fn to_endpoint(anchor: &ProbeAnchor) -> ContactEndpoint {
    match &anchor.selector {
        AnchorSelector::PartCentroid { part } => {
            ContactEndpoint::PartCentroid { part: part.clone() }
        }
        AnchorSelector::Vertex { mesh, index } => ContactEndpoint::Vertex {
            mesh: mesh.clone(),
            vertex: *index,
        },
    }
}

/// Evaluate one rig at rest (settled, empty pose) and
/// harvest its geometry in scene coordinates.
fn rest_geometry(rig: &LoadedRig, settle_steps: u32, dt: f32) -> HostResult<Vec<EvaluatedMesh>> {
    let mut eval = PuppetEval::new(&rig.stage, &rig.textures)
        .map_err(|e| format!("rig {}: eval builds: {e}", rig.report.id))?;
    settle(&mut eval, settle_steps, dt);
    set_pose(&mut eval, &[]);
    eval.end_frame(dt).map_err(|e| format!("rest frame: {e}"))?;
    Ok(harvest(&eval, &rig.model))
}

/// Signed margin of a point against one part's world
/// triangles (negative = inside the silhouette): the
/// minimum of the C2 instrument's per-triangle signed
/// distance over the part's triangles.
fn signed_margin_to_part(
    model: &Model,
    evaluated: &[EvaluatedMesh],
    placement: &Placement,
    part: &str,
    point: [f64; 2],
) -> Option<f64> {
    let mut best = f64::INFINITY;
    for (declared, eval) in model.meshes.iter().zip(evaluated.iter()) {
        if declared.part != part {
            continue;
        }
        let world: Vec<[f64; 2]> = eval.vertices.iter().map(|v| placement.apply(*v)).collect();
        for tri in declared.indices.as_chunks::<3>().0 {
            let pts = [
                world[tri[0] as usize],
                world[tri[1] as usize],
                world[tri[2] as usize],
            ];
            let margin = point_triangle_signed(point, &pts);
            if margin < best {
                best = margin;
            }
        }
    }
    if best == f64::INFINITY {
        None
    } else {
        Some(best)
    }
}

/// Sample one rig's authored placement at `t`.
fn sample_probe_placement(placement: &ProbePlacement, bounds: &Bounds, t: f64) -> Placement {
    match placement {
        ProbePlacement::Fixed { x, y } => Placement {
            translation: [*x, *y],
            rotation_degrees: 0.0,
        },
        ProbePlacement::PivotTurn { keys, compensation } => {
            let pivot = [bounds.foot_centre_x, -bounds.foot_y];
            sample_pivot_turn(keys, *compensation, bounds.foot_y, pivot, t)
        }
        ProbePlacement::StepInOut {
            base_x,
            step_in,
            step_in_window,
            step_back,
            step_back_window,
        } => sample_step_in_out(
            *base_x,
            *step_in,
            *step_in_window,
            *step_back,
            *step_back_window,
            bounds.foot_y,
            t,
        ),
    }
}

/// The seam offset of one declared contact: target
/// minus the B endpoint's rest point, both B-local
/// scene coordinates (the staging derivation).
fn contact_offset(contact: &ProbeContact, rest_geos: &[Vec<EvaluatedMesh>]) -> [f64; 2] {
    let b_rest = endpoint_world(
        &rest_geos[contact.b.rig],
        &Placement::IDENTITY,
        &to_endpoint(&contact.b),
    );
    [contact.target[0] - b_rest[0], contact.target[1] - b_rest[1]]
}

/// Run a probe spec end to end and return its report.
/// `inx_base` resolves the spec's relative `.inx`
/// paths. An illegal timeline is refused before any
/// evaluation runs.
pub fn run_probe(spec: &ProbeSpec, inx_base: &Path) -> HostResult<ProbeReport> {
    spec.validate_shape().map_err(|e| e.to_string())?;
    let dt = 1.0 / spec.rate as f32;
    let mut rigs: Vec<LoadedRig> = Vec::with_capacity(spec.rigs.len());
    for spec_rig in &spec.rigs {
        rigs.push(load_rig(spec_rig, inx_base, spec.settle_steps, dt)?);
    }
    // Validator-semantics legality, before evaluation.
    let tables: Vec<RigParameterTable> = rigs.iter().map(|r| r.table.clone()).collect();
    spec.validate_timeline(&tables).map_err(|e| e.to_string())?;

    // Rest geometry per rig (contact offsets, band
    // boxes, part resolution checks all read it).
    let mut rest_geos: Vec<Vec<EvaluatedMesh>> = Vec::with_capacity(rigs.len());
    for rig in &rigs {
        rest_geos.push(rest_geometry(rig, spec.settle_steps, dt)?);
    }
    let offsets: Vec<[f64; 2]> = spec
        .contacts
        .iter()
        .map(|c| contact_offset(c, &rest_geos))
        .collect();
    let contact_defs: Vec<ContactDef> = spec
        .contacts
        .iter()
        .zip(offsets.iter())
        .map(|(contact, offset)| ContactDef {
            id: contact.id.clone(),
            a_rig: contact.a.rig,
            a: to_endpoint(&contact.a),
            b_rig: contact.b.rig,
            b: to_endpoint(&contact.b),
            driven_rig: contact.driven_rig,
            offset: *offset,
        })
        .collect();
    let roots: Vec<ContactEndpoint> = spec
        .roots
        .iter()
        .map(|part| ContactEndpoint::PartCentroid { part: part.clone() })
        .collect();
    let models: Vec<Model> = rigs.iter().map(|r| r.model.clone()).collect();
    let scene = Scene::new(models, contact_defs, Vec::new(), roots).map_err(|e| e.to_string())?;

    // Band boxes per pair, from the band part's rest
    // AABB on its rig.
    let mut accumulators: Vec<PairAccumulator> = Vec::with_capacity(spec.pairs.len());
    for pair in &spec.pairs {
        let band_box_local = match &pair.band {
            Some(band) => {
                let mut lo = [f64::INFINITY; 2];
                let mut hi = [f64::NEG_INFINITY; 2];
                let mut found = false;
                for mesh in &rest_geos[band.rig] {
                    if mesh.part != band.part {
                        continue;
                    }
                    found = true;
                    for v in &mesh.vertices {
                        lo = [lo[0].min(v[0]), lo[1].min(v[1])];
                        hi = [hi[0].max(v[0]), hi[1].max(v[1])];
                    }
                }
                if !found {
                    return err(format!(
                        "pair {}: band part '{}' has no evaluated mesh on rig {}",
                        pair.id, band.part, band.rig
                    ));
                }
                Some(band_box((lo, hi), band.near, band.far))
            }
            None => None,
        };
        accumulators.push(PairAccumulator::new(pair, spec.rate, band_box_local));
    }

    // Resolved pose tracks (plain values, record order).
    let tracks: Vec<Vec<ResolvedPoseKey>> = spec
        .rigs
        .iter()
        .map(|rig| {
            rig.pose_track
                .iter()
                .map(|key| ResolvedPoseKey {
                    t: key.t as f32,
                    params: key
                        .params
                        .iter()
                        .map(|(n, v)| (n.clone(), *v as f32))
                        .collect(),
                })
                .collect()
        })
        .collect();

    let mut evals: Vec<PuppetEval> = Vec::with_capacity(rigs.len());
    for rig in &rigs {
        let mut eval = PuppetEval::new(&rig.stage, &rig.textures)
            .map_err(|e| format!("rig {}: eval builds: {e}", rig.report.id))?;
        settle(&mut eval, spec.settle_steps, dt);
        evals.push(eval);
    }

    let mut clearance_series: Vec<f64> = Vec::with_capacity(spec.steps as usize);
    let mut any_engaged: Vec<bool> = Vec::with_capacity(spec.steps as usize);
    for step in 0..spec.steps {
        let t = step as f64 / f64::from(spec.rate);
        let tf = t as f32;
        let mut geos: Vec<Vec<EvaluatedMesh>> = Vec::with_capacity(rigs.len());
        for (index, rig) in rigs.iter().enumerate() {
            set_pose(&mut evals[index], &sample_pose_track(&tracks[index], tf));
            evals[index]
                .end_frame(dt)
                .map_err(|e| format!("step {step}: frame evaluates: {e}"))?;
            geos.push(harvest(&evals[index], &rig.model));
        }
        let placements: Vec<Placement> = spec
            .rigs
            .iter()
            .zip(rigs.iter())
            .map(|(spec_rig, rig)| sample_probe_placement(&spec_rig.placement, &rig.bounds, t))
            .collect();
        let input = FrameInput {
            step,
            time_seconds: t,
            driven: vec![std::collections::BTreeMap::new(); rigs.len()],
            placements,
            active: spec
                .contacts
                .iter()
                .map(|c| (c.active_from..c.active_to).contains(&t))
                .collect(),
        };
        let read = evaluate_frame_with_geometry(&scene, &input, &geos)
            .map_err(|e| format!("step {step}: frame solves: {e}"))?;

        if let Some(clearance_rig) = spec.clearance_rig {
            let mut min_y = f64::INFINITY;
            for mesh in &geos[clearance_rig] {
                if !is_leg_part(&mesh.id) {
                    continue;
                }
                for v in &mesh.vertices {
                    min_y = min_y.min(read.placements[clearance_rig].apply(*v)[1]);
                }
            }
            clearance_series.push(min_y);
        }
        any_engaged.push(
            spec.pairs
                .iter()
                .any(|p| (p.engaged_from..p.engaged_to).contains(&t)),
        );

        for (pair, acc) in spec.pairs.iter().zip(accumulators.iter_mut()) {
            let a_point = endpoint_world(
                &geos[pair.a.rig],
                &read.placements[pair.a.rig],
                &to_endpoint(&pair.a),
            );
            let a_world = [a_point[0] + pair.a_offset[0], a_point[1] + pair.a_offset[1]];
            // Distance: the bound contact's seam slip
            // when no measurement offset is planted; with
            // an offset (or unbound) it is the offset A
            // point's separation from the reference point
            // (the contact's fixed point, or B's anchor).
            let distance = match &pair.contact {
                Some(contact_id) if pair.a_offset == [0.0, 0.0] => read
                    .contacts
                    .iter()
                    .find(|c| &c.id == contact_id)
                    .map(|c| c.slip),
                Some(contact_id) => {
                    let contact_index = spec
                        .contacts
                        .iter()
                        .position(|c| &c.id == contact_id)
                        .expect("pair contact validated");
                    let contact = &spec.contacts[contact_index];
                    let b_local = endpoint_world(
                        &geos[contact.b.rig],
                        &Placement::IDENTITY,
                        &to_endpoint(&contact.b),
                    );
                    let fixed_local = [
                        b_local[0] + offsets[contact_index][0],
                        b_local[1] + offsets[contact_index][1],
                    ];
                    let fixed_world = read.placements[contact.b.rig].apply(fixed_local);
                    Some((a_world[0] - fixed_world[0]).hypot(a_world[1] - fixed_world[1]))
                }
                None => {
                    let b_world = endpoint_world(
                        &geos[pair.b.rig],
                        &read.placements[pair.b.rig],
                        &to_endpoint(&pair.b),
                    );
                    Some((a_world[0] - b_world[0]).hypot(a_world[1] - b_world[1]))
                }
            };
            let band = match (&pair.band, acc.band_box_local()) {
                (Some(band), Some(bbox)) => {
                    let corners = [
                        [bbox[0], bbox[1]],
                        [bbox[0], bbox[3]],
                        [bbox[2], bbox[1]],
                        [bbox[2], bbox[3]],
                    ];
                    let mut blo = [f64::INFINITY; 2];
                    let mut bhi = [f64::NEG_INFINITY; 2];
                    for corner in corners {
                        let w = read.placements[band.rig].apply(corner);
                        blo = [blo[0].min(w[0]), blo[1].min(w[1])];
                        bhi = [bhi[0].max(w[0]), bhi[1].max(w[1])];
                    }
                    Some(box_distance(a_world, blo, bhi))
                }
                _ => None,
            };
            let penetration = pair.penetration_part.as_ref().and_then(|part| {
                signed_margin_to_part(
                    &rigs[pair.b.rig].model,
                    &geos[pair.b.rig],
                    &read.placements[pair.b.rig],
                    part,
                    a_world,
                )
            });
            acc.push(step, a_world, distance, band, penetration);
        }
    }

    let clearance_min_engaged = if spec.clearance_rig.is_some() {
        let mut min = f64::INFINITY;
        for (index, engaged) in any_engaged.iter().enumerate() {
            if *engaged {
                min = min.min(clearance_series[index]);
            }
        }
        if min == f64::INFINITY {
            None
        } else {
            Some(min)
        }
    } else {
        None
    };

    let pair_reports: Vec<_> = accumulators
        .into_iter()
        .map(|acc| acc.finish(true))
        .collect();

    let checkpoints = spec
        .checkpoints
        .iter()
        .map(|checkpoint| {
            let step = checkpoint.step as usize;
            let mut pairs = std::collections::BTreeMap::new();
            for report in &pair_reports {
                pairs.insert(
                    report.id.clone(),
                    ProbeStepRead {
                        distance: report.distance_series.get(step).copied().flatten(),
                        band_distance: report.band_series.get(step).copied().flatten(),
                        penetration: report.penetration_series.get(step).copied().flatten(),
                    },
                );
            }
            ProbeCheckpointRead {
                step: checkpoint.step,
                label: checkpoint.label.clone(),
                clearance: clearance_series.get(step).copied(),
                pairs,
            }
        })
        .collect();

    Ok(ProbeReport {
        version: spec.version,
        spec_id: spec.id.clone(),
        steps: spec.steps,
        rigs: rigs.iter().map(|r| r.report.clone()).collect(),
        pairs: pair_reports,
        clearance_rig: spec.clearance_rig,
        clearance_min_engaged,
        clearance_series: if spec.clearance_rig.is_some() {
            Some(clearance_series)
        } else {
            None
        },
        checkpoints,
    })
}
