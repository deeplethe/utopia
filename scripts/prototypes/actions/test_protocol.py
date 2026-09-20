import concurrent.futures
import http.server
import os
import socket
import threading
import unittest
import uuid
from protocol import Model, Conflict


class Remote(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        self.rfile.read(int(self.headers.get('Content-Length', 0)))
        with self.server.guard:
            self.server.requests.append(self.path)
            self.server.effects += 1
        if self.path == '/drop':
            # A real remote effect is committed BEFORE losing the response.
            self.server.effect_committed.set()
            self.server.release.wait(5)
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
            return
        if self.path == '/slow-body':
            self.send_response(202)
            self.send_header('Content-Length', '100')
            self.end_headers()
            self.wfile.flush()
            self.server.release.wait(2)
            return
        status = int(self.path[1:]) if self.path[1:].isdigit() else 200
        body = ('汉字' * 1000).encode() if self.path == '/large' else b'observed response'
        self.send_response(status)
        if status == 302:
            self.send_header('Location', '/second-target')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class ProtocolTests(unittest.TestCase):
    def setUp(self):
        self.remote = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Remote)
        self.remote.requests, self.remote.effects = [], 0
        self.remote.guard = threading.Lock()
        self.remote.effect_committed = threading.Event()
        self.remote.release = threading.Event()
        self.thread = threading.Thread(target=self.remote.serve_forever, daemon=True)
        self.thread.start()
        self.model = Model(os.environ['UTOPIA_DATABASE_URL'], self.remote.server_port)
        self.key = uuid.uuid4()

    def tearDown(self):
        self.remote.release.set()
        self.remote.shutdown()
        self.remote.server_close()
        self.thread.join()
        self.model.close()

    def prepared(self):
        return self.model.prepare(self.key, {'n': 1})[0]

    def test_preview_has_neither_run_nor_network_effect(self):
        self.assertEqual(self.model.preview({'n': 1})['revision'], 1)
        with self.model.connect() as c:
            self.assertEqual(c.execute('SELECT count(*) n FROM runs').fetchone()['n'], 0)
        self.assertEqual(self.remote.effects, 0)

    def test_stale_preview_and_revoked_authority_never_dispatch(self):
        self.model.definition(revision=2)
        with self.assertRaises(Conflict):
            self.model.execute(self.key, {}, revision=1)
        self.model.definition(revision=1)
        row = self.prepared()
        for field in ['enabled', 'granted', 'editor']:
            self.model.definition(**{field: False})
            with self.assertRaises(PermissionError):
                self.model.gate(row['id'], 1)
            with self.assertRaises(PermissionError):
                self.model.execute(self.key, {'n': 1})
            self.model.definition(**{field: True})
        self.assertEqual(self.remote.effects, 0)

    def test_concurrent_duplicate_registry_scope_has_one_run_and_one_dispatch(self):
        barrier = threading.Barrier(8)
        def call(_):
            barrier.wait(timeout=5)
            return self.model.execute(self.key, {'n': 1})['id']
        with concurrent.futures.ThreadPoolExecutor(8) as executor:
            ids = list(executor.map(call, range(8)))
        self.assertEqual(len(set(ids)), 1)
        self.assertEqual(self.remote.effects, 1)

    def test_kb_scope_retries_and_new_explicit_operation(self):
        scope = uuid.uuid4()
        a = self.model.execute(self.key, {'n': 1}, scope=scope)
        b = self.model.execute(self.key, {'n': 1}, scope=scope)
        self.assertEqual(a['id'], b['id'])
        self.model.execute(uuid.uuid4(), {'n': 1}, scope=scope)
        self.assertEqual(self.remote.effects, 2)

    def test_same_identity_cannot_change_inputs(self):
        self.model.execute(self.key, {'n': 1})
        with self.assertRaises(Conflict):
            self.model.execute(self.key, {'n': 2})
        self.assertEqual(self.remote.effects, 1)

    def test_prepared_insert_failure_cannot_send(self):
        with self.model.connect() as c:
            c.execute("ALTER TABLE runs ADD CONSTRAINT injected_failure CHECK (state <> 'prepared')")
        with self.assertRaises(Exception):
            self.model.execute(self.key, {})
        self.assertEqual(self.remote.effects, 0)

    def test_prepared_replay_is_not_a_recovery_sender(self):
        row = self.prepared()
        self.assertEqual(self.model.execute(self.key, {'n': 1})['state'], 'prepared')
        self.model.expire(row['id'])
        self.assertIsNone(self.model.gate(row['id'], 1))
        self.assertEqual(self.remote.effects, 0)

    def test_expiration_and_dispatch_gate_are_mutually_exclusive(self):
        row = self.prepared()
        barrier = threading.Barrier(2)
        def expire():
            barrier.wait(timeout=5)
            self.model.expire(row['id'])
        def gate():
            barrier.wait(timeout=5)
            return self.model.gate(row['id'], 1)
        with concurrent.futures.ThreadPoolExecutor(2) as ex:
            a, b = ex.submit(expire), ex.submit(gate)
            a.result()
            token = b.result()
        self.assertEqual(self.model.read(row['id'])['state'], 'dispatching' if token else 'not_sent')
        self.assertIsNone(self.model.gate(row['id'], 1))

    def test_lost_gate_commit_acknowledgement_does_not_authorize_send(self):
        row = self.prepared()
        with self.assertRaises(ConnectionError):
            self.model.gate(row['id'], 1, commit_uncertain=True)
        self.assertEqual(self.model.read(row['id'])['state'], 'dispatching')
        self.model.recover()
        self.assertEqual(self.model.execute(self.key, {'n': 1})['state'], 'outcome_unknown')
        self.assertEqual(self.remote.effects, 0)

    def test_gate_then_crash_before_send_is_conservatively_unknown(self):
        row = self.prepared()
        self.model.gate(row['id'], 1)
        self.model.recover()
        self.assertEqual(self.model.execute(self.key, {'n': 1})['state'], 'outcome_unknown')
        self.assertEqual(self.remote.effects, 0)

    def test_remote_effect_then_dropped_response_is_not_retried(self):
        with concurrent.futures.ThreadPoolExecutor(1) as ex:
            future = ex.submit(self.model.execute, self.key, {'n': 1}, path='/drop')
            try:
                self.assertTrue(self.remote.effect_committed.wait(3))
                self.assertEqual(self.remote.effects, 1)
            finally:
                self.remote.release.set()
            row = future.result(timeout=3)
        self.assertEqual(row['state'], 'outcome_unknown')
        self.model.recover()
        self.model.execute(self.key, {'n': 1})
        self.assertEqual(self.remote.effects, 1)

    def test_http_status_is_observation_not_business_success(self):
        for status in [200, 202, 400, 500]:
            row = self.model.execute(uuid.uuid4(), {}, path=f'/{status}')
            self.assertEqual(row['state'], 'response_received')
            self.assertEqual(row['status'], status)
            self.assertEqual(row['capture'], 'complete')
        self.assertEqual(self.remote.effects, 4)

    def test_body_timeout_preserves_observed_status(self):
        row = self.model.execute(self.key, {}, path='/slow-body')
        self.assertEqual((row['state'], row['status'], row['capture']), ('response_received', 202, 'read_error'))
        self.assertEqual(self.remote.effects, 1)

    def test_body_cap_is_reported_and_excerpt_is_unicode(self):
        row = self.model.execute(self.key, {}, path='/large')
        self.assertEqual(row['capture'], 'truncated')
        self.assertIsInstance(row['excerpt'], str)
        self.assertLessEqual(len(row['excerpt']), 256)
        self.assertEqual(self.remote.effects, 1)

    def test_observation_write_failure_recovery_never_resends(self):
        row = self.prepared()
        token = self.model.gate(row['id'], 1)
        with self.assertRaises(ConnectionError):
            self.model.send(row['id'], token, '/ok', fail_write=True)
        self.model.recover()
        self.assertEqual(self.model.execute(self.key, {'n': 1})['state'], 'outcome_unknown')
        self.assertEqual(self.remote.effects, 1)

    def test_late_observation_uses_original_token_without_dispatch(self):
        row = self.prepared()
        token = self.model.gate(row['id'], 1)
        self.model.recover()
        self.model.observe(row['id'], uuid.uuid4(), 'response_received', 200)
        self.assertEqual(self.model.read(row['id'])['state'], 'outcome_unknown')
        self.model.observe(row['id'], token, 'response_received', 202)
        self.assertEqual(self.model.execute(self.key, {'n': 1})['status'], 202)
        self.assertEqual(self.remote.effects, 0)

    def test_redirect_is_an_observed_response_without_a_second_request(self):
        row = self.model.execute(self.key, {}, path='/302')
        self.assertEqual(row['status'], 302)
        self.assertEqual(self.remote.requests, ['/302'])

    def test_proxy_environment_cannot_redirect_loopback_experiment(self):
        old = os.environ.get('HTTP_PROXY')
        os.environ['HTTP_PROXY'] = 'http://127.0.0.1:1'
        try:
            self.assertEqual(self.model.execute(self.key, {})['status'], 200)
        finally:
            if old is None:
                os.environ.pop('HTTP_PROXY', None)
            else:
                os.environ['HTTP_PROXY'] = old


if __name__ == '__main__':
    unittest.main(verbosity=2)
