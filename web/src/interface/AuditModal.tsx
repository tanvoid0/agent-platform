import React, { useState } from 'react';
import { X, CheckCircle2, AlertCircle, GitPullRequest, PencilLine } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import { useCoreStore } from '../integration/store/coreStore';
import { useShallow } from 'zustand/react/shallow';
import { useAuditModalUiActions } from '../integration/store/uiSelectors';
import { ModalBackdrop, ModalRoot } from './components/ModalChrome';
import { getAllAgents } from '../data/agents';
import { useActiveTeam } from '../integration/store/teamStore';
import { AgentPresenceBadge } from './components/AgentPresenceBadge';
import { Avatar } from './components/Avatar';
import { InfoBubble } from './components/InfoBubble';
import { USER_COLOR, USER_COLOR_LIGHT, USER_COLOR_SOFT } from '../theme/brand';

interface AuditModalProps {
  taskId: string;
  isOpen: boolean;
  onClose: () => void;
  viewOnly?: boolean;
}

export const AuditModal: React.FC<AuditModalProps> = ({ taskId, isOpen, onClose, viewOnly }) => {
  const { tasks, approveTask, rejectTask, approveProposedTask, amendProposedTaskPlan, removeTask } = useCoreStore(
    useShallow((s) => ({
      tasks: s.tasks,
      approveTask: s.approveTask,
      rejectTask: s.rejectTask,
      approveProposedTask: s.approveProposedTask,
      amendProposedTaskPlan: s.amendProposedTaskPlan,
      removeTask: s.removeTask,
    })),
  );
  const { setSelectedNpc, setChatting } = useAuditModalUiActions();
  const activeTeam = useActiveTeam();
  const agents = getAllAgents(activeTeam);
  const [feedback, setFeedback] = useState('');
  const [selectedRevisionIndex, setSelectedRevisionIndex] = useState<number | null>(null);

  const task = tasks.find(t => t.id === taskId);
  if (!task || !isOpen) return null;

  const isPlanApprovalMode = task.status === 'scheduled' && task.requiresUserApproval;
  const isViewMode = viewOnly || task.status === 'done';

  const agent = agents.find(a => a.index === task.assignedAgentId);

  const handleApprove = () => {
    approveTask(taskId);
    setSelectedNpc(null);
    setChatting(false);
    onClose();
  };

  const handleReject = () => {
    if (!feedback.trim()) return;
    rejectTask(taskId, feedback);
    setSelectedNpc(null);
    setChatting(false);
    onClose();
  };

  const handleApprovePlan = () => {
    approveProposedTask(taskId);
    setSelectedNpc(null);
    setChatting(false);
    onClose();
  };

  const handleRemoveProposedTask = () => {
    removeTask(taskId);
    setSelectedNpc(null);
    setChatting(false);
    onClose();
  };

  const handleAmendPlan = () => {
    if (!feedback.trim()) return;
    amendProposedTaskPlan(taskId, feedback);
    setSelectedNpc(null);
    setChatting(false);
    onClose();
  };

  return (
    <ModalRoot layer="audit" paddingClassName="p-6 md:p-12">
      <ModalBackdrop
        tone="lightXl"
        onRequestClose={onClose}
        className="animate-in fade-in duration-500"
      />

      {/* Modal Container */}
      <div className="relative bg-white w-full max-w-4xl max-h-full rounded-[32px] shadow-[0_32px_64px_-16px_rgba(0,0,0,0.1)] border border-zinc-200/50 flex flex-col overflow-hidden animate-in zoom-in-95 fade-in duration-500 ease-out fill-mode-both">

        {/* Header: Character Focus */}
        <div className="px-8 py-8 flex items-center justify-between bg-white shrink-0">
          <div className="flex items-center gap-6">
            <div className="relative pb-3">
              <Avatar
                type={agent?.index === 0 ? 'user' : 'sub'}
                color={agent?.color}
                size={64}
              />
              {agent != null ? (
                <div className="absolute -bottom-2 left-1/2 z-10 flex -translate-x-1/2 justify-center rounded-full border border-zinc-200/80 bg-white px-2 py-0.5 shadow-sm">
                  <AgentPresenceBadge agentIndex={agent.index} size="sm" />
                </div>
              ) : null}
            </div>

            <div className="flex flex-col">
              <div className="flex items-center gap-2 mb-1">
                <span className="text-sm font-black text-darkDelegation uppercase tracking-widest">{agent?.name}</span>
                {!isViewMode && (
                  <span
                    className="text-[10px] font-bold uppercase tracking-widest px-2 py-0.5 rounded-full"
                    style={
                      isPlanApprovalMode
                        ? { backgroundColor: '#eef2ff', color: '#4f46e5' }
                        : { backgroundColor: USER_COLOR_LIGHT, color: USER_COLOR }
                    }
                  >
                    {isPlanApprovalMode ? 'Approve plan' : 'Requires Review'}
                  </span>
                )}
              </div>
              <h2 className="text-2xl font-semibold text-darkDelegation tracking-tight leading-tight">
                {task.title}
              </h2>
              {isViewMode && (
                <div className="flex items-center gap-2 mt-2">
                  <div className="w-2 h-2 rounded-full bg-emerald-500 animate-pulse" />
                  <span className="text-[10px] font-black uppercase tracking-widest text-emerald-600">COMPLETED WORK</span>
                </div>
              )}
            </div>
          </div>

          <Button
            type="button"
            variant="ghost"
            size="icon-lg"
            onClick={onClose}
            className="rounded-2xl text-zinc-300 hover:bg-zinc-100 hover:text-darkDelegation"
            aria-label="Close"
          >
            <X size={24} />
          </Button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto [scrollbar-width:none] px-6 pb-6">
          <div className="flex gap-6">
            {/* Main content */}
            <div className={`flex-1 space-y-10 ${task.revisions.length > 0 ? 'border-r border-zinc-100 pr-6' : ''}`}>
              {/* Draft / Result Output */}
              <section className="space-y-4">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <div className="w-1 h-4 bg-darkDelegation rounded-full" />
                    <h3 className="text-[10px] font-black text-zinc-400 uppercase tracking-widest">
                      {selectedRevisionIndex !== null
                        ? `Revision V${selectedRevisionIndex + 1}`
                        : isPlanApprovalMode
                          ? 'Proposed task'
                          : isViewMode
                            ? 'Final Output'
                            : 'Current Proposal'}
                    </h3>
                  </div>
                  {selectedRevisionIndex !== null && (
                    <Button
                      type="button"
                      variant="ghost"
                      onClick={() => setSelectedRevisionIndex(null)}
                      className="h-auto px-0 py-0 text-[9px] font-black uppercase text-zinc-400 hover:bg-transparent hover:text-darkDelegation"
                    >
                      Back to latest
                    </Button>
                  )}
                </div>
                <div className="p-8 bg-zinc-50/50 rounded-3xl border border-zinc-100 min-h-[350px] shadow-inner">
                  <div className="markdown-content text-darkDelegation text-sm leading-relaxed">
                    <ReactMarkdown remarkPlugins={[remarkGfm]}>
                      {selectedRevisionIndex !== null
                        ? task.revisions[selectedRevisionIndex].output
                        : isPlanApprovalMode
                          ? task.description || '_No description._'
                          : isViewMode
                            ? task.output || task.draftOutput || ''
                            : task.draftOutput || 'No content produced.'}
                    </ReactMarkdown>
                  </div>
                </div>
              </section>

            </div>

            {/* Revision Sidebar (Creativa) */}
            {(task.revisions?.length ?? 0) > 0 && (
              <div className="w-56 shrink-0 flex flex-col pt-4 animate-in fade-in slide-in-from-right-4 duration-700">
                <div className="flex items-center gap-2 mb-6" style={{ color: USER_COLOR }}>
                  <div className="w-2 h-2 rounded-full animate-pulse" style={{ backgroundColor: USER_COLOR }} />
                  <span className="text-[10px] font-black uppercase tracking-widest">Version History</span>
                  <InfoBubble text="View previous iterations of this task. You can see how the work evolved or revert to a stronger version." />
                </div>
                <div className="space-y-2 overflow-y-auto pr-2 max-h-[60vh] [scrollbar-width:none]">
                  {task.revisions.map((rev, idx) => (
                    <Button
                      key={idx}
                      type="button"
                      variant="ghost"
                      onClick={() => setSelectedRevisionIndex(idx)}
                      className={`
                        group/rev h-auto w-full justify-start whitespace-normal rounded-xl border p-2.5 text-left transition-all
                        ${selectedRevisionIndex === idx
                          ? 'border-darkDelegation bg-darkDelegation text-white shadow-xl hover:bg-darkDelegation hover:text-white'
                          : 'border-zinc-100 bg-white text-darkDelegation hover:border-zinc-300 hover:bg-zinc-50'}
                      `}
                    >
                      <div className="flex items-center justify-between mb-1">
                        <span className={`text-[10px] font-black uppercase tracking-widest ${selectedRevisionIndex === idx ? 'text-zinc-400' : 'text-zinc-400'}`}>Version {idx + 1}</span>
                        <span className="text-[9px] font-bold opacity-50 uppercase">{new Date(rev.timestamp).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
                      </div>
                      {rev.feedback && (
                        <p className={`text-[12px] leading-tight line-clamp-2 italic ${selectedRevisionIndex === idx ? 'text-zinc-300' : 'text-zinc-500'}`}>
                          "{rev.feedback}"
                        </p>
                      )}
                    </Button>
                  ))}
                  {/* Current one indicator */}
                  {!isViewMode && (
                    <Button
                      type="button"
                      variant="ghost"
                      onClick={() => setSelectedRevisionIndex(null)}
                      className={`
                        mt-2 flex h-auto w-full flex-col gap-1 whitespace-normal rounded-xl border p-3 text-left transition-all
                        ${selectedRevisionIndex === null
                          ? 'border-emerald-100 bg-emerald-50 hover:bg-emerald-50'
                          : 'border-zinc-100 bg-white hover:border-emerald-200 hover:bg-white'}
                      `}
                    >
                      <span className="text-[12px] font-black uppercase tracking-widest text-emerald-600">Active Review</span>
                      <p className="text-[12px] text-emerald-400 font-medium leading-none">In review process...</p>
                    </Button>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>

        {/* Feedback: draft review (reject) or plan approval (amend & replan) */}
        {!isViewMode && selectedRevisionIndex === null && (
          <div className="px-8 py-4 border-t border-zinc-100 bg-white">
            <div className="flex items-center gap-2 mb-2 text-zinc-400">
              <div className="flex items-center gap-2">
                <GitPullRequest size={12} />
                <span className="text-[9px] font-black uppercase tracking-widest">
                  {isPlanApprovalMode ? 'Amendments & direction' : 'Your Feedback'}
                </span>
              </div>
              <InfoBubble
                text={
                  isPlanApprovalMode
                    ? 'Optional for approve/remove. For amend: describe scope changes, splits, or constraints. The lead removes this card and proposes revised task(s); you can require approval again on each.'
                    : 'Provide specific instructions for what to change. The agent will read this and attempt a new version.'
                }
              />
            </div>
            <Textarea
              value={feedback}
              onChange={(e) => setFeedback(e.target.value)}
              placeholder={
                isPlanApprovalMode
                  ? 'e.g. Add unit tests, split UI vs data layer, narrow scope to MVP…'
                  : 'Describe what needs to be changed before rejecting...'
              }
              className="h-20 resize-none rounded-xl border-zinc-100 bg-zinc-50 p-3 text-[13px] placeholder:italic placeholder:text-zinc-300 focus-visible:ring-darkDelegation/10"
            />
          </div>
        )}

        {/* Footer Actions */}
        <div className="flex shrink-0 flex-wrap items-center justify-end gap-4 border-t border-zinc-100 bg-zinc-50/50 px-8 py-8">
          {isPlanApprovalMode ? (
            <>
              <Button
                type="button"
                variant="outline"
                onClick={handleRemoveProposedTask}
                className="h-12 gap-2 rounded-2xl border-zinc-200 bg-white px-8 text-[10px] font-black uppercase tracking-widest text-zinc-600 shadow-sm hover:bg-zinc-100 active:scale-95"
              >
                <AlertCircle size={14} strokeWidth={3} />
                Remove from board
              </Button>
              <Button
                type="button"
                variant="outline"
                onClick={handleAmendPlan}
                disabled={!feedback.trim()}
                className="h-12 gap-2 rounded-2xl border-zinc-200 bg-white px-8 text-[10px] font-black uppercase tracking-widest text-zinc-600 shadow-sm hover:bg-zinc-100 enabled:active:scale-95 disabled:cursor-not-allowed disabled:opacity-50 disabled:text-zinc-300"
              >
                <PencilLine size={14} strokeWidth={3} />
                Amend & replan
              </Button>
              <Button
                type="button"
                onClick={handleApprovePlan}
                className="h-12 gap-2 rounded-2xl bg-darkDelegation px-10 text-[10px] font-black uppercase tracking-widest text-white shadow-xl shadow-darkDelegation/10 hover:bg-black active:scale-95"
              >
                <CheckCircle2 size={14} strokeWidth={3} />
                Approve & queue work
              </Button>
            </>
          ) : (isViewMode || selectedRevisionIndex !== null) ? (
            <Button
              type="button"
              onClick={selectedRevisionIndex !== null ? () => setSelectedRevisionIndex(null) : onClose}
              className="h-12 gap-2 rounded-2xl bg-darkDelegation px-10 text-[10px] font-black uppercase tracking-widest text-white shadow-xl shadow-darkDelegation/10 hover:bg-black active:scale-95"
            >
              {selectedRevisionIndex !== null ? 'Show Active Review' : 'Close Viewer'}
            </Button>
          ) : (
            <>
              <Button
                type="button"
                variant="outline"
                onClick={handleReject}
                disabled={!feedback.trim()}
                className="h-12 gap-2 rounded-2xl border-zinc-200 bg-white px-8 text-[10px] font-black uppercase tracking-widest shadow-sm hover:bg-zinc-100 enabled:text-zinc-600 enabled:active:scale-95 disabled:cursor-not-allowed disabled:opacity-50 disabled:text-zinc-200"
              >
                <AlertCircle size={14} strokeWidth={3} />
                Reject with Feedback
              </Button>

              <Button
                type="button"
                onClick={handleApprove}
                className="h-12 gap-2 rounded-2xl bg-darkDelegation px-10 text-[10px] font-black uppercase tracking-widest text-white shadow-xl shadow-darkDelegation/10 hover:bg-black active:scale-95"
              >
                <CheckCircle2 size={14} strokeWidth={3} />
                Approve Task
              </Button>
            </>
          )}
        </div>
      </div>
    </ModalRoot>
  );
};
