<script lang="ts">
  // The app's own title bar, for every window except the main one (whose
  // header does the job). Drag it to move the window; double-click to maximise.
  import Icon from '../lib/Icon.svelte';
  import WindowControls from './WindowControls.svelte';
  import { frame } from '../lib/frame.svelte';

  let { maximizable = true }: { maximizable?: boolean } = $props();
</script>

<div class="bar glass-chrome art-head" data-tauri-drag-region>
  <span class="logo"><Icon name="odm-mark" size={12} stroke={2.8} /></span>
  <span class="title ellipsis">{frame.title}</span>
  <WindowControls {maximizable} />
</div>

<style>
  .bar {
    flex: none; height: 32px; display: flex; align-items: center; gap: 9px; padding-left: 10px;
    border-bottom: var(--outline-w, 1px) solid var(--glass-border); position: relative; z-index: 5;
  }
  /* the bar itself is the drag handle: clicks on the logo and title fall through to it */
  .logo, .title { pointer-events: none; }
  .logo { width: 18px; height: 18px; border-radius: 5px; background: linear-gradient(140deg, var(--accent-hi), var(--violet)); color: #fff; display: flex; align-items: center; justify-content: center; flex: none; }
  .title { flex: 1; min-width: 0; font-size: var(--fs-sm); font-weight: 500; color: var(--text-muted); }
</style>
