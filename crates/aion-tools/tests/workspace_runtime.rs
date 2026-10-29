use std::sync::{Arc, RwLock};

use aion_tools::Tool;
use aion_tools::exec_command::ExecCommandTool;
use aion_tools::glob::GlobTool;
use aion_tools::grep::GrepTool;
use aion_tools::workspace::{WorkspaceContext, WorkspaceRoot};
use serde_json::json;

#[tokio::test]
async fn workspace_tools_follow_default_changes_and_require_explicit_directory_without_default() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    std::fs::write(first.path().join("first.txt"), "first-marker").unwrap();
    std::fs::write(second.path().join("second.txt"), "second-marker").unwrap();
    let workspace = Arc::new(RwLock::new(WorkspaceContext {
        default_cwd: Some(first.path().to_path_buf()),
        roots: vec![WorkspaceRoot {
            id: "second".into(),
            name: "第二项目".into(),
            path: second.path().to_path_buf(),
            available: true,
        }],
    }));
    let exec = ExecCommandTool::with_workspace(workspace.clone(), vec![]);
    let glob = GlobTool::with_workspace(workspace.clone());
    let grep = GrepTool::with_workspace(workspace.clone());
    let result = glob.execute(json!({"pattern": "*.txt"})).await;
    assert!(result.content.contains("first.txt"), "{}", result.content);
    workspace.write().unwrap().default_cwd = Some(second.path().to_path_buf());
    let command = if cfg!(windows) {
        "Get-Content second.txt"
    } else {
        "cat second.txt"
    };
    let result = exec.execute(json!({"cmd": command})).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains("second-marker"), "{}", result.content);
    assert!(result.content.contains("第二项目"), "{}", result.content);
    let result = glob.execute(json!({"pattern": "*.txt"})).await;
    assert!(result.content.contains("second.txt") && !result.content.contains("first.txt"));
    let result = grep.execute(json!({"pattern": "second-marker"})).await;
    assert!(
        !result.is_error && result.content.contains("second-marker"),
        "{}",
        result.content
    );
    workspace.write().unwrap().default_cwd = None;
    for result in [
        exec.execute(json!({"cmd": "echo forbidden"})).await,
        exec.execute(json!({"cmd": "echo forbidden", "workdir": "."})).await,
        glob.execute(json!({"pattern": "*.txt"})).await,
        grep.execute(json!({"pattern": "marker"})).await,
    ] {
        assert!(
            result.is_error && result.content.contains("No default working directory"),
            "{}",
            result.content
        );
    }
    let result = exec.execute(json!({"cmd": command, "workdir": second.path()})).await;
    assert!(
        !result.is_error && result.content.contains("second-marker"),
        "{}",
        result.content
    );
    // Removing a root changes metadata only, not permission to execute there.
    workspace.write().unwrap().roots.clear();
    let result = exec.execute(json!({"cmd": command, "workdir": second.path()})).await;
    assert!(!result.is_error, "{}", result.content);
}

#[tokio::test]
async fn workspace_deleted_default_does_not_fall_back_to_process_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let workspace = Arc::new(RwLock::new(WorkspaceContext {
        default_cwd: Some(path),
        roots: vec![],
    }));
    dir.close().unwrap();
    let tool = ExecCommandTool::with_workspace(workspace, vec![]);
    let result = tool.execute(json!({"cmd": "echo forbidden"})).await;
    assert!(
        result.is_error && !result.content.contains("forbidden"),
        "{}",
        result.content
    );
}
