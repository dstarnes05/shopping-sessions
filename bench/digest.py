import hashlib, sys
from pathlib import Path
sys.path.insert(0, "scripts")
from compare import load
from run_fixtures import SCHEMAS
for table, schema in SCHEMAS.items():
    keys = [k for k in ("user_id","campaign_id","day") if k in schema]
    data = load(Path(sys.argv[1]),table).select(list(schema)).sort(keys).write_csv().encode()
    print(table, hashlib.sha256(data).hexdigest())
