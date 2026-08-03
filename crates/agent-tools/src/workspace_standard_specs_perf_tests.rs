use super::standard_workspace_file_specs;

#[test]
fn standard_workspace_declarations_stay_inside_a_prompt_budget() {
    let specs = standard_workspace_file_specs();
    let total: usize = specs
        .iter()
        .map(|spec| spec.name.len() + spec.description.len() + spec.schema.to_string().len())
        .sum();
    assert!(
        total < 12_000,
        "mutable file declarations exceeded prompt budget"
    );
}
