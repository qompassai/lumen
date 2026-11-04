//! Harness dispatch: drive the UNMODIFIED `inox2d-opengl` renderer's
//! own `InoxRenderer` methods (the rasterizer: shaders, blending,
//! stencil masks, composite buffers) in a caller-chosen draw order
//! with caller-chosen opacities.
//!
//! This replaces only inox2d's generic dispatcher
//! (`InoxRendererExt::draw`, inox2d/src/render.rs:301-376, ~40 lines
//! of tree walking) with a structural copy that takes its order and
//! opacity from a [`Plan`]. That closes the two fields branch 1
//! cannot inject — draw order and opacity live in inox2d-private
//! state — while every GL call still comes from vendored code.
//! Validated by replaying inox2d's own recorded plan through it and
//! requiring byte-identity with `renderer.draw()`.

use std::collections::HashMap;

use inox2d::node::InoxNodeUuid;
use inox2d::node::components::{Blending, Composite, Drawable, Mesh, TexturedMesh, TransformStore};
use inox2d::node::drawables::{CompositeComponents, TexturedMeshComponents};
use inox2d::puppet::World;
use inox2d::render::{CompositeRenderCtx, InoxRenderer, TexturedMeshRenderCtx};
use lumen_rig_core::OracleState;

use crate::trace::DrawEvent;

/// Draw order + opacities for one frame.
pub struct Plan {
    /// Composite → its children in draw order.
    pub children: HashMap<InoxNodeUuid, Vec<InoxNodeUuid>>,
    /// Drawable → opacity to hand the renderer.
    pub opacity: HashMap<InoxNodeUuid, f32>,
    /// Root drawables in draw order.
    pub roots: Vec<InoxNodeUuid>,
}

impl Plan {
    /// Lumen's order and opacities (its evaluated state).
    pub fn from_lumen(state: &OracleState, map: &[InoxNodeUuid]) -> Plan {
        let mut children = HashMap::new();
        let mut opacity = HashMap::new();
        for (ni, node) in state.nodes.iter().enumerate() {
            opacity.insert(map[ni], node.opacity);
            if !node.composite_children.is_empty() {
                children.insert(
                    map[ni],
                    node.composite_children.iter().map(|&k| map[k]).collect(),
                );
            }
        }
        Plan {
            children,
            opacity,
            roots: state.root_draw_order.iter().map(|&ni| map[ni]).collect(),
        }
    }

    /// inox2d's own order and opacities, recovered from its recorded
    /// dispatch: non-mask draws outside a composite are roots; draws
    /// between a composite's begin/end are its children. Mask-source
    /// draws (`as_mask`) are re-derived from the masks themselves.
    pub fn from_inox(events: &[DrawEvent]) -> Plan {
        let mut plan = Plan {
            children: HashMap::new(),
            opacity: HashMap::new(),
            roots: Vec::new(),
        };
        let mut open: Option<InoxNodeUuid> = None;
        for event in events {
            match *event {
                DrawEvent::CompositeBegin { node } => {
                    plan.roots.push(node);
                    plan.children.insert(node, Vec::new());
                    open = Some(node);
                }
                DrawEvent::CompositeEnd { node, opacity } => {
                    plan.opacity.insert(node, opacity);
                    open = None;
                }
                DrawEvent::Draw {
                    as_mask: false,
                    node,
                    opacity,
                } => {
                    plan.opacity.insert(node, opacity);
                    match open {
                        Some(c) => plan.children.entry(c).or_default().push(node),
                        None => plan.roots.push(node),
                    }
                }
                _ => {}
            }
        }
        plan
    }
}

/// Draw the whole puppet through `renderer` per `plan`.
pub fn draw<R: InoxRenderer>(renderer: &R, comps: &World, plan: &Plan) {
    for &id in &plan.roots {
        drawable(renderer, comps, plan, false, id);
    }
}

/// inox2d's `draw_drawable` (render.rs:302), order/opacity from `plan`.
fn drawable<R: InoxRenderer>(r: &R, comps: &World, plan: &Plan, as_mask: bool, id: InoxNodeUuid) {
    let source: &Drawable = comps.get(id).expect("plan names a drawable");
    if let Some(masks) = &source.masks {
        r.on_begin_masks(masks);
        for mask in &masks.masks {
            r.on_begin_mask(mask);
            drawable(r, comps, plan, true, mask.source);
        }
        r.on_begin_masked_content();
    }
    if comps.get::<TexturedMesh>(id).is_some() {
        mesh(r, comps, plan, as_mask, id);
    } else if let Some(data) = comps.get::<Composite>(id) {
        composite(r, comps, plan, as_mask, id, data);
    }
    if source.masks.is_some() {
        r.on_end_mask();
    }
}

/// inox2d's `draw_composite` (render.rs:333): children as plain meshes.
fn composite<R: InoxRenderer>(
    r: &R,
    comps: &World,
    plan: &Plan,
    as_mask: bool,
    id: InoxNodeUuid,
    data: &Composite,
) {
    let kids = plan.children.get(&id).map_or(&[][..], Vec::as_slice);
    if kids.is_empty() {
        return;
    }
    let shown = with_opacity(comps, plan, id);
    let parts = CompositeComponents {
        transform: &comps.get::<TransformStore>(id).expect("transform").absolute,
        drawable: &shown,
        data,
    };
    let rc: &CompositeRenderCtx = comps.get(id).expect("composite render ctx");
    r.begin_composite_content(as_mask, &parts, rc, id);
    for &kid in kids {
        mesh(r, comps, plan, as_mask, kid);
    }
    r.finish_composite_content(as_mask, &parts, rc, id);
}

fn mesh<R: InoxRenderer>(r: &R, comps: &World, plan: &Plan, as_mask: bool, id: InoxNodeUuid) {
    let shown = with_opacity(comps, plan, id);
    let parts = TexturedMeshComponents {
        transform: &comps.get::<TransformStore>(id).expect("transform").absolute,
        drawable: &shown,
        texture: comps.get::<TexturedMesh>(id).expect("textured mesh"),
        mesh: comps.get::<Mesh>(id).expect("mesh"),
    };
    let rc: &TexturedMeshRenderCtx = comps.get(id).expect("mesh render ctx");
    r.draw_textured_mesh_content(as_mask, &parts, rc, id);
}

/// The node's blending with the plan's opacity. Masks are omitted:
/// the renderer's draw methods read only `blending` (inox2d-opengl
/// lib.rs:498-520, 586-616); masks are issued by `drawable` above.
fn with_opacity(comps: &World, plan: &Plan, id: InoxNodeUuid) -> Drawable {
    let source: &Drawable = comps.get(id).expect("drawable");
    let opacity = plan
        .opacity
        .get(&id)
        .copied()
        .unwrap_or(source.blending.opacity);
    Drawable {
        blending: Blending {
            mode: source.blending.mode,
            opacity,
            screen_tint: source.blending.screen_tint,
            tint: source.blending.tint,
        },
        masks: None,
    }
}
