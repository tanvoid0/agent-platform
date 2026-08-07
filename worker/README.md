# model-ops build worker

The LoRA fine-tuning pipeline. `agent-platformd` runs each stage here as a
subprocess — this is not a server and imports nothing from one.

    MODEL_OPS_PYTHON        interpreter to run stages with (the one with torch)
    MODEL_OPS_WORKER_PATH   this directory; defaults to `worker/` beside the exe
    MODEL_OPS_DATA_DIR      projects, adapters, logs
    CONFIG_DIR              where `config.yaml` lives
    OLLAMA_API_BASE         used by the `eval` stage

The server passes all but the first two. Stages report structured results by
printing `@@AGP:<kind>@@ {json}` lines, which the server parses out of the same
stream it tees into the job log — see `model_ops/registry_hook.py` and
`desktop/crates/server/src/model_ops.rs`.

Run a stage by hand:

    PYTHONPATH=. python -c "from model_ops.pipeline.train_lora import train; train('my-app')"
