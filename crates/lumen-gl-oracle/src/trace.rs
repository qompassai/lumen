//! Draw traces: the renderer-consumed fields branch 1 cannot inject
//! (draw order, composite child order, mask structure, opacity).
//!
//! The inox2d side is OBSERVED, not reimplemented: [`Recorder`] is an
//! `InoxRenderer` driven by inox2d's own `InoxRendererExt::draw`
//! dispatch (render.rs:366), so its trace is exactly the call sequence
//! the GL renderer receives. The lumen side is built from lumen's
//! evaluated state (`OracleState`) along the same dispatch structure.
//! Equal traces mean the un-injected fields carry lumen's values too.

use std::cell::RefCell;

use inox2d::node::InoxNodeUuid;
use inox2d::node::components::{Mask, MaskMode, Masks};
use inox2d::node::drawables::{CompositeComponents, TexturedMeshComponents};
use inox2d::render::{CompositeRenderCtx, InoxRenderer, TexturedMeshRenderCtx};
use lumen_rig_core::{OracleState, PuppetMaskMode, PuppetNodeKind, PuppetStage};

/// One renderer call, with the fields that reach GL.
#[derive(Clone, Copy, PartialEq)]
pub enum DrawEvent {
    CompositeBegin {
        node: InoxNodeUuid,
    },
    CompositeEnd {
        node: InoxNodeUuid,
        opacity: f32,
    },
    Draw {
        as_mask: bool,
        node: InoxNodeUuid,
        opacity: f32,
    },
    MaskSource {
        is_mask: bool,
        node: InoxNodeUuid,
    },
    MaskedContent,
    MasksBegin {
        threshold: f32,
    },
    MasksEnd,
}

impl DrawEvent {
    /// The event with opacity erased: order/structure identity.
    fn shape(self) -> DrawEvent {
        match self {
            DrawEvent::CompositeEnd { node, .. } => DrawEvent::CompositeEnd { node, opacity: 0.0 },
            DrawEvent::Draw { as_mask, node, .. } => DrawEvent::Draw {
                as_mask,
                node,
                opacity: 0.0,
            },
            other => other,
        }
    }

    fn opacity(self) -> Option<f32> {
        match self {
            DrawEvent::CompositeEnd { opacity, .. } | DrawEvent::Draw { opacity, .. } => {
                Some(opacity)
            }
            _ => None,
        }
    }
}

/// Records inox2d's draw dispatch. Empty mask sets are normalized
/// away (inox2d parses `"masks": []` to `Some`, which its GL path
/// treats as unmasked: stencil cleared to pass-all).
#[derive(Default)]
pub struct Recorder {
    empty_masks: RefCell<Vec<bool>>,
    pub events: RefCell<Vec<DrawEvent>>,
}

impl Recorder {
    fn push(&self, event: DrawEvent) {
        self.events.borrow_mut().push(event);
    }

    fn top_is_empty(&self) -> bool {
        *self
            .empty_masks
            .borrow()
            .last()
            .expect("mask stack underflow")
    }
}

impl InoxRenderer for Recorder {
    fn begin_composite_content(
        &self,
        _as_mask: bool,
        _components: &CompositeComponents,
        _render_ctx: &CompositeRenderCtx,
        id: InoxNodeUuid,
    ) {
        self.push(DrawEvent::CompositeBegin { node: id });
    }

    fn draw_textured_mesh_content(
        &self,
        as_mask: bool,
        components: &TexturedMeshComponents,
        _render_ctx: &TexturedMeshRenderCtx,
        id: InoxNodeUuid,
    ) {
        self.push(DrawEvent::Draw {
            as_mask,
            node: id,
            opacity: components.drawable.blending.opacity,
        });
    }

    fn finish_composite_content(
        &self,
        _as_mask: bool,
        components: &CompositeComponents,
        _render_ctx: &CompositeRenderCtx,
        id: InoxNodeUuid,
    ) {
        self.push(DrawEvent::CompositeEnd {
            node: id,
            opacity: components.drawable.blending.opacity,
        });
    }

    fn on_begin_mask(&self, mask: &Mask) {
        self.push(DrawEvent::MaskSource {
            is_mask: mask.mode == MaskMode::Mask,
            node: mask.source,
        });
    }

    fn on_begin_masked_content(&self) {
        if !self.top_is_empty() {
            self.push(DrawEvent::MaskedContent);
        }
    }

    fn on_begin_masks(&self, masks: &Masks) {
        let empty = masks.masks.is_empty();
        self.empty_masks.borrow_mut().push(empty);
        if !empty {
            self.push(DrawEvent::MasksBegin {
                threshold: masks.threshold,
            });
        }
    }

    fn on_end_mask(&self) {
        if !self.top_is_empty() {
            self.push(DrawEvent::MasksEnd);
        }
        self.empty_masks.borrow_mut().pop();
    }
}

/// Lumen's trace along inox2d's dispatch structure: root drawables
/// in lumen's order; masks before content; composite children drawn
/// as plain meshes (inox2d's `draw_composite`, render.rs:342-350, does
/// not apply composite children's own masks).
pub fn lumen_trace(
    stage: &PuppetStage,
    state: &OracleState,
    map: &[InoxNodeUuid],
) -> Vec<DrawEvent> {
    let mut out = Vec::new();
    for &ni in &state.root_draw_order {
        lumen_drawable(stage, state, map, false, ni, &mut out);
    }
    out
}

fn lumen_drawable(
    stage: &PuppetStage,
    state: &OracleState,
    map: &[InoxNodeUuid],
    as_mask: bool,
    ni: usize,
    out: &mut Vec<DrawEvent>,
) {
    let node = &stage.nodes[ni];
    let masked = !node.masks.is_empty();
    if masked {
        out.push(DrawEvent::MasksBegin {
            threshold: node.mask_threshold,
        });
        for mask in &node.masks {
            let src = stage
                .nodes
                .iter()
                .position(|n| n.id == mask.source)
                .expect("validated mask source");
            out.push(DrawEvent::MaskSource {
                is_mask: mask.mode == PuppetMaskMode::Mask,
                node: map[src],
            });
            lumen_drawable(stage, state, map, true, src, out);
        }
        out.push(DrawEvent::MaskedContent);
    }
    let draw = |ni: usize| DrawEvent::Draw {
        as_mask,
        node: map[ni],
        opacity: state.nodes[ni].opacity,
    };
    match node.kind {
        PuppetNodeKind::Composite => {
            let kids = &state.nodes[ni].composite_children;
            if !kids.is_empty() {
                out.push(DrawEvent::CompositeBegin { node: map[ni] });
                out.extend(kids.iter().map(|&k| draw(k)));
                out.push(DrawEvent::CompositeEnd {
                    node: map[ni],
                    opacity: state.nodes[ni].opacity,
                });
            }
        }
        PuppetNodeKind::Part => out.push(draw(ni)),
        _ => {}
    }
    if masked {
        out.push(DrawEvent::MasksEnd);
    }
}

/// Render one event with node names (for divergence reports).
pub fn describe(event: DrawEvent, name: &dyn Fn(InoxNodeUuid) -> String) -> String {
    match event {
        DrawEvent::CompositeBegin { node } => format!("CompositeBegin {}", name(node)),
        DrawEvent::CompositeEnd { node, opacity } => {
            format!("CompositeEnd {} opacity={opacity}", name(node))
        }
        DrawEvent::Draw {
            as_mask,
            node,
            opacity,
        } => {
            format!("Draw {} as_mask={as_mask} opacity={opacity}", name(node))
        }
        DrawEvent::MaskSource { is_mask, node } => {
            format!("MaskSource {} is_mask={is_mask}", name(node))
        }
        DrawEvent::MaskedContent => "MaskedContent".into(),
        DrawEvent::MasksBegin { threshold } => format!("MasksBegin threshold={threshold}"),
        DrawEvent::MasksEnd => "MasksEnd".into(),
    }
}

/// How two traces compare.
#[derive(Debug, PartialEq)]
pub struct TraceDiff {
    pub events_inox: usize,
    pub events_lumen: usize,
    /// First index where order/structure differs (opacity erased).
    pub first_shape_divergence: Option<usize>,
    /// Opacity-carrying events whose f32 bits differ (same shape).
    pub opacity_mismatches: usize,
    pub opacity_max_abs_delta: f32,
}

impl TraceDiff {
    /// Shape and every opacity bit-identical.
    pub fn identical(&self) -> bool {
        self.first_shape_divergence.is_none() && self.opacity_mismatches == 0
    }
}

pub fn diff(inox: &[DrawEvent], lumen: &[DrawEvent]) -> TraceDiff {
    let first_shape_divergence = inox
        .iter()
        .zip(lumen.iter())
        .position(|(a, b)| a.shape() != b.shape())
        .or((inox.len() != lumen.len()).then_some(inox.len().min(lumen.len())));
    let mut opacity_mismatches = 0;
    let mut opacity_max_abs_delta = 0.0f32;
    for (a, b) in inox.iter().zip(lumen.iter()) {
        if let (Some(x), Some(y)) = (a.opacity(), b.opacity())
            && x.to_bits() != y.to_bits()
        {
            opacity_mismatches += 1;
            opacity_max_abs_delta = opacity_max_abs_delta.max((x - y).abs());
        }
    }
    TraceDiff {
        events_inox: inox.len(),
        events_lumen: lumen.len(),
        first_shape_divergence,
        opacity_mismatches,
        opacity_max_abs_delta,
    }
}
