<script lang="ts">
  // Playwright e2e harness route (src/lib/e2e/harness.ts). Static import
  // so `window.__e2e` is ready by the time page.goto's load event fires.
  // Not linked anywhere in the app.
  import "../../lib/e2e/harness";
</script>
