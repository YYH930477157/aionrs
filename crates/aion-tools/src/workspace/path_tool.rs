use super::{SharedWorkspaceContext, snapshot};
use crate::{Tool, ToolExecutionOutput};
use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};
use async_trait::async_trait;
use serde_json::Value;

/// Only wraps built-in path tools; command bodies and file contents are never rewritten.
pub(crate) struct WorkspacePathTool {
    inner: Box<dyn Tool>,
    workspace: SharedWorkspaceContext,
    parameter: &'static str,
    description: String,
}
impl WorkspacePathTool {
    pub(crate) fn new(inner: Box<dyn Tool>, workspace: SharedWorkspaceContext, parameter: &'static str) -> Self {
        let description = format!(
            "{}\nWorkspace references: {parameter} also accepts @name/path or @{{root-id}}/path, resolved locally before execution. Use Workspace to discover current roots if their paths are unknown. Never guess an ambiguous root. This syntax applies only to {parameter}, not command text or file contents.",
            inner.description()
        );
        Self {
            inner,
            workspace,
            parameter,
            description,
        }
    }
}
#[async_trait]
impl Tool for WorkspacePathTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonSchema {
        let mut schema = self.inner.input_schema();
        let field = &mut schema["properties"][self.parameter]["description"];
        *field = Value::String(format!(
            "{} Also accepts @name/path or @{{root-id}}/path. Use Workspace to discover or disambiguate roots.",
            field.as_str().unwrap_or_default()
        ));
        schema
    }
    fn prepare_input(&self, mut input: Value) -> Result<Value, String> {
        if let Some(reference) = input
            .get(self.parameter)
            .and_then(Value::as_str)
            .filter(|s| s.starts_with('@'))
        {
            let workspace = snapshot(&self.workspace)?;
            if let Some(path) = workspace.resolve_reference(reference)? {
                input[self.parameter] = Value::String(path.to_string_lossy().into_owned());
            }
        }
        Ok(input)
    }
    fn is_concurrency_safe(&self, input: &Value) -> bool {
        self.inner.is_concurrency_safe(input)
    }
    fn category(&self) -> ToolCategory {
        self.inner.category()
    }
    fn max_result_size(&self) -> usize {
        self.inner.max_result_size()
    }
    fn requires_image_input(&self) -> bool {
        self.inner.requires_image_input()
    }
    fn is_deferred(&self) -> bool {
        self.inner.is_deferred()
    }
    fn describe(&self, input: &Value) -> String {
        match self.prepare_input(input.clone()) {
            Ok(input) => self.inner.describe(&input),
            Err(error) => error,
        }
    }
    async fn execute(&self, input: Value) -> ToolResult {
        self.execute_with_follow_up(input).await.result
    }
    async fn execute_with_follow_up(&self, input: Value) -> ToolExecutionOutput {
        match self.prepare_input(input) {
            Ok(input) => self.inner.execute_with_follow_up(input).await,
            Err(content) => ToolResult {
                content,
                is_error: true,
            }
            .into(),
        }
    }
}
