<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { Quote } from "lucide-svelte";
  import { formatQuoteText, quoteSourceMessageId } from "./quote";

  /**
   * Floating "quote into composer" button. Appears next to a text
   * selection that resolves to exactly one transcript message
   * (`[data-message-id]`); clicking hands the selected text to the
   * composer via `on_quote` and clears the selection.
   *
   * The component never holds a live Range: streaming re-renders and code
   * block enhancement replace DOM nodes under the selection, so the text
   * is captured eagerly on every selectionchange.
   *
   * Scope: selections must resolve to exactly one `[data-message-id]`
   * container (user/assistant transcript messages). Tool outputs inside
   * activity groups carry no message id and are not quotable yet.
   * `message_id` is reported for future source-aware use; the composer
   * currently consumes only the text.
   */
  let {
    on_quote,
  }: {
    on_quote: (text: string, message_id: string) => void;
  } = $props();

  const BTN_HEIGHT = 32;
  const BTN_WIDTH = 32; // icon-only square button
  const VIEWPORT_MARGIN = 8;

  let visible = $state(false);
  let quoteText = $state("");
  let messageId = $state<string | null>(null);
  let left = $state(0);
  let top = $state(0);

  function hide() {
    visible = false;
    quoteText = "";
    messageId = null;
  }

  function updateFromSelection() {
    const sel = window.getSelection();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) {
      hide();
      return;
    }
    // Cheapest gate first: the DOM walk rejects selections outside the
    // transcript before any string work happens (selectionchange fires
    // near-continuously while dragging).
    const mid = quoteSourceMessageId(sel);
    if (!mid) {
      hide();
      return;
    }
    const text = formatQuoteText(sel.toString());
    if (!text) {
      hide();
      return;
    }
    const rect = sel.getRangeAt(0).getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) {
      hide();
      return;
    }
    quoteText = text;
    messageId = mid;
    const above = rect.top - BTN_HEIGHT - 6;
    top = Math.min(
      above >= VIEWPORT_MARGIN ? above : rect.bottom + 6,
      window.innerHeight - BTN_HEIGHT - VIEWPORT_MARGIN,
    );
    left = Math.min(
      Math.max(rect.right - BTN_WIDTH / 2, VIEWPORT_MARGIN),
      window.innerWidth - BTN_WIDTH - VIEWPORT_MARGIN,
    );
    visible = true;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") hide();
  }

  function accept() {
    if (quoteText && messageId) on_quote(quoteText, messageId);
    window.getSelection()?.removeAllRanges();
    hide();
  }

  onMount(() => {
    document.addEventListener("selectionchange", updateFromSelection);
    // Scroll does not bubble — capture phase catches the transcript's
    // inner scroller too. The button anchors to live viewport geometry,
    // so any scroll/resize invalidates its position.
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    window.addEventListener("keydown", handleKeydown);
  });

  onDestroy(() => {
    document.removeEventListener("selectionchange", updateFromSelection);
    window.removeEventListener("scroll", hide, true);
    window.removeEventListener("resize", hide);
    window.removeEventListener("keydown", handleKeydown);
  });
</script>

{#if visible}
  <!-- mousedown preventDefault keeps the selection (and the textarea
       focus) intact until click fires. -->
  <button
    type="button"
    class="fixed z-50 inline-flex h-8 w-8 items-center justify-center rounded-md border border-border bg-card text-muted-foreground shadow-md transition-colors hover:bg-secondary hover:text-foreground"
    style:left={`${left}px`}
    style:top={`${top}px`}
    onmousedown={(e) => e.preventDefault()}
    onclick={accept}
    aria-label="Quote selection in composer"
    title="Quote selection"
  >
    <Quote size={13} aria-hidden="true" />
  </button>
{/if}
