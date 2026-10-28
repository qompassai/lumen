//! Versioned API-format workflow templates and the only patching atelier does.
//!
//! Contract: a template is a JSON object mapping node IDs to
//! `{class_type, inputs[, _meta]}`. Validation requires the exact node set
//! (IDs and classes) the template spec names, every graph link
//! (`[node_id, output_index]`) to target a node in the map, and every named
//! patch point to exist as a literal (non-link) input. UI-format workflow
//! JSON (`nodes`/`links` arrays) is refused, never converted. Patching
//! writes only the named patch points; every other byte of the graph is
//! preserved.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::generation::admission::sha256_bytes;
use crate::generation::error::{AtelierError, ErrorCode, Result, err};

/// Samplers and schedulers S1 admits (a subset of ComfyUI KSampler's lists).
pub const SAMPLERS: [&str; 6] = [
    "euler",
    "euler_ancestral",
    "dpmpp_2m",
    "dpmpp_2m_sde",
    "dpmpp_sde",
    "ddim",
];
pub const SCHEDULERS: [&str; 5] = ["normal", "karras", "exponential", "sgm_uniform", "simple"];
/// CFG operating range (inclusive).
pub const CFG_MIN: f64 = 1.0;
pub const CFG_MAX: f64 = 15.0;
/// img2img denoise operating range (inclusive), inside the [0, 1] contract.
pub const DENOISE_MIN: f64 = 0.05;
pub const DENOISE_MAX: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateKind {
    Txt2Img,
    Img2Img,
}

/// A named patch point: one literal input of one node.
#[derive(Debug, Clone, Copy)]
pub struct PatchPoint {
    pub name: &'static str,
    pub node: &'static str,
    pub input: &'static str,
}

/// Static description of one template.
#[derive(Debug)]
pub struct TemplateSpec {
    pub name: &'static str,
    pub version: &'static str,
    pub kind: TemplateKind,
    pub text: &'static str,
    pub nodes: &'static [(&'static str, &'static str)],
    pub patch_points: &'static [PatchPoint],
}

const fn point(name: &'static str, node: &'static str, input: &'static str) -> PatchPoint {
    PatchPoint { name, node, input }
}

const SAMPLER_POINTS: [PatchPoint; 9] = [
    point("checkpoint", "4", "ckpt_name"),
    point("positive", "6", "text"),
    point("negative", "7", "text"),
    point("seed", "3", "seed"),
    point("steps", "3", "steps"),
    point("cfg", "3", "cfg"),
    point("sampler_name", "3", "sampler_name"),
    point("scheduler", "3", "scheduler"),
    point("denoise", "3", "denoise"),
];

const fn txt2img_points() -> [PatchPoint; 12] {
    let s = SAMPLER_POINTS;
    [
        s[0],
        s[1],
        s[2],
        s[3],
        s[4],
        s[5],
        s[6],
        s[7],
        s[8],
        point("width", "5", "width"),
        point("height", "5", "height"),
        point("filename_prefix", "9", "filename_prefix"),
    ]
}

const fn img2img_points() -> [PatchPoint; 13] {
    let s = SAMPLER_POINTS;
    [
        s[0],
        s[1],
        s[2],
        s[3],
        s[4],
        s[5],
        s[6],
        s[7],
        s[8],
        point("width", "12", "width"),
        point("height", "12", "height"),
        point("filename_prefix", "9", "filename_prefix"),
        point("image", "10", "image"),
    ]
}

const TXT2IMG_POINTS: [PatchPoint; 12] = txt2img_points();
const IMG2IMG_POINTS: [PatchPoint; 13] = img2img_points();

pub static IRIS_TXT2IMG: TemplateSpec = TemplateSpec {
    name: "iris_txt2img",
    version: "1",
    kind: TemplateKind::Txt2Img,
    text: include_str!("../../assets/workflows/iris_txt2img.v1.json"),
    nodes: &[
        ("3", "KSampler"),
        ("4", "CheckpointLoaderSimple"),
        ("5", "EmptyLatentImage"),
        ("6", "CLIPTextEncode"),
        ("7", "CLIPTextEncode"),
        ("8", "VAEDecode"),
        ("9", "SaveImage"),
    ],
    patch_points: &TXT2IMG_POINTS,
};

pub static IRIS_IMG2IMG: TemplateSpec = TemplateSpec {
    name: "iris_img2img",
    version: "1",
    kind: TemplateKind::Img2Img,
    text: include_str!("../../assets/workflows/iris_img2img.v1.json"),
    nodes: &[
        ("3", "KSampler"),
        ("4", "CheckpointLoaderSimple"),
        ("6", "CLIPTextEncode"),
        ("7", "CLIPTextEncode"),
        ("8", "VAEDecode"),
        ("9", "SaveImage"),
        ("10", "LoadImage"),
        ("11", "VAEEncode"),
        ("12", "ImageScale"),
    ],
    patch_points: &IMG2IMG_POINTS,
};

/// A validated template graph plus its identity.
#[derive(Debug, Clone)]
pub struct Template {
    pub name: &'static str,
    pub version: &'static str,
    pub kind: TemplateKind,
    pub sha256: String,
    pub graph: Map<String, Value>,
    patch_points: &'static [PatchPoint],
}

/// Template identity recorded in every manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TemplateIdentity {
    pub name: String,
    pub version: String,
    pub sha256: String,
}

fn is_link(value: &Value) -> Option<(&str, &Value)> {
    match value.as_array().map(Vec::as_slice) {
        Some([Value::String(node), index]) if index.is_u64() => Some((node.as_str(), index)),
        _ => None,
    }
}

/// Validate `text` against `spec`. Used for the embedded templates and, in
/// tests, for hostile variants.
pub fn validate(spec: &'static TemplateSpec, text: &str) -> Result<Template> {
    let value: Value = serde_json::from_str(text).map_err(|cause| {
        AtelierError::new(
            ErrorCode::TemplateNotApiFormat,
            format!("not JSON: {cause}"),
        )
    })?;
    let Value::Object(graph) = value else {
        return err(
            ErrorCode::TemplateNotApiFormat,
            "template root is not an object",
        );
    };
    if graph.contains_key("nodes") || graph.contains_key("links") {
        return err(
            ErrorCode::TemplateNotApiFormat,
            "UI-format workflow (nodes/links) refused; only API format is accepted",
        );
    }
    for (node_id, node) in &graph {
        let Some(object) = node.as_object() else {
            return err(
                ErrorCode::TemplateNotApiFormat,
                format!("node {node_id} is not an object"),
            );
        };
        let class_ok = object.get("class_type").is_some_and(Value::is_string);
        let inputs_ok = object.get("inputs").is_some_and(Value::is_object);
        let keys_ok = object
            .keys()
            .all(|key| matches!(key.as_str(), "class_type" | "inputs" | "_meta"));
        if !class_ok || !inputs_ok || !keys_ok {
            return err(
                ErrorCode::TemplateNotApiFormat,
                format!("node {node_id} lacks API-format class_type/inputs"),
            );
        }
    }
    check_node_set(spec, &graph)?;
    check_links(&graph)?;
    check_patch_points(spec, &graph)?;
    Ok(Template {
        name: spec.name,
        version: spec.version,
        kind: spec.kind,
        sha256: sha256_bytes(text.as_bytes()),
        graph,
        patch_points: spec.patch_points,
    })
}

fn check_node_set(spec: &TemplateSpec, graph: &Map<String, Value>) -> Result<()> {
    let expected: BTreeMap<&str, &str> = spec.nodes.iter().copied().collect();
    for (node_id, node) in graph {
        let class = node["class_type"].as_str().unwrap_or_default();
        match expected.get(node_id.as_str()) {
            Some(expected_class) if *expected_class == class => {}
            _ => {
                return err(
                    ErrorCode::TemplateUnexpectedNode,
                    format!("node {node_id} ({class}) is not in the {} spec", spec.name),
                );
            }
        }
    }
    for (node_id, class) in &expected {
        if !graph.contains_key(*node_id) {
            return err(
                ErrorCode::TemplateUnexpectedNode,
                format!("spec node {node_id} ({class}) is missing"),
            );
        }
    }
    Ok(())
}

fn check_links(graph: &Map<String, Value>) -> Result<()> {
    for (node_id, node) in graph {
        let inputs = node["inputs"].as_object().map(|inputs| inputs.iter());
        for (input, value) in inputs.into_iter().flatten() {
            if value.is_array() && is_link(value).is_none() {
                return err(
                    ErrorCode::TemplateLinkInvalid,
                    format!("node {node_id} input {input} is a malformed link"),
                );
            }
            if let Some((target, _)) = is_link(value)
                && !graph.contains_key(target)
            {
                return err(
                    ErrorCode::TemplateLinkInvalid,
                    format!("node {node_id} input {input} links to absent node {target}"),
                );
            }
        }
    }
    Ok(())
}

fn check_patch_points(spec: &TemplateSpec, graph: &Map<String, Value>) -> Result<()> {
    for patch in spec.patch_points {
        let value = graph
            .get(patch.node)
            .and_then(|node| node["inputs"].get(patch.input));
        match value {
            Some(literal) if is_link(literal).is_none() && !literal.is_array() => {}
            _ => {
                return err(
                    ErrorCode::TemplatePatchPointMissing,
                    format!(
                        "patch point {} ({}.{}) is absent or a link",
                        patch.name, patch.node, patch.input
                    ),
                );
            }
        }
    }
    Ok(())
}

impl Template {
    pub fn embedded(spec: &'static TemplateSpec) -> Template {
        // The embedded templates are compiled in; failing here is a build defect.
        match validate(spec, spec.text) {
            Ok(template) => template,
            Err(error) => panic!("embedded template {} invalid: {error}", spec.name),
        }
    }

    pub fn identity(&self) -> TemplateIdentity {
        TemplateIdentity {
            name: self.name.to_string(),
            version: self.version.to_string(),
            sha256: self.sha256.clone(),
        }
    }

    fn point(&self, name: &str) -> &PatchPoint {
        let found = self.patch_points.iter().find(|patch| patch.name == name);
        assert!(found.is_some(), "unknown patch point {name}");
        found.expect("asserted above")
    }

    /// The template's literal default for a patch point.
    pub fn default_of(&self, name: &str) -> &Value {
        let patch = self.point(name);
        &self.graph[patch.node]["inputs"][patch.input]
    }

    /// Return a copy of the graph with exactly the named points replaced.
    /// Every name must be a declared patch point (asserted: callers are
    /// atelier code, not tool input).
    pub fn patch(&self, values: &[(&str, Value)]) -> Map<String, Value> {
        let mut graph = self.graph.clone();
        for (name, value) in values {
            let patch = self.point(name);
            let inputs = graph
                .get_mut(patch.node)
                .and_then(|node| node.get_mut("inputs"))
                .and_then(Value::as_object_mut);
            assert!(inputs.is_some(), "validated node {} vanished", patch.node);
            if let Some(inputs) = inputs {
                inputs.insert(patch.input.to_string(), value.clone());
            }
        }
        graph
    }

    /// Node classes the engine must provide (for `/object_info` preflight).
    pub fn node_classes(&self) -> Vec<String> {
        let mut classes: Vec<String> = self
            .graph
            .values()
            .filter_map(|node| node["class_type"].as_str().map(str::to_string))
            .collect();
        classes.sort();
        classes.dedup();
        classes
    }
}
