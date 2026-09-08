# SPDX-License-Identifier: MPL-2.0
"""Bounded metadata snapshots of an explicitly owned benchmark state directory."""
import os
from pathlib import Path
import stat
import threading
import time

CATEGORIES = ('recovery', 'extensions', 'temporary', 'user_cache', 'other')


def snapshot(root, max_entries=100000):
    root = Path(root).resolve(strict=True)
    totals = {category: 0 for category in CATEGORIES}
    pending = [(root, 0)]
    entries = 0
    while pending:
        directory, depth = pending.pop()
        if depth > 64:
            raise ValueError('owned disk traversal exceeds depth quota')
        with os.scandir(directory) as children:
            for entry in children:
                entries += 1
                if entries > max_entries:
                    raise ValueError('owned disk traversal exceeds entry quota')
                try:
                    info = entry.stat(follow_symlinks=False)
                except FileNotFoundError:
                    continue  # A concurrently removed file has no current footprint.
                if stat.S_ISLNK(info.st_mode) or getattr(info, 'st_file_attributes', 0) & 0x400:
                    raise ValueError('owned disk traversal refuses links/reparse points')
                path = Path(entry.path)
                if stat.S_ISDIR(info.st_mode):
                    pending.append((path, depth + 1))
                elif stat.S_ISREG(info.st_mode):
                    first = path.relative_to(root).parts[0]
                    category = {'recovery': 'recovery', 'extensions': 'extensions',
                                'temporary': 'temporary', 'user-state': 'user_cache'}.get(first, 'other')
                    totals[category] += info.st_size
                else:
                    raise ValueError('owned disk traversal encountered a non-regular object')
    return totals


class DiskSampler:
    def __init__(self, root, interval=.1):
        self.root = Path(root).resolve(strict=True)
        self.interval = interval
        self.baseline = snapshot(self.root)
        self.peak = dict(self.baseline)
        self.final = dict(self.baseline)
        self.samples = 1
        self.errors = []
        self.stop_event = threading.Event()
        self.thread = None

    def observe(self):
        value = snapshot(self.root)
        self.final = value
        self.peak = {key: max(self.peak[key], value[key]) for key in CATEGORIES}
        self.samples += 1

    def start(self):
        def worker():
            while not self.stop_event.wait(self.interval):
                try:
                    self.observe()
                    if self.samples >= 2000:
                        raise ValueError('owned disk sampling quota reached')
                except (OSError, ValueError) as error:
                    self.errors.append(str(error))
                    return
        self.thread = threading.Thread(target=worker, daemon=True)
        self.thread.start()
        return self

    def stop(self):
        self.stop_event.set()
        if self.thread:
            self.thread.join(timeout=2)
            if self.thread.is_alive():
                self.errors.append('owned disk sampler did not stop')
                return self
        try:
            self.observe()
        except (OSError, ValueError) as error:
            self.errors.append(str(error))
        return self

    def metrics(self):
        if self.errors:
            raise RuntimeError('incomplete owned disk measurement: ' + '; '.join(self.errors))
        values = {'owned_disk_sample_count': self.samples,
                  'owned_disk_sample_interval_us': int(self.interval * 1_000_000)}
        for category in CATEGORIES:
            values.update({f'{category}_disk_before_bytes': self.baseline[category],
                           f'{category}_disk_after_bytes': self.final[category],
                           f'{category}_disk_peak_growth_bytes': max(0, self.peak[category] - self.baseline[category]),
                           f'{category}_disk_released_bytes': max(0, self.baseline[category] - self.final[category])})
        return values


def isolated_environment(root):
    root = Path(root).resolve(strict=True)
    temporary = root / 'temporary'
    local = root / 'user-state' / 'local'
    roaming = root / 'user-state' / 'roaming'
    for path in (temporary, local, roaming):
        path.mkdir(parents=True, exist_ok=True)
    return {**{key: value for key, value in os.environ.items() if not key.startswith('=')}, 'TEMP': str(temporary), 'TMP': str(temporary),
            'LOCALAPPDATA': str(local), 'APPDATA': str(roaming)}
