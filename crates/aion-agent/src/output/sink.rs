/// Per-LLM-call usage snapshot, emitted once per provider stream completion.
///
/// Distinct from the cumulative totals in `AgentResult::usage`: every field
/// describes the single call that just finished, so hosts can render
/// tokens/sec, cache-hit rate and honest context occupancy live.
#[derive(Debug, Clone, Default)]
pub struct UsageReport {
    /// This call's provider-reported input tokens (prompt size).
    pub input_tokens: u64,
    /// This call's provider-reported output tokens.
    pub output_tokens: u64,
    /// This call's tokens newly committed to a provider-side prompt cache.
    pub cache_creation_tokens: u64,
    /// This call's tokens read from an existing prompt cache.
    pub cache_read_tokens: u64,
    /// Engine's best-known context occupancy after folding this call.
    pub context_usage: u64,
    /// Whether `context_usage` is provider-reported or a local projection.
    pub context_provider_reported: bool,
    /// Configured context window (compact config), as the occupancy denominator.
    pub context_window: u64,
    /// Wall-clock duration of this provider call's stream, in milliseconds.
    pub duration_ms: u64,
}

/// Abstraction over output channels (terminal vs JSON stream protocol)
pub trait OutputSink: Send + Sync {
    /// Stream text delta from LLM
    fn emit_text_delta(&self, text: &str, msg_id: &str);

    /// Stream thinking content from LLM
    fn emit_thinking(&self, text: &str, msg_id: &str);

    /// Announce a tool call.
    fn emit_tool_call(&self, tool_use_id: &str, name: &str, input: &str);

    /// Display tool result.
    fn emit_tool_result(&self, tool_use_id: &str, name: &str, is_error: bool, content: &str);

    /// Signal start of a new message stream
    fn emit_stream_start(&self, msg_id: &str);

    /// Signal end of a message stream with usage stats
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
    );

    /// Display error
    fn emit_error(&self, msg: &str);

    /// Display informational message
    fn emit_info(&self, msg: &str);

    /// Report per-call token usage. Default no-op: sinks that don't care
    /// (terminal UI, test doubles, spawned sub-agents) stay unchanged.
    fn emit_usage(&self, _report: &UsageReport) {}
}
