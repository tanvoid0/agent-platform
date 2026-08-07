"""Unit tests for leaked tool-call recovery."""

from coder.tool_call_parse import parse_leaked_tool_calls, strip_leaked_tool_syntax


def test_strip_leaked_tool_syntax_removes_tag():
    assert strip_leaked_tool_syntax("I'll read. <function=read_file>") == "I'll read."


def test_parse_leaked_tool_calls_with_json_args():
    calls = parse_leaked_tool_calls(
        'Reading <function=read_file>{"path": "app.py"}</function>'
    )
    assert len(calls) == 1
    assert calls[0]["name"] == "read_file"
    assert calls[0]["arguments"] == {"path": "app.py"}


def test_parse_leaked_tool_calls_bare_tag():
    calls = parse_leaked_tool_calls("Explore. <function=list_dir>")
    assert len(calls) == 1
    assert calls[0]["name"] == "list_dir"
    assert calls[0]["arguments"] == {}


def test_parse_leaked_tool_calls_ignores_unknown_tools():
    assert parse_leaked_tool_calls("<function=delete_everything>") == []


def test_parse_leaked_tool_calls_covers_every_tool_spec():
    """`KNOWN_TOOLS` drifted behind `TOOL_SPECS` once: `search` and `repo_map`
    were added to the executors and not here, so those leaked calls were dropped
    while their markup was stripped from the answer. Assert against the spec list
    rather than a second hand-written set, so the next tool cannot drift."""
    from coder.executor import TOOL_SPECS
    from coder.tool_call_parse import KNOWN_TOOLS

    assert KNOWN_TOOLS == {spec["function"]["name"] for spec in TOOL_SPECS}

    calls = parse_leaked_tool_calls('<function=search>{"pattern": "TODO"}</function>')
    assert [c["name"] for c in calls] == ["search"]
    assert calls[0]["arguments"] == {"pattern": "TODO"}
