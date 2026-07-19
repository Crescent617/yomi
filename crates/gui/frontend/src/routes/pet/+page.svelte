<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import {
    cursorPosition,
    getCurrentWindow,
    monitorFromPoint,
    primaryMonitor,
  } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import * as api from "../../lib/api";
  import type { PetPack, PetSnapshot } from "../../lib/api";
  import CodexPetSprite from "../../lib/components/CodexPetSprite.svelte";
  import {
    CODEX_PET_LOOK_DEADZONE,
    getCodexPetFrameDuration,
    horizontalMovementAnimation,
    moodToCodexPetAnimation,
    resolveCodexPetLookDirection,
    type CodexPetAnimationName,
    type CodexPetLookVector,
  } from "../../lib/codex-pet";
  import {
    CodexPetLookController,
    type CodexPetLookOutput,
  } from "../../lib/codex-pet-look";
  import { PET_SIZE, type PetSpriteVersion } from "../../lib/pet";

  let snapshot = $state<PetSnapshot | null>(null);
  let spritesheet_url = $state<string | null>(null);
  let sprite_version_number = $state<PetSpriteVersion>(1);
  let pet_scale = $state(1);
  let look_index = $state<number | null>(null);
  let pet_window: ReturnType<typeof getCurrentWindow> | undefined;
  let load_generation = 0;
  let interaction_animation = $state<CodexPetAnimationName | null>(null);
  let interaction_revision = $state(0);
  // Window position where the current play-once interaction started; used to
  // tell click jitter (keep playing) from a deliberate drag (cancel it).
  let interaction_anchor_x: number | null = null;
  let interaction_anchor_y: number | null = null;
  let last_window_x: number | null = null;
  let last_window_y: number | null = null;
  let drag_start_x: number | null = null;
  let drag_start_y: number | null = null;
  let dragging = false;
  let window_scale_factor = $state(1);
  let primary_scale_factor = 1;
  let normalize_macos_coordinates = $state(false);
  let reduced_motion = $state(false);
  let look_request_pending = false;
  let primary_scale_request_pending = false;
  let visibility_request_pending = false;
  let pet_window_visible = true;
  let last_cursor_x: number | null = null;
  let last_cursor_y: number | null = null;
  const look_controller = new CodexPetLookController();
  let movement_timeout: ReturnType<typeof setTimeout> | undefined;
  let drag_watchdog: ReturnType<typeof setTimeout> | undefined;
  // OS-level window drags swallow the pointerup that ends them on some
  // platforms (e.g. the Windows HTCAPTION drag loop), so a drag is settled
  // from this watchdog instead of relying on pointerup/pointercancel alone.
  const DRAG_WATCHDOG_MS = 250;
  // Window travel (physical px) that distinguishes a real drag from click
  // jitter; every click starts an OS drag session, so small jiggles are
  // common and must not suppress the click jump.
  const DRAG_MOVE_THRESHOLD_PX = 8;
  // Interaction animations that run once and report completion; movement
  // animations loop until the drag settles.
  const PLAY_ONCE_INTERACTIONS: ReadonlySet<CodexPetAnimationName> = new Set([
    "waving",
    "jumping",
  ]);

  function isPlayOnceInteraction(value: CodexPetAnimationName | null): boolean {
    return value !== null && PLAY_ONCE_INTERACTIONS.has(value);
  }

  const animation = $derived(
    interaction_animation ?? moodToCodexPetAnimation(snapshot?.mood ?? "idle"),
  );
  const play_once = $derived(isPlayOnceInteraction(interaction_animation));

  function applyLookOutput(output: CodexPetLookOutput) {
    look_index = output.look_index;
  }

  function syncLookEligibility(now = performance.now()) {
    const output = look_controller.set_eligibility(
      {
        sprite_version_number,
        mood: snapshot?.mood ?? "idle",
        has_interaction: interaction_animation !== null || dragging,
        reduced_motion,
        visible: pet_window_visible,
      },
      now,
    );
    if (output.mode === "ineligible") {
      last_cursor_x = null;
      last_cursor_y = null;
    }
    applyLookOutput(output);
  }

  onMount(() => {
    let disposed = false;
    let scale_revision = 0;
    let position_revision = 0;
    pet_window = getCurrentWindow();
    normalize_macos_coordinates = navigator.userAgent.includes("Macintosh");
    const reduced_motion_query = window.matchMedia(
      "(prefers-reduced-motion: reduce)",
    );
    const update_reduced_motion = () => {
      reduced_motion = reduced_motion_query.matches;
      syncLookEligibility();
    };
    update_reduced_motion();
    reduced_motion_query.addEventListener("change", update_reduced_motion);
    void api
      .getPetScale()
      .then((scale) => {
        if (!disposed) pet_scale = scale;
      })
      .catch((error) =>
        console.error("Failed to load desktop pet scale:", error),
      );

    const unlisten_state = listen<PetSnapshot>("pet:state", (event) => {
      if (!disposed) applySnapshot(event.payload);
    });
    const unlisten_pack = listen<PetPack | null>(
      "pet:pack_changed",
      () => void loadSpritesheet(),
    );
    const unlisten_scale_changed = listen<number>(
      "pet:scale_changed",
      (event) => {
        if (!disposed) pet_scale = event.payload;
      },
    );
    const unlisten_scale = pet_window.onScaleChanged(({ payload }) => {
      if (disposed) return;
      scale_revision += 1;
      window_scale_factor = payload.scaleFactor;
      refreshWindowPosition();
      refreshPrimaryScaleFactor();
    });
    void unlisten_scale.then(() => {
      const initial_scale_revision = scale_revision;
      void pet_window?.scaleFactor().then((scale_factor) => {
        if (!disposed && scale_revision === initial_scale_revision) {
          window_scale_factor = scale_factor;
        }
      });
    });
    const unlisten_moved = pet_window.onMoved(({ payload: position }) => {
      if (disposed) return;
      position_revision += 1;
      const previous_x = last_window_x;
      last_window_x = position.x;
      last_window_y = position.y;
      if (previous_x === null) return;
      const direction = horizontalMovementAnimation(position.x - previous_x);
      if (!direction) return;
      if (isPlayOnceInteraction(interaction_animation)) {
        // A play-once interaction (click jump) owns the sprite: sub-threshold
        // window jitter and late move events from the finished OS drag must
        // not truncate it, but a deliberate drag cancels it.
        const cancel =
          interaction_anchor_x === null ||
          interaction_anchor_y === null ||
          Math.hypot(
            position.x - interaction_anchor_x,
            position.y - interaction_anchor_y,
          ) > DRAG_MOVE_THRESHOLD_PX;
        if (!cancel) return;
        clearInteraction();
      }
      playInteraction(direction);
      if (movement_timeout) window.clearTimeout(movement_timeout);
      movement_timeout = window.setTimeout(
        () => {
          movement_timeout = undefined;
          if (!disposed && !isPlayOnceInteraction(interaction_animation)) {
            interaction_animation = null;
            syncLookEligibility();
          }
        },
        getCodexPetFrameDuration(direction, 0),
      );
    });
    void unlisten_moved.then(() => refreshWindowPosition());
    refreshPrimaryScaleFactor();
    const visibility_interval = window.setInterval(() => {
      if (normalize_macos_coordinates) refreshPrimaryScaleFactor();
      if (disposed || visibility_request_pending) return;
      visibility_request_pending = true;
      void pet_window
        ?.isVisible()
        .then((visible) => {
          if (disposed) return;
          const was_visible = pet_window_visible;
          pet_window_visible = visible;
          if (!visible) {
            syncLookEligibility();
          } else if (!was_visible) {
            last_cursor_x = null;
            last_cursor_y = null;
            syncLookEligibility();
          }
        })
        .catch(() => {
          if (!disposed) {
            pet_window_visible = false;
            syncLookEligibility();
          }
        })
        .finally(() => {
          visibility_request_pending = false;
        });
    }, 1000);
    const look_interval = window.setInterval(() => {
      const now = performance.now();
      syncLookEligibility(now);
      if (look_controller.get_output().mode === "ineligible") return;
      if (look_request_pending) return;

      look_request_pending = true;
      void (async () => {
        try {
          const position = await cursorPosition();
          if (disposed) return;
          const sampled_at = performance.now();
          syncLookEligibility(sampled_at);
          if (look_controller.get_output().mode === "ineligible") return;

          const cursor_available =
            last_window_x !== null &&
            last_window_y !== null &&
            (position.x !== 0 || position.y !== 0);
          if (!cursor_available) {
            last_cursor_x = null;
            last_cursor_y = null;
            applyLookOutput(look_controller.cursor_unavailable(sampled_at));
            return;
          }

          const cursor_moved =
            last_cursor_x === null ||
            last_cursor_y === null ||
            position.x !== last_cursor_x ||
            position.y !== last_cursor_y;
          last_cursor_x = position.x;
          last_cursor_y = position.y;

          if (cursor_moved) {
            let cursor_scale_factor = primary_scale_factor;
            if (normalize_macos_coordinates) {
              // The cursor position is global; normalize it with the scale of
              // the monitor actually under the cursor so mixed-DPI multi-
              // monitor setups still aim the gaze correctly.
              const monitor = await monitorFromPoint(
                position.x,
                position.y,
              ).catch(() => null);
              if (disposed) return;
              if (monitor) cursor_scale_factor = monitor.scaleFactor;
            }
            // Bridges the physical cursor space and the window's logical
            // frame; on macOS both sides are normalized individually instead.
            const k = normalize_macos_coordinates ? 1 : window_scale_factor;
            const direction = cursorLookDirection(
              position.x,
              position.y,
              cursor_scale_factor,
              k,
            );
            applyLookOutput(
              look_controller.cursor_moved(
                resolveCodexPetLookDirection(
                  direction,
                  CODEX_PET_LOOK_DEADZONE * k,
                ),
                sampled_at,
              ),
            );
          } else {
            applyLookOutput(look_controller.tick(sampled_at));
          }
        } catch {
          if (!disposed) {
            applyLookOutput(
              look_controller.cursor_unavailable(performance.now()),
            );
          }
        } finally {
          look_request_pending = false;
        }
      })();
    }, 100);

    void api
      .getPetState()
      .then(applySnapshot)
      .catch((error) => {
        console.error("Failed to load desktop pet state:", error);
      });
    void loadSpritesheet();

    function cursorLookDirection(
      cursor_x: number,
      cursor_y: number,
      cursor_scale_factor: number,
      k: number,
    ): CodexPetLookVector {
      const cursor_scale = normalize_macos_coordinates
        ? cursor_scale_factor
        : 1;
      return {
        x:
          cursor_x / cursor_scale -
          k *
            (last_window_x! / window_scale_factor +
              (PET_SIZE.width * pet_scale) / 2),
        y:
          cursor_y / cursor_scale -
          k *
            (last_window_y! / window_scale_factor +
              (PET_SIZE.height * pet_scale) / 2),
      };
    }

    function refreshWindowPosition() {
      const requested_position_revision = position_revision;
      void pet_window
        ?.outerPosition()
        .then((position) => {
          if (disposed || position_revision !== requested_position_revision)
            return;
          last_window_x = position.x;
          last_window_y = position.y;
        })
        .catch(() => {
          if (!disposed) {
            applyLookOutput(
              look_controller.cursor_unavailable(performance.now()),
            );
          }
        });
    }

    function refreshPrimaryScaleFactor() {
      if (
        disposed ||
        !normalize_macos_coordinates ||
        primary_scale_request_pending
      ) {
        return;
      }
      primary_scale_request_pending = true;
      void primaryMonitor()
        .then((monitor) => {
          if (!disposed && monitor) primary_scale_factor = monitor.scaleFactor;
        })
        .catch(() => {
          if (!disposed) {
            applyLookOutput(
              look_controller.cursor_unavailable(performance.now()),
            );
          }
        })
        .finally(() => {
          primary_scale_request_pending = false;
        });
    }

    async function loadSpritesheet() {
      const generation = ++load_generation;
      try {
        const selected_pack = await api.getSelectedPetPack();
        if (disposed || generation !== load_generation) return;
        if (!selected_pack) {
          sprite_version_number = 1;
          syncLookEligibility();
          replaceSpritesheetUrl(null);
          return;
        }
        const bytes = await api.readSelectedPetSpritesheet(
          selected_pack.id,
          selected_pack.sprite_version_number,
        );
        const next_url = URL.createObjectURL(
          new Blob([bytes], { type: "image/webp" }),
        );
        if (disposed || generation !== load_generation) {
          URL.revokeObjectURL(next_url);
          return;
        }
        const first_load = spritesheet_url === null;
        sprite_version_number = selected_pack.sprite_version_number;
        syncLookEligibility();
        last_cursor_x = null;
        last_cursor_y = null;
        applyLookOutput(look_controller.restart(performance.now()));
        replaceSpritesheetUrl(next_url);
        if (first_load) playInteraction("waving");
      } catch (error) {
        if (disposed || generation !== load_generation) return;
        replaceSpritesheetUrl(null);
        sprite_version_number = 1;
        syncLookEligibility();
        console.error("Failed to load desktop pet spritesheet:", error);
      }
    }

    function replaceSpritesheetUrl(next_url: string | null) {
      if (spritesheet_url) URL.revokeObjectURL(spritesheet_url);
      spritesheet_url = next_url;
    }

    return () => {
      disposed = true;
      load_generation += 1;
      window.clearInterval(visibility_interval);
      window.clearInterval(look_interval);
      reduced_motion_query.removeEventListener("change", update_reduced_motion);
      if (movement_timeout) window.clearTimeout(movement_timeout);
      if (drag_watchdog) window.clearTimeout(drag_watchdog);
      replaceSpritesheetUrl(null);
      void unlisten_state.then((stop) => stop());
      void unlisten_pack.then((stop) => stop());
      void unlisten_scale_changed.then((stop) => stop());
      void unlisten_scale.then((stop) => stop());
      void unlisten_moved.then((stop) => stop());
    };
  });

  function applySnapshot(value: PetSnapshot) {
    if (snapshot && value.revision < snapshot.revision) return;
    snapshot = value;
    syncLookEligibility();
  }

  function clearInteraction() {
    interaction_animation = null;
    interaction_anchor_x = null;
    interaction_anchor_y = null;
  }

  function playInteraction(next_animation: CodexPetAnimationName) {
    const play_once = isPlayOnceInteraction(next_animation);
    // Without a rendered sprite a play-once interaction could never complete
    // and would block movement animations and look behavior forever.
    if (play_once && !spritesheet_url) return;
    const should_restart =
      interaction_animation !== next_animation || play_once;
    interaction_animation = next_animation;
    if (play_once) {
      interaction_anchor_x = last_window_x;
      interaction_anchor_y = last_window_y;
    }
    syncLookEligibility();
    if (should_restart) interaction_revision += 1;
  }

  function completeInteraction(completed_animation: CodexPetAnimationName) {
    if (
      interaction_animation !== completed_animation ||
      !isPlayOnceInteraction(completed_animation)
    )
      return;
    clearInteraction();
    syncLookEligibility();
  }

  function startDragging(event: PointerEvent) {
    if (event.button !== 0 || !pet_window) return;
    event.preventDefault();
    if (drag_watchdog) window.clearTimeout(drag_watchdog);
    drag_watchdog = window.setTimeout(() => {
      drag_watchdog = undefined;
      // The OS consumed the rest of this gesture (modal window drag) or the
      // pointer stream was lost; settle so look behavior cannot stick off.
      if (dragging) settleDrag();
    }, DRAG_WATCHDOG_MS);
    dragging = true;
    syncLookEligibility();
    drag_start_x = last_window_x;
    drag_start_y = last_window_y;
    void pet_window.startDragging().catch((error) => {
      console.error("Failed to drag desktop pet:", error);
    });
  }

  function resetDragState() {
    if (drag_watchdog) {
      window.clearTimeout(drag_watchdog);
      drag_watchdog = undefined;
    }
    dragging = false;
    drag_start_x = null;
    drag_start_y = null;
  }

  function settleDrag() {
    // A gesture whose window never left the threshold counts as a click,
    // including drags that wandered out and returned.
    const should_jump =
      drag_start_x === null ||
      drag_start_y === null ||
      last_window_x === null ||
      last_window_y === null ||
      Math.hypot(last_window_x - drag_start_x, last_window_y - drag_start_y) <=
        DRAG_MOVE_THRESHOLD_PX;
    resetDragState();
    if (should_jump) {
      playInteraction("jumping");
    } else {
      syncLookEligibility();
    }
  }

  function finishDragging(event: PointerEvent) {
    if (event.button !== 0 || !dragging) return;
    settleDrag();
  }

  function cancelDragging() {
    resetDragState();
    syncLookEligibility();
  }
</script>

<svelte:head><title>Yomi Pet</title></svelte:head>

<main
  class="pet-window-root"
  onpointerdown={startDragging}
  onpointerup={finishDragging}
  onpointercancel={cancelDragging}
  aria-label="Yomi desktop pet. Click to play or drag to move."
>
  {#if spritesheet_url}
    <CodexPetSprite
      src={spritesheet_url}
      {animation}
      {play_once}
      {sprite_version_number}
      {look_index}
      scale={pet_scale}
      restart_nonce={interaction_revision}
      on_complete={completeInteraction}
    />
  {/if}
</main>

<style>
  .pet-window-root {
    display: grid;
    width: 100vw;
    height: 100vh;
    place-items: center;
    overflow: hidden;
    background: transparent;
    cursor: grab;
    user-select: none;
  }

  .pet-window-root:active {
    cursor: grabbing;
  }
</style>
