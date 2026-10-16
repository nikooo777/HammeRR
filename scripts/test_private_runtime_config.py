import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import private_runtime_config as private_runtime
import validate_native_runtime as runtime
from private_runtime_config import reject_configuration_writes


class PrivateRuntimeGuards(unittest.TestCase):
    def test_readonly_launch_keeps_host_paths_readonly_and_tmp_private(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory).resolve()
            native = ["/installed/cstrike_linux64", "-default", "-insecure"]
            environment = {"TMPDIR": "/host/tmp", "DISPLAY": ":private"}
            command, isolated = private_runtime.private_readonly_launch(
                native, output, environment
            )
            self.assertEqual(command, [
                "/usr/bin/bwrap", "--die-with-parent", "--ro-bind", "/", "/",
                "--dev-bind", "/dev", "/dev", "--proc", "/proc",
                "--bind", str(output), str(output), "--", *native,
            ])
            self.assertEqual(isolated["DISPLAY"], ":private")
            self.assertEqual(isolated["TMPDIR"], str(output / "tmp"))
            self.assertEqual((output / "tmp").stat().st_mode & 0o777, 0o700)
            self.assertEqual(environment["TMPDIR"], "/host/tmp")
            with self.assertRaises(FileExistsError):
                private_runtime.private_readonly_launch(native, output, environment)

    def test_identity_preflight_rejects_uploads_and_log_rotation(self):
        with tempfile.TemporaryDirectory() as directory:
            steam = Path(directory) / "steam"
            root = steam / "steamapps/common/installed"
            game = Path(directory) / "private/cstrike"
            game.mkdir(parents=True)
            log = steam / "logs/cloud_log.txt"
            log.parent.mkdir(parents=True)
            initial = b"[AppID 240] Earlier synchronized state\n"
            identity = {"steam_id64": "76561197960265745", "account": "17"}
            for event in [
                "Need to upload file",
                "HTTP upload",
                "Upload batch initiated",
                "Upload OK",
                None,
            ]:
                with self.subTest(event=event):
                    log.write_bytes(initial)

                    def identify(actual_root, actual_game, event=event):
                        self.assertEqual((actual_root, actual_game), (root, game))
                        log.write_bytes(
                            initial + f"[AppID 240] {event}\n".encode()
                            if event is not None
                            else b""
                        )
                        return identity

                    with (
                        patch.object(
                            private_runtime, "live_steam_identity", side_effect=identify
                        ),
                        patch.object(
                            private_runtime, "preservation_snapshot", return_value={}
                        ),
                        self.assertRaisesRegex(RuntimeError, "identity preflight"),
                    ):
                        private_runtime.begin_private_launch(root, game)
                    self.assertFalse(
                        (game.parent / "private-preservation-before.json").exists()
                    )

    def test_identity_preflight_keeps_earliest_log_cursor_for_final_audit(self):
        with tempfile.TemporaryDirectory() as directory:
            steam = Path(directory) / "steam"
            root = steam / "steamapps/common/installed"
            game = Path(directory) / "private/cstrike"
            game.mkdir(parents=True)
            log = steam / "logs/cloud_log.txt"
            log.parent.mkdir(parents=True)
            initial = b"[AppID 240] Upload OK before validation\n"
            log.write_bytes(initial)
            config = steam / "userdata/17/240/remote/cfg/config.cfg"
            config.parent.mkdir(parents=True)
            config.write_text('bind "w" "+forward"\n')
            identity = {"steam_id64": "76561197960265745", "account": "17"}
            snapshot = {
                "files": {"config.cfg": "settled configuration"},
                "cloud": {"sync": "synchronized"},
                "steam": str(steam),
                "account": "17",
                "root": str(root),
            }

            def identify(actual_root, actual_game):
                self.assertEqual((actual_root, actual_game), (root, game))
                with log.open("ab") as stream:
                    stream.write(b"[AppID 999] Upload OK\n[AppID 240] Synchronized\n")
                return identity

            def settled_snapshot(actual_root, actual_steam, account, *, strict=True):
                self.assertEqual(
                    (actual_root, actual_steam, account), (root, steam, "17")
                )
                return {**snapshot, "cloud_log_size": log.stat().st_size}

            with (
                patch.object(
                    private_runtime, "live_steam_identity", side_effect=identify
                ),
                patch.object(
                    private_runtime,
                    "preservation_snapshot",
                    side_effect=settled_snapshot,
                ),
            ):
                private_runtime.begin_private_launch(root, game)
                before = json.loads(
                    (game.parent / "private-preservation-before.json").read_text()
                )
                self.assertEqual(before["cloud_log_size"], len(initial))
                self.assertEqual(before["files"], snapshot["files"])
                self.assertEqual(before["cloud"], snapshot["cloud"])
                self.assertEqual(before["steam_identity"], identity)
                with log.open("ab") as stream:
                    stream.write(b"[AppID 240] Upload batch initiated after launch\n")
                with (
                    patch.object(
                        private_runtime.time, "monotonic", side_effect=[0, 0, 6]
                    ),
                    patch.object(private_runtime.time, "sleep"),
                    self.assertRaisesRegex(RuntimeError, "uploaded Cloud data"),
                ):
                    private_runtime.finish_private_launch(game)
            report = json.loads((game.parent / "private-preservation.json").read_text())
            self.assertFalse(report["success"])
            self.assertEqual(report["before"]["cloud_log_size"], len(initial))
            self.assertEqual(
                report["app240_cloud_uploads"],
                ["[AppID 240] Upload batch initiated after launch"],
            )

    def test_imported_config_cannot_chain_quoted_commands(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "config.cfg"
            path.write_text('bind "w" "+forward"\nmat_hdr_level "0"\n')
            reject_configuration_writes(path, imported=True)
            for command in [
                '"exec" "user.cfg"',
                '"alias" "save" "host_writeconfig profile"',
                '"stuffcmds"',
            ]:
                with self.subTest(command=command):
                    path.write_text(command + "\n")
                    with self.assertRaises(ValueError):
                        reject_configuration_writes(path, imported=True)

    def test_installed_output_is_rejected_before_config_write(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "installed"
            cfg = root / "cstrike/cfg/config.cfg"
            cfg.parent.mkdir(parents=True)
            cfg.write_bytes(b"preserve installed preferences\n")
            with self.assertRaises(ValueError):
                runtime.private_game(root, root)
            self.assertEqual(cfg.read_bytes(), b"preserve installed preferences\n")

    def test_private_cfg_symlink_is_rejected_before_config_write(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "installed"
            cfg = root / "cstrike/cfg/config.cfg"
            cfg.parent.mkdir(parents=True)
            cfg.write_bytes(b"preserve installed preferences\n")
            output = Path(directory) / "private"
            game = output / "cstrike"
            game.mkdir(parents=True)
            (game / "gameinfo.txt").write_text(
                '"GameInfo" { game "HammeRR validation" }'
            )
            (game / "cfg").symlink_to(cfg.parent, target_is_directory=True)
            with self.assertRaises(ValueError):
                runtime.private_game(root, output)
            self.assertEqual(cfg.read_bytes(), b"preserve installed preferences\n")

    def test_timeout_audits_after_group_cleanup_and_keeps_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / "private-preservation-before.json").write_text("{}")
            process = Mock(pid=12345)
            process.wait.side_effect = subprocess.TimeoutExpired("private worker", 1)
            sequence = []

            def drain(child):
                self.assertIs(child, process)
                sequence.append("drain")

            def audit(game, *, observer):
                self.assertEqual(game, output / "cstrike")
                self.assertEqual(observer, "compositor")
                sequence.append("audit")
                raise RuntimeError("preservation failed")

            with (
                patch.object(runtime.subprocess, "Popen", return_value=process),
                patch.object(runtime, "drain_private_group", side_effect=drain),
                patch.object(runtime, "finish_private_launch", side_effect=audit),
                patch.object(runtime, "private_group_members", return_value=[]),
                self.assertRaises(subprocess.TimeoutExpired) as caught,
            ):
                runtime.private_compositor(["private worker"], output, timeout=1)
            self.assertEqual(sequence, ["drain", "audit"])
            self.assertIn("preservation failed", " ".join(caught.exception.__notes__))
            report = json.loads((output / "private-cleanup.json").read_text())
            self.assertEqual(report["worker_failure"]["type"], "TimeoutExpired")
            self.assertEqual(
                report["cleanup_failures"][0]["message"], "preservation failed"
            )


if __name__ == "__main__":
    unittest.main()
