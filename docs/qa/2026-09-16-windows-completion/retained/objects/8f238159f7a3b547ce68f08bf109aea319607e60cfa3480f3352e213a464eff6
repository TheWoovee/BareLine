#!/usr/bin/env python3
"""Internal child of the evidence supervisor; its descendants stay in the owned tree."""
import json
from pathlib import Path
import subprocess
import sys
import threading


def main(request_path):
    request = json.loads(Path(request_path).read_text(encoding='utf-8'))
    overflow = threading.Event(); errors = []
    process = None
    try:
        process = subprocess.Popen(request['command'], cwd=request['cwd'], stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        def drain(pipe, destination):
            try:
                with open(destination, 'xb') as stream:
                    remaining = request['max_output_bytes']
                    while raw := pipe.read1(4096):
                        retained = min(len(raw), remaining)
                        stream.write(raw[:retained]);stream.flush()
                        remaining -= retained
                        if len(raw) > retained:
                            # The exact retained prefix is evidence; truncation is failure.
                            overflow.set();return
                        if remaining == 0:
                            if pipe.read(1): overflow.set()
                            return
            except OSError as error:
                errors.append(str(error));overflow.set()
            finally: pipe.close()
        threads = [threading.Thread(target=drain, args=(pipe,request[key]),daemon=True)
                   for pipe,key in [(process.stdout,'stdout'),(process.stderr,'stderr')]]
        for thread in threads: thread.start()
        while process.poll() is None:
            if overflow.wait(0.05): break
        if process.poll() is None: process.terminate()
        try: code = process.wait(timeout=2)
        except subprocess.TimeoutExpired: code = None
        for thread in threads: thread.join(timeout=1)
        status = 'output_limit' if overflow.is_set() else 'inherited_pipe_open' if any(t.is_alive() for t in threads) else 'completed'
        result = dict(status=status,exit_code=code,errors=errors)
    except OSError as error:
        result = dict(status='spawn_error',exit_code=None,errors=[str(error)])
    Path(request['result']).write_text(json.dumps(result),encoding='utf-8')


if __name__=='__main__':main(sys.argv[1])
