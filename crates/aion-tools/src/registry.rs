use aion_types::tool::ToolDef;

use crate::Tool;
use crate::workspace::{SharedWorkspaceContext, WorkspacePathTool};

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    workspace: Option<SharedWorkspaceContext>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            workspace: None,
        }
    }

    /// Attach the same workspace handle used by this registry's built-in tools.
    pub fn with_workspace(workspace: SharedWorkspaceContext) -> Self {
        Self {
            tools: Vec::new(),
            workspace: Some(workspace),
        }
    }

    pub fn workspace_context(&self) -> Option<&SharedWorkspaceContext> {
        self.workspace.as_ref()
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Register a built-in path tool with local workspace reference resolution.
    pub fn register_path_tool(&mut self, tool: Box<dyn Tool>, parameter: &'static str) {
        let tool = match &self.workspace {
            Some(workspace) => Box::new(WorkspacePathTool::new(tool, workspace.clone(), parameter)) as Box<dyn Tool>,
            None => tool,
        };
        self.register(tool);
    }

    /// Find a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
    }

    /// Get all registered tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Generate API tool definitions for all registered tools
    pub fn to_tool_defs(&self) -> Vec<ToolDef> {
        self.tools
            .iter()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
                deferred: t.is_deferred(),
            })
            .collect()
    }

    /// Generate API tool definitions for tools matching a predicate.
    ///
    /// Used by plan mode to restrict the tool set sent to the LLM.
    pub fn to_tool_defs_filtered<F>(&self, filter: F) -> Vec<ToolDef>
    where
        F: Fn(&dyn Tool) -> bool,
    {
        self.tools
            .iter()
            .filter(|t| filter(t.as_ref()))
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
                deferred: t.is_deferred(),
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "registry_test.rs"]
mod registry_test;
