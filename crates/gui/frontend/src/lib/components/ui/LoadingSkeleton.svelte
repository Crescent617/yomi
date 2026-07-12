<script lang="ts">
  let {
    class: className = "",
    rounded = "md",
    style,
  }: {
    class?: string;
    rounded?: "sm" | "md" | "lg" | "full";
    style?: string;
  } = $props();

  const radii = {
    sm: "rounded-sm",
    md: "rounded-md",
    lg: "rounded-lg",
    full: "rounded-full",
  } as const;
</script>

<div
  class="loading-skeleton relative overflow-hidden bg-subtle {radii[
    rounded
  ]} {className}"
  {style}
  aria-hidden="true"
></div>

<style>
  .loading-skeleton::after {
    position: absolute;
    inset: 0;
    content: "";
    background: linear-gradient(
      100deg,
      transparent 20%,
      hsl(var(--foreground) / 0.06) 45%,
      transparent 70%
    );
    transform: translateX(-100%);
    animation: loading-skeleton-shimmer 1.8s ease-in-out infinite;
  }

  @keyframes loading-skeleton-shimmer {
    to {
      transform: translateX(100%);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .loading-skeleton::after {
      display: none;
    }
  }
</style>
