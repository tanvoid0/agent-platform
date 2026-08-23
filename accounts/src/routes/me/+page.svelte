<script>
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { base } from '$app/paths';
  import { api, setTokens, clearTokens, tokens } from '$lib/api.js';

  let user = $state(null);
  let usage = $state([]);
  let err = $state('');
  let price = $state('');

  onMount(async () => {
    if (!tokens().access) {
      goto(`${base}/`);
      return;
    }
    try {
      user = await api('/me');
      const u = await api('/me/usage');
      usage = u.days || [];
    } catch (e) {
      err = e.message;
    }
  });

  async function quote() {
    const card = prompt('Card country (ISO, e.g. US)', 'US');
    const bill = prompt('Billing country (ISO, e.g. BD)', 'BD');
    try {
      const q = await api('/billing/quote', {
        method: 'POST',
        body: JSON.stringify({ card_country: card, billing_country: bill })
      });
      price = `${q.region} · ${q.amount_minor / 100} ${q.currency.toUpperCase()} / month`;
    } catch (e) {
      price = e.message;
    }
  }

  async function portal() {
    try {
      const p = await api('/billing/portal', { method: 'POST', body: '{}' });
      location.href = p.url;
    } catch (e) {
      alert(e.message);
    }
  }

  async function checkout() {
    try {
      const p = await api('/billing/checkout', { method: 'POST', body: '{}' });
      location.href = p.url;
    } catch (e) {
      alert(e.message);
    }
  }

  async function out() {
    await api('/auth/logout', {
      method: 'POST',
      body: JSON.stringify({ refresh_token: tokens().refresh })
    }).catch(() => {});
    clearTokens();
    goto(`${base}/`);
  }
</script>

<svelte:head><title>Account — Portal accounts</title></svelte:head>

{#if err}
  <p class="err">{err}</p>
  <p><a href="{base}/">Sign in</a></p>
{:else if user}
  <h1>{user.email}</h1>
  <p>Entitlement: <strong>{user.entitlement}</strong></p>
  <p class="muted">Trial ends: {user.trial_ends_at || '—'} · Region: {user.billing_region || '(set at checkout)'}</p>
  <div class="row">
    <button onclick={quote}>Preview price</button>
    <button onclick={checkout}>Subscribe</button>
    <button class="ghost" onclick={portal}>Manage billing</button>
    <button class="ghost" onclick={out}>Sign out</button>
  </div>
  <p class="muted">{price}</p>
  <h2>Usage</h2>
  <table>
    <tr><th>Date</th><th>Requests</th><th>Tokens</th></tr>
    {#each usage as d}
      <tr><td>{d.date}</td><td>{d.requests}</td><td>{d.tokens}</td></tr>
    {:else}
      <tr><td colspan="3" class="muted">None yet</td></tr>
    {/each}
  </table>
{/if}
