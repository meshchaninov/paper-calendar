"""Package a built ARM executable with generic settings, without credentials."""
from pathlib import Path
import hashlib
import json
import shutil
import sys
import zipfile

root = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1]).resolve()
if not binary.is_file():
    raise SystemExit('ARM executable does not exist')
dist = root / 'dist'
package = dist / 'paper-calendar-pocketbook'
if package.exists():
    shutil.rmtree(package)
app = package / 'applications' / 'paper-calendar'
app.mkdir(parents=True)
shutil.copy2(binary, app / 'paper-calendar')
(app / 'paper-calendar').chmod(0o755)
shutil.copy2(root / 'packaging/pocketbook/PaperCalendar.app', package / 'applications/PaperCalendar.app')
(package / 'applications/PaperCalendar.app').chmod(0o755)
shutil.copy2(root / 'packaging/pocketbook/config.example.json', app / 'config.json')
shutil.copy2(root / 'docs/pocketbook.md', package / 'INSTALL.md')
manifest = {str(p.relative_to(package)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in package.rglob('*') if p.is_file()}
(package / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
archive = dist / 'paper-calendar-pocketbook.zip'
with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as z:
    for p in package.rglob('*'):
        if p.is_file():
            z.write(p, str(p.relative_to(package)))
print(archive)
