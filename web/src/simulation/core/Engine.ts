
import * as THREE from 'three/webgpu';

export class Engine {
  public renderer: THREE.WebGPURenderer;
  public timer: THREE.Timer;
  private reducedPixelRatio = false;
  private readonly container: HTMLElement;
  private isWebGLFallback = false;

  constructor(container: HTMLElement) {
    this.container = container;
    this.renderer = this.createRenderer(false);
    this.container.appendChild(this.renderer.domElement);
    this.timer = new THREE.Timer();
  }

  public async init() {
    try {
      await this.renderer.init();
    } catch (e) {
      console.error('WebGPU initialization failed, retrying with WebGL fallback:', e);
      this.replaceRendererWithWebGLFallback();
      try {
        await this.renderer.init();
      } catch (fallbackError) {
        console.error('WebGL fallback initialization failed:', fallbackError);
      }
    }
  }

  public onResize(width: number, height: number) {
    this.renderer.setSize(width, height, false);
  }

  /** Cap device pixel ratio (e.g. to 1) to ease fill-rate cost on high-DPI displays. */
  public setReducedPixelRatio(reduced: boolean): void {
    this.reducedPixelRatio = reduced;
    const pr = reduced ? Math.min(1, window.devicePixelRatio) : window.devicePixelRatio;
    this.renderer.setPixelRatio(pr);
  }

  public getReducedPixelRatio(): boolean {
    return this.reducedPixelRatio;
  }

  public render(scene: THREE.Scene, camera: THREE.PerspectiveCamera) {
    this.renderer.render(scene, camera);
  }

  public dispose() {
    this.renderer.dispose();
  }

  private createRenderer(forceWebGL: boolean): THREE.WebGPURenderer {
    const renderer = new THREE.WebGPURenderer({ antialias: true, forceWebGL });
    const pr = this.reducedPixelRatio ? Math.min(1, window.devicePixelRatio) : window.devicePixelRatio;
    renderer.setPixelRatio(pr);
    renderer.setSize(this.container.clientWidth, this.container.clientHeight, false);

    // Ensure the canvas is sized by CSS so physical resizing is fluid
    renderer.domElement.style.width = '100%';
    renderer.domElement.style.height = '100%';
    renderer.domElement.style.display = 'block';

    // Use default shadow map (PCF) as VSM support in WebGPU/NodeMaterial can be sensitive
    renderer.shadowMap.enabled = true;
    return renderer;
  }

  private replaceRendererWithWebGLFallback(): void {
    if (this.isWebGLFallback) return;

    const oldDom = this.renderer.domElement;
    this.renderer.dispose();
    this.renderer = this.createRenderer(true);
    this.isWebGLFallback = true;
    this.container.replaceChild(this.renderer.domElement, oldDom);
  }
}
