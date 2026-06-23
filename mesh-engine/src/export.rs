//! Encrypted, passphrase-protected Space export/import — the no-account recovery path.
//!
//! A whole Space (its replica, blobs, membership log, epoch keys, and group/at-rest keys)
//! is archived, the passphrase is stretched with **Argon2id**, and the archive is sealed
//! with XChaCha20-Poly1305 ([`crate::atrest`]). The result restores byte-identically on a
//! fresh device with the passphrase. Because there is no account, this is the ONLY recovery
//! path if every device is lost — losing all devices *without* an export means the data is
//! gone (stated plainly in the app).

use std::path::Path;

use argon2::Argon2;

use crate::error::{CoreError, Result};
use crate::space::SpaceId;
use crate::{atrest, keys};

/// Bundle magic + version.
const MAGIC: &[u8; 8] = b"KITHSPC1";
const SALT_LEN: usize = 16;

/// Argon2id: stretch a passphrase to a 32-byte key under `salt`.
fn derive(passphrase: &[u8], salt: &[u8]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase, salt, &mut out)
        .map_err(|e| CoreError::Other(anyhow::anyhow!("argon2 kdf: {e}")))?;
    Ok(out)
}

/// Seal a Space's `files` (relative path + bytes) and `blobs` (raw content) into an
/// encrypted, passphrase-protected bundle:
/// `MAGIC || salt || nonce || AEAD( space_id || files || blobs )`.
pub fn seal(
    id: &SpaceId,
    files: &[(String, Vec<u8>)],
    blobs: &[Vec<u8>],
    passphrase: &str,
) -> Result<Vec<u8>> {
    let mut plain = Vec::new();
    plain.extend_from_slice(id.as_bytes());
    plain.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (name, data) in files {
        let nb = name.as_bytes();
        plain.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        plain.extend_from_slice(nb);
        plain.extend_from_slice(&(data.len() as u64).to_le_bytes());
        plain.extend_from_slice(data);
    }
    plain.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
    for data in blobs {
        plain.extend_from_slice(&(data.len() as u64).to_le_bytes());
        plain.extend_from_slice(data);
    }
    let salt_full = keys::generate();
    let salt = &salt_full[..SALT_LEN];
    let key = derive(passphrase.as_bytes(), salt)?;
    let sealed = atrest::encrypt(&key, &plain);
    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + sealed.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(salt);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// One opened bundle: the Space id, its files, and its blob contents.
pub struct Opened {
    pub id: SpaceId,
    pub files: Vec<(String, Vec<u8>)>,
    pub blobs: Vec<Vec<u8>>,
}

/// Open a bundle. `Err` on a wrong passphrase (AEAD tag mismatch), a corrupt/truncated
/// bundle, or a bad magic.
pub fn open(bundle: &[u8], passphrase: &str) -> Result<Opened> {
    if bundle.len() < MAGIC.len() + SALT_LEN || &bundle[..MAGIC.len()] != MAGIC {
        return Err(CoreError::Other(anyhow::anyhow!("not a Kith space export")));
    }
    let salt = &bundle[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let sealed = &bundle[MAGIC.len() + SALT_LEN..];
    let key = derive(passphrase.as_bytes(), salt)?;
    let plain = atrest::decrypt(&key, sealed)
        .ok_or_else(|| CoreError::Other(anyhow::anyhow!("wrong passphrase or corrupt export")))?;

    let mut c = Cursor::new(&plain);
    let id_bytes = c.take(32).ok_or_else(bad)?;
    let mut id = [0u8; 32];
    id.copy_from_slice(id_bytes);
    let nfiles = c.u32().ok_or_else(bad)?;
    let mut files = Vec::with_capacity(nfiles as usize);
    for _ in 0..nfiles {
        let nlen = c.u16().ok_or_else(bad)? as usize;
        let name = std::str::from_utf8(c.take(nlen).ok_or_else(bad)?)
            .map_err(|_| bad())?
            .to_string();
        let dlen = c.u64().ok_or_else(bad)? as usize;
        let data = c.take(dlen).ok_or_else(bad)?.to_vec();
        files.push((name, data));
    }
    let nblobs = c.u32().ok_or_else(bad)?;
    let mut blobs = Vec::with_capacity(nblobs as usize);
    for _ in 0..nblobs {
        let dlen = c.u64().ok_or_else(bad)? as usize;
        blobs.push(c.take(dlen).ok_or_else(bad)?.to_vec());
    }
    Ok(Opened {
        id: SpaceId::from_bytes(id),
        files,
        blobs,
    })
}

/// Read every file under `dir` (recursively) as `(relative forward-slash path, bytes)`,
/// skipping transient `*.tmp` write files. Used to gather a Space dir for [`seal`].
pub fn collect_dir(dir: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    collect(dir, dir, &mut out)?;
    Ok(out)
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let fname = entry.file_name().to_string_lossy().into_owned();
        if fname.contains(".tmp") {
            continue;
        }
        // The blob store dir is exported separately as portable content (its on-disk files
        // are locked while open and not portable), so skip it here.
        if path.is_dir() && fname == "blobs" {
            continue;
        }
        if path.is_dir() {
            collect(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, std::fs::read(&path)?));
        }
    }
    Ok(())
}

/// Write `files` into `dest_dir` (created), rejecting any path that would escape it.
pub fn extract_to(dest_dir: &Path, files: &[(String, Vec<u8>)]) -> Result<()> {
    std::fs::create_dir_all(dest_dir)?;
    for (name, data) in files {
        if name.is_empty()
            || name.contains("..")
            || name.starts_with('/')
            || name.starts_with('\\')
            || name.contains(':')
        {
            return Err(CoreError::Other(anyhow::anyhow!(
                "refusing unsafe path in export: {name}"
            )));
        }
        let rel: std::path::PathBuf = name.split('/').collect();
        let target = dest_dir.join(rel);
        if !target.starts_with(dest_dir) {
            return Err(CoreError::Other(anyhow::anyhow!("path escapes export dir")));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, data)?;
    }
    Ok(())
}

/// Insert or replace a named file in a gathered file list.
pub fn upsert(files: &mut Vec<(String, Vec<u8>)>, name: &str, data: Vec<u8>) {
    if let Some(slot) = files.iter_mut().find(|(n, _)| n == name) {
        slot.1 = data;
    } else {
        files.push((name.to_string(), data));
    }
}

fn bad() -> CoreError {
    CoreError::Other(anyhow::anyhow!("corrupt Kith space export"))
}

/// Minimal forward-only byte cursor for decoding a bundle.
struct Cursor<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Cursor<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}
