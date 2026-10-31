from support import load_config
"""CalDAV write-back integration: conditional merge, undo, races, restart/offline."""
import base64
import datetime
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
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]
TODAY = datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=7))).strftime('%Y%m%d')
ICS = f'BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:write-test\r\nSUMMARY:Тестовая задача\r\nDUE;VALUE=DATE:{TODAY}\r\nDESCRIPTION:Original note\r\nX-APPLE-KEEP:yes\r\nSTATUS:NEEDS-ACTION\r\nEND:VTODO\r\nEND:VCALENDAR\r\n'
VERSION = 1
FAIL = False
CONFLICT = True
PUTS = 0
BLOCK_NEXT = False
STARTED = threading.Event()
RELEASE = threading.Event()

class Dav(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def valid(self):
        assert self.headers['Authorization'] == 'Basic ' + base64.b64encode(b'test:secret').decode()
        if FAIL:
            self.send_error(503); return False
        return True
    def do_REPORT(self):
        body = self.rfile.read(int(self.headers['Content-Length'])).decode()
        if not self.valid(): return
        data = ''
        if 'name="VTODO"' in body:
            data = '<d:response><d:href>/calendar/task.ics</d:href><d:propstat><d:prop><d:getetag>"v'+str(VERSION)+'"</d:getetag><c:calendar-data>'+escape(ICS)+'</c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>'
        self.send_response(207); self.end_headers()
        self.wfile.write(('<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">'+data+'</d:multistatus>').encode())
    def do_GET(self):
        if not self.valid(): return
        assert self.path == '/calendar/task.ics'
        self.send_response(200); self.send_header('ETag',f'"v{VERSION}"'); self.end_headers(); self.wfile.write(ICS.encode())
    def do_PUT(self):
        global ICS, VERSION, CONFLICT, PUTS, BLOCK_NEXT
        body = self.rfile.read(int(self.headers['Content-Length'])).decode()
        if not self.valid(): return
        assert self.path == '/calendar/task.ics'
        assert self.headers.get('If-Match')
        PUTS += 1
        if BLOCK_NEXT:
            BLOCK_NEXT = False; STARTED.set(); assert RELEASE.wait(20)
        if CONFLICT:
            CONFLICT = False; VERSION += 1
            ICS = ICS.replace('Original note','Edited by another client')
            self.send_response(412); self.end_headers(); return
        if self.headers['If-Match'] != f'"v{VERSION}"':
            self.send_response(412); self.end_headers(); return
        assert 'X-APPLE-KEEP:yes' in body
        assert 'Edited by another client' in body
        ICS = body; VERSION += 1
        self.send_response(204); self.send_header('ETag',f'"v{VERSION}"'); self.end_headers()

def port():
    with socket.socket() as s: s.bind(('127.0.0.1',0)); return s.getsockname()[1]
def wait(test, seconds=25):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        try:
            if test(): return
        except (OSError, ValueError): pass
        time.sleep(.1)
    raise AssertionError('Timed out')

with tempfile.TemporaryDirectory() as tmp:
    tmp=Path(tmp)
    dav=http.server.ThreadingHTTPServer(('127.0.0.1',0),Dav)
    threading.Thread(target=dav.serve_forever,daemon=True).start()
    config=load_config(ROOT)
    config.update(listen=f'127.0.0.1:{port()}',cache_dir=str(tmp/'cache'),refresh_seconds=3600)
    config['calendars'][0]['url']=f'http://127.0.0.1:{dav.server_port}/calendar/'
    config_file=tmp/'config.json'; config_file.write_text(json.dumps(config))
    env=dict(os.environ,CALDAV_USERNAME='test',CALDAV_PASSWORD='secret')
    binary=Path(os.environ.get('CARGO_TARGET_DIR',ROOT/'target'))/'debug/paper-calendar'
    command=[str(binary),'--config',str(config_file)]
    base='http://'+config['listen']
    def get(path): return json.load(urllib.request.urlopen(base+path,timeout=4))
    def post(path,body):
        r=urllib.request.Request(base+path,data=json.dumps(body).encode(),headers={'Content-Type':'application/json'},method='POST')
        assert urllib.request.urlopen(r,timeout=6).status==204
    def ready(): return get('/status')['ready'] and not get('/status')['busy']
    def empty(): return get('/status')['pending_task_updates']==0 and not get('/status')['busy']
    def refresh():
        c=next(c for c in get('/controls') if c['label']=='Обновить')
        post('/tap',dict(x=c['x']+10,y=c['y']+10))
    process=subprocess.Popen(command,env=env,stdout=subprocess.DEVNULL)
    try:
        wait(ready); task=get('/tasks')[0]; ident=task['id']
        post('/tasks/complete',dict(id=ident,completed=True))
        wait(lambda:'STATUS:COMPLETED' in ICS and empty())
        assert PUTS==2 and 'PERCENT-COMPLETE:100' in ICS and '\r\nCOMPLETED:' in ICS
        print('PASS: complete via PUT + If-Match; 412 re-reads and preserves concurrent edits')
        post('/tasks/complete',dict(id=ident,completed=False))
        wait(lambda:'STATUS:NEEDS-ACTION' in ICS and empty())
        assert '\r\nCOMPLETED:' not in ICS and 'PERCENT-COMPLETE:0' in ICS
        print('PASS: undo completion updates server')
        BLOCK_NEXT=True; STARTED.clear(); RELEASE.clear()
        post('/tasks/complete',dict(id=ident,completed=True))
        assert STARTED.wait(20)
        post('/tasks/complete',dict(id=ident,completed=False))
        RELEASE.set()
        wait(lambda:'STATUS:NEEDS-ACTION' in ICS and empty())
        print('PASS: undo while PUT is in flight is preserved and sent afterwards')
        FAIL=True
        post('/tasks/complete',dict(id=ident,completed=True))
        assert json.loads((tmp/'cache/task-outbox.json').read_text())[ident]['completed']
        process.terminate(); process.wait(timeout=5)
        process=subprocess.Popen(command,env=env,stdout=subprocess.DEVNULL)
        wait(ready)
        assert get('/status')['pending_task_updates']==1
        assert any(t['id']==ident and t['completed'] for t in get('/tasks'))
        FAIL=False; refresh()
        wait(lambda:'STATUS:COMPLETED' in ICS and empty())
        print('PASS: durable offline outbox survives restart and sends when online')
        ICS=ICS.replace('STATUS:COMPLETED','STATUS:NEEDS-ACTION').replace('PERCENT-COMPLETE:100','PERCENT-COMPLETE:0')
        ICS='\r\n'.join(line for line in ICS.split('\r\n') if not line.startswith('COMPLETED:'))
        VERSION+=1; refresh()
        wait(lambda: any(t['id']==ident and not t['completed'] for t in get('/tasks')))
        print('PASS: reopening task on another client is reflected locally')
    finally:
        RELEASE.set(); process.terminate(); process.wait(timeout=5); dav.shutdown()
