<script>
  import { api } from '$lib/api.js';

  let email = $state('');
  let msg = $state('');

  async function send() {
    msg = '';
    try {
      const b = await api('/auth/magic-link', {
        method: 'POST',
        body: JSON.stringify({ email })
      });
      msg = b.message + ' Locally, the link is in the server log.';
    } catch (e) {
      msg = e.message;
    }
  }
</script>

<svelte:head><title>Sign in — Portal accounts</title></svelte:head>

<h1>Sign in</h1>
<p class="muted">AI Access, billed monthly after trial. No country and no price on this screen.</p>
<div class="row">
  <input type="email" bind:value={email} placeholder="you@example.com" />
  <button onclick={send}>Email me a link</button>
</div>
<p class="muted">{msg}</p>
