import pytest

from inventory.cli import main


@pytest.fixture
def inv(tmp_path):
    state = tmp_path / "inventory.json"

    def run(*args: str, now: str = "2026-03-02T09:00:00") -> int:
        return main(["--state", str(state), "--now", now, *args])

    return run


def test_reserve_and_status(inv, capsys):
    assert inv("add-item", "BOLT-M6", "M6 hex bolt") == 0
    assert inv("receive", "BOLT-M6", "10") == 0
    assert inv("reserve", "BOLT-M6", "4", "--ttl", "15") == 0
    capsys.readouterr()

    assert inv("status") == 0
    lines = capsys.readouterr().out.splitlines()
    assert lines[1].split() == ["BOLT-M6", "10", "4", "6"]


def test_reserve_prints_id_and_expiry(inv, capsys):
    inv("add-item", "BOLT-M6", "M6 hex bolt")
    inv("receive", "BOLT-M6", "10")
    capsys.readouterr()
    inv("reserve", "BOLT-M6", "2", "--ttl", "15")
    assert capsys.readouterr().out.strip() == "R0001: 2 x BOLT-M6 until 2026-03-02T09:15:00"


def test_errors_exit_non_zero(inv, capsys):
    inv("add-item", "BOLT-M6", "M6 hex bolt")
    assert inv("reserve", "BOLT-M6", "1") == 1
    assert "only 0 available" in capsys.readouterr().err


def test_status_for_unknown_item_fails(inv, capsys):
    assert inv("status", "SCREW-M3") == 1
    assert "unknown item" in capsys.readouterr().err
