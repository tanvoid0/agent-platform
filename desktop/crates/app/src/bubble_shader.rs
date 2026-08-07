//! The E.V. orb as a WGSL fragment shader.
//!
//! The canvas version of this ([`crate::assistant_view`]'s `draw_bubble`) is
//! kept as [`crate::shell::HudStyle::BubbleCanvas`], and the reason both exist
//! is worth stating: iced's canvas has neither a radial gradient nor a blur, so
//! every soft falloff there has to be bought as stacked fills, and stacked fills
//! band. The reference this is built against has a halo reaching 2.1× the orb's
//! radius and a colour field wandering inside it — both are gradient problems,
//! and a fragment shader is where gradients are free.
//!
//! Every number in the shader below came from measuring a reference loop frame
//! by frame rather than from taste; the constants carry what was measured. That
//! loop is checked in as `docs/images/ev-orb-reference.mp4` — re-measure against
//! it before changing a constant here.
//!
//! This draws through iced's `wgpu` backend. If iced falls back to its
//! `tiny-skia` software renderer the widget renders nothing, which is what the
//! canvas style is for.

use crate::assistant::{Mode, State};
use crate::ui::theme;
use iced::advanced::graphics::Viewport;
use iced::widget::shader;
use iced::{mouse, wgpu, Color, Rectangle, Theme};

/// Everything the shader needs for a frame. `repr(C)` with explicit padding:
/// WGSL aligns `vec4` to 16 bytes, and a mismatch here is a silently wrong
/// picture rather than an error.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    accent: [f32; 4],
    cool: [f32; 4],
    warm: [f32; 4],
    paper: [f32; 4],
    /// Seconds of animation time, already scaled for the mode.
    time: f32,
    /// Width / height of the widget, so the orb stays round in a wide box.
    aspect: f32,
    /// Power-on fade, 0..1.
    boot: f32,
    energy: f32,
    bass: f32,
    beat: f32,
    _pad: [f32; 2],
}

#[derive(Debug)]
pub struct Bubble {
    uniforms: Uniforms,
}

impl Bubble {
    pub fn new(state: &State, iced_theme: &Theme) -> Self {
        let t = theme::tokens(iced_theme);
        let hue = |m: Mode| match m {
            Mode::Idle => mix(SPIDEY_BLUE, HOLO_CYAN, 0.35),
            // An open mic is never the same colour as an idle one.
            Mode::Armed => t.success,
            Mode::Listening => SPIDEY_RED,
            Mode::Thinking => t.warning,
            Mode::Speaking => HOLO_CYAN,
        };
        let ink = |c: Color| if t.dark { c } else { mix(c, Color::BLACK, 0.18) };
        let ease = |x: f32| {
            let x = 1.0 - x.clamp(0.0, 1.0);
            1.0 - x * x * x
        };
        let accent = ink(mix(hue(state.mode_prev), hue(state.mode()), ease(state.mode_t)));
        // Measured: the reference's interior cycles rose ↔ periwinkle ↔ mint.
        // Anchoring both partners on the mode's own hue keeps Listening reading
        // red instead of everything always landing on violet.
        // The partners lean far harder toward their own hue than the mode's:
        // measured, the reference is never monochrome — a green Armed orb that
        // was green everywhere read as a flat disc, not glass.
        let cool = ink(mix(accent, HOLO_CYAN, 0.72));
        let warm = ink(mix(accent, Color::from_rgb(1.0, 0.42, 0.82), 0.72));
        let paper =
            if t.dark { Color::from_rgb(0.04, 0.05, 0.08) } else { Color::from_rgb(0.99, 0.99, 1.0) };

        Self {
            uniforms: Uniforms {
                accent: rgba(accent),
                cool: rgba(cool),
                warm: rgba(warm),
                paper: rgba(paper),
                // Thinking churns faster; the shader has no notion of mode.
                time: state.phase * if state.mode() == Mode::Thinking { 1.9 } else { 1.0 },
                aspect: 1.0,
                boot: ease(state.boot),
                energy: state.energy.clamp(0.0, 1.0),
                bass: crate::assistant_view::band_at(&state.bands, 0.08),
                beat: state.beat,
                _pad: [0.0; 2],
            },
        }
    }
}

const SPIDEY_RED: Color = Color::from_rgb(0.902, 0.169, 0.180);
const SPIDEY_BLUE: Color = Color::from_rgb(0.263, 0.451, 0.918);
const HOLO_CYAN: Color = Color::from_rgb(0.208, 0.816, 1.0);

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

fn rgba(c: Color) -> [f32; 4] {
    [c.r, c.g, c.b, c.a]
}

impl<Message> shader::Program<Message> for Bubble {
    type State = ();
    type Primitive = Bubble;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, bounds: Rectangle) -> Bubble {
        let mut uniforms = self.uniforms;
        uniforms.aspect = if bounds.height > 0.0 { bounds.width / bounds.height } else { 1.0 };
        Bubble { uniforms }
    }
}

impl shader::Primitive for Bubble {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        queue.write_buffer(&pipeline.uniforms, 0, bytemuck::bytes_of(&self.uniforms));
    }

    fn draw(&self, pipeline: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &pipeline.bind_group, &[]);
        // No vertex buffer: the vertex stage builds a covering triangle from the
        // vertex index alone, and iced has already set the pass viewport to this
        // widget's bounds.
        pass.draw(0..3, 0..1);
        true
    }
}

#[derive(Debug)]
pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ev.bubble.wgsl"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(SHADER)),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ev.bubble.uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ev.bubble.layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ev.bubble.bind"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ev.bubble.pipeline.layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ev.bubble.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Straight alpha over whatever the container painted, so the
                    // panel's rounded corners and border survive.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self { pipeline, uniforms, bind_group }
    }
}

const SHADER: &str = r#"
struct U {
  accent: vec4<f32>,
  cool:   vec4<f32>,
  warm:   vec4<f32>,
  paper:  vec4<f32>,
  time:   f32,
  aspect: f32,
  boot:   f32,
  energy: f32,
  bass:   f32,
  beat:   f32,
  pad0:   f32,
  pad1:   f32,
};
@group(0) @binding(0) var<uniform> u: U;

struct VOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

// One triangle large enough to cover the widget. Cheaper than a quad and needs
// no vertex buffer at all.
@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
  var corner = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
  var out: VOut;
  out.pos = vec4<f32>(corner[i], 0.0, 1.0);
  out.uv = corner[i];
  return out;
}

fn hash(p: vec2<f32>) -> f32 {
  var q = fract(p * vec2<f32>(12.9898, 78.233));
  q += dot(q, q + 34.56);
  return fract(q.x * q.y);
}

fn noise(p: vec2<f32>) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let w = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), w.x),
             mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), w.x), w.y);
}

fn fbm(p: vec2<f32>) -> f32 {
  var v = 0.0;
  var a = 0.5;
  var q = p;
  for (var i = 0; i < 4; i = i + 1) {
    v += a * noise(q);
    q *= 2.0;
    a *= 0.5;
  }
  return v;
}

// The liquid edge. A few integer harmonics in the angle, phases drifting at
// unrelated rates so the shape never visibly repeats. Integer harmonics close
// seamlessly at the wrap and, being analytic, are smooth at every zoom — `fbm`
// over the direction gave four octaves of noise, which reads as a lumpy, uneven
// outline rather than something liquid.
//
// Each harmonic is voiced by a different part of the signal, so speaking does
// not just inflate the orb: bass swells the slow two-lobe stretch, overall
// energy drives the three-lobe roll, and a transient snaps the fine ripple.
fn morph(a: f32, t: f32, e: f32, b: f32, k: f32) -> f32 {
  let n2 = sin(2.0 * a + t * 0.61) * (0.55 + 0.45 * b);
  let n3 = sin(3.0 * a - t * 0.47 + 1.3) * (0.35 + 0.35 * e);
  let n4 = sin(4.0 * a + t * 0.33 + 2.7) * (0.18 + 0.40 * k);
  return (n2 + n3 + n4) / 1.9;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
  let t = u.time;
  var p = in.uv;
  p.x *= u.aspect;                      // square the space up, keep the orb round

  // Measured: the whole orb wanders about ±0.10 R rather than sitting centred.
  // Wider than that and the drift reads as the widget sliding, not the orb
  // breathing — the reference stays put and moves its *shells*.
  let drift = vec2<f32>(sin(t * 0.23) * 0.6 + sin(t * 0.41 + 1.7) * 0.4,
                        cos(t * 0.19 + 0.8) * 0.6 + sin(t * 0.31 + 2.3) * 0.4);
  // Measured: radius breathes about ±8% on a 2-3 s cycle.
  let R = 0.38 * (1.0 + 0.08 * sin(t * 0.5) + 0.10 * u.energy + 0.05 * u.bass)
        * (0.55 + 0.45 * u.boot);
  let c = drift * 0.10 * R;
  let q = p - c;
  let dist = length(q);
  let ang = atan2(q.y, q.x);
  // Measured: the loop is eccentric, longest over shortest radius about 1.15
  // at rest and half again as much on a loud syllable.
  let amp = 0.075 + 0.075 * u.energy + 0.050 * u.bass;
  let redge = R * (1.0 + amp * morph(ang, t, u.energy, u.bass, u.beat));
  let sd = dist - redge;                // signed distance: negative inside
  let aa = max(fwidth(sd), 0.0008);
  let r = clamp(dist / max(redge, 1e-4), 0.0, 2.0);

  // -- Second shell ---------------------------------------------------------
  // Measured: the reference is two rings, not one — a fainter, fatter lobe
  // offset a tenth of a radius, which is where the "petal" sticking out past
  // the orb comes from. One ring alone reads as a sticker with a stroke.
  let c2 = c + vec2<f32>(sin(t * 0.17 + 1.1), cos(t * 0.21 + 0.4)) * 0.10 * R;
  let q2 = p - c2;
  let a2 = atan2(q2.y, q2.x);
  // Same morph, its clock offset — the two shells must not breathe in step or
  // they read as one thick ring instead of two.
  let sd2 = length(q2) - R * (1.03 + amp * 1.3 * morph(a2, t + 11.0, u.energy, u.bass, u.beat));

  // -- Interior: a colour mass wandering within ±0.45 R ---------------------
  let mass = vec2<f32>(fbm(vec2<f32>(t * 0.11, 3.7)) - 0.5,
                       fbm(vec2<f32>(9.1, t * 0.09)) - 0.5) * 0.8;
  let field = fbm((q / R - mass) * 0.9 + vec2<f32>(t * 0.07, t * 0.05));
  // A smooth sweep across the sphere carries the hue; the noise only nudges it.
  // Driving the rotation off the noise alone turned the interior into visible
  // fbm blotches, where the reference is one gradient corner to corner.
  let n = q / max(redge, 1e-4);
  let sweep = clamp(0.5 + 0.6 * dot(n, normalize(vec2<f32>(-0.45, 0.72))), 0.0, 1.0);
  // Measured: rose ↔ periwinkle ↔ mint on a 2-3.5 s cycle. Rotated through
  // three weights rather than lerped between two: `mix(warm, cool, 0.5)` walks
  // through grey in RGB, and a mass that is grey in its middle is exactly the
  // washed-out look this had. Rotating never passes through the middle.
  let ic = sweep * 2.6 + field * 1.1 + t * 0.30;
  let v1 = 0.5 + 0.5 * sin(ic);
  let v2 = 0.5 + 0.5 * sin(ic + 2.0944);
  let v3 = 0.5 + 0.5 * sin(ic + 4.1888);
  var inner = (u.warm.rgb * v1 + u.cool.rgb * v2 + u.accent.rgb * v3) / max(v1 + v2 + v3, 1e-4);
  // Measured: the interior is *pale* everywhere and near-white at the rim —
  // frosted glass, not a coloured disc. This is most of what separated ours.
  inner = mix(inner, u.paper.rgb, 0.34 + 0.52 * smoothstep(0.42, 1.02, r));

  // Light reflect: one broad sheen from the upper left, strongest where the
  // surface faces the light, plus the near-white lip just inside the rim that
  // makes the edge read as thickness rather than as a cut-out. The lip is thin
  // on purpose — wide enough and it stops being a highlight and becomes a
  // white donut with the colour pushed into the middle.
  inner += u.paper.rgb * 0.12 * smoothstep(0.05, 1.0, dot(n, normalize(vec2<f32>(-0.5, 0.72))))
         * (1.0 - 0.45 * r);
  inner = mix(inner, u.paper.rgb, exp(-pow((r - 0.92) / 0.07, 2.0)) * 0.22);

  let inside = smoothstep(aa, -aa, sd);

  // -- The rim --------------------------------------------------------------
  // Measured: a *closed* chromatic ring, hue rotating around the circumference
  // (cyan → periwinkle → rose) and one segment several times fatter and softer
  // than the rest. The old single hot arc that faded out is why ours read as an
  // outline that blinked instead of a rim that turns.
  // Three weights a third of a turn apart rather than two nested `mix`es: with
  // nesting, whichever hue the second mix carried only appeared where the first
  // had already faded, and the ring came out one colour. Barycentric keeps all
  // three on the circumference at once, which is what the reference does.
  let ph = ang + t * 0.25;
  let w1 = 0.5 + 0.5 * sin(ph);
  let w2 = 0.5 + 0.5 * sin(ph + 2.0944);
  let w3 = 0.5 + 0.5 * sin(ph + 4.1888);
  let rimc = (u.cool.rgb * w1 + u.accent.rgb * w2 + u.warm.rgb * w3) / max(w1 + w2 + w3, 1e-4);
  let fat = 0.5 + 0.5 * sin(ang - t * 0.31);
  // Gaussians across the distance field: smooth by construction, which is the
  // whole reason this is a shader and not stacked strokes.
  let wid = R * (0.016 + 0.055 * fat + 0.030 * u.bass + 0.012 * u.beat);
  // The ring never goes out: with a low floor the hue that happened to land on
  // a dim arc simply never appeared, and the whole rim read as one colour.
  let gl = 0.78 + 0.22 * pow(0.5 + 0.5 * sin(ang - t * 0.40), 1.4);
  let wo = wid * 2.2;
  let rim = clamp(exp(-(sd / wid) * (sd / wid)) * gl
                + exp(-(sd2 / wo) * (sd2 / wo)) * 0.55, 0.0, 1.0);

  // -- Glow behind ----------------------------------------------------------
  // Measured: a tight bloom hugging the orb plus a very faint wash still
  // readable at 3 R, and the wash is *tinted* — pink one side, blue the other.
  // Two exponentials, so it has no edge anywhere and nothing to band.
  let halo = exp(-dist / (0.80 * R)) * 0.26 + exp(-dist / (2.20 * R)) * 0.18;
  var halo_col = mix(u.warm.rgb, u.cool.rgb, 0.5 + 0.5 * sin(ang - t * 0.27));
  halo_col = mix(halo_col, u.paper.rgb, 0.35);

  var col = mix(halo_col, inner, inside);
  col = mix(col, rimc, rim * 0.80);
  col += rimc * rim * 0.18;             // the rim blooms into the glow
  let alpha = clamp(halo * (1.0 - inside) + inside * 0.97 + rim, 0.0, 1.0) * u.boot;
  // Every falloff here is a gradient over a few hundred pixels, which is where
  // 8-bit output banks into visible steps. A sub-LSB dither costs one hash and
  // turns the rings into noise the eye does not resolve.
  let dith = (hash(in.pos.xy) - 0.5) / 255.0;
  return vec4<f32>(col + vec3<f32>(dith), alpha);
}
"#;

#[cfg(test)]
mod tests {
    /// The GPU compiles this shader the first time the Dashboard opens, so a
    /// typo in it is a panic on the app's landing page — the most expensive
    /// place to find one, and invisible to every other test here. `naga` is the
    /// same front end `wgpu` uses, so parsing and validating it here catches
    /// exactly what the driver would.
    #[test]
    fn the_orb_shader_compiles() {
        let module = naga::front::wgsl::parse_str(super::SHADER).expect("WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("WGSL validates");
    }

    /// The uniform block is written straight into a GPU buffer, so Rust and WGSL
    /// have to agree on its size. WGSL rounds a struct up to its largest
    /// member's alignment — 16 for `vec4` — and a mismatch is not an error, just
    /// a wrong picture.
    #[test]
    fn the_uniform_block_matches_the_shader_layout() {
        let size = std::mem::size_of::<super::Uniforms>();
        assert_eq!(size % 16, 0, "{size} is not a multiple of vec4 alignment");
        // 4 × vec4 of colour, then 8 scalars (6 live + 2 pad) = 96 bytes.
        assert_eq!(size, 96);
    }
}
