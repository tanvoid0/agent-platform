import * as THREE from 'three/webgpu';
import { getAgentSet, getAllAgents, AgenticSystem } from '../data/agents';
import { CharacterController } from './CharacterController';
import { Engine } from './core/Engine';
import { Stage } from './core/Stage';
import { DriverManager } from './drivers/DriverManager';
import { CharacterManager } from './entities/CharacterManager';
import { InputManager } from './input/InputManager';
import { NavMeshManager } from './pathfinding/NavMeshManager';
import { PoiManager } from './world/PoiManager';
import { WorldManager } from './world/WorldManager';
import { LlmServerRackVisual } from './world/LlmServerRackVisual';
import { buildSimulationWorldResources } from './visual/buildSimulationWorldResources';
import { DEFAULT_SIMULATION_THEME } from './visual/SimulationTheme';
import type { ISimulationWorldResource } from './visual/WorldResource';
import { useLlmConnectivityStore } from '../integration/store/llmConnectivityStore';

import { AgentSimulation } from './core/AgentSimulation';
import {
  PROJECT_SESSION_SCENE_CAPTURE_MS,
  parseLocomotion,
  type AgentPose,
} from '../integration/projectSession';
import {
  submitUserChatMessage,
  type SubmitUserChatMessageOptions,
} from '../integration/chat/submitUserChatMessage';
import { registerProposedPlanAmendmentHandler } from '../integration/orchestration/proposedPlanAmendment';
import { runAfterProjectBootstrap } from '../integration/projectPersistence';
import { useCoreStore } from '../integration/store/coreStore';
import { getActiveAgentSet, useTeamStore } from '../integration/store/teamStore';
import { useUiStore } from '../integration/store/uiStore';
import { AgentBehavior, ChatMessage, OfficeVisualStyle } from '../types';
import { BUBBLE_Y_OFFSET } from './constants';

/**
 * SceneManager — Client renderer for the office sim.
 *
 * Poses come from `coreStore.sessionPoseByAgent`. Live GPU state is merged on an interval; when
 * `sessionSceneRevision` bumps (project session loaded from API), avatars teleport to authority poses.
 */
export class SceneManager {
  private readonly simulationTheme = DEFAULT_SIMULATION_THEME;
  private engine: Engine;
  private stage: Stage;
  private characterManager: CharacterManager;
  private controller: CharacterController | null = null;
  private navMesh: NavMeshManager;
  private poiManager: PoiManager;
  private worldManager: WorldManager;
  private driverManager: DriverManager | null = null;
  private simulation: AgentSimulation | null = null;
  private worldResources: ISimulationWorldResource[] = [];
  private llmRackVisual: LlmServerRackVisual | null = null;

  private lastAgentSetId: string | null = null;
  private selectedIndex: number | null = null;
  private coreHandler: ((npcIndex: number, text: string) => Promise<string | null>) | null = null;

  private unsubs: (() => void)[] = [];
  private isDisposed = false;
  private container: HTMLElement;
  private resizeObserver: ResizeObserver;
  /** Last `sessionSceneRevision` applied to the 3D scene (authority poses). */
  private appliedSessionSceneRevision = -1;
  private lastSessionPoseCapture = 0;

  constructor(container: HTMLElement) {
    this.container = container;
    this.engine = new Engine(container);
    this.stage = new Stage(this.engine.renderer.domElement, this.simulationTheme);
    this.characterManager = new CharacterManager(this.stage.scene);
    this.navMesh = new NavMeshManager();
    this.poiManager = new PoiManager();
    this.characterManager.setPoiManager(this.poiManager);
    this.worldManager = new WorldManager(this.stage.scene, this.navMesh, this.poiManager, this.simulationTheme);

    this.resizeObserver = new ResizeObserver(() => this.onResize());
    this.resizeObserver.observe(container);

    const activeSet = getActiveAgentSet();
    this.simulation = new AgentSimulation(activeSet);
    registerProposedPlanAmendmentHandler((payload) => {
      this.simulation?.requestProposedPlanAmendment(payload);
    });
    this.setCoreHandler((idx, text) => this.simulation!.handleUserMessage(idx, text));

    void Promise.resolve(useCoreStore.persist.rehydrate()).then(() => {
      if (this.isDisposed) return;
      runAfterProjectBootstrap(() => {
        if (this.isDisposed) return;
        this.simulation?.resumePersistedWorkflow();
      });
    });

    useCoreStore.persist.onFinishHydration(() => {
      if (this.isDisposed) return;
      const style = useCoreStore.getState().officeVisualStyle;
      this.worldManager.syncOfficeAppearance(getActiveAgentSet().color, style);
      this.syncOfficeRenderingPreset(style);
    });

    this.unsubs.push(
      useCoreStore.subscribe((s, p) => {
        if (s.officeVisualStyle === p.officeVisualStyle) return;
        if (this.isDisposed) return;
        const set = getActiveAgentSet();
        this.worldManager.syncOfficeAppearance(set.color, s.officeVisualStyle);
        this.syncOfficeRenderingPreset(s.officeVisualStyle);
      })
    );

    this.unsubs.push(
      useLlmConnectivityStore.subscribe((s) => {
        this.llmRackVisual?.setConnectionState(s.serverChatHealth);
      })
    );

    this.init();
    this.startWatchingCoreStore();
  }

  private syncOfficeRenderingPreset(style: OfficeVisualStyle): void {
    const performant = style === 'performant';
    this.engine.setReducedPixelRatio(performant);
    this.stage.setShadowMapResolution(performant ? 768 : 2048);
  }

  private startWatchingCoreStore() {
    this.unsubs.push(
      useCoreStore.subscribe((state, prevState) => {
        const agentIndices = Array.from(new Set(state.tasks.flatMap(t => [t.assignedAgentId].filter(id => id !== undefined && id !== 0))));

        agentIndices.forEach(id => {
          const myTasks = state.tasks.filter(t => t.assignedAgentId === id);
          const hasChange = myTasks.some(t => {
            const pt = prevState.tasks.find(old => old.id === t.id);
            return !pt || pt.status !== t.status;
          });
          
          if (!hasChange) return;

          const needsHumanReview = myTasks.find(t => t.status === 'review');
          const inProgress = myTasks.find(t => t.status === 'in_progress');
          const justDone = myTasks.some(t => t.status === 'done' && !prevState.tasks.find(pt => pt.id === t.id && pt.status === 'done'));

          if (needsHumanReview) {
            this.moveNpcToBoardroom(id);
          } else if (inProgress) {
            this.setNpcWorking(id, true);
          } else if (justDone) {
            this.setNpcWorking(id, false);
            this.moveNpcToSpawn(id);
          }
        });
      })
    );
  }

  /** Centroid of desk POIs so the LLM rack sits in the office, not the outer void. */
  private getLlmServerWorkClusterAnchor(): THREE.Vector3 | null {
    const work = this.poiManager.getPoisByPrefix('sit_work');
    if (work.length > 0) {
      const v = new THREE.Vector3();
      for (const p of work) v.add(p.position);
      return v.multiplyScalar(1 / work.length);
    }
    const spawns = this.poiManager.getPoisByPrefix('spawn');
    if (spawns.length > 0) {
      const v = new THREE.Vector3();
      for (const p of spawns) v.add(p.position);
      return v.multiplyScalar(1 / spawns.length);
    }
    return null;
  }

  /** Aligns with `public/models/office.glb` (`area-canteen` / `area-hub` / `area-lounge`). */
  private getLoungeAnchorWorld(): THREE.Vector3 | null {
    const exact =
      this.poiManager.getPoi('area-lounge') ??
      this.poiManager.getPoi('area-canteen') ??
      this.poiManager.getPoi('area-hub');
    if (exact) return exact.position.clone();
    const candidates = this.poiManager.getPoisByPrefix('area-').filter((p) => {
      const s = p.id.toLowerCase();
      return (
        s.includes('lounge') ||
        s.includes('canteen') ||
        s.includes('hub') ||
        s.includes('breakout')
      );
    });
    return candidates[0]?.position.clone() ?? null;
  }

  private async init() {
    await this.engine.init();
    if (this.isDisposed) return;

    await this.worldManager.load({
      themeColor: getActiveAgentSet().color,
      style: useCoreStore.getState().officeVisualStyle,
    });
    this.worldResources = buildSimulationWorldResources({
      scene: this.stage.scene,
      getOffice: () => this.worldManager.getOffice(),
      camera: this.stage.camera,
      getWorkClusterAnchor: () => this.getLlmServerWorkClusterAnchor(),
      getLoungeAnchorWorld: () => this.getLoungeAnchorWorld(),
      getOfficeAnimations: () => this.worldManager.getOfficeAnimations(),
      poiManager: this.poiManager,
      theme: this.simulationTheme,
    });
    this.llmRackVisual =
      this.worldResources.find((r): r is LlmServerRackVisual => r instanceof LlmServerRackVisual) ?? null;
    if (this.llmRackVisual) {
      this.llmRackVisual.setConnectionState(useLlmConnectivityStore.getState().serverChatHealth);
    }
    this.syncOfficeRenderingPreset(useCoreStore.getState().officeVisualStyle);
    await this.characterManager.load();
    if (this.isDisposed) return;

    const state = useUiStore.getState();
    this.characterManager.setInstanceCount(state.instanceCount);
    this.controller = new CharacterController(this.characterManager, this.navMesh, this.poiManager);
    this.driverManager = new DriverManager(this.controller);
    
    const activeSet = getActiveAgentSet();
    const playerIndex = activeSet.user.index;
    this.driverManager.registerPlayer(playerIndex);

    getAllAgents(activeSet).forEach((agent) => {
      if (agent.index !== playerIndex) this.driverManager!.registerNpc(agent.index, agent);
    });

    new InputManager(
      this.engine.renderer.domElement, this.stage.camera,
      () => this.controller!.getCPUPositions(), () => this.controller!.getCount(),
      (idx) => {
        if (useUiStore.getState().isChatting) useUiStore.getState().setChatting(false);
        this.selectedIndex = idx !== activeSet.user.index ? idx : null;
        if (this.selectedIndex !== null) {
          this.startChat(this.selectedIndex, { expandProjectRail: true });
        } else {
          useUiStore.getState().setSelectedNpc(null);
        }
      },
      (x, z) => this.driverManager?.getPlayerDriver().onFloorClick(x, z),
      (idx, pos) => useUiStore.getState().setHoveredNpc(idx, pos),
      () => this.poiManager.getAllPois(),
      (id, label, pos) => useUiStore.getState().setHoveredPoi(id, label, pos),
      (id) => this.driverManager?.getPlayerDriver().onPoiClick(id),
      this.worldManager.getOffice() ?? undefined, (p) => this.navMesh.isPointOnNavMesh(p)
    );

    this.engine.renderer.setAnimationLoop(this.animate.bind(this));

    this.registerSessionSceneRevisionListener();
    this.applyInitialSessionPosesFromStore();

    this.unsubs.push(useUiStore.subscribe((s, prev) => {
      if (s.instanceCount !== prev.instanceCount) this.controller?.setInstanceCount(s.instanceCount);
      const team = useTeamStore.getState();
      if (team.selectedAgentSetId !== this.lastAgentSetId) {
        this.lastAgentSetId = team.selectedAgentSetId;
        const set = getAgentSet(team.selectedAgentSetId, team.customSystems);
        this.reinitializeSimulation(set);
        this.worldManager.syncOfficeAppearance(set.color, useCoreStore.getState().officeVisualStyle);
        if (this.controller) {
          this.controller.setColors();
          this.controller.warpAllToSpawn(set.user.index, getAllAgents(set).map(a => a.index));
        }
      }
      if (
        (s.isChatting !== prev.isChatting ||
          s.isThinking !== prev.isThinking ||
          s.isTyping !== prev.isTyping ||
          s.npcSpeechPulseActive !== prev.npcSpeechPulseActive) &&
        this.controller
      ) {
        const set = getActiveAgentSet();
        if (s.isChatting && !prev.isChatting && s.selectedNpcIndex !== null) {
           this._startChatVisuals(s.selectedNpcIndex);
        }
        
        if (s.isChatting && s.selectedNpcIndex !== null) {
          const npc = s.selectedNpcIndex, user = set.user.index;
          if (this.controller.getState(npc) !== 'walk') {
            if (s.isThinking) this.controller.play(npc, 'listen');
            else if (s.npcSpeechPulseActive) this.controller.play(npc, 'talk');
            else this.controller.play(npc, 'listen');
          }
          this.controller.setSpeaking(npc, !s.isThinking && s.npcSpeechPulseActive);
          if (this.controller.getState(user) !== 'walk') this.controller.play(user, s.isTyping ? 'talk' : 'listen');
          this.controller.setSpeaking(user, s.isTyping);
        } else if (!s.isChatting && prev.isChatting) {
          // Cleanup Chat Visuals
          const npc = prev.selectedNpcIndex;
          const user = set.user.index;
          if (npc !== null) {
            this.driverManager?.getNpcDriver(npc)?.setChatting(false);
            this.controller.setSpeaking(npc, false);
            this.controller.play(npc, 'idle');
            this.controller.poiManager.releaseAll(npc);
          }
          this.controller.setSpeaking(user, false);
          this.controller.play(user, 'idle');
          this.selectedIndex = null;
        }
      }

      // Monitor individual agent status changes for autonomous animations
      if (s.agentStatuses !== prev.agentStatuses && this.controller) {
        Object.keys(s.agentStatuses).forEach(key => {
          const idx = parseInt(key);
          const status = s.agentStatuses[idx];
          const prevStatus = prev.agentStatuses[idx];
          if (status !== prevStatus) {
             if (status === 'talking') this.setNpcTalking(idx, true);
             else if (prevStatus === 'talking') this.setNpcTalking(idx, false);
          }
        });
      }
    }));
  }

  /** On `sessionSceneRevision` bump, apply `sessionPoseByAgent` (saved project / persisted session). */
  private registerSessionSceneRevisionListener(): void {
    this.unsubs.push(
      useCoreStore.subscribe((s) => {
        if (this.isDisposed || !this.controller || !this.characterManager) return;
        if (s.sessionSceneRevision === this.appliedSessionSceneRevision) return;
        this.appliedSessionSceneRevision = s.sessionSceneRevision;
        this.applyAuthorityPosesToScene(s.sessionPoseByAgent);
      }),
    );
  }

  private applyInitialSessionPosesFromStore(): void {
    if (!this.controller) return;
    const s = useCoreStore.getState();
    this.appliedSessionSceneRevision = s.sessionSceneRevision;
    if (Object.keys(s.sessionPoseByAgent).length > 0) {
      this.applyAuthorityPosesToScene(s.sessionPoseByAgent);
    }
  }

  private applyAuthorityPosesToScene(poses: Record<number, AgentPose>): void {
    if (!this.controller || !this.characterManager) return;
    const count = this.controller.getCount();
    for (const [ks, pose] of Object.entries(poses)) {
      const idx = Number(ks);
      if (!Number.isFinite(idx) || idx < 0 || idx >= count) continue;
      this.controller.cancelMovement(idx);
      this.characterManager.setPositionAndZeroVelocity(idx, new THREE.Vector3(pose.x, pose.y, pose.z));
      const st = parseLocomotion(pose.locomotion);
      if (st) {
        try {
          this.controller.play(idx, st);
        } catch {
          /* invalid transition — keep placement only */
        }
      }
    }
  }

  private _startChatVisuals(npcIndex: number): void {
    if (!this.controller) return;
    const pos = this.controller.getCPUPositions(); if (!pos) return;
    const set = getActiveAgentSet();
    const npc = new THREE.Vector3(pos[npcIndex * 4], 0, pos[npcIndex * 4 + 2]);
    const player = new THREE.Vector3(pos[set.user.index * 4], 0, pos[set.user.index * 4 + 2]);
    let dir = new THREE.Vector3().subVectors(player, npc).normalize();
    if (dir.length() < 0.01) dir.set(1, 0, 0);
    const target = npc.clone().addScaledVector(dir, 1.2);

    this.selectedIndex = npcIndex;
    this.driverManager?.getNpcDriver(npcIndex)?.setChatting(true);
    this.controller.cancelMovement(npcIndex);
    this.controller.play(npcIndex, 'listen');
    this.controller.getAgentStateBuffer()?.setWaypoint(npcIndex, dir.x, dir.z);
    this.driverManager?.getPlayerDriver()?.walkTo(target, 'listen', () => {
      const p = this.controller!.getCPUPositions()!;
      const fx = p[npcIndex * 4] - p[set.user.index * 4], fz = p[npcIndex * 4 + 2] - p[set.user.index * 4 + 2];
      this.controller!.getAgentStateBuffer()?.setWaypoint(set.user.index, fx, fz);
      this.controller!.getAgentStateBuffer()?.setWaypoint(npcIndex, -fx, -fz);
      this._triggerNpcGreeting(npcIndex);
    });
  }

  public startChat(
    npcIndex: number,
    options?: {
      /** When true, ask the app shell to expand the right project rail (e.g. user picked an agent in the 3D office). */
      expandProjectRail?: boolean;
    },
  ): void {
    useUiStore.getState().setSelectedNpc(npcIndex);
    useUiStore.getState().setChatting(true);
    if (options?.expandProjectRail) {
      useUiStore.getState().bumpProjectRailExpandRequest();
    }
  }


  public async sendMessage(text: string, options?: SubmitUserChatMessageOptions): Promise<void> {
    const { selectedNpcIndex } = useUiStore.getState();
    if (selectedNpcIndex === null) return;
    await submitUserChatMessage(this.coreHandler, selectedNpcIndex, text, options);
  }

  private reinitializeSimulation(activeSet: AgenticSystem) {
    if (this.simulation) this.simulation.dispose();
    this.simulation = new AgentSimulation(activeSet);
    registerProposedPlanAmendmentHandler((payload) => {
      this.simulation?.requestProposedPlanAmendment(payload);
    });
    this.setCoreHandler((idx, text) => this.simulation!.handleUserMessage(idx, text));
    void Promise.resolve(useCoreStore.persist.rehydrate()).then(() => {
      if (this.isDisposed) return;
      runAfterProjectBootstrap(() => {
        if (this.isDisposed) return;
        this.simulation?.resumePersistedWorkflow();
      });
    });
    if (this.driverManager) {
      const playerIndex = activeSet.user.index;
      this.driverManager.dispose();
      this.driverManager.registerPlayer(playerIndex);
      getAllAgents(activeSet).forEach((a) => {
        if (a.index !== playerIndex) this.driverManager!.registerNpc(a.index, a);
      });
    }
  }
  public setCoreHandler(handler: ((npcIndex: number, text: string) => Promise<string | null>) | null): void {
    this.coreHandler = handler;
  }

  public getLeadBrain() {
    if (!this.simulation) return null;
    const set = getActiveAgentSet();
    const lead = this.simulation.getAgent(set.leadAgent.index);
    return lead?.brain || null;
  }

  public retryTaskExecution(taskId: string): boolean {
    return this.simulation?.retryTaskExecution(taskId) ?? false;
  }

  /** Requeue all failed / stalled execution rows in board (`tasks`) order. */
  public retryAllFailedOrStalledTaskExecutions(): number {
    return this.simulation?.retryAllFailedOrStalledTaskExecutions() ?? 0;
  }

  /** 
   * SMART DESK ASSIGNMENT
   * Attempts to find a work POI. If work-${index} is missing, it picks 
   * a desk from the 'sit_work' group based on the agent's unique index.
   */
  public setNpcWorking(index: number, working: boolean): void {
    if (!this.controller) return;
    if (working) {
      const id = `sit_work-${index}`;
      let poi = this.poiManager.getPoi(id);
      if (!poi) {
         // Smart fallback: assign a desk based on order (agent index is 1-based)
         const desks = this.poiManager.getPoisByPrefix('sit_work');
         if (desks.length > 0) poi = desks[(index - 1) % desks.length];
      }
      if (poi) this.controller.walkToPoi(index, poi.id);
    }
  }
  
  public setNpcTalking(index: number, talking: boolean): void {
    if (!this.controller) return;
    if (talking) {
      if (this.controller.getState(index) !== 'walk') this.controller.play(index, 'talk');
      this.controller.setSpeaking(index, true);
    } else {
      this.controller.setSpeaking(index, false);
      const task = useCoreStore.getState().tasks.find(t => t.status === 'review' && t.assignedAgentId === index);
      this.controller.play(index, task ? 'listen' : 'idle');
    }
  }

  public moveNpcToBoardroom(index: number): void {
    if (!this.controller) return;
    const meetingChairs = this.poiManager.getFreePoisByPrefix('sit_idle-meeting', index);
    const glbSitIdle =
      meetingChairs.length === 0 ? this.poiManager.getFreePois('sit_idle', index) : [];
    const chair =
      meetingChairs.length > 0
        ? meetingChairs[(index - 1) % meetingChairs.length]!
        : glbSitIdle.length > 0
          ? glbSitIdle[(index - 1) % glbSitIdle.length]!
          : undefined;
    const fallback =
      this.poiManager.getPoi('area-boardroom') ?? this.poiManager.getPoi('boardroom');
    const poi = chair ?? fallback;
    if (poi) {
      this.controller.walkToPoi(index, poi.id);
    }
  }

  public moveNpcToSpawn(index: number, onArrival?: () => void): void {
    if (!this.controller) return;
    const poi = this.poiManager.getPoi(`spawn-${index}`);
    if (poi) this.controller.moveTo(index, poi.position, 'idle', onArrival, undefined, poi.quaternion);
    else if (onArrival) onArrival();
  }

  private async _triggerNpcGreeting(idx: number): Promise<void> {
    const set = getActiveAgentSet();
    const agent = getAllAgents(set).find(a => a.index === idx);
    if (!agent) return;
    const msg: ChatMessage = { role: 'assistant', text: `Hello. I am ${agent.name}. How can I assist you?`, timestamp: new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) };
    useUiStore.setState({ chatMessages: [msg] });
    useUiStore.getState().triggerNpcSpeechPulse();
  }

  private onResize() { 
    const w = this.container.clientWidth, h = this.container.clientHeight;
    if (w === 0 || h === 0) return;
    this.stage.onResize(w, h);
    if (!useCoreStore.getState().isResizing) this.engine.onResize(w, h);
  }

  private animate() {
    this.engine.timer.update(); const delta = this.engine.timer.getDelta();
    for (const r of this.worldResources) r.update?.(delta);
    this.stage.update(); this.controller?.update(delta, this.engine.renderer);
    this.controller?.syncFromGPU(this.engine.renderer).then((pos) => {
      if (!pos || !this.controller) return;
      this.controller.updatePaths(pos); this.driverManager?.update(pos, delta);
      this.updateTransparency(pos, delta);

      const now = Date.now();
      const corePhase = useCoreStore.getState().phase;
      if (
        !this.isDisposed &&
        now - this.lastSessionPoseCapture >= PROJECT_SESSION_SCENE_CAPTURE_MS &&
        (corePhase === 'working' || corePhase === 'done')
      ) {
        this.lastSessionPoseCapture = now;
        const count = this.controller.getCount();
        const entries: Record<number, AgentPose> = {};
        for (let i = 0; i < count; i++) {
          entries[i] = {
            x: pos[i * 4],
            y: pos[i * 4 + 1],
            z: pos[i * 4 + 2],
            locomotion: this.controller.getState(i),
          };
        }
        useCoreStore.getState().mergeSessionPosesFromSimCapture(entries);
      }
    });
    const player = getActiveAgentSet().user.index;
    this.stage.setFollowTarget(this.controller?.getCPUPosition(this.selectedIndex ?? player) ?? null);
    const { selectedNpcIndex, setSelectedPosition, selectedPosition } = useUiStore.getState();
    const npcScreenPositions: Record<number, { x: number; y: number }> = {};
    const rect = this.container.getBoundingClientRect();
    if (this.controller) {
      for (let i = 0; i < this.controller.getCount(); i++) {
        const p = this.controller.getCPUPosition(i);
        if (p) {
          const s = p.clone(); s.y += BUBBLE_Y_OFFSET; s.project(this.stage.camera);
          npcScreenPositions[i] = { x: (s.x * 0.5 + 0.5) * rect.width, y: (s.y * -0.5 + 0.5) * rect.height };
        }
      }
      useUiStore.setState({ npcScreenPositions });
    }
    if (selectedNpcIndex !== null && npcScreenPositions[selectedNpcIndex]) {
      const p = npcScreenPositions[selectedNpcIndex];
      if (Math.abs(p.x - (selectedPosition?.x ?? 0)) > 0.5 || Math.abs(p.y - (selectedPosition?.y ?? 0)) > 0.5) setSelectedPosition(p);
    } else if (selectedPosition !== null) setSelectedPosition(null);
    this.stage.setChatMode(useUiStore.getState().isChatting, this.controller?.getAgentState(player) === AgentBehavior.GOTO);
    this.engine.render(this.stage.scene, this.stage.camera);
  }

  private updateTransparency(pos: Float32Array, delta: number) {
    if (!this.controller) return;
    const count = this.controller.getCount(), buffer = this.controller.getAgentStateBuffer();
    if (!buffer) return;
    for (let i = 0; i < count; i++) {
      let overlap = false;
      for (let j = 0; j < count; j++) {
        if (i === j) continue;
        if ((pos[i * 4] - pos[j * 4]) ** 2 + (pos[i * 4 + 2] - pos[j * 4 + 2]) ** 2 < 0.36) { overlap = true; break; }
      }
      const cur = buffer.getAlpha(i), tar = overlap ? 0.4 : 1.0;
      if (Math.abs(cur - tar) > 0.01) buffer.setAlpha(i, THREE.MathUtils.lerp(cur, tar, Math.min(delta * 2.0, 1.0)));
    }
  }

  public resetScene() {
    if (!this.controller) return;
    useUiStore.getState().setChatting(false);
    const set = getActiveAgentSet();
    getAllAgents(set).forEach((a) => this.controller?.setSpeaking(a.index, false));
    this.controller.warpAllToSpawn(set.user.index, getAllAgents(set).map(a => a.index));
    this.stage.setFollowTarget(null);
    this.stage.setChatMode(false, false);
  }

  public dispose() {
    this.isDisposed = true;
    registerProposedPlanAmendmentHandler(null);
    this.resizeObserver.disconnect();
    this.unsubs.forEach((u) => u());
    for (let i = this.worldResources.length - 1; i >= 0; i--) {
      this.worldResources[i]!.dispose();
    }
    this.worldResources = [];
    this.llmRackVisual = null;
    this.driverManager?.dispose();
    this.engine.dispose();
  }
}
