"""Isolated PostgreSQL protocol experiment, NOT Utopia's action implementation.

No production tables, routes, credentials or generic worker integration. Targets
are a test server on loopback only. The model establishes dispatch identities and
fault boundaries; it does not establish Utopia RBAC, templates or DNS pinning.
"""
import hashlib
import http.client
import json
import os
import uuid
from contextlib import contextmanager

import psycopg
from psycopg import sql
from psycopg.rows import dict_row


class Conflict(Exception):
    pass


class Model:
    def __init__(self, dsn, port):
        self.dsn, self.port = dsn, port
        self.schema = 'action_model_' + uuid.uuid4().hex
        with psycopg.connect(dsn, autocommit=True) as c:
            c.execute(sql.SQL('CREATE SCHEMA {}').format(sql.Identifier(self.schema)))
        with self.connect() as c:
            c.execute('''CREATE TABLE definition (
                id integer PRIMARY KEY, revision integer NOT NULL,
                enabled boolean NOT NULL, granted boolean NOT NULL,
                editor boolean NOT NULL);
                INSERT INTO definition VALUES(1,1,true,true,true);
                CREATE TABLE runs (
                    id uuid PRIMARY KEY, actor text NOT NULL, scope uuid,
                    request_id uuid NOT NULL, fingerprint text NOT NULL,
                    revision integer NOT NULL, state text NOT NULL,
                    token uuid, status integer, capture text, excerpt text,
                    UNIQUE NULLS NOT DISTINCT(actor, scope, request_id));''')

    @contextmanager
    def connect(self):
        with psycopg.connect(self.dsn, row_factory=dict_row) as c:
            c.execute(sql.SQL('SET search_path TO {}').format(sql.Identifier(self.schema)))
            yield c

    def close(self):
        with psycopg.connect(self.dsn, autocommit=True) as c:
            c.execute(sql.SQL('DROP SCHEMA {} CASCADE').format(sql.Identifier(self.schema)))

    def definition(self, **changes):
        with self.connect() as c:
            for key, value in changes.items():
                assert key in ('revision', 'enabled', 'granted', 'editor')
                c.execute(sql.SQL('UPDATE definition SET {}=%s').format(sql.Identifier(key)), (value,))

    @staticmethod
    def authorize(row, revision):
        if not row['enabled'] or not row['granted'] or not row['editor']:
            raise PermissionError('denied at dispatch boundary')
        if row['revision'] != revision:
            raise Conflict('preview_stale')

    def preview(self, args):
        with self.connect() as c:
            d = c.execute('SELECT * FROM definition WHERE id=1').fetchone()
            self.authorize(d, d['revision'])
            return {'revision': d['revision'], 'args': args}

    def prepare(self, request_id, args, revision=1, scope=None, actor='editor'):
        fingerprint = hashlib.sha256(json.dumps([revision, args], sort_keys=True,
                                               separators=(',', ':'), allow_nan=False).encode()).hexdigest()
        with self.connect() as c:
            # Replays are authorized as well. This model has one definition, not
            # production role tables; it tests the cutoff, not the RBAC resolver.
            d = c.execute('SELECT * FROM definition WHERE id=1 FOR SHARE').fetchone()
            self.authorize(d, revision)
            new = c.execute('''INSERT INTO runs(id,actor,scope,request_id,fingerprint,revision,state)
                VALUES(%s,%s,%s,%s,%s,%s,'prepared') ON CONFLICT DO NOTHING RETURNING *''',
                (uuid.uuid4(), actor, scope, request_id, fingerprint, revision)).fetchone()
            if new:
                return new, True
            old = c.execute('SELECT * FROM runs WHERE actor=%s AND scope IS NOT DISTINCT FROM %s AND request_id=%s',
                            (actor, scope, request_id)).fetchone()
            if old['fingerprint'] != fingerprint:
                raise Conflict('execution_request_reused')
            return old, False

    def gate(self, run_id, revision, commit_uncertain=False):
        token = uuid.uuid4()
        with self.connect() as c:
            d = c.execute('SELECT * FROM definition WHERE id=1 FOR SHARE').fetchone()
            self.authorize(d, revision)
            got = c.execute("UPDATE runs SET state='dispatching',token=%s WHERE id=%s AND state='prepared' RETURNING id",
                            (token, run_id)).fetchone()
        # Commit happened, but caller cannot know: it must NOT send. Deliberate
        # fault injection after real commit models a lost commit acknowledgement.
        if commit_uncertain:
            raise ConnectionError('injected lost gate commit acknowledgement')
        return token if got else None

    def read(self, run_id):
        with self.connect() as c:
            return c.execute('SELECT * FROM runs WHERE id=%s', (run_id,)).fetchone()

    def expire(self, run_id):
        with self.connect() as c:
            c.execute("UPDATE runs SET state='not_sent' WHERE id=%s AND state='prepared'", (run_id,))

    def recover(self):
        with self.connect() as c:
            c.execute("UPDATE runs SET state='outcome_unknown' WHERE state='dispatching'")

    def observe(self, run_id, token, state, status=None, capture='absent', excerpt=''):
        with self.connect() as c:
            c.execute('''UPDATE runs SET state=%s,status=%s,capture=%s,excerpt=%s
                WHERE id=%s AND token=%s AND state IN ('dispatching','outcome_unknown')''',
                (state, status, capture, excerpt, run_id, token))

    def send(self, run_id, token, path, fail_write=False):
        # Fixed loopback destination: this is intentionally NOT a reusable sender.
        # http.client neither follows redirects nor retries requests. No proxies.
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=0.3)
        status, capture, excerpt = None, 'absent', ''
        try:
            conn.request('POST', path, body=b'{}', headers={'Content-Type': 'application/json'})
            response = conn.getresponse()
            status = response.status
            try:
                body = response.read(1025)
                capture = 'truncated' if len(body) > 1024 else 'complete'
                excerpt = body[:256].decode('utf-8', errors='replace')
            except (OSError, http.client.HTTPException):
                capture = 'read_error'
        except (OSError, http.client.HTTPException):
            pass
        finally:
            conn.close()
        if fail_write:
            raise ConnectionError('injected observation persistence failure')
        self.observe(run_id, token, 'response_received' if status is not None else 'outcome_unknown',
                     status, capture, excerpt)

    def execute(self, request_id, args, revision=1, scope=None, path='/ok'):
        row, created = self.prepare(request_id, args, revision, scope)
        # Only the original creator proceeds. Returning prepared after a restart
        # does not grant replay callers permission to take over and send.
        if created:
            token = self.gate(row['id'], revision)
            if token:
                self.send(row['id'], token, path)
        return self.read(row['id'])
