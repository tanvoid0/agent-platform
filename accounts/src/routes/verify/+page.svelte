<script>
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { base } from '$app/paths';
  import { api, setTokens } from '$lib/api.js';

  let msg = $state('Signing you in…');

  onMount(async () => {
    const token = $page.url.searchParams.get('token');
    if (!token) {
      msg = 'Missing token.';
      return;
    }
    try {
      const b = await api('/auth/verify', {
        method: 'POST',
        body: JSON.stringify({ token })
      });
      setTokens(b.access_token, b.refresh_token);
      goto(`${base}/me`);
    } catch (e) {
      msg = e.message;
    }
  });
</script>

<p>{msg}</p>
