"""Register trained Ollama tags as config.yaml aliases."""

from __future__ import annotations


import yaml

from llm_proxy.core.config_cache import load_config_yaml_dict, resolved_config_yaml_path


def register_ollama_alias(alias: str, ollama_model: str) -> None:
    """Add or update an Ollama model alias in config.yaml."""
    path = resolved_config_yaml_path()
    data = load_config_yaml_dict()
    providers = data.get("providers")
    if not isinstance(providers, list):
        providers = []
        data["providers"] = providers

    ollama_block = None
    for p in providers:
        if isinstance(p, dict) and p.get("name") == "ollama":
            ollama_block = p
            break
    if ollama_block is None:
        ollama_block = {"name": "ollama", "models": []}
        providers.append(ollama_block)

    models = ollama_block.setdefault("models", [])
    if not isinstance(models, list):
        models = []
        ollama_block["models"] = models

    for m in models:
        if isinstance(m, dict) and m.get("model_name") == alias:
            m["model"] = ollama_model
            break
    else:
        models.append({"model_name": alias, "model": ollama_model})

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.safe_dump(data, sort_keys=False), encoding="utf-8")
