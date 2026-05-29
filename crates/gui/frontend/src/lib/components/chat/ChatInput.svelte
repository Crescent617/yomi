<script lang="ts">
  import { Send } from "lucide-svelte";
  import * as api from "../../api";
  import { sessionState, addUserMessage } from "../../state.svelte";

  let content = $state("");
  let textareaRef: HTMLTextAreaElement | null = $state(null);

  async function handleSubmit() {
    if (!content.trim() || !sessionState.activeSessionId) return;

    const sessionId = sessionState.activeSessionId;
    const text = content.trim();
    content = "";

    addUserMessage(sessionId, text);

    try {
      await api.sendMessage(sessionId, text);
    } catch (e: any) {
      console.error("Failed to send message:", e?.message ?? e);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  }

  function autoResize() {
    if (textareaRef) {
      textareaRef.style.height = "auto";
      textareaRef.style.height = Math.min(textareaRef.scrollHeight, 200) + "px";
    }
  }
</script>

<div class="border-t border-border p-3">
  <div class="flex items-end gap-2">
    <textarea
      bind:this={textareaRef}
      bind:value={content}
      oninput={autoResize}
      onkeydown={handleKeydown}
      placeholder="Ask anything..."
      rows={1}
      class="flex-1 resize-none rounded-lg border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
    ></textarea>
    <button
      onclick={handleSubmit}
      disabled={!content.trim() || !sessionState.activeSessionId}
      class="inline-flex items-center justify-center rounded-lg bg-primary text-primary-foreground h-9 w-9 hover:bg-primary/90 disabled:opacity-50"
    >
      <Send size={16} />
    </button>
  </div>
</div>
