//! ADR 0006 Phase 0 spike. Throwaway.
//!
//! Two questions, no more: does `llama-cpp-2` build in this repo's toolchain
//! (and with an accelerator feature on), and what is tok/s against the Ollama
//! path we shell out to today. Prints the same two rates `ollama run --verbose`
//! prints, so the numbers compare directly.
//!
//! ```text
//! cargo run --release -- <model.gguf> [n_gpu_layers]
//! ```

use std::num::NonZeroU32;
use std::time::Instant;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

/// Same prompt every run, so the numbers are comparable across backends.
const PROMPT: &str = "Explain what a topological sort is, and why a task planner needs one.";
const N_PREDICT: i32 = 256;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model_path = args
        .next()
        .ok_or("usage: spike-llama <model.gguf> [n_gpu_layers]")?;
    // 0 is CPU-only. The point of the accelerator build is passing something
    // large here; 999 means "all of them" to llama.cpp.
    let n_gpu_layers: u32 = args.next().unwrap_or_else(|| "0".into()).parse()?;

    let backend = LlamaBackend::init()?;
    println!("backend: {}", accelerator());
    println!("model:   {model_path}");
    println!("ngl:     {n_gpu_layers}");

    let load_start = Instant::now();
    let model = LlamaModel::load_from_file(
        &backend,
        &model_path,
        &LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers),
    )?;
    println!(
        "load:    {:.0} ms",
        load_start.elapsed().as_secs_f64() * 1000.0
    );

    let tokens = model.str_to_token(PROMPT, AddBos::Always)?;
    let n_ctx = (tokens.len() as u32 + N_PREDICT as u32 + 64).max(512);
    let mut ctx = model.new_context(
        &backend,
        LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            // llama.cpp's own default is 4 threads regardless of the machine.
            .with_n_threads(threads())
            .with_n_threads_batch(threads()),
    )?;

    let mut batch = LlamaBatch::new(tokens.len().max(N_PREDICT as usize), 1);
    let last = tokens.len() as i32 - 1;
    for (i, token) in tokens.iter().enumerate() {
        // Only the final prompt token needs logits — the rest are just context.
        batch.add(*token, i as i32, &[0], i as i32 == last)?;
    }
    let prompt_start = Instant::now();
    ctx.decode(&mut batch)?;
    let prompt_ms = prompt_start.elapsed().as_secs_f64() * 1000.0;

    // Greedy: sampling strategy is not what is being measured, and it keeps the
    // two backends comparable without matching every sampler default.
    let mut sampler = LlamaSampler::chain_simple([LlamaSampler::greedy()]);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut pos = tokens.len() as i32;
    let mut out = String::new();
    let mut generated = 0;

    let eval_start = Instant::now();
    for _ in 0..N_PREDICT {
        let token = sampler.sample(&ctx, -1);
        if model.is_eog_token(token) {
            break;
        }
        out.push_str(&model.token_to_piece(token, &mut decoder, false, None)?);
        sampler.accept(token);

        batch.clear();
        batch.add(token, pos, &[0], true)?;
        pos += 1;
        ctx.decode(&mut batch)?;
        generated += 1;
    }
    let eval_ms = eval_start.elapsed().as_secs_f64() * 1000.0;

    // Wall clock, not `ctx.timings()`: llama.cpp's own counters come back zero
    // here because its perf instrumentation is off by default. Wall clock is
    // what `ollama run --verbose` effectively reports anyway, and it is the
    // number a user feels.
    println!(
        "prompt:  {} tokens, {prompt_ms:.0} ms, {:.2} tok/s",
        tokens.len(),
        tokens.len() as f64 / (prompt_ms / 1000.0),
    );
    println!(
        "eval:    {generated} tokens, {eval_ms:.0} ms, {:.2} tok/s",
        f64::from(generated) / (eval_ms / 1000.0),
    );
    println!("\n--- output ---\n{}", out.trim());
    Ok(())
}

fn threads() -> i32 {
    std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).max(1) as i32)
}

fn accelerator() -> &'static str {
    if cfg!(feature = "cuda") {
        "cuda"
    } else if cfg!(feature = "vulkan") {
        "vulkan"
    } else {
        "cpu"
    }
}
