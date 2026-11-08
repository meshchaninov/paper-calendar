"""Local-only integration of persisted 12-hour sync and manual refresh."""
from support import load_config
import http.server
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
CALLS = 0
FAIL = False

class Dav(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_REPORT(self):
        global CALLS
        self.rfile.read(int(self.headers['Content-Length']))
        CALLS += 1
        if FAIL:
            self.send_error(503)
            return
        self.send_response(207)
        self.end_headers()
        self.wfile.write(b'<d:multistatus xmlns:d="DAV:"/>')

with tempfile.TemporaryDirectory() as tmp:
    tmp = Path(tmp)
    dav = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Dav)
    threading.Thread(target=dav.serve_forever, daemon=True).start()
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    config = load_config(ROOT)
    config.update(listen=f'127.0.0.1:{port}', cache_dir=str(tmp/'cache'), refresh_seconds=43200)
    config['calendars'][0].update(url=f'http://127.0.0.1:{dav.server_port}/calendar/', tasks=False)
    path = tmp/'config.json'
    path.write_text(json.dumps(config))
    binary = Path(os.environ.get('CARGO_TARGET_DIR', ROOT/'target'))/'debug/paper-calendar'
    env = dict(os.environ, CALDAV_USERNAME='test', CALDAV_PASSWORD='test')
    process = None
    def get(path):
        return json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}'+path, timeout=3))
    def wait(test):
        until = time.monotonic() + 15
        while time.monotonic() < until:
            try:
                if test(): return
            except (OSError, ValueError): pass
            time.sleep(.1)
        raise AssertionError('Timed out waiting for app')
    def start():
        global process
        process = subprocess.Popen([str(binary), '--config', str(path)], env=env, stdout=subprocess.DEVNULL)
        wait(lambda: get('/status')['ready'] and not get('/status')['busy'])
    def stop():
        global process
        process.terminate()
        process.wait(timeout=5)
        process = None
    try:
        start()
        assert CALLS == 1
        stop()
        image = tmp/'cache/calendar.png'
        events = tmp/'cache/events.json'
        times = (image.stat().st_mtime_ns, events.stat().st_mtime_ns)
        start()
        assert CALLS == 1 and get('/status')['power']['sync_checks'] == 0
        assert events.stat().st_mtime_ns == times[1]
        print('PASS: fresh restart renders cached data without network sync or event-cache writes')
        c = next(c for c in get('/controls') if c['label'] == 'Обновить')
        req = urllib.request.Request(f'http://127.0.0.1:{port}/tap', data=json.dumps(dict(x=c['x']+10, y=c['y']+10)).encode(), headers={'Content-Type':'application/json'}, method='POST')
        assert urllib.request.urlopen(req).status == 204
        wait(lambda: CALLS == 2 and not get('/status')['busy'])
        assert events.stat().st_mtime_ns == times[1]
        print('PASS: manual refresh overrides 12-hour interval; unchanged events are not rewritten')
        stop()
        state_path = tmp/'cache/sync-state.json'
        state = json.loads(state_path.read_text())
        state['last_attempt'] = int(time.time()) - 43201
        state_path.write_text(json.dumps(state))
        FAIL = True
        start()
        assert CALLS == 3 and get('/status')['sync_error']
        stop()
        start()
        assert CALLS == 3 and get('/status')['sync_error']
        print('PASS: expired schedule retries; a failed sync retains data and does not loop on restart')
    finally:
        if process: stop()
        dav.shutdown()
