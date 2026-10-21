# demo_toon_fire landed — posterize Some(4) (Owner's ruling, 2026-10-18)

The study spec left demo_toon_fire's posterize value open; it failed
closed as an unknown style everywhere. Owner ruled Some(4) (cel-fire)
on 2026-10-18 after a None/4/6 comparison visual.

- `src/style.rs`: preset added as the third demo_toon peer (hue +4.0,
  sat x1.15, light x1.0, contrast 1.06, posterize Some(4)); preset
  table 15 -> 16; "listing all 15" doc comments updated.
- Fail-closed test pins moved off the fire name in three files:
  `tests/stdio_style.rs` (count 15 -> 16, fire in expected names,
  unknown-style probe now `no_such_style`),
  `tests/style_dream.rs` (exact-list 15 -> 16 with fire inserted;
  `style_apply_demo_toon_fire_is_not_a_style` converted to the positive
  `style_apply_demo_toon_fire_posterizes_gray_to_level` — gray 128 ->
  170 by the shared lightness math; error-listing expectation gains
  fire), `tests/vrm.rs` (unknown-style probe now `no_such_style`).
- Gates: workspace tests 406/0, fmt clean, clippy -D warnings clean.
  Local commit only; not pushed.

Reconciliation flag: docs/factory/FACTORY-PLAN.md assumption A1 records
"P6 verdict: HELD exactly — demo_toon_fire does not exist; the landed
surface fails closed on it". That verdict is now stale; per the plan's
own terms it reconciles in the final integration-update phase.
