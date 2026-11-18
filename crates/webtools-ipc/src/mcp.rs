//! MCP bridge data only. No HTTP SDK or plugin runtime dependency.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, io};
pub const CAPABILITY: &str = "plugin-mcp-gateway-v1";
pub const MAX_CALLS: usize = 8;
pub fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpCallRequest {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub deadline_unix_ms: u64,
}
impl McpCallRequest {
    pub fn validate(&self) -> io::Result<()> {
        if self.call_id.is_empty()
            || self.call_id.len() > 128
            || self.tool_name.is_empty()
            || self.tool_name.len() > 512
            || !self.arguments.is_object()
            || self.deadline_unix_ms == 0
        {
            return Err(invalid("Invalid MCP call"));
        }
        if serde_json::to_vec(self)?.len() > 64 * 1024 + 1024 {
            return Err(invalid("MCP call too large"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolProjection {
    pub plugin_id: String,
    pub feature_id: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}
impl McpToolProjection {
    pub fn name(&self) -> String {
        format!("{}.{}", self.plugin_id, self.feature_id)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolsProjection {
    pub schema_version: u32,
    pub revision: u64,
    pub tools: Vec<McpToolProjection>,
}
impl McpToolsProjection {
    pub fn validate(&self) -> io::Result<()> {
        if self.schema_version != 1 {
            return Err(invalid("Unsupported MCP catalog version"));
        }
        let mut names = BTreeSet::new();
        for t in &self.tools {
            if t.plugin_id.is_empty()
                || t.feature_id.is_empty()
                || !t.input_schema.is_object()
                || t.output_schema.as_ref().is_some_and(|s| !s.is_object())
                || !names.insert(t.name())
            {
                return Err(invalid("Invalid MCP tool catalog"));
            }
        }
        let envelope = crate::Envelope::new("mcp-tools-update-v1", serde_json::to_value(self)?);
        if serde_json::to_vec(&envelope)?.len() > crate::MAX_MESSAGE_BYTES {
            return Err(invalid("MCP catalog exceeds pipe frame limit"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpCallReply {
    pub call_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
impl McpCallReply {
    pub fn failed(id: &str, status: &str, message: &str) -> Self {
        Self {
            call_id: id.into(),
            status: status.into(),
            result: None,
            error: Some(message.into()),
        }
    }
}
