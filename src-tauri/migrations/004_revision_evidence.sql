CREATE TABLE point_versions(point_id TEXT NOT NULL REFERENCES points(id),revision_id TEXT NOT NULL REFERENCES revisions(id),name TEXT NOT NULL,definition TEXT NOT NULL,evidence TEXT NOT NULL,PRIMARY KEY(point_id,revision_id));
INSERT INTO point_versions SELECT p.id,p.revision_id,p.name,COALESCE(m.definition,p.name),p.evidence FROM points p LEFT JOIN point_meta m ON m.point_id=p.id;
CREATE TABLE chunk_meta(chunk_rowid INTEGER PRIMARY KEY REFERENCES chunks(rowid) ON DELETE CASCADE,heading_path TEXT NOT NULL,content_hash TEXT NOT NULL,chunker_version TEXT NOT NULL);
INSERT INTO m1_migrations VALUES(2,CAST(strftime('%s','now') AS INTEGER)*1000);

