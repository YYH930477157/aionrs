use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::task;

use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::grep_search::{MAX_FILE_BYTES, MAX_RESULT_LINES, SearchOutcome, SearchRequest, search};
use crate::workspace::{SharedWorkspaceContext, WorkspaceContext, snapshot};

pub struct GrepTool {
    workspace: SharedWorkspaceContext,
}

impl GrepTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self::with_workspace(Arc::new(RwLock::new(WorkspaceContext::legacy(cwd))))
    }

    pub fn with_workspace(workspace: SharedWorkspaceContext) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> &str {
        "Searches file contents using regex patterns.\n\n\
         IMPORTANT: ALWAYS use this Grep tool for content search. \
         NEVER run grep, rg or findstr as a ExecCommand command.\n\n\
         - Uses Rust regex syntax (e.g., \"log.*Error\", \"fn\\\\s+\\\\w+\", \"foo|bar\"); \
         look-around and backreferences are not supported.\n\
         - Searches folders recursively, honours .gitignore, and skips hidden and binary files.\n\
         - Use the glob parameter to filter by file pattern (e.g., \"*.rs\" or \"*.{c,h}\").\n\
         - Output is truncated to 250 lines.\n\
         - Set case_insensitive to true for case-insensitive search."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (default: cwd)"
                },
                "glob": {
                    "type": "string",
                    "description": "File filter pattern, e.g. \"*.rs\""
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                }
            },
            "required": ["pattern"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(pattern) = input["pattern"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: pattern".to_string(),
                is_error: true,
            };
        };

        let workspace = match snapshot(&self.workspace) {
            Ok(workspace) => workspace,
            Err(content) => {
                return ToolResult {
                    content,
                    is_error: true,
                };
            }
        };
        let root = match workspace.resolve_path(&input, "path", false) {
            Ok(path) => path,
            Err(content) => {
                return ToolResult {
                    content,
                    is_error: true,
                };
            }
        };

        tracing::debug!(resolved_path = %root.display(), "GrepTool searching");

        let pattern = pattern.to_owned();
        let glob = input["glob"].as_str().map(str::to_owned);
        let case_insensitive = input["case_insensitive"].as_bool().unwrap_or(false);
        // Walking and matching are blocking file and CPU work.
        let searched = task::spawn_blocking(move || {
            search(&SearchRequest {
                pattern: &pattern,
                root: &root,
                glob: glob.as_deref(),
                case_insensitive,
            })
        })
        .await;

        match searched {
            Ok(Ok(outcome)) => render(outcome),
            Ok(Err(content)) => ToolResult {
                content,
                is_error: true,
            },
            Err(error) => ToolResult {
                content: format!("Grep failed: {error}"),
                is_error: true,
            },
        }
    }

    fn max_result_size(&self) -> usize {
        20_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let raw_path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        format!("Grep '{}' in {}", pattern, raw_path)
    }
}

/// Format matches as `path:line:text` rows, and say why a search came back
/// empty when files were skipped, so "no matches" is never a silent guess.
fn render(outcome: SearchOutcome) -> ToolResult {
    let mut notes = Vec::new();
    if outcome.unreadable > 0 {
        notes.push(format!("{} files or folders could not be read", outcome.unreadable));
    }
    if outcome.lines.is_empty() {
        if outcome.skipped_binary > 0 {
            notes.push(format!("{} binary files were skipped", outcome.skipped_binary));
        }
        if outcome.skipped_large > 0 {
            notes.push(format!(
                "{} files over {} MiB were skipped",
                outcome.skipped_large,
                MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
    }

    let mut content = if outcome.lines.is_empty() {
        "No matches found".to_string()
    } else {
        outcome.lines.join("\n")
    };
    if outcome.truncated {
        content.push_str(&format!(
            "\n[Output truncated to the first {MAX_RESULT_LINES} matching lines]"
        ));
    }
    if !notes.is_empty() {
        content.push_str(&format!("\n[{}]", notes.join("; ")));
    }
    ToolResult {
        content,
        is_error: false,
    }
}

#[cfg(test)]
#[path = "grep_test.rs"]
mod grep_test;
