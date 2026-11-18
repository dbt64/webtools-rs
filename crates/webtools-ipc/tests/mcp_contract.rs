use serde_json::json;
use webtools_ipc::mcp::{McpCallRequest, McpToolsProjection};
#[test]
fn call_requires_internal_identity_deadline_and_object_arguments() {
    let valid = json!({"callId":"internal-call-1","toolName":"org.test.plugin.note","arguments":{"text":"hello"},"deadlineUnixMs":12345});
    assert!(serde_json::from_value::<McpCallRequest>(valid.clone())
        .unwrap()
        .validate()
        .is_ok());
    for (key, value) in [
        ("callId", json!("")),
        ("arguments", json!([])),
        ("deadlineUnixMs", json!(0)),
        ("toolName", json!("")),
    ] {
        let mut bad = valid.clone();
        bad[key] = value;
        assert!(
            serde_json::from_value::<McpCallRequest>(bad)
                .map(|value| value.validate().is_ok())
                .unwrap_or(false)
                == false
        );
    }
}
#[test]
fn catalog_preserves_schemas_and_rejects_duplicate_names_or_wrong_schema_version() {
    let valid = json!({"schemaVersion":1,"revision":1,"tools":[{"pluginId":"org.test.plugin","featureId":"note","title":"Note","description":"desc","inputSchema":{"type":"object"},"outputSchema":{"type":"object"}}]});
    let catalog: McpToolsProjection = serde_json::from_value(valid.clone()).unwrap();
    assert!(catalog.validate().is_ok());
    assert_eq!(catalog.tools[0].name(), "org.test.plugin.note");
    let mut wrong = valid.clone();
    wrong["schemaVersion"] = json!(2);
    assert!(serde_json::from_value::<McpToolsProjection>(wrong)
        .unwrap()
        .validate()
        .is_err());
    let mut dup = valid.clone();
    dup["tools"]
        .as_array_mut()
        .unwrap()
        .push(valid["tools"][0].clone());
    assert!(serde_json::from_value::<McpToolsProjection>(dup)
        .unwrap()
        .validate()
        .is_err());
}
