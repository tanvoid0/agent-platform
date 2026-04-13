import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import * as THREE from 'three/webgpu';
import { DEFAULT_SIMULATION_THEME, type SimulationTheme } from '../visual/SimulationTheme';

export class Stage {
  public scene: THREE.Scene;
  public camera: THREE.PerspectiveCamera;
  public controls: OrbitControls;
  /** Main shadow-casting light (used to scale shadow map for performance presets). */
  public mainDirectional!: THREE.DirectionalLight;
  private ambientLight!: THREE.AmbientLight;

  private followTarget: THREE.Vector3 | null = null;
  private readonly defaultTarget = new THREE.Vector3(0, 0.8, 0);

  constructor(rendererElement: HTMLElement, simulationTheme: SimulationTheme = DEFAULT_SIMULATION_THEME) {
    this.scene = new THREE.Scene();
    this.scene.background = new THREE.Color(simulationTheme.scene.background);

    this.camera = new THREE.PerspectiveCamera(45, window.innerWidth / window.innerHeight, 0.1, 500);
    this.camera.position.set(10, 8, 15);

    this.controls = new OrbitControls(this.camera, rendererElement);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.05;
    this.controls.rotateSpeed = 0.8;
    this.controls.enableRotate = true;
    this.controls.enablePan = false;
    this.controls.enableZoom = true;
    this.controls.minPolarAngle = Math.PI / 4.5;
    this.controls.maxPolarAngle = Math.PI / 2.4;
    this.controls.minDistance = 3;
    this.controls.maxDistance = 10;
    this.controls.target.set(0, 0.8, 0);

    this.controls.addEventListener('start', () => {
      rendererElement.style.cursor = 'grabbing';
    });
    this.controls.addEventListener('end', () => {
      rendererElement.style.cursor = 'auto';
    });

    this.setupLights(simulationTheme);
  }

  /** Background + key light colors (call when user switches global appearance preset). */
  public applySceneTheme(theme: Pick<SimulationTheme, 'scene' | 'lighting'>): void {
    this.scene.background = new THREE.Color(theme.scene.background);
    if (this.ambientLight) {
      this.ambientLight.color.setHex(theme.lighting.ambientColor);
      this.ambientLight.intensity = theme.lighting.ambientIntensity;
    }
    if (this.mainDirectional) {
      this.mainDirectional.color.setHex(theme.lighting.directionalColor);
      this.mainDirectional.intensity = theme.lighting.directionalIntensity;
    }
  }

  private setupLights(theme: SimulationTheme) {
    const L = theme.lighting;
    this.ambientLight = new THREE.AmbientLight(L.ambientColor, L.ambientIntensity);
    this.scene.add(this.ambientLight);

    const dirLight = new THREE.DirectionalLight(L.directionalColor, L.directionalIntensity);
    dirLight.position.set(10, 20, 10);
    dirLight.castShadow = true;
    dirLight.shadow.camera.near = 0.1;
    dirLight.shadow.camera.far = 100;
    dirLight.shadow.camera.top = 10;
    dirLight.shadow.camera.bottom = -10;
    dirLight.shadow.camera.right = 10;
    dirLight.shadow.camera.left = -10;
    dirLight.shadow.mapSize.set(2048, 2048);
    dirLight.shadow.bias = -0.0001;
    dirLight.shadow.radius = 2;
    dirLight.shadow.autoUpdate = true;
    this.scene.add(dirLight);
    this.mainDirectional = dirLight;
  }

  /** Lower resolution shadow maps reduce GPU cost when most casters are hidden (performant office mode). */
  public setShadowMapResolution(size: number): void {
    const light = this.mainDirectional;
    const { width, height } = light.shadow.mapSize;
    if (width === size && height === size) return;
    light.shadow.mapSize.set(size, size);
    light.shadow.needsUpdate = true;
    // Do not dispose `light.shadow.map` here. WebGPURenderer's ShadowNode keeps `shadowMap` + compiled
    // nodes referencing the depth texture; disposing/nulling out-of-band leaves stale bindings and Chrome
    // reports: Destroyed texture [ShadowDepthTexture] used in a submit.
  }

  public onResize(width: number, height: number) {
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
  }

  /** Call every frame with the character's world position to follow, or null to return to origin. */
  public setFollowTarget(pos: THREE.Vector3 | null) {
    this.followTarget = pos ? pos.clone() : null;
  }

  public update() {
    const lerpTarget = this.followTarget
      ? new THREE.Vector3(this.followTarget.x, 0.8, this.followTarget.z)
      : this.defaultTarget;
    this.controls.target.lerp(lerpTarget, 0.06);
    this.controls.update();
  }

  /**
   * Drive camera behavior based on chat state.
   * Call every frame from the animation loop.
   *
   * @param isChatting  True while a conversation is active.
   * @param playerMoving True while player is walking toward the NPC (GOTO state).
   */
  public setChatMode(isChatting: boolean, playerMoving: boolean): void {
    if (!this.controls) return;

    if (isChatting) {
      if (playerMoving) {
        // Lock controls and zoom in while walking
        this.controls.enabled = false;
        this.controls.minDistance = THREE.MathUtils.lerp(this.controls.minDistance, 4, 0.05);
        this.controls.maxDistance = THREE.MathUtils.lerp(this.controls.maxDistance, 6, 0.05);
      } else {
        // Arrived — re-enable controls, stay slightly zoomed
        this.controls.enabled = true;
        this.controls.minDistance = THREE.MathUtils.lerp(this.controls.minDistance, 3, 0.05);
        this.controls.maxDistance = THREE.MathUtils.lerp(this.controls.maxDistance, 10, 0.05);
      }
    } else {
      // Free roam
      this.controls.enabled = true;
      this.controls.minDistance = THREE.MathUtils.lerp(this.controls.minDistance, 3, 0.05);
      this.controls.maxDistance = THREE.MathUtils.lerp(this.controls.maxDistance, 50, 0.05);
    }
  }
}
