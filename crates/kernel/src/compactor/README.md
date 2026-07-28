# Compactor

The compactor keeps long conversations within the model context window while preserving prompt-cache reuse.

## Trigger

Compaction runs when estimated request input reaches the earliest of:

```text
context_window × threshold_ratio
110_000 tokens
context_window - safety_buffer - summary_prompt - minimum_summary_output
```

The default ratio is `0.9`. The reserve threshold is applied only when the context window can fit it; smaller windows keep the ratio/hard-cap trigger and rely on request budgeting to return an explicit insufficient-context error. Token accounting uses the same estimator as request output budgeting:

- Reuse the latest assistant message's real total usage and estimate only later messages.
- Otherwise estimate all model-facing messages and tool definitions. Internal metadata is excluded.

## Full compaction

Full compaction is the default path:

1. Send the original system message, sanitized conversation history, and the same ordered tool definitions as a normal agent request.
2. Append `summary_prompt.txt` as the final user message. This preserves the normal request prefix for prompt-cache sharing.
3. Accept only a non-empty response that finishes normally and makes no tool calls. Retain a complete `<summary>` block, reject malformed compactor XML, and accept plain text only when no compactor tags are present.
4. Replace old history with a continuation summary plus any configured recent-message suffix. Internal metadata is excluded and retained tool results always include their assistant tool-call batch.
5. Preserve the Agent's original system message separately, clear stale token-usage baselines, and persist every actual history rewrite.

The summary opens with a Conversation Environment section recording the user's primary interaction language (`User Language: ...`) and any still-relevant skills loaded during the conversation (`Loaded Skills: ...`), so the continued conversation keeps the user's language and can reload skills whose contents were compacted away. The continuation message also reminds the agent to re-read files and reload skills mentioned in the summary before relying on their contents.

The first summary attempt uses the complete history to preserve prompt-cache reuse. If the provider explicitly reports a context-window overflow, full compaction retries up to three times, removing the oldest 20% of complete user-led conversation rounds each time. This emergency trimming only affects the summary request; stored history is replaced only after a valid summary succeeds.

The summary request disables extended thinking and uses the configured `summary_max_tokens` subject to the normal remaining-context calculation and safety buffer.

## Micro-compaction

Micro-compaction replaces old tool results with a marker while retaining recent tool results. It is disabled by default because rewriting history invalidates the cacheable message prefix.

It can be enabled with `agent.compactor.micro_compact_enabled = true` in the TOML configuration. When enabled, auto-compaction tries micro-compaction first and falls back to full compaction if the result is still above the threshold.
