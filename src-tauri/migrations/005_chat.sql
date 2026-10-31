CREATE TABLE conversations(id TEXT PRIMARY KEY,title TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,active_message_id TEXT REFERENCES messages(id) DEFERRABLE INITIALLY DEFERRED);
CREATE INDEX conversations_recent ON conversations(updated_at DESC,id);
CREATE TABLE chat_turns(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id),ordinal INTEGER NOT NULL,user_text TEXT NOT NULL CHECK(length(trim(user_text))>0),created_at INTEGER NOT NULL,assistant_id TEXT NOT NULL UNIQUE REFERENCES messages(id) DEFERRABLE INITIALLY DEFERRED,UNIQUE(conversation_id,ordinal));
CREATE TABLE chat_meta(message_id TEXT PRIMARY KEY REFERENCES messages(id),conversation_id TEXT NOT NULL REFERENCES conversations(id),turn_id TEXT NOT NULL UNIQUE REFERENCES chat_turns(id) DEFERRABLE INITIALLY DEFERRED,answer_kind TEXT CHECK(answer_kind IN('answered','insufficient_context')),created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
CREATE INDEX chat_meta_conversation ON chat_meta(conversation_id,message_id);
CREATE TABLE chat_contexts(run_id TEXT PRIMARY KEY REFERENCES runs(id),message_id TEXT NOT NULL REFERENCES messages(id),query TEXT NOT NULL,history_json TEXT NOT NULL,evidence_json TEXT NOT NULL,budget_json TEXT NOT NULL,rewrite_status TEXT NOT NULL,created_at INTEGER NOT NULL);
INSERT INTO m1_migrations VALUES(3,CAST(strftime('%s','now') AS INTEGER)*1000);
