import importlib.util,json,sqlite3,unittest
from unittest.mock import patch
from pathlib import Path
spec=importlib.util.spec_from_file_location('scheduled',Path(__file__).with_name('scheduled.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class ScheduledCheckpoint(unittest.TestCase):
 def test_backup_follows_explicit_development_versions_without_accepting_production(self):
  config={'TYPEDB_ADDR':'https://example.invalid:80','TYPEDB_DB':'business_native_dev_v5','TYPEDB_USERNAME':'business_dev_backup','TYPEDB_PASSWORD':'synthetic-fixture-only','TYPEDB_TLS':'true'}
  for name in ['business_native_dev_v2','business_native_dev_v5','business_native_dev_v6','business_native_dev_v9999']:
   self.assertTrue(m.valid_database_configuration({**config,'TYPEDB_DB':name}))
  for name in ['business_prod','business_dev','business_backend_test_fixture','business_native_dev_v01','business_native_dev_v1','business_native_dev_v10000']:
   self.assertFalse(m.valid_database_configuration({**config,'TYPEDB_DB':name}))
  self.assertFalse(m.valid_database_configuration({**config,'TYPEDB_TLS':'false'}))
  self.assertFalse(m.valid_database_configuration({**config,'TYPEDB_USERNAME':'admin'}))
  self.assertFalse(m.valid_database_configuration({**config,'UNREVIEWED':'value'}))
 def test_production_profile_cannot_reuse_development_source_or_send_to_development_stores(self):
  config={'TYPEDB_ADDR':'https://example.invalid:80','TYPEDB_DB':'business_prod','TYPEDB_USERNAME':'business_prod_backup','TYPEDB_PASSWORD':'synthetic-fixture-only','TYPEDB_TLS':'true'}
  self.assertTrue(m.valid_database_configuration(config,'production'))
  for key,value in [('TYPEDB_DB','business_native_dev_v10'),('TYPEDB_USERNAME','business_dev_backup'),('TYPEDB_TLS','false')]:
   self.assertFalse(m.valid_database_configuration({**config,key:value},'production'))
  self.assertFalse(m.valid_database_configuration(config,'development'))
  self.assertFalse(m.valid_database_configuration(config,'unrecognized'))
  with patch.dict(m.os.environ,{'BUSINESS_BACKUP_PROFILE':'production'}):
   self.assertIn('production-secrets',m.endpoint('vault','snapshot'))
   with self.assertRaises(ValueError):m.request('https://jitpomi-business-dev-secrets.steep-hill-e6f3.workers.dev/snapshot','x'*43)
  with patch.dict(m.os.environ,{'BUSINESS_BACKUP_PROFILE':'unknown'}):
   with self.assertRaises(ValueError):m.endpoint('vault','snapshot')
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
