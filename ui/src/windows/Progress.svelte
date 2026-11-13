<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import Icon from '../lib/Icon.svelte';
  import FileBadge from '../components/FileBadge.svelte';
  import StatusPill from '../components/StatusPill.svelte';
  import SegmentMap from '../components/SegmentMap.svelte';
  import Sparkline from '../components/Sparkline.svelte';
  import Modal from '../components/Modal.svelte';
  import { api, errText } from '../lib/ipc';
  import { store, initStore, byId } from '../lib/store.svelte';
  import * as f from '../lib/format';
  import { toast } from '../components/Toast.svelte';
  import { frame } from '../lib/frame.svelte';

  let { id }: { id: string } = $props();
  const win = getCurrentWindow();
  const d = $derived(store.ready ? byId(id) : undefined);
  const active = $derived(!!d && ['downloading', 'connecting', 'assembling'].includes(d.status));
  const pct = $derived(d ? f.pct(d) : 0);

  let limitOn = $state(false);
  let limitMB = $state('5');
  let confirmCancel = $state(false);
  let hovering = false;
  let closeIn = $state<number | null>(null);
  let wasComplete: boolean | null = null;

  $effect(() => {
    if (!d) return;
    frame.title = `${pct}% ${d.fileName}`;
    win.setTitle(frame.title).catch(() => {});
    const complete = d.status === 'complete';
    if (wasComplete === false && complete) closeIn = 5;
    wasComplete = complete;
  });

  $effect(() => {
    if (closeIn == null) return;
    const t = setInterval(() => {
      if (hovering || closeIn == null) return;
      closeIn -= 1;
      if (closeIn <= 0) { clearInterval(t); win.close(); }
    }, 1000);
    return () => clearInterval(t);
  });

  async function act(fn: () => Promise<unknown>) {
    try { await fn(); } catch (e) { toast(errText(e), 'error'); }
  }
  function applyLimit() {
    if (!limitOn) return act(() => api.setDownloadLimit(id, null));
    const bps = f.parseLimitMB(limitMB);
    if (bps == null) return toast(f.LIMIT_ERROR, 'error');
    act(() => api.setDownloadLimit(id, bps));
  }

  const conns = $derived.by(() => {
    if (!d) return [];
    return [...d.segments].sort((a, b) => a.start - b.start).map((s, i) => {
      const size = s.end < 1.8e19 ? s.end - s.start : null;
      return { n: String(i + 1).padStart(2, '0'), recv: s.received, size, state: s.state, speed: s.speedBps };
    }).filter((c) => active ? c.state !== 'done' || true : true);
  });
  const stateLabel: Record<string, string> = { receiving: 'Receiving', reconnecting: 'Reconnecting', done: 'Done', connecting: 'Connecting', idle: 'Idle' };
  const stateColor: Record<string, string> = { receiving: 'var(--text)', reconnecting: 'var(--warning)', done: 'var(--success)', connecting: 'var(--text-muted)', idle: 'var(--text-muted)' };

  onMount(() => {
    const cleanups: (() => void)[] = [];
    (async () => {
    await initStore();
    const cur = byId(id);
    if (cur?.speedLimitBps) { limitOn = true; limitMB = String(Math.round((cur.speedLimitBps / f.MBPS) * 10) / 10); }
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape' && !confirmCancel) win.close(); };
    window.addEventListener('keydown', onKey);
    cleanups.push(() => window.removeEventListener('keydown', onKey));
    })();
    return () => cleanups.forEach((c) => c());
  });
</script>

<div class="dialog" role="presentation" onmouseenter={() => (hovering = true)} onmouseleave={() => (hovering = false)}>
  {#if !d}
    <div class="dialog-body"><div class="muted">{store.ready ? 'This download no longer exists.' : 'Loading…'}</div></div>
  {:else}
    <div class="dialog-head">
      <FileBadge ext={d.ext} large />
      <div style="flex:1;min-width:0">
        <h1 class="ellipsis selectable">{d.fileName}</h1>
        <div class="mono muted small ellipsis selectable">{f.host(d.finalUrl || d.url)} · {d.saveDir}</div>
      </div>
      <StatusPill {d} />
    </div>

    <div class="dialog-body">
      <div class="bar"><div class="barfill" style:width="{pct}%" style:background-color={f.statusColor(d.status)}></div></div>
      <div class="hero">
        {#if d.status === 'complete'}
          <div class="done"><Icon name="st-complete" size={36} stroke={2} /><span>Complete</span></div>
        {:else}
          <div class="pct mono" style:--c={f.statusColor(d.status)}>{pct}<span>%</span></div>
        {/if}
        <div class="stats">
          <div class="stat c-teal"><div class="label">Downloaded</div><div class="mono v">{f.bytes(d.receivedBytes)} / {f.bytes(d.totalBytes)}</div></div>
          <div class="stat c-blue"><div class="label">Speed</div><div class="mono v hl">{f.speed(d.speedBps)}</div></div>
          <div class="stat c-violet"><div class="label">Time left</div><div class="mono v">{d.status === 'downloading' ? f.eta(d.etaSecs) : '—'}</div></div>
          <div class="stat {d.resumable ? 'c-green' : 'c-amber'}"><div class="label">Resume</div><div class="v hl">{d.resumable ? 'Supported' : 'Not supported'}</div></div>
        </div>
      </div>

      {#if d.status === 'error' && d.error}<div class="errbox">{d.error}</div>{/if}
      {#if d.checksumOk === true}<div class="okline"><Icon name="verify" size={14} />Checksum verified</div>{/if}

      <div>
        <div class="cap">
          <span>Segment map · {d.activeConnections || d.segments.length} {d.activeConnections === 1 ? 'connection' : 'connections'}</span>
          <span>Faded = segment done · light edge = active write head</span>
        </div>
        <SegmentMap segments={d.segments} status={d.status} total={d.totalBytes} height={30} head />
      </div>

      <div class="lower">
        <div class="conns card">
          <div class="crow chead label"><span>#</span><span>Received</span><span>State</span><span class="r">Speed</span></div>
          <div class="cbody">
            {#each conns as c}
              <div class="crow mono">
                <span class="muted">{c.n}</span>
                <span>{f.bytes(c.recv)}{c.size != null ? ` / ${f.bytes(c.size)}` : ''}</span>
                <span style:color={stateColor[c.state]}>{stateLabel[c.state]}</span>
                <span class="r">{c.state === 'receiving' ? f.speed(c.speed) : '—'}</span>
              </div>
            {:else}
              <div class="crow muted"><span></span><span>No connections yet</span></div>
            {/each}
          </div>
        </div>
        <div class="right">
          <Sparkline samples={d.speedHistory} height={90} />
          <div class="limit card">
            <label class="check" style="justify-content:space-between">
              <span style="display:flex;flex-direction:column"><span>Limit this download</span><span class="muted small">Overrides the per-download setting</span></span>
              <input type="checkbox" bind:checked={limitOn} onchange={applyLimit} />
            </label>
            {#if limitOn}
              <div class="row"><input class="input mono" style="width:100px;text-align:right" bind:value={limitMB} onchange={applyLimit} inputmode="decimal" /><span class="mono muted">MB/s</span></div>
            {/if}
          </div>
          <label class="field">
            <span>When finished</span>
            <select class="select" value={d.whenFinished} onchange={(e) => act(() => api.updateDownload(id, { whenFinished: (e.target as HTMLSelectElement).value }))}>
              <option value="notify">Show notification</option>
              <option value="openFile">Open file</option>
              <option value="openFolder">Open folder</option>
              <option value="none">Do nothing</option>
            </select>
          </label>
        </div>
      </div>
    </div>

    <div class="dialog-foot">
      {#if d.status === 'complete'}
        <button class="btn" onclick={() => win.close()}>Close{closeIn != null ? ` (${closeIn})` : ''}</button>
        <span class="spacer"></span>
        <button class="btn c-orange" onclick={() => act(() => api.openFolder(id))}><Icon name="open-folder" size={15} />Open folder</button>
        <button class="btn primary" onclick={() => act(async () => { await api.openFile(id); win.close(); })}><Icon name="open-file" size={15} />Open file</button>
      {:else}
        <button class="btn" onclick={() => win.close()}>Hide</button>
        <span class="spacer"></span>
        <button class="btn danger" onclick={() => (confirmCancel = true)}>Cancel download</button>
        {#if active || d.status === 'queued'}
          <button class="btn primary" onclick={() => act(() => api.pause([id]))}><Icon name="pause" size={15} />Pause</button>
        {:else}
          <button class="btn primary" onclick={() => act(() => api.resume([id]))}><Icon name="resume" size={15} />{d.status === 'error' ? 'Retry' : 'Resume'}</button>
        {/if}
      {/if}
    </div>
  {/if}
</div>

{#if confirmCancel}
  <Modal title="Cancel this download?" onclose={() => (confirmCancel = false)}>
    <p class="muted" style="margin:0">The partial file is deleted and the download is removed from the list.</p>
    {#snippet footer()}
      <button class="btn" onclick={() => (confirmCancel = false)}>Keep downloading</button>
      <button class="btn primary destructive" onclick={() => act(async () => { await api.delete([id], false); win.close(); })}>Cancel download</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .small { font-size: var(--fs-sm); }
  h1 { font-size: var(--fs-xl); }
  .hero { display: flex; align-items: flex-end; gap: 20px; flex-wrap: wrap; }
  .bar { height: 4px; margin: -18px -20px 0; background: var(--track); flex: none; }
  .barfill { height: 100%; background-image: linear-gradient(90deg, rgba(255,255,255,0), rgba(255,255,255,.45)); box-shadow: 0 0 10px var(--accent-glow); }
  .pct { font-size: var(--fs-hero); font-weight: 600; line-height: 1; letter-spacing: -0.02em; min-width: 110px; color: var(--c); }
  .pct span { font-size: 22px; color: var(--text-muted); }
  .done { display: flex; align-items: center; gap: 10px; color: var(--success); font-size: 22px; font-weight: 600; min-width: 110px; }
  .stats { flex: 1; display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; min-width: 380px; }
  .stat { padding: 8px 10px; border-radius: var(--r-md); background: var(--c-soft); border: 1px solid var(--c-soft); min-width: 0; overflow: hidden; }
  .stat .label { color: var(--c); margin-bottom: 2px; }
  .hl { color: var(--c); font-weight: 600; }
  .v { font-size: var(--fs-md); white-space: nowrap; }
  .cap { display: flex; justify-content: space-between; gap: 12px; font-size: var(--fs-sm); color: var(--text-muted); margin-bottom: 6px; }
  .okline { color: var(--success); display: flex; align-items: center; gap: 6px; font-size: var(--fs-sm); }
  .lower { display: flex; gap: 16px; min-height: 180px; flex: 1; }
  .conns { flex: 1.25; min-width: 0; display: flex; flex-direction: column; overflow: hidden; background: transparent; }
  .crow { display: grid; grid-template-columns: 34px 1.4fr 1fr 90px; gap: 8px; padding: 6px 12px; font-size: var(--fs-sm); border-bottom: 1px solid var(--border-subtle); }
  .chead { background: var(--tint); border-bottom: 1px solid var(--border); padding: 8px 12px; }
  .cbody { overflow: auto; flex: 1; }
  .r { text-align: right; }
  .right { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 12px; }
  .limit { padding: 10px 12px; display: flex; flex-direction: column; gap: 8px; }
  .row { display: flex; gap: 8px; align-items: center; }
</style>
