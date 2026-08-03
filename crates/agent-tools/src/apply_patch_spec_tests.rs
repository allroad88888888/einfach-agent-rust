use super::apply_patch_spec;
use serde_json::json;

#[test]
fn apply_patch_schema_is_closed_and_requires_safe_preconditions() {
    let spec = apply_patch_spec();
    assert_eq!(spec.name.as_ref(), "apply_patch");
    assert_eq!(spec.schema["additionalProperties"], false);
    assert_eq!(spec.schema["properties"]["operations"]["maxItems"], 16);
    let variants = spec.schema["properties"]["operations"]["items"]["oneOf"]
        .as_array()
        .unwrap();
    assert_eq!(variants.len(), 4);
    for variant in variants {
        assert_eq!(variant["additionalProperties"], false);
    }
    assert_eq!(
        variants[1]["oneOf"],
        json!([
            { "required": ["oldContent"] },
            { "required": ["expectedContentHash"] }
        ])
    );
}
