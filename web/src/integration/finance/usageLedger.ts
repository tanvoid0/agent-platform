import { calculateCost } from '../../core/llm/pricing';
import type { LLMTokenUsage } from '../../core/llm/types';

export type UsageLedgerKind = 'chat' | 'final_media';

export interface UsageLedgerEntry {
  id: string;
  timestamp: number;
  agentIndex: number;
  agentName: string;
  taskId?: string;
  kind: UsageLedgerKind;
  model: string;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  estimatedCostUsd: number;
}

const USAGE_LEDGER_CAP = 400;

export interface UsageTotalsSnapshot {
  totalTokenUsage: LLMTokenUsage;
  agentTokenUsage: Record<number, LLMTokenUsage>;
  totalEstimatedCost: number;
  agentEstimatedCost: Record<number, number>;
  usageLedger: UsageLedgerEntry[];
}

export function accumulateUsageAfterResponse(
  snapshot: UsageTotalsSnapshot,
  params: {
    agentIndex: number;
    agentName: string;
    taskId?: string;
    usage: LLMTokenUsage;
    raw?: unknown;
    modelForPricing: string;
    ledgerId: string;
    timestamp: number;
  }
): UsageTotalsSnapshot {
  const { usage, modelForPricing, agentIndex, agentName, taskId, raw, ledgerId, timestamp } = params;
  const r = raw as { duration?: unknown; count?: unknown; model?: unknown } | undefined;
  const durationOrCount = r?.duration ?? r?.count;
  const callCost = calculateCost(
    usage.promptTokens,
    usage.completionTokens,
    modelForPricing,
    durationOrCount as number | undefined
  );

  let nextLedger = [...snapshot.usageLedger, {
    id: ledgerId,
    timestamp,
    agentIndex,
    agentName,
    taskId,
    kind: (agentIndex === -1 ? 'final_media' : 'chat') as UsageLedgerKind,
    model: modelForPricing,
    promptTokens: usage.promptTokens,
    completionTokens: usage.completionTokens,
    totalTokens: usage.totalTokens,
    estimatedCostUsd: callCost,
  } satisfies UsageLedgerEntry];
  if (nextLedger.length > USAGE_LEDGER_CAP) {
    nextLedger = nextLedger.slice(-USAGE_LEDGER_CAP);
  }

  const currentAgentUsage = snapshot.agentTokenUsage[agentIndex] ?? {
    promptTokens: 0,
    completionTokens: 0,
    totalTokens: 0,
  };

  return {
    totalTokenUsage: {
      promptTokens: snapshot.totalTokenUsage.promptTokens + usage.promptTokens,
      completionTokens: snapshot.totalTokenUsage.completionTokens + usage.completionTokens,
      totalTokens: snapshot.totalTokenUsage.totalTokens + usage.totalTokens,
    },
    agentTokenUsage: {
      ...snapshot.agentTokenUsage,
      [agentIndex]: {
        promptTokens: currentAgentUsage.promptTokens + usage.promptTokens,
        completionTokens: currentAgentUsage.completionTokens + usage.completionTokens,
        totalTokens: currentAgentUsage.totalTokens + usage.totalTokens,
      },
    },
    totalEstimatedCost: snapshot.totalEstimatedCost + callCost,
    agentEstimatedCost: {
      ...snapshot.agentEstimatedCost,
      [agentIndex]: (snapshot.agentEstimatedCost[agentIndex] ?? 0) + callCost,
    },
    usageLedger: nextLedger,
  };
}
