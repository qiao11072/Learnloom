ALTER TABLE model_calls ADD COLUMN response_json TEXT;
CREATE TABLE review_sessions(session_id TEXT PRIMARY KEY REFERENCES sessions(id), timezone TEXT NOT NULL, offset_minutes INTEGER NOT NULL, created_at INTEGER NOT NULL, selection TEXT NOT NULL, reasons TEXT NOT NULL DEFAULT '[]');
CREATE TABLE answer_drafts(question_id TEXT PRIMARY KEY REFERENCES questions(id), answer TEXT NOT NULL, revision INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE review_questions(question_id TEXT PRIMARY KEY REFERENCES questions(id), standard TEXT NOT NULL, difficulty TEXT NOT NULL, repeat_reason TEXT, rubric_details TEXT NOT NULL);
CREATE TABLE grading_assessments(attempt_id TEXT NOT NULL REFERENCES attempts(id),run_id TEXT NOT NULL REFERENCES runs(id),raw TEXT NOT NULL,PRIMARY KEY(attempt_id,run_id));
CREATE TABLE mastery_projection(point_id TEXT PRIMARY KEY REFERENCES points(id), algorithm_version TEXT NOT NULL, input_watermark TEXT NOT NULL, projection_updated_at INTEGER NOT NULL);
CREATE TRIGGER immutable_attempt BEFORE UPDATE OF answer,submitted_at,question_id ON attempts BEGIN SELECT RAISE(ABORT,'IMMUTABLE_ANSWER'); END;
CREATE TRIGGER immutable_question BEFORE UPDATE ON questions BEGIN SELECT RAISE(ABORT,'IMMUTABLE_QUESTION'); END;
INSERT INTO m1_migrations VALUES(4,CAST(strftime('%s','now') AS INTEGER)*1000);
