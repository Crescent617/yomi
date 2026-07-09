<script lang="ts">
  import { Send } from "lucide-svelte";
  import { getActiveSession, showNotification } from "../../state.svelte";
  import * as api from "../../api";

  const activeSession = $derived(getActiveSession());
  const askUser = $derived(activeSession?.pending_ask_users[0]);

  // Map question header -> selected option labels
  let selections = $state<Record<string, string[]>>({});
  // Map question header -> custom text input
  let customInputs = $state<Record<string, string>>({});

  function toggleOption(header: string, label: string, multi: boolean) {
    const current = selections[header] ?? [];
    if (multi) {
      if (current.includes(label)) {
        selections[header] = current.filter((l) => l !== label);
      } else {
        selections[header] = [...current, label];
      }
    } else {
      selections[header] = [label];
    }
  }

  async function submit() {
    if (!activeSession || !askUser) return;
    const sessionId = askUser.session_id || activeSession.id;
    const answers: [string, string][] = [];
    for (const q of askUser.questions) {
      const selected = selections[q.header] ?? [];
      const custom = (customInputs[q.header] ?? "").trim();
      let answer = selected.join(", ");
      if (custom) {
        answer = answer ? answer + "\n" + custom : custom;
      }
      answers.push([q.header, answer || "(skipped)"]);
    }
    try {
      await api.respondAskUser(sessionId, askUser.req_id, answers);
      activeSession.pending_ask_users.shift();
      selections = {};
      customInputs = {};
    } catch (e: unknown) {
      showNotification(
        "Response failed: " + api.errorMessage(e),
        "error",
        3000,
      );
    }
  }

  async function dismiss() {
    if (!activeSession || !askUser) return;
    const sessionId = askUser.session_id || activeSession.id;
    // Send empty response to unblock the agent
    try {
      await api.respondAskUser(sessionId, askUser.req_id, []);
    } catch {
      /* ignore */
    }
    activeSession.pending_ask_users.shift();
    selections = {};
    customInputs = {};
  }
</script>

{#if askUser && askUser.questions.length > 0}
  <div
    class="shrink-0 border-t border-border bg-blue-50/40 dark:bg-blue-950/20 px-4 py-3"
  >
    <div
      class="rounded-lg border border-blue-200 dark:border-blue-800 bg-background px-3 py-2.5 space-y-3"
    >
      {#each askUser.questions as question (question.header)}
        <div>
          <div
            class="text-xs font-medium text-blue-700 dark:text-blue-400 mb-1.5"
          >
            {question.question}
          </div>
          {#if question.options.length > 0}
            <div class="flex flex-wrap gap-1.5 mb-2">
              {#each question.options as opt (opt.label)}
                {@const selected = (selections[question.header] ?? []).includes(
                  opt.label,
                )}
                <button
                  type="button"
                  onclick={() =>
                    toggleOption(
                      question.header,
                      opt.label,
                      question.multi_select,
                    )}
                  class="px-2.5 py-1 rounded-md border text-xs transition-all {selected
                    ? 'bg-blue-600 text-white border-blue-600'
                    : 'border-border text-muted-foreground hover:bg-secondary hover:text-foreground'}"
                  title={opt.description}
                >
                  {opt.label}
                </button>
              {/each}
            </div>
            {#if question.options.some((o) => o.preview)}
              {#each question.options.filter((o) => o.preview && (selections[question.header] ?? []).includes(o.label)) as opt (opt.label)}
                <pre
                  class="mb-2 text-[10px] bg-black/5 dark:bg-white/5 rounded px-2 py-1 overflow-x-auto">{opt.preview}</pre>
              {/each}
            {/if}
          {/if}
          <!-- Custom text input -->
          <textarea
            bind:value={customInputs[question.header]}
            placeholder="Type your response here..."
            rows={2}
            class="w-full resize-none rounded-md border border-border bg-background px-2 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-ring"
          ></textarea>
        </div>
      {/each}
      <div class="flex items-center justify-end gap-2 pt-1">
        <button
          type="button"
          onclick={dismiss}
          class="px-3 py-1.5 rounded-md border border-border text-xs text-muted-foreground hover:bg-secondary transition-colors"
        >
          Skip
        </button>
        <button
          type="button"
          onclick={submit}
          class="px-3 py-1.5 rounded-md bg-blue-600 text-white text-xs font-medium hover:bg-blue-700 active:scale-95 transition-colors flex items-center gap-1"
        >
          <Send class="w-3 h-3" />
          Submit
        </button>
      </div>
    </div>
  </div>
{/if}
