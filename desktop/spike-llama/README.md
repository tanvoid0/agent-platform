# spike-llama

Throwaway harness for the [ADR 0006](../../docs/adr/0006-in-process-rust-core.md)
Phase 0 spike: does `llama-cpp-2` build here, and what is tok/s against the Ollama
path the app shells out to today. **Results and conclusions live in the ADR** — this
file is only how to rerun it.

Excluded from the desktop workspace (`exclude` in [../Cargo.toml](../Cargo.toml)), so
a plain `cargo build` in `desktop/` never drags llama.cpp — or a CUDA toolchain —
into the app. Delete the directory and that one `exclude` line when the ADR closes.

## Run it

Any GGUF works. The easiest one is whatever Ollama already downloaded — it prints
the blob path:

```bash
ollama show --modelfile llama3.1:8b | head -1
```

```bash
cargo run --release -- <path-to.gguf> 0
```

The second argument is `n_gpu_layers`: `0` is CPU-only, `999` is everything on the
GPU. It only does anything on an accelerator build.

llama.cpp logs to stderr from C and there is no public hook in the binding to quiet
it, so send stderr to `/dev/null` if the numbers are all you want.

## Accelerator builds

Each needs its own SDK installed, not just a driver.

```bash
cargo build --release --features cuda      # needs the CUDA Toolkit (nvcc), not just the driver
cargo build --release --features vulkan    # needs VULKAN_SDK set
```

`vulkan` goes through a direct `llama-cpp-sys-2` dependency because `llama-cpp-2`
forwards `cuda`/`metal`/`rocm`/`opencl` and not `vulkan`.

## The Ollama baseline

Match the context — Ollama defaults to the model's full trained context, which on an
8B here means 23 GB and a partial CPU spill. That is a 5× difference on its own.

```bash
curl -s http://127.0.0.1:11434/api/generate -d '{"model":"llama3.1:8b","prompt":"Explain what a topological sort is, and why a task planner needs one.","stream":false,"options":{"num_predict":256,"temperature":0,"top_k":1,"num_ctx":2048}}'
```

`eval_count / eval_duration` (ns) is the tok/s to compare. `ollama ps` shows the
CPU/GPU split that explains it.
