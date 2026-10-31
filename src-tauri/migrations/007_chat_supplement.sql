-- Expand answer provenance without rewriting messages, references or learning data.
CREATE TABLE chat_meta_next(message_id TEXT PRIMARY KEY REFERENCES messages(id),conversation_id TEXT NOT NULL REFERENCES conversations(id),turn_id TEXT NOT NULL UNIQUE REFERENCES chat_turns(id) DEFERRABLE INITIALLY DEFERRED,answer_kind TEXT CHECK(answer_kind IN('answered','insufficient_context','supplemented')),created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
INSERT INTO chat_meta_next SELECT * FROM chat_meta;
DROP TABLE chat_meta;
ALTER TABLE chat_meta_next RENAME TO chat_meta;
CREATE INDEX chat_meta_conversation ON chat_meta(conversation_id,message_id);
INSERT INTO m1_migrations VALUES(5,CAST(strftime('%s','now') AS INTEGER)*1000);
