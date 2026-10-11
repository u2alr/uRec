//! Cheap structural check of an MP4 file: walks the top-level boxes, makes sure the file isn't
//! truncated and that it has the pieces a player needs (`ftyp`, `moov`, media data). Used before
//! a clip is reported as saved, and by the library to flag damaged files. Reads only box headers
//! plus the small `mvhd` box, so it's fast even for large files.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Mp4Info {
    /// Duration from the movie header, if it states one (fragmented files may report 0).
    pub duration_secs: Option<f64>,
    pub fragmented: bool,
    pub bytes: u64,
}

fn read_exact_at(f: &mut File, pos: u64, buf: &mut [u8]) -> std::io::Result<()> {
    f.seek(SeekFrom::Start(pos))?;
    f.read_exact(buf)
}

/// Returns (box type, header length, total box length) for the box at `pos`.
fn box_at(f: &mut File, pos: u64, end: u64) -> Result<([u8; 4], u64, u64), String> {
    if end - pos < 8 {
        return Err(format!("truncated box header at byte {pos}"));
    }
    let mut h = [0u8; 8];
    read_exact_at(f, pos, &mut h).map_err(|e| e.to_string())?;
    let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
    let kind = [h[4], h[5], h[6], h[7]];
    let (header, size) = match size32 {
        0 => (8, end - pos),
        1 => {
            let mut l = [0u8; 8];
            read_exact_at(f, pos + 8, &mut l).map_err(|e| e.to_string())?;
            (16, u64::from_be_bytes(l))
        }
        n => (8, n),
    };
    if size < header {
        return Err(format!("invalid box size at byte {pos}"));
    }
    if pos + size > end {
        return Err(format!(
            "file is truncated: box '{}' needs {} bytes, only {} left",
            String::from_utf8_lossy(&kind),
            size,
            end - pos
        ));
    }
    Ok((kind, header, size))
}

fn mvhd_duration(f: &mut File, start: u64, end: u64) -> Option<f64> {
    let mut pos = start;
    while pos < end {
        let (kind, header, size) = box_at(f, pos, end).ok()?;
        if &kind == b"mvhd" {
            let mut b = [0u8; 32];
            let n = ((size - header) as usize).min(32);
            read_exact_at(f, pos + header, &mut b[..n]).ok()?;
            let (scale, dur) = if b[0] == 1 {
                (
                    u32::from_be_bytes([b[20], b[21], b[22], b[23]]) as f64,
                    u64::from_be_bytes([b[24], b[25], b[26], b[27], b[28], b[29], b[30], b[31]]) as f64,
                )
            } else {
                (
                    u32::from_be_bytes([b[12], b[13], b[14], b[15]]) as f64,
                    u32::from_be_bytes([b[16], b[17], b[18], b[19]]) as f64,
                )
            };
            return (scale > 0.0).then(|| dur / scale);
        }
        pos += size;
    }
    None
}

/// Validate the file at `path`. Errors describe what's wrong in plain words.
pub fn check(path: &Path) -> Result<Mp4Info, String> {
    let mut f = File::open(path).map_err(|e| format!("can't open the file: {e}"))?;
    let end = f.metadata().map_err(|e| e.to_string())?.len();
    if end < 32 {
        return Err("the file is empty or almost empty".into());
    }
    let (mut ftyp, mut moov, mut mdat, mut moof) = (false, false, false, false);
    let mut duration = None;
    let mut pos = 0u64;
    let mut first = true;
    while pos < end {
        let (kind, header, size) = box_at(&mut f, pos, end)?;
        if first && &kind != b"ftyp" {
            return Err("not an MP4 file (no 'ftyp' at the start)".into());
        }
        first = false;
        match &kind {
            b"ftyp" => ftyp = true,
            b"moov" => {
                moov = true;
                duration = mvhd_duration(&mut f, pos + header, pos + size);
            }
            b"mdat" => mdat = mdat || size > header,
            b"moof" => moof = true,
            _ => {}
        }
        pos += size;
    }
    if !ftyp {
        return Err("not an MP4 file".into());
    }
    if !moov {
        return Err("the file wasn't finished (no 'moov' index)".into());
    }
    if !mdat {
        return Err("the file contains no media data".into());
    }
    Ok(Mp4Info { duration_secs: duration, fragmented: moof, bytes: end })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn mvhd(scale: u32, dur: u32) -> Vec<u8> {
        let mut b = vec![0u8; 100];
        b[12..16].copy_from_slice(&scale.to_be_bytes());
        b[16..20].copy_from_slice(&dur.to_be_bytes());
        bx(b"mvhd", &b)
    }

    fn write(name: &str, data: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("urec-mp4check-{}-{name}", std::process::id()));
        std::fs::write(&p, data).unwrap();
        p
    }

    #[test]
    fn valid_file_and_duration() {
        let mut d = bx(b"ftyp", b"isom\0\0\x02\0isomiso2");
        d.extend(bx(b"moov", &mvhd(1000, 8000)));
        d.extend(bx(b"mdat", &[1u8; 64]));
        let p = write("ok.mp4", &d);
        let i = check(&p).unwrap();
        assert_eq!(i.duration_secs, Some(8.0));
        assert!(!i.fragmented);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn rejects_truncated_unfinished_and_garbage() {
        let mut d = bx(b"ftyp", b"isom\0\0\x02\0isomiso2");
        d.extend(bx(b"moov", &mvhd(1000, 8000)));
        d.extend(bx(b"mdat", &[1u8; 64]));
        let p = write("trunc.mp4", &d[..d.len() - 10]);
        assert!(check(&p).unwrap_err().contains("truncated"));
        let mut no_moov = bx(b"ftyp", b"isom\0\0\x02\0isomiso2");
        no_moov.extend(bx(b"mdat", &[1u8; 64]));
        let p2 = write("nomoov.mp4", &no_moov);
        assert!(check(&p2).unwrap_err().contains("moov"));
        let p3 = write("garbage.mp4", &[7u8; 100]);
        assert!(check(&p3).is_err());
        let p4 = write("empty.mp4", b"");
        assert!(check(&p4).is_err());
        for p in [p, p2, p3, p4] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn fragmented_layout() {
        let mut d = bx(b"ftyp", b"isom\0\0\x02\0isomiso2");
        d.extend(bx(b"moov", &mvhd(1000, 0)));
        d.extend(bx(b"moof", &[0u8; 16]));
        d.extend(bx(b"mdat", &[1u8; 16]));
        let p = write("frag.mp4", &d);
        let i = check(&p).unwrap();
        assert!(i.fragmented);
        let _ = std::fs::remove_file(p);
    }
}
