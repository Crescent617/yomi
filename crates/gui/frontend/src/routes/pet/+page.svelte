<script lang="ts">
  import { LogicalSize } from "@tauri-apps/api/dpi";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import * as api from "../../lib/api";
  import type { PetSnapshot } from "../../lib/api";
  import { PET_COMPACT_SIZE, getPetWindowSize } from "../../lib/pet";
  import type { PixelPetController } from "../../lib/pixel-pet";

  let petWindow: ReturnType<typeof getCurrentWindow> | undefined;
  let gameHost: HTMLDivElement;
  let pixelPet = $state<PixelPetController | undefined>();
  let snapshot = $state<PetSnapshot | null>(null);
  let requestBubbleVisible = $state(false);
  let observedRequestKey: string | null = null;
  let lastSpeech = "";

  const request = $derived(snapshot?.request ?? null);
  const notice = $derived(snapshot?.notice ?? null);
  const visibleRequest = $derived(requestBubbleVisible ? request : null);
  const visibleNotice = $derived(requestBubbleVisible ? null : notice);
  const petMood = $derived(snapshot?.mood ?? "idle");
  const bubbleText = $derived(
    visibleRequest?.kind === "permission"
      ? "Can I do this?"
      : visibleRequest?.kind === "ask_user"
        ? "I have a question!"
        : visibleNotice?.kind === "completed"
          ? "Mission complete!"
          : visibleNotice?.kind === "failed"
            ? "I got stuck…"
            : visibleNotice?.kind === "max_iterations"
              ? "I need a breather…"
              : visibleNotice?.kind === "cancelled"
                ? "Mission cancelled"
                : "",
  );
  const statusLabel = $derived(
    snapshot?.connection_status === "disconnected"
      ? "OFFLINE"
      : snapshot?.running_count
        ? `WORKING ×${snapshot.running_count}`
        : petMood === "sleepy"
          ? "SLEEPING"
          : "READY",
  );
  const bubbleVisible = $derived(Boolean(visibleRequest || visibleNotice));

  $effect(() => {
    if (!pixelPet || !snapshot) return;
    pixelPet.setMood(snapshot.mood);
  });

  $effect(() => {
    const currentWindow = petWindow;
    const size = getPetWindowSize(bubbleVisible);
    if (!currentWindow) return;

    void currentWindow
      .setSize(new LogicalSize(size.width, size.height))
      .catch((resizeError) => {
        console.error("Failed to resize desktop pet window:", resizeError);
      });
  });

  $effect(() => {
    const speech = document.querySelector<HTMLElement>(".pet-speech");
    if (!speech || lastSpeech === bubbleText) return;
    lastSpeech = bubbleText;
    speech.animate(
      [
        { opacity: 0, transform: "translateY(5px) scale(.94)" },
        { opacity: 1, transform: "translateY(0) scale(1)" },
      ],
      { duration: 180, easing: "steps(3, end)" },
    );
  });

  onMount(() => {
    let disposed = false;
    petWindow = getCurrentWindow();
    void petWindow.setSize(
      new LogicalSize(PET_COMPACT_SIZE.width, PET_COMPACT_SIZE.height),
    );

    const unlistenState = listen<PetSnapshot>("pet:state", (event) => {
      if (!disposed) applySnapshot(event.payload);
    });

    void import("../../lib/pixel-pet")
      .then(({ mountPixelPet }) => mountPixelPet(gameHost, petMood))
      .then((controller) => {
        if (disposed) {
          controller.destroy();
          return;
        }
        pixelPet = controller;
        if (snapshot) pixelPet.setMood(snapshot.mood);
      })
      .catch((renderError) => {
        console.error("Failed to start pixel pet renderer:", renderError);
      });

    void api
      .getPetState()
      .then(applySnapshot)
      .catch((loadError) => {
        console.error("Failed to load desktop pet state:", loadError);
      });

    return () => {
      disposed = true;
      pixelPet?.destroy();
      pixelPet = undefined;
      void unlistenState.then((stop) => stop());
    };
  });

  function applySnapshot(value: PetSnapshot) {
    if (snapshot && value.revision < snapshot.revision) return;
    snapshot = value;
    updateRequestBubble(value);
  }

  function updateRequestBubble(value: PetSnapshot) {
    const requestKey = value.request
      ? `${value.request.kind}:${value.request.req_id}`
      : null;
    if (requestKey === observedRequestKey) return;

    observedRequestKey = requestKey;
    requestBubbleVisible = requestKey !== null;
  }

  function handlePointerLeave() {
    pixelPet?.resetGaze();
  }

  function startDragging(event: PointerEvent) {
    if (event.button !== 0 || !petWindow) return;
    event.preventDefault();
    void petWindow.startDragging().catch((dragError) => {
      console.error("Failed to drag desktop pet:", dragError);
    });
  }
</script>

<svelte:head><title>Yomi Pet</title></svelte:head>

<main
  class="pet-window-root"
  aria-live="polite"
  aria-label={`Yomi desktop pet. ${statusLabel}`}
>
  <div class="pet-stage" data-mood={petMood}>
    {#if bubbleVisible}
      <div class="pet-speech" data-mood={petMood}>
        <span class="pet-speech-icon" aria-hidden="true">
          {visibleRequest
            ? "!"
            : visibleNotice?.kind === "completed"
              ? "★"
              : "×"}
        </span>
        <span>{bubbleText}</span>
      </div>
    {/if}

    <div
      class="pixel-game"
      bind:this={gameHost}
      role="presentation"
      onpointerdown={startDragging}
      onpointerleave={handlePointerLeave}
      title={`Yomi · ${statusLabel} · Drag to move`}
    ></div>
  </div>
</main>

<style>
  :global(.pet-window canvas) {
    display: block;
    width: 152px !important;
    height: 112px !important;
    background: transparent !important;
    cursor: grab;
    image-rendering: pixelated;
  }

  :global(.pet-window canvas:active) {
    cursor: grabbing;
  }

  .pet-window-root {
    width: 100vw;
    height: 100vh;
    overflow: hidden;
    background: transparent;
    user-select: none;
    pointer-events: none;
  }

  .pet-stage {
    position: relative;
    width: 200px;
    height: 216px;
    filter: drop-shadow(0 6px 9px rgb(23 19 50 / 0.2));
  }

  .pixel-game {
    position: absolute;
    top: 0;
    left: 0;
    width: 152px;
    height: 112px;
    border: 0;
    pointer-events: auto;
  }

  .pet-speech {
    position: absolute;
    top: 120px;
    left: 8px;
    z-index: 5;
    display: flex;
    box-sizing: border-box;
    width: 184px;
    min-height: 68px;
    align-items: center;
    gap: 6px;
    padding: 5px 7px;
    border: 3px solid #24213f;
    background: #fffdf5;
    box-shadow:
      4px 4px 0 rgb(36 33 63 / 0.26),
      inset 0 -4px 0 #e8defb;
    color: #24213f;
    font-family: ui-monospace, "SFMono-Regular", Menlo, monospace;
    font-size: 9px;
    font-weight: 800;
    line-height: 1.35;
    image-rendering: pixelated;
    pointer-events: auto;
  }

  .pet-speech::before {
    content: "";
    position: absolute;
    top: -11px;
    left: 34px;
    width: 12px;
    height: 11px;
    background: linear-gradient(
      45deg,
      transparent 0 38%,
      #24213f 39% 58%,
      #fffdf5 59% 78%,
      transparent 79%
    );
  }

  .pet-speech-icon {
    display: grid;
    width: 19px;
    height: 19px;
    flex: 0 0 auto;
    place-items: center;
    border: 2px solid #24213f;
    background: #7868d7;
    box-shadow: 2px 2px 0 #c6bcff;
    color: white;
    font-size: 10px;
    text-shadow: 1px 1px 0 #24213f;
  }

  .pet-speech[data-mood="happy"] .pet-speech-icon {
    background: #50cda9;
  }

  .pet-speech[data-mood="alert"] .pet-speech-icon {
    background: #ffcf70;
    color: #24213f;
  }

  .pet-speech[data-mood="worried"] .pet-speech-icon {
    background: #ff719b;
  }

  @media (prefers-reduced-motion: reduce) {
    .pixel-game {
      animation: none;
    }
  }
</style>
