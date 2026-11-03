//! Process-private Segoe UI faces for the widget's font override.
//!
//! GDI applies Windows FontSubstitutes even when a theme explicitly requests
//! "Segoe UI". Load the installed files under a private family name so opting
//! out of the Windows font actually bypasses that substitution. The original
//! files and system settings are never changed or redistributed.

use std::path::PathBuf;
use std::sync::OnceLock;

use windows::Win32::Graphics::Gdi::AddFontMemResourceEx;

const PRIVATE_FAMILY: &str = "CCUMSans";

pub(crate) fn segoe_ui_family(weight: i32) -> &'static str {
    static FACES: OnceLock<Option<(bool, bool)>> = OnceLock::new();
    let faces = FACES.get_or_init(|| {
        let directory = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join("Fonts");
        if !register_face(&directory.join("segoeui.ttf")) {
            crate::diagnose::log("unable to load private Segoe UI; using the Segoe UI font name");
            return None;
        }
        // GDI chooses the appropriate weight from the privately loaded family.
        // Missing optional faces can be synthesized from the regular face.
        register_face(&directory.join("segoeuib.ttf"));
        let semibold = register_face(&directory.join("seguisb.ttf"));
        let light = register_face(&directory.join("segoeuil.ttf"));
        Some((semibold, light))
    });
    match faces {
        None => "Segoe UI",
        Some((_, true)) if weight <= 300 => "CCUMSans Light",
        Some((true, _)) if (600..700).contains(&weight) => "CCUMSans Semibold",
        Some(_) => PRIVATE_FAMILY,
    }
}

fn register_face(path: &std::path::Path) -> bool {
    let Ok(mut bytes) = std::fs::read(path) else {
        return false;
    };
    if rename_family(&mut bytes).is_none() {
        return false;
    }
    let Ok(size) = u32::try_from(bytes.len()) else {
        return false;
    };
    let mut count = 0;
    // Windows copies the buffer. These non-enumerable resources remain private
    // to this process and Windows releases them when the process exits.
    unsafe {
        // The generated binding marks this output pointer const. Keep a raw
        // mutable pointer so Windows can write the count without aliasing an
        // immutable Rust reference.
        let handle = AddFontMemResourceEx(bytes.as_ptr().cast(), size, None, &raw mut count);
        !handle.is_invalid() && count > 0
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

// Return the directory record and its validated table range.
fn table(bytes: &[u8], tag: &[u8; 4]) -> Option<(usize, std::ops::Range<usize>)> {
    if u32_at(bytes, 0)? != 0x0001_0000 {
        return None;
    }
    for index in 0..usize::from(u16_at(bytes, 4)?) {
        let record = 12 + index * 16;
        if bytes.get(record..record + 4)? == tag {
            let start = u32_at(bytes, record + 8)? as usize;
            let end = start.checked_add(u32_at(bytes, record + 12)? as usize)?;
            bytes.get(start..end)?;
            return Some((record, start..end));
        }
    }
    None
}

fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks(4).fold(0_u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// Replace only naming records, using equal-length aliases so every table and
/// glyph offset stays intact. Keep copyright, licence and trademark records.
fn rename_family(bytes: &mut [u8]) -> Option<()> {
    let (record, names) = table(bytes, b"name")?;
    let (_, head) = table(bytes, b"head")?;
    if names.len() < 6 || head.len() < 12 {
        return None;
    }
    let count = usize::from(u16_at(bytes, names.start + 2)?);
    let storage = usize::from(u16_at(bytes, names.start + 4)?);
    if 6 + count * 12 > storage || storage > names.len() {
        return None;
    }
    // Records may share their strings. Collect ranges first and edit each byte
    // only once, including overlapping strings, below the record directory.
    let mut ranges = Vec::new();
    for index in 0..count {
        let entry = names.start + 6 + index * 12;
        if !matches!(u16_at(bytes, entry + 6)?, 1 | 3 | 4 | 6 | 16 | 18 | 21 | 25) {
            continue;
        }
        let length = usize::from(u16_at(bytes, entry + 8)?);
        let start = storage + usize::from(u16_at(bytes, entry + 10)?);
        let end = start.checked_add(length)?;
        if end > names.len() {
            return None;
        }
        ranges.push(names.start + start..names.start + end);
    }
    let mut changed = false;
    for range in ranges {
        let text = &mut bytes[range];
        for (original, replacement) in [("Segoe UI", PRIVATE_FAMILY), ("SegoeUI", "CCUMSns")] {
            for unicode in [false, true] {
                let encode = |value: &str| -> Vec<u8> {
                    if unicode {
                        value.encode_utf16().flat_map(u16::to_be_bytes).collect()
                    } else {
                        value.as_bytes().to_vec()
                    }
                };
                let original = encode(original);
                let replacement = encode(replacement);
                let mut offset = 0;
                while offset + original.len() <= text.len() {
                    if text[offset..offset + original.len()] == original {
                        text[offset..offset + original.len()].copy_from_slice(&replacement);
                        changed = true;
                        offset += original.len();
                    } else {
                        offset += 1;
                    }
                }
            }
        }
    }
    if !changed {
        return None;
    }
    let names_checksum = checksum(&bytes[names]);
    bytes[record + 4..record + 8].copy_from_slice(&names_checksum.to_be_bytes());
    bytes[head.start + 8..head.start + 12].fill(0);
    let adjustment = 0xB1B0_AFBA_u32.wrapping_sub(checksum(bytes));
    bytes[head.start + 8..head.start + 12].copy_from_slice(&adjustment.to_be_bytes());
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::*;

    #[test]
    fn private_segoe_ui_is_selected_and_keeps_original_glyphs() {
        let directory = PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("Fonts");
        for (weight, file) in [
            (300, "segoeuil.ttf"),
            (400, "segoeui.ttf"),
            (500, "segoeui.ttf"),
            (600, "seguisb.ttf"),
            (700, "segoeuib.ttf"),
        ] {
            let family = segoe_ui_family(weight);
            assert!(family.starts_with(PRIVATE_FAMILY));
            let name: Vec<u16> = family.encode_utf16().chain(Some(0)).collect();
            let mut bytes = std::fs::read(directory.join(file)).unwrap();
            let (_, glyphs) = table(&bytes, b"glyf").unwrap();
            let original_glyphs = bytes[glyphs.clone()].to_vec();
            rename_family(&mut bytes).unwrap();
            assert_eq!(checksum(&bytes), 0xB1B0_AFBA);
            assert_eq!(bytes[glyphs], original_glyphs);
            unsafe {
                let dc = CreateCompatibleDC(None);
                assert!(!dc.is_invalid());
                let font = CreateFontW(
                    -18,
                    0,
                    0,
                    0,
                    weight,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_TT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    0,
                    PCWSTR::from_raw(name.as_ptr()),
                );
                assert!(!font.is_invalid());
                let previous = SelectObject(dc, font.into());
                let mut face = [0_u16; 64];
                let length = GetTextFaceW(dc, Some(&mut face));
                assert!(length > 1);
                assert!(String::from_utf16_lossy(&face[..length as usize - 1])
                    .starts_with(PRIVATE_FAMILY));
                // GDI must select the original Segoe outlines, not an aliased
                // family that silently falls back to another installed font.
                let tag = u32::from_le_bytes(*b"glyf");
                let size = GetFontData(dc, tag, 0, None, 0);
                assert_eq!(size as usize, original_glyphs.len());
                let mut actual = vec![0; size as usize];
                assert_eq!(
                    GetFontData(dc, tag, 0, Some(actual.as_mut_ptr().cast()), size),
                    size
                );
                assert_eq!(actual, original_glyphs, "weight {weight}");
                SelectObject(dc, previous);
                let _ = DeleteObject(font.into());
                let _ = DeleteDC(dc);
            }
        }
    }

    #[test]
    fn malformed_fonts_are_rejected_without_panicking() {
        for mut bytes in [vec![], vec![0; 12], vec![0xFF; 128]] {
            assert!(rename_family(&mut bytes).is_none());
        }
    }
}
