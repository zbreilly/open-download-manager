<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import Icon from '../lib/Icon.svelte';
  import FileBadge from '../components/FileBadge.svelte';
  import Modal from '../components/Modal.svelte';
  import { api, errText, type Queue } from '../lib/ipc';
  import { store, initStore, byId } from '../lib/store.svelte';
  import * as f from '../lib/format';
  import { toast } from '../components/Toast.svelte';

  let { params }: { params: any } = $props();
  const win = getCurrentWindow();
  let selId = $state('main');
  let draft = $state<Queue | null>(null);
  let dirty = $state(false);
  let naming = $state<{ name: string } | null>(null);
  let dragIdx = $state<number | null>(null);

  const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const current = $derived(store.queues.find((q) => q.id === selId));

  function pick(id: string) {
    selId = id;
    const q = store.queues.find((x) => x.id === id);
    draft = q ? structuredClone($state.snapshot(q)) as Queue : null;
    dirty = false;
  }
  $effect(() => {
    // keep the run state and membership fresh without clobbering edits
    const q = current;
    if (q && draft && draft.id === q.id) {
      draft.running = q.running;
      if (!dirty) draft.order = [...q.order];
    }
  });

  const files = $derived((draft?.order ?? []).map((id) => byId(id)).filter((d) => !!d));
  const pendingFiles = $derived(files.filter((d) => d!.status !== 'complete'));
  const totalSize = $derived(pendingFiles.reduce((a, d) => a + Math.max(0, (d!.totalBytes ?? 0) - d!.receivedBytes), 0));
  const nextRun = $derived.by(() => {
    if (!draft?.startEnabled) return 'not scheduled';
    const [h, m] = draft.startTime.split(':').map(Number);
    const now = new Date();
    for (let add = 0; add < 8; add++) {
      const t = new Date(now.getFullYear(), now.getMonth(), now.getDate() + add, h, m);
      const wd = (t.getDay() + 6) % 7;
      if (!draft.days[wd] || t <= now) continue;
      const day = add === 0 ? (h >= 18 ? 'tonight' : 'today') : add === 1 ? 'tomorrow' : DAYS[wd];
      return `${day} ${draft.startTime}`;
    }
    return 'not scheduled';
  });

  async function act(fn: () => Promise<unknown>) {
    try { await fn(); } catch (e) { toast(errText(e), 'error'); }
  }
  async function save() {
    if (!draft) return;
    await act(async () => {
      const q = await api.saveQueue($state.snapshot(draft) as Queue);
      if (draft) await api.reorderQueue(q.id, [...draft.order]);
      dirty = false;
      toast('Schedule saved');
    });
  }
  async function createQueue() {
    const name = naming?.name.trim();
    naming = null;
    if (!name) return;
    await act(async () => {
      const q = await api.saveQueue({
        id: '', name, builtin: false, maxConcurrent: 2, retryFailed: 3, startEnabled: false, startTime: '01:00',
        stopEnabled: false, stopTime: '07:00', days: [true, true, true, true, true, true, true], onFinish: 'none',
        order: [], running: false, nextRun: null,
      });
      store.queues = await api.listQueues();
      pick(q.id);
    });
  }
  function move(i: number, delta: number) {
    if (!draft) return;
    const j = i + delta;
    if (j < 0 || j >= draft.order.length) return;
    const [x] = draft.order.splice(i, 1);
    draft.order.splice(j, 0, x);
    dirty = true;
  }
  function change() { dirty = true; }

  onMount(() => {
    const cleanups: (() => void)[] = [];
    (async () => {
    await initStore();
    pick(params?.queueId && store.queues.some((q) => q.id === params.queueId) ? params.queueId : store.queues[0]?.id ?? 'main');
    const k = (e: KeyboardEvent) => { if (e.key === 'Escape' && !naming) win.close(); };
    window.addEventListener('keydown', k);
    cleanups.push(() => window.removeEventListener('keydown', k));
    })();
    return () => cleanups.forEach((c) => c());
  });
</script>

<div class="dialog">
  <div class="body">
    <nav class="rail" aria-label="Queues">
      {#each store.queues as q (q.id)}
        <button class="qi" class:on={q.id === selId} onclick={() => pick(q.id)}>
          <span class="ellipsis">{q.name}</span>
          {#if q.running}<span class="run" title="Running"></span>{/if}
          <span class="mono cnt">{q.order.length}</span>
        </button>
      {/each}
      <button class="newq" onclick={() => (naming = { name: '' })}><Icon name="plus" size={14} />New queue</button>
    </nav>

    {#if draft}
      <div class="pane">
        <div class="titlerow">
          <h2>{draft.name}</h2>
          {#if draft.running}<span class="runlbl"><span class="run"></span>Running</span>{/if}
          <span class="spacer"></span>
          <button class="btn sm" disabled={draft.running} onclick={() => act(() => api.startQueue(draft!.id))}><Icon name="start-queue" size={15} />Start now</button>
          <button class="btn sm" disabled={!draft.running} onclick={() => act(() => api.stopQueue(draft!.id))}><Icon name="stop-queue" size={15} />Stop</button>
          {#if !draft.builtin}
            <button class="btn sm icon" style="width:32px;padding:0" aria-label="Delete queue" onclick={() => act(async () => { await api.deleteQueue(draft!.id); store.queues = await api.listQueues(); pick('main'); })}><Icon name="delete" size={15} /></button>
          {/if}
        </div>

        {#if !draft.builtin}
          <label class="field"><span>Name</span><input class="input" bind:value={draft.name} oninput={change} /></label>
        {/if}

        <div class="grid2">
          <div class="tcard card">
            <label class="check between"><span class="b">Start at</span><input type="checkbox" bind:checked={draft.startEnabled} onchange={change} aria-label="Enable start time" /></label>
            <input class="input mono t" type="time" bind:value={draft.startTime} oninput={change} disabled={!draft.startEnabled} />
          </div>
          <div class="tcard card">
            <label class="check between"><span class="b">Stop at</span><input type="checkbox" bind:checked={draft.stopEnabled} onchange={change} aria-label="Enable stop time" /></label>
            <input class="input mono t" type="time" bind:value={draft.stopTime} oninput={change} disabled={!draft.stopEnabled} />
          </div>
        </div>

        <fieldset>
          <legend class="muted small">Repeat on</legend>
          <div class="days">
            {#each DAYS as day, i}
              <button type="button" class="day" aria-pressed={draft.days[i]} class:on={draft.days[i]} onclick={() => { draft!.days[i] = !draft!.days[i]; change(); }}>{day}</button>
            {/each}
          </div>
        </fieldset>

        <div class="grid2">
          <div class="field">
            <span>Files at the same time</span>
            <div class="stepper">
              <button type="button" aria-label="Decrease" onclick={() => { draft!.maxConcurrent = Math.max(1, draft!.maxConcurrent - 1); change(); }}>−</button>
              <input class="mono" bind:value={draft.maxConcurrent} oninput={change} inputmode="numeric" aria-label="Files at the same time" />
              <button type="button" aria-label="Increase" onclick={() => { draft!.maxConcurrent = Math.min(32, draft!.maxConcurrent + 1); change(); }}>+</button>
            </div>
          </div>
          <label class="field">
            <span>Retry failed downloads</span>
            <select class="select" bind:value={draft.retryFailed} onchange={change}>
              <option value={10}>10 times</option>
              <option value={5}>5 times</option>
              <option value={3}>3 times</option>
              <option value={1}>Once</option>
              <option value={0}>Never</option>
            </select>
          </label>
        </div>

        <label class="field">
          <span>When the queue finishes</span>
          <select class="select" bind:value={draft.onFinish} onchange={change}>
            <option value="none">Do nothing</option>
            <option value="sleep">Put computer to sleep</option>
            <option value="shutdown">Shut down computer</option>
            <option value="exit">Exit app</option>
          </select>
        </label>
        {#if draft.onFinish === 'sleep' || draft.onFinish === 'shutdown'}
          <div class="muted small">A 60-second countdown with a Cancel button appears before this happens.</div>
        {/if}

        <div class="field">
          <span>Order (drag or Alt+↑/↓ to reorder)</span>
          <ol class="order">
            {#each draft.order as did, i (did)}
              {@const d = byId(did)}
              {#if d}
                <li draggable="true" class:dragging={dragIdx === i}
                  ondragstart={() => (dragIdx = i)} ondragend={() => (dragIdx = null)}
                  ondragover={(e) => { e.preventDefault(); if (dragIdx != null && dragIdx !== i) { move(dragIdx, i - dragIdx); dragIdx = i; } }}>
                  <button class="handle" aria-label="Reorder {d.fileName}" onkeydown={(e) => { if (e.altKey && e.key === 'ArrowUp') { e.preventDefault(); move(i, -1); } if (e.altKey && e.key === 'ArrowDown') { e.preventDefault(); move(i, 1); } }}><Icon name="drag-handle" size={16} /></button>
                  <span class="mono muted idx">{i + 1}</span>
                  <FileBadge ext={d.ext} />
                  <span class="ellipsis fn">{d.fileName}</span>
                  <span class="muted small">{f.statusLabel(d)}</span>
                  <span class="mono small sz">{f.bytes(d.totalBytes)}</span>
                </li>
              {/if}
            {:else}
              <li class="muted">No files in this queue. Drag downloads onto it in the main window, or pick it when adding.</li>
            {/each}
          </ol>
        </div>
      </div>
    {/if}
  </div>

  <div class="dialog-foot">
    <span class="muted small">Next run: {nextRun} · {pendingFiles.length} {pendingFiles.length === 1 ? 'file' : 'files'} · {f.bytes(totalSize)}</span>
    <span class="spacer"></span>
    <button class="btn" onclick={() => win.close()}>Close</button>
    <button class="btn primary" disabled={!dirty} onclick={save}>Save schedule</button>
  </div>
</div>

{#if naming}
  <Modal title="New queue" width={360} onclose={() => (naming = null)}>
    <input class="input" placeholder="Queue name" bind:value={naming.name} onkeydown={(e) => e.key === 'Enter' && createQueue()} />
    {#snippet footer()}
      <button class="btn" onclick={() => (naming = null)}>Cancel</button>
      <button class="btn primary" onclick={createQueue}>Create</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .body { flex: 1; min-height: 0; display: flex; }
  .rail { width: 210px; flex: none; border-right: 1px solid var(--border); padding: 12px 10px; display: flex; flex-direction: column; gap: 2px; background: #111418; overflow: auto; }
  :global([data-theme='light']) .rail { background: var(--surface-2); }
  .qi { display: flex; align-items: center; gap: 8px; padding: 8px 10px; border-radius: 7px; border: 0; background: transparent; color: var(--text-muted); text-align: left; }
  .qi span:first-child { flex: 1; }
  .qi:hover { background: var(--surface-2); color: var(--text); }
  .qi.on { background: var(--accent-soft); color: var(--text); font-weight: 600; }
  .cnt { font-size: 11px; font-weight: 400; color: var(--text-muted); }
  .run { width: 7px; height: 7px; border-radius: 50%; background: var(--success); flex: none; display: inline-block; }
  .runlbl { display: flex; align-items: center; gap: 6px; color: var(--success); font-size: var(--fs-sm); }
  .newq { margin-top: 8px; display: flex; align-items: center; gap: 6px; padding: 8px 10px; border-radius: 7px; border: 1px dashed var(--switch-off); background: transparent; color: var(--text-muted); }
  .pane { flex: 1; min-width: 0; padding: 18px 20px; display: flex; flex-direction: column; gap: 18px; overflow: auto; }
  .titlerow { display: flex; align-items: center; gap: 10px; }
  h2 { margin: 0; font-size: var(--fs-lg); font-weight: 600; }
  .grid2 { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
  .tcard { padding: 12px; display: flex; flex-direction: column; gap: 6px; }
  .between { justify-content: space-between; }
  .b { font-weight: 500; }
  .t { font-size: var(--fs-lg); }
  .t:disabled { opacity: .5; }
  fieldset { margin: 0; padding: 0; border: 0; }
  legend { padding: 0; margin-bottom: 8px; }
  .small { font-size: var(--fs-sm); }
  .days { display: flex; gap: 6px; flex-wrap: wrap; }
  .day { width: 52px; height: 34px; border-radius: var(--r-md); border: 1px solid var(--border); background: transparent; color: var(--text-muted); }
  .day.on { border-color: var(--accent); background: var(--accent-soft); color: var(--text); font-weight: 600; }
  .stepper { display: flex; align-items: center; height: var(--input-h); border-radius: var(--r-md); border: 1px solid var(--border); background: var(--surface-sunken); overflow: hidden; }
  .stepper button { width: 36px; height: 100%; border: 0; background: transparent; color: var(--text-muted); font-size: 16px; }
  .stepper input { flex: 1; min-width: 0; text-align: center; border: 0; background: transparent; font-size: var(--fs-lg); outline: none; }
  .order { margin: 0; padding: 0; list-style: none; border: 1px solid var(--border); border-radius: var(--r-lg); overflow: hidden; }
  .order li { display: flex; align-items: center; gap: 10px; padding: 7px 12px; border-bottom: 1px solid var(--border-subtle); background: var(--surface); }
  .order li:last-child { border-bottom: 0; }
  .order li.dragging { opacity: .5; }
  .handle { border: 0; background: transparent; color: var(--text-muted); padding: 0; display: flex; cursor: grab; }
  .idx { width: 22px; font-size: var(--fs-sm); }
  .fn { flex: 1; min-width: 0; }
  .sz { width: 80px; text-align: right; }
</style>
