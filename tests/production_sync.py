"""Verify monthly subscription caching across fresh processes, without accounts."""
from support import load_config
import datetime
import http.server
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile
import threading
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]
CALLS = []
FAIL = False


class Dav(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_REPORT(self):
        body = self.rfile.read(int(self.headers['Content-Length'])).decode()
        CALLS.append(body)
        if FAIL:
            self.send_error(503)
            return
        assert '<c:expand ' in body
        year = datetime.datetime.now(datetime.timezone.utc).year
        ics = (f'BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n'
               f'UID:holiday\r\nSUMMARY:Праздник\r\nDTSTART;VALUE=DATE:{year}0101\r\n'
               f'DTEND;VALUE=DATE:{year}0102\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n')
        xml = ('<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">'
               '<d:response><d:propstat><d:prop><c:calendar-data>' + escape(ics) +
               '</c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status>'
               '</d:propstat></d:response></d:multistatus>')
        self.send_response(207)
        self.end_headers()
        self.wfile.write(xml.encode())


with tempfile.TemporaryDirectory() as tmp:
    tmp = Path(tmp)
    dav = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Dav)
    threading.Thread(target=dav.serve_forever, daemon=True).start()
    config = load_config(ROOT)
    config['cache_dir'] = str(tmp/'cache')
    source = config['calendars'][0]
    source.update(url=f'http://127.0.0.1:{dav.server_port}/production/',
                  production_calendar=True, tasks=False)
    config['calendars'] = [source]
    path = tmp/'config.json'
    path.write_text(json.dumps(config))
    binary = Path(os.environ.get('CARGO_TARGET_DIR', ROOT/'target'))/'debug/paper-calendar'
    env = dict(os.environ, CALDAV_USERNAME='test', CALDAV_PASSWORD='test')

    def render():
        subprocess.run([str(binary), '--config', str(path), '--render', str(tmp/'frame.png')],
                       env=env, check=True, stdout=subprocess.DEVNULL, timeout=30)

    try:
        render()
        first_calls = len(CALLS)
        caches = list((tmp/'cache/production').glob('*.json'))
        assert first_calls == len(caches) and first_calls >= 1
        for body in CALLS:
            start, end = re.search(r'<c:expand start="([^"]+)" end="([^"]+)"', body).groups()
            span = datetime.datetime.strptime(end, '%Y%m%dT%H%M%SZ') - datetime.datetime.strptime(start, '%Y%m%dT%H%M%SZ')
            assert span.days in (365, 366)
        snapshots = {p: p.read_bytes() for p in caches}
        render()  # A fresh process, same month: no REPORT and no cache rewrite.
        assert len(CALLS) == first_calls
        assert all(p.read_bytes() == snapshots[p] for p in caches)
        print('PASS: whole-year subscription cache survives restart; no repeated REPORT or cache write')
        for p in caches:
            cache = json.loads(p.read_text())
            cache.update(last_success=0, last_attempt=0)
            p.write_text(json.dumps(cache))
        render()
        assert len(CALLS) == first_calls * 2
        assert all(json.loads(p.read_text())['last_success'] > 0 for p in caches)
        print('PASS: expired monthly cache refreshes and saves the next deadline')
        FAIL = True
        saved = {}
        for p in caches:
            cache = json.loads(p.read_text())
            saved[p] = cache['events']
            cache.update(last_success=0, last_attempt=0)
            p.write_text(json.dumps(cache))
        render()
        assert len(CALLS) == first_calls * 3
        assert all(json.loads(p.read_text())['events'] == saved[p] for p in caches)
        render()
        assert len(CALLS) == first_calls * 3
        print('PASS: source failure keeps saved days and persists 24-hour retry backoff')
    finally:
        dav.shutdown()
