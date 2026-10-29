//! Decoder abstraction so HTJ2K backends can be swapped, plus the `j2k` (pure Rust, CPU) backend.

mod inspect;
mod metadata;

pub use inspect::{
    inspect, Cod, Colour, Component, Container, Inspection, Jp2Info, Markers, Qcd, TileParts,
};
pub use metadata::{
    describe_xmp, read_xmp, read_xmp_file, XmpMetadata, XmpProperty, XMP_COLUMNS, XMP_UUID,
};

use std::fs::File;
use std::path::Path;

use j2k::{J2kDecoder, J2kScratchPool, PixelFormat};
use memmap2::Mmap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Facts about a codestream that the viewer needs to plan tiles.
#[derive(Debug, Clone)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    /// Wavelet decomposition levels: `decode(.., reduce)` works for `reduce <= levels`.
    pub levels: u8,
}

/// 8-bit grayscale pixels, row-major, `w * h` bytes.
pub struct Gray8Buf {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

pub trait TileDecoder: Send + Sync {
    fn info(&self) -> &Info;
    /// Decode `region` (full-resolution coordinates, aligned to `1 << reduce` on its origin)
    /// at `reduce` resolution levels down (0 = full, 1 = half, ...). The result is
    /// `ceil(w / 2^reduce) x ceil(h / 2^reduce)`.
    fn decode(&self, region: Rect, reduce: u8) -> Result<Gray8Buf, String>;
}

/// A memory-mapped JPEG 2000 / HTJ2K file decoded with the `j2k` crate.
pub struct J2kFile {
    map: Mmap,
    /// Start of the raw codestream (after any JP2/JPH box wrapper).
    offset: usize,
    info: Info,
}

impl J2kFile {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // SAFETY: the map is read-only; the file being modified underneath us can only
        // produce garbage pixels or a decode error, not memory unsafety in our code paths.
        let map = unsafe { Mmap::map(&file) }.map_err(|e| format!("{}: {e}", path.display()))?;
        let offset = codestream_offset(&map)?;
        let info = parse_info(&map[offset..])?;
        Ok(Self { map, offset, info })
    }

    pub fn file_len(&self) -> usize {
        self.map.len()
    }

    /// Describe the file's container, markers and tile layout. Walks every tile-part header, so it
    /// touches the whole file; call it off the UI thread.
    pub fn inspect(&self) -> Result<Inspection, String> {
        inspect(&self.map)
    }

    /// Embedded XMP metadata. Only walks the top-level boxes, so it is cheap even on huge files.
    pub fn xmp(&self) -> Result<Option<XmpMetadata>, String> {
        read_xmp(&self.map)
    }

    /// True for a bare codestream (`.j2c` / `.j2k`) with no JP2 boxes around it.
    pub fn is_raw_codestream(&self) -> bool {
        self.offset == 0
    }

    fn codestream(&self) -> &[u8] {
        &self.map[self.offset..]
    }
}

impl TileDecoder for J2kFile {
    fn info(&self) -> &Info {
        &self.info
    }

    fn decode(&self, region: Rect, reduce: u8) -> Result<Gray8Buf, String> {
        let info = &self.info;
        if region.w == 0
            || region.h == 0
            || region
                .x
                .checked_add(region.w)
                .is_none_or(|e| e > info.width)
            || region
                .y
                .checked_add(region.h)
                .is_none_or(|e| e > info.height)
        {
            return Err(format!(
                "region {region:?} outside {}x{}",
                info.width, info.height
            ));
        }
        let d = 1u32 << reduce;
        if region.x % d != 0 || region.y % d != 0 {
            return Err(format!("region origin {region:?} not aligned to {d}"));
        }
        let (ow, oh) = (region.w.div_ceil(d), region.h.div_ceil(d));
        let mut data = vec![0u8; ow as usize * oh as usize];
        let roi = j2k::Rect {
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
        };
        // The decoder is cheap to construct (lazy parse); a fresh one per call keeps this `Sync`.
        let mut dec = J2kDecoder::new(self.codestream()).map_err(|e| e.to_string())?;
        let mut pool = J2kScratchPool::new();
        dec.decode_region_scaled_pow2_into(
            &mut pool,
            &mut data,
            ow as usize,
            PixelFormat::Gray8,
            roi,
            reduce,
        )
        .map_err(|e| e.to_string())?;
        Ok(Gray8Buf { w: ow, h: oh, data })
    }
}

/// Wrap a raw 8-bit grayscale codestream in the JP2 file format, with the same box layout as common
/// encoders: signature, `ftyp` (jp2), `jp2h` {`ihdr`, `colr` greyscale}, `jp2c`.
pub fn wrap_jp2(codestream: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(codestream.len() + 100);
    v.extend_from_slice(&[0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A]);
    v.extend_from_slice(&[
        0, 0, 0, 20, b'f', b't', b'y', b'p', b'j', b'p', b'2', b' ', 0, 0, 0, 0, b'j', b'p', b'2',
        b' ',
    ]);
    v.extend_from_slice(&45u32.to_be_bytes()); // jp2h = 8 + ihdr 22 + colr 15
    v.extend_from_slice(b"jp2h");
    v.extend_from_slice(&22u32.to_be_bytes());
    v.extend_from_slice(b"ihdr");
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&[0, 1, 7, 7, 0, 0]); // 1 component, 8-bit unsigned, codec 7, no IPR
    v.extend_from_slice(&15u32.to_be_bytes());
    v.extend_from_slice(b"colr");
    v.extend_from_slice(&[1, 0, 0]); // enumerated colour space
    v.extend_from_slice(&17u32.to_be_bytes()); // greyscale
    let len = codestream.len() as u64 + 8;
    if len <= u64::from(u32::MAX) {
        v.extend_from_slice(&(len as u32).to_be_bytes());
        v.extend_from_slice(b"jp2c");
    } else {
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(b"jp2c");
        v.extend_from_slice(&(len + 8).to_be_bytes());
    }
    v.extend_from_slice(codestream);
    v
}

/// Offset of the raw codestream: 0 for a bare `.j2c`, else the payload of the `jp2c` box.
/// (The `j2k` crate's own JP2 parser rejects some HTJ2K-in-JP2 files, so we unwrap ourselves.)
pub fn codestream_offset(bytes: &[u8]) -> Result<usize, String> {
    if bytes.starts_with(&[0xFF, 0x4F]) {
        return Ok(0);
    }
    let mut p = 0usize;
    while p + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[p..p + 4].try_into().unwrap()) as u64;
        let kind = &bytes[p + 4..p + 8];
        let (header, total) = match len {
            0 => (8, (bytes.len() - p) as u64),
            1 => {
                if p + 16 > bytes.len() {
                    break;
                }
                (
                    16,
                    u64::from_be_bytes(bytes[p + 8..p + 16].try_into().unwrap()),
                )
            }
            n => (8, n),
        };
        if kind == b"jp2c" {
            return Ok(p + header);
        }
        if total < header as u64 {
            break;
        }
        p = p.saturating_add(total as usize);
    }
    Err("no JPEG 2000 codestream found".into())
}

/// Read SIZ and COD from a raw codestream (`FF4F` first).
pub fn parse_info(cs: &[u8]) -> Result<Info, String> {
    if !cs.starts_with(&[0xFF, 0x4F]) {
        return Err("not a JPEG 2000 codestream".into());
    }
    let be16 = |p: usize| u16::from_be_bytes([cs[p], cs[p + 1]]);
    let be32 = |p: usize| u32::from_be_bytes([cs[p], cs[p + 1], cs[p + 2], cs[p + 3]]);
    let (mut siz, mut levels) = (None, None);
    let mut p = 2;
    while p + 4 <= cs.len() {
        let marker = be16(p);
        if marker == 0xFF90 {
            break; // first tile-part: main header is over
        }
        let len = be16(p + 2) as usize;
        if p + 2 + len > cs.len() {
            return Err("truncated main header".into());
        }
        match marker {
            0xFF51 => {
                // SIZ: Lsiz Rsiz Xsiz Ysiz XOsiz YOsiz XTsiz YTsiz XTOsiz YTOsiz Csiz {Ssiz XRsiz YRsiz}
                let (xsiz, ysiz) = (be32(p + 6), be32(p + 10));
                let (xo, yo) = (be32(p + 14), be32(p + 18));
                let (xt, yt) = (be32(p + 22), be32(p + 26));
                let comps = be16(p + 38);
                let ssiz = cs[p + 40];
                if comps != 1 || (ssiz & 0x7F) + 1 != 8 || ssiz & 0x80 != 0 {
                    return Err(format!(
                        "only single-component unsigned 8-bit images are supported (components={comps}, ssiz={ssiz:#x})"
                    ));
                }
                if xo != 0 || yo != 0 {
                    return Err("image offsets are not supported".into());
                }
                siz = Some((xsiz, ysiz, xt, yt));
            }
            0xFF52 => levels = Some(cs[p + 9]), // COD: Lcod Scod SGcod(4) then SPcod[0] = levels
            _ => {}
        }
        p += 2 + len;
    }
    let (width, height, tile_width, tile_height) = siz.ok_or("missing SIZ")?;
    Ok(Info {
        width,
        height,
        tile_width,
        tile_height,
        levels: levels.ok_or("missing COD")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_codestream_has_offset_zero() {
        assert_eq!(codestream_offset(&[0xFF, 0x4F, 0xFF, 0x51]), Ok(0));
    }

    #[test]
    fn jp2_box_walk_finds_jp2c() {
        // signature box (12) + ftyp (20) + jp2c box with 2-byte payload.
        let mut v = vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];
        v.extend_from_slice(&[
            0, 0, 0, 20, b'f', b't', b'y', b'p', b'j', b'p', b'2', b' ', 0, 0, 0, 0, b'j', b'p',
            b'2', b' ',
        ]);
        v.extend_from_slice(&[0, 0, 0, 10, b'j', b'p', b'2', b'c', 0xFF, 0x4F]);
        assert_eq!(codestream_offset(&v), Ok(32 + 8));
    }

    #[test]
    fn rejects_garbage() {
        assert!(codestream_offset(&[1, 2, 3, 4, 5, 6, 7, 8, 9]).is_err());
        assert!(parse_info(&[0, 1, 2]).is_err());
    }
}
