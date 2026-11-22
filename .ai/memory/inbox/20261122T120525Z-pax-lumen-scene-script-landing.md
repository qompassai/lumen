# lumen-scene-script landed as a root workspace member

- What: `crates/lumen-scene-script` (lumen's scene schema: integral-frame
  time, piecewise-linear curves, provenance labels) is now a member of the
  root workspace, so torogao-scene can depend on it via a git rev instead of
  an absolute path into a lane worktree.
- Source: branch `pax/scene-tooling-mvp-20261112` @ c7f04df, crate tree
  9fc2c89; `src/lib.rs` is byte-identical (blob 997564f). The manifest
  follows the lumen-canon member convention (explicit version, edition and
  license; description; `[lints.rust] unsafe_code = "forbid"`). The root has
  no `[workspace.package]` to inherit from.
- The crate has no tests yet: `cargo test --workspace` gains 2 empty suites.
  On the base d2c76ec the counts stay 618 passed / 0 failed / 8 ignored.
- Not landed: `lumen-scene-runner` from the same lane workspace (out of scope).
- Validate: `cargo build --workspace && cargo test --workspace &&
  cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
- Tooling note: the theodolite bacon producer fails with "path must be shorter
  than SUN_LEN" when the workspace path is long (bacon's socket path).
  On a cold target, the driver's 120s producer window is too short; pre-warm
  with the bacon-ls clippy command first.
