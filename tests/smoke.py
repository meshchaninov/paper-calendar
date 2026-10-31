from support import load_config
"""Integration smoke: local CalDAV REPORT, rendering, ETag, cache and failure recovery.
Run after cargo build. Uses no accounts and never sends data outside localhost.
"""
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
import urllib.error
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[1]
TODAY = datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=7))).strftime('%Y%m%d')
FAIL = False
CALLS = []

class Dav(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_REPORT(self):
        body = self.rfile.read(int(self.headers['Content-Length'])).decode()
        assert self.headers['Depth'] == '1'
        assert self.headers['Authorization'] == 'Basic ' + base64.b64encode(b'test:secret').decode()
        CALLS.append(body)
        if FAIL:
            self.send_error(503)
            return
        if 'name="VTODO"' in body:
            component = f'BEGIN:VTODO\r\nUID:task-1\r\nSUMMARY:Заказать продукты\r\nDUE;VALUE=DATE:{TODAY}\r\nEND:VTODO\r\n' + ''.join(f'BEGIN:VTODO\r\nUID:extra-{i}\r\nSUMMARY:Тестовая задача {i}\r\nEND:VTODO\r\n' for i in range(6))
        else:
            assert '<c:expand ' in body
            component = f'BEGIN:VEVENT\r\nDTSTART:{TODAY}T030000Z\r\nDTEND:{TODAY}T040000Z\r\nSUMMARY:Проверка CalDAV & Rust\r\nEND:VEVENT\r\n'
        ics = 'BEGIN:VCALENDAR\r\nVERSION:2.0\r\n' + component + 'END:VCALENDAR\r\n'
        xml = '<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:response><d:propstat><d:prop><c:calendar-data>' + escape(ics) + '</c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>'
        self.send_response(207)
        self.send_header('Content-Type', 'application/xml')
        self.end_headers()
        self.wfile.write(xml.encode())

def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]

def get(base, path, headers=None):
    return urllib.request.urlopen(urllib.request.Request(base+path, headers=headers or {}), timeout=3)

def wait_ready(base):
    for _ in range(100):
        try:
            status = json.load(get(base, '/status'))
            if status['ready'] and not status.get('busy',False): return status
        except (OSError, urllib.error.URLError): pass
        time.sleep(.1)
    raise AssertionError('app did not become ready')

with tempfile.TemporaryDirectory() as tmp:
    tmp = Path(tmp)
    dav = http.server.ThreadingHTTPServer(('127.0.0.1',0),Dav)
    threading.Thread(target=dav.serve_forever,daemon=True).start()
    config = load_config(ROOT)
    config.update(listen=f'127.0.0.1:{free_port()}',cache_dir=str(tmp/'cache'),refresh_seconds=30)
    config['calendars'][0]['url'] = f'http://127.0.0.1:{dav.server_port}/calendar/'
    config_file = tmp/'config.json'
    config_file.write_text(json.dumps(config))
    env = dict(os.environ, CALDAV_USERNAME='test', CALDAV_PASSWORD='secret')
    command = [str(Path(os.environ.get('CARGO_TARGET_DIR', ROOT/'target'))/'debug/paper-calendar'),'--config',str(config_file)]
    base = 'http://'+config['listen']
    process = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL)
    try:
        status = wait_ready(base)
        assert not status['sync_error'] and status['last_success']
        first = get(base,'/calendar.png')
        etag = first.headers['ETag']
        png = first.read()
        assert png[:8] == b'\x89PNG\r\n\x1a\n'
        assert len(CALLS) == 2
        try:
            get(base,'/calendar.png',{'If-None-Match':etag})
            raise AssertionError('Expected 304')
        except urllib.error.HTTPError as e:
            assert e.code == 304
        assert 'Проверка CalDAV' in (tmp/'cache/calendar.svg').read_text()
        assert 'Проверка CalDAV & Rust' in [e['title'] for e in json.loads((tmp/'cache/events.json').read_text())['events']]
        print('PASS: authenticated REPORT, VEVENT/VTODO, Cyrillic rendering, PNG and HTTP 304')
        tasks = json.load(get(base,'/tasks'))
        task = tasks[0]
        completed_id = task['id']
        assert task['width'] >= 300 and task['height'] >= 100
        controls=json.load(get(base,'/controls'))
        assert not any(c['label'].startswith('Выполнить:') for c in controls)
        cell=next(c for c in controls if c['label']==datetime.datetime.strptime(TODAY,'%Y%m%d').strftime('%d.%m.%Y'))
        def press(x,y):
            request=urllib.request.Request(base+'/tap',data=json.dumps(dict(x=x,y=y)).encode(),headers={'Content-Type':'application/json'},method='POST')
            assert urllib.request.urlopen(request).status==204
        press(cell['x']+23,cell['y']+85)
        assert not next(t for t in json.load(get(base,'/tasks')) if t['id']==completed_id)['completed']
        press(task['x']+20,task['y']+20)
        assert next(t for t in json.load(get(base,'/tasks')) if t['id']==completed_id)['completed']
        press(cell['x']+23,cell['y']+85)
        assert next(t for t in json.load(get(base,'/tasks')) if t['id']==completed_id)['completed']
        controls=json.load(get(base,'/controls'))
        press_control=next(c for c in controls if c['label']=='Сегодня')
        press(press_control['x']+10,press_control['y']+10)
        etag=get(base,'/calendar.png').headers['ETag']
        print('PASS: calendar task taps select day without changing completion; sidebar toggles it')
        controls = json.load(get(base,'/controls'))
        before = (tmp/'cache/calendar.png').read_bytes()
        assert not any(c['label']=='Открыть: Заказать продукты' for c in controls)
        opener = next(c for c in controls if c['label']=='Открыть: Проверка CalDAV & Rust')
        def tap_control(control):
            request=urllib.request.Request(base+'/tap',data=json.dumps({'x':control['x']+control['width']//2,'y':control['y']+control['height']//2}).encode(),headers={'Content-Type':'application/json'},method='POST')
            assert urllib.request.urlopen(request).status==204
        tap_control(opener)
        assert json.load(get(base,'/tasks')) == []
        assert 'Описание не добавлено' in (tmp/'cache/calendar.svg').read_text()
        controls=json.load(get(base,'/controls'))
        assert any(c['label']=='Назад к списку' for c in controls)
        tap_control(next(c for c in controls if c['label']=='Назад к списку'))
        assert get(base,'/calendar.png').headers['ETag']==etag
        assert json.load(get(base,'/tasks'))[0]['completed']
        print('PASS: only timed events open details; task completion and Back remain intact')
        controls = json.load(get(base,'/controls'))
        first_page=json.load(get(base,'/tasks'))
        assert len(first_page)==5
        first_ids={t['id'] for t in first_page}
        tap_control(next(c for c in controls if c['label']=='Следующие задачи'))
        second_page=json.load(get(base,'/tasks'))
        assert len(second_page)==2
        assert first_ids.isdisjoint({t['id'] for t in second_page})
        controls=json.load(get(base,'/controls'))
        tap_control(next(c for c in controls if c['label']=='Предыдущие задачи'))
        assert get(base,'/calendar.png').headers['ETag']==etag
        task=json.load(get(base,'/tasks'))[0]
        row=dict(x=task['x']+100,y=task['y'],width=200,height=task['height'])
        tap_control(row)
        assert not json.load(get(base,'/tasks'))[0]['completed']
        assert not any(c['label']=='Назад к списку' for c in json.load(get(base,'/controls')))
        tap_control(row)
        assert get(base,'/calendar.png').headers['ETag']==etag
        print('PASS: five tasks per page, next/previous preserve marks, whole-row tap toggles without opening')
        controls = json.load(get(base,'/controls'))
        dates = [c for c in controls if c['label'].count('.') == 2]
        target = next(c for c in dates if c['label'] != datetime.datetime.strptime(TODAY,'%Y%m%d').strftime('%d.%m.%Y'))
        def tap(control):
            request = urllib.request.Request(base+'/tap',data=json.dumps({'x':control['x']+control['width']//2,'y':control['y']+control['height']//2}).encode(),headers={'Content-Type':'application/json'},method='POST')
            assert urllib.request.urlopen(request).status == 204
        tap(target)
        assert get(base,'/calendar.png').headers['ETag'] != etag
        tap(next(c for c in controls if c['label']=='Сегодня'))
        assert get(base,'/calendar.png').headers['ETag'] == etag
        print('PASS: date tap opens another day and Today restores the agenda')
        request=urllib.request.Request(base+'/tasks/complete',data=json.dumps({'id':completed_id,'completed':False}).encode(),headers={'Content-Type':'application/json'},method='POST')
        assert urllib.request.urlopen(request).status==204
        etag=get(base,'/calendar.png').headers['ETag']
        before_sync=json.load(get(base,'/status'))['power']
        old_frame=get(base,'/calendar.png').headers['ETag']
        tap_control(next(c for c in controls if c['label']=='Обновить'))
        for _ in range(100):
            st=json.load(get(base,'/status'))
            if st['power']['manual_refreshes']>before_sync['manual_refreshes'] and st['power']['sync_checks']>before_sync['sync_checks'] and not st['busy']: break
            time.sleep(.1)
        else: raise AssertionError('Manual refresh did not sync immediately')
        assert st['power']['image_renders']==before_sync['image_renders']
        assert get(base,'/calendar.png').headers['ETag']==old_frame
        print('PASS: Refresh syncs immediately without changing an identical frame')
        expected_renders=json.load(get(base,'/status'))['power']['image_renders']
        png_mtime=(tmp/'cache/calendar.png').stat().st_mtime_ns
        events_mtime=(tmp/'cache/events.json').stat().st_mtime_ns
        time.sleep(31)
        assert json.load(get(base,'/status'))['power']['image_renders']==expected_renders
        assert (tmp/'cache/calendar.png').stat().st_mtime_ns==png_mtime
        assert (tmp/'cache/events.json').stat().st_mtime_ns==events_mtime
        assert get(base,'/calendar.png').headers['ETag'] == etag
        assert len(CALLS) >= 4
        print('PASS: unchanged sync skips rasterization, image/cache writes and screen changes')
        home_frame=get(base,'/calendar.png').headers['ETag']
        tap(target)
        assert get(base,'/calendar.png').headers['ETag']!=home_frame
        for _ in range(340):
            if get(base,'/calendar.png').headers['ETag']==home_frame: break
            time.sleep(.1)
        else: raise AssertionError('Calendar did not return to today after 30 idle seconds')
        print('PASS: 30 seconds without interaction restores today and its agenda')
        before_ids=[t['id'] for t in json.load(get(base,'/tasks'))]
        hidden_id=next(ident for ident in before_ids if ident!=completed_id)
        request=urllib.request.Request(base+'/tasks/complete',data=json.dumps({'id':hidden_id,'completed':True}).encode(),headers={'Content-Type':'application/json'},method='POST')
        assert urllib.request.urlopen(request).status==204
        assert any(t['id']==hidden_id and t['completed'] for t in json.load(get(base,'/tasks')))
        for _ in range(340):
            visible=json.load(get(base,'/tasks'))
            if not any(t['id']==hidden_id for t in visible): break
            time.sleep(.1)
        else: raise AssertionError('Completed task was not hidden after 30 seconds')
        assert len(visible)==5
        assert [t['id'] for t in visible[:4]]==[ident for ident in before_ids if ident!=hidden_id]
        assert hidden_id in json.loads((tmp/'cache/completed-tasks.json').read_text())
        print('PASS: completed task stays checked briefly, then disappears and next task fills its place')
    finally:
        process.terminate(); process.wait(timeout=5)
    FAIL = True
    process = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL)
    try:
        status = wait_ready(base)
        assert status['sync_error'] and status['last_success']
        task = json.load(get(base,'/tasks'))[0]
        assert not any(t['id']==hidden_id for t in json.load(get(base,'/tasks')))
        request = urllib.request.Request(base+'/tasks/complete',data=json.dumps({'id':hidden_id,'completed':False}).encode(),headers={'Content-Type':'application/json'},method='POST')
        assert urllib.request.urlopen(request).status == 204
        assert any(t['id']==hidden_id and not t['completed'] for t in json.load(get(base,'/tasks')))
        print('PASS: hidden completion survives restart and can be undone offline')
        assert 'Проверка CalDAV' in (tmp/'cache/calendar.svg').read_text()
        assert 'Проверка CalDAV & Rust' in [e['title'] for e in json.loads((tmp/'cache/events.json').read_text())['events']]
        print('PASS: restart with CalDAV unavailable retains saved events and marks stale screen')
    finally:
        process.terminate(); process.wait(timeout=5); dav.shutdown()
