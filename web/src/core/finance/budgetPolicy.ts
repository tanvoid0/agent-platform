import { useCoreStore } from '../../integration/store/coreStore';

export class BudgetExceededError extends Error {
  readonly code = 'BUDGET_EXCEEDED' as const;

  constructor(message = 'Project budget limit reached. Increase the cap or clear usage on the Finance page.') {
    super(message);
    this.name = 'BudgetExceededError';
  }

  static is(e: unknown): e is BudgetExceededError {
    return e instanceof BudgetExceededError;
  }
}

/**
 * Blocks further cloud (Gemini) calls when estimated spend is already at or over the user cap.
 * Server-routed chat should not call this (not billed as cloud).
 */
export function assertBudgetAllowsCloudSpend(): void {
  const { budgetLimitUsd, totalEstimatedCost } = useCoreStore.getState();
  if (budgetLimitUsd == null || budgetLimitUsd <= 0) return;
  if (totalEstimatedCost >= budgetLimitUsd) {
    throw new BudgetExceededError();
  }
}
