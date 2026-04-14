import { AlertCircle, CheckCircle2, Loader2, RefreshCw, X } from 'lucide-react';
import React, { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import type { ChatCompletionBackendId } from '../../core/llm/chatBackendEnv';
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
import { getChatProviderMeta } from '../../core/llm/providerRegistry';
import type { ChatModelsByBackend } from '../../core/llm/types';
import {
  ApiError,
  getAgentPlatformApiOriginForDisplay,
  postLlmProxyEnv,
} from '../../api/client';
import { useChatPathStore } from '../../integration/store/chatPathStore';
import { useLlmConnectivityStore } from '../../integration/store/llmConnectivityStore';
import { useLlmSessionStore } from '../../integration/store/llmSessionStore';
import { useUiStore } from '../../integration/store/uiStore';
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
  const [providerSaving, setProviderSaving] = useState(false);
  const [providerSaveError, setProviderSaveError] = useState<string | null>(null);

  const llmSetup = describeLlmSetup();
  const chatPathLoadStatus = useChatPathStore((s) => s.status);
  const chatPathServerProvider = useChatPathStore((s) => s.serverProvider);
  const chatPathServerModel = useChatPathStore((s) => s.serverModel);
  const chatPathLastError = useChatPathStore((s) => s.lastError);
  const chatPathLastLoadedAt = useChatPathStore((s) => s.lastLoadedAt);
  const reloadChatPath = useChatPathStore((s) => s.load);
  const prod = import.meta.env.PROD;
  const localChatBackend: ChatCompletionBackendId | null =
    !prod &&
    (llmSetup.chatBackendId === 'ollama' ||
      llmSetup.chatBackendId === 'lm_studio' ||
      llmSetup.chatBackendId === 'aimlapi')
      ? llmSetup.chatBackendId
      : null;
  const showLocalChatModelPicker = localChatBackend !== null;
  const showCloudChatModelPicker = prod || llmSetup.chatBackendId === 'gemini';
  const chatBackendIdForPicker: ChatCompletionBackendId | null = showLocalChatModelPicker
    ? localChatBackend
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

  const serverHealthFailureHint = (() => {
    const d = (serverChatHealthDetail || '').toLowerCase();
    if (!d) return '';
    if (d.includes('agent_platform_master_key')) {
      return 'Set AGENT_PLATFORM_MASTER_KEY in server .env and restart the backend container/process.';
    }
    if (d.includes('missing or invalid authorization') || d.includes('401')) {
      return 'Agent Platform API auth failed: ensure VITE_AGENT_PLATFORM_MASTER_KEY matches server AGENT_PLATFORM_MASTER_KEY.';
    }
    if (d.includes('timeout')) {
      return 'Readiness probe timed out. Check backend startup logs and upstream proxy reachability.';
    }
    return '';
  })();

  useEffect(() => {
    setChatModelsDraft({ ...llmConfig.chatModelsByBackend });
    setImageModel(llmConfig.imageModel ?? defaultOutputModelForType('image'));
    setMusicModel(llmConfig.musicModel ?? defaultOutputModelForType('music'));
    setVideoModel(llmConfig.videoModel ?? defaultOutputModelForType('video'));
  }, [llmConfig.chatModelsByBackend, llmConfig.imageModel, llmConfig.musicModel, llmConfig.videoModel]);

  const runServerChatHealthCheck = useCallback(() => {
    void useLlmConnectivityStore.getState().runServerChatHealthCheck();
  }, []);
  const refreshChatPath = useCallback(() => {
    void reloadChatPath();
  }, [reloadChatPath]);
  const handleProviderChange = useCallback(
    async (nextProvider: string) => {
      const provider = nextProvider.trim();
      if (!provider) return;
      setProviderSaveError(null);
      setProviderSaving(true);
      try {
        await postLlmProxyEnv({ DEFAULT_PROVIDER: provider });
        await reloadChatPath();
        await useLlmConnectivityStore.getState().runServerChatHealthCheck();
      } catch (e) {
        const message = e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e);
        setProviderSaveError(message);
      } finally {
        setProviderSaving(false);
      }
    },
    [reloadChatPath]
  );

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
    if (!v) return slice?.defaultModel ?? '';
    if (slice?.options.includes(v)) return v;
    return v;
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

  const selectClass =
    'w-full min-w-0 bg-white border border-zinc-200/90 rounded-lg px-2.5 py-2 text-xs text-darkDelegation focus:outline-none focus:ring-2 focus:ring-zinc-200/80 focus:border-zinc-300 transition-colors';

  const sectionTitle = variant === 'page' ? 'text-xl font-black text-darkDelegation' : 'text-3xl font-black text-darkDelegation';

  return (
    <div className={variant === 'page' ? '' : 'max-w-md mx-auto'}>
      <div className="mb-6">
        <h2 className={`${sectionTitle} tracking-tight mb-2`}>
          {variant === 'page' ? 'Backend & models' : 'API & models'}
        </h2>
        <p className="text-zinc-500 text-[13px] leading-snug mb-3">
          Chat and defaults go through the Agent Platform API; the server maps model ids to your stack.
        </p>

        {variant === 'modal' && (
          <p className="text-[10px] text-zinc-400 mb-3">
            <Link
              to="/settings/ai"
              className="text-darkDelegation font-bold hover:underline"
              onClick={onSaved}
            >
              Open full settings
            </Link>{' '}
            for asset quality defaults and 3D performance.
          </p>
        )}

        <div className="mb-3">
          <div className="flex items-center justify-between gap-2 mb-1.5">
            <span className="text-[11px] font-semibold text-zinc-600">LLM provider</span>
            {!prod && (
              <span className="text-[10px] text-zinc-400 truncate max-w-[min(100%,14rem)] text-right" title={getAgentPlatformApiOriginForDisplay() || undefined}>
                {getAgentPlatformApiOriginForDisplay() || '—'}
              </span>
            )}
          </div>
          {prod ? (
            <div className="rounded-lg border border-zinc-200 bg-zinc-50/80 px-3 py-2 text-center">
              <span className="text-xs font-semibold text-darkDelegation">Gemini (production)</span>
              <p className="text-[10px] text-zinc-500 mt-0.5">Key from deploy env.</p>
            </div>
          ) : (
            <>
              <div className="flex items-center gap-2">
                <select
                  className={`${selectClass} flex-1`}
                  aria-label="LLM provider"
                  value={chatPathLoadStatus === 'ok' && chatPathServerProvider ? chatPathServerProvider : ''}
                  disabled={providerSaving || chatPathLoadStatus === 'loading'}
                  onChange={(e) => {
                    void handleProviderChange(e.target.value);
                  }}
                  title="Resolved from Agent Platform (embedded LLM proxy defaults + config)"
                >
                  {chatPathLoadStatus !== 'ok' && (
                    <option value="">
                      {chatPathLoadStatus === 'loading' || chatPathLoadStatus === 'idle'
                        ? 'Loading provider...'
                        : 'Provider unavailable'}
                    </option>
                  )}
                  {CHAT_COMPLETION_BACKEND_IDS.map((id) => (
                    <option key={id} value={id}>
                      {getChatProviderMeta(id).label}
                    </option>
                  ))}
                </select>
                <Button
                  type="button"
                  variant="outline"
                  size="icon-sm"
                  onClick={refreshChatPath}
                  disabled={chatPathLoadStatus === 'loading'}
                  className="border-zinc-200 text-zinc-600 hover:bg-white"
                  title="Refresh resolved chat path from API"
                  aria-label="Refresh chat path"
                >
                  <RefreshCw
                    size={14}
                    strokeWidth={2}
                    className={chatPathLoadStatus === 'loading' ? 'animate-spin' : ''}
                  />
                </Button>
              </div>
              <p className="text-[10px] text-zinc-400 mt-1.5 leading-snug">
                {(chatPathLoadStatus === 'idle' || chatPathLoadStatus === 'loading') &&
                  'Loading resolved provider from Agent Platform…'}
                {chatPathLoadStatus === 'error' &&
                  `Could not load resolved provider from API: ${chatPathLastError || 'unknown error'}`}
                {chatPathLoadStatus === 'ok' &&
                  `Resolved by API: ${chatPathServerProvider ? getChatProviderMeta(chatPathServerProvider).label : 'Unknown'}${chatPathServerModel ? ` (${chatPathServerModel})` : ''}. Provider changes are backend-only.`}
              </p>
              <p className="text-[10px] text-zinc-400 mt-1 leading-snug">
                Runtime backend in this session:{' '}
                <span className="font-medium text-zinc-600">{getChatProviderMeta(llmSetup.chatBackendId).label}</span>
                {chatPathLastLoadedAt ? ` · last synced ${new Date(chatPathLastLoadedAt).toLocaleTimeString()}` : ''}
              </p>
              {providerSaveError && (
                <p className="text-[10px] text-amber-800 mt-1 leading-snug" role="status">
                  Provider update failed: {providerSaveError}
                </p>
              )}
            </>
          )}
        </div>

        {!prod && llmSetup.chatBackendId === 'gemini' && (
          <p className="text-[10px] text-zinc-500 mb-3">
            Gemini credentials are resolved by the backend.
          </p>
        )}

        <p className="text-[10px] text-zinc-400 mb-4 leading-snug">
          Media defaults follow chat unless you set per-modality routing in{' '}
          <code className="font-mono text-[9px]">model-config.ts</code> (<code className="font-mono text-[9px]">follow-chat</code>).
        </p>

        <div className="space-y-3 mb-5">
          {chatBackendIdForPicker && pickerCatalog && pickerSlice && (
            <div className="rounded-lg border border-zinc-200 bg-zinc-50/40 p-3">
              <div className="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between sm:gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="text-[11px] font-semibold text-darkDelegation">Chat model</span>
                    <span className="text-[10px] text-zinc-400">{pickerCatalog.label}</span>
                    <span
                      className={`text-[9px] font-medium px-1.5 py-0 rounded ${
                        pickerSlice.defaultModel === pickerValue ? 'bg-zinc-200/80 text-zinc-600' : 'bg-amber-100/80 text-amber-800'
                      }`}
                    >
                      {pickerSlice.defaultModel === pickerValue ? 'default' : 'custom'}
                    </span>
                  </div>
                </div>
                {llmSetup.showServerChatHealth && (
                  <div className="flex items-center gap-1.5 shrink-0 sm:ml-auto">
                    <div className="flex items-center gap-1.5 text-[10px]">
                      {serverChatHealth === 'checking' && (
                        <>
                          <Loader2 className="animate-spin text-zinc-400 shrink-0" size={14} strokeWidth={2} />
                          <span className="text-zinc-500">Checking…</span>
                        </>
                      )}
                      {serverChatHealth === 'ok' && (
                        <>
                          <CheckCircle2 className="text-emerald-500 shrink-0" size={15} strokeWidth={2} />
                          <span className="text-emerald-700 font-medium">Up</span>
                          {serverChatHealthDetail ? (
                            <span className="font-mono text-zinc-400">{serverChatHealthDetail}</span>
                          ) : null}
                        </>
                      )}
                      {serverChatHealth === 'error' && (
                        <>
                          <AlertCircle className="text-amber-500 shrink-0" size={15} strokeWidth={2} />
                          <span className="text-amber-800 font-medium">Down</span>
                        </>
                      )}
                      {serverChatHealth === 'idle' && <span className="text-zinc-400">—</span>}
                    </div>
                    <Button
                      type="button"
                      variant="outline"
                      size="icon-sm"
                      onClick={() => void runServerChatHealthCheck()}
                      disabled={serverChatHealth === 'checking'}
                      className="border-zinc-200 text-zinc-600 hover:bg-white"
                      title="Re-check embedded LLM proxy (server)"
                      aria-label="Re-check chat backend"
                    >
                      <RefreshCw
                        size={14}
                        strokeWidth={2}
                        className={serverChatHealth === 'checking' ? 'animate-spin' : ''}
                      />
                    </Button>
                  </div>
                )}
              </div>
              <Input
                id="settings-chat-model"
                className={`${selectClass} mt-2 font-mono`}
                list="settings-chat-model-suggestions"
                value={pickerValue}
                placeholder={pickerSlice.defaultModel || 'Model id'}
                autoComplete="off"
                onChange={(e) =>
                  setChatModelsDraft((p) => ({
                    ...p,
                    [chatBackendIdForPicker]: e.target.value,
                  }))
                }
              />
              <datalist id="settings-chat-model-suggestions">
                {pickerSlice.options.map((mid) => (
                  <option key={mid} value={mid} />
                ))}
              </datalist>
              <p className="text-[10px] text-zinc-500 mt-1.5 leading-snug">
                Suggestions merge server catalog and app defaults; you can type any id. Bad ids surface when chat runs. Status checks only hit the embedded proxy for server chat — they never call Gemini.
              </p>
              {serverChatHealth === 'error' && serverChatHealthDetail && (
                <p className="text-[10px] text-amber-800 mt-2 font-mono break-all leading-snug" role="status">
                  {serverChatHealthDetail}
                </p>
              )}
              {serverChatHealth === 'error' && serverHealthFailureHint && (
                <p className="text-[10px] text-amber-900 mt-1 leading-snug">
                  {serverHealthFailureHint}
                </p>
              )}
              {pickerValue.trim() && !pickerSlice.options.includes(pickerValue) && (
                <p className="text-[10px] text-zinc-600 mt-2 leading-snug">
                  Custom model id — not in the merged suggestion list. Chat may error if the backend rejects it.
                </p>
              )}
            </div>
          )}

          <div>
            <p className="text-[11px] font-semibold text-zinc-600 mb-2">Media defaults</p>
            <div className="grid sm:grid-cols-3 gap-2">
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
                  <label className="block text-[10px] font-medium text-zinc-500 mb-1" htmlFor={`media-${kind}`}>
                    {label}
                  </label>
                  {options.length === 0 ? (
                    <p className="text-[10px] text-zinc-500 px-0.5 py-1 leading-snug">
                      Disabled in <code className="font-mono text-[9px]">model-config.ts</code>.
                    </p>
                  ) : (
                    <select
                      id={`media-${kind}`}
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
        </div>

        <div className="rounded-2xl border border-zinc-200 bg-zinc-50/50 px-4 py-3 mb-3 max-w-xl">
          <p className="text-[10px] font-black uppercase tracking-wider text-zinc-500 mb-1">
            Credentials
          </p>
          <p className="text-sm font-medium text-darkDelegation leading-relaxed">
            API secrets are backend-managed and not editable in this UI.
          </p>
          {llmSetup.showServerChatHealth && (
            <p className="text-[11px] text-zinc-500 mt-2 font-medium leading-relaxed">
              {anyMediaRoutedToGemini()
                ? 'Gemini media routes require server-side credentials.'
                : 'Current routing does not require Gemini credentials.'}
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
            className="cursor-pointer rounded-[24px] bg-darkDelegation px-10 py-4 text-xs font-black uppercase tracking-[0.2em] text-white shadow-xl shadow-black/10 hover:bg-black active:scale-95 disabled:cursor-not-allowed disabled:opacity-30 disabled:active:scale-100"
          >
            Save
          </Button>
        </div>
      </div>
    </div>
  );
};
