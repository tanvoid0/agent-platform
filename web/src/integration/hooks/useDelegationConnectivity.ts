import { useMemo } from 'react';
import { describeLlmSetup } from '../../core/llm/llmFacade';
import { useLlmSessionStore } from '../store/llmSessionStore';
import { useLlmConnectivityStore } from '../store/llmConnectivityStore';
import { useProjectsApiStatus } from './useProjectsApiStatus';

/**
 * “Can the delegation UI use Agent Platform?” The browser **never** talks to an LLM or Ollama
 * directly; it only calls Agent Platform. For the server chat path, LLM readiness comes from
 * `GET /api/v1/orchestrator/ready` (backend probes upstream). Cloud chat key checks are separate.
 */
export function useDelegationConnectivity() {
  const projectsStatus = useProjectsApiStatus();
  const serverChatHealth = useLlmConnectivityStore((s) => s.serverChatHealth);
  const serverChatHealthDetail = useLlmConnectivityStore((s) => s.serverChatHealthDetail);
  const apiKey = useLlmSessionStore((s) => s.llmConfig.apiKey);
  const llmSetup = describeLlmSetup();
  const hasStoredOrEnvKey = !!apiKey?.trim();

  return useMemo(() => {
    const projectsDown = projectsStatus === 'offline';
    const projectsChecking = projectsStatus === 'checking';

    /** Blocks chat and agent operations that need the API or model. */
    let backendBlocksChat = false;
    let backendReason = '';

    if (projectsDown) {
      backendBlocksChat = true;
      backendReason = 'Agent Platform API unreachable — check the server and network.';
    } else if (llmSetup.showServerChatHealth) {
      if (serverChatHealth === 'error') {
        backendBlocksChat = true;
        backendReason =
          serverChatHealthDetail.trim() ||
          'LLM stack unreachable — Agent Platform could not reach the orchestrator (browser only uses Agent Platform).';
      }
    } else if (llmSetup.chatRequiresStoredApiKey && !hasStoredOrEnvKey) {
      backendBlocksChat = true;
      backendReason = 'API key required for cloud chat — open AI settings.';
    }

    /** Header traffic light for the robot icon (server path = live probe; cloud = key). */
    let llmTraffic: 'green' | 'yellow' | 'red' = 'green';
    let llmTitle = '';
    if (llmSetup.showServerChatHealth) {
      if (serverChatHealth === 'ok') {
        llmTraffic = 'green';
        llmTitle = serverChatHealthDetail
          ? `Backend LLM path OK (${serverChatHealthDetail}) — UI only called Agent Platform`
          : 'Backend LLM path OK — UI only called Agent Platform';
      } else if (serverChatHealth === 'checking' || serverChatHealth === 'idle') {
        llmTraffic = 'yellow';
        llmTitle = 'Checking backend LLM path — Agent Platform is probing upstream…';
      } else {
        llmTraffic = 'red';
        llmTitle = backendReason || 'LLM stack unreachable';
      }
    } else {
      if (llmSetup.chatRequiresStoredApiKey && !hasStoredOrEnvKey) {
        llmTraffic = 'red';
        llmTitle = backendReason;
      } else {
        llmTraffic = 'green';
        llmTitle =
          'Gemini key OK — text chat uses the GenAI SDK in the browser to Google (not the Agent Platform /api/v1/chat proxy)';
      }
    }

    const projectsTitle =
      projectsStatus === 'online'
        ? 'Agent Platform projects API — reachable'
        : projectsStatus === 'offline'
          ? 'Agent Platform projects API — unreachable'
          : projectsStatus === 'checking'
            ? 'Agent Platform projects API — checking…'
            : 'Projects API disabled';

    const projectsTraffic: 'green' | 'yellow' | 'red' | 'off' =
      projectsStatus === 'disabled'
        ? 'off'
        : projectsStatus === 'online'
          ? 'green'
          : projectsStatus === 'offline'
            ? 'red'
            : 'yellow';

    return {
      projectsStatus,
      projectsDown,
      projectsChecking,
      projectsTitle,
      projectsTraffic,
      serverChatHealth,
      serverChatHealthDetail,
      llmTraffic,
      llmTitle,
      backendBlocksChat,
      backendReason,
    };
  }, [
    projectsStatus,
    serverChatHealth,
    serverChatHealthDetail,
    llmSetup.showServerChatHealth,
    llmSetup.chatRequiresStoredApiKey,
    hasStoredOrEnvKey,
  ]);
}
