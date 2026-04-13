import { ExternalLink } from 'lucide-react';
import React, { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import type { AgentNode, AgenticSystem } from '../data/agents';
import {
  getCachedProjectId,
  hasRemoteProjectBackend,
  listRemoteProjects,
  patchRemoteProjectMeta,
} from '../integration/api/projectRemoteApi';
import { showToast } from '../integration/store/toastStore';
import { useTeamStore } from '../integration/store/teamStore';
import { useSceneManager } from '../simulation/SceneContext';
import { ModalBackdrop, ModalPanel, ModalRoot } from './components/ModalChrome';

function patchLeadTree(
  node: AgentNode,
  targetId: string,
  patch: Partial<Pick<AgentNode, 'name' | 'description'>>,
): AgentNode {
  if (node.id === targetId) return { ...node, ...patch };
  if (!node.subagents?.length) return node;
  return {
    ...node,
    subagents: node.subagents.map((c) => patchLeadTree(c, targetId, patch)),
  };
}

function RoleStageFields({
  node,
  depth,
  stageIndex,
  onPatch,
}: {
  node: AgentNode;
  depth: number;
  stageIndex: number;
  onPatch: (id: string, field: 'name' | 'description', value: string) => void;
}): React.ReactElement {
  const label =
    depth === 0
      ? 'Lead (orchestrator)'
      : depth === 1
        ? `Stage ${stageIndex}`
        : `Nested role ${stageIndex}`;

  return (
    <div className={depth > 0 ? 'mt-4 border-l-2 border-teal-100 pl-3' : ''}>
      <p className="text-[9px] font-black uppercase tracking-widest text-zinc-400 mb-2">{label}</p>
      <label className="block text-[10px] font-bold text-zinc-500 mb-1">Name</label>
      <Input
        value={node.name}
        onChange={(e) => onPatch(node.id, 'name', e.target.value)}
        className="mb-2 rounded-xl border-zinc-200 text-sm"
        maxLength={120}
      />
      <label className="block text-[10px] font-bold text-zinc-500 mb-1">What they own</label>
      <Textarea
        value={node.description}
        onChange={(e) => onPatch(node.id, 'description', e.target.value)}
        className="min-h-[72px] rounded-xl border-zinc-200 text-sm resize-y"
        maxLength={2000}
      />
      {node.subagents?.map((c, i) => (
        <React.Fragment key={c.id}>
          <RoleStageFields node={c} depth={depth + 1} stageIndex={i + 1} onPatch={onPatch} />
        </React.Fragment>
      ))}
    </div>
  );
}

export interface TeamTemplateReviewModalProps {
  isOpen: boolean;
  teamId: string | null;
  headlineName?: string;
  onClose: () => void;
}

const TeamTemplateReviewModal: React.FC<TeamTemplateReviewModalProps> = ({
  isOpen,
  teamId,
  headlineName,
  onClose,
}) => {
  const navigate = useNavigate();
  const scene = useSceneManager();
  const customSystems = useTeamStore((s) => s.customSystems);
  const saveCustomSystem = useTeamStore((s) => s.saveCustomSystem);
  const setActiveTeam = useTeamStore((s) => s.setActiveTeam);

  const [draft, setDraft] = useState<AgenticSystem | null>(null);
  const [projectTitle, setProjectTitle] = useState('');
  const [metaLoading, setMetaLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [switchAfter, setSwitchAfter] = useState(true);

  useEffect(() => {
    if (!isOpen || !teamId) {
      setDraft(null);
      return;
    }
    const raw = customSystems.find((s) => s.id === teamId);
    if (!raw) {
      showToast('That team template was not found in your saved teams.', 'error');
      onClose();
      return;
    }
    setDraft(JSON.parse(JSON.stringify(raw)) as AgenticSystem);
    setProjectTitle('');
    setSwitchAfter(true);
  }, [isOpen, teamId, onClose]);

  useEffect(() => {
    if (!isOpen || !hasRemoteProjectBackend()) return;
    const pid = getCachedProjectId();
    if (!pid) return;
    let cancelled = false;
    setMetaLoading(true);
    void (async () => {
      try {
        const { projects } = await listRemoteProjects(100, 0);
        const row = projects.find((p) => p.id === pid);
        if (!cancelled && row) setProjectTitle(row.meta.title);
      } catch {
        /* keep editable blank */
      } finally {
        if (!cancelled) setMetaLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [isOpen]);

  const patchNode = useCallback((id: string, field: 'name' | 'description', value: string) => {
    setDraft((d) => {
      if (!d) return d;
      return { ...d, leadAgent: patchLeadTree(d.leadAgent, id, { [field]: value }) };
    });
  }, []);

  const setTeamRootField = useCallback((field: 'teamName' | 'teamType' | 'teamDescription', value: string) => {
    setDraft((d) => (d ? { ...d, [field]: value } : d));
  }, []);

  const validate = useCallback((): string | null => {
    if (!draft) return 'Nothing to save.';
    if (!draft.teamName.trim()) return 'Team name is required.';
    if (!draft.leadAgent.name.trim()) return 'Lead name is required.';
    return null;
  }, [draft]);

  const persist = useCallback(async () => {
    const err = validate();
    if (err) {
      showToast(err, 'error');
      return;
    }
    if (!draft || !teamId) return;

    const normalized: AgenticSystem = {
      ...draft,
      teamName: draft.teamName.trim(),
      teamType: draft.teamType.trim(),
      teamDescription: draft.teamDescription.trim(),
      leadAgent: {
        ...draft.leadAgent,
        name: draft.leadAgent.name.trim(),
        description: draft.leadAgent.description.trim(),
      },
    };

    setSaving(true);
    try {
      saveCustomSystem(normalized);

      if (hasRemoteProjectBackend()) {
        const pid = getCachedProjectId();
        const t = projectTitle.trim();
        if (pid && t) await patchRemoteProjectMeta(pid, t);
      }

      if (switchAfter) {
        setActiveTeam(teamId);
        scene?.resetScene();
        showToast(`Now using team "${normalized.teamName}".`, 'success');
        navigate('/');
      } else {
        showToast('Team template and project name saved.', 'success');
      }
      onClose();
    } catch (e) {
      showToast(e instanceof Error ? e.message : 'Could not save.', 'error');
    } finally {
      setSaving(false);
    }
  }, [
    draft,
    teamId,
    validate,
    saveCustomSystem,
    projectTitle,
    switchAfter,
    setActiveTeam,
    scene,
    navigate,
    onClose,
  ]);

  if (!isOpen || !teamId || !draft) return null;

  const serverProjects = hasRemoteProjectBackend();

  return (
    <ModalRoot layer="modalAlert" paddingClassName="p-4 sm:p-6">
      <ModalBackdrop tone="dim" onRequestClose={() => !saving && onClose()} />
      <ModalPanel
        maxWidthClass="max-w-lg"
        className="max-h-[min(92vh,880px)] flex flex-col rounded-3xl border-zinc-200 shadow-2xl"
      >
        <div className="overflow-y-auto flex-1 px-6 pt-6 pb-4 [scrollbar-width:thin]">
          <h3 className="text-lg font-black text-darkDelegation leading-tight pr-8">
            Review setup
          </h3>
          <p className="text-xs text-zinc-500 mt-1">
            {headlineName
              ? `Template “${headlineName}” — adjust the project name and team before you start.`
              : 'Adjust the project name and team roster, then save.'}
          </p>

          {serverProjects && (
            <div className="mt-5 rounded-2xl border border-zinc-100 bg-zinc-50/80 p-4">
              <label className="block text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-2">
                Project name
              </label>
              <Input
                value={projectTitle}
                onChange={(e) => setProjectTitle(e.target.value)}
                placeholder={metaLoading ? 'Loading…' : 'Name this project'}
                disabled={metaLoading}
                className="rounded-xl border-zinc-200 text-sm"
                maxLength={200}
              />
              <p className="text-[10px] text-zinc-400 mt-2 leading-snug">
                Shown in the project list. The Consultant can still suggest changes in chat.
              </p>
            </div>
          )}

          <div className="mt-5 rounded-2xl border border-zinc-100 bg-white p-4 shadow-sm">
            <p className="text-[10px] font-black uppercase tracking-widest text-teal-800 mb-3">
              Team
            </p>
            <label className="block text-[10px] font-bold text-zinc-500 mb-1">Team name</label>
            <Input
              value={draft.teamName}
              onChange={(e) => setTeamRootField('teamName', e.target.value)}
              className="mb-3 rounded-xl border-zinc-200 text-sm"
              maxLength={120}
            />
            <label className="block text-[10px] font-bold text-zinc-500 mb-1">Type / label</label>
            <Input
              value={draft.teamType}
              onChange={(e) => setTeamRootField('teamType', e.target.value)}
              className="mb-3 rounded-xl border-zinc-200 text-sm"
              maxLength={80}
            />
            <label className="block text-[10px] font-bold text-zinc-500 mb-1">Overview</label>
            <Textarea
              value={draft.teamDescription}
              onChange={(e) => setTeamRootField('teamDescription', e.target.value)}
              className="min-h-[64px] rounded-xl border-zinc-200 text-sm resize-y mb-4"
              maxLength={2000}
            />

            <p className="text-[10px] font-black uppercase tracking-widest text-zinc-400 mb-2">
              Roles & stages
            </p>
            <RoleStageFields node={draft.leadAgent} depth={0} stageIndex={0} onPatch={patchNode} />
          </div>

          <label className="mt-4 flex cursor-pointer items-start gap-2 text-xs text-zinc-600">
            <input
              type="checkbox"
              className="mt-0.5 rounded border-zinc-300"
              checked={switchAfter}
              onChange={(e) => setSwitchAfter(e.target.checked)}
            />
            <span>
              After saving, switch the simulation to this team (recommended to start the handoff brief).
            </span>
          </label>

          <button
            type="button"
            className="mt-3 flex items-center gap-1.5 text-[11px] font-semibold text-indigo-600 hover:text-indigo-800"
            onClick={() => navigate(`/teams?focusTeam=${encodeURIComponent(teamId)}`)}
          >
            <ExternalLink size={12} />
            Open full visual editor (Teams page)
          </button>
        </div>

        <div className="flex flex-col gap-2 border-t border-zinc-100 bg-zinc-50/90 px-6 py-4 shrink-0">
          <Button
            type="button"
            disabled={saving}
            onClick={() => void persist()}
            className="w-full rounded-2xl bg-darkDelegation py-3 text-[10px] font-black uppercase tracking-widest text-white hover:bg-black disabled:opacity-50"
          >
            {saving
              ? 'Saving…'
              : switchAfter
                ? 'Save & use this team'
                : 'Save changes'}
          </Button>
          <Button
            type="button"
            variant="secondary"
            disabled={saving}
            onClick={onClose}
            className="w-full rounded-2xl py-3 text-[10px] font-black uppercase tracking-widest"
          >
            Cancel
          </Button>
        </div>
      </ModalPanel>
    </ModalRoot>
  );
};

export default TeamTemplateReviewModal;
