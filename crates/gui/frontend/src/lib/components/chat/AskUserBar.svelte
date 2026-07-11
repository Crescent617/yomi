<script lang="ts">
  import {
    ArrowLeft,
    ArrowRight,
    Check,
    CheckSquare2,
    Circle,
    Loader2,
    MessageCircleQuestion,
    Send,
    Square,
  } from "lucide-svelte";
  import { getActiveSession, showNotification } from "../../state.svelte";
  import * as api from "../../api";

  const activeSession = $derived(getActiveSession());
  const askUser = $derived(activeSession?.pending_ask_users[0]);

  let selections = $state<Record<string, string[]>>({});
  let customInputs = $state<Record<string, string>>({});
  let currentQuestionIndex = $state(0);
  let activeRequestId = $state<string | null>(null);
  let submitting = $state(false);
  let skipping = $state(false);

  $effect(() => {
    if (askUser?.req_id !== activeRequestId) {
      activeRequestId = askUser?.req_id ?? null;
      selections = {};
      customInputs = {};
      currentQuestionIndex = 0;
      submitting = false;
      skipping = false;
    }
  });

  const questionCount = $derived(askUser?.questions.length ?? 0);
  const currentQuestion = $derived(
    askUser?.questions[Math.min(currentQuestionIndex, questionCount - 1)],
  );
  const isLastQuestion = $derived(
    questionCount > 0 && currentQuestionIndex === questionCount - 1,
  );
  const hasCurrentAnswer = $derived(
    currentQuestion
      ? (selections[currentQuestion.header]?.length ?? 0) > 0 ||
          Boolean(customInputs[currentQuestion.header]?.trim())
      : false,
  );

  function toggleOption(header: string, label: string, multi: boolean) {
    if (submitting || skipping) return;
    const current = selections[header] ?? [];
    if (multi) {
      selections[header] = current.includes(label)
        ? current.filter((item) => item !== label)
        : [...current, label];
    } else {
      selections[header] = [label];
    }
  }

  function nextQuestion() {
    if (!hasCurrentAnswer || isLastQuestion) return;
    currentQuestionIndex += 1;
  }

  function previousQuestion() {
    if (currentQuestionIndex > 0) currentQuestionIndex -= 1;
  }

  function buildAnswers(): [string, string][] {
    if (!askUser) return [];

    const answers: [string, string][] = [];
    for (const question of askUser.questions) {
      const selected = selections[question.header] ?? [];
      const custom = (customInputs[question.header] ?? "").trim();
      const values = custom ? [...selected, custom] : selected;
      if (values.length > 0) {
        answers.push([question.header, values.join("\n")]);
      }
    }
    return answers;
  }

  async function respondToAskUser(isSkipping: boolean) {
    if (!activeSession || !askUser || submitting || skipping) return;
    if (isSkipping) skipping = true;
    else submitting = true;

    const sessionId = askUser.session_id || activeSession.id;
    const requestId = askUser.req_id;
    try {
      await api.respondAskUser(sessionId, requestId, buildAnswers());
      activeSession.pending_ask_users = activeSession.pending_ask_users.filter(
        (item) => item.req_id !== requestId,
      );
    } catch (e: unknown) {
      showNotification("Response failed: " + api.errorMessage(e), "error");
      submitting = false;
      skipping = false;
    }
  }

  async function submit() {
    await respondToAskUser(false);
  }

  async function skipQuestion() {
    if (!activeSession || !askUser || submitting || skipping) return;

    if (!isLastQuestion) {
      currentQuestionIndex += 1;
      return;
    }

    await respondToAskUser(true);
  }
</script>

{#if askUser && currentQuestion}
  <div
    class="shrink-0 border-t border-border bg-background px-3 py-2.5 sm:px-4"
  >
    <section
      class="relative overflow-hidden rounded-lg border border-info/25 bg-info/5 shadow-sm"
      aria-labelledby="ask-user-title"
    >
      <span class="absolute inset-y-0 left-0 w-0.5 bg-info" aria-hidden="true"
      ></span>

      <div class="px-3.5 py-3 pl-4">
        <div class="flex items-start justify-between gap-3">
          <div class="flex min-w-0 items-start gap-2.5">
            <div
              class="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md bg-info/10 text-info"
            >
              <MessageCircleQuestion class="size-4" />
            </div>
            <div class="min-w-0">
              <h2 id="ask-user-title" class="text-sm font-medium">
                Agent needs your input
              </h2>
              <p class="mt-0.5 text-xs text-muted-foreground">
                Choose an option or provide your own response.
              </p>
            </div>
          </div>
          {#if questionCount > 1}
            <span
              class="shrink-0 text-[11px] tabular-nums text-muted-foreground"
            >
              {currentQuestionIndex + 1} of {questionCount}
            </span>
          {/if}
        </div>

        <div class="mt-3">
          <p class="text-[11px] font-medium uppercase tracking-wide text-info">
            {currentQuestion.header}
          </p>
          <p class="mt-1 text-sm font-medium leading-relaxed">
            {currentQuestion.question}
          </p>
        </div>

        {#if currentQuestion.options.length > 0}
          <div class="mt-3 grid gap-1.5" role="group">
            {#each currentQuestion.options as option (option.label)}
              {@const selected = (
                selections[currentQuestion.header] ?? []
              ).includes(option.label)}
              <button
                type="button"
                onclick={() =>
                  toggleOption(
                    currentQuestion.header,
                    option.label,
                    currentQuestion.multi_select,
                  )}
                disabled={submitting || skipping}
                aria-pressed={selected}
                class="flex w-full items-start gap-2.5 rounded-md border px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50 {selected
                  ? 'border-info/40 bg-info/10'
                  : 'border-border bg-background hover:bg-secondary/50'}"
              >
                {#if currentQuestion.multi_select}
                  {#if selected}
                    <CheckSquare2 class="mt-0.5 size-4 shrink-0 text-info" />
                  {:else}
                    <Square
                      class="mt-0.5 size-4 shrink-0 text-muted-foreground"
                    />
                  {/if}
                {:else if selected}
                  <span
                    class="mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border border-info bg-info text-info-foreground"
                  >
                    <Check class="size-2.5" />
                  </span>
                {:else}
                  <Circle
                    class="mt-0.5 size-4 shrink-0 text-muted-foreground"
                  />
                {/if}
                <span class="min-w-0 flex-1">
                  <span class="block text-xs font-medium text-foreground">
                    {option.label}
                  </span>
                  {#if option.description}
                    <span
                      class="mt-0.5 block text-[11px] leading-relaxed text-muted-foreground"
                    >
                      {option.description}
                    </span>
                  {/if}
                  {#if selected && option.preview}
                    <pre
                      class="mt-2 max-h-32 overflow-auto whitespace-pre-wrap rounded bg-code-bg px-2 py-1.5 font-mono text-[10px] leading-relaxed text-muted-foreground">{option.preview}</pre>
                  {/if}
                </span>
              </button>
            {/each}
          </div>
        {/if}

        <div class="mt-3">
          <label
            for={`ask-user-custom-${currentQuestionIndex}`}
            class="mb-1.5 block text-xs font-medium text-muted-foreground"
          >
            Other response <span class="font-normal">optional</span>
          </label>
          <textarea
            id={`ask-user-custom-${currentQuestionIndex}`}
            bind:value={customInputs[currentQuestion.header]}
            placeholder="Add context or type a different answer..."
            rows={2}
            disabled={submitting || skipping}
            class="w-full resize-y rounded-md border border-input bg-background px-3 py-2 text-xs outline-none transition-shadow placeholder:text-muted-foreground/60 focus:ring-2 focus:ring-ring disabled:opacity-50"
          ></textarea>
        </div>

        <div class="mt-3 flex items-center justify-between gap-2">
          <div>
            {#if currentQuestionIndex > 0}
              <button
                type="button"
                onclick={previousQuestion}
                disabled={submitting || skipping}
                class="inline-flex h-8 items-center gap-1.5 rounded-md px-2 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:opacity-50"
              >
                <ArrowLeft class="size-3.5" /> Back
              </button>
            {/if}
          </div>

          <div class="flex items-center gap-1.5">
            <button
              type="button"
              onclick={skipQuestion}
              disabled={submitting || skipping}
              class="inline-flex h-8 items-center rounded-md border border-border bg-background px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
            >
              {#if skipping}
                <Loader2 class="mr-1.5 size-3.5 animate-spin" />
              {/if}
              Skip question
            </button>
            {#if isLastQuestion}
              <button
                type="button"
                onclick={submit}
                disabled={!hasCurrentAnswer || submitting || skipping}
                class="inline-flex h-8 items-center gap-1.5 rounded-md border border-info/30 bg-info/10 px-3 text-xs font-medium text-info transition-colors hover:border-info/40 hover:bg-info/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
              >
                {#if submitting}
                  <Loader2 class="size-3.5 animate-spin" />
                {:else}
                  <Send class="size-3.5" />
                {/if}
                Submit
              </button>
            {:else}
              <button
                type="button"
                onclick={nextQuestion}
                disabled={!hasCurrentAnswer || submitting || skipping}
                class="inline-flex h-8 items-center gap-1.5 rounded-md border border-info/30 bg-info/10 px-3 text-xs font-medium text-info transition-colors hover:border-info/40 hover:bg-info/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
              >
                Next <ArrowRight class="size-3.5" />
              </button>
            {/if}
          </div>
        </div>
      </div>
    </section>
  </div>
{/if}
