CREATE TABLE run_page_bases(run_id TEXT NOT NULL REFERENCES runs(id),page_id TEXT NOT NULL,revision_id TEXT,PRIMARY KEY(run_id,page_id));
PRAGMA user_version=2;
