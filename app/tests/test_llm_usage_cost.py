"""usage_cost_from_completion_response parses LiteLLM / OpenRouter style payloads."""

from llm_client import usage_cost_from_completion_response


def test_no_usage_returns_zero():
    assert usage_cost_from_completion_response({}) == 0.0


def test_usage_cost_field():
    data = {
        "choices": [{"message": {"content": "x"}}],
        "usage": {"total_tokens": 10, "cost": 0.00042},
    }
    assert usage_cost_from_completion_response(data) == 0.00042


def test_usage_total_cost_alias():
    data = {
        "usage": {"total_tokens": 5, "total_cost": 0.01},
    }
    assert usage_cost_from_completion_response(data) == 0.01


def test_usage_response_cost_object():
    data = {
        "usage": {
            "total_tokens": 3,
            "response_cost": {"prompt_cost": 0.001, "completion_cost": 0.002, "total_cost": 0.003},
        }
    }
    assert usage_cost_from_completion_response(data) == 0.003


def test_top_level_response_cost_dict():
    data = {"response_cost": {"total_cost": 0.5}}
    assert usage_cost_from_completion_response(data) == 0.5


def test_hidden_params_litellm():
    data = {"_hidden_params": {"response_cost": 0.00007}}
    assert usage_cost_from_completion_response(data) == 0.00007
