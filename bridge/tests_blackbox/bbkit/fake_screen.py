"""A fake agent's screen for the release journeys (J65, « Je le taperai moi-même »): a small RFB 3.8 server the bridge
logs in to as it does to an agent's own VNC (`desktop_vnc`, desktop.rs `vnc_login`: security « None »), so the apps'
Bureau shows a live screen — a grey frame, sent again on each update request — and the person can take the hand and
give it back. It answers whatever the viewer asks (pixel format, encodings, updates, keys, pointer, clipboard); what
the person types is only counted (`keys`), never kept. Nothing of this machine is shown."""
from __future__ import annotations

import socket
import struct
import threading
import time

WIDTH, HEIGHT = 320, 200
NAME = b"Bureau de Vie (parcours)"
# client messages and their length after the type byte (ClientCutText: 7 more, then its text)
SIZES = {0: 19, 2: 3, 3: 9, 4: 7, 5: 5, 6: 7}


class FakeScreen:
    def __init__(self) -> None:
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(8)
        self.port = self.sock.getsockname()[1]
        self.connections = 0
        self.frames = 0
        self.keys = 0
        threading.Thread(target=self._serve, daemon=True).start()

    @property
    def address(self) -> str:
        return f"tcp://127.0.0.1:{self.port}"

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass

    def _serve(self) -> None:
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            self.connections += 1
            threading.Thread(target=self._talk, args=(conn,), daemon=True).start()

    @staticmethod
    def _exactly(conn: socket.socket, n: int) -> bytes:
        data = b""
        while len(data) < n:
            chunk = conn.recv(n - len(data))
            if not chunk:
                raise ConnectionError
            data += chunk
        return data

    def _frame(self, conn: socket.socket, bytes_per_pixel: int) -> None:
        # one raw rectangle, every byte the same: grey whatever the viewer's channel shifts
        pixels = b"\xd8" * (WIDTH * HEIGHT * bytes_per_pixel)
        conn.sendall(b"\x00\x00" + struct.pack(">H", 1) + struct.pack(">HHHHi", 0, 0, WIDTH, HEIGHT, 0) + pixels)
        self.frames += 1

    def _talk(self, conn: socket.socket) -> None:
        try:
            conn.sendall(b"RFB 003.008\n")
            self._exactly(conn, 12)                       # the bridge's version
            conn.sendall(b"\x01\x01")                     # one security type: None
            self._exactly(conn, 1)
            conn.sendall(b"\x00\x00\x00\x00")             # SecurityResult: OK
            self._exactly(conn, 1)                        # ClientInit
            # 32 bits per pixel, depth 24, little-endian, true colour, 8 bits a channel
            pixel = struct.pack(">BBBBHHHBBB3x", 32, 24, 0, 1, 255, 255, 255, 16, 8, 0)
            conn.sendall(struct.pack(">HH", WIDTH, HEIGHT) + pixel + struct.pack(">I", len(NAME)) + NAME)
            bytes_per_pixel = 4
            last = 0.0
            while True:
                kind = self._exactly(conn, 1)[0]
                if kind not in SIZES:
                    return                                # something this screen does not speak: the viewer reconnects
                body = self._exactly(conn, SIZES[kind])
                if kind == 0:                             # SetPixelFormat
                    bytes_per_pixel = max(1, body[3] // 8)
                elif kind == 2:                           # SetEncodings
                    self._exactly(conn, 4 * struct.unpack(">H", body[1:3])[0])
                elif kind == 6:                           # ClientCutText
                    self._exactly(conn, struct.unpack(">I", body[3:7])[0])
                elif kind == 4:
                    self.keys += 1
                elif kind == 3:                           # FramebufferUpdateRequest: a frame, gently when incremental
                    if body[0] and time.time() - last < 0.5:
                        time.sleep(0.5)
                    self._frame(conn, bytes_per_pixel)
                    last = time.time()
        except (OSError, ConnectionError, struct.error):
            return
        finally:
            conn.close()
