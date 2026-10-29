//! Canon machinery (B2, D-292/D-312): the SFW canon store, the
//! measurement gate (proportion + register bands), and the anchored
//! generation broker with workflow-JSON provenance receipts.
//!
//! The format, handle protocol, lane law and band math live in the
//! absorbed B1 crate (`canon_record`, crates/lumen-canon); this
//! module is the server surface over it (canon SPEC §5/§6):
//!
//! - Records are homed by lane. The store under the project root
//!   (`canon/<lane>/<character>.json`) is lumen's SFW home; an
//!   nsfw record fetched by handle is used transiently for the call
//!   and never written back by this module — lumen persists no
//!   adult record or chain (SPEC §5 write-back goes to the homed
//!   registry, which for adult records is not this store).
//! - `canon_generate` has NO unanchored mode. A job is refused
//!   pre-generation, with a typed refusal, when: it names no anchor
//!   role (`unanchored_lineage`); its inputs lack the selected
//!   anchor's hash or any style-authority hash
//!   (`missing_keeper_anchor`); an input presented as an anchor is
//!   outside the Owner-adopted keeper set (`anchor_not_adopted`,
//!   PIPELINE §11 rule 10); the record is nsfw and the caller is
//!   qompass-lane (`lane_mismatch`); or the record hash does not
//!   match the handle pin (`record_stale`).
//! - Reference conditioning (ComfyUI intake item 1): turnaround
//!   and plate inputs are first-class job inputs, required, and
//!   hash-pinned in the receipt (intake item 5): every admitted or
//!   gate-refused job persists a workflow-JSON receipt whose
//!   `receipt_sha256` makes tampering detectable.
//! - The broker validates, gates and records; it does not itself
//!   run a generation backend and it never generates characters in
//!   tests. Outputs supplied by a backend are gated (proportion +
//!   register) before the job returns as admitted; a drifted
//!   output returns as a `drift_detected` refusal with its receipt
//!   persisted as evidence.
//!
//! All functions take the project root explicitly, like the rest
//! of the library, and fail closed with typed `canon_record`
//! refusals.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use canon_record::{
    CanonHandle, CanonRecord, ChainParams, GateVerdict, Lane, Measurements, Refusal, gate_all,
    resolve, sha256_hex,
};
use serde::{Deserialize, Serialize};

/// Receipt format tag (ComfyUI intake item 5 shape, estate-owned).
pub const RECEIPT_FORMAT: &str = "lumen-generation-receipt/1";

fn caller_lane(text: &str) -> Result<Lane, Refusal> {
    text.parse::<Lane>().map_err(|_| Refusal::MalformedRecord {
        detail: format!("unknown caller lane: {text}"),
    })
}

/// Where a record lives in the store. Lane and character come from
/// the parsed handle, so no segment can carry a path separator.
fn store_path(root: &Path, handle: &CanonHandle) -> PathBuf {
    root.join("canon")
        .join(handle.lane.as_str())
        .join(format!("{}.json", handle.character))
}

fn load_resolved(
    root: &Path,
    handle_text: &str,
    record_sha256: &str,
    caller: &str,
) -> Result<CanonRecord, Refusal> {
    let handle = CanonHandle::parse(handle_text)?;
    let caller = caller_lane(caller)?;
    let path = store_path(root, &handle);
    let bytes = std::fs::read(&path).map_err(|e| Refusal::RecordStale {
        detail: format!("no record in the store for {handle}: {e}"),
    })?;
    resolve(handle_text, record_sha256, &bytes, caller)
}

/// Hashes of the Owner-adopted keeper set: record anchors plus
/// adopted keeper-chain entries (PIPELINE §11 rule 10).
fn adopted_hashes(record: &CanonRecord) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = record.anchors.iter().map(|a| a.sha256.clone()).collect();
    for entry in &record.keeper_chain {
        if entry.adopted {
            set.insert(entry.sha256.clone());
        }
    }
    set
}

// ---------------------------------------------------------------------
// canon_resolve
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveRequest {
    pub handle: String,
    pub record_sha256: String,
    /// `qompass` or `ntranch` — enforced in server code, never
    /// trusted from the record (SPEC §2.3).
    pub caller_lane: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolveSummary {
    pub id: String,
    pub handle: String,
    pub version: u64,
    pub record_sha256: String,
    pub origin_lane: String,
    pub content_label: String,
    pub anchor_count: usize,
    pub proportion_band_count: usize,
    pub register_band_count: usize,
    pub adopted_anchor_hashes: Vec<String>,
    pub chain_head_sha256: Option<String>,
}

/// `canon_resolve`: handle + pinned hash -> record summary, with
/// the lane check of SPEC §2.3.
pub fn canon_resolve(root: &Path, req: &ResolveRequest) -> Result<ResolveSummary, Refusal> {
    let record = load_resolved(root, &req.handle, &req.record_sha256, &req.caller_lane)?;
    Ok(ResolveSummary {
        id: record.id.clone(),
        handle: req.handle.clone(),
        version: record.version,
        record_sha256: req.record_sha256.clone(),
        origin_lane: record.origin_lane.to_string(),
        content_label: format!("{:?}", record.effective_label()).to_lowercase(),
        anchor_count: record.anchors.len(),
        proportion_band_count: record.style_fingerprint.proportion_bands.len(),
        register_band_count: record.style_fingerprint.register_bands.len(),
        adopted_anchor_hashes: adopted_hashes(&record).into_iter().collect(),
        chain_head_sha256: record.keeper_chain.last().map(|e| e.sha256.clone()),
    })
}

// ---------------------------------------------------------------------
// canon_gate
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateRequest {
    pub handle: String,
    pub record_sha256: String,
    pub caller_lane: String,
    /// The anchor (by hash) this artifact declares lineage from.
    /// Absent = unanchored = refused (SPEC §6).
    pub anchor_sha256: Option<String>,
    pub measurements: Measurements,
}

/// `canon_gate`: proportion + register bands in one verdict
/// (D-312). PASS releases to the eye sheet, never to adoption
/// (SPEC §6.1).
pub fn canon_gate(root: &Path, req: &GateRequest) -> Result<GateVerdict, Refusal> {
    let record = load_resolved(root, &req.handle, &req.record_sha256, &req.caller_lane)?;
    let anchor = req
        .anchor_sha256
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or(Refusal::UnanchoredLineage)?;
    if !adopted_hashes(&record).contains(anchor) {
        return Err(Refusal::AnchorNotAdopted {
            sha256: anchor.to_string(),
        });
    }
    if record.style_fingerprint.register_bands.is_empty() {
        return Err(Refusal::MalformedRecord {
            detail: "record carries no register bands; register enforcement (D-312) cannot pass what it does not score".to_string(),
        });
    }
    Ok(gate_all(&record, &req.measurements))
}

// ---------------------------------------------------------------------
// canon_generate — anchored broker + provenance receipt
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputKind {
    Anchor,
    Turnaround,
    Plate,
    Reference,
    StyleAuthority,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobInput {
    pub kind: InputKind,
    pub handle: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobOutput {
    pub handle: String,
    pub sha256: String,
    pub measurements: Measurements,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateRequest {
    pub handle: String,
    pub record_sha256: String,
    pub caller_lane: String,
    /// Anchor role in the record (e.g. `primary`). Empty/absent is
    /// unanchored generation, which does not exist on this surface.
    pub anchor_role: String,
    pub backend: String,
    pub params: ChainParams,
    pub inputs: Vec<JobInput>,
    #[serde(default)]
    pub outputs: Vec<JobOutput>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReceiptOutput {
    pub handle: String,
    pub sha256: String,
    pub gate: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GenerationReceipt {
    pub format: String,
    pub job_id: String,
    /// `admitted` (validated pre-generation, or outputs gated
    /// PASS) or `refused_drift` (outputs gated FAIL; job returned
    /// as a refusal, receipt persists as evidence).
    pub status: String,
    pub handle: String,
    pub record_id: String,
    pub record_version: u64,
    pub record_sha256: String,
    pub anchor_role: String,
    pub anchor_sha256: String,
    pub backend: String,
    pub params: ChainParams,
    pub inputs: Vec<JobInput>,
    pub outputs: Vec<ReceiptOutput>,
    /// ComfyUI-workflow-shaped provenance (intake item 5): the
    /// replayable graph of the job — resolve, reference
    /// conditioning, generate, gate — with anchors by hash.
    pub workflow: serde_json::Value,
    /// SHA-256 over the receipt with this field blanked; any later
    /// edit to the persisted receipt is detectable.
    pub receipt_sha256: String,
}

fn receipt_digest(receipt: &GenerationReceipt) -> String {
    let mut blanked = receipt.clone();
    blanked.receipt_sha256 = String::new();
    let text = serde_json::to_string(&blanked).unwrap_or_default();
    sha256_hex(text.as_bytes())
}

fn persist_receipt(root: &Path, receipt: &GenerationReceipt) -> Result<PathBuf, Refusal> {
    let dir = root.join("canon-receipts");
    std::fs::create_dir_all(&dir).map_err(|e| Refusal::MalformedRecord {
        detail: format!("receipt store unavailable: {e}"),
    })?;
    let path = dir.join(format!("{}.json", receipt.job_id));
    let text = serde_json::to_string_pretty(receipt).map_err(|e| Refusal::MalformedRecord {
        detail: format!("receipt does not serialize: {e}"),
    })?;
    std::fs::write(&path, text).map_err(|e| Refusal::MalformedRecord {
        detail: format!("receipt does not persist: {e}"),
    })?;
    Ok(path)
}

/// Read a persisted receipt back and verify its digest. Any edit
/// to the file after persistence is `receipt_tampered`.
pub fn verify_receipt(root: &Path, job_id: &str) -> Result<GenerationReceipt, Refusal> {
    if job_id.is_empty()
        || !job_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Refusal::MalformedRecord {
            detail: format!("job_id is not a digest: {job_id}"),
        });
    }
    let path = root.join("canon-receipts").join(format!("{job_id}.json"));
    let bytes = std::fs::read(&path).map_err(|e| Refusal::RecordStale {
        detail: format!("no receipt for job {job_id}: {e}"),
    })?;
    let receipt: GenerationReceipt =
        serde_json::from_slice(&bytes).map_err(|e| Refusal::ReceiptTampered {
            detail: format!("receipt does not parse: {e}"),
        })?;
    if receipt.job_id != job_id {
        return Err(Refusal::ReceiptTampered {
            detail: "receipt job_id does not match its file name".to_string(),
        });
    }
    let actual = receipt_digest(&receipt);
    if actual != receipt.receipt_sha256 {
        return Err(Refusal::ReceiptTampered {
            detail: format!(
                "receipt digest mismatch: stored {}, recomputed {actual}",
                receipt.receipt_sha256
            ),
        });
    }
    Ok(receipt)
}

/// `canon_generate`: validate the anchored job, gate any supplied
/// outputs, persist the workflow-JSON receipt, and return it. All
/// refusals are pre-generation except output drift, which is
/// refused at the gate before the job can return as admitted.
pub fn canon_generate(root: &Path, req: &GenerateRequest) -> Result<GenerationReceipt, Refusal> {
    let record = load_resolved(root, &req.handle, &req.record_sha256, &req.caller_lane)?;

    // No unanchored mode (SPEC §6/§6.1).
    if req.anchor_role.trim().is_empty() {
        return Err(Refusal::UnanchoredLineage);
    }
    let anchor = record
        .anchors
        .iter()
        .find(|a| a.role == req.anchor_role)
        .ok_or_else(|| Refusal::MalformedRecord {
            detail: format!("anchor role {:?} is not in the record", req.anchor_role),
        })?;

    // Input shape: every input hash-pinned.
    for input in &req.inputs {
        if !canon_record::is_sha256_hex(&input.sha256) {
            return Err(Refusal::MalformedRecord {
                detail: format!("input {:?} sha256 is not a digest", input.handle),
            });
        }
    }
    let input_hashes: BTreeSet<&str> = req.inputs.iter().map(|i| i.sha256.as_str()).collect();

    // Keeper hashes must ride the job (SPEC §6.1): the selected
    // anchor plus every style-authority ref.
    let mut missing: Vec<String> = Vec::new();
    if !input_hashes.contains(anchor.sha256.as_str()) {
        missing.push(anchor.sha256.clone());
    }
    for refr in &record.authority_refs {
        if !input_hashes.contains(refr.sha256.as_str()) {
            missing.push(refr.sha256.clone());
        }
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        return Err(Refusal::MissingKeeperAnchor { missing });
    }

    // Anchor law (§11 rule 10): anything presented as an anchor
    // must be in the Owner-adopted keeper set.
    let adopted = adopted_hashes(&record);
    for input in &req.inputs {
        if input.kind == InputKind::Anchor && !adopted.contains(&input.sha256) {
            return Err(Refusal::AnchorNotAdopted {
                sha256: input.sha256.clone(),
            });
        }
    }

    // Reference conditioning (intake item 1): turnaround + plates
    // are required first-class inputs.
    let has = |kind: InputKind| req.inputs.iter().any(|i| i.kind == kind);
    if !has(InputKind::Turnaround) || !has(InputKind::Plate) {
        return Err(Refusal::MalformedRecord {
            detail: "reference conditioning requires at least one turnaround and one plate input (ComfyUI intake item 1)".to_string(),
        });
    }
    if record.style_fingerprint.register_bands.is_empty() {
        return Err(Refusal::MalformedRecord {
            detail: "record carries no register bands; register enforcement (D-312) cannot pass what it does not score".to_string(),
        });
    }

    // Deterministic job id over the canonical job description.
    let mut sorted_inputs = req.inputs.clone();
    sorted_inputs.sort_by(|a, b| {
        (format!("{:?}", a.kind), &a.handle, &a.sha256).cmp(&(
            format!("{:?}", b.kind),
            &b.handle,
            &b.sha256,
        ))
    });
    let job_seed = serde_json::json!({
        "handle": req.handle,
        "record_sha256": req.record_sha256,
        "anchor_role": req.anchor_role,
        "anchor_sha256": anchor.sha256,
        "backend": req.backend,
        "params": req.params,
        "inputs": sorted_inputs,
    });
    let job_id = sha256_hex(
        serde_json::to_string(&job_seed)
            .unwrap_or_default()
            .as_bytes(),
    );

    // Gate any backend-supplied outputs before admission.
    let mut receipt_outputs = Vec::new();
    let mut drift: Option<Refusal> = None;
    for output in &req.outputs {
        if !canon_record::is_sha256_hex(&output.sha256) {
            return Err(Refusal::MalformedRecord {
                detail: format!("output {:?} sha256 is not a digest", output.handle),
            });
        }
        match gate_all(&record, &output.measurements) {
            GateVerdict::Pass => receipt_outputs.push(ReceiptOutput {
                handle: output.handle.clone(),
                sha256: output.sha256.clone(),
                gate: "pass".to_string(),
            }),
            GateVerdict::Refused(r) => {
                receipt_outputs.push(ReceiptOutput {
                    handle: output.handle.clone(),
                    sha256: output.sha256.clone(),
                    gate: "refused".to_string(),
                });
                drift = Some(r);
            }
        }
    }

    let workflow = serde_json::json!({
        "format": "comfy-workflow-json/1",
        "nodes": [
            {"id": "canon_resolve", "handle": req.handle, "record_sha256": req.record_sha256},
            {"id": "reference_conditioning", "inputs": sorted_inputs},
            {"id": "generate", "backend": req.backend, "params": req.params,
             "anchor_role": req.anchor_role, "anchor_sha256": anchor.sha256},
            {"id": "canon_gate", "classes": ["proportion", "register"]},
        ],
        "anchors_by_hash": adopted.into_iter().collect::<Vec<_>>(),
    });

    let mut receipt = GenerationReceipt {
        format: RECEIPT_FORMAT.to_string(),
        job_id: job_id.clone(),
        status: if drift.is_some() {
            "refused_drift"
        } else {
            "admitted"
        }
        .to_string(),
        handle: req.handle.clone(),
        record_id: record.id.clone(),
        record_version: record.version,
        record_sha256: req.record_sha256.clone(),
        anchor_role: req.anchor_role.clone(),
        anchor_sha256: anchor.sha256.clone(),
        backend: req.backend.clone(),
        params: req.params.clone(),
        inputs: sorted_inputs,
        outputs: receipt_outputs,
        workflow,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt_digest(&receipt);
    persist_receipt(root, &receipt)?;

    match drift {
        Some(r) => Err(r),
        None => Ok(receipt),
    }
}

// ---------------------------------------------------------------------
// canon_convert (R-607, SPEC §6 fourth tool)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConvertRequest {
    pub handle: String,
    pub record_sha256: String,
    pub caller_lane: String,
    /// The anchor (by hash) the input artifact declares lineage
    /// from. Absent = unanchored = refused, exactly as the gate.
    pub anchor_sha256: Option<String>,
    /// Project-relative PNG input.
    pub input_path: String,
    /// Project-relative PNG output (written only on gate PASS).
    pub output_path: String,
    /// Measurements of the converted artifact for the band gate.
    pub measurements: Measurements,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvertReport {
    pub input_path: String,
    pub output_path: String,
    /// Governing palette colours consumed from the fingerprint.
    pub palette_entries: usize,
    /// Opaque pixels examined.
    pub pixels_total: usize,
    /// Pixels whose colour changed (snapped to a palette colour).
    pub pixels_remapped: usize,
    /// Opaque pixels outside every palette colour's tolerance;
    /// they keep their colour (the record governs partially,
    /// never silently).
    pub pixels_outside_palette: usize,
    /// The band gate's verdict on the converted artifact.
    pub gate: String,
}

fn parse_hex_rgb(hex: &str) -> Option<[u8; 3]> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&h[0..2], 16).ok()?,
        u8::from_str_radix(&h[2..4], 16).ok()?,
        u8::from_str_radix(&h[4..6], 16).ok()?,
    ])
}

/// `canon_convert`: the conversion leg of the canon surface
/// (SPEC §6). With a handle, the conversion consumes the
/// record's fingerprint: the palette governs a colour remap of
/// a project PNG (each opaque pixel snaps to its nearest
/// palette colour when within that colour's tolerance,
/// normalized RGB distance in [0, 1]; pixels outside every
/// tolerance keep their colour and are counted), and the
/// converted artifact is gated against the record's bands
/// before anything is written — a drift refusal writes no
/// output. Lane law is `load_resolved`'s, identical to the
/// other canon tools: an nsfw record is refused to a qompass
/// caller before any file is touched. Without a handle there
/// is no conversion on this surface at all (the handle is a
/// required field); the pre-canon conversion tools keep their
/// existing unhandled behaviour, per SPEC §6.
pub fn canon_convert(root: &Path, req: &ConvertRequest) -> Result<ConvertReport, Refusal> {
    let record = load_resolved(root, &req.handle, &req.record_sha256, &req.caller_lane)?;
    let anchor = req
        .anchor_sha256
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or(Refusal::UnanchoredLineage)?;
    if !adopted_hashes(&record).contains(anchor) {
        return Err(Refusal::AnchorNotAdopted {
            sha256: anchor.to_string(),
        });
    }
    if record.style_fingerprint.register_bands.is_empty() {
        return Err(Refusal::MalformedRecord {
            detail: "record carries no register bands; register enforcement (D-312) cannot pass what it does not score".to_string(),
        });
    }
    if record.style_fingerprint.palette.is_empty() {
        return Err(Refusal::MalformedRecord {
            detail: "record carries no palette; canon conversion has nothing to consume"
                .to_string(),
        });
    }
    // Gate first: a drifted conversion writes nothing.
    match gate_all(&record, &req.measurements) {
        GateVerdict::Pass => {}
        GateVerdict::Refused(r) => return Err(r),
    }

    let map_err = |e: crate::LumenError| Refusal::MalformedRecord {
        detail: e.to_string(),
    };
    let input = crate::resolve_sprite_path(&req.input_path, root).map_err(map_err)?;
    let output = crate::resolve_write_path(&req.output_path, root, &["png"]).map_err(map_err)?;
    let mut img = image::open(&input)
        .map_err(|e| Refusal::MalformedRecord {
            detail: format!("input png does not decode: {e}"),
        })?
        .to_rgba8();

    let mut palette: Vec<([u8; 3], f64)> =
        Vec::with_capacity(record.style_fingerprint.palette.len());
    for colour in &record.style_fingerprint.palette {
        let rgb = parse_hex_rgb(&colour.hex).ok_or_else(|| Refusal::MalformedRecord {
            detail: format!(
                "palette colour {:?} hex {:?} is not #RRGGBB",
                colour.role, colour.hex
            ),
        })?;
        palette.push((rgb, colour.tolerance));
    }

    let norm = (3.0f64 * 255.0 * 255.0).sqrt();
    let (mut total, mut remapped, mut outside) = (0usize, 0usize, 0usize);
    for pixel in img.pixels_mut() {
        if pixel[3] == 0 {
            continue;
        }
        total += 1;
        let mut best: Option<([u8; 3], f64)> = None;
        for (rgb, tolerance) in &palette {
            let d = ((f64::from(pixel[0]) - f64::from(rgb[0])).powi(2)
                + (f64::from(pixel[1]) - f64::from(rgb[1])).powi(2)
                + (f64::from(pixel[2]) - f64::from(rgb[2])).powi(2))
            .sqrt()
                / norm;
            if d <= *tolerance && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((*rgb, d));
            }
        }
        match best {
            Some((rgb, _)) => {
                if [pixel[0], pixel[1], pixel[2]] != rgb {
                    remapped += 1;
                }
                pixel[0] = rgb[0];
                pixel[1] = rgb[1];
                pixel[2] = rgb[2];
            }
            None => outside += 1,
        }
    }
    img.save(&output).map_err(|e| Refusal::MalformedRecord {
        detail: format!("converted png does not write: {e}"),
    })?;

    Ok(ConvertReport {
        input_path: req.input_path.clone(),
        output_path: req.output_path.clone(),
        palette_entries: palette.len(),
        pixels_total: total,
        pixels_remapped: remapped,
        pixels_outside_palette: outside,
        gate: "pass".to_string(),
    })
}
