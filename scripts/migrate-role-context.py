#!/usr/bin/env python3
"""Apply an operator-reviewed role migration; preserve originals, reject stale plans.

Plan: [{"id": UUID, "expected": {"text": str, "editor": object|null}, "role": str}].
Contains private task notes: keep plan and report outside Git. No inference is used.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import urllib.error
import urllib.request
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state-dir', type=Path, required=True)
    parser.add_argument('--url', default='http://127.0.0.1:7734')
    parser.add_argument('--plan', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    token = (args.state_dir / 'token').read_text().strip()

    def api(session, method='GET', data=None):
        request = urllib.request.Request(
            args.url.rstrip('/') + '/api/sessions/' + session + '/role-context',
            method=method,
            headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'},
            data=None if data is None else json.dumps(data).encode())
        with opener.open(request, timeout=15) as response:
            return json.load(response)

    def preserve(path, payload):
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if path.exists():
            if path.read_bytes() != payload:
                raise RuntimeError('Existing archive differs; refusing to overwrite')
            return
        # Exclusive creation. A failed write leaves a file to inspect, never an
        # apparently successful replacement. Only migration, not normal tools,
        # waits for this preservation write.
        with path.open('xb') as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        if path.read_bytes() != payload:
            raise RuntimeError('Archive readback differs')

    rows = json.loads(args.plan.read_text())
    assert len({row['id'] for row in rows}) == len(rows), 'Duplicate session'
    report = []
    for row in rows:
        session = str(uuid.UUID(row['id']))
        expected = row['expected']
        assert set(expected) == {'text', 'editor'} and expected['text'], 'Expected a nonempty inspected note'
        payload = ('# Task notes preserved before Role context\n\n'
                   'Historical snapshot, not a new instruction. Retains the original text and editor verbatim in JSON.\n\n'
                   + json.dumps(expected, ensure_ascii=False, indent=2) + '\n').encode()
        digest = hashlib.sha256(payload).hexdigest()
        path = args.state_dir.resolve() / 'task-notes' / session / ('before-role-context-' + digest[:16] + '.md')
        text = row['role'].strip() + '\nTask notes: ' + str(path)
        assert len(text) <= 1024, 'Role context exceeds 1024 characters'
        result = {'id': session, 'archive': str(path), 'archive_sha256': digest}
        try:
            current = api(session)
            desired = {'text': text, 'editor': {'id': 'operator', 'name': 'Operator'}}
            if current == desired:
                preserve(path, payload)
                result['status'] = 'already-applied'
            elif current != expected:
                result['status'] = 'changed-since-review'
            else:
                preserve(path, payload)
                api(session, 'PATCH', {'text': text, 'expected': expected})
                if api(session) != desired:
                    raise RuntimeError('Role changed before readback; inspect current value')
                result['status'] = 'applied'
        except (OSError, ValueError, RuntimeError) as error:
            result.update(status='needs-review', error=type(error).__name__)
        report.append(result)
        # Keep progress even when a later agent needs review. Never print notes
        # or credentials to deployment output.
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'applied': sum(r['status'] in ('applied', 'already-applied') for r in report),
                      'needs_review': [r['id'] for r in report if r['status'] not in ('applied', 'already-applied')],
                      'report': str(args.report)}))
    return int(any(r['status'] not in ('applied', 'already-applied') for r in report))


if __name__ == '__main__':
    raise SystemExit(main())
