import { AlertCircle, CheckCircle2, Loader2, RefreshCw, X } from 'lucide-react';
import React, { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import type { ChatCompletionBackendId } from '../../core/llm/chatBackendEnv';
import { getGeminiApiKeyFromEnv } from '../../core/llm/geminiApiKeyEnv';
import {
  anyMediaRoutedToGemini,
  defaultOutputModelForType,
  describeLlmSetup,
  getOutputModelPickerOptions,
  resolveMediaBackend,
} from '../../core/llm/llmFacade';
import { persistLlmConfigToStorage } from '../../core/llm/llmConfigStorage';
import {
  CHAT_COMPLETION_BACKEND_IDS,
  getProviderModelCatalog,
} from '../../core/llm/providerModelCatalog';
import type { ChatModelsByBackend } from '../../core/llm/types';
import { getAgentPlatformApiOriginForDisplay } from '../../api/client';
import { useLlmConnectivityStore } from '../../integration/store/llmConnectivityStore';
import { useLlmSessionStore } from '../../integration/store/llmSessionStore';
import { useUiStore } from '../../integration/store/uiStore';
import { getChatCompletionEndpointLabel } from '../../core/llm/chatBackendUi';

export type AiClientsSettingsPanelProps = {
  variant: 'modal' | 'page';
  /** Called after a successful save (e.g. close modal). */
  onSaved?: () => void;
};

export const AiClientsSettingsPanel: React.FC<AiClientsSettingsPanelProps> = ({
  variant,
  onSaved,
}) => {
  const llmConfig = useLlmSessionStore((s) => s.llmConfig);
  const setLlmConfig = useLlmSessionStore((s) => s.setLlmConfig);
  const byokError = useUiStore((s) => s.byokError);

  const [chatModelsDraft, setChatModelsDraft] = useState<ChatModelsByBackend>(() => ({
    ...llmConfig.chatModelsByBackend,
  }));
  const [imageModel, setImageModel] = useState(
    () => llmConfig.imageModel ?? defaultOutputModelForType('image')
  );
  const [musicModel, setMusicModel] = useState(
    () => llmConfig.musicModel ?? defaultOutputModelForType('music')
  );
  const [videoModel, setVideoModel] = useState(
    () => llmConfig.videoModel ?? defaultOutputModelForType('video')
  );
  const [isErrorExpanded, setIsErrorExpanded] = useState(false);
  const [justSaved, setJustSaved] = useState(false);

  const llmSetup = describeLlmSetup();
  const prod = import.meta.env.PROD;
  const showServerChatModelPicker = !prod && llmSetup.chatBackendId === 'ollama';
  const showCloudChatModelPicker = prod || llmSetup.chatBackendId === 'gemini';
  const chatBackendIdForPicker: ChatCompletionBackendId | null = showServerChatModelPicker
    ? 'ollama'
    : showCloudChatModelPicker
      ? 'gemini'
      : null;
  const pickerCatalog = chatBackendIdForPicker
    ? getProviderModelCatalog(chatBackendIdForPicker)
    : null;
  const pickerSlice = pickerCatalog?.chat;
  const pickerValue = chatBackendIdForPicker ? chatModelsDraft[chatBackendIdForPicker] : '';

  const serverChatHealth = useLlmConnectivityStore((s) => s.serverChatHealth);
  const serverChatHealthDetail = useLlmConnectivityStore((s) => s.serverChatHealthDetail);

  useEffect(() => {
    setChatModelsDraft({ ...llmConfig.chatModelsByBackend });
    setImageModel(llmConfig.imageModel ?? defaultOutputModelForType('image'));
    setMusicModel(llmConfig.musicModel ?? defaultOutputModelForType('music'));
    setVideoModel(llmConfig.videoModel ?? defaultOutputModelForType('video'));
  }, [llmConfig.chatModelsByBackend, llmConfig.imageModel, llmConfig.musicModel, llmConfig.videoModel]);

  const runServerChatHealthCheck = useCallback(() => {
    void useLlmConnectivityStore.getState().runServerChatHealthCheck();
  }, []);

  const sanitizeChatModels = (): ChatModelsByBackend =>
    CHAT_COMPLETION_BACKEND_IDS.reduce((acc, id) => {
      const slice = getProviderModelCatalog(id).chat;
      const v = chatModelsDraft[id]?.trim() || slice.defaultModel;
      acc[id] = v;
      return acc;
    }, {} as ChatModelsByBackend);

  const sanitizeMediaModel = (kind: 'image' | 'music' | 'video', raw: string): string => {
    const backend = resolveMediaBackend(kind);
    if (backend === 'disabled') return raw.trim();
    const slice = getProviderModelCatalog(backend)[kind];
    const v = raw.trim();
    if (slice?.options.includes(v)) return v;
    return slice?.defaultModel ?? v;
  };

  const buildConfig = () => {
    const chatModelsByBackend = sanitizeChatModels();
    return {
      chatModelsByBackend,
      imageModel: sanitizeMediaModel('image', imageModel),
      musicModel: sanitizeMediaModel('music', musicModel),
      videoModel: sanitizeMediaModel('video', videoModel),
    };
  };

  const handleSave = () => {
    const config = buildConfig();
    setLlmConfig(config);
    try {
      persistLlmConfigToStorage(useLlmSessionStore.getState().llmConfig);
    } catch (e) {
      console.error('Failed to save LLM config', e);
    }
    if (variant === 'page') {
      setJustSaved(true);
      window.setTimeout(() => setJustSaved(false), 2000);
    }
    onSaved?.();
  };

  const envGeminiKey = getGeminiApiKeyFromEnv();
  const saveDisabled = llmSetup.chatRequiresStoredApiKey && !envGeminiKey;

  const selectClass =
    'w-full bg-zinc-50 border border-zinc-100 rounded-2xl px-4 py-3 text-sm text-darkDelegation focus:outline-none focus:border-zinc-200 transition-all shadow-sm';

  const sectionTitle = variant === 'page' ? 'text-xl font-black text-darkDelegation' : 'text-3xl font-black text-darkDelegation';

  return (
    <div className={variant === 'page' ? '' : 'max-w-md mx-auto'}>
      <div className="mb-6">
        <h2 className={`${sectionTitle} tracking-tight mb-2`}>
          {variant === 'page' ? 'Backend & models' : 'API & models'}
        </h2>
        <p className="text-zinc-400 text-sm font-medium leading-relaxed mb-4">
          Agent chat uses the Agent Platform HTTP API. Default models below are sent with requests; the server decides
          how they map to your LLM stack.
        </p>

        {variant === 'modal' && (
          <p className="text-[10px] text-zinc-400 mb-3">
            <Link
              to="/settings"
              className="text-darkDelegation font-bold hover:underline"
              onClick={onSaved}
            >
              Open full settings
            </Link>{' '}
            for asset quality defaults and 3D performance.
          </p>
        )}

        <div className="mb-1">
          <p className="text-[10px] font-black uppercase tracking-[0.2em] text-zinc-300 mb-2 ml-1">
            Agent chat (this session)
          </p>
          {prod ? (
            <div className="rounded-2xl border border-zinc-200 bg-zinc-50 px-4 py-3 text-center">
              <span className="text-xs font-black uppercase tracking-wider text-darkDelegation">Cloud</span>
              <p className="text-[10px] text-zinc-500 mt-1 font-medium">
                Production build uses cloud chat (API key in env).
              </p>
            </div>
          ) : (
            <div
              className="flex rounded-2xl border border-zinc-200 bg-zinc-100/80 p-1 gap-1"
              role="group"
              aria-label="Agent chat path"
            >
              <div
                className={`flex-1 rounded-xl px-3 py-2.5 text-center transition-all ${
                  llmSetup.chatBackendId === 'ollama'
                    ? 'bg-white shadow-sm ring-1 ring-black/5'
                    : 'opacity-60'
                }`}
              >
                <span className="block text-[10px] font-black uppercase tracking-wider text-darkDelegation">
                  Server
                </span>
                <span className="block text-[9px] text-zinc-500 mt-0.5 font-medium">
                  API{' '}
                  <code className="text-[8px] font-mono">{getAgentPlatformApiOriginForDisplay() || '—'}</code>
                </span>
              </div>
              <div
                className={`flex-1 rounded-xl px-3 py-2.5 text-center transition-all ${
                  llmSetup.chatBackendId === 'gemini'
                    ? 'bg-white shadow-sm ring-1 ring-black/5'
                    : 'opacity-60'
                }`}
              >
                <span className="block text-[10px] font-black uppercase tracking-wider text-darkDelegation">
                  Cloud
                </span>
                <span className="block text-[9px] text-zinc-500 mt-0.5 font-medium">
                  Toggle env <code className="text-[8px] font-mono">VITE_USE_GEMINI_IN_DEV</code>
                </span>
              </div>
            </div>
          )}
        </div>

        {!prod && (
          <p className="text-[10px] font-medium text-zinc-500 mb-3 ml-0.5">
            {llmSetup.chatBackendId === 'ollama'
              ? `Chat requests: ${getChatCompletionEndpointLabel()} on the Agent Platform API (readiness below).`
              : llmSetup.geminiChatForcedInDev
                ? 'Cloud chat is forced for this dev server — a valid key is required.'
                : 'Chat uses the cloud API key from env.'}
          </p>
        )}

        <p className="text-[10px] text-zinc-400 leading-relaxed mt-1 mb-4 ml-0.5">
          Image, audio, and video use the same platform as agent chat by default (<code className="text-[9px] font-mono">follow-chat</code> in{' '}
          <code className="text-[9px] font-mono">model-config.ts</code>). Override per modality there if you need a split setup.
        </p>

        {llmSetup.showServerChatHealth && (
          <div className="rounded-2xl border border-zinc-200 bg-white px-4 py-3 flex items-center justify-between gap-3 mb-6">
            <div className="min-w-0">
              <p className="text-[10px] font-black uppercase tracking-wider text-zinc-400 mb-1">Chat backend</p>
              <div className="flex items-center gap-2 flex-wrap">
                {serverChatHealth === 'checking' && (
                  <>
                    <Loader2 className="animate-spin text-zinc-400 shrink-0" size={16} strokeWidth={2} />
                    <span className="text-xs font-bold text-zinc-500">Checking…</span>
                  </>
                )}
                {serverChatHealth === 'ok' && (
                  <>
                    <CheckCircle2 className="text-emerald-500 shrink-0" size={18} strokeWidth={2} />
                    <span className="text-xs font-black uppercase tracking-wide text-emerald-700">Ready</span>
                    {serverChatHealthDetail && (
                      <span className="text-[10px] font-mono text-zinc-400">{serverChatHealthDetail}</span>
                    )}
                  </>
                )}
                {serverChatHealth === 'error' && (
                  <>
                    <AlertCircle className="text-amber-500 shrink-0" size={18} strokeWidth={2} />
                    <span className="text-xs font-black uppercase tracking-wide text-amber-800">Unreachable</span>
                  </>
                )}
                {serverChatHealth === 'idle' && <span className="text-xs text-zinc-400">—</span>}
              </div>
              {serverChatHealth === 'error' && serverChatHealthDetail && (
                <p className="text-[10px] text-amber-700/90 mt-1.5 font-mono break-all">{serverChatHealthDetail}</p>
              )}
            </div>
            <Button
              type="button"
              variant="outline"
              onClick={() => void runServerChatHealthCheck()}
              disabled={serverChatHealth === 'checking'}
              className="flex shrink-0 items-center gap-1.5 rounded-xl border-zinc-200 px-3 py-2 text-[10px] font-black uppercase tracking-wider text-zinc-500 hover:border-zinc-300 hover:bg-zinc-50 hover:text-darkDelegation disabled:opacity-40"
              title="Re-check chat backend"
            >
              <RefreshCw
                size={14}
                strokeWidth={2.5}
                className={serverChatHealth === 'checking' ? 'animate-spin' : ''}
              />
              Status
            </Button>
          </div>
        )}

        <div className="space-y-4 mb-6">
          {chatBackendIdForPicker && pickerCatalog && pickerSlice && (
            <div>
              <label className="block text-[10px] font-black uppercase tracking-wider text-zinc-400 mb-2 ml-1">
                Chat model ({pickerCatalog.label} ·{' '}
                {pickerSlice.defaultModel === pickerValue ? 'default' : 'custom'})
              </label>
              <select
                className={selectClass}
                value={pickerValue}
                onChange={(e) =>
                  setChatModelsDraft((p) => ({
                    ...p,
                    [chatBackendIdForPicker]: e.target.value,
                  }))
                }
              >
                {!pickerSlice.options.includes(pickerValue) && (
                  <option value={pickerValue}>{pickerValue} (stored)</option>
                )}
                {pickerSlice.options.map((id) => (
                  <option key={id} value={id}>
                    {id}
                  </option>
                ))}
              </select>
              {!pickerSlice.options.includes(pickerValue) && (
                <p className="text-[10px] text-amber-700 mt-1 ml-1">
                  Stored id is not in this backend&apos;s catalog — pick a listed model or adjust your local
                  server.
                </p>
              )}
            </div>
          )}

          <div className="grid sm:grid-cols-3 gap-3">
            {(
              [
                ['image', imageModel, setImageModel] as const,
                ['music', musicModel, setMusicModel] as const,
                ['video', videoModel, setVideoModel] as const,
              ] as const
            ).map(([kind, value, setVal]) => {
              const backend = resolveMediaBackend(kind);
              const modalityTitle = kind === 'music' ? 'Audio' : kind === 'image' ? 'Image' : 'Video';
              const label =
                backend === 'disabled'
                  ? `${modalityTitle} (disabled)`
                  : `${modalityTitle} (${getProviderModelCatalog(backend).label})`;
              const options = getOutputModelPickerOptions(kind);
              return (
                <div key={kind}>
                  <label className="block text-[10px] font-black uppercase tracking-wider text-zinc-400 mb-2 ml-1">
                    {label}
                  </label>
                  {options.length === 0 ? (
                    <p className="text-[10px] text-zinc-500 font-medium px-1 py-2">
                      No models — routing is disabled for this modality in{' '}
                      <code className="font-mono text-[9px]">model-config.ts</code>.
                    </p>
                  ) : (
                    <select
                      className={selectClass}
                      value={value}
                      onChange={(e) => setVal(e.target.value)}
                    >
                      {!options.includes(value) && (
                        <option value={value}>{value} (stored)</option>
                      )}
                      {options.map((id) => (
                        <option key={id} value={id}>
                          {id}
                        </option>
                      ))}
                    </select>
                  )}
                </div>
              );
            })}
          </div>
        </div>

        <h3 className="text-lg font-black text-darkDelegation tracking-tight mb-2">Cloud API key (Gemini)</h3>
        <a
          href="https://aistudio.google.com/app/apikey"
          target="_blank"
          rel="noopener"
          className="group inline-flex items-center gap-2 px-3 py-1.5 bg-emerald-50 hover:bg-emerald-100 border border-emerald-100 hover:border-emerald-200 rounded-full transition-all duration-200 mb-3"
        >
          <span className="text-[10px] font-black uppercase tracking-wider text-emerald-600">
            Get Gemini API Key
          </span>
          <svg
            className="text-emerald-500 group-hover:translate-x-0.5 group-hover:-translate-y-0.5 transition-transform"
            width="10"
            height="10"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="3"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <line x1="7" y1="17" x2="17" y2="7" />
            <polyline points="7 7 17 7 17 17" />
          </svg>
        </a>
        <div
          className={`rounded-2xl border px-4 py-3 mb-3 max-w-xl ${
            envGeminiKey ? 'border-emerald-100 bg-emerald-50/50' : 'border-amber-100 bg-amber-50/40'
          }`}
        >
          <p className="text-[10px] font-black uppercase tracking-wider text-zinc-500 mb-1">
            {envGeminiKey ? 'Configured' : 'Not configured'}
          </p>
          <p className="text-sm font-medium text-darkDelegation leading-relaxed">
            Set <code className="text-[11px] font-mono">VITE_GEMINI_API_KEY</code> in{' '}
            <code className="text-[11px] font-mono">.env.local</code> (or your deploy env) and restart the dev server
            / rebuild. Keys are not read from browser storage.
          </p>
          {llmSetup.showServerChatHealth && (
            <p className="text-[11px] text-zinc-500 mt-2 font-medium leading-relaxed">
              Optional for server chat in dev.
              {anyMediaRoutedToGemini() ? (
                <>
                  {' '}
                  Required when you use <code className="text-[11px] font-mono">VITE_USE_GEMINI_IN_DEV</code> or when
                  image / audio / video routing targets Gemini in <code className="text-[11px] font-mono">model-config.ts</code>.
                </>
              ) : (
                <> Not required for your current routing (local-only media targets).</>
              )}
            </p>
          )}
        </div>
      </div>

      {byokError &&
        (() => {
          const isLongError = byokError.length > 120;
          const displayError = isErrorExpanded || !isLongError ? byokError : byokError.slice(0, 110) + '...';
          return (
            <div className="mb-6 p-3 bg-red-50 border border-red-100 rounded-2xl flex items-start gap-2 animate-in fade-in slide-in-from-top-2">
              <div className="mt-0.5 text-red-500 shrink-0">
                <X size={14} strokeWidth={3} className="rotate-45" />
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-[10px] font-black uppercase tracking-wider text-red-500 mb-0.5">API Error</p>
                <div className={`${isErrorExpanded ? 'max-h-48' : 'max-h-24'} overflow-y-auto pr-1`}>
                  <p className="text-[11px] font-medium text-red-600 leading-tight break-words whitespace-pre-wrap">
                    {displayError}
                  </p>
                  {isLongError && (
                    <Button
                      type="button"
                      variant="link"
                      onClick={() => setIsErrorExpanded(!isErrorExpanded)}
                      className="mt-1 h-auto px-0 text-[9px] font-black uppercase tracking-widest text-red-500 hover:text-red-700"
                    >
                      {isErrorExpanded ? 'Show Less' : 'Show More'}
                    </Button>
                  )}
                </div>
              </div>
            </div>
          );
        })()}

      <div className="flex flex-col sm:flex-row sm:items-center sm:justify-end gap-4">
        <div className="flex items-center gap-3">
          {variant === 'page' && justSaved && (
            <span className="text-[10px] font-black uppercase tracking-wider text-emerald-600">Saved</span>
          )}
          <Button
            type="button"
            onClick={handleSave}
            disabled={saveDisabled}
            className="cursor-pointer rounded-[24px] bg-darkDelegation px-10 py-4 text-xs font-black uppercase tracking-[0.2em] text-white shadow-xl shadow-black/10 hover:bg-black active:scale-95 disabled:cursor-not-allowed disabled:opacity-30 disabled:active:scale-100"
          >
            Save
          </Button>
        </div>
      </div>
    </div>
  );
};
