//! agent-memory's document schema: a list of memory entries on the engine's
//! replicated Automerge doc. Each device appends under its own actor, so concurrent
//! writes across devices merge with zero conflict. `forget` tombstones by id (a
//! conflict-safe delete). The engine stays schema-agnostic; this is the only place
//! that knows what a "memory" is.

use mesh_engine::automerge::transaction::Transactable;
use mesh_engine::automerge::{ObjType, ReadDoc, ScalarValue, Value, ROOT};
use mesh_engine::{Result, SharedDoc};

/// One memory entry.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub text: String,
    pub kind: String,
}

/// Append a memory. Returns the new entry's id. Creates the list on first use.
pub async fn append(doc: &SharedDoc, text: &str, kind: &str) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    let list = match tx.get(ROOT, "memory")? {
        Some((_, l)) => l,
        None => tx.put_object(ROOT, "memory", ObjType::List)?,
    };
    let idx = tx.length(&list);
    let entry = tx.insert_object(&list, idx, ObjType::Map)?;
    tx.put(&entry, "id", id.as_str())?;
    tx.put(&entry, "text", text)?;
    tx.put(&entry, "kind", if kind.is_empty() { "fact" } else { kind })?;
    tx.commit();
    Ok(id)
}

/// All non-forgotten entries, oldest first.
pub async fn all(doc: &SharedDoc) -> Vec<Entry> {
    let guard = doc.lock().await;
    let mut out = Vec::new();
    let Some((_, list)) = guard.get(ROOT, "memory").ok().flatten() else {
        return out;
    };
    let n = guard.length(&list);
    for i in 0..n {
        let Ok(Some((_, e))) = guard.get(&list, i) else {
            continue;
        };
        // Skip tombstoned entries.
        let deleted = matches!(
            guard.get(&e, "deleted"),
            Ok(Some((Value::Scalar(s), _))) if matches!(s.as_ref(), ScalarValue::Boolean(true))
        );
        if deleted {
            continue;
        }
        let field = |key: &str| -> String {
            match guard.get(&e, key) {
                Ok(Some((Value::Scalar(s), _))) => match s.as_ref() {
                    ScalarValue::Str(t) => t.to_string(),
                    _ => String::new(),
                },
                _ => String::new(),
            }
        };
        out.push(Entry {
            id: field("id"),
            text: field("text"),
            kind: field("kind"),
        });
    }
    out
}

/// Case-insensitive substring search over entry text (local, over your own data).
/// An empty/whitespace query returns NOTHING (not everything) — guarding the
/// surprising privacy hazard of `search("")` dumping the entire memory.
pub async fn search(doc: &SharedDoc, query: &str) -> Vec<Entry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    all(doc)
        .await
        .into_iter()
        .filter(|e| e.text.to_lowercase().contains(&q))
        .collect()
}

/// Forget an entry by id (tombstone — propagates as a conflict-free delete).
/// Returns whether a matching entry was found.
pub async fn forget(doc: &SharedDoc, id: &str) -> Result<bool> {
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    let list = match tx.get(ROOT, "memory")? {
        Some((_, l)) => l,
        None => return Ok(false),
    };
    let n = tx.length(&list);
    for i in 0..n {
        let Some((_, e)) = tx.get(&list, i)? else {
            continue;
        };
        let is_match = matches!(
            tx.get(&e, "id")?,
            Some((Value::Scalar(s), _)) if matches!(s.as_ref(), ScalarValue::Str(t) if t.as_str() == id)
        );
        if is_match {
            tx.put(&e, "deleted", true)?;
            tx.commit();
            return Ok(true);
        }
    }
    Ok(false)
}
