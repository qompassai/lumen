# GL same-route oracle (D-870): durable findings

Origin: claude worker, branch `pax/lumen-gl-oracle`, 2026-11-04.
Evidence root: `~/.local/share/pax/lumen-gl-oracle-20261103/evidence/`
(outside the repo; report `ORACLE-REPORT.md` beside it).

## Facts (each with its evidence)

1. **Corpus pose table is not recorded in the repo.** Recovered by exact
   pixel match of lumen-CPU renders against the proving build's frames
   (`evidence/pose-table-recovery*.txt`, `scripts/pose_search.py`):
   rest/aka-rest = 30 frames, no sets; head-max = `Head:: Yaw-Pitch=1,1`;
   head-min = `=-1,-1`; blink = `Eye:: Left:: Blink=1,0` +
   `Eye:: Right:: Blink=1,0`; phys-early = 3 frames and phys-settled =
   120 frames, both `Head:: Yaw-Pitch=1,0`; proof-opacity-faded =
   Aka-opacity, 30 frames, `Proof:: Opacity=1,0`. All at 1/60 s.
2. **The staged vendored inox2d keeps the (0,0)-skip**
   (`inox2d/src/params.rs:320`). Unmodified, it evaluates SKIP
   semantics; `frames/fixed` (apply-all) came from a skip-removed
   build. Matched comparisons use lumen's `set_zero_skip_for_proof`.
3. **zsort tie-break differs.** For equal zsort, inox2d keeps arena
   order (`render.rs:162`, `nodes.iter()`); lumen keeps pre-order
   (`puppet_eval.rs` `root_draw_list`). Midori swaps
   `Eye Right:: Shadow:: Upper` / `Nose` on every corpus frame.
4. **Composite children's masks:** inox2d GL draws composite
   children without their masks (`render.rs:342-350`); lumen's CPU
   route applies them. 6 masked composite children per puppet
   (`*_Hair_*_Shadow`).
5. **Xvfb GL is Mesa llvmpipe**: no GPU device is involved under
   xvfb-run on primo (GL_RENDERER in every render report).
6. **rustup picks the toolchain from the cwd, not --manifest-path.**
   Use `env RUSTUP_TOOLCHAIN=1.99.0 cargo ... --manifest-path` for
   rig-core when the cwd is elsewhere.
7. Branch-1 seam (inox2d unmodified): only `vertex_buffers.deforms`
   is writable, but `TransformStore` (`pub absolute`) is readable via
   `Puppet::node_comps().get`, and `InoxRenderer` + all-pub
   `TexturedMeshComponents`/`CompositeComponents` let a harness drive
   the unmodified GL renderer in any order/opacity (validated
   byte-identical vs `renderer.draw()`, `evidence/cmp-dispatch-validation.txt`).

## Validation commands

See `ORACLE-REPORT.md` "Reproduce" section.
