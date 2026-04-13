export type ProposedPlanAmendmentPayload = {
  title: string;
  description: string;
  assignedAgentId: number;
  feedback: string;
};

type Handler = ((payload: ProposedPlanAmendmentPayload) => void) | null;

let handler: Handler = null;

export function registerProposedPlanAmendmentHandler(h: Handler) {
  handler = h;
}

export function dispatchProposedPlanAmendment(payload: ProposedPlanAmendmentPayload) {
  handler?.(payload);
}
