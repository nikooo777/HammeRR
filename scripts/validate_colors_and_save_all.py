import argparse
import hashlib
import json
import os
import sys
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from validate_face_lighting import Client
from validate_recovery_exit import private_output_directory, require_clean_eof


def decoded(result):
    return json.loads(next(item["text"] for item in result["content"] if item["type"] == "text"))


def stamp(path):
    info = path.stat()
    return [info.st_ino, info.st_size, info.st_mtime_ns, hashlib.sha256(path.read_bytes()).hexdigest()]


def maps_manifest():
    directory = ROOT / "maps"
    return {str(path.relative_to(directory)): stamp(path) for path in sorted(directory.rglob("*")) if path.is_file() and not path.is_symlink()}


class Probe:
    def __init__(self, client, output):
        self.client, self.output, self.checks = client, output, []

    def check(self, condition, label, detail=None):
        assert condition, f"{label}: {detail}"
        self.checks.append(label)

    def call(self, document, tool, arguments=None):
        return self.client.call(tool, {**(arguments or {}), "document_id": document})

    def reject(self, tool, arguments):
        try:
            return self.client.call(tool, arguments, error=True)
        except RuntimeError as error:
            return str(error)

    def protected(self, document, entity):
        info = self.call(document, "get_map_info")
        return {
            "objects": self.call(document, "get_objects", {"ids": [entity]}),
            "history": self.call(document, "get_history"),
            "camera": self.call(document, "get_camera"),
            "selection": self.call(document, "get_selection"),
            "draft": self.call(document, "get_property_edit") if info["property_draft"] else None,
            "dirty": info["unsaved_changes"],
        }

    def color(self, document):
        fgd = self.output / "fixture.fgd"
        fgd.write_text('''@PointClass = color_fixture [
_light(color255) : "Light" : "20 40 80 450.5"
rendercolor(color255) : "Tint" : "20 40 80 099"
tint(color1) : "Normalized" : "0.1234567890 0.2500 0.75"
locked(color255) readonly : "Locked" : "255 255 255"
label(string) : "Label" : "initial"
]
''')
        self.call(document, "load_entity_definitions", {"path": str(fgd)})
        entity = self.call(document, "create_entity", {"classname": "color_fixture", "origin": [16, 32, 64]})["id"]
        self.call(document, "save_map", {"path": str(self.output / "first.vmf")})
        before = self.protected(document, entity)
        source = stamp(self.output / "first.vmf")
        for space, channels, expected in [
            ("hsv", [240, 100, 100], [0, 0, 255]),
            ("hsl", [210, 50, 50], [64, 128, 191]),
            ("hsv", [360, 100, 100], [255, 0, 0]),
            ("rgb1", [0.25, 0.5, 0.75], [64, 128, 191]),
        ]:
            converted = self.call(document, "convert_color", {"space": space, "channels": channels})
            self.check(converted["rgb255"] == expected, f"{space} {channels} matches a literal RGB reference", converted)
        gray = self.call(document, "convert_color", {"space": "rgb1", "channels": [0.125] * 3})
        self.check(gray["hsv"] == [0, 0, 12.5] and gray["hsl"] == [0, 0, 12.5], "Achromatic conversion has explicit zero hue")
        self.check(self.protected(document, entity) == before and stamp(self.output / "first.vmf") == source, "Conversions preserve source, history, selection, drafts and camera")
        cases = [
            ("_light", {"space": "hsv", "channels": [240, 100, 100]}, "0 0 255 450.5"),
            ("rendercolor", {"space": "hsl", "channels": [120, 100, 50]}, "0 255 0 099"),
            ("tint", {"space": "rgb1", "channels": [0.3141592653589793, 0.125, 0.875]}, "0.3141592653589793 0.125 0.875"),
        ]
        for key, color, expected in cases:
            history = self.call(document, "get_history")["undo_count"]
            result = self.call(document, "set_entity_color_space", {"id": entity, "key": key, "color": color})
            self.check(dict(result["properties"])[key] == expected, f"{key} preserves the required brightness, fourth component or normalized precision", result)
            self.check(self.call(document, "get_history")["undo_count"] == history + 1, f"{key} Apply adds exactly one Undo step")
            self.call(document, "undo")
            self.check(self.call(document, "get_objects", {"ids": [entity]}) == before["objects"], f"Undo restores exact authored {key} source")
        state = self.protected(document, entity)
        for arguments in [
            {"key": "tint", "color": {"space": "rgb1", "channels": [0, 0, 1.01]}},
            {"key": "_light", "color": {"space": "hsv", "channels": [361, 100, 50]}},
            {"key": "_light", "color": {"space": "rgb255", "channels": [12.5, 0, 0]}},
            {"key": "_light", "color": {"space": "hsl", "channels": [0, 100, 50]}, "intensity": -1},
            {"key": "rendercolor", "color": {"space": "hsl", "channels": [0, 100, 50]}, "intensity": 100},
            {"key": "locked", "color": {"space": "hsl", "channels": [0, 100, 50]}},
        ]:
            self.reject("set_entity_color_space", {"document_id": document, "id": entity, **arguments})
            self.check(self.protected(document, entity) == state, f"Invalid color Apply rejects atomically: {arguments}")
        legacy = self.call(document, "set_entity_color", {"id": entity, "key": "_light", "color": [255, 128, 0]})
        self.check(dict(legacy["properties"])["_light"] == "255 128 0 450.5", "Existing byte color API keeps brightness semantics")
        self.call(document, "undo")
        self.check(stamp(self.output / "first.vmf") == source, "Color editing and Undo never write the saved map")
        return entity

    def save_all(self, first, first_entity):
        second = self.client.call("new_document")["document_id"]
        second_entity = self.call(second, "create_entity", {"classname": "info_target", "origin": [-16, 0, 0], "properties": {"targetname": "peer", "label": "initial"}})["id"]
        self.call(first, "set_entity_properties", {"id": first_entity, "set": {"label": "round_one"}})
        peer = self.protected(second, second_entity)
        report = decoded(self.client.call("save_all_named_maps", error=True))
        rows = {row["document_id"]: row for row in report["documents"]}
        self.check(not report["complete"] and report["saved"] == 1 and rows[first]["status"] == "saved" and rows[second]["status"] == "needs_save_as", "Partial Save all saves the named map and reports the unnamed map", report)
        self.check(self.protected(second, second_entity) == peer, "Unnamed partial result preserves peer source, selection, history and camera")
        self.call(second, "save_map", {"path": str(self.output / "second.vmf")})
        for document, entity in [(first, first_entity), (second, second_entity)]:
            self.call(document, "set_entity_properties", {"id": entity, "set": {"label": "round_two"}})
        token = self.call(second, "begin_property_edit", {"ids": [second_entity]})["draft"]["token"]
        self.call(second, "update_property_edit", {"token": token, "changes": [{"action": "set", "key": "label", "value": "pending"}]})
        peer, peer_file = self.protected(second, second_entity), stamp(self.output / "second.vmf")
        report = decoded(self.client.call("save_all_named_maps", error=True))
        rows = {row["document_id"]: row for row in report["documents"]}
        self.check(rows[first]["status"] == "saved" and rows[second]["status"] == "pending_review" and not report["complete"], "Pending review blocks only its originating map", report)
        self.check(self.protected(second, second_entity) == peer and stamp(self.output / "second.vmf") == peer_file, "Partial Save all preserves the exact pending draft and peer file")
        self.call(second, "cancel_property_edit", {"token": token})
        self.call(first, "set_entity_properties", {"id": first_entity, "set": {"label": "round_three"}})
        preserved = {document: self.protected(document, entity) for document, entity in [(first, first_entity), (second, second_entity)]}
        report = self.client.call("save_all_named_maps")
        self.check(report["complete"] and report["saved"] == 2 and all(row["status"] == "saved" for row in report["documents"]), "Both dirty named maps save through the fixed workspace action", report)
        for document, entity in [(first, first_entity), (second, second_entity)]:
            after = self.protected(document, entity)
            self.check(not after["dirty"] and after["objects"] == preserved[document]["objects"] and after["camera"] == preserved[document]["camera"] and after["selection"] == preserved[document]["selection"] and after["history"]["undo_count"] == preserved[document]["history"]["undo_count"], "Save all preserves editing state and Undo count while clearing dirty state")
        paths = [self.output / "first.vmf", self.output / "second.vmf"]
        files = [stamp(path) for path in paths]
        histories = [self.call(document, "get_history") for document in [first, second]]
        report = self.client.call("save_all_named_maps")
        self.check(report["complete"] and report["saved"] == 0 and report["unchanged"] == 2 and files == [stamp(path) for path in paths] and histories == [self.call(document, "get_history") for document in [first, second]], "Clean retry verifies sources without rewriting files, timestamps or history", report)
        self.reject("save_all_named_maps", {"document_id": first})
        self.check(files == [stamp(path) for path in paths], "Save all rejects a document selector without writing files")
        paths[0].write_bytes(paths[0].read_bytes() + b"\n// private external change\n")
        changed = stamp(paths[0])
        report = decoded(self.client.call("save_all_named_maps", error=True))
        rows = {row["document_id"]: row for row in report["documents"]}
        self.check(not report["complete"] and rows[first]["status"] == "error" and rows[second]["status"] == "unchanged" and stamp(paths[0]) == changed, "External changes to a clean source are explicit per-map failures and remain untouched", report)
        self.call(first, "open_map", {"path": str(paths[0]), "discard_changes": True})
        listed = self.client.call("list_documents")
        self.check(all(row["visibility"] == "headless" for row in listed["documents"]), "Workspace operations never create or present desktop windows", listed)
        self.check(all(not self.call(document, "get_map_info")["rendering"]["initialized"] for document in [first, second]), "Color and Save all workflows remain CPU-only")

    def run(self):
        listed = self.client.call("list_documents")
        first = listed["documents"][0]["document_id"]
        schemas = {tool["name"]: tool["inputSchema"] for tool in self.client.rpc("tools/list", {})["tools"]}
        self.check({"convert_color", "set_entity_color_space", "save_all_named_maps"} <= schemas.keys(), "Both slices are discoverable in the production MCP service")
        first_entity = self.color(first)
        self.save_all(first, first_entity)
        return self.checks


def main():
    parser = argparse.ArgumentParser(description="Private actual-MCP color and Save all acceptance for both production binaries.")
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
            with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(directory / "recent"), "HAMMERR_BUILD_RECIPES": str(directory / "recipes.json"), "HAMMERR_PREFABS_DIR": str(directory / "prefabs"), "XDG_CONFIG_HOME": str(directory / "config"), "XDG_STATE_HOME": str(directory / "state")}):
                client = Client(None, binary.resolve(), directory, gameinfo)
            try:
                checks = Probe(client, directory).run()
                reports.append({"binary": str(binary.resolve()), "checks": len(checks), "passed": checks})
            finally:
                require_clean_eof(client)
    finally:
        assert maps_manifest() == original_maps, "Repository maps/ changed during private acceptance"
    (output / "validation.json").write_text(json.dumps({"binaries": reports, "repository_maps_preserved": True}, indent=2) + "\n")
    print(json.dumps({"binaries": len(reports), "checks": sum(report["checks"] for report in reports), "output": str(output)}))


if __name__ == "__main__":
    main()
