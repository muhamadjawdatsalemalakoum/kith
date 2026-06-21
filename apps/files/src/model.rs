//! Kith file-sharing schema: a list of *offers* on the engine's replicated document
//! (ROOT `"files"`). An offer advertises a file by content `hash` + size + the device
//! that has it, so any of your linked devices can pull it directly over the blob
//! primitive. The bytes never live in this document — only the small offer does.
//! Coexists with the memory (`"memory"`) and tabs (`"spaces"`) schemas in one doc.

use mesh_engine::automerge::transaction::Transactable;
use mesh_engine::automerge::{ObjId, ObjType, ReadDoc, ScalarValue, Value, ROOT};
use mesh_engine::{Result, SharedDoc};

/// One file offered to the circle.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub hash: String,
    /// Endpoint id of the device that has the bytes (where to fetch from).
    pub from_id: String,
    /// Friendly name of that device.
    pub from_name: String,
}

/// Advertise a file offer. Creates the list on first use.
pub async fn add_file(doc: &SharedDoc, e: &FileEntry) -> Result<()> {
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    let list = match tx.get(ROOT, "files")? {
        Some((_, l)) => l,
        None => tx.put_object(ROOT, "files", ObjType::List)?,
    };
    let idx = tx.length(&list);
    let m = tx.insert_object(&list, idx, ObjType::Map)?;
    tx.put(&m, "id", e.id.as_str())?;
    tx.put(&m, "name", e.name.as_str())?;
    tx.put(&m, "size", e.size as i64)?;
    tx.put(&m, "hash", e.hash.as_str())?;
    tx.put(&m, "from_id", e.from_id.as_str())?;
    tx.put(&m, "from_name", e.from_name.as_str())?;
    tx.commit();
    Ok(())
}

/// All current (non-withdrawn) offers.
pub async fn all_files(doc: &SharedDoc) -> Vec<FileEntry> {
    let guard = doc.lock().await;
    let mut out = Vec::new();
    let Some((_, list)) = guard.get(ROOT, "files").ok().flatten() else {
        return out;
    };
    let strf = |obj: &ObjId, key: &str| -> String {
        match guard.get(obj, key) {
            Ok(Some((Value::Scalar(s), _))) => match s.as_ref() {
                ScalarValue::Str(t) => t.to_string(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    };
    let intf = |obj: &ObjId, key: &str| -> u64 {
        match guard.get(obj, key) {
            Ok(Some((Value::Scalar(s), _))) => match s.as_ref() {
                ScalarValue::Int(n) => (*n).max(0) as u64,
                ScalarValue::Uint(n) => *n,
                _ => 0,
            },
            _ => 0,
        }
    };
    for i in 0..guard.length(&list) {
        let Ok(Some((_, m))) = guard.get(&list, i) else {
            continue;
        };
        let deleted = matches!(
            guard.get(&m, "deleted"),
            Ok(Some((Value::Scalar(s), _))) if matches!(s.as_ref(), ScalarValue::Boolean(true))
        );
        if deleted {
            continue;
        }
        out.push(FileEntry {
            id: strf(&m, "id"),
            name: strf(&m, "name"),
            size: intf(&m, "size"),
            hash: strf(&m, "hash"),
            from_id: strf(&m, "from_id"),
            from_name: strf(&m, "from_name"),
        });
    }
    out
}

/// Withdraw an offer by id (tombstone — a conflict-free delete). Returns whether found.
pub async fn remove_file(doc: &SharedDoc, id: &str) -> Result<bool> {
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    let Some((_, list)) = tx.get(ROOT, "files")? else {
        return Ok(false);
    };
    for i in 0..tx.length(&list) {
        let Some((_, m)) = tx.get(&list, i)? else {
            continue;
        };
        let is_match = matches!(
            tx.get(&m, "id")?,
            Some((Value::Scalar(s), _)) if matches!(s.as_ref(), ScalarValue::Str(t) if t.as_str() == id)
        );
        if is_match {
            tx.put(&m, "deleted", true)?;
            tx.commit();
            return Ok(true);
        }
    }
    Ok(false)
}

/// Rename an offer's display name by id. Returns whether it was found.
pub async fn rename_file(doc: &SharedDoc, id: &str, name: &str) -> Result<bool> {
    let mut guard = doc.lock().await;
    let mut tx = guard.transaction();
    let Some((_, list)) = tx.get(ROOT, "files")? else {
        return Ok(false);
    };
    for i in 0..tx.length(&list) {
        let Some((_, m)) = tx.get(&list, i)? else {
            continue;
        };
        let is_match = matches!(
            tx.get(&m, "id")?,
            Some((Value::Scalar(s), _)) if matches!(s.as_ref(), ScalarValue::Str(t) if t.as_str() == id)
        );
        if is_match {
            tx.put(&m, "name", name)?;
            tx.commit();
            return Ok(true);
        }
    }
    Ok(false)
}
