/// Token usage of one completed LLM call, with the context occupancy it leaves behind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallUsage {
    /// Provider input of this call, including cache-read and cache-creation tokens.
    pub input_tokens: u64,
    /// Provider output of this call.
    pub output_tokens: u64,
    /// Subset of `input_tokens` used to create a provider-side prompt cache.
    pub cache_creation_tokens: u64,
    /// Subset of `input_tokens` read from a provider-side prompt cache.
    pub cache_read_tokens: u64,
    /// Best-known context occupancy after this call.
    pub context_usage: u64,
    /// Whether `context_usage` is the provider's exact report rather than a local estimate.
    pub context_provider_reported: bool,
    /// Configured context window of the model.
    pub context_window: u64,
    /// Wall time from sending the request until the response stream ended.
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

    /// Report the usage of one completed LLM call. A turn with tool use makes several
    /// calls, each reported here; `emit_stream_end` still carries the turn totals.
    /// Optional: the default ignores it.
    fn emit_usage(&self, _usage: &CallUsage) {}
}
