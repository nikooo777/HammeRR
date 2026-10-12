import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check("get_component_sources" in tools, "Source brush pagination is discoverable")
    ids = [
        client.call(
            "create_block",
            {
                "mins": [index * 96, 0, 0],
                "maxs": [index * 96 + 64, 64, 64],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        for index in range(205)
    ]
    owner = client.call("tie_to_entity", {"ids": ids, "classname": "func_detail"})["id"]
    path = output / "source.vmf"
    client.call("save_map", {"path": str(path)})
    client.call("open_map", {"path": str(path)})
    client.call("select", {"ids": [owner]})
    original = detail(client, [owner])[0]
    history = client.call("get_map_info")["undo"]
    page = client.call("get_component_sources")
    snapshot = page["snapshot"]
    second = client.call("get_component_sources", {"offset": 100, "limit": 100})
    last = client.call("get_component_sources", {"offset": 200, "limit": 100})
    check(
        page["total"] == 205
        and page["inspection_limit"] == 16
        and len(page["sources"]) == 100
        and len(second["sources"]) == 100
        and len(last["sources"]) == 5,
        "Large owner: bounded source pages cover 205 brushes while geometry inspection stays at sixteen",
    )
    all_sources = page["sources"] + second["sources"] + last["sources"]
    check(
        [source["solid"] for source in all_sources]
        == [int(value.split(":")[1]) for value in ids],
        "Pages cover every child once in deterministic ID order",
    )
    check(
        all(
            source["owner"] == owner
            and source["material"] == "dev/dev_measuregeneric01"
            and source["bounds"]["maxs"][2] == 64
            for source in all_sources
        ),
        "Source records expose committed owners, materials and bounds",
    )
    client.call("get_brush_components", error=True)
    for limit in [0, 257]:
        client.call("get_component_sources", {"limit": limit}, error=True)
    check(
        client.call("get_component_sources", {"offset": 999999})["sources"] == [],
        "An out-of-range source page returns no records",
    )
    check(
        client.call("get_component_sources")["snapshot"] == snapshot
        and client.call("get_map_info")["undo"] == history
        and detail(client, [owner])[0] == original,
        "Queries and invalid page inputs preserve scope, snapshot, geometry and history",
    )
    chosen = last["sources"][0]["solid"]
    scoped = client.call(
        "set_component_picking",
        {"enabled": True, "kind": "face", "solids": [chosen], "snapshot": snapshot},
    )
    check(
        scoped["picking"]["solids"] == [chosen] and len(scoped["brushes"]) == 1,
        "Shared picking captures one child from the final source page",
    )
    sources = client.call("get_component_sources", {"offset": 200})
    check(
        sources["total"] == 205 and sources["active_solids"] == [chosen],
        "Capturing scope retains source owner choices and reports the active brush",
    )
    client.call(
        "set_component_picking",
        {
            "enabled": True,
            "kind": "face",
            "solids": [int(ids[0].split(":")[1])],
            "snapshot": snapshot,
        },
        error=True,
    )
    check(
        client.call("get_component_sources")["active_solids"] == [chosen],
        "Stale scope snapshots preserve the captured brush",
    )
    face = scoped["brushes"][0]["faces"][0]["side"]
    selection = client.call(
        "select_brush_components",
        {
            "components": [{"solid": chosen, "sides": [face]}],
            "snapshot": scoped["snapshot"],
        },
    )
    draft = client.call(
        "preview_component_move",
        {
            "delta": [0, 0, 8],
            "snap": False,
            "texture_lock": False,
            "snapshot": selection["snapshot"],
        },
    )
    check(
        client.call("get_component_sources", {"offset": 200})["sources"]
        == last["sources"]
        and detail(client, [owner])[0] == original,
        "Source choices keep committed bounds during the geometry preview",
    )
    client.call("commit_component_edit", {"token": draft["preview"]["token"]})
    changed = detail(client, [owner])[0]
    check(
        changed["brushes"][:200] == original["brushes"][:200]
        and changed["brushes"][201:] == original["brushes"][201:]
        and changed["brushes"][200] != original["brushes"][200]
        and changed["properties"] == original["properties"],
        "Applying the captured child edit preserves owner properties and all 204 siblings",
    )
    check(
        len(client.call("get_map_info")["undo"]) == len(history) + 1,
        "Child editing adds one undo step",
    )
    client.call("undo")
    check(detail(client, [owner])[0] == original, "Undo restores every child exactly")
    client.call(
        "set_component_picking",
        {
            "enabled": False,
            "snapshot": client.call("get_component_sources")["snapshot"],
        },
    )
    check(
        len(client.call("get_component_sources")["active_solids"]) == 205,
        "Stopping picking returns to the original owner scope without changing the map",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-component-sources")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output)
        result = {"passed": len(checks), "checks": checks}
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
