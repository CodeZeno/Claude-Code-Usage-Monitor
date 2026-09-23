use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use sha2::{Digest, Sha256};

// A hard ceiling independent of the server's Content-Length or release metadata.
pub(super) const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct AssetIntegrity {
    size: u64,
    sha256: [u8; 32],
}

impl AssetIntegrity {
    pub(super) fn new(size: u64, digest: Option<&str>) -> Result<Self, String> {
        if size == 0 || size > MAX_DOWNLOAD_BYTES {
            return Err(format!(
                "Update size must be between 1 and {MAX_DOWNLOAD_BYTES} bytes."
            ));
        }
        let hex = digest
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| "The update is missing a valid GitHub SHA-256 digest.".to_string())?;
        let mut sha256 = [0; 32];
        for (byte, pair) in sha256.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            // Validated as ASCII hex above.
            *byte = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
        }
        Ok(Self { size, sha256 })
    }

    pub(super) fn size_arg(&self) -> String {
        self.size.to_string()
    }

    pub(super) fn digest_arg(&self) -> String {
        let hex: String = self
            .sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("sha256:{hex}")
    }
}

pub(super) fn download_release_asset(
    release: &super::ReleaseDescriptor,
    partial_path: &Path,
    final_path: &Path,
) -> Result<(), String> {
    let response = super::build_agent()?
        .get(&release.asset_url)
        .header("User-Agent", super::user_agent())
        .call()
        .map_err(|e| format!("Unable to download the latest release: {e}"))?;

    stage_verified_download(
        response.into_body().into_reader(),
        &release.integrity,
        partial_path,
        final_path,
    )
}

fn stage_verified_download(
    reader: impl Read,
    integrity: &AssetIntegrity,
    partial_path: &Path,
    final_path: &Path,
) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(partial_path)
        .map_err(|e| format!("Unable to create temporary download file: {e}"))?;
    let result = (|| {
        copy_verified(reader, &mut file, integrity)?;
        file.sync_all()
            .map_err(|e| format!("Unable to finalize the downloaded update: {e}"))?;
        drop(file);
        std::fs::rename(partial_path, final_path)
            .map_err(|e| format!("Unable to finalize the downloaded update file: {e}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(partial_path);
    }
    result
}

fn copy_verified(
    mut reader: impl Read,
    mut writer: impl Write,
    integrity: &AssetIntegrity,
) -> Result<(), String> {
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        // Read at most one extra byte to detect an oversized (even endless) stream.
        let limit = (integrity.size - total + 1).min(buffer.len() as u64) as usize;
        let count = match reader.read(&mut buffer[..limit]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result.map_err(|e| format!("Unable to read the update: {e}"))?,
        };
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > integrity.size || total > MAX_DOWNLOAD_BYTES {
            return Err("The update exceeds its expected size or the download limit.".into());
        }
        hash.update(&buffer[..count]);
        writer
            .write_all(&buffer[..count])
            .map_err(|e| format!("Unable to write the downloaded update: {e}"))?;
    }
    if total != integrity.size {
        return Err("The update size does not match the release metadata.".into());
    }
    let actual: [u8; 32] = hash.finalize().into();
    if actual != integrity.sha256 {
        return Err("The update SHA-256 does not match the release metadata.".into());
    }
    Ok(())
}

pub(super) fn open_verified_source(
    source: &Path,
    integrity: &AssetIntegrity,
) -> Result<File, String> {
    // Allow reads, but deny writes and deletion until replacement has completed.
    // This also rejects a source that another process already has open for writing.
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ
        .open(source)
        .map_err(|e| format!("Unable to open the downloaded update: {e}"))?;
    copy_verified(&mut file, io::sink(), integrity)?;
    file.rewind()
        .map_err(|e| format!("Unable to rewind the update: {e}"))?;
    Ok(file)
}

#[cfg(test)]
mod tests;
