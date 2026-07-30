/**
 * Playwright e2e harness, mounted by the /e2e route (never navigated to
 * by the app itself). Exposes the app's own module instances on
 * `window.__e2e`.
 *
 * Specs must NEVER dynamic-import /src URLs inside page.evaluate: a bare
 * `import("/src/lib/x.ts")` in the browser bypasses vite's import
 * analysis, so URL mismatches (`/@id/svelte` vs the optimized-dep URL,
 * extensionless vs `.ts`) instantiate a SECOND copy of the module — a
 * duplicate state store or svelte runtime silently breaks reactivity.
 * Importing through this harness keeps every spec on the app's own
 * module graph.
 */
import * as svelte from "svelte";
import * as state from "../state.svelte";
import * as sessionLib from "../session";
import * as phaseLib from "../session-phase";
import * as settings from "../settings.svelte";
import * as events from "../events";
import * as updateCheck from "../update-check.svelte";
import * as mermaid from "../mermaid";
import * as MessageList from "../components/chat/MessageList.svelte";
import * as StatusBar from "../components/layout/StatusBar.svelte";
import * as MermaidBlock from "../components/chat/MermaidBlock.svelte";
import * as CodeBlock from "../components/chat/CodeBlock.svelte";

export const api = {
  svelte,
  state,
  sessionLib,
  phaseLib,
  settings,
  events,
  updateCheck,
  mermaid,
  MessageList,
  StatusBar,
  MermaidBlock,
  CodeBlock,
};

(window as unknown as { __e2e: typeof api }).__e2e = api;
