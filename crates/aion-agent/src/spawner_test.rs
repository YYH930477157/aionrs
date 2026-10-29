use super::*;

mod cancellation_tests {
    use super::*;
    use aion_config::config::CliArgs;
    use aion_providers::error::ProviderError;
    use aion_types::llm::{LlmEvent, LlmRequest};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::{Semaphore, mpsc::Receiver};

    struct ActiveCall(Arc<AtomicUsize>);
    impl Drop for ActiveCall {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    struct WaitingProvider {
        active: Arc<AtomicUsize>,
        started: Semaphore,
        release: Semaphore,
    }
    #[async_trait]
    impl LlmProvider for WaitingProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<Receiver<LlmEvent>, ProviderError> {
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = ActiveCall(self.active.clone());
            self.started.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            Err(ProviderError::Parse("test completed".into()))
        }
    }
    fn spawner(provider: Arc<WaitingProvider>, directory: &std::path::Path) -> AgentSpawner {
        let mut config = Config::resolve(&CliArgs {
            provider: Some("anthropic".into()),
            api_key: Some("test".into()),
            base_url: None,
            model: Some("test".into()),
            max_tokens: Some(100),
            thinking: None,
            thinking_budget: None,
            max_turns: Some(1),
            max_tool_call_malformed_turns: None,
            max_tool_call_failure_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: true,
            project_dir: Some(directory.to_path_buf()),
        })
        .unwrap();
        config.hooks = Default::default();
        AgentSpawner::new(provider, config, directory.to_path_buf(), ToolPolicy::Unrestricted)
    }
    fn tasks() -> Vec<SubAgentConfig> {
        ["first", "second"]
            .into_iter()
            .map(|name| SubAgentConfig {
                name: name.into(),
                prompt: name.into(),
                max_turns: 1,
                max_tokens: 100,
                system_prompt: None,
            })
            .collect()
    }
    fn provider(permits: usize) -> Arc<WaitingProvider> {
        Arc::new(WaitingProvider {
            active: Arc::new(AtomicUsize::new(0)),
            started: Semaphore::new(0),
            release: Semaphore::new(permits),
        })
    }
    #[tokio::test]
    async fn workspace_cancel_parent_drops_all_concurrent_children() {
        let directory = tempfile::tempdir().unwrap();
        let provider = provider(0);
        let spawner = spawner(provider.clone(), directory.path());
        let parent = tokio::spawn(async move { spawner.spawn_parallel(tasks()).await });
        let started = tokio::time::timeout(Duration::from_secs(5), provider.started.acquire_many(2)).await;
        parent.abort();
        let _ = parent.await;
        let surviving = provider.active.load(Ordering::SeqCst);
        provider.release.add_permits(2);
        assert!(started.is_ok(), "both children must run concurrently");
        assert_eq!(surviving, 0, "parent cancellation detached running child tasks");
    }
    #[tokio::test]
    async fn workspace_parallel_results_keep_input_order() {
        let directory = tempfile::tempdir().unwrap();
        let spawner = spawner(provider(2), directory.path());
        let results = spawner.spawn_parallel(tasks()).await;
        assert_eq!(
            results.iter().map(|result| result.name.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }
}

#[tokio::test]
async fn workspace_child_tools_share_current_default_and_null_state() {
    use aion_tools::workspace::WorkspaceContext;
    use serde_json::json;
    use std::sync::{Arc, RwLock};
    let initial = tempfile::tempdir().unwrap();
    let workspace = Arc::new(RwLock::new(WorkspaceContext::legacy(initial.path().to_path_buf())));
    let registry = build_tool_registry(&ToolPolicy::Unrestricted, workspace.clone(), &[]);
    workspace.write().unwrap().default_cwd = None;
    let result = registry
        .get("ExecCommand")
        .unwrap()
        .execute(json!({"cmd": "echo forbidden"}))
        .await;
    assert!(
        result.is_error && result.content.contains("No default working directory"),
        "{}",
        result.content
    );
}

#[cfg(test)]
mod phase7_tests {
    use super::{ForkOverrides, SubAgentConfig, ToolPolicy, build_tool_registry, effective_child_tool_policy};
    use aion_tools::workspace::WorkspaceContext;
    use std::sync::{Arc, RwLock};

    #[test]
    fn tc_7_1_fork_overrides_default_values() {
        let o = ForkOverrides::default();
        assert!(o.model.is_none());
        assert!(o.effort.is_none());
        assert!(o.allowed_tools.is_empty());
    }

    #[test]
    fn tc_7_40_build_tool_registry_unrestricted_registers_all() {
        let workspace = Arc::new(RwLock::new(WorkspaceContext::legacy(std::env::temp_dir())));
        let registry = build_tool_registry(&ToolPolicy::Unrestricted, workspace, &[]);
        for name in &["Read", "Write", "Edit", "ExecCommand", "Grep", "Glob"] {
            assert!(registry.get(name).is_some(), "tool '{name}' should be registered");
        }
    }

    #[test]
    fn tc_7_43_build_tool_registry_filters_to_policy() {
        let policy = ToolPolicy::allow_only(["ExecCommand", "Read"]);
        let workspace = Arc::new(RwLock::new(WorkspaceContext::legacy(std::env::temp_dir())));
        let registry = build_tool_registry(&policy, workspace, &[]);
        assert!(registry.get("ExecCommand").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.get("Write").is_none());
    }

    #[test]
    fn fork_overrides_can_only_narrow_parent_policy() {
        let parent = ToolPolicy::allow_only(["Read", "Grep", "Spawn"]);
        let allowed_tools = vec!["Read".to_string(), "ExecCommand".to_string()];

        let child = effective_child_tool_policy(&parent, &allowed_tools);

        assert!(child.allows("Read"));
        assert!(!child.allows("Grep"));
        assert!(!child.allows("ExecCommand"));
        assert!(!child.allows("Write"));
    }

    #[test]
    fn empty_fork_override_inherits_parent_policy() {
        let parent = ToolPolicy::allow_only(["Read", "Grep", "Spawn"]);

        let child = effective_child_tool_policy(&parent, &[]);

        assert_eq!(child, parent);
    }

    #[test]
    fn tc_7_sub_agent_config_original_fields_intact() {
        let config = SubAgentConfig {
            name: "test-agent".to_string(),
            prompt: "do the task".to_string(),
            max_turns: 5,
            max_tokens: 1024,
            system_prompt: Some("you are helpful".to_string()),
        };
        assert_eq!(config.name, "test-agent");
        assert_eq!(config.max_turns, 5);
    }
}
