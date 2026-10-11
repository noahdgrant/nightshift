import json

from inventory.cli import main


def test_status_json_lists_items_in_sku_order(tmp_path, capsys):
    state = tmp_path / "inventory.json"

    def inv(*args: str) -> int:
        return main(["--state", str(state), "--now", "2026-03-02T09:00:00", *args])

    inv("add-item", "NUT-M6", "M6 hex nut")
    inv("add-item", "BOLT-M6", "M6 hex bolt")
    capsys.readouterr()

    assert inv("status", "--json") == 0
    assert [e["sku"] for e in json.loads(capsys.readouterr().out)] == ["BOLT-M6", "NUT-M6"]
