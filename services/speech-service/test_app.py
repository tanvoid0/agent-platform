"""One check: the voice is loaded once, not per request.

That is the whole point of the in-process path — reloading it per sentence is
what made E.V. lag seconds behind its own text. Skipped when neither `piper-tts`
nor a voice is installed.
"""

import time

import pytest

from app import DEFAULT_VOICE, load_voice, synthesize, voice_path

pytest.importorskip("piper")


def test_second_synthesis_skips_the_model_load():
    try:
        model = voice_path(DEFAULT_VOICE)
    except Exception:
        pytest.skip(f"voice {DEFAULT_VOICE} not installed")

    load_voice.cache_clear()
    start = time.perf_counter()
    first = synthesize("Systems are nominal, boss.", model)
    cold = time.perf_counter() - start

    start = time.perf_counter()
    second = synthesize("All subsystems green.", model)
    warm = time.perf_counter() - start

    assert first and first[:4] == b"RIFF"
    assert second and second[:4] == b"RIFF"
    # The load is ~1.4 s and a sentence ~50 ms, so a cached voice is an order of
    # magnitude apart. Anything close to `cold` means it reloaded.
    assert warm < cold / 3, f"cold {cold:.2f}s, warm {warm:.2f}s — voice reloaded"
