//! Web 交互工具的描述符预算：它们每轮都会进入模型工具表，必须保持紧凑稳定。

use agent_tools::interaction_specs;

const MAX_DESCRIPTOR_BYTES: usize = 3_500;

#[test]
fn web_interaction_descriptors_stay_within_the_prompt_budget() {
    let specs = interaction_specs();
    assert_eq!(specs.len(), 3);
    for spec in specs {
        let encoded = serde_json::to_vec(&serde_json::json!({
            "name": spec.name,
            "description": spec.description,
            "schema": spec.schema,
        }))
        .unwrap();
        assert!(
            encoded.len() <= MAX_DESCRIPTOR_BYTES,
            "{} descriptor is {} bytes, budget is {MAX_DESCRIPTOR_BYTES}",
            spec.name,
            encoded.len()
        );
    }
}
