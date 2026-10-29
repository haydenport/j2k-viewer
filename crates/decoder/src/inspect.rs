//! Read-only inspection of a JPEG 2000 / HTJ2K file: the JP2 wrapper, the main-header marker
//! segments and the tile-part layout. Pure parsing (no decoding); every read is bounds-checked so a
//! damaged file yields an error or partial result, never a panic.

/// How the codestream is stored.
#[derive(Debug, Clone, PartialEq)]
pub enum Container {
    /// A bare codestream (`.j2c` / `.j2k`).
    Raw,
    /// A JP2 / JPH file with boxes around the codestream.
    Jp2(Jp2Info),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Jp2Info {
    /// Major brand from the `ftyp` box, e.g. `jp2 ` or `jph `.
    pub brand: String,
    pub compatible: Vec<String>,
    pub colour: Option<Colour>,
    /// Image header box (`ihdr`): width, height, components, raw bits-per-component byte.
    pub header: Option<(u32, u32, u16, u8)>,
    /// Other top-level boxes (`xml `, `uuid`, `jp2i`, ...), in file order.
    pub other_boxes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    /// Enumerated colour space (16 = sRGB, 17 = greyscale, 18 = sYCC).
    Enumerated(u32),
    /// Embedded ICC profile of this many bytes.
    Icc(usize),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Component {
    pub bit_depth: u8,
    pub signed: bool,
    pub dx: u8,
    pub dy: u8,
}

/// Default coding style (COD marker) of the whole image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cod {
    pub scod: u8,
    /// 0 LRCP, 1 RLCP, 2 RPCL, 3 PCRL, 4 CPRL.
    pub progression: u8,
    pub layers: u16,
    pub mct: u8,
    pub levels: u8,
    /// Code-block width and height in samples.
    pub cb_w: u32,
    pub cb_h: u32,
    /// Code-block style byte: 0x40 = HT block coder, 0x80 = mixed HT / Part 1, low bits Part 1 modes.
    pub cb_style: u8,
    /// Wavelet: true = reversible 5/3, false = irreversible 9/7.
    pub reversible: bool,
    /// Precinct size exponents `(PPx, PPy)` per resolution, when user-defined.
    pub precincts: Vec<(u8, u8)>,
}

impl Cod {
    pub fn user_precincts(&self) -> bool {
        self.scod & 1 != 0
    }
    pub fn sop(&self) -> bool {
        self.scod & 2 != 0
    }
    pub fn eph(&self) -> bool {
        self.scod & 4 != 0
    }
    pub fn ht(&self) -> bool {
        self.cb_style & 0x40 != 0
    }
}

/// Quantization default (QCD marker).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Qcd {
    /// 0 none (reversible), 1 scalar derived, 2 scalar expounded.
    pub style: u8,
    pub guard_bits: u8,
    pub entries: usize,
}

/// Marker segments found in the main header (counts of segments, `Some` when present).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Markers {
    /// Tile-part length markers: total number of (tile, length) entries.
    pub tlm: Option<u32>,
    /// Packet length, main header: number of PLM segments.
    pub plm: u32,
    /// Packed packet headers, main header.
    pub ppm: u32,
    /// Extended capabilities (HTJ2K); the 32-bit Pcap field.
    pub cap: Option<u32>,
    pub crg: u32,
    pub poc: u32,
    pub rgn: u32,
    pub coc: u32,
    pub qcc: u32,
    pub cpf: u32,
}

/// What the tile-part headers contain (found by walking every tile-part).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TileParts {
    pub count: u32,
    /// Distinct tiles seen (tile-parts with index 0).
    pub tiles: u32,
    /// Largest declared number of tile-parts for one tile (0 = not declared).
    pub max_per_tile: u8,
    /// Tile-parts whose header carries packet-length markers (PLT).
    pub plt: u32,
    /// Tile-parts with packed packet headers (PPT).
    pub ppt: u32,
    /// Tile-parts that override coding parameters (COD/COC/QCD/QCC/POC/RGN).
    pub overrides: u32,
    /// The codestream ends with an EOC marker.
    pub eoc: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    pub file_len: u64,
    pub container: Container,
    pub codestream_len: u64,
    /// Rsiz: profile / capability bits (bit 14 = HTJ2K).
    pub rsiz: u16,
    pub width: u32,
    pub height: u32,
    pub image_offset: (u32, u32),
    pub tile_w: u32,
    pub tile_h: u32,
    pub tile_offset: (u32, u32),
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub components: Vec<Component>,
    pub cod: Option<Cod>,
    pub qcd: Option<Qcd>,
    pub markers: Markers,
    /// Text of Latin-1 COM markers (often names the encoder).
    pub comments: Vec<String>,
    pub tile_parts: TileParts,
}

/// Boxes inside `bytes[start..end]`: `(type, payload_start, payload_end)`.
pub(crate) fn boxes(bytes: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let end = end.min(bytes.len());
    let mut p = start;
    while p + 8 <= end {
        let len = u32::from_be_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]]) as u64;
        let kind = [bytes[p + 4], bytes[p + 5], bytes[p + 6], bytes[p + 7]];
        let (header, total) = match len {
            0 => (8usize, (end - p) as u64),
            1 => {
                if p + 16 > end {
                    break;
                }
                let xl = u64::from_be_bytes(bytes[p + 8..p + 16].try_into().unwrap());
                (16usize, xl)
            }
            n => (8usize, n),
        };
        if total < header as u64 {
            break;
        }
        let payload_end = (p as u64 + total).min(end as u64) as usize;
        out.push((kind, p + header, payload_end));
        p = payload_end;
    }
    out
}

fn tag(k: &[u8; 4]) -> String {
    k.iter()
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '?'
            }
        })
        .collect()
}

/// Split a file into its container description and the codestream start offset.
fn parse_container(file: &[u8]) -> Result<(Container, usize), String> {
    if file.starts_with(&[0xFF, 0x4F]) {
        return Ok((Container::Raw, 0));
    }
    let mut info = Jp2Info::default();
    let mut cs = None;
    for (kind, ps, pe) in boxes(file, 0, file.len()) {
        match &kind {
            b"jP  " => {}
            b"ftyp" if pe >= ps + 8 => {
                info.brand = tag(&[file[ps], file[ps + 1], file[ps + 2], file[ps + 3]]);
                info.compatible = file[ps + 8..pe]
                    .chunks_exact(4)
                    .map(|c| tag(&[c[0], c[1], c[2], c[3]]))
                    .collect();
            }
            b"jp2h" => {
                for (k, s, e) in boxes(file, ps, pe) {
                    match &k {
                        b"ihdr" if e >= s + 14 => {
                            let be32 = |o: usize| {
                                u32::from_be_bytes([file[o], file[o + 1], file[o + 2], file[o + 3]])
                            };
                            info.header = Some((
                                be32(s + 4),
                                be32(s),
                                u16::from_be_bytes([file[s + 8], file[s + 9]]),
                                file[s + 10],
                            ));
                        }
                        b"colr" if e >= s + 3 && info.colour.is_none() => {
                            info.colour = Some(match file[s] {
                                1 if e >= s + 7 => Colour::Enumerated(u32::from_be_bytes([
                                    file[s + 3],
                                    file[s + 4],
                                    file[s + 5],
                                    file[s + 6],
                                ])),
                                2 | 3 => Colour::Icc(e - s - 3),
                                _ => Colour::Other,
                            });
                        }
                        _ => {}
                    }
                }
            }
            b"jp2c" => {
                if cs.is_none() {
                    cs = Some(ps);
                }
            }
            // Say so when a `uuid` box carries XMP: the Metadata panel has content for this file.
            b"uuid" if pe >= ps + 16 && file[ps..ps + 16] == crate::metadata::XMP_UUID => info
                .other_boxes
                .push(format!("uuid (XMP, {})", size_text((pe - ps - 16) as u64))),
            _ => info.other_boxes.push(tag(&kind)),
        }
    }
    match cs {
        Some(off) => Ok((Container::Jp2(info), off)),
        None => Err("no JPEG 2000 codestream found".into()),
    }
}

/// Bounds-checked big-endian reads (out of range reads as 0).
struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn u8(&self, p: usize) -> u8 {
        self.0.get(p).copied().unwrap_or(0)
    }
    fn u16(&self, p: usize) -> u16 {
        u16::from_be_bytes([self.u8(p), self.u8(p + 1)])
    }
    fn u32(&self, p: usize) -> u32 {
        u32::from_be_bytes([self.u8(p), self.u8(p + 1), self.u8(p + 2), self.u8(p + 3)])
    }
}

/// Inspect a whole file (container + codestream).
pub fn inspect(file: &[u8]) -> Result<Inspection, String> {
    let (container, off) = parse_container(file)?;
    let cs = &file[off..];
    let b = Bytes(cs);
    if cs.len() < 4 || b.u16(0) != 0xFF4F {
        return Err("not a JPEG 2000 codestream".into());
    }

    let mut siz = None;
    let mut cod = None;
    let mut qcd = None;
    let mut markers = Markers::default();
    let mut comments = Vec::new();
    let mut p = 2usize;
    // Main header: marker segments up to the first tile-part (SOT) or EOC.
    loop {
        if p + 2 > cs.len() {
            break;
        }
        let m = b.u16(p);
        if m == 0xFF90 || m == 0xFFD9 {
            break;
        }
        if b.u8(p) != 0xFF {
            return Err(format!("bad marker {m:#06x} at codestream offset {p}"));
        }
        if (0xFF30..=0xFF3F).contains(&m) {
            p += 2; // reserved markers without a length
            continue;
        }
        let len = b.u16(p + 2) as usize;
        if len < 2 || p + 2 + len > cs.len() {
            return Err("truncated main header".into());
        }
        let q = p + 4; // payload start
        match m {
            0xFF51 if len >= 41 => {
                let csiz = b.u16(q + 34) as usize;
                let comps = (0..csiz)
                    .map(|i| {
                        let ssiz = b.u8(q + 36 + 3 * i);
                        Component {
                            bit_depth: (ssiz & 0x7F) + 1,
                            signed: ssiz & 0x80 != 0,
                            dx: b.u8(q + 37 + 3 * i),
                            dy: b.u8(q + 38 + 3 * i),
                        }
                    })
                    .collect::<Vec<_>>();
                siz = Some((
                    b.u16(q),
                    (b.u32(q + 2), b.u32(q + 6)),
                    (b.u32(q + 10), b.u32(q + 14)),
                    (b.u32(q + 18), b.u32(q + 22)),
                    (b.u32(q + 26), b.u32(q + 30)),
                    comps,
                ));
            }
            0xFF50 => markers.cap = Some(b.u32(q)),
            0xFF52 if len >= 12 && cod.is_none() => {
                let scod = b.u8(q);
                let levels = b.u8(q + 5);
                let precincts = if scod & 1 != 0 {
                    (0..=levels as usize)
                        .map(|i| {
                            let v = b.u8(q + 10 + i);
                            (v & 0x0F, v >> 4)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                cod = Some(Cod {
                    scod,
                    progression: b.u8(q + 1),
                    layers: b.u16(q + 2),
                    mct: b.u8(q + 4),
                    levels,
                    cb_w: 1u32 << (u32::from(b.u8(q + 6) & 0x0F) + 2),
                    cb_h: 1u32 << (u32::from(b.u8(q + 7) & 0x0F) + 2),
                    cb_style: b.u8(q + 8),
                    reversible: b.u8(q + 9) == 1,
                    precincts,
                });
            }
            0xFF5C if len >= 4 && qcd.is_none() => {
                let sqcd = b.u8(q);
                let style = sqcd & 0x1F;
                let body = len - 3;
                qcd = Some(Qcd {
                    style,
                    guard_bits: sqcd >> 5,
                    entries: match style {
                        0 => body,
                        _ => body / 2,
                    },
                });
            }
            0xFF55 if len >= 4 => {
                // Ztlm, Stlm (ST = bytes of tile index: 0/1/2, SP = 0: 16-bit / 1: 32-bit length)
                let stlm = b.u8(q + 1);
                let st = usize::from((stlm >> 4) & 3);
                let sp = if (stlm >> 6) & 1 == 1 { 4 } else { 2 };
                let entries = (len - 4) / (st + sp);
                markers.tlm = Some(markers.tlm.unwrap_or(0) + entries as u32);
            }
            0xFF57 => markers.plm += 1,
            0xFF60 => markers.ppm += 1,
            0xFF63 => markers.crg += 1,
            0xFF5F => markers.poc += 1,
            0xFF5E => markers.rgn += 1,
            0xFF53 => markers.coc += 1,
            0xFF5D => markers.qcc += 1,
            0xFF59 => markers.cpf += 1,
            0xFF64 if len >= 4 => {
                // Rcom = 1: Latin-1 text. Other registrations are binary: skip them.
                if b.u16(q) == 1 {
                    let text: String = cs[q + 2..p + 2 + len]
                        .iter()
                        .map(|&c| {
                            if (0x20..0x7f).contains(&c) {
                                c as char
                            } else {
                                '.'
                            }
                        })
                        .collect();
                    comments.push(text.trim().to_string());
                }
            }
            _ => {}
        }
        p += 2 + len;
    }
    let (rsiz, (xsiz, ysiz), (xo, yo), (xt, yt), (xto, yto), components) =
        siz.ok_or("missing SIZ marker")?;
    if xt == 0 || yt == 0 {
        return Err("tile size is zero".into());
    }
    let tiles_x = xsiz.saturating_sub(xto).div_ceil(xt);
    let tiles_y = ysiz.saturating_sub(yto).div_ceil(yt);

    // Walk every tile-part: SOT(FF90) Lsot=10 Isot(2) Psot(4) TPsot(1) TNsot(1), header markers, SOD.
    let mut tp = TileParts::default();
    while p + 12 <= cs.len() && b.u16(p) == 0xFF90 {
        let psot = b.u32(p + 6) as usize;
        let (tpsot, tnsot) = (b.u8(p + 10), b.u8(p + 11));
        tp.count += 1;
        if tpsot == 0 {
            tp.tiles += 1;
        }
        tp.max_per_tile = tp.max_per_tile.max(tnsot);
        let end = if psot == 0 {
            cs.len()
        } else {
            (p + psot).min(cs.len())
        };
        let (mut plt, mut ppt, mut over) = (false, false, false);
        let mut h = p + 12;
        while h + 4 <= end && b.u16(h) != 0xFF93 {
            match b.u16(h) {
                0xFF58 => plt = true,
                0xFF61 => ppt = true,
                0xFF52 | 0xFF53 | 0xFF5C | 0xFF5D | 0xFF5F | 0xFF5E => over = true,
                _ => {}
            }
            let l = b.u16(h + 2) as usize;
            if l < 2 {
                break;
            }
            h += 2 + l;
        }
        tp.plt += u32::from(plt);
        tp.ppt += u32::from(ppt);
        tp.overrides += u32::from(over);
        if psot == 0 {
            // The last tile-part runs to the end of the codestream (before EOC).
            p = cs.len();
            tp.eoc = cs.len() >= 2 && b.u16(cs.len() - 2) == 0xFFD9;
            break;
        }
        p += psot;
    }
    if !tp.eoc {
        tp.eoc = p + 2 <= cs.len() && b.u16(p) == 0xFFD9;
    }

    Ok(Inspection {
        file_len: file.len() as u64,
        container,
        codestream_len: cs.len() as u64,
        rsiz,
        width: xsiz.saturating_sub(xo),
        height: ysiz.saturating_sub(yo),
        image_offset: (xo, yo),
        tile_w: xt,
        tile_h: yt,
        tile_offset: (xto, yto),
        tiles_x,
        tiles_y,
        components,
        cod,
        qcd,
        markers,
        comments,
        tile_parts: tp,
    })
}

fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.1} KB", b / 1e3)
    }
}

fn yes_no(v: bool) -> &'static str {
    if v {
        "yes"
    } else {
        "no"
    }
}

impl Inspection {
    pub fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub fn tile_count(&self) -> u64 {
        u64::from(self.tiles_x) * u64::from(self.tiles_y)
    }

    /// True for HTJ2K (Part 15): Rsiz capability bit, CAP marker for Part 15, or the HT block coder.
    pub fn htj2k(&self) -> bool {
        self.rsiz & 0x4000 != 0
            || self.markers.cap.is_some_and(|c| c & 0x0002_0000 != 0)
            || self.cod.as_ref().is_some_and(Cod::ht)
    }

    /// Short human-readable description, one fact per line (ASCII, at most ~62 columns).
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        v.push(match &self.container {
            Container::Raw => format!("File: raw codestream, {}", size_text(self.file_len)),
            Container::Jp2(j) => format!(
                "File: JP2 wrapper, brand \"{}\", {}",
                j.brand,
                size_text(self.file_len)
            ),
        });
        v.push(format!(
            "Size: {} x {} px ({:.1} MP)",
            self.width,
            self.height,
            self.pixels() as f64 / 1e6
        ));
        if self.image_offset != (0, 0) {
            v.push(format!(
                "Image offset on canvas: ({}, {})",
                self.image_offset.0, self.image_offset.1
            ));
        }
        v.push(self.components_line());
        if let Container::Jp2(j) = &self.container {
            if let Some(c) = j.colour {
                v.push(format!(
                    "Colour: {}",
                    match c {
                        Colour::Enumerated(16) => "sRGB".to_string(),
                        Colour::Enumerated(17) => "greyscale".to_string(),
                        Colour::Enumerated(18) => "sYCC".to_string(),
                        Colour::Enumerated(n) => format!("enumerated space {n}"),
                        Colour::Icc(n) => format!("ICC profile ({n} bytes)"),
                        Colour::Other => "restricted ICC / other".to_string(),
                    }
                ));
            }
        }
        let ht = self.htj2k();
        let mixed = self.cod.as_ref().is_some_and(|c| c.cb_style & 0x80 != 0);
        v.push(format!(
            "Codec: {}",
            match (ht, mixed) {
                (true, true) => "HTJ2K (Part 15), mixed with Part 1 blocks",
                (true, false) => "HTJ2K (Part 15, high-throughput)",
                (false, _) => "JPEG 2000 Part 1 (EBCOT)",
            }
        ));
        if let Some(c) = &self.cod {
            let lossless = c.reversible && self.qcd.is_some_and(|q| q.style == 0);
            v.push(format!(
                "Wavelet: {}{}",
                if c.reversible {
                    "5/3 reversible"
                } else {
                    "9/7 irreversible"
                },
                if lossless { " (lossless)" } else { " (lossy)" }
            ));
            v.push(format!(
                "Levels: {} ({} resolutions), layers: {}, order: {}",
                c.levels,
                u32::from(c.levels) + 1,
                c.layers,
                ["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"]
                    .get(c.progression as usize)
                    .copied()
                    .unwrap_or("?")
            ));
        }
        v.push(if self.tile_count() <= 1 {
            "Tiles: 1 (single tile)".to_string()
        } else {
            format!(
                "Tiles: {} x {} = {} ({} x {} px)",
                self.tiles_x,
                self.tiles_y,
                self.tile_count(),
                self.tile_w,
                self.tile_h
            )
        });
        if self.tile_offset != (0, 0) {
            v.push(format!(
                "Tile grid origin: ({}, {})",
                self.tile_offset.0, self.tile_offset.1
            ));
        }
        let tp = &self.tile_parts;
        v.push(if tp.count == 0 {
            "Tile-parts: none found".to_string()
        } else if tp.count == tp.tiles {
            format!("Tile-parts: {} (1 per tile)", tp.count)
        } else {
            format!(
                "Tile-parts: {} (up to {} per tile)",
                tp.count,
                tp.max_per_tile.max(1)
            )
        });
        if let Some(c) = &self.cod {
            let mut blocks = format!("Code-blocks: {} x {}", c.cb_w, c.cb_h);
            if c.ht() {
                blocks.push_str(" (HT)");
            } else if c.cb_style & 0x3F != 0 {
                let names = [
                    (0x01, "bypass"),
                    (0x02, "reset"),
                    (0x04, "terminate"),
                    (0x08, "causal"),
                    (0x10, "predict"),
                    (0x20, "segsym"),
                ];
                let on: Vec<&str> = names
                    .iter()
                    .filter(|(m, _)| c.cb_style & m != 0)
                    .map(|(_, n)| *n)
                    .collect();
                blocks.push_str(&format!(" ({})", on.join("+")));
            }
            v.push(blocks);
            v.push(if c.user_precincts() && !c.precincts.is_empty() {
                let dim = |(x, y): (u8, u8)| format!("{}x{}", 1u64 << x.min(31), 1u64 << y.min(31));
                let first = dim(c.precincts[0]);
                let last = dim(*c.precincts.last().unwrap());
                if first == last {
                    format!("Precincts: user-defined, all {first}")
                } else {
                    format!("Precincts: user-defined, {first} .. {last}")
                }
            } else {
                "Precincts: default (one per resolution)".to_string()
            });
        }
        if let Some(q) = &self.qcd {
            v.push(format!(
                "Quantization: {}, guard bits {}",
                match q.style {
                    0 => "none",
                    1 => "scalar derived",
                    2 => "scalar expounded",
                    _ => "unknown",
                },
                q.guard_bits
            ));
        }
        let m = &self.markers;
        let count = |n: u32, unit: &str| {
            if n > 0 {
                format!("yes ({n} {unit})")
            } else {
                "no".to_string()
            }
        };
        v.push(format!(
            "TLM: {}  PLM: {}",
            match m.tlm {
                Some(n) => format!("yes ({n})"),
                None => "no".to_string(),
            },
            yes_no(m.plm > 0)
        ));
        v.push(format!(
            "PLT: {}  PPM/PPT: {}",
            count(tp.plt, "tile-parts"),
            yes_no(m.ppm > 0 || tp.ppt > 0)
        ));
        if let Some(c) = &self.cod {
            v.push(format!(
                "SOP: {}  EPH: {}  MCT: {}",
                yes_no(c.sop()),
                yes_no(c.eph()),
                yes_no(c.mct != 0)
            ));
        }
        let mut other = Vec::new();
        if let Some(c) = m.cap {
            other.push(format!("CAP({c:#x})"));
        }
        for (n, name) in [
            (m.poc, "POC"),
            (m.rgn, "RGN"),
            (m.coc, "COC"),
            (m.qcc, "QCC"),
            (m.crg, "CRG"),
            (m.cpf, "CPF"),
        ] {
            if n > 0 {
                other.push(name.to_string());
            }
        }
        if !self.comments.is_empty() {
            other.push("COM".to_string());
        }
        if tp.overrides > 0 {
            other.push(format!("tile overrides({})", tp.overrides));
        }
        v.push(if other.is_empty() {
            "Other markers: none".to_string()
        } else {
            format!("Other markers: {}", other.join(", "))
        });
        if let Container::Jp2(j) = &self.container {
            if !j.other_boxes.is_empty() {
                v.push(format!("Extra boxes: {}", j.other_boxes.join(", ")));
            }
        }
        let raw_bits: u64 = self
            .components
            .iter()
            .map(|c| u64::from(c.bit_depth))
            .sum::<u64>()
            * self.pixels();
        if self.codestream_len > 0 && raw_bits > 0 {
            v.push(format!(
                "Rate: {:.2} bits/px ({:.1}:1)",
                self.codestream_len as f64 * 8.0 / self.pixels().max(1) as f64,
                raw_bits as f64 / 8.0 / self.codestream_len as f64
            ));
        }
        for c in self.comments.iter().take(2) {
            let mut t: String = c.chars().take(52).collect();
            if c.chars().count() > 52 {
                t.push_str("...");
            }
            v.push(format!("Comment: \"{t}\""));
        }
        v.push(self.random_access_line());
        if !tp.eoc {
            v.push("Warning: no EOC marker (file may be truncated)".to_string());
        }
        v
    }

    fn components_line(&self) -> String {
        let c = &self.components;
        if c.is_empty() {
            return "Components: none".to_string();
        }
        let same = c.iter().all(|x| *x == c[0]);
        let depth = |x: &Component| {
            format!(
                "{}-bit {}",
                x.bit_depth,
                if x.signed { "signed" } else { "unsigned" }
            )
        };
        if same {
            format!("Components: {} x {}", c.len(), depth(&c[0]))
        } else {
            format!(
                "Components: {} ({})",
                c.len(),
                c.iter().map(depth).collect::<Vec<_>>().join(", ")
            )
        }
    }

    /// How well the layout supports decoding only the visible part (what the viewer relies on).
    fn random_access_line(&self) -> String {
        let levels = self.cod.as_ref().map_or(0, |c| c.levels);
        let precincts = self.cod.as_ref().is_some_and(Cod::user_precincts);
        let big = self.tile_w.max(self.tile_h);
        let verdict = if self.tile_count() <= 1 && !precincts {
            "poor: one tile, no precincts"
        } else if self.tile_count() <= 1 {
            "ok: one tile, precincts help"
        } else if big > 2048 {
            "ok: large tiles slow small views"
        } else if levels == 0 {
            "ok: tiled, but no zoomed-out levels"
        } else {
            "good: tiled with resolution levels"
        };
        format!("Random access: {verdict}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use j2k::{
        encode_j2k_lossless, J2kBlockCodingMode, J2kEncodeValidation, J2kLosslessEncodeOptions,
        J2kLosslessSamples, J2kProgressionOrder,
    };

    fn image(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .map(|i| ((i % w) / 3 + (i / w) / 5) as u8)
            .collect()
    }

    fn encode(w: u32, h: u32, tweak: impl FnOnce(&mut J2kLosslessEncodeOptions)) -> Vec<u8> {
        let img = image(w, h);
        let mut o = J2kLosslessEncodeOptions::default();
        o.block_coding_mode = J2kBlockCodingMode::HighThroughput;
        o.tile_size = Some((128, 128));
        o.max_decomposition_levels = Some(3);
        o.validation = J2kEncodeValidation::External;
        tweak(&mut o);
        encode_j2k_lossless(
            J2kLosslessSamples::new(&img, w, h, 1, 8, false).unwrap(),
            &o,
        )
        .expect("encode")
        .codestream
    }

    #[test]
    fn htj2k_tiled_stream_without_optional_markers() {
        let i = inspect(&encode(300, 200, |_| {})).unwrap();
        assert_eq!((i.width, i.height), (300, 200));
        assert!(i.htj2k());
        assert_eq!((i.tiles_x, i.tiles_y, i.tile_count()), (3, 2, 6));
        assert_eq!((i.tile_w, i.tile_h), (128, 128));
        let c = i.cod.as_ref().unwrap();
        assert!(c.reversible && c.ht());
        assert_eq!(c.levels, 3);
        assert_eq!((c.cb_w, c.cb_h), (64, 64));
        assert_eq!(i.qcd.unwrap().style, 0);
        assert_eq!(
            i.components,
            vec![Component {
                bit_depth: 8,
                signed: false,
                dx: 1,
                dy: 1
            }]
        );
        assert_eq!(i.markers.tlm, None);
        assert_eq!(i.markers.plm, 0);
        assert_eq!(i.tile_parts.count, 6);
        assert_eq!(i.tile_parts.tiles, 6);
        assert_eq!(i.tile_parts.plt, 0);
        assert!(i.tile_parts.eoc);
        assert!(!c.sop() && !c.eph());
        assert_eq!(i.container, Container::Raw);
        let text = i.lines().join("\n");
        assert!(text.contains("HTJ2K"), "{text}");
        assert!(text.contains("5/3 reversible (lossless)"), "{text}");
        assert!(text.contains("Tiles: 3 x 2 = 6 (128 x 128 px)"), "{text}");
        assert!(text.contains("TLM: no  PLM: no"), "{text}");
    }

    #[test]
    fn classic_stream_is_not_htj2k() {
        let i = inspect(&encode(200, 200, |o| {
            o.block_coding_mode = J2kBlockCodingMode::Classic;
        }))
        .unwrap();
        assert!(!i.htj2k());
        assert!(!i.cod.as_ref().unwrap().ht());
        assert!(i.lines().join("\n").contains("Part 1 (EBCOT)"));
    }

    #[test]
    fn detects_tlm_plt_sop_eph() {
        let i = inspect(&encode(300, 200, |o| {
            o.write_tlm = true;
            o.write_plt = true;
            o.write_sop = true;
            o.write_eph = true;
        }))
        .unwrap();
        assert_eq!(i.markers.tlm, Some(6), "one TLM entry per tile-part");
        assert_eq!(i.tile_parts.plt, 6, "every tile-part header carries a PLT");
        let c = i.cod.as_ref().unwrap();
        assert!(c.sop() && c.eph());
        let text = i.lines().join("\n");
        assert!(text.contains("TLM: yes (6)"), "{text}");
        assert!(text.contains("PLT: yes (6 tile-parts)"), "{text}");
        assert!(text.contains("SOP: yes  EPH: yes"), "{text}");
    }

    #[test]
    fn detects_progression_and_layers() {
        let i = inspect(&encode(256, 256, |o| {
            o.progression = J2kProgressionOrder::Rpcl;
        }))
        .unwrap();
        assert_eq!(i.cod.as_ref().unwrap().progression, 2);
        assert!(i.lines().join("\n").contains("order: RPCL"));
    }

    #[test]
    fn splits_into_multiple_tile_parts() {
        let i = inspect(&encode(256, 256, |o| {
            o.tile_part_packet_limit = Some(3);
        }))
        .unwrap();
        assert!(
            i.tile_parts.count > i.tile_parts.tiles,
            "{:?}",
            i.tile_parts
        );
        assert!(i.tile_parts.max_per_tile > 1);
        assert!(i.lines().join("\n").contains("per tile)"));
    }

    #[test]
    fn jp2_wrapper_is_described() {
        let cs = encode(200, 200, |_| {});
        let jp2 = crate::wrap_jp2(&cs, 200, 200);
        let i = inspect(&jp2).unwrap();
        let Container::Jp2(j) = &i.container else {
            panic!("expected a JP2 container")
        };
        assert_eq!(j.brand, "jp2 ");
        assert_eq!(j.colour, Some(Colour::Enumerated(17)));
        assert_eq!(j.header, Some((200, 200, 1, 7)));
        assert_eq!(i.codestream_len as usize, cs.len());
        assert_eq!(i.file_len as usize, jp2.len());
        let text = i.lines().join("\n");
        assert!(text.contains("JP2 wrapper, brand \"jp2 \""), "{text}");
        assert!(text.contains("Colour: greyscale"), "{text}");
    }

    #[test]
    fn xmp_box_is_mentioned_in_the_extra_boxes_line() {
        let cs = encode(200, 200, |_| {});
        let mut jp2 = crate::wrap_jp2(&cs, 200, 200);
        let packet = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>";
        let mut payload = crate::metadata::XMP_UUID.to_vec();
        payload.extend_from_slice(packet);
        jp2.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
        jp2.extend_from_slice(b"uuid");
        jp2.extend_from_slice(&payload);
        let i = inspect(&jp2).unwrap();
        let Container::Jp2(j) = &i.container else {
            panic!("expected a JP2 container")
        };
        assert_eq!(j.other_boxes.len(), 1);
        assert!(
            j.other_boxes[0].starts_with("uuid (XMP, "),
            "{:?}",
            j.other_boxes
        );
        assert!(i.lines().join("\n").contains("Extra boxes: uuid (XMP, "));
        // A file without extra boxes has no such line.
        let plain = inspect(&crate::wrap_jp2(&cs, 200, 200)).unwrap();
        assert!(!plain.lines().join("\n").contains("Extra boxes"));
    }

    #[test]
    fn single_tile_is_flagged_as_poor_random_access() {
        let i = inspect(&encode(100, 100, |o| o.tile_size = None)).unwrap();
        assert_eq!(i.tile_count(), 1);
        assert!(i.lines().join("\n").contains("Random access: poor"));
    }

    #[test]
    fn truncated_stream_warns_instead_of_panicking() {
        let cs = encode(300, 200, |_| {});
        let cut = &cs[..cs.len() - 40];
        let i = inspect(cut).unwrap();
        assert!(!i.tile_parts.eoc);
        assert!(i.lines().join("\n").contains("Warning: no EOC"));
    }

    #[test]
    fn garbage_and_short_inputs_are_errors_not_panics() {
        assert!(inspect(&[]).is_err());
        assert!(inspect(&[1, 2, 3, 4, 5, 6, 7, 8, 9]).is_err());
        assert!(inspect(&[0xFF, 0x4F]).is_err());
        // Every prefix of a valid file must be handled without panicking.
        let cs = encode(300, 200, |_| {});
        for n in (0..cs.len()).step_by(7) {
            let _ = inspect(&cs[..n]);
        }
    }

    #[test]
    fn hand_built_markers_are_counted() {
        // SOC, SIZ (64x64, one 64x64 tile, 1 comp), COD, QCD, PLM, PPM, COM, then SOT/SOD/EOC.
        let mut v = vec![0xFF, 0x4F];
        let mut siz = vec![0xFF, 0x51, 0, 41, 0x40, 0x00];
        for n in [64u32, 64, 0, 0, 64, 64, 0, 0] {
            siz.extend_from_slice(&n.to_be_bytes());
        }
        siz.extend_from_slice(&[0, 1, 7, 1, 1]);
        v.extend(siz);
        // COD: Scod 0 (default precincts), LRCP, 1 layer, no MCT, 2 levels, 64x64 blocks, HT, 5/3.
        v.extend_from_slice(&[0xFF, 0x52, 0, 12, 0x00, 0, 0, 1, 0, 2, 4, 4, 0x40, 1]);
        v.extend_from_slice(&[0xFF, 0x5C, 0, 4, 0x40, 0x88]);
        v.extend_from_slice(&[0xFF, 0x57, 0, 4, 0, 0]);
        v.extend_from_slice(&[0xFF, 0x60, 0, 4, 0, 0]);
        v.extend_from_slice(&[0xFF, 0x64, 0, 9, 0, 1, b'a', b'b', b'c', b'd', b'e']);
        // SOT (12 bytes) + SOD (2) + 4 data bytes = Psot 18.
        v.extend_from_slice(&[
            0xFF, 0x90, 0, 10, 0, 0, 0, 0, 0, 18, 0, 1, 0xFF, 0x93, 1, 2, 3, 4,
        ]);
        v.extend_from_slice(&[0xFF, 0xD9]);
        let i = inspect(&v).unwrap();
        assert!(i.htj2k(), "Rsiz bit 14 marks HTJ2K");
        assert_eq!(i.markers.plm, 1);
        assert_eq!(i.markers.ppm, 1);
        assert_eq!(i.comments, vec!["abcde".to_string()]);
        assert_eq!(i.qcd.unwrap().guard_bits, 2);
        assert_eq!(i.tile_parts.count, 1);
        assert!(i.tile_parts.eoc);
    }
}
