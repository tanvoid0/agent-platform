<script>
  let { children } = $props();
  import { base } from '$app/paths';
  import '../app.css';
</script>

<header>
  <a href="{base}/"><strong>Portal accounts</strong></a>
  <nav>
    <a href="{base}/">Sign in</a>
    <a href="{base}/me">Account</a>
    <a href="{base}/admin">Admin</a>
  </nav>
</header>
<main>
  {@render children()}
</main>
