"""What `parse_decision_response` hands the user.

Its `thought` is not internal: `review_service` stores it as the review summary
and `assistant_chat` may speak it as the assistant's own reply. So a raw model
data structure reaching that field is a visible defect, not a parsing nicety —
it showed up in the app as a review banner reading ```json {"reasoning": …
truncated mid-sentence.
"""

from action_orchestrator.engine import parse_decision_response


JSON_ANSWER = """```json
{
  "reasoning": "There are no items on the board yet, so there is nothing to adjust.",
  "actions": [
    {"action_id": "create_item", "name": "Add to your board",
     "parameters": {"title": "Weekly review"}, "confidence": 0.8}
  ]
}
```"""


def test_a_fenced_json_answer_yields_its_reasoning_and_its_actions():
    parsed = parse_decision_response(JSON_ANSWER)
    assert parsed["thought"].startswith("There are no items")
    assert "```" not in parsed["thought"]
    assert [a.action_id for a in parsed["actions"]] == ["create_item"]
    assert parsed["actions"][0].parameters == {"title": "Weekly review"}


def test_an_action_without_an_id_is_dropped_rather_than_named_from_its_label():
    # This path does not check ids against the action set, so a guessed one
    # would travel as a real proposal.
    parsed = parse_decision_response(
        '{"reasoning": "hi", "actions": [{"name": "Add to your board"}]}'
    )
    assert parsed["actions"] == []
    assert parsed["thought"] == "hi"


def test_unreadable_machine_output_leaves_no_thought_at_all():
    # Truncated JSON: unparseable, and its head must not become chat copy.
    parsed = parse_decision_response('```json\n{"reasoning": "cut off here')
    assert parsed["thought"] is None
    assert parsed["actions"] == []


def test_prose_still_falls_back_to_the_response_itself():
    parsed = parse_decision_response("Nothing needs changing this week.")
    assert parsed["thought"] == "Nothing needs changing this week."


def test_the_tagged_format_still_wins_where_it_is_used():
    parsed = parse_decision_response(
        "<reasoning>Two habits slipped.</reasoning>\n"
        '<actions>[{"action_id": "create_item", "name": "Add"}]</actions>'
    )
    assert parsed["thought"] == "Two habits slipped."
    assert [a.action_id for a in parsed["actions"]] == ["create_item"]
