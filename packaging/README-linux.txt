RePlayrer (Linux x86_64) — v0 vertical slice
=============================================

This is an AppImage: a single self-contained executable, no install step.

  cd replayrer-<version>-linux-x86_64
  chmod +x replayrer.AppImage
  ./replayrer.AppImage

If it refuses to run with a FUSE-related error (common on minimal/server
distros and inside some containers), either install libfuse2 or run it
extracted instead:
  ./replayrer.AppImage --appimage-extract-and-run

If the window fails to open with a WebKitGTK-related error, install your
distro's WebKitGTK package (e.g. on Debian/Ubuntu: `apt install
libwebkit2gtk-4.1-0`) — the AppImage bundles most of the app's
dependencies but relies on the system's webview library.

Trying it out
-------------

There's no scenario file picker yet (v0) — the toolbar takes a plain
path. This package includes a ready-to-use sample:

  1. Run ./replayrer.AppImage from *inside* this extracted folder (so
     the relative paths in scenario.toml resolve).
  2. Paste this folder's scenario.toml path into the toolbar's path
     field.
  3. Click Play.

sample.pcap contains 4 UDP packets spaced 300/400/500 ms apart
(payloads packet-0..packet-3); scenario.toml replays it over UDP to
127.0.0.1:5000. Listen for it in another terminal, e.g.:
  python3 -c "
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(('127.0.0.1', 5000))
while True:
    data, addr = s.recvfrom(4096)
    print(addr, data)
"

Only Start/Stop are wired to the real engine in this v0 slice — Pause,
live speed changes, and seeking are visible in the UI but intentionally
inert (see backend/docs/frontend_integration_gaps.md).
