<script lang="ts">
  let {
    size = "md",
    label = "Loading",
    class: className = "",
  }: {
    size?: "sm" | "md" | "lg";
    label?: string;
    class?: string;
  } = $props();

  const sizes = {
    sm: "h-2 w-5",
    md: "h-2.5 w-7",
    lg: "h-3 w-9",
  } as const;
</script>

<span
  class="loading-indicator relative inline-block shrink-0 overflow-hidden {sizes[
    size
  ]} {className}"
  role="status"
  aria-label={label}
>
  <span
    class="absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-current opacity-15"
  ></span>
  <span class="loading-indicator-comet absolute inset-y-0 left-0 w-3">
    <span
      class="absolute left-0 top-1/2 h-px w-2.5 -translate-y-1/2 bg-gradient-to-r from-transparent via-current to-current opacity-70"
    ></span>
    <span
      class="absolute right-0 top-1/2 size-1.5 -translate-y-1/2 rounded-full bg-current"
    ></span>
  </span>
</span>

<style>
  @keyframes loading-indicator-flow {
    0% {
      opacity: 0;
      transform: translateX(-0.75rem);
    }
    20%,
    80% {
      opacity: 1;
    }
    100% {
      opacity: 0;
      transform: translateX(1.5rem);
    }
  }

  .loading-indicator-comet {
    animation: loading-indicator-flow 1.2s ease-in-out infinite;
    will-change: transform, opacity;
  }

  @media (prefers-reduced-motion: reduce) {
    .loading-indicator-comet {
      animation: none;
      opacity: 1;
      transform: translateX(0.5rem);
    }
  }
</style>
