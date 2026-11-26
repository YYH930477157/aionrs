use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use async_trait::async_trait;
use serde_json::{Value, json};

use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::workspace::{SharedWorkspaceContext, WorkspaceContext, snapshot};

const MAX_RESULTS: usize = 100;
/// Matches collected before sorting; past this the result says the scan stopped.
const MAX_SCANNED: usize = 20_000;
/// Patterns one `{a,b}` expansion may produce.
const MAX_EXPANSIONS: usize = 64;

pub struct GlobTool {
    workspace: SharedWorkspaceContext,
}

impl GlobTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self::with_workspace(Arc::new(RwLock::new(WorkspaceContext::legacy(cwd))))
    }

    pub fn with_workspace(workspace: SharedWorkspaceContext) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        "Fast file pattern matching tool that works with any codebase size.\n\n\
         - Supports glob patterns like \"**/*.rs\", \"src/**/*.ts\" and alternatives such as \"**/*.{xlsx,csv}\".\n\
         - Returns matching file paths sorted by modification time (newest first).\n\
         - Returns the 100 newest results and says how many matched when there are more. Only returns files, not directories.\n\
         - A recursive pattern rooted above the workspace, at a drive root or at the home folder is refused.\n\
         - The path parameter defaults to the current working directory.\n\
         - Use this tool when you need to find files by name or extension patterns."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern, e.g. \"**/*.rs\""
                },
                "path": {
                    "type": "string",
                    "description": "Root directory (default: cwd)"
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
        let root_path = match workspace.resolve_path(&input, "path", true) {
            Ok(path) => path,
            Err(content) => {
                return ToolResult {
                    content,
                    is_error: true,
                };
            }
        };

        tracing::debug!(resolved_root = %root_path.display(), "GlobTool scanning");

        let absolute = Path::new(pattern).is_absolute();
        let search_root = if absolute {
            literal_prefix(Path::new(pattern))
        } else {
            root_path.clone()
        };
        if pattern.contains("**")
            && let Some(refusal) = workspace.broad_search_refusal(&search_root)
        {
            return ToolResult {
                content: refusal,
                is_error: true,
            };
        }

        // The glob crate has no `{a,b}` alternatives: expand them into separate patterns.
        let base = glob::Pattern::escape(&root_path.display().to_string());
        let mut seen = HashSet::new();
        let mut files: Vec<(SystemTime, String)> = Vec::new();
        let mut scan_stopped = false;
        'patterns: for each in expand_braces(pattern) {
            let full_pattern = if absolute { each } else { format!("{base}/{each}") };
            let entries = match glob::glob(&full_pattern) {
                Ok(paths) => paths,
                Err(e) => {
                    return ToolResult {
                        content: format!("Invalid glob pattern: {}", e),
                        is_error: true,
                    };
                }
            };
            for entry in entries {
                let Ok(path) = entry else {
                    continue;
                };
                if !path.is_file() || !seen.insert(path.clone()) {
                    continue;
                }
                if files.len() >= MAX_SCANNED {
                    scan_stopped = true;
                    break 'patterns;
                }
                let mtime = path
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                // Make path relative to root
                let display_path = path.strip_prefix(&root_path).unwrap_or(&path).display().to_string();
                files.push((mtime, display_path));
            }
        }

        if files.is_empty() {
            return ToolResult {
                content: "No files matched the pattern".to_string(),
                is_error: false,
            };
        }

        // Newest first across every match, and only then the limit, so the newest files are never cut.
        files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let total = files.len();
        files.truncate(MAX_RESULTS);
        let mut content = files.into_iter().map(|(_, path)| path).collect::<Vec<_>>().join("\n");
        if scan_stopped {
            content.push_str(&format!(
                "\n\n(Stopped after {MAX_SCANNED} matching files, so newer files may be missing. Narrow the pattern or the path.)"
            ));
        } else if total > MAX_RESULTS {
            content.push_str(&format!(
                "\n\n(Showing the {MAX_RESULTS} newest of {total} matching files. Narrow the pattern or the path to see the rest.)"
            ));
        }
        ToolResult {
            content,
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
        format!("Search for {}", pattern)
    }
}

/// The leading components of an absolute pattern that contain no wildcard.
fn literal_prefix(pattern: &Path) -> PathBuf {
    let mut prefix = PathBuf::new();
    for component in pattern.components() {
        if component.as_os_str().to_string_lossy().contains(['*', '?', '[', '{']) {
            break;
        }
        prefix.push(component);
    }
    prefix
}

/// Expands `{a,b}` alternatives (nested too) into separate patterns, at most `MAX_EXPANSIONS`.
fn expand_braces(pattern: &str) -> Vec<String> {
    let mut out = Vec::new();
    expand_into(pattern, &mut out);
    out
}

fn expand_into(pattern: &str, out: &mut Vec<String>) {
    if out.len() >= MAX_EXPANSIONS {
        return;
    }
    let Some((open, close, alternatives)) = first_brace_group(pattern) else {
        out.push(pattern.to_string());
        return;
    };
    for alternative in alternatives {
        expand_into(
            &format!("{}{alternative}{}", &pattern[..open], &pattern[close + 1..]),
            out,
        );
    }
}

/// The first balanced `{...}` with at least one top-level comma: its byte span and alternatives.
fn first_brace_group(pattern: &str) -> Option<(usize, usize, Vec<&str>)> {
    let bytes = pattern.as_bytes();
    let mut from = 0;
    while let Some(offset) = pattern[from..].find('{') {
        let open = from + offset;
        let mut depth = 0usize;
        let mut parts = Vec::new();
        let mut part_start = open + 1;
        for (index, &byte) in bytes.iter().enumerate().skip(open) {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        parts.push(&pattern[part_start..index]);
                        if parts.len() > 1 {
                            return Some((open, index, parts));
                        }
                        break;
                    }
                }
                b',' if depth == 1 => {
                    parts.push(&pattern[part_start..index]);
                    part_start = index + 1;
                }
                _ => {}
            }
        }
        from = open + 1;
    }
    None
}

#[cfg(test)]
#[path = "glob_test.rs"]
mod glob_test;
