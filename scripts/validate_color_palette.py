import argparse
import json
import os
import sys
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from validate_face_lighting import Client
from validate_colors_and_save_all import maps_manifest, stamp
from validate_recovery_exit import private_output_directory, require_clean_eof

PRECISE = [0.12345678901234566, 0.25000000000000006, 0.9876543210987654]


class Probe:
    def __init__(self, binary, directory, gameinfo):
        self.binary, self.directory, self.gameinfo = binary, directory, gameinfo
        self.checks = []
        self.clients = []

    def check(self, condition, label, detail=None):
        assert condition, f"{label}: {detail}"
        self.checks.append(label)

    def start(self, label):
        client = Client(None, self.binary, self.directory, self.gameinfo)
        (self.directory / "editor.log").rename(self.directory / f"{label}.log")
        self.clients.append(client)
        return client

    def action(self, client, kind, name=None, color=None, snapshot=None, error=False, document=None, new_name=None):
        args = {"action": kind, "snapshot": snapshot or client.call("get_color_palette")["snapshot"]}
        if name is not None:
            args["name"] = name
        if color is not None:
            args["color"] = color
        if new_name is not None:
            args["new_name"] = new_name
        if document:
            args["document_id"] = document
        return client.call("set_color_palette", args, error=error)

    def run(self):
        first = self.start("first")
        schemas = {row["name"]: row["inputSchema"] for row in first.rpc("tools/list", {})["tools"]}
        self.check({"get_color_palette", "set_color_palette"} <= schemas.keys(), "Production MCP exposes both palette tools")
        initial = first.call("get_color_palette")
        self.check(initial["ready"] and initial["entries"] == [] and not initial["stored"], "Missing palette is an intentional empty usable palette", initial)
        palette = Path(initial["path"])
        saved = self.action(first, "save", "Precise", {"space": "rgb1", "channels": PRECISE})
        self.check(saved["ready"] and saved["write_committed"] and saved["entries"][0]["rgb1"] == PRECISE, "Explicit Save stores exact normalized RGB values", saved)
        source = stamp(palette)
        self.action(first, "save", "Precise", {"space": "rgb255", "channels": [0, 0, 0]}, error=True)
        self.action(first, "save", "Invalid", {"space": "hsv", "channels": [361, 0, 0]}, error=True)
        self.action(first, "replace", "Missing", {"space": "rgb1", "channels": PRECISE}, error=True)
        self.check(stamp(palette) == source, "Duplicate Save and invalid Replace/input leave the exact saved source untouched")
        fgd = self.directory / "palette.fgd"
        fgd.write_text('@PointClass = palette_fixture [\ntint(color1) : "Tint" : "0.2 0.3 0.4"\n_light(color255) : "Light" : "20 40 80 450.5"\n]\n')
        first.call("load_entity_definitions", {"path": str(fgd)})
        entity = first.call("create_entity", {"classname": "palette_fixture", "origin": [0, 0, 0]})["id"]
        first.call("save_map", {"path": str(self.directory / "palette.vmf")})
        token = first.call("begin_property_edit", {"ids": [entity]})["draft"]["token"]
        before = {
            "objects": first.call("get_objects", {"ids": [entity]}),
            "history": first.call("get_history"),
            "draft": first.call("get_property_edit"),
            "camera": first.call("get_camera"),
            "selection": first.call("get_selection"),
            "source": stamp(self.directory / "palette.vmf"),
        }
        chosen = self.action(first, "use", "Precise")
        self.check(chosen["color"]["rgb1"] == PRECISE, "Use returns the exact checked normalized color")
        self.action(first, "save", "Draft survives Cancel", {"space": "hsl", "channels": [210, 50, 50]})
        stale_snapshot = first.call("get_color_palette")["snapshot"]
        renamed = self.action(first, "rename", "Precise", new_name="Renamed precise")
        self.check(len(renamed["entries"]) == 2 and next(row for row in renamed["entries"] if row["name"] == "Renamed precise")["rgb1"] == PRECISE and all(row["name"] != "Precise" for row in renamed["entries"]), "Rename preserves exact normalized color and entry count in one explicit mutation", renamed)
        renamed_source = stamp(palette)
        for old, new in [("Renamed precise", "Draft survives Cancel"), ("Renamed precise", " invalid "), ("Missing", "Unused"), ("Renamed precise", "Renamed precise")]:
            self.action(first, "rename", old, new_name=new, error=True)
        self.action(first, "rename", "Renamed precise", new_name="Stale", snapshot=stale_snapshot, error=True)
        self.action(first, "rename", "Renamed precise", {"space": "rgb1", "channels": [0, 0, 0]}, new_name="Rejected color", error=True)
        self.check(stamp(palette) == renamed_source, "Invalid, duplicate, missing, unchanged, stale and color-bearing Rename requests never rewrite storage")
        after = {
            "objects": first.call("get_objects", {"ids": [entity]}),
            "history": first.call("get_history"),
            "draft": first.call("get_property_edit"),
            "camera": first.call("get_camera"),
            "selection": first.call("get_selection"),
            "source": stamp(self.directory / "palette.vmf"),
        }
        self.check(after == before, "Use, Save and Rename preserve map, pending draft, history, camera, selection and saved source")
        first.call("cancel_property_edit", {"token": token})
        self.check(len(first.call("get_color_palette")["entries"]) == 2, "Cancelling the map property draft keeps explicit palette Save and Rename writes")
        normalized = first.call("set_entity_color_space", {"id": entity, "key": "tint", "color": {"space": "rgb1", "channels": chosen["color"]["rgb1"]}})
        self.check(dict(normalized["properties"])["tint"] == " ".join(map(str, PRECISE)), "Existing Apply keeps exact normalized palette precision", normalized)
        first.call("undo")
        lit = first.call("set_entity_color_space", {"id": entity, "key": "_light", "color": {"space": "rgb1", "channels": chosen["color"]["rgb1"]}})
        self.check(dict(lit["properties"])["_light"].endswith(" 450.5"), "Applying a saved light color preserves authored brightness", lit)
        first.call("undo")
        second = first.call("new_document")["document_id"]
        second_state = first.call("get_color_palette", {"document_id": second})
        self.check(second_state["entries"] == first.call("get_color_palette", {"document_id": first.call("list_documents")["documents"][0]["document_id"]})["entries"], "Both managed documents share the same Application palette")
        first_document = next(row["document_id"] for row in first.call("list_documents")["documents"] if row["document_id"] != second)
        review = first.call("get_map_info", {"document_id": second})["document_close"]["review_token"]
        first.call("close_document", {"document_id": second, "review_token": review, "discard_changes": True})
        self.check(first.call("get_color_palette", {"document_id": first_document})["ready"], "Palette remains available after closing its peer document")
        require_clean_eof(first)
        restarted = self.start("restarted")
        restarted_entries = restarted.call("get_color_palette")["entries"]
        self.check(restarted_entries[1]["name"] == "Renamed precise" and restarted_entries[1]["rgb1"] == PRECISE, "A fresh production process reloads the renamed entry with exact normalized color")
        peer = self.start("independent-writer")
        self.action(peer, "save", "Peer", {"space": "rgb255", "channels": [255, 128, 0]})
        peer_source = stamp(palette)
        self.action(restarted, "rename", "Renamed precise", new_name="Stale external rename", error=True)
        self.check(not restarted.call("get_color_palette")["ready"] and stamp(palette) == peer_source, "Independent writer blocks stale Rename until explicit Reload")
        self.action(restarted, "replace", "Renamed precise", {"space": "rgb255", "channels": [0, 0, 0]}, error=True)
        blocked = restarted.call("get_color_palette")
        self.check(not blocked["ready"] and blocked["error"] and stamp(palette) == peer_source, "Independent writer invalidates the inspected source and cannot be overwritten by stale Replace", blocked)
        self.action(restarted, "use", "Renamed precise", error=True)
        self.action(restarted, "reload")
        self.check(len(restarted.call("get_color_palette")["entries"]) == 3, "Explicit Reload admits the independent writer's colors")
        self.action(restarted, "replace", "Renamed precise", {"space": "hsv", "channels": [240, 100, 100]})
        self.check(self.action(restarted, "use", "Renamed precise")["color"]["rgb255"] == [0, 0, 255], "Explicit Replace updates an existing name")
        self.action(restarted, "delete", "Peer")
        for index in range(14):
            self.action(restarted, "save", f"Slot {index:02}", {"space": "rgb1", "channels": PRECISE})
        bounded = stamp(palette)
        self.action(restarted, "save", "Overflow", {"space": "rgb1", "channels": PRECISE}, error=True)
        self.check(len(restarted.call("get_color_palette")["entries"]) == 16 and stamp(palette) == bounded, "All 16 entries are reported and a seventeenth write is rejected without touching storage")
        full = self.action(restarted, "rename", "Slot 00", new_name="A renamed full palette")
        self.check(len(full["entries"]) == 16 and full["entries"][0]["name"] == "A renamed full palette" and full["entries"][0]["rgb1"] == PRECISE, "Rename works at the 16-entry limit without changing the saved color")
        valid = palette.read_bytes()
        palette.write_text("broken")
        self.action(restarted, "reload", error=True)
        self.action(restarted, "rename", "A renamed full palette", new_name="Unavailable rename", error=True)
        self.action(restarted, "save", "Repair must be explicit", {"space": "rgb1", "channels": PRECISE}, error=True)
        self.check(not restarted.call("get_color_palette")["ready"] and palette.read_text() == "broken", "Malformed storage is unavailable and cannot be silently replaced")
        broken_process = self.start("malformed-startup")
        broken = broken_process.call("get_color_palette")
        self.check(not broken["ready"] and broken["entries"] == [] and broken["error"], "Fresh-process malformed startup reports unavailable, never a successful empty load", broken)
        palette.write_bytes(valid)
        self.action(restarted, "reload")
        self.check(restarted.call("get_color_palette")["ready"], "External repair plus explicit Reload restores availability")
        final_process = self.start("renamed-full-restart")
        final_entries = final_process.call("get_color_palette")["entries"]
        self.check(len(final_entries) == 16 and final_entries[0]["name"] == "A renamed full palette" and final_entries[0]["rgb1"] == PRECISE, "A fresh process preserves full-palette Rename and normalized precision")
        self.check(not restarted.call("get_map_info")["rendering"]["initialized"], "Palette workflows remain CPU-only")
        return self.checks

    def close(self):
        for client in self.clients:
            if client.process is not None:
                require_clean_eof(client)


def main():
    parser = argparse.ArgumentParser(description="Private production-MCP persistent palette acceptance for both binaries.")
    parser.add_argument("--binary", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = private_output_directory(parser, args.output)
    original_maps = maps_manifest()
    reports = []
    try:
        for index, binary in enumerate(args.binary):
            directory = output / f"{index + 1}-{binary.name}"
            directory.mkdir()
            game = directory / "game"
            game.mkdir()
            gameinfo = game / "gameinfo.txt"
            gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
            probe = Probe(binary.resolve(), directory, gameinfo)
            try:
                with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(directory / "recent"), "HAMMERR_BUILD_RECIPES": str(directory / "recipes.json"), "HAMMERR_PREFABS_DIR": str(directory / "prefabs"), "XDG_CONFIG_HOME": str(directory / "config"), "XDG_STATE_HOME": str(directory / "state")}):
                    checks = probe.run()
                reports.append({"binary": str(binary.resolve()), "checks": len(checks), "passed": checks})
            finally:
                probe.close()
    finally:
        assert maps_manifest() == original_maps, "Repository maps/ changed during private acceptance"
    (output / "validation.json").write_text(json.dumps({"binaries": reports, "repository_maps_preserved": True}, indent=2) + "\n")
    print(json.dumps({"binaries": len(reports), "checks": sum(report["checks"] for report in reports), "output": str(output)}))


if __name__ == "__main__":
    main()
