<script>
  import { onMount } from 'svelte';
  import { api } from '$lib/api.js';

  let users = $state([]);
  let err = $state('');
  let email = $state('');
  let reason = $state('');
  let entitlement = $state('trial');

  async function load() {
    try {
      const b = await api('/admin/users');
      users = b.users || [];
    } catch (e) {
      err = e.message;
    }
  }

  onMount(load);

  async function comp() {
    await api('/admin/grant-comp', { method: 'POST', body: JSON.stringify({ email, reason }) });
    await load();
  }
  async function setEnt() {
    await api('/admin/set-entitlement', {
      method: 'POST',
      body: JSON.stringify({ email, entitlement })
    });
    await load();
  }
  async function rev() {
    await api('/admin/revoke-sessions', { method: 'POST', body: JSON.stringify({ email }) });
    await load();
  }
</script>

<svelte:head><title>Admin — Portal accounts</title></svelte:head>

<h1>Users</h1>
{#if err}
  <p class="err">{err}</p>
{:else}
  <div class="row">
    <input bind:value={email} placeholder="email@domain" />
    <input bind:value={reason} placeholder="comp reason" />
    <button onclick={comp}>Grant comp</button>
    <select bind:value={entitlement}>
      <option>trial</option>
      <option>paid</option>
      <option>comp</option>
      <option>blocked</option>
    </select>
    <button class="ghost" onclick={setEnt}>Set entitlement</button>
    <button class="ghost" onclick={rev}>Revoke sessions</button>
  </div>
  <table>
    <tr><th>Email</th><th>State</th><th>Region</th><th>Admin</th><th>Stripe</th></tr>
    {#each users as u}
      <tr>
        <td>{u.email}</td>
        <td>{u.entitlement}</td>
        <td>{u.billing_region || ''}</td>
        <td>{u.is_admin ? 'yes' : ''}</td>
        <td>{u.stripe_customer_id || ''}</td>
      </tr>
    {/each}
  </table>
{/if}
