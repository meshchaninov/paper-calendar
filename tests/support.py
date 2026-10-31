"""Portable font selection for localhost integration tests."""
import json
import os
from pathlib import Path

def load_config(root):
    config = json.loads((root / 'config.example.json').read_text())
    override = os.environ.get('PAPER_CALENDAR_TEST_FONT')
    if override:
        config['font_path'] = override
    elif not Path(config['font_path']).exists():
        config['font_path'] = '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf'
        config['font_family'] = 'DejaVu Sans'
    if not Path(config['font_path']).is_file():
        raise RuntimeError('Install DejaVu Sans or set PAPER_CALENDAR_TEST_FONT')
    return config
