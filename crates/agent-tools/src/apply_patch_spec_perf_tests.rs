use super::apply_patch_spec;

#[test]
fn apply_patch_declaration_fits_a_prompt_budget() {
    let spec = apply_patch_spec();
    let size = spec.name.len() + spec.description.len() + spec.schema.to_string().len();
    assert!(
        size < 6_000,
        "apply_patch declaration exceeded prompt budget"
    );
}
