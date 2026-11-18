import importlib.util,json,sqlite3,unittest
from pathlib import Path
spec=importlib.util.spec_from_file_location('scheduled',Path(__file__).with_name('scheduled.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class ScheduledCheckpoint(unittest.TestCase):
 def test_all_ciphertext_columns_survive_independent_sql_restore(self):
  row={'id':'12345678-1234-4234-8234-123456789abc','ciphertext':'A'*40,'created_at':'2026-11-18 07:00:00'};sql=m.vault_sql({'version':1,'rows':[row]});db=sqlite3.connect(':memory:');db.executescript(sql.decode());self.assertEqual(db.execute('SELECT * FROM secrets').fetchall(),[(row['id'],row['ciphertext'],row['created_at'])])
  with self.assertRaises(Exception):m.vault_sql({'version':1,'rows':[row,row]})
 def test_monitor_restores_acknowledged_idempotency_journal(self):
  snapshot={'version':1,'tables':{'lease':[],'monitors':[{'id':'backup','state':'{}'}],'outbox':[{'id':'immutable-id','body':'{}','created':10,'first_attempt':10,'accepted':'provider-id','uncertain':0}]}};db=sqlite3.connect(':memory:');db.executescript(m.monitor_sql(snapshot).decode());self.assertEqual(db.execute('SELECT id,accepted,first_attempt FROM outbox').fetchall(),[('immutable-id','provider-id',10)])
  snapshot['tables']['outbox'][0]['unexpected']='not silently dropped'
  with self.assertRaises(ValueError):m.monitor_sql(snapshot)
 def test_credentials_cannot_be_forwarded_to_arbitrary_hosts(self):
  with self.assertRaises(ValueError):m.request('https://untrusted.invalid/snapshot','x'*43)
if __name__=='__main__':unittest.main()
