import { Edit2, Pipette, Trash2, Users, X } from 'lucide-react';
import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { AgenticSystem, DEFAULT_AGENTIC_SET_ID, getAllAgents } from '../../data/agents';
import { defaultOutputModelForType, getOutputModelPickerOptions } from '../../core/llm/llmFacade';
import { USER_COLOR } from '../../theme/brand';
import { useCoreStore } from '../../integration/store/coreStore';
import { useTeamStore } from '../../integration/store/teamStore';
import { useSceneManager } from '../../simulation/SceneContext';
import { getBrightness, getDarkenedColor } from './colorUtils';
import { ColorPicker } from './ColorPicker';
import ConfirmModal from '../components/ConfirmModal';
import { InfoBubble } from '../components/InfoBubble';
import { TeamOutputBadge } from '../components/TeamOutputBadge';

interface TeamCardProps {
  system: AgenticSystem;
  isSelected: boolean;
  isActive: boolean;
  canDelete: boolean;
  mode: 'view' | 'edit';
  onSelectTeam: (id: string) => void;
  onModeChange: (mode: 'view' | 'edit') => void;
}

export const TeamCard: React.FC<TeamCardProps> = ({
  system,
  isSelected,
  isActive,
  canDelete,
  mode,
  onSelectTeam,
  onModeChange,
}) => {
  const { setActiveTeam, updateSystem, deleteCustomSystem, selectedAgentSetId } = useTeamStore();
  const scene = useSceneManager();
  const colorInputRef = useRef<HTMLInputElement>(null);

  const [localEditData, setLocalEditData] = useState<Partial<AgenticSystem>>({});
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [showSwitchConfirm, setShowSwitchConfirm] = useState(false);
  const [hasColorSuggestion, setHasColorSuggestion] = useState(false);

  const isEditing = mode === 'edit' && isSelected;
  const agentCount = useMemo(() => getAllAgents(system).length, [system]);

  useEffect(() => {
    if (isEditing) {
      setLocalEditData({
        teamName: system.teamName || '',
        teamType: system.teamType || '',
        teamDescription: system.teamDescription || 'A custom agentic team.',
        color: system.color || '#A855F7',
        outputType: system.outputType || 'text',
        outputModel: system.outputModel || defaultOutputModelForType(system.outputType || 'text'),
        outputAutoApprove: system.outputAutoApprove !== undefined ? system.outputAutoApprove : (system.outputType === 'text')
      });
      setErrorMsg(null);
      setShowDeleteConfirm(false);
      setShowSwitchConfirm(false);
      setHasColorSuggestion(false);
    } else {
      setErrorMsg(null);
      setShowDeleteConfirm(false);
      setShowSwitchConfirm(false);
      setHasColorSuggestion(false);
    }
  }, [isEditing, system]);

  useEffect(() => {
    if (!isSelected || isEditing) setShowSwitchConfirm(false);
  }, [isSelected, isEditing]);

  const hasUnsavedChanges = useMemo(() => {
    return localEditData.teamName !== (system.teamName || '') ||
      localEditData.teamType !== (system.teamType || '') ||
      localEditData.teamDescription !== (system.teamDescription || '') ||
      localEditData.color !== (system.color || '#A855F7') ||
      localEditData.outputType !== (system.outputType || 'text') ||
      localEditData.outputModel !==
        (system.outputModel || defaultOutputModelForType(system.outputType || 'text')) ||
      localEditData.outputAutoApprove !== (system.outputAutoApprove);
  }, [localEditData, system]);

  const isFormValid = !!(localEditData.teamName?.trim() &&
    localEditData.teamType?.trim() &&
    localEditData.teamDescription?.trim() &&
    !hasColorSuggestion);

  const handleSwitch = (e: React.MouseEvent) => {
    e.stopPropagation();
    const { phase, tasks, userBrief } = useCoreStore.getState();
    const hasBoardWork =
      phase !== 'idle' || tasks.length > 0 || (userBrief?.trim().length ?? 0) > 0;
    if (hasBoardWork) {
      setShowSwitchConfirm(true);
      return;
    }
    scene?.resetScene();
    setActiveTeam(system.id);
  };

  const performTeamSwitch = () => {
    scene?.resetScene();
    setActiveTeam(system.id);
    setShowSwitchConfirm(false);
  };

  const handleSave = (e?: React.MouseEvent) => {
    e?.stopPropagation();
    if (!isFormValid) {
      setErrorMsg(
        canDelete
          ? 'Please fill Name, Type and Description or delete the team.'
          : 'Please fill Name, Type and Description to save.',
      );
      return;
    }
    updateSystem(system.id, localEditData);
    onModeChange('view');
  };

  const handleCloseEdit = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!isFormValid) {
      const colorMsg = 'Please choose a darker color (use suggestion or pick another) before saving.';
      const fillDiscardMsg =
        'Please fill Name, Type and Description to continue, or close again to discard.';
      if (!canDelete) {
        if (hasColorSuggestion) {
          if (errorMsg === colorMsg) {
            onModeChange('view');
            return;
          }
          setErrorMsg(colorMsg);
        } else {
          if (errorMsg === fillDiscardMsg) {
            onModeChange('view');
            return;
          }
          setErrorMsg(fillDiscardMsg);
        }
        return;
      }
      if (hasColorSuggestion) {
        setErrorMsg(colorMsg);
      } else {
        setErrorMsg('Please fill Name, Type and Description or delete the team.');
      }
      return;
    }
    if (hasUnsavedChanges) {
      setErrorMsg('Unsaved changes will be lost. Save or close again to discard.');
      if (errorMsg?.includes('Unsaved changes')) {
        onModeChange('view');
      }
      return;
    }
    onModeChange('view');
  };

  const handleDelete = (e: React.MouseEvent) => {
    e.stopPropagation();
    setShowDeleteConfirm(true);
    setErrorMsg('Delete this team?');
  };

  const confirmDelete = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (system.id === selectedAgentSetId) {
      scene?.resetScene();
      setActiveTeam(DEFAULT_AGENTIC_SET_ID);
    }
    deleteCustomSystem(system.id);
    onModeChange('view');
  };

  const handleColorChange = (newColor: string) => {
    setLocalEditData(prev => ({ ...prev, color: newColor }));

    // Check if new color is too light to update form validity status
    const brightness = getBrightness(newColor);
    setHasColorSuggestion(brightness > 180);
    setErrorMsg(null);
  };

  return (
    <>
    <div
      onClick={() => onSelectTeam(system.id)}
      className={`group relative p-3.5 rounded-2xl transition-all cursor-pointer border-[3px] ${isSelected ? 'bg-zinc-50/50 shadow-sm' : 'bg-white hover:border-zinc-200/50'
        }`}
      style={{
        borderColor: isSelected
          ? system.color
          : (isActive ? `${system.color}50` : 'transparent')
      }}
    >
      {isEditing && (
        <div className="mb-3">
          <div className="flex items-center justify-between pb-2 mb-2 border-b border-zinc-100">
            <div className="flex items-center gap-2">
              <h3 className="text-[9px] font-black uppercase tracking-[0.1em] text-darkDelegation">Edit Team</h3>
            </div>
            <Button type="button" variant="ghost" size="icon-xs" onClick={handleCloseEdit} className="text-zinc-400 hover:bg-zinc-200">
              <X size={14} strokeWidth={3} />
            </Button>
          </div>
          {errorMsg && (
            <div className="flex items-center justify-between gap-2 p-2 bg-red-50 border border-red-100 rounded-xl mb-2">
              <p className="text-[9px] font-bold text-red-600 leading-tight uppercase tracking-tight">
                {errorMsg}
              </p>
              {showDeleteConfirm && (
                <div className="flex items-center gap-1 shrink-0">
                  <Button type="button" variant="outline" onClick={(e) => { e.stopPropagation(); setShowDeleteConfirm(false); setErrorMsg(null); }} className="h-auto rounded-md border-red-100 bg-white px-2 py-0.5 text-[8px] font-black uppercase tracking-wider text-red-400">Cancel</Button>
                  <Button type="button" onClick={confirmDelete} className="h-auto rounded-md bg-red-500 px-2 py-0.5 text-[8px] font-black uppercase tracking-wider text-white hover:bg-red-600">OK</Button>
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {isSelected && !isEditing && (
        <Button
          type="button"
          variant="secondary"
          onClick={(e) => { e.stopPropagation(); onModeChange('edit'); }}
          className="absolute top-3.5 right-3.5 z-10 flex items-center gap-1.5 rounded-xl bg-zinc-100 px-3 py-1.5 text-[9px] font-black uppercase tracking-widest text-darkDelegation opacity-0 transition-all hover:bg-zinc-200 group-hover:opacity-100"
        >
          <Edit2 size={12} strokeWidth={2.5} />
          Edit Team
        </Button>
      )}

      <div className="flex flex-col">
        {/* Header Row: Badge + Name/Type */}
        <div className="flex items-start gap-3.5 mb-3">
          {!isEditing && (
            <div className="relative shrink-0">
              <div
                className="h-9 px-3 rounded-xl flex items-center justify-center gap-2 shadow-lg shadow-black/5"
                style={{ backgroundColor: system.color }}
              >
                <Users size={14} className="text-white opacity-90" strokeWidth={3} />
                <span className="text-xs font-black text-white leading-none">
                  {agentCount}
                </span>
              </div>
            </div>
          )}

          <div className="flex-1 min-w-0 flex flex-col">
            {isEditing ? (
              <div className="space-y-1 mb-2">
                <label className="text-[7px] font-black uppercase text-zinc-400 ml-1">Team Color</label>
                <div className="px-1">
                  <ColorPicker
                    color={localEditData.color || '#A855F7'}
                    onChange={handleColorChange}
                  />
                </div>
              </div>
            ) : (
              <div className="space-y-0.5">
                <h4 className={`text-[11px] font-black leading-tight uppercase tracking-wider truncate mb-0.5 ${system.teamName ? 'text-darkDelegation' : 'text-zinc-300'}`}>{system.teamName || 'Untitled Team'}</h4>
                <p className={`text-[9px] font-bold uppercase tracking-[0.1em] ${system.teamType ? 'text-zinc-400' : 'text-zinc-200'}`}>{system.teamType || 'Unspecified Type'}</p>
              </div>
            )}
          </div>
        </div>

        {/* Body Content: Spans full width */}
        <div className="flex flex-col flex-1 min-w-0">
          {isEditing ? (
            <div className="space-y-2 mb-3" onClick={(e) => e.stopPropagation()}>
              <div className="space-y-1">
                <label className="text-[7px] font-black uppercase text-zinc-400 ml-1">Team Name</label>
                <Input
                  value={localEditData.teamName || ''}
                  onChange={(e) => { setLocalEditData(prev => ({ ...prev, teamName: e.target.value })); setErrorMsg(null); }}
                  className="h-auto w-full rounded-xl border-zinc-100 bg-white px-2.5 py-1.5 text-[13px] font-medium"
                  style={{ '--tw-focus-border-color': USER_COLOR } as React.CSSProperties}
                  onFocus={(e) => e.target.style.borderColor = USER_COLOR}
                  onBlur={(e) => e.target.style.borderColor = '#f4f4f5'}
                />
              </div>
              <div className="space-y-1">
                <label className="text-[7px] font-black uppercase text-zinc-400 ml-1">Team Type</label>
                <Input
                  value={localEditData.teamType || ''}
                  onChange={(e) => { setLocalEditData(prev => ({ ...prev, teamType: e.target.value })); setErrorMsg(null); }}
                  className="h-auto w-full rounded-xl border-zinc-100 bg-white px-2.5 py-1.5 text-[13px] font-medium"
                  onFocus={(e) => e.target.style.borderColor = USER_COLOR}
                  onBlur={(e) => e.target.style.borderColor = '#f4f4f5'}
                />
              </div>
              <div className="space-y-1">
                <label className="text-[7px] font-black uppercase text-zinc-400 ml-1">Description</label>
                <Textarea
                  value={localEditData.teamDescription || ''}
                  onChange={(e) => { setLocalEditData(prev => ({ ...prev, teamDescription: e.target.value })); setErrorMsg(null); }}
                  className="h-20 w-full resize-none rounded-xl border-zinc-100 bg-white p-2.5 text-[13px] font-medium leading-snug"
                  onFocus={(e) => e.target.style.borderColor = USER_COLOR}
                  onBlur={(e) => e.target.style.borderColor = '#f4f4f5'}
                />
              </div>
              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <label className="text-[7px] font-black uppercase text-zinc-400 ml-1">Output Type</label>
                  <select
                    value={localEditData.outputType || 'text'}
                    onChange={(e) => {
                      const newType = e.target.value as 'text' | 'image' | 'music' | 'video';
                      setLocalEditData((prev) => ({
                        ...prev,
                        outputType: newType,
                        outputModel: defaultOutputModelForType(newType),
                        outputAutoApprove: newType === 'text',
                      }));
                    }}
                    className="w-full bg-white border border-zinc-100 text-[11px] font-bold rounded-xl px-2.5 py-1.5 outline-none cursor-pointer"
                  >
                    <option value="text">TEXT</option>
                    <option value="image">IMAGE</option>
                    <option value="music">MUSIC</option>
                    <option value="video">VIDEO</option>
                  </select>
                </div>
                <div className="space-y-1">
                  <label className="text-[7px] font-black uppercase text-zinc-400 ml-1">Output Model</label>
                  <select
                    value={
                      localEditData.outputModel ||
                      defaultOutputModelForType(localEditData.outputType || 'text')
                    }
                    onChange={(e) => setLocalEditData(prev => ({ ...prev, outputModel: e.target.value }))}
                    className="w-full bg-white border border-zinc-100 text-[10px] font-bold rounded-xl px-2.5 py-1.5 outline-none cursor-pointer lowercase"
                  >
                    {(() => {
                      const ot = localEditData.outputType || 'text';
                      const opts = getOutputModelPickerOptions(ot);
                      const v =
                        localEditData.outputModel || defaultOutputModelForType(ot);
                      return (
                        <>
                          {!opts.includes(v) && (
                            <option key="__stored" value={v}>{v}</option>
                          )}
                          {opts.map((model) => (
                            <option key={model} value={model}>{model}</option>
                          ))}
                        </>
                      );
                    })()}
                  </select>
                </div>
              </div>

              <div className="flex items-center justify-between p-2.5 bg-zinc-50 border border-zinc-100/50 rounded-xl mt-0.5">
                <div className="flex flex-col">
                  <div className="flex items-center gap-1">
                    <span className="text-[8px] font-black uppercase text-darkDelegation tracking-wider">Auto-Approve Output</span>
                    <InfoBubble text="When enabled, the team will generate the final asset immediately after finishing all tasks without waiting for your review." />
                  </div>
                  <span className="text-[7px] text-zinc-400 font-bold leading-tight">Generate asset without review</span>
                </div>
                <button
                  type="button"
                  onClick={() => setLocalEditData(prev => ({ ...prev, outputAutoApprove: !prev.outputAutoApprove }))}
                  className={`relative h-4 w-8 shrink-0 rounded-full transition-all ${localEditData.outputAutoApprove !== false ? 'bg-darkDelegation shadow-[0_0_8px_rgba(0,0,0,0.15)]' : 'bg-zinc-200'}`}
                  aria-pressed={localEditData.outputAutoApprove !== false}
                >
                  <div className={`absolute top-0.5 size-3 rounded-full bg-white transition-all ${localEditData.outputAutoApprove !== false ? 'left-[16px]' : 'left-[4px]'}`} />
                </button>
              </div>

              <Button type="button" onClick={handleSave} disabled={!isFormValid} className={`mt-1 w-full rounded-xl py-2.5 text-[10px] font-black uppercase tracking-[0.1em] shadow-lg ${isFormValid ? 'bg-darkDelegation text-white shadow-black/10' : 'cursor-not-allowed bg-zinc-50 text-zinc-300 shadow-none'}`}>Save Changes</Button>
            </div>
          ) : (
            <div className="space-y-0.5 mb-2.5 px-2">
              <TeamOutputBadge system={system} className="mt-1" />

              <p className={`text-[10px] leading-relaxed font-medium mt-2 line-clamp-2 ${system.teamDescription ? 'text-zinc-500/80' : 'text-zinc-300 italic'}`}>{system.teamDescription || 'No description provided.'}</p>
            </div>
          )}

          <div className={`flex items-center justify-between mt-auto pt-2 ${isEditing ? 'border-t border-zinc-100/30' : ''}`}>
            <div className="flex items-center gap-1.5 px-2 py-1 bg-zinc-50 text-[8px] font-black text-zinc-400 rounded-lg">
              <Users size={10} strokeWidth={3} />
              {agentCount} {agentCount === 1 ? 'AGENT' : 'AGENTS'}
            </div>
            <div className="flex items-center gap-2">
              {isActive && !isEditing && (
                <div className="px-2 py-0.5 rounded-full text-white text-[7px] font-black uppercase tracking-[0.15em]" style={{ backgroundColor: system.color }}>Active</div>
              )}
              {isSelected && !isActive && !isEditing && (
                <Button type="button" onClick={handleSwitch} className="rounded-full bg-darkDelegation px-3 py-1.5 text-[9px] font-black uppercase tracking-wider text-white shadow-md hover:bg-black">Switch</Button>
              )}
              {isEditing && canDelete && (
                <Button type="button" variant="ghost" onClick={handleDelete} className="flex h-auto items-center gap-1.5 rounded-lg px-2 py-1 text-[8px] font-black uppercase tracking-widest text-red-500 hover:bg-red-50">
                  <Trash2 size={12} />
                  Delete Team
                </Button>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>

    <ConfirmModal
      isOpen={showSwitchConfirm}
      onClose={() => setShowSwitchConfirm(false)}
      onConfirm={performTeamSwitch}
      title="Switch team?"
      description="Switching teams clears the current board and starts fresh for the new team. Continue?"
      confirmLabel="Continue"
      cancelLabel="Cancel"
      icon={
        <div
          className="w-14 h-14 rounded-2xl flex items-center justify-center text-white shadow-sm"
          style={{ backgroundColor: system.color }}
        >
          <Users size={32} strokeWidth={2.5} />
        </div>
      }
    />
    </>
  );
};
