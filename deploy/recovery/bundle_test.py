import importlib.util, json, tempfile, unittest, zipfile
from pathlib import Path
spec=importlib.util.spec_from_file_location('bundle',Path(__file__).with_name('bundle.py'));bundle=importlib.util.module_from_spec(spec);spec.loader.exec_module(bundle)
class RecoveryBundle(unittest.TestCase):
 def test_roundtrip_and_refuse_existing_target(self):
  with tempfile.TemporaryDirectory() as root:
   r=Path(root);(r/'source').mkdir();(r/'source/data').write_bytes(b'retained work');bundle.pack(r/'source',r/'bundle.zip');bundle.unpack(r/'bundle.zip',r/'restored');self.assertEqual((r/'restored/data').read_bytes(),b'retained work')
   with self.assertRaises(ValueError):bundle.unpack(r/'bundle.zip',r/'restored')
 def test_reject_traversal_and_content_corruption_before_creating_destination(self):
  with tempfile.TemporaryDirectory() as root:
   r=Path(root)
   for index,name in enumerate(['../outside','data']):
    p=r/f'{index}.zip'
    with zipfile.ZipFile(p,'w') as z:z.writestr(name,b'bad');z.writestr('MANIFEST.json',json.dumps({'version':1,'files':{name:{'sha256':'wrong','size':3}}}))
    with self.assertRaises(ValueError):bundle.unpack(p,r/f'restored{index}')
    self.assertFalse((r/f'restored{index}').exists())
if __name__=='__main__':unittest.main()
