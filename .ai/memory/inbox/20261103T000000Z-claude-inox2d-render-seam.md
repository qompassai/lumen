# inox2d-opengl render seam: only `deforms` is externally writable

Found while doing the D-870 GL oracle seam investigation. This was read-only
work; nothing was built.

- `puppet.render_ctx.vertex_buffers.deforms` is a `pub` chain and gets
  re-uploaded every frame in `OpenglRenderer::on_begin_draw`. The other
  vertex buffers are `STATIC_DRAW` and uploaded once at init.
- Per-node `TransformStore.absolute`, `Drawable.blending` and `ZSort` live in
  `Puppet.node_comps`. That field is `pub(crate)`, and only `&World` is
  exposed. `RenderCtx.root_drawables_zsorted` is private.
- So external injection without changing inox2d can only replace vertex
  positions, via `d = A_inox^-1 * w_lumen - v_rest`. Draw order and opacity
  stay inox2d-evaluated.
- `PuppetEval` (`crates/lumen-rig-core/src/puppet_eval.rs:347`) keeps
  `absolute`, `deforms`, `zsort_abs` and `opacity_factor` private. An oracle
  needs a read-only accessor added behind a feature.

Evidence: `ref/inox2d/inox2d/src/render.rs:35-48,366`,
`ref/inox2d/inox2d/src/puppet.rs:19-32,75`,
`ref/inox2d/inox2d-opengl/src/lib.rs:501,628-648`.
Full write-up: `~/.local/share/pax/lumen-gl-oracle-20261103/ORACLE-REPORT.md`.
Validation: read those lines. Also confirm that `node_comps()` returns
`&World`.
