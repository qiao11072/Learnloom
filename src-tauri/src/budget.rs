use crate::Result;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Budget {
    pub context: usize,
    pub margin: usize,
    pub output: usize,
    pub input: usize,
    pub system: usize,
    pub task: usize,
    pub history: usize,
    pub evidence: usize,
}
impl Budget {
    pub fn new(context: usize) -> Result<Self> {
        if context < 2048 {
            return Err("CONTEXT_BUDGET_INSUFFICIENT".into());
        }
        let margin = 256.max(context.div_ceil(10));
        let output = 2048.min(context / 4);
        let input = context - margin - output;
        let system = input / 4;
        let task = input / 4;
        let history = 15 * input / 100;
        let evidence = input - system - task - history;
        Ok(Self {
            context,
            margin,
            output,
            input,
            system,
            task,
            history,
            evidence,
        })
    }
    pub fn check(&self, system: &str, task: &str, history: &str, evidence: &str) -> Result<()> {
        if system.len() + 64 > self.system {
            return Err("CONTEXT_BUDGET_INSUFFICIENT".into());
        }
        // UTF-8 byte estimate plus per-message overhead; no answer truncation.
        let total = system.len() + task.len() + history.len() + evidence.len() + 4 * 64;
        if total > self.input {
            return Err("INPUT_TOO_LONG".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub start: usize,
    pub end: usize,
    pub text: String,
}
pub fn split(text: &str, capacity: usize, max_chunks: usize) -> Result<Vec<Chunk>> {
    if capacity < 64 {
        return Err("CONTEXT_BUDGET_INSUFFICIENT".into());
    }
    let mut chunks = vec![];
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + capacity).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if chunks.len() >= max_chunks {
            return Err("SOURCE_TOO_LARGE".into());
        }
        chunks.push(Chunk {
            start,
            end,
            text: text[start..end].into(),
        });
        start = end;
    }
    Ok(chunks)
}
