//! BIND-1 format tests: the part `translation_keyforms`
//! amendment (SPEC.md section 3.2). The amendment is
//! additive and defaulted — pre-amendment documents parse
//! unchanged and re-emit byte-identically, documents with
//! the field round-trip exactly, and the lattice law
//! (section 3.7) governs the new payload like every other.

use lumen_rig_core::parse_model;

/// A minimal model whose `brow` part carries a translation
/// lattice over `brow_y` (-1..=1): lowered at -1, rest at 0,
/// raised at 1 (model space, +y up).
fn bind1_document() -> String {
    r#"{
      "format": "lumen-rig-model",
      "format_version": 1,
      "id": "bind1",
      "parameters": [
        { "id": "brow_y", "kind": "scalar", "min": -1.0, "max": 1.0, "default": 0.0 }
      ],
      "parts": [
        { "id": "root" },
        {
          "id": "brow",
          "parent": "root",
          "translation_keyforms": {
            "axes": ["brow_y"],
            "keys": [[-1.0, 0.0, 1.0]],
            "forms": [
              { "at": [0], "translation": [0.0, -2.0] },
              { "at": [1], "translation": [0.0, 0.0] },
              { "at": [2], "translation": [0.0, 2.0] }
            ]
          }
        }
      ],
      "textures": [{ "id": "tex0", "source": "tex0.png" }],
      "deformers": [],
      "meshes": [
        {
          "id": "quad",
          "part": "brow",
          "texture": "tex0",
          "vertices": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
          "uvs": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
          "indices": [0, 1, 2, 0, 2, 3]
        }
      ],
      "physics": []
    }"#
    .to_string()
}

#[test]
fn translation_keyforms_parse_and_round_trip() {
    let document = bind1_document();
    let model = parse_model(document.as_bytes()).expect("BIND-1 document parses");
    let brow = model
        .parts
        .iter()
        .find(|part| part.id == "brow")
        .expect("brow part");
    let lattice = brow
        .translation_keyforms
        .as_ref()
        .expect("translation lattice survives validation");
    assert_eq!(lattice.axes, vec!["brow_y".to_string()]);
    assert_eq!(lattice.forms.len(), 3);
    assert_eq!(lattice.forms[0].translation, [0.0, -2.0]);
    assert_eq!(lattice.forms[2].translation, [0.0, 2.0]);

    // Canonical emission carries the field, and the emitted
    // text re-parses to an identical model that re-emits
    // byte-identically (the round-trip law).
    let emitted = model.to_canonical_json();
    assert!(
        emitted.contains("\"translation_keyforms\""),
        "canonical emission carries the amendment field:\n{emitted}"
    );
    let reparsed = parse_model(emitted.as_bytes()).expect("emitted model re-parses");
    assert_eq!(reparsed, model);
    assert_eq!(reparsed.to_canonical_json(), emitted);
}

#[test]
fn pre_amendment_documents_carry_no_translation() {
    // The landed minimal fixture predates the amendment: it
    // parses, its parts resolve the field to absent, no
    // translation_keyforms key appears in its canonical
    // emission, and the emission re-parses to an identical
    // model with byte-stable re-emission (the round-trip
    // law — the amendment must not perturb it).
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("minimal.model.json");
    let bytes = std::fs::read(&path).expect("minimal fixture readable");
    let model = parse_model(&bytes).expect("minimal fixture parses");
    assert!(
        model
            .parts
            .iter()
            .all(|part| part.translation_keyforms.is_none())
    );
    let emitted = model.to_canonical_json();
    assert!(!emitted.contains("translation_keyforms"));
    let reparsed = parse_model(emitted.as_bytes()).expect("emission re-parses");
    assert_eq!(reparsed, model);
    assert_eq!(reparsed.to_canonical_json(), emitted);
}

#[test]
fn translation_lattice_law_is_enforced() {
    // Incomplete lattice (a missing form) is LRF-404 class.
    let incomplete = bind1_document().replace("{ \"at\": [1], \"translation\": [0.0, 0.0] },", "");
    let error = parse_model(incomplete.as_bytes()).expect_err("incomplete lattice refused");
    assert_eq!(error.code(), "LRF-404", "{error}");

    // A non-finite translation component is refused.
    let non_finite = bind1_document().replace("[0.0, -2.0]", "[0.0, -1e999]");
    let error = parse_model(non_finite.as_bytes()).expect_err("non-finite translation refused");
    assert_eq!(error.code(), "LRF-301", "{error}");

    // An axis naming no parameter is a dangling reference.
    let dangling = bind1_document().replace("\"brow_y\"]", "\"nope\"]");
    let error = parse_model(dangling.as_bytes()).expect_err("dangling axis refused");
    assert_eq!(error.code(), "LRF-203", "{error}");

    // An unknown field on a part is still LRF-104 — the
    // amendment permits exactly one new field, not a class.
    let unknown = bind1_document().replace(
        "\"id\": \"brow\",",
        "\"id\": \"brow\", \"slide_keyforms\": null,",
    );
    let error = parse_model(unknown.as_bytes()).expect_err("unknown part field refused");
    assert_eq!(error.code(), "LRF-104", "{error}");
}
