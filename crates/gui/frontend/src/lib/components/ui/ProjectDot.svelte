<script lang="ts">
  import { projectColor } from "../../utils";

  let {
    name,
    dir,
    class: className = "size-1.5",
  }: {
    name: string;
    dir: string;
    class?: string;
  } = $props();
</script>

<span
  class={`shrink-0 rounded-full ${className}`}
  style={`background: ${projectColor(name + dir)}`}
  aria-hidden="true"
></span>
