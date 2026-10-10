from datetime import timedelta

import pytest

from inventory.durations import parse_duration


@pytest.mark.parametrize(
    "text, expected",
    [
        ("15", timedelta(minutes=15)),
        ("15m", timedelta(minutes=15)),
        ("2h", timedelta(hours=2)),
        ("1d", timedelta(days=1)),
        ("1h30m", timedelta(minutes=90)),
        (" 2H ", timedelta(hours=2)),
    ],
)
def test_parses_minutes_hours_days_and_compounds(text, expected):
    assert parse_duration(text) == expected


@pytest.mark.parametrize("text", ["", "m", "15s", "1.5h", "-5m", "h1"])
def test_rejects_what_is_not_a_duration(text):
    with pytest.raises(ValueError, match="not a duration"):
        parse_duration(text)


@pytest.mark.parametrize("text", ["0", "0m", "0h0m"])
def test_rejects_zero(text):
    with pytest.raises(ValueError, match="positive"):
        parse_duration(text)
