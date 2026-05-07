<!--
  Counter UI.

  Three buttons (-N / reset / +N), a single value display, and a status
  line for SSE state. The page wires up `api.live.subscribe()` in onMount
  and tears it down in onDestroy so opening the same demo in two tabs
  shows both staying in sync — the broadcast hub on the server fans every
  mutation out to every subscriber.

  We do *not* await `api.current()` separately on mount: the `live`
  subscription emits the current value as its first frame (see the server
  comment about subscribing-then-snapshotting), so rolling it into the
  same code path keeps the UI source-of-truth single.

  All u64s cross the wire as `bigint`, so the increment input is `5n`,
  not `5`. Codegen surfaces this in the type signature; passing `5`
  would be a TS error.
-->

<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { isTautError } from "taut-rpc";
  import { api } from "$lib/api";

  /** Latest counter value. `null` until the first `live` frame arrives. */
  let value: bigint | null = null;

  /** SSE connection status, surfaced in the UI for visibility. */
  let status: "connecting" | "live" | "error" | "closed" = "connecting";

  /** Last error message, if any. */
  let errorMessage = "";

  /** Step size for +/- buttons. Bigint to match the wire type. */
  let step: bigint = 1n;

  /**
   * Cancellation handle for the SSE iterator. We capture the iterator
   * itself so onDestroy can call `.return()` and unwind the underlying
   * fetch — without that, navigating away leaks the SSE connection.
   */
  let liveIterator: AsyncIterator<bigint> | null = null;

  onMount(async () => {
    const stream = api.live.subscribe();
    const it = stream[Symbol.asyncIterator]();
    liveIterator = it;
    try {
      while (true) {
        const r = await it.next();
        if (r.done) break;
        value = r.value;
        status = "live";
      }
      status = "closed";
    } catch (e) {
      status = "error";
      errorMessage = e instanceof Error ? e.message : String(e);
    }
  });

  onDestroy(() => {
    // Cancel the SSE iterator so the browser closes the underlying fetch.
    // Ignore the returned promise — we're tearing down regardless.
    void liveIterator?.return?.();
  });

  async function increment(by: bigint) {
    try {
      await api.increment({ by });
      // No need to update `value` here — the broadcast will round-trip
      // through `live` and write it.
    } catch (e) {
      if (isTautError(e, "validation_error")) {
        errorMessage = "validation_error: " + JSON.stringify(e.payload);
      } else {
        errorMessage = e instanceof Error ? e.message : String(e);
      }
    }
  }

  async function reset() {
    try {
      await api.reset();
    } catch (e) {
      errorMessage = e instanceof Error ? e.message : String(e);
    }
  }
</script>

<main>
  <h1>taut-rpc Counter</h1>
  <p class="subtitle">axum + SvelteKit, Phase 5 demo.</p>

  <section class="display">
    <span class="value">{value === null ? "…" : value.toString()}</span>
    <span class="status status--{status}">{status}</span>
  </section>

  <section class="controls">
    <button on:click={() => increment(step)}>+{step}</button>
    <button on:click={reset} class="reset">reset</button>
  </section>
  <p class="note">
    The server only exposes <code>increment</code> (with <code>by ∈ [1,
    1000]</code>) and <code>reset</code>; a "−" button would have nothing
    to call. Add a <code>decrement</code> procedure if you want one.
  </p>

  <section class="step">
    <label>
      step:
      <input
        type="number"
        min="1"
        max="1000"
        value={step.toString()}
        on:input={(e) => {
          // Coerce the HTMLInputElement string value back to bigint. The
          // input is `bind`-less because Svelte's `bind:value` coerces to
          // `number`, but the procedure takes `bigint` (u64 → bigint per
          // SPEC §3.1). The server's Validate impl rejects anything
          // outside [1, 1000] either way.
          const n = Number((e.currentTarget as HTMLInputElement).value);
          step = BigInt(Number.isFinite(n) && n > 0 ? Math.floor(n) : 1);
        }}
      />
    </label>
  </section>

  {#if errorMessage}
    <p class="error">{errorMessage}</p>
  {/if}

  <footer>
    <p>
      Open this page in another tab to watch the live subscription stay in
      sync — the server broadcasts every mutation to every connected client.
    </p>
  </footer>
</main>

<style>
  main {
    max-width: 32rem;
    margin: 4rem auto;
    padding: 0 1rem;
    font-family: system-ui, -apple-system, sans-serif;
  }
  h1 {
    margin: 0;
    font-size: 1.5rem;
  }
  .subtitle {
    color: #666;
    margin: 0.25rem 0 2rem;
  }
  .display {
    display: flex;
    align-items: baseline;
    gap: 1rem;
    padding: 2rem 0;
    border-top: 1px solid #eee;
    border-bottom: 1px solid #eee;
  }
  .value {
    font-size: 4rem;
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }
  .status {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 0.125rem 0.5rem;
    border-radius: 4px;
  }
  .status--connecting {
    background: #fef3c7;
    color: #92400e;
  }
  .status--live {
    background: #d1fae5;
    color: #065f46;
  }
  .status--error,
  .status--closed {
    background: #fee2e2;
    color: #991b1b;
  }
  .controls {
    display: flex;
    gap: 0.5rem;
    margin-top: 1.5rem;
  }
  button {
    padding: 0.5rem 1rem;
    font-size: 1rem;
    border: 1px solid #ddd;
    border-radius: 4px;
    background: white;
    cursor: pointer;
  }
  button:hover:not(:disabled) {
    background: #f5f5f5;
  }
  button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  button.reset {
    margin-left: auto;
  }
  .step {
    margin-top: 1.5rem;
  }
  .note {
    margin-top: 0.75rem;
    color: #666;
    font-size: 0.85rem;
  }
  .note code {
    background: #f5f5f5;
    padding: 0 0.25rem;
    border-radius: 2px;
  }
  .step input {
    width: 5rem;
    padding: 0.25rem 0.5rem;
    font-size: 1rem;
  }
  .error {
    color: #991b1b;
    background: #fee2e2;
    padding: 0.5rem;
    border-radius: 4px;
    font-family: ui-monospace, monospace;
    font-size: 0.85rem;
    margin-top: 1rem;
  }
  footer {
    margin-top: 3rem;
    color: #666;
    font-size: 0.85rem;
  }
</style>
