#!/usr/bin/env python3
"""Publish a freshness receipt only after GitHub has acknowledged the encrypted artifact."""
import json, os, sys
from pathlib import Path
from scheduled import private, request, endpoint
try:
    path=Path(sys.argv[1]);v=json.loads(path.read_text());v['artifact_url']=os.environ['BACKUP_ARTIFACT_URL']
    receipt=path.with_name('uploaded-receipt.json');private(receipt,json.dumps(v).encode())
    result=request(endpoint('monitor','backup'),os.environ['BUSINESS_BACKUP_MONITOR_TOKEN'],receipt)
    if result!={'accepted':True}:raise ValueError('Freshness acknowledgement rejected')
    print('Encrypted artifact and verified graph checkpoint acknowledged')
except Exception:
    print('Backup freshness was not acknowledged; investigate this run without discarding existing backups',file=sys.stderr);sys.exit(1)
