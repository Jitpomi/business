"""Generate the checked-in typed configuration fields; security policies stay handwritten."""
import json,sys
from pathlib import Path
root=Path(__file__).resolve().parents[2]
spec=json.loads((root/'server/public/checkout-configuration.json').read_text())
lines=['define','# Generated field definitions. Run deploy/checkout/generate_schema.py --check in CI.']
for f in spec['fields']:
 k=f['kind'];t='integer' if k=='integer' else 'boolean' if k=='boolean' else 'string';a=''
 if k=='integer':a=f" @range({f['min']}..{f['max']})"
 elif k=='boolean':pass
 elif k=='choice':a=' @values('+','.join(json.dumps(x) for x in f['choices'])+')'
 else:
  pattern={'color':'#[A-Fa-f0-9]{6}','url':'(|https://[^\\s@]{1,1992})','locale':'[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8}){0,4}','timezone':'[A-Za-z0-9_+/-]{1,64}'}.get(k,'[\\s\\S]{0,'+str(f.get('max',200))+'}')
  if f.get('single_line'):pattern='[^\\r\\n]{0,'+str(f.get('max',200))+'}'
  a=' @regex('+json.dumps("^"+pattern+"$").replace('\\\\', '\\')+')'
 lines.append(f"attribute {f['attribute']}, value {t}{a};")
lines+=['attribute cfg_hash,value string @regex("^[a-f0-9]{64}$");','attribute cfg_active,value string;', 'entity commerce_configuration,owns cx_id @key,owns cx_created @card(1),owns cfg_hash @card(1),owns cx_currency @card(1..6),owns cx_mode @card(1..3),owns cx_kind @card(1..3),'+','.join('owns '+f['attribute']+' @card(1)' for f in spec['fields'])+';']
output='\n'.join(lines)+'\n'
p=root/'server/src/commerce-configuration-fields.tql'
if '--check' in sys.argv:
 if p.read_text()!=output:raise SystemExit('Regenerate checkout configuration fields')
else:p.write_text(output)
