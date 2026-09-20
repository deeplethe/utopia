"""Proposed write-time lock boundary on PostgreSQL; not production API validation."""
import concurrent.futures
import os
import time
import unittest
import uuid
import psycopg
from psycopg import sql

class DeclarationLock(unittest.TestCase):
    def test_share_lock_keeps_unit_stable_through_definition_commit(self):
        schema='unit_model_'+uuid.uuid4().hex
        dsn=os.environ['UTOPIA_DATABASE_URL']
        with psycopg.connect(dsn,autocommit=True) as admin:
            admin.execute(sql.SQL('CREATE SCHEMA {}').format(sql.Identifier(schema)))
            admin.execute(sql.SQL('CREATE TABLE {}.attribute(id int PRIMARY KEY, unit text); INSERT INTO {}.attribute VALUES(1,\'USD\')').format(sql.Identifier(schema),sql.Identifier(schema)))
            def connection():
                c=psycopg.connect(dsn)
                c.execute(sql.SQL('SET search_path TO {}').format(sql.Identifier(schema)))
                return c
            try:
                with connection() as writer:
                    pid=writer.execute('SELECT pg_backend_pid()').fetchone()[0]
                    self.assertEqual(writer.execute('SELECT unit FROM attribute WHERE id=1 FOR SHARE').fetchone()[0],'USD')
                    def update():
                        with connection() as updater:
                            updater.execute("UPDATE attribute SET unit='EUR' WHERE id=1")
                    with concurrent.futures.ThreadPoolExecutor(1) as ex:
                        future=ex.submit(update)
                        try:
                            deadline=time.monotonic()+5
                            while time.monotonic()<deadline:
                                n=admin.execute('SELECT count(*) FROM pg_stat_activity WHERE %s=ANY(pg_blocking_pids(pid))',(pid,)).fetchone()[0]
                                if n: break
                                time.sleep(.01)
                            else: self.fail('did not observe actual blocked declaration update')
                            self.assertFalse(future.done())
                            self.assertEqual(writer.execute('SELECT unit FROM attribute WHERE id=1').fetchone()[0],'USD')
                        finally:
                            writer.commit()  # always release even if the observation fails
                        future.result(timeout=5)
                with connection() as later:
                    self.assertEqual(later.execute('SELECT unit FROM attribute WHERE id=1 FOR SHARE').fetchone()[0],'EUR')
                    # A subsequent USD expression must reject at validation.
                    self.assertNotEqual(later.execute('SELECT unit FROM attribute WHERE id=1').fetchone()[0],'USD')
            finally:
                admin.execute(sql.SQL('DROP SCHEMA {} CASCADE').format(sql.Identifier(schema)))

if __name__=='__main__':unittest.main(verbosity=2)
