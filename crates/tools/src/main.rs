//! Phase 0 decoder benchmark: `tools bench <file.jp2> [runs]`.

use std::time::Instant;

use j2k::{J2kDecoder, J2kScratchPool, PixelFormat, Rect};

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("bench") => {
            let path = args.next().expect("usage: tools bench <file> [runs]");
            let runs: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(3);
            bench_j2k(&path, runs);
        }
        Some("verify") => {
            let path = args.next().expect("usage: tools verify <file.png>");
            verify_png(&path);
        }
        Some("encode") => {
            let src = args.next().expect("usage: tools encode <in.png> <out.j2c>");
            let dst = args.next().expect("usage: tools encode <in.png> <out.j2c>");
            encode_streaming(&src, &dst);
        }
        Some("interop") => {
            let n = |a: Option<String>| a.unwrap().parse::<u32>().unwrap();
            let (w, h, tile, levels) = (n(args.next()), n(args.next()), n(args.next()), n(args.next()));
            interop(w, h, tile, levels);
        }
        Some("scan") => {
            // tools scan <file>: main-header markers and tile-part layout.
            let path = args.next().expect("file");
            let all = std::fs::read(&path).unwrap();
            let cs = &all[decoder::codestream_offset(&all).expect("codestream")..];
            let mut p = 2;
            let mut names = Vec::new();
            while u16::from_be_bytes([cs[p], cs[p + 1]]) != 0xFF90 {
                names.push(format!("{:04X}", u16::from_be_bytes([cs[p], cs[p + 1]])));
                p += 2 + u16::from_be_bytes([cs[p + 2], cs[p + 3]]) as usize;
            }
            println!("main header ends at {p}; markers: {}", names.join(" "));
            let (mut parts, mut multi, mut ordered, mut last) = (0u32, 0u32, true, -1i64);
            let mut sizes = Vec::new();
            while p + 12 <= cs.len() && cs[p] == 0xFF && cs[p + 1] == 0x90 {
                let isot = i64::from(u16::from_be_bytes([cs[p + 4], cs[p + 5]]));
                let psot = u32::from_be_bytes([cs[p + 6], cs[p + 7], cs[p + 8], cs[p + 9]]) as usize;
                if cs[p + 11] > 1 {
                    multi += 1;
                }
                ordered &= isot > last;
                last = isot;
                sizes.push(psot);
                parts += 1;
                if psot == 0 {
                    break;
                }
                p += psot;
            }
            sizes.sort_unstable();
            println!(
                "tile-parts {parts}, with TNsot>1: {multi}, tiles in raster order: {ordered}, size min/median/max {} / {} / {} bytes",
                sizes[0],
                sizes[sizes.len() / 2],
                sizes[sizes.len() - 1]
            );
        }
        Some("pngtime") => {
            // tools pngtime <file.png>: seconds to decode the whole PNG into memory.
            let path = args.next().expect("file.png");
            let t = Instant::now();
            let (w, h, data) = read_gray_png(&path);
            println!("{:.3} s ({w}x{h}, {} MB decoded)", t.elapsed().as_secs_f64(), data.len() / 1_000_000);
        }
        Some("mktest") => {
            // tools mktest <out.j2c> <out.png> <w> <h>: deterministic test image as tiled HTJ2K + PNG.
            use j2k::{encode_j2k_lossless, J2kBlockCodingMode, J2kEncodeValidation,
                      J2kLosslessEncodeOptions, J2kLosslessSamples};
            let j2c = args.next().expect("out.j2c");
            let png_path = args.next().expect("out.png");
            let w: u32 = args.next().expect("w").parse().unwrap();
            let h: u32 = args.next().expect("h").parse().unwrap();
            let mut s = 0x1234_5678u32;
            let img: Vec<u8> = (0..w * h)
                .map(|i| {
                    s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                    let (x, y) = (i % w, i / w);
                    (x / 5 + y / 3 + (x * y / 4096) + (s >> 28)) as u8
                })
                .collect();
            let mut o = J2kLosslessEncodeOptions::default();
            o.block_coding_mode = J2kBlockCodingMode::HighThroughput;
            o.tile_size = Some((1024, 1024));
            o.max_decomposition_levels = Some(5);
            o.validation = J2kEncodeValidation::External;
            let enc = encode_j2k_lossless(J2kLosslessSamples::new(&img, w, h, 1, 8, false).unwrap(), &o).expect("encode");
            std::fs::write(&j2c, &enc.codestream).unwrap();
            let f = std::io::BufWriter::new(std::fs::File::create(&png_path).unwrap());
            let mut e = png::Encoder::new(f, w, h);
            e.set_color(png::ColorType::Grayscale);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&img).unwrap();
            println!("wrote {j2c} ({} bytes) and {png_path} ({w}x{h})", enc.codestream.len());
        }
        Some("cmp-snapshot") => {
            // tools cmp-snapshot <snapshot.png (RGBA)> <source.png (gray)> <x0> <y0>
            let snap = args.next().expect("snapshot.png");
            let src = args.next().expect("source.png");
            let x0: usize = args.next().expect("x0").parse().unwrap();
            let y0: usize = args.next().expect("y0").parse().unwrap();
            let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&snap).unwrap()));
            let mut rd = dec.read_info().unwrap();
            let mut rgba = vec![0u8; rd.output_buffer_size().unwrap()];
            let info = rd.next_frame(&mut rgba).unwrap();
            assert_eq!(info.color_type, png::ColorType::Rgba);
            let (sw, sh) = (info.width as usize, info.height as usize);
            let (stride, _h, gray) = read_gray_png(&src);
            let stride = stride as usize;
            let (mut bad, mut max_diff) = (0u64, 0u8);
            for y in 0..sh {
                for x in 0..sw {
                    let (a, b) = (rgba[(y * sw + x) * 4], gray[(y0 + y) * stride + x0 + x]);
                    // R, G, B must agree too (grayscale output).
                    assert_eq!(a, rgba[(y * sw + x) * 4 + 1]);
                    if a != b {
                        bad += 1;
                        max_diff = max_diff.max(a.abs_diff(b));
                    }
                }
            }
            println!("{sw}x{sh} at ({x0},{y0}): {bad} differing pixels, max diff {max_diff}");
            println!("{}", if bad == 0 { "PASS: snapshot is pixel-identical to the source" } else { "FAIL" });
        }
        Some("jlevels") => {
            // tools jlevels W H TILE REQ [rpcl]
            use j2k::{encode_j2k_lossless, J2kBlockCodingMode, J2kEncodeValidation,
                      J2kLosslessEncodeOptions, J2kLosslessSamples, J2kProgressionOrder};
            let n = |a: Option<String>| a.unwrap().parse::<u32>().unwrap();
            let (w, h, tile, req) = (n(args.next()), n(args.next()), n(args.next()), n(args.next()));
            let img: Vec<u8> = (0..w * h).map(|i| (i % 251) as u8).collect();
            let mut o = J2kLosslessEncodeOptions::default();
            o.block_coding_mode = J2kBlockCodingMode::HighThroughput;
            o.tile_size = Some((tile, tile));
            o.max_decomposition_levels = Some(req as u8);
            o.validation = J2kEncodeValidation::External;
            if args.next().as_deref() == Some("rpcl") {
                o.progression = J2kProgressionOrder::Rpcl;
            }
            let e = encode_j2k_lossless(J2kLosslessSamples::new(&img, w, h, 1, 8, false).unwrap(), &o).unwrap();
            let i = e.codestream.windows(2).position(|m| m == [0xFF, 0x52]).unwrap();
            println!("{w}x{h} tile {tile} req {req}: COD prog={} levels={}", e.codestream[i + 5], e.codestream[i + 9]);
        }
        Some("check") => {
            let png = args.next().expect("usage: tools check <in.png> <file.j2c>");
            let j2c = args.next().expect("usage: tools check <in.png> <file.j2c>");
            check_regions(&png, &j2c);
        }
        _ => eprintln!(
            "usage: tools bench <file> [runs] | verify <png> | encode <png> <j2c> | check <png> <j2c>"
        ),
    }
}

fn read_gray_png(path: &str) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut src = vec![0u8; reader.output_buffer_size().expect("png too large")];
    let info = reader.next_frame(&mut src).unwrap();
    assert!(
        info.color_type == png::ColorType::Grayscale && info.bit_depth == png::BitDepth::Eight,
        "only 8-bit grayscale PNGs are supported"
    );
    src.truncate(info.width as usize * info.height as usize);
    (info.width, info.height, src)
}

/// Lossless HTJ2K encode of a grayscale PNG in one pass (needs the vendored j2k-core with the
/// raised allocation cap). Tiles are 1024-ish with 5 wavelet levels; the output is a raw
/// codestream, or a `.jp2` file when `out_path` ends in `.jp2`.
fn encode_streaming(png_path: &str, out_path: &str) {
    use j2k::{
        encode_j2k_lossless, J2kBlockCodingMode, J2kEncodeValidation, J2kLosslessEncodeOptions,
        J2kLosslessSamples,
    };
    let t = Instant::now();
    let (w, h, src) = read_gray_png(png_path);
    // The j2k encoder limits the global level count to what the smallest tile supports, so a
    // sliver tile (< 32 px) at the right/bottom edge cuts 5 levels to 2. Pick a tile size with no
    // sliver instead of cropping the image.
    let tile = choose_tile(w, h);
    println!(
        "png {w}x{h} read in {:.1} s; tile {tile}x{tile}",
        t.elapsed().as_secs_f64()
    );

    let mut opts = J2kLosslessEncodeOptions::default();
    opts.block_coding_mode = J2kBlockCodingMode::HighThroughput;
    opts.tile_size = Some((tile, tile));
    opts.max_decomposition_levels = Some(5);
    // Validation would decode the whole image again; `tools check` verifies regions instead.
    opts.validation = J2kEncodeValidation::External;

    let t = Instant::now();
    let enc = encode_j2k_lossless(
        J2kLosslessSamples::new(&src, w, h, 1, 8, false).unwrap(),
        &opts,
    )
    .expect("encode");
    let cs = enc.codestream;
    let cod = cs.windows(2).position(|m| m == [0xFF, 0x52]).expect("COD");
    assert_eq!(cs[cod + 9], 5, "encoder reduced the wavelet levels");
    let out = if out_path.to_ascii_lowercase().ends_with(".jp2") {
        wrap_jp2(&cs, w, h)
    } else {
        cs
    };
    println!(
        "encoded {:.1} MB in {:.1} s",
        out.len() as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );
    std::fs::write(out_path, out).unwrap();
}
/// Smallest power-of-two tile edge (>= 1024) that leaves no sliver tile at the right or bottom
/// edge: each of `w` and `h` must divide evenly or leave a remainder of at least 64 px.
/// Powers of two only: the j2k encoder/decoder disagree on code-block boundaries for other sizes.
/// `J2K_TILE` overrides the choice (for experiments).
fn choose_tile(w: u32, h: u32) -> u32 {
    if let Some(t) = std::env::var("J2K_TILE").ok().and_then(|v| v.parse().ok()) {
        return t;
    }
    let ok = |n: u32, t: u32| n % t == 0 || n % t >= 64;
    [1024, 2048, 4096, 8192]
        .into_iter()
        .find(|&t| ok(w, t) && ok(h, t))
        .unwrap_or(1024)
}

/// Wrap a raw codestream in the JP2 file format, with the same box layout as your existing
/// `.jp2` files: signature, `ftyp` (jp2), `jp2h` {`ihdr`, `colr` greyscale}, `jp2c`.
fn wrap_jp2(codestream: &[u8], w: u32, h: u32) -> Vec<u8> {
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

/// Encode a synthetic image with openjph-core and decode with j2k: `tools interop W H TILE LEVELS`.
fn interop(w: u32, h: u32, tile: u32, levels: u32) {
    use openjph_core::codestream::Codestream;
    use openjph_core::file::MemOutfile;
    use openjph_core::types::{Point, Size};

    let mut s = 12345u32;
    let img: Vec<u8> = (0..w * h)
        .map(|i| {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            ((i % w) / 7 + (i / w) / 5 + (s >> 29)) as u8
        })
        .collect();
    let mut cs = Codestream::new();
    cs.access_siz_mut().set_image_extent(Point::new(w, h));
    cs.access_siz_mut().set_num_components(1);
    cs.access_siz_mut()
        .set_comp_info(0, Point::new(1, 1), 8, false);
    cs.access_siz_mut().set_tile_size(Size::new(tile, tile));
    cs.access_cod_mut().set_num_decomposition(levels);
    cs.access_cod_mut().set_reversible(true);
    cs.access_cod_mut().set_color_transform(false);
    cs.set_planar(0);
    let mut out = MemOutfile::new();
    cs.write_headers(&mut out, &[]).unwrap();
    let mut line = vec![0i32; w as usize];
    for y in 0..h as usize {
        for (d, s) in line
            .iter_mut()
            .zip(&img[y * w as usize..(y + 1) * w as usize])
        {
            *d = i32::from(*s);
        }
        cs.exchange(&line, 0).unwrap();
    }
    cs.flush(&mut out).unwrap();
    let data = out.get_data().to_vec();

    let mut dec = J2kDecoder::new(&data).expect("j2k parse");
    let mut buf = vec![0u8; (w * h) as usize];
    let mut pool = J2kScratchPool::new();
    let roi = Rect { x: 0, y: 0, w, h };
    match dec.decode_region_into(&mut pool, &mut buf, w as usize, PixelFormat::Gray8, roi) {
        Ok(_) => {
            let bad = img.iter().zip(&buf).filter(|(a, b)| a != b).count();
            println!(
                "{w}x{h} tile {tile} levels {levels}: {} B, mismatches {bad}",
                data.len()
            );
        }
        Err(e) => println!("{w}x{h} tile {tile} levels {levels}: j2k decode error: {e}"),
    }
}

/// Decode random and edge regions from a codestream and compare against the PNG source.
fn check_regions(png_path: &str, j2c_path: &str) {
    let (stride, _png_h, src) = read_gray_png(png_path);
    let bytes = std::fs::read(j2c_path).unwrap();
    // Unwrap a JP2 container ourselves (the j2k crate rejects some HTJ2K-in-JP2 files).
    let bytes = &bytes[decoder::codestream_offset(&bytes).expect("codestream")..];
    let t = Instant::now();
    let mut dec = J2kDecoder::new(bytes).expect("parse");
    println!(
        "parse {:.1} ms, dims {:?}",
        t.elapsed().as_secs_f64() * 1e3,
        dec.info().dimensions
    );
    // The codestream may be a left/top crop of the PNG; `stride` indexes the PNG rows.
    let (w, h) = dec.info().dimensions;
    let mut pool = J2kScratchPool::new();
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut rnd = |m: u32| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % m as u64) as u32
    };
    let mut regions = vec![(0, 0, 64, 64), (w - 64, h - 64, 64, 64), (1023, 1023, 5, 5)];
    for _ in 0..40 {
        let (rw, rh) = (1 + rnd(2500), 1 + rnd(1600));
        regions.push((rnd(w - rw), rnd(h - rh), rw, rh));
    }
    let (mut bad, mut ms_total) = (0u64, 0.0);
    let mut buf = Vec::new();
    for (x, y, rw, rh) in regions {
        let roi = Rect { x, y, w: rw, h: rh };
        buf.resize((rw * rh) as usize, 0);
        let t = Instant::now();
        dec.decode_region_into(&mut pool, &mut buf, rw as usize, PixelFormat::Gray8, roi)
            .expect("region");
        ms_total += t.elapsed().as_secs_f64() * 1e3;
        for r in 0..rh {
            let s = ((y + r) as usize) * stride as usize + x as usize;
            let d = (r * rw) as usize;
            bad += src[s..s + rw as usize]
                .iter()
                .zip(&buf[d..d + rw as usize])
                .filter(|(a, b)| a != b)
                .count() as u64;
        }
    }
    println!(
        "43 regions, avg {:.1} ms each, mismatched pixels: {bad}",
        ms_total / 43.0
    );
    println!("{}", if bad == 0 { "PASS: bit-exact" } else { "FAIL" });
}

/// Encode a grayscale PNG to lossless tiled HTJ2K, decode it by region, and
/// require every pixel to match.
fn verify_png(path: &str) {
    use j2k::{
        encode_j2k_lossless, J2kBlockCodingMode, J2kLosslessEncodeOptions, J2kLosslessSamples,
    };

    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut src = vec![0u8; reader.output_buffer_size().expect("png too large")];
    let info = reader.next_frame(&mut src).unwrap();
    println!(
        "{path}: {}x{} {:?} {:?}",
        info.width, info.height, info.color_type, info.bit_depth
    );
    assert!(
        info.color_type == png::ColorType::Grayscale && info.bit_depth == png::BitDepth::Eight,
        "only 8-bit grayscale PNGs are supported"
    );
    // Verify on a crop around the middle (default 16384x10240, like the real stitched files);
    // the encoder caps allocations at 512 MB, so the 1.85 GP source can't be encoded whole.
    let (cw, ch) = (16384.min(info.width), 10240.min(info.height));
    let (ox, oy) = ((info.width - cw) / 2, (info.height - ch) / 2);
    let mut crop = Vec::with_capacity((cw * ch) as usize);
    for r in 0..ch {
        let s = ((oy + r) as usize) * info.width as usize + ox as usize;
        crop.extend_from_slice(&src[s..s + cw as usize]);
    }
    drop(src);
    let src = crop;
    let (w, h) = (cw, ch);
    println!(
        "crop {w}x{h} at ({ox},{oy}), non-zero fraction {:.3}",
        src.iter().filter(|&&p| p != 0).count() as f64 / src.len() as f64
    );

    let mut opts = J2kLosslessEncodeOptions::default();
    opts.block_coding_mode = J2kBlockCodingMode::HighThroughput;
    opts.tile_size = Some((1024, 1024));
    // The built-in check does a whole-image decode, which exceeds the crate's 512 MB cap.
    opts.validation = j2k::J2kEncodeValidation::External;
    let t = Instant::now();
    let enc = encode_j2k_lossless(
        J2kLosslessSamples::new(&src, w, h, 1, 8, false).unwrap(),
        &opts,
    )
    .expect("encode");
    println!(
        "encoded {:.1} MB in {:.1} s",
        enc.codestream.len() as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );

    let mut dec = J2kDecoder::new(&enc.codestream).expect("parse encoded");
    let mut pool = J2kScratchPool::new();
    let t = Instant::now();
    let mut mismatches = 0u64;
    let mut buf = Vec::new();
    // Decode in 2048-row strips (tile aligned) to stay under the allocation cap.
    let strip = 2048u32;
    let mut y = 0;
    while y < h {
        let roi = Rect {
            x: 0,
            y,
            w,
            h: strip.min(h - y),
        };
        buf.resize((roi.w * roi.h) as usize, 0);
        dec.decode_region_into(&mut pool, &mut buf, roi.w as usize, PixelFormat::Gray8, roi)
            .expect("decode strip");
        let a = &src[(y * w) as usize..((y + roi.h) * w) as usize];
        mismatches += a.iter().zip(&buf).filter(|(p, q)| p != q).count() as u64;
        y += strip;
    }
    println!(
        "strip decode {:.1} s, mismatched pixels: {mismatches}",
        t.elapsed().as_secs_f64()
    );

    // Unaligned small regions (what the viewer will request).
    let mut region_bad = 0u64;
    for (x, y, rw, rh) in [
        (0, 0, 1, 1),
        (1023, 1023, 3, 3),
        (w / 3 + 7, h / 3 + 5, 1920.min(w), 1080.min(h)),
        (w - 100, h - 100, 100, 100),
    ] {
        let roi = Rect {
            x,
            y,
            w: rw.min(w - x),
            h: rh.min(h - y),
        };
        buf.resize((roi.w * roi.h) as usize, 0);
        dec.decode_region_into(&mut pool, &mut buf, roi.w as usize, PixelFormat::Gray8, roi)
            .expect("decode region");
        for r in 0..roi.h {
            let s = ((roi.y + r) * w + roi.x) as usize;
            let d = (r * roi.w) as usize;
            region_bad += src[s..s + roi.w as usize]
                .iter()
                .zip(&buf[d..d + roi.w as usize])
                .filter(|(p, q)| p != q)
                .count() as u64;
        }
    }
    println!("unaligned region mismatches: {region_bad}");
    println!(
        "{}",
        if mismatches == 0 && region_bad == 0 {
            "PASS: bit-exact"
        } else {
            "FAIL"
        }
    );
}

fn time<T, E: std::fmt::Display>(label: &str, runs: usize, mut f: impl FnMut() -> Result<T, E>) {
    let mut best = f64::MAX;
    let mut all = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        if let Err(e) = std::hint::black_box(f()) {
            println!("{label:<44} FAILED: {e}");
            return;
        }
        let ms = t.elapsed().as_secs_f64() * 1e3;
        all.push(ms);
        best = best.min(ms);
    }
    println!("{label:<44} best {best:8.1} ms   runs {all:.1?}");
}

fn bench_j2k(path: &str, runs: usize) {
    let mut bytes = std::fs::read(path).expect("read");
    if J2kDecoder::new(&bytes).is_err() {
        // Retry on the bare codestream (SOC+SIZ marker) in case container detection rejects it.
        if let Some(i) = bytes.windows(4).position(|w| w == [0xFF, 0x4F, 0xFF, 0x51]) {
            println!("container rejected; retrying raw codestream at offset {i}");
            bytes.drain(..i);
        }
    }
    println!(
        "{path}\n{:.1} MB, cpus={}",
        bytes.len() as f64 / 1e6,
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    );
    let mut dec = J2kDecoder::new(&bytes).expect("parse");
    let (w, h) = dec.info().dimensions;
    println!("{w}x{h}");
    let mut pool = J2kScratchPool::new();

    // Full-resolution full decode (skipped for huge images: the crate caps this at 512 MB).
    if u64::from(w) * u64::from(h) <= 300_000_000 {
        let mut full = vec![0u8; w as usize * h as usize];
        time("full decode (level 0)", runs, || {
            dec.decode_into(&mut full, w as usize, PixelFormat::Gray8)
        });
    } else {
        println!("full decode (level 0): skipped (> 300 MP)");
    }

    // Viewport-sized region at full res (zoomed in 1:1), centre of the image.
    let roi = Rect {
        x: w / 3,
        y: h / 3,
        w: 1920.min(w),
        h: 1080.min(h),
    };
    let mut region = vec![0u8; roi.w as usize * roi.h as usize];
    time("region 1920x1080 @ level 0", runs, || {
        dec.decode_region_into(
            &mut pool,
            &mut region,
            roi.w as usize,
            PixelFormat::Gray8,
            roi,
        )
    });

    // Reduced-resolution full images (fit-to-screen views).
    for levels in 1..=6u8 {
        let d = 1u32 << levels;
        let (rw, rh) = (w.div_ceil(d), h.div_ceil(d));
        let mut out = vec![0u8; rw as usize * rh as usize];
        let full_roi = Rect { x: 0, y: 0, w, h };
        time(
            &format!("whole image @ level {levels} ({rw}x{rh})"),
            runs,
            || {
                dec.decode_region_scaled_pow2_into(
                    &mut pool,
                    &mut out,
                    rw as usize,
                    PixelFormat::Gray8,
                    full_roi,
                    levels,
                )
            },
        );
    }

    // Viewport region at reduced levels (zoomed-out partial view), centred.
    for levels in 1..=5u8 {
        let d = 1u32 << levels;
        let rw_full = ((1920 * d).min(w)) & !(d - 1);
        let rh_full = ((1080 * d).min(h)) & !(d - 1);
        let roi = Rect {
            x: ((w - rw_full) / 2) & !(d - 1),
            y: ((h - rh_full) / 2) & !(d - 1),
            w: rw_full,
            h: rh_full,
        };
        let (rw, rh) = (roi.w.div_ceil(d), roi.h.div_ceil(d));
        let mut out = vec![0u8; rw as usize * rh as usize];
        time(
            &format!("viewport region @ level {levels} ({rw}x{rh})"),
            runs,
            || {
                dec.decode_region_scaled_pow2_into(
                    &mut pool,
                    &mut out,
                    rw as usize,
                    PixelFormat::Gray8,
                    roi,
                    levels,
                )
            },
        );
    }
}
