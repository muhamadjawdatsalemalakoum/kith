//! centralTabs' document schema (spaces -> groups -> tabs), defined ON the engine's
//! replicated Automerge document. The engine stays schema-agnostic; this is the one
//! place that knows what a "tab" is. Each device writes under its own actor (the
//! engine manages that), so concurrent edits across devices merge cleanly.
//!
//! Note we reach for `automerge` THROUGH the engine's re-export, so there is exactly
//! one CRDT version across the whole family — never a direct `automerge` dependency.

use mesh_engine::automerge::transaction::Transactable;
use mesh_engine::automerge::{ObjType, ReadDoc, ScalarValue, Value, ROOT};
use mesh_engine::{Result, SharedDoc};

/// One saved tab.
#[derive(Debug, Clone)]
pub struct Tab {
    pub id: String,
    pub url: String,
    pub title: String,
}

/// Seed an example space/group/tab so the schema is exercised end to end.
pub async fn seed_example(doc: &SharedDoc) -> Result<()> {
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    let spaces = tx.put_object(ROOT, "spaces", ObjType::Map)?;
    let space = tx.put_object(&spaces, "space-1", ObjType::Map)?;
    tx.put(&space, "name", "Work")?;
    let groups = tx.put_object(&space, "groups", ObjType::List)?;
    let group = tx.insert_object(&groups, 0, ObjType::Map)?;
    tx.put(&group, "title", "Research")?;
    let tabs = tx.put_object(&group, "tabs", ObjType::List)?;
    let tab = tx.insert_object(&tabs, 0, ObjType::Map)?;
    tx.put(&tab, "url", "https://example.com")?;
    tx.put(&tab, "title", "Example")?;
    tx.commit();
    Ok(())
}

/// Add a tab to the default space/group, creating the path if it doesn't exist yet.
/// Returns the new tab's id.
pub async fn add_tab(doc: &SharedDoc, url: &str, title: &str) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    let spaces = match tx.get(ROOT, "spaces")? {
        Some((_, id)) => id,
        None => tx.put_object(ROOT, "spaces", ObjType::Map)?,
    };
    let space = match tx.get(&spaces, "space-1")? {
        Some((_, id)) => id,
        None => {
            let s = tx.put_object(&spaces, "space-1", ObjType::Map)?;
            tx.put(&s, "name", "Default")?;
            s
        }
    };
    let groups = match tx.get(&space, "groups")? {
        Some((_, id)) => id,
        None => tx.put_object(&space, "groups", ObjType::List)?,
    };
    let group = match tx.get(&groups, 0)? {
        Some((_, id)) => id,
        None => {
            let g = tx.insert_object(&groups, 0, ObjType::Map)?;
            tx.put(&g, "title", "Imported")?;
            g
        }
    };
    let tabs = match tx.get(&group, "tabs")? {
        Some((_, id)) => id,
        None => tx.put_object(&group, "tabs", ObjType::List)?,
    };
    let idx = tx.length(&tabs);
    let tab = tx.insert_object(&tabs, idx, ObjType::Map)?;
    tx.put(&tab, "id", id.as_str())?;
    tx.put(&tab, "url", url)?;
    tx.put(&tab, "title", title)?;
    tx.commit();
    Ok(id)
}

/// All non-forgotten tabs across every group in the default space, oldest first.
pub async fn all_tabs(doc: &SharedDoc) -> Vec<Tab> {
    let guard = doc.lock().await;
    let mut out = Vec::new();
    let Some((_, spaces)) = guard.get(ROOT, "spaces").ok().flatten() else {
        return out;
    };
    let Some((_, space)) = guard.get(&spaces, "space-1").ok().flatten() else {
        return out;
    };
    let Some((_, groups)) = guard.get(&space, "groups").ok().flatten() else {
        return out;
    };
    let field = |obj: &mesh_engine::automerge::ObjId, key: &str| -> String {
        match guard.get(obj, key) {
            Ok(Some((Value::Scalar(s), _))) => match s.as_ref() {
                ScalarValue::Str(t) => t.to_string(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    };
    for g in 0..guard.length(&groups) {
        let Ok(Some((_, group))) = guard.get(&groups, g) else {
            continue;
        };
        let Ok(Some((_, tabs))) = guard.get(&group, "tabs") else {
            continue;
        };
        for t in 0..guard.length(&tabs) {
            let Ok(Some((_, tab))) = guard.get(&tabs, t) else {
                continue;
            };
            let deleted = matches!(
                guard.get(&tab, "deleted"),
                Ok(Some((Value::Scalar(s), _))) if matches!(s.as_ref(), ScalarValue::Boolean(true))
            );
            if deleted {
                continue;
            }
            out.push(Tab {
                id: field(&tab, "id"),
                url: field(&tab, "url"),
                title: field(&tab, "title"),
            });
        }
    }
    out
}

/// Forget a tab by id (tombstone — a conflict-free delete). Returns whether found.
pub async fn remove_tab(doc: &SharedDoc, id: &str) -> Result<bool> {
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    let Some((_, spaces)) = tx.get(ROOT, "spaces")? else {
        return Ok(false);
    };
    let Some((_, space)) = tx.get(&spaces, "space-1")? else {
        return Ok(false);
    };
    let Some((_, groups)) = tx.get(&space, "groups")? else {
        return Ok(false);
    };
    for g in 0..tx.length(&groups) {
        let Some((_, group)) = tx.get(&groups, g)? else {
            continue;
        };
        let Some((_, tabs)) = tx.get(&group, "tabs")? else {
            continue;
        };
        for t in 0..tx.length(&tabs) {
            let Some((_, tab)) = tx.get(&tabs, t)? else {
                continue;
            };
            let is_match = matches!(
                tx.get(&tab, "id")?,
                Some((Value::Scalar(s), _)) if matches!(s.as_ref(), ScalarValue::Str(t) if t.as_str() == id)
            );
            if is_match {
                tx.put(&tab, "deleted", true)?;
                tx.commit();
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// How many tabs are saved across ALL groups in the default space.
///
/// Iterates every group (not just `groups[0]`): two devices that each create a group
/// concurrently merge into a list with multiple front groups, so counting only `[0]`
/// would silently undercount after a cross-device merge.
pub async fn count_tabs(doc: &SharedDoc) -> u64 {
    let guard = doc.lock().await;
    let Some((_, spaces)) = guard.get(ROOT, "spaces").ok().flatten() else {
        return 0;
    };
    let Some((_, space)) = guard.get(&spaces, "space-1").ok().flatten() else {
        return 0;
    };
    let Some((_, groups)) = guard.get(&space, "groups").ok().flatten() else {
        return 0;
    };
    let mut total = 0u64;
    for g in 0..guard.length(&groups) {
        if let Ok(Some((_, group))) = guard.get(&groups, g) {
            if let Ok(Some((_, tabs))) = guard.get(&group, "tabs") {
                for t in 0..guard.length(&tabs) {
                    let Ok(Some((_, tab))) = guard.get(&tabs, t) else {
                        continue;
                    };
                    let deleted = matches!(
                        guard.get(&tab, "deleted"),
                        Ok(Some((Value::Scalar(s), _))) if matches!(s.as_ref(), ScalarValue::Boolean(true))
                    );
                    if !deleted {
                        total += 1;
                    }
                }
            }
        }
    }
    total
}

/// The url of the first tab found across the space's groups (demonstrates the
/// descent + convergence; scans all groups, not just `[0]`).
pub async fn first_tab_url(doc: &SharedDoc) -> Option<String> {
    let guard = doc.lock().await;
    let (_, spaces) = guard.get(ROOT, "spaces").ok()??;
    let (_, space) = guard.get(&spaces, "space-1").ok()??;
    let (_, groups) = guard.get(&space, "groups").ok()??;
    for g in 0..guard.length(&groups) {
        let Ok(Some((_, group))) = guard.get(&groups, g) else {
            continue;
        };
        let Ok(Some((_, tabs))) = guard.get(&group, "tabs") else {
            continue;
        };
        if guard.length(&tabs) == 0 {
            continue;
        }
        let Ok(Some((_, tab))) = guard.get(&tabs, 0) else {
            continue;
        };
        // A string scalar's Display wraps the value in quotes; pull the raw text out.
        if let Ok(Some((Value::Scalar(s), _))) = guard.get(&tab, "url") {
            if let ScalarValue::Str(text) = s.as_ref() {
                return Some(text.to_string());
            }
        }
    }
    None
}
