import { AlertCircle, Loader2 } from 'lucide-react';
import React, { useCallback, useEffect, useState } from 'react';
import {
  ApiError,
  fetchLlmProxyConfigYaml,
  fetchLlmProxyEnv,
  fetchLlmProxySnippet,
  getAgentPlatformApiOriginForDisplay,
  type LlmProxyEnvKeyMeta,
} from '../../api/client';

const ENV_FIELDS = [
  ['AGENT_PLATFORM_MASTER_KEY', 'Bearer token for /v1 (OpenAI API key)'],
  ['GEMINI_API_KEY', 'Google Gemini API key'],
  ['AIMLAPI_API_KEY', 'AIMLAPI API key'],
  ['AIMLAPI_OPENAI_BASE', 'AIMLAPI OpenAI base URL (default https://api.aimlapi.com/v1)'],
  ['OLLAMA_API_BASE', 'Ollama base URL (e.g. http://127.0.0.1:11434)'],
  ['LM_STUDIO_API_BASE', 'LM Studio server base (no /v1), e.g. http://127.0.0.1:1234'],
  ['LM_STUDIO_API_KEY', 'Optional Bearer key if LM Studio requires auth'],
] as const;

type ProxyDefaultProvider = 'ollama' | 'lm_studio' | 'aimlapi' | 'gemini';

const DEFAULT_PROVIDER_OPTIONS: { value: ProxyDefaultProvider; label: string }[] = [
  { value: 'lm_studio', label: 'LM Studio' },
  { value: 'ollama', label: 'Ollama' },
  { value: 'aimlapi', label: 'AIMLAPI' },
  { value: 'gemini', label: 'Gemini' },
];

/** Aligns with app/llm_proxy/routes/llm.py ``_effective_defaults`` fallbacks. */
const DEFAULT_MODEL_OPTIONS: Record<ProxyDefaultProvider, readonly string[]> = {
  /** First entry must match ``default_model_for_provider`` in ``provider_config.py``. */
  ollama: ['llama3', 'gemma4:latest', 'qwen2.5vl:7b', 'mistral:latest'],
  lm_studio: ['google/gemma-4-e4b'],
  aimlapi: ['openai/gpt-4.1-mini', 'openai/gpt-4.1', 'anthropic/claude-3.7-sonnet'],
  gemini: [
    'gemini-2.0-flash',
    'gemini-3-flash-preview',
    'gemini-3.1-pro-preview',
    'gemini-3.1-flash-lite-preview',
  ],
};

function isProxyProvider(s: string): s is ProxyDefaultProvider {
  return s === 'ollama' || s === 'lm_studio' || s === 'aimlapi' || s === 'gemini';
}

/** First model per provider — aligned with server ``default_model_for_provider``. */
function firstDefaultModelForProvider(p: ProxyDefaultProvider): string {
  return DEFAULT_MODEL_OPTIONS[p][0];
}

function isSecretEnvKey(key: string): boolean {
  return key === 'AGENT_PLATFORM_MASTER_KEY' || key.endsWith('_KEY');
}

function envPlain(meta: LlmProxyEnvKeyMeta | undefined): string {
  if (!meta || !('value' in meta)) return '';
  return meta.value ?? '';
}

export const LlmProxySettingsPanel: React.FC = () => {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [masked, setMasked] = useState<Record<string, LlmProxyEnvKeyMeta>>({});
  const [effectiveDefaults, setEffectiveDefaults] = useState<{
    OLLAMA_API_BASE?: string;
    LM_STUDIO_API_BASE?: string;
  }>({});
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [yamlText, setYamlText] = useState('');
  const [snippet, setSnippet] = useState('');
  const [publicBase, setPublicBase] = useState('');

  const load = useCallback(async () => {
    setError(null);
    setLoading(true);
    try {
      const [envRes, snip] = await Promise.all([
        fetchLlmProxyEnv(),
        fetchLlmProxySnippet().catch(() => ({ public_base: '', snippet: '' })),
      ]);
      setMasked(envRes.keys);
      setEffectiveDefaults(envRes.effective_defaults ?? {});
      const dpFile = envPlain(envRes.keys.DEFAULT_PROVIDER).trim();
      const dmFile = envPlain(envRes.keys.DEFAULT_MODEL).trim();
      const rs = envRes.resolved_defaults;
      const rp = rs?.provider?.trim().toLowerCase() ?? '';
      const rm = rs?.model?.trim() ?? '';
      const providerDraft =
        (dpFile && isProxyProvider(dpFile) ? dpFile : null) ??
        (rp && isProxyProvider(rp) ? rp : null) ??
        DEFAULT_PROVIDER_OPTIONS[0].value;
      const modelDraft =
        dmFile ||
        rm ||
        firstDefaultModelForProvider(providerDraft);
      setDraft({
        DEFAULT_PROVIDER: providerDraft,
        DEFAULT_MODEL: modelDraft,
      });
      setPublicBase(snip.public_base);
      setSnippet(snip.snippet);
      try {
        const y = await fetchLlmProxyConfigYaml();
        setYamlText(y.content);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) {
          setYamlText('# config.yaml\n# Save to create the file on the server.\n');
        } else throw e;
      }
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e);
      setError(msg);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const effectiveProvider = (): ProxyDefaultProvider => {
    const raw = (draft.DEFAULT_PROVIDER ?? '').trim().toLowerCase();
    if (isProxyProvider(raw)) return raw;
    return DEFAULT_PROVIDER_OPTIONS[0].value;
  };

  const selectClass =
    'w-full bg-zinc-50 border border-zinc-100 rounded-2xl px-4 py-3 text-sm text-darkDelegation focus:outline-none focus:border-zinc-200 transition-all shadow-sm font-mono text-[12px]';

  const origin = getAgentPlatformApiOriginForDisplay();

  const providerForModels = effectiveProvider();
  const modelChoices = DEFAULT_MODEL_OPTIONS[providerForModels];
  const storedModel = draft.DEFAULT_MODEL ?? '';
  const modelKnown = !storedModel || modelChoices.includes(storedModel);

  return (
    <div>
      <h2 className="text-xl font-black text-darkDelegation tracking-tight mb-2">LLM proxy (server)</h2>
      <p className="text-zinc-400 text-sm font-medium leading-relaxed mb-4">
        OpenAI-compatible API is served at{' '}
        <code className="text-[11px] font-mono text-darkDelegation">{origin || '—'}/v1</code> on this
        process. Provider keys and <code className="text-[11px] font-mono">config.yaml</code> are backend-managed.
      </p>
      <p className="text-[10px] text-amber-800 bg-amber-50 border border-amber-100 rounded-xl px-3 py-2 mb-2 font-medium">
        Requires <code className="font-mono text-[10px]">VITE_AGENT_PLATFORM_MASTER_KEY</code> matching the
        API&apos;s <code className="font-mono text-[10px]">AGENT_PLATFORM_MASTER_KEY</code> — same as other
        protected routes.
      </p>
      <p className="text-[10px] text-zinc-500 bg-zinc-50 border border-zinc-200 rounded-xl px-3 py-2 mb-4">
        This page is read-only. Update proxy env/config on the backend host, then reload this page.
      </p>

      {loading && (
        <div className="flex items-center gap-2 text-zinc-500 text-sm mb-4">
          <Loader2 className="animate-spin" size={18} />
          Loading…
        </div>
      )}

      {error && (
        <div className="rounded-2xl border border-red-100 bg-red-50 px-4 py-3 flex items-start gap-2 mb-4">
          <AlertCircle className="text-red-500 shrink-0 mt-0.5" size={18} />
          <p className="text-[12px] text-red-800 font-medium break-words">{error}</p>
        </div>
      )}

      {!loading && (
        <>
          <div className="space-y-3 mb-6">
            {ENV_FIELDS.map(([key, label]) => {
              const meta = masked[key];
              const fallback =
                key === 'OLLAMA_API_BASE'
                  ? effectiveDefaults.OLLAMA_API_BASE
                  : key === 'LM_STUDIO_API_BASE'
                    ? effectiveDefaults.LM_STUDIO_API_BASE
                    : undefined;
              const plain = envPlain(meta);
              return (
                <div key={key}>
                  <label className="block text-[10px] font-black uppercase tracking-wider text-zinc-400 mb-1.5 ml-1">
                    {key}
                    <span className="font-mono font-normal normal-case text-zinc-300"> — {label}</span>
                  </label>
                  <p className="text-[10px] text-zinc-400 mb-1 ml-1">
                    {isSecretEnvKey(key) && meta && 'masked' in meta && meta.set ? (
                      <>
                        Stored <span className="font-mono">{meta.masked || '****'}</span> — leave blank to
                        keep; enter a new value to replace.
                      </>
                    ) : !isSecretEnvKey(key) && meta && 'value' in meta && meta.set ? (
                      <>
                        Stored: <span className="font-mono text-zinc-500">{meta.value}</span>
                      </>
                    ) : fallback ? (
                      <>
                        Not in <span className="font-mono">.env</span> — process uses{' '}
                        <span className="font-mono text-zinc-500">{fallback}</span> unless you save a value
                        here.
                      </>
                    ) : (
                      'Not set.'
                    )}
                  </p>
                  <input
                    type={isSecretEnvKey(key) ? 'password' : 'text'}
                    name={key}
                    autoComplete="off"
                    className={selectClass}
                    placeholder={
                      isSecretEnvKey(key) ? '••••••••' : fallback ? fallback : ''
                    }
                    value={draft[key] ?? plain}
                    readOnly
                  />
                </div>
              );
            })}

            <div>
              <label className="block text-[10px] font-black uppercase tracking-wider text-zinc-400 mb-1.5 ml-1">
                DEFAULT_PROVIDER
                <span className="font-mono font-normal normal-case text-zinc-300">
                  {' '}
                  — Default upstream when the request does not specify a model alias (env overrides empty
                  config.yaml defaults)
                </span>
              </label>
              <p className="text-[10px] text-zinc-400 mb-1 ml-1">
                {masked.DEFAULT_PROVIDER && 'value' in masked.DEFAULT_PROVIDER && masked.DEFAULT_PROVIDER.set
                  ? `Stored: ${masked.DEFAULT_PROVIDER.value || '(empty)'}`
                  : 'Unset — falls back to config.yaml or code defaults.'}
              </p>
              <select
                className={selectClass}
                value={draft.DEFAULT_PROVIDER ?? ''}
                disabled
              >
                <option value="">Unset (config.yaml / auto)</option>
                {DEFAULT_PROVIDER_OPTIONS.map(({ value, label }) => (
                  <option key={value} value={value}>
                    {label}
                  </option>
                ))}
              </select>
            </div>

            <div>
              <label className="block text-[10px] font-black uppercase tracking-wider text-zinc-400 mb-1.5 ml-1">
                DEFAULT_MODEL
                <span className="font-mono font-normal normal-case text-zinc-300">
                  {' '}
                  — Fallback model id when none is set elsewhere (matches provider above)
                </span>
              </label>
              <p className="text-[10px] text-zinc-400 mb-1 ml-1">
                {masked.DEFAULT_MODEL && 'value' in masked.DEFAULT_MODEL && masked.DEFAULT_MODEL.set
                  ? `Stored: ${masked.DEFAULT_MODEL.value || '(empty)'}`
                  : 'Unset — server uses built-in default for the active provider.'}
              </p>
              <select
                className={selectClass}
                disabled
                value={!draft.DEFAULT_PROVIDER?.trim() ? '' : storedModel}
              >
                {!draft.DEFAULT_PROVIDER?.trim() ? (
                  <option value="">Choose DEFAULT_PROVIDER first</option>
                ) : (
                  <>
                    <option value="">Unset (server default for provider)</option>
                    {modelChoices.map((id) => (
                      <option key={id} value={id}>
                        {id}
                      </option>
                    ))}
                    {!modelKnown && storedModel && (
                      <option value={storedModel}>{storedModel} (stored)</option>
                    )}
                  </>
                )}
              </select>
              {!modelKnown && storedModel && (
                <p className="text-[10px] text-amber-800 mt-1 ml-1">
                  Stored id is not in the list — pick a listed model or adjust the list in the UI source.
                </p>
              )}
            </div>
          </div>

          <h3 className="text-lg font-black text-darkDelegation tracking-tight mb-2">config.yaml</h3>
          <p className="text-[10px] text-zinc-500 mb-2">
            Model aliases and defaults for the proxy. Invalid YAML or schema violations are rejected.
          </p>
          <textarea
            className="w-full min-h-[220px] bg-zinc-50 border border-zinc-100 rounded-2xl px-4 py-3 text-[12px] font-mono text-darkDelegation focus:outline-none focus:border-zinc-200 transition-all shadow-sm"
            value={yamlText}
            readOnly
            spellCheck={false}
          />
          <div className="mt-3 mb-8 text-[10px] text-zinc-500">
            Read-only mirror of backend config.
          </div>

          {snippet && (
            <div>
              <h3 className="text-lg font-black text-darkDelegation tracking-tight mb-2">CLI clients</h3>
              <p className="text-[10px] text-zinc-500 mb-2">
                Public base: <code className="font-mono text-[11px]">{publicBase || origin}/v1</code>
              </p>
              <pre className="text-[11px] font-mono bg-zinc-100 border border-zinc-200 rounded-2xl p-4 overflow-x-auto whitespace-pre-wrap">
                {snippet}
              </pre>
            </div>
          )}
        </>
      )}
    </div>
  );
};
