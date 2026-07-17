# Yomi Compactor / LLM Budget Handoff

## Review goal

Review the current implementation of Claude Code-inspired context compaction, prompt-cache sharing, tool-token estimation, and dynamic output-token budgeting in:

```text
/home/hrli/repos/yomi
```

This work is intentionally focused on preserving the normal request prefix for cache sharing and preventing `max_tokens` from exceeding the remaining context window.

## Current design decisions

### 1. Compactor cache sharing

The compactor summary request should reuse the normal request prefix:

```text
original system message
+ original conversation history
+ final compact-summary user message
```

It should also receive the same `ToolDefinition` list as the main Agent request. The provider layer is not responsible for cache sharing; the caller supplies the exact messages/tools/config.

Relevant code:

```text
crates/kernel/src/compactor/mod.rs
crates/kernel/src/agent/agent.rs
```

Important behavior:

- Do not replace the original system prompt with a compactor-specific system prompt.
- Summary instructions are appended as the final user message.
- `update_goal` is included in the main Agent tools and therefore also in compactor tools.
- Compactor tools are passed through from `ToolRegistry::definitions()`.

### 2. Micro compact is disabled by default

Client-side micro-compaction rewrites old tool results and can invalidate the message prefix used by prompt caching. It remains implemented but defaults to disabled through:

```toml
[agent.compactor]
micro_compact_enabled = false
```

Auto compact should use the unmodified history by default. If the configuration switch is explicitly enabled, the old micro-first behavior remains available.

### 3. Summary prompt and output processing

Prompt file:

```text
crates/kernel/src/compactor/summary_prompt.txt
```

Current prompt requirements:

- Text only.
- Never call/request/attempt to invoke tools.
- Historical tool activity may be described in ordinary prose.
- Use a concise `<analysis>` block and a durable `<summary>` block.
- Only preserve user messages that materially affect requirements, decisions, corrections, constraints, feedback, or pending work.
- Later user corrections override earlier decisions.
- Use nine summary sections:
  1. Primary Request and Intent
  2. Current Outcome and Decisions
  3. Key Technical Concepts and Constraints
  4. Files and Code Sections
  5. Errors and Fixes
  6. Important User Messages
  7. Pending Tasks
  8. Current Work
  9. Optional Next Step

`parse_summary_xml()` removes `<analysis>`, extracts `<summary>`, prefixes XML summaries with `Summary:`, accepts plain-text fallback, and rejects empty summaries.

After parsing, `build_continuation_summary()` wraps the result with continuation instructions so the next model turn resumes the unfinished task without acknowledging or recapping compaction.

### 4. Dynamic max-token calculation belongs at call sites

This is an important recent correction. Providers must not calculate or mutate output budgets. Callers resolve a request-specific `ModelConfig` before calling `Provider::stream()`:

```rust
resolve_request_config(messages, tools, config)
```

Current call sites:

- Main Agent request: `crates/kernel/src/agent/agent.rs`
- Compactor summary request: `crates/kernel/src/compactor/mod.rs`
- Session title request: `crates/kernel/src/kernel/tasks/session_title/mod.rs`

Providers should only serialize the already-resolved config:

```text
Anthropic: config.max_tokens
OpenAI Chat: config.max_tokens
OpenAI Responses: config.max_output_tokens
```

Do not reintroduce context estimation into provider implementations.

### 5. Output budget formula

Constants:

```rust
DEFAULT_MAX_OUTPUT_TOKENS = 8192
CONTEXT_SAFETY_BUFFER_TOKENS = 4096
```

Current formula:

```text
input_tokens = estimated/full request input
available_output = context_window - input_tokens - 4096
resolved_max_tokens = min(config.max_tokens or 8192, available_output)
```

If no output space remains, `resolve_request_config()` returns a configuration error instead of sending a request.

### 6. Tools token estimation

Tool definitions are estimated once while `ToolRegistry::definitions()` builds its cached `Arc<ToolDefinition>` values. `ToolDefinition` now has a runtime-only field:

```rust
#[serde(skip, default)]
pub estimated_tokens: u32
```

The estimate reuses `crates/kernel/src/utils/tokens.rs` conceptually/partially:

```text
normal text ≈ 4 bytes/token
JSON schema ≈ 2 bytes/token
```

The cached estimate includes name, description, serialized parameter schema, and a small per-tool overhead. Subsequent requests reuse the cached value instead of serializing schemas again. Registry mutation invalidates/rebuilds definitions.

## Compaction threshold policy

The requested policy is:

```text
ratio_threshold = context_window * user_configured_threshold_ratio
hard_cap = 110_000
actual_threshold = min(ratio_threshold, hard_cap)
```

Constant:

```rust
DEFAULT_THRESHOLD_TOKENS = 110_000
```

Thus either the user ratio threshold or the hard-coded cap can trigger compaction; whichever is reached first wins. The ratio remains respected.

### Tools and threshold accounting

Tools should not be subtracted directly from the threshold. Claude Code uses a threshold policy and compares it against current context usage; API usage already includes tools. Therefore:

- If the latest assistant message has real API usage, use `usage.total_tokens` and estimate only messages after it. Do **not** add tools again.
- If there is no assistant usage, estimate all messages plus cached tools token estimates.

`Compactor::calculate_tokens()` and `Compactor::should_compact()` now accept `tools` and follow this rule. The Agent passes its current tool definitions into `should_compact()`.

## Key files changed

```text
crates/kernel/src/agent/agent.rs
crates/kernel/src/compactor/mod.rs
crates/kernel/src/compactor/summary_prompt.txt
crates/kernel/src/compactor/tests.rs
crates/kernel/src/kernel/tasks/session_title/mod.rs
crates/kernel/src/provider/mod.rs
crates/kernel/src/provider/anthropic.rs
crates/kernel/src/provider/anthropic_test.rs
crates/kernel/src/provider/openai.rs
crates/kernel/src/provider/openai_response.rs
crates/kernel/src/provider/openai_response_test.rs
crates/kernel/src/tools/mod.rs
crates/kernel/src/types/mod.rs
```

## Review priorities / likely risks

1. **Compile correctness of the recent API changes**
   - `Compactor::auto_compact()` and `Compactor::should_compact()` now take `tools`.
   - Check every call site, especially tests and any code outside `crates/kernel/src`.

2. **Provider purity**
   - Verify Anthropic/OpenAI providers do not independently cap or infer `max_tokens` in normal call paths.
   - Direct low-level provider tests may intentionally pass `None`; decide whether those should be updated to call `resolve_request_config()` or retain compatibility behavior.

3. **Usage semantics**
   - `MessageTokenUsage::total_tokens` currently includes prompt and completion usage.
   - Verify that the stored usage is attached to an assistant message representing the complete API response.
   - Parallel tool-call/split assistant messages may require anchoring at the first sibling response, similar to Claude Code, to avoid undercounting interleaved tool results.

4. **Tool estimate duplication**
   - Real API usage includes tools; do not add cached tool tokens after a real assistant usage baseline.
   - No-usage estimation must include tools.
   - Check that the cached `estimated_tokens` field is initialized for every `ToolDefinition` constructor and is not accidentally serialized.

5. **Threshold vs max-token safety**
   - Threshold uses `min(context_window * ratio, 110_000)`.
   - Request budget uses remaining context minus 4096.
   - These are intentionally separate buffers: 110K is the compact trigger cap; 4096 is request-level estimation headroom.

6. **Cache sharing and system prompt preservation**
   - Confirm summary API messages retain the original system message.
   - Confirm summary prompt is appended, not inserted as a replacement system message.
   - Confirm main Agent and compactor use the same stable tool definitions and order.

7. **Summary XML robustness**
   - `parse_summary_xml()` is intentionally tolerant rather than a strict XML parser.
   - Verify malformed/partial model output does not silently erase a useful summary.

## Validation commands

Run from `/home/hrli/repos/yomi`:

```bash
cargo fmt --all
cargo fmt --all -- --check
git diff --check
cargo check -p kernel
cargo test -p kernel compactor
```

Formatting and diff checks have passed during implementation. Full Cargo compilation/tests were previously blocked by the environment lacking `pkg-config` / OpenSSL development files (`openssl-sys`), not by a reported test assertion failure.

## Suggested reviewer deliverables

Please report:

1. Compile errors or missed call sites.
2. Whether the usage baseline logic is semantically correct for Yomi's message model.
3. Whether tools are counted exactly once in both max-token and compact-threshold paths.
4. Whether provider-layer purity is respected.
5. Whether compactor summary requests preserve the cacheable system/history/tools prefix.
6. Any simpler or safer design for the 4096 safety buffer and 110K threshold cap.
