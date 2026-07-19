<script lang="ts">
  import { onMount } from "svelte";
  import {
    CODEX_PET_ANIMATIONS,
    CODEX_PET_CELL_HEIGHT,
    CODEX_PET_CELL_WIDTH,
    getCodexPetFrameDuration,
    getCodexPetFrameGeometry,
    type CodexPetAnimationName,
    type CodexPetLookVector,
  } from "../codex-pet";
  import type { PetSpriteVersion } from "../pet";

  interface Props {
    src: string;
    animation: CodexPetAnimationName;
    play_once?: boolean;
    restart_nonce?: number;
    sprite_version_number?: PetSpriteVersion;
    look_direction?: CodexPetLookVector | null;
    look_deadzone?: number;
    look_index?: number | null;
    scale?: number;
    label?: string;
    on_complete?: (animation: CodexPetAnimationName) => void;
  }

  let {
    src,
    animation,
    play_once = false,
    restart_nonce = 0,
    sprite_version_number = 1,
    look_direction = null,
    look_deadzone = 0,
    look_index = null,
    scale = 1,
    label = "Yomi desktop pet",
    on_complete,
  }: Props = $props();
  let canvas = $state<HTMLCanvasElement>();
  let image = $state<HTMLImageElement | null>(null);
  let frame = $state(0);
  let reduced_motion = $state(false);
  let active_animation = $state<CodexPetAnimationName | null>(null);
  let active_play_once = $state(false);
  let active_restart_nonce = $state(-1);
  let completed = $state(false);

  $effect(() => {
    const current_src = src;
    const next_image = new Image();
    let cancelled = false;

    image = null;
    next_image.decoding = "async";
    next_image.onload = () => {
      if (cancelled) return;
      image = next_image;
      frame = 0;
    };
    next_image.onerror = () => {
      if (!cancelled) console.error("Failed to decode desktop pet spritesheet");
    };
    next_image.src = current_src;

    return () => {
      cancelled = true;
      next_image.onload = null;
      next_image.onerror = null;
      next_image.src = "";
    };
  });

  $effect(() => {
    if (
      animation === active_animation &&
      play_once === active_play_once &&
      restart_nonce === active_restart_nonce
    )
      return;
    active_animation = animation;
    active_play_once = play_once;
    active_restart_nonce = restart_nonce;
    completed = false;
    frame = 0;
  });

  $effect(() => {
    if (completed) return;
    if (reduced_motion) {
      if (!play_once) return;
      const timeout = window.setTimeout(() => {
        completed = true;
        on_complete?.(animation);
      }, 0);
      return () => window.clearTimeout(timeout);
    }
    const timeout = window.setTimeout(
      () => {
        const next_frame = frame + 1;
        if (next_frame >= CODEX_PET_ANIMATIONS[animation].frames) {
          if (play_once) {
            completed = true;
            on_complete?.(animation);
          } else {
            frame = 0;
          }
          return;
        }
        frame = next_frame;
      },
      getCodexPetFrameDuration(animation, frame),
    );
    return () => window.clearTimeout(timeout);
  });

  $effect(() => {
    if (!canvas) return;
    const context = canvas.getContext("2d");
    if (!context) return;

    context.clearRect(0, 0, CODEX_PET_CELL_WIDTH, CODEX_PET_CELL_HEIGHT);
    if (!image) return;

    const geometry = getCodexPetFrameGeometry(
      animation,
      frame,
      sprite_version_number,
      look_direction,
      look_deadzone,
      look_index,
    );
    context.imageSmoothingEnabled = false;
    context.drawImage(
      image,
      -geometry.background_x,
      -geometry.background_y,
      geometry.width,
      geometry.height,
      0,
      0,
      geometry.width,
      geometry.height,
    );
  });

  onMount(() => {
    const media_query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => {
      reduced_motion = media_query.matches;
      if (reduced_motion) frame = 0;
    };
    update();
    media_query.addEventListener("change", update);
    return () => {
      media_query.removeEventListener("change", update);
      canvas
        ?.getContext("2d")
        ?.clearRect(0, 0, CODEX_PET_CELL_WIDTH, CODEX_PET_CELL_HEIGHT);
    };
  });
</script>

<canvas
  bind:this={canvas}
  class="codex-pet-sprite"
  width={CODEX_PET_CELL_WIDTH}
  height={CODEX_PET_CELL_HEIGHT}
  style="width: {CODEX_PET_CELL_WIDTH *
    scale}px; height: {CODEX_PET_CELL_HEIGHT * scale}px;"
  aria-label={label}
  data-animation={animation}
  data-frame={frame}
  data-play-once={play_once}
  data-sprite-version={sprite_version_number}
></canvas>

<style>
  .codex-pet-sprite {
    display: block;
    image-rendering: pixelated;
  }
</style>
