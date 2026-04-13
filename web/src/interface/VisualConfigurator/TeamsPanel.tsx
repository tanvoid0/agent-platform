import { Plus } from 'lucide-react';
import React, { useMemo } from 'react';
import { Button } from '@/components/ui/button';
import { AGENTIC_SETS, AgenticSystem, CONSULTANT_WORKSHOP_TEAM_ID, DEFAULT_AGENT_CHAT_MODEL } from '../../data/agents';
import { DEFAULT_MODELS } from '../../core/llm/constants';
import { useTeamStore } from '../../integration/store/teamStore';
import { createClientId } from '../utils/createClientId';
import { TeamCard } from './TeamCard';

interface TeamsPanelProps {
  onSelectTeam: (id: string) => void;
  selectedTeamId: string;
  onModeChange: (mode: 'view' | 'edit') => void;
  mode: 'view' | 'edit';
}

export const TeamsPanel: React.FC<TeamsPanelProps> = ({ onSelectTeam, selectedTeamId, onModeChange, mode }) => {
  const { customSystems, selectedAgentSetId, saveCustomSystem } = useTeamStore();

  const allSystems = useMemo(() => {
    const combined = [...customSystems, ...AGENTIC_SETS];
    const unique = combined.filter((sys, index, self) =>
      index === self.findIndex((s) => s.id === sys.id)
    );
    unique.sort((a, b) => {
      if (a.id === CONSULTANT_WORKSHOP_TEAM_ID) return -1;
      if (b.id === CONSULTANT_WORKSHOP_TEAM_ID) return 1;
      return 0;
    });
    return unique;
  }, [customSystems]);

  const handleCreateNew = () => {
    const newId = createClientId('team');
    const newSystem: AgenticSystem = {
      id: newId,
      teamName: 'New Team',
      teamType: 'Custom',
      teamDescription: 'A custom agentic team.',
      color: '#A855F7',
      outputType: 'text',
      outputModel: DEFAULT_MODELS.text,
      user: {
        index: 0,
        model: 'Human',
        position: { x: 0, y: 0 }
      },
      leadAgent: {
        id: createClientId('agent'),
        index: 1,
        name: 'Lead Agent',
        description: 'Coordinate the team to finish the project.',
        color: '#A855F7',
        model: DEFAULT_AGENT_CHAT_MODEL,
        position: { x: 0, y: 150 },
        subagents: []
      }
    };
    saveCustomSystem(newSystem);
    onSelectTeam(newId);
    onModeChange('edit');
  };

  return (
    <div className="w-96 border-l border-zinc-100 bg-white flex flex-col h-full shrink-0">
      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {allSystems.map((system) => {
          const isSelected = selectedTeamId === system.id;
          const isActive = selectedAgentSetId === system.id;
          const canDelete = customSystems.some((cs) => cs.id === system.id);

          return (
            <TeamCard
              key={system.id}
              system={system}
              isSelected={isSelected}
              isActive={isActive}
              canDelete={canDelete}
              mode={mode}
              onSelectTeam={onSelectTeam}
              onModeChange={onModeChange}
            />
          );
        })}
      </div>
      <div className="p-4 border-t border-zinc-50 bg-white">
        <Button
          type="button"
          onClick={handleCreateNew}
          className="flex w-full items-center justify-center gap-2 rounded-xl bg-darkDelegation py-3 text-[10px] font-black uppercase tracking-[0.1em] text-white shadow-lg shadow-black/5 hover:bg-darkDelegation active:scale-[0.98]"
        >
          <Plus size={14} strokeWidth={3} />
          Create New Team
        </Button>
      </div>
    </div>
  );
};
