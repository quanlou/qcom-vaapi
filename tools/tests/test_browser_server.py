"""Real HTTP byte-range responses must support browser random access."""
import http.client
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ServerTests(unittest.TestCase):
    def test_media_ranges_and_concurrent_telemetry(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            payload = bytes(range(256)) * 1000
            (directory / "sample.mp4").write_bytes(payload)
            process = subprocess.Popen([sys.executable, str(ROOT / "tools/browser-playback-server.py"),
                                        "--directory", tmp, "--events", str(directory / "events"),
                                        "--port-file", str(directory / "port")],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                until = time.monotonic() + 5
                while not (directory / "port").exists() and time.monotonic() < until:
                    time.sleep(.02)
                port = int((directory / "port").read_text())
                for value, start, end in (("bytes=100-199", 100, 199),
                                          ("bytes=255990-", 255990, 255999),
                                          ("bytes=-7", 255993, 255999),
                                          ("bytes=0-999999", 0, 255999)):
                    with self.subTest(value=value):
                        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
                        connection.request("GET", "/sample.mp4", headers={"Range": value})
                        response = connection.getresponse()
                        self.assertEqual(response.status, 206)
                        self.assertEqual(response.getheader("Content-Range"), f"bytes {start}-{end}/{len(payload)}")
                        self.assertEqual(int(response.getheader("Content-Length")), end - start + 1)
                        self.assertEqual(response.read(), payload[start:end + 1])
                        connection.close()
                for value in ("bytes=256000-", "bytes=90-80", "bytes=-0", "bytes=0-1,9-10"):
                    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
                    connection.request("GET", "/sample.mp4", headers={"Range": value})
                    response = connection.getresponse()
                    self.assertEqual(response.status, 416)
                    self.assertEqual(response.getheader("Content-Range"), "bytes */256000")
                    self.assertEqual(response.read(), b"")
                    connection.close()
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
                connection.request("HEAD", "/sample.mp4", headers={"Range": "bytes=15-18"})
                response = connection.getresponse()
                self.assertEqual(response.status, 206)
                self.assertEqual(response.read(), b"")
                connection.close()
                # A browser may leave its media connection open while posting
                # telemetry. The listener must serve both independently.
                media = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
                media.request("GET", "/sample.mp4")
                response = media.getresponse()
                telemetry = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
                telemetry.request("POST", "/telemetry", body='{"event":"seeked"}',
                                  headers={"Origin": f"http://127.0.0.1:{port}"})
                self.assertEqual(telemetry.getresponse().status, 204)
                self.assertEqual(response.read(), payload)
                media.close()
                telemetry.close()
                self.assertIn('"seeked"', (directory / "events").read_text())
            finally:
                process.terminate()
                process.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()
