//! Local, bounded lexical retrieval over the M1 indexes; immutable hit versions.
use crate::{hash, knowledge::Library, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
fn err(e: impl std::fmt::Display) -> String {
    format!("INDEX_ERROR: {e}")
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hit {
    pub page_id: String,
    pub revision_id: String,
    pub title: String,
    pub chunk_id: i64,
    pub ordinal: i64,
    pub anchor: String,
    pub heading: String,
    pub excerpt: String,
    pub content_hash: String,
    pub score: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub query: String,
    pub status: String,
    pub items: Vec<Hit>,
    pub excluded_index_pending_count: usize,
    pub excluded_unavailable_count: usize,
    pub damaged_index_count: usize,
    pub limited: bool,
}
fn han(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c)
}
pub fn query_terms(query: &str) -> Vec<(String, f64)> {
    let mut out = BTreeMap::<String, f64>::new();
    let mut run = String::new();
    let mut prior = None;
    let stop = "的了是在与和为如何什么为何怎么请问一个这种多少是否可以有无到时中对";
    for c in query.to_lowercase().chars().chain(std::iter::once(' ')) {
        if c.is_ascii_alphanumeric() {
            run.push(c);
            prior = None;
            continue;
        }
        if !run.is_empty() {
            let token = std::mem::take(&mut run);
            if ![
                "what", "how", "why", "the", "and", "with", "does", "this", "that", "from", "when",
                "where", "which", "are", "is", "an", "to", "of", "in", "a", "explain",
            ]
            .contains(&token.as_str())
            {
                out.insert(token, 3.0);
            }
        }
        if han(c) {
            if !stop.contains(c) {
                out.entry(c.to_string()).or_insert(0.25);
            }
            if let Some(old) = prior {
                let pair = format!("{old}{c}");
                if ![
                    "如何", "什么", "为何", "怎么", "请问", "多少", "是否", "可以", "一个", "这种",
                    "哪些", "说明", "一下",
                ]
                .contains(&pair.as_str())
                {
                    out.insert(pair, 2.0);
                }
            }
            prior = Some(c);
        } else {
            prior = None;
        }
    }
    let mut terms: Vec<_> = out.into_iter().collect();
    terms.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    terms.truncate(64);
    terms
}
fn bounded_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.into();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].into()
}
pub fn excerpt(text: &str, terms: &[(String, f64)], max: usize) -> String {
    if text.len() <= max {
        return text.into();
    }
    let folded = text.to_lowercase();
    let position = terms
        .iter()
        .find_map(|(t, _)| folded.find(t))
        .unwrap_or(0)
        .min(text.len());
    let mut start = position.saturating_sub(max / 4);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + max).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    // Prefer a complete sentence/paragraph when available, retaining the matched term.
    if let Some((n, _)) = text[start..end]
        .char_indices()
        .rev()
        .find(|(n, c)| start + *n > position && matches!(c, '。' | '！' | '？' | '\n'))
    {
        end = start + n + text[start + n..].chars().next().unwrap().len_utf8();
    }
    text[start..end].to_string()
}
pub fn search(lib: &Library, query: &str, limit: usize) -> Result<SearchResult> {
    if query.chars().count() > 200 {
        return Err("QUERY_TOO_LONG".into());
    }
    let terms = query_terms(query);
    let limit = limit.clamp(1, 20);
    let tx = lib.db.conn.unchecked_transaction().map_err(err)?;
    let pending: i64 = tx
        .query_row(
            "SELECT count(*) FROM pages WHERE index_state IN('pending','failed')",
            [],
            |r| r.get(0),
        )
        .map_err(err)?;
    let unavailable: i64 = tx
        .query_row(
            "SELECT count(*) FROM pages WHERE index_state='unavailable'",
            [],
            |r| r.get(0),
        )
        .map_err(err)?;
    let damaged:i64=tx.query_row("SELECT count(*) FROM pages p WHERE p.index_state='ready' AND (p.current_revision<>p.indexed_revision OR NOT EXISTS(SELECT 1 FROM chunks c WHERE c.page_id=p.id AND c.revision_id=p.current_revision) OR EXISTS(SELECT 1 FROM chunks c WHERE c.page_id=p.id AND c.revision_id=p.current_revision AND NOT EXISTS(SELECT 1 FROM page_fts f WHERE f.rowid=c.rowid)))",[],|r|r.get(0)).map_err(err)?;
    let total: i64 = tx
        .query_row(
            "SELECT count(*) FROM pages WHERE index_state='ready'",
            [],
            |r| r.get(0),
        )
        .map_err(err)?;
    let mut scores = BTreeMap::<String, (f64, bool)>::new();
    let mut limited = false;
    for (term, weight) in &terms {
        let mut stmt=tx.prepare("SELECT t.page_id,t.weight FROM page_terms t JOIN pages p ON p.id=t.page_id WHERE t.term=? AND p.current_revision=t.revision_id AND p.indexed_revision=t.revision_id AND p.index_state='ready' ORDER BY t.weight DESC,t.page_id LIMIT 1001").map_err(err)?;
        let rows = stmt
            .query_map([term], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        limited |= rows.len() > 1000;
        let idf = (1.0 + (total as f64 + 1.0) / (rows.len() as f64 + 1.0)).ln();
        for (pid, title_weight) in rows.into_iter().take(1000) {
            let entry = scores.entry(pid).or_default();
            entry.0 += weight * idf * (if title_weight > 1 { 3.0 } else { 1.0 });
            entry.1 |= *weight >= 2.0;
        }
    }
    let mut phrases: Vec<String> = terms
        .iter()
        .filter(|(s, _)| s.chars().count() >= 3)
        .take(12)
        .map(|(s, _)| format!("\"{}\"", s.replace('"', "\"\"")))
        .collect();
    if query.chars().count() >= 3 && query.chars().all(han) {
        phrases.push(format!("\"{}\"", query));
    }
    if !phrases.is_empty() {
        let mut stmt=tx.prepare("SELECT c.page_id FROM page_fts f JOIN chunks c ON c.rowid=f.rowid JOIN pages p ON p.id=c.page_id WHERE page_fts MATCH ? AND c.revision_id=p.current_revision AND c.revision_id=p.indexed_revision AND p.index_state='ready' LIMIT 256").map_err(err)?;
        for row in stmt
            .query_map([phrases.join(" OR ")], |r| r.get::<_, String>(0))
            .map_err(err)?
        {
            let x = scores.entry(row.map_err(err)?).or_default();
            x.0 += 4.0;
            x.1 = true;
        }
    }
    let need_strong = terms.iter().any(|(_, w)| *w >= 2.0);
    let mut ranked: Vec<_> = scores
        .into_iter()
        .filter(|(_, (_, strong))| !need_strong || *strong)
        .map(|(id, (s, _))| (id, s))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    limited |= ranked.len() > 64;
    ranked.truncate(64);
    let mut hits = vec![];
    for (pid, score) in ranked {
        let mut stmt=tx.prepare("SELECT p.current_revision,p.title,c.rowid,c.ordinal,c.text,COALESCE(m.heading_path,'') FROM pages p JOIN chunks c ON c.page_id=p.id LEFT JOIN chunk_meta m ON m.chunk_rowid=c.rowid JOIN page_fts f ON f.rowid=c.rowid WHERE p.id=? AND c.revision_id=p.current_revision AND c.revision_id=p.indexed_revision AND p.index_state='ready' ORDER BY c.ordinal LIMIT 128").map_err(err)?;
        let rows = stmt
            .query_map([&pid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        let best = rows.into_iter().max_by(|a, b| {
            let value = |text: &str| {
                let text = text.to_lowercase();
                terms
                    .iter()
                    .filter(|(t, _)| text.contains(t))
                    .map(|(_, w)| *w)
                    .sum::<f64>()
            };
            value(&a.4).total_cmp(&value(&b.4)).then(b.3.cmp(&a.3))
        });
        if let Some((rev, title, rowid, ordinal, text, heading)) = best {
            hits.push(Hit {
                page_id: pid,
                revision_id: rev,
                title,
                chunk_id: rowid,
                ordinal,
                anchor: format!("chunk-{ordinal}"),
                heading,
                excerpt: excerpt(&text, &terms, 600),
                content_hash: hash(&text),
                score,
            });
        }
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.page_id.cmp(&b.page_id)));
    hits.truncate(limit);
    tx.commit().map_err(err)?;
    let status = if hits.is_empty() {
        if pending + unavailable + damaged > 0 {
            "index_unavailable"
        } else {
            "no_results"
        }
    } else {
        "ok"
    };
    Ok(SearchResult {
        query: query.into(),
        status: status.into(),
        items: hits,
        excluded_index_pending_count: pending as usize,
        excluded_unavailable_count: unavailable as usize,
        damaged_index_count: damaged as usize,
        limited,
    })
}
/// Related snippets are supplementary and always lower priority than lexical hits.
pub fn related(lib: &Library, hits: &[Hit], limit: usize) -> Result<Vec<Hit>> {
    let mut seen: BTreeSet<String> = hits.iter().map(|h| h.page_id.clone()).collect();
    let mut out = vec![];
    let tx = lib.db.conn.unchecked_transaction().map_err(err)?;
    for h in hits.iter().take(3) {
        let mut q=tx.prepare("SELECT p.id,p.current_revision,p.title,c.rowid,c.ordinal,c.text,COALESCE(m.heading_path,'') FROM page_links l JOIN pages p ON p.id=l.to_page JOIN chunks c ON c.page_id=p.id LEFT JOIN chunk_meta m ON m.chunk_rowid=c.rowid WHERE l.from_page=?1 AND l.from_revision=?2 AND p.index_state='ready' AND c.revision_id=p.current_revision AND c.revision_id=p.indexed_revision AND c.ordinal=0 LIMIT 3").map_err(err)?;
        let rows = q
            .query_map(params![h.page_id, h.revision_id], |r| {
                Ok(Hit {
                    page_id: r.get(0)?,
                    revision_id: r.get(1)?,
                    title: r.get(2)?,
                    chunk_id: r.get(3)?,
                    ordinal: r.get(4)?,
                    excerpt: bounded_text(&r.get::<_, String>(5)?, 400),
                    heading: r.get(6)?,
                    anchor: "chunk-0".into(),
                    content_hash: String::new(),
                    score: 0.1,
                })
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        drop(q);
        for row in rows {
            let mut item = row;
            if seen.insert(item.page_id.clone()) {
                item.content_hash = hash(&item.excerpt);
                out.push(item);
                if out.len() >= limit {
                    tx.commit().map_err(err)?;
                    return Ok(out);
                }
            }
        }
    }
    tx.commit().map_err(err)?;
    Ok(out)
}
