"""Seek acceptance must use matching replies and active hardware decoding."""
import contextlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("seek_driver", ROOT / "tools/mpv_seek_drive.py")
seek = importlib.util.module_from_spec(spec)
spec.loader.exec_module(seek)


class FakeSocket:
    def __init__(self, hwdec="vaapi-copy", reject_seek=False, disconnect=False,
                 stuck_position=False, stuck_seeking=False, restart=True,
                 landing_offset=-0.5, seek_event=True, events_after_reply=False):
        self.hwdec = hwdec
        self.reject_seek = reject_seek
        self.disconnect = disconnect
        self.pending = b""
        self.commands = []
        self.position = 1.25
        self.stuck_position = stuck_position
        self.stuck_seeking = stuck_seeking
        self.restart = restart
        self.landing_offset = landing_offset
        self.seek_event = seek_event
        self.events_after_reply = events_after_reply

    def sendall(self, payload):
        request = json.loads(payload)
        command = request["command"]
        self.commands.append(command)
        if self.disconnect:
            return
        data = None
        error = "invalid parameter" if self.reject_seek and command[0] == "seek" else "success"
        events = []
        if command == ["get_property", "hwdec-current"]:
            data = self.hwdec.pop(0) if isinstance(self.hwdec, list) else self.hwdec
            if data == "property unavailable":
                error = data
        if command == ["get_property", "time-pos"]:
            data = self.position
            if not self.stuck_position:
                self.position += 0.04
        if command == ["get_property", "seeking"]:
            data = self.stuck_seeking
        if command[0] == "seek" and not self.reject_seek:
            if not self.stuck_position:
                self.position = max(0, command[1] + self.landing_offset)
            if self.seek_event:
                events.append({"event": "seek"})
            if self.restart:
                events.append({"event": "playback-restart"})
        response = dict(request_id=request["request_id"], data=data, error=error)
        # An event and another request's reply must not satisfy this request.
        unrelated = dict(request_id=request["request_id"] + 1, error="success", data="wrong")
        event_lines = "".join(json.dumps(event) + "\n" for event in events)
        reply_lines = json.dumps(unrelated) + "\n" + json.dumps(response) + "\n"
        self.pending += (reply_lines + event_lines if self.events_after_reply
                         else event_lines + reply_lines).encode()

    def recv(self, length):
        result, self.pending = self.pending[:length], self.pending[length:]
        return result

    def settimeout(self, value):
        pass


class SeekIpcTests(unittest.TestCase):
    def test_async_events_and_other_replies_are_not_acknowledgments(self):
        self.assertEqual(seek.RpcClient(FakeSocket()).request(["get_property", "time-pos"]), 1.25)

    def test_all_seek_commands_must_be_acknowledged(self):
        sock = FakeSocket()
        output = io.StringIO()
        with mock.patch.object(seek.time, "sleep"), contextlib.redirect_stdout(output):
            seek.drive(sock, 3, 10)
        self.assertEqual(sum(c[0] == "seek" for c in sock.commands), 3)
        self.assertIn("acknowledged=3", output.getvalue())
        self.assertEqual(sock.commands[-1], ["quit"])

    def test_rejected_seek_does_not_pass(self):
        with self.assertRaisesRegex(RuntimeError, "rejected"):
            seek.drive(FakeSocket(reject_seek=True), 1, 10)

    def test_unavailable_startup_property_waits_for_hardware(self):
        sock = FakeSocket(hwdec=["property unavailable"] + ["vaapi-copy"] * 4)
        with mock.patch.object(seek.time, "sleep"), contextlib.redirect_stdout(io.StringIO()):
            seek.drive(sock, 1, 10)
        self.assertEqual(sock.commands[:2], [["get_property", "hwdec-current"]] * 2)

    def test_unavailable_startup_property_has_bounded_wait(self):
        client = mock.Mock()
        client.request.side_effect = seek.PropertyUnavailable("property unavailable")
        with mock.patch.object(seek, "RpcClient", return_value=client), mock.patch.object(seek.time, "monotonic", side_effect=[0, 11]):
            with self.assertRaisesRegex(RuntimeError, "not active"):
                seek.drive(FakeSocket(), 1, 10)
        self.assertEqual(client.request.call_count, 1)

    def test_unavailable_property_after_seek_fails(self):
        client = mock.Mock()
        client.request.side_effect = ["vaapi-copy", None, seek.PropertyUnavailable("property unavailable")]
        with mock.patch.object(seek, "RpcClient", return_value=client), mock.patch.object(seek.time, "sleep"):
            with self.assertRaises(seek.PropertyUnavailable):
                seek.drive(FakeSocket(), 1, 10)

    def assert_bounded_stall(self, sock, count=1):
        clock = mock.Mock()
        clock.now = 0.0
        def advance(seconds):
            clock.now += seconds
        output = io.StringIO()
        with mock.patch.object(seek.time, "monotonic", side_effect=lambda: clock.now), \
             mock.patch.object(seek.time, "sleep", side_effect=advance), \
             contextlib.redirect_stdout(output):
            with self.assertRaisesRegex(TimeoutError, "forward playback progress"):
                seek.drive(sock, count, 10)
        self.assertLessEqual(clock.now, count * seek.SEEK_COMPLETION_SECONDS + 0.001)
        self.assertNotIn("seek_commands=pass", output.getvalue())
        self.assertFalse(any(command[0] == "quit" for command in sock.commands))

    def test_acknowledged_seek_with_constant_position_fails(self):
        self.assert_bounded_stall(FakeSocket(stuck_position=True))

    def test_seek_that_never_finishes_fails_even_with_restart_event(self):
        self.assert_bounded_stall(FakeSocket(stuck_seeking=True))

    def test_clock_progress_without_fresh_playback_restart_is_insufficient(self):
        self.assert_bounded_stall(FakeSocket(restart=False))

    def test_restart_without_fresh_seek_event_is_insufficient(self):
        self.assert_bounded_stall(FakeSocket(seek_event=False))

    def test_keyframe_landing_can_differ_from_requested_target(self):
        sock = FakeSocket(landing_offset=-1.0)
        output = io.StringIO()
        with mock.patch.object(seek.time, "sleep"), contextlib.redirect_stdout(output):
            seek.drive(sock, 3, 10)
        self.assertIn("target=1.500 landed=0.500 progressed=0.580", output.getvalue())
        self.assertIn("completed=3 progressed=3", output.getvalue())

    def test_restart_events_after_command_reply_are_preserved(self):
        output = io.StringIO()
        with mock.patch.object(seek.time, "sleep"), contextlib.redirect_stdout(output):
            seek.drive(FakeSocket(events_after_reply=True), 3, 10)
        self.assertIn("completed=3 progressed=3", output.getvalue())

    def test_previous_seek_events_cannot_complete_next_seek(self):
        class FirstSeekOnly(FakeSocket):
            def sendall(self, payload):
                if json.loads(payload)["command"][0] == "seek" and any(
                        command[0] == "seek" for command in self.commands):
                    self.seek_event = self.restart = False
                super().sendall(payload)
        self.assert_bounded_stall(FirstSeekOnly(), count=2)

    def test_fallback_during_post_seek_progress_fails(self):
        sock = FakeSocket(hwdec=["vaapi-copy", "vaapi-copy", "no"])
        with mock.patch.object(seek.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "stopped after seek"):
                seek.drive(sock, 1, 10)

    def test_boolean_position_is_not_numeric_progress(self):
        sock = FakeSocket(stuck_position=True)
        sock.position = True
        with mock.patch.object(seek.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "finite playback position"):
                seek.drive(sock, 1, 10)

    def test_rpc_classifies_only_property_unavailable(self):
        sock = FakeSocket()
        sock.sendall = lambda payload: setattr(sock, "pending", b'{"request_id":1,"error":"property unavailable"}\n')
        with self.assertRaises(seek.PropertyUnavailable):
            seek.RpcClient(sock).request(["get_property", "hwdec-current"])

    def test_software_or_other_hardware_decoder_does_not_pass(self):
        with self.assertRaisesRegex(RuntimeError, "not active"):
            seek.drive(FakeSocket(hwdec="nvdec"), 1, 10)

    def test_disconnection_does_not_count_as_success(self):
        with self.assertRaisesRegex(RuntimeError, "disconnected"):
            seek.RpcClient(FakeSocket(disconnect=True)).request(["seek", 1, "absolute+keyframes"])

    def test_invalid_probe_parameters_do_not_count_as_a_seek(self):
        for count, duration in [(0, 10), (-1, 10), (1, 0), (1, float("inf"))]:
            with self.subTest(count=count, duration=duration), self.assertRaises(ValueError):
                seek.drive(FakeSocket(), count, duration)


class SeekShellTests(unittest.TestCase):
    def test_disk_backed_long_results_path_can_bind_and_cleans_ipc(self):
        source = (ROOT / "tools/verify-seek-storm.sh").read_text()
        setup = "ipc_dir=" + source.split("ipc_dir=", 1)[1].split("declare -a phase_results", 1)[0]
        probe = ('import os,socket; '
                 's=socket.socket(socket.AF_UNIX); '
                 's.bind(os.environ["SOCK"]); '
                 'print(os.environ["SOCK"]); s.close()')
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp) / ("results-" + "x" * 150)
            work.mkdir()
            result = subprocess.run(
                ["bash", "-c", setup + '\nSOCK="$sock" python3 -c "$1"', "_", probe],
                env={**os.environ, "work_dir": str(work), "TMPDIR": str(work)},
                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        ipc_path = Path(result.stdout.strip())
        self.assertFalse(ipc_path.exists())
        self.assertFalse(ipc_path.parent.exists())

    def run_storm(self, controller_status, mpv_status):
        source = (ROOT / "tools/verify-seek-storm.sh").read_text()
        function = "run_storm() {" + source.split("run_storm() {", 1)[1].split("\nexport -f run_storm", 1)[0]
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for name, body in [("mpv", f"exit {mpv_status}"), ("ffprobe", "echo 10")]:
                script = base / name
                script.write_text("#!/bin/sh\n" + body + "\n")
                script.chmod(0o755)
            controller = base / "controller.py"
            controller.write_text(f"raise SystemExit({controller_status})\n")
            return subprocess.run(["bash", "-c", function + '\nrun_storm "$@"', "_",
                                   "sample.mp4", "1", str(base / "ipc.sock"), str(base / "storm.log")],
                                  env={**os.environ, "PATH": str(base) + os.pathsep + os.environ["PATH"],
                                       "STORM_DRIVER": str(controller)},
                                  capture_output=True, text=True, timeout=10)

    def test_controller_failure_is_not_hidden_by_successful_mpv_exit(self):
        self.assertEqual(self.run_storm(3, 0).returncode, 3)

    def test_player_failure_is_not_hidden_by_successful_controller(self):
        self.assertEqual(self.run_storm(0, 7).returncode, 7)



if __name__ == "__main__":
    unittest.main()
