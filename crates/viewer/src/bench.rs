//! Headless benchmark of the tile cache and layer prefetch: `viewer --bench <folder>`.
//!
//! Three scenarios on the real pipeline (decode workers, GPU upload, eviction):
//!   A. stepping through layers, with prefetch off and on;
//!   B. panning at a fixed speed, counting frames that would show stand-in tiles;
//!   C. a zoom sweep with a small cache budget, checking that eviction keeps memory bounded.

use std::time::{Duration, Instant};

use crate::viewer::Viewer;

pub struct Opts {
    pub steps: usize,
    pub zoom: f64,
    pub cache_mb: u64,
    /// Horizontal pan per 60 Hz frame in screen px (vertical is 9/16 of it).
    pub pan: f64,
}

fn pump(v: &mut Viewer, done: impl Fn(&Viewer) -> bool, timeout: Duration) -> Option<f64> {
    let t = Instant::now();
    loop {
        v.update();
        v.simulate_frame();
        if done(v) {
            return Some(t.elapsed().as_secs_f64() * 1e3);
        }
        if t.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

fn dwell(v: &mut Viewer, d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d {
        v.update();
        v.simulate_frame();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn stats(mut xs: Vec<f64>) -> String {
    if xs.is_empty() {
        return "n/a".into();
    }
    xs.sort_by(f64::total_cmp);
    let avg = xs.iter().sum::<f64>() / xs.len() as f64;
    let p = |q: f64| xs[((xs.len() - 1) as f64 * q).round() as usize];
    format!(
        "avg {avg:6.1}  median {:6.1}  p95 {:6.1}  max {:6.1} ms",
        p(0.5),
        p(0.95),
        xs[xs.len() - 1]
    )
}

fn centre_view(v: &mut Viewer, scale: f64) {
    if let Some(i) = v.current_info() {
        v.set_view(
            Some(f64::from(i.width) / 2.0),
            Some(f64::from(i.height) / 2.0),
            Some(scale),
        );
    }
}

pub fn run(mk: &dyn Fn(usize) -> Viewer, o: &Opts) {
    let timeout = Duration::from_secs(120);

    println!(
        "== A. layer stepping (zoom {:.0}%, 1.5 s dwell on each layer before stepping) ==",
        o.zoom * 100.0
    );
    for prefetch in [0usize, 1] {
        let mut v = mk(prefetch);
        centre_view(&mut v, o.zoom);
        let cold = pump(&mut v, Viewer::current_ready, timeout);
        let mut lat = Vec::new();
        for _ in 0..o.steps.min(v.file_count().saturating_sub(1)) {
            dwell(&mut v, Duration::from_millis(1500));
            let t = Instant::now();
            v.step(1);
            if pump(&mut v, Viewer::current_ready, timeout).is_some() {
                lat.push(t.elapsed().as_secs_f64() * 1e3);
            }
        }
        println!(
            "prefetch {prefetch}: first layer ready in {:>6.1} ms | step latency: {}",
            cold.unwrap_or(f64::NAN),
            stats(lat)
        );
    }

    println!(
        "\n== B. panning at {:.0}% zoom, {:.0}x{:.0} px per 60 Hz frame ({:.0} px/s), 180 frames ==",
        o.zoom.max(1.0) * 100.0,
        o.pan,
        o.pan * 0.5625,
        o.pan * 60.0
    );
    for prefetch in [0usize, 1] {
        let mut v = mk(prefetch);
        centre_view(&mut v, o.zoom.max(1.0));
        let _ = pump(&mut v, Viewer::current_ready, timeout);
        let frame = Duration::from_micros(16_667);
        let (mut stalled, mut run, mut longest) = (0, 0, 0);
        let start = Instant::now();
        for n in 0..180u32 {
            let deadline = start + frame * (n + 1);
            v.pan(-o.pan, -o.pan * 0.5625);
            v.update();
            v.simulate_frame();
            if v.current_ready() {
                run = 0;
            } else {
                stalled += 1;
                run += 1;
                longest = longest.max(run);
            }
            while Instant::now() < deadline {
                v.update();
                v.simulate_frame();
                std::thread::sleep(Duration::from_micros(300));
            }
        }
        let after = pump(&mut v, Viewer::current_ready, timeout);
        println!(
            "prefetch {prefetch}: {stalled}/180 frames missing tiles ({:.0}%), longest run {longest} frames, ready {:.0} ms after the pan stops",
            f64::from(stalled) / 1.8,
            after.unwrap_or(f64::NAN)
        );
    }

    println!(
        "\n== C. zoom sweep with a {} MB cache budget ==",
        o.cache_mb
    );
    let mut v = mk(1);
    v.set_cache_budget(o.cache_mb << 20);
    centre_view(&mut v, 0.05);
    let mut peak = 0u64;
    let sweep = [
        0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 4.0, 1.0, 0.25, 0.05, 1.0,
    ];
    for s in sweep {
        v.set_view(None, None, Some(s));
        let ms = pump(&mut v, Viewer::current_ready, timeout);
        let _ = pump(&mut v, Viewer::is_settled, timeout);
        v.update();
        peak = peak.max(v.cache_bytes());
        println!(
            "zoom {:>6.1}%  level {}  ready {:>7.1} ms  cache {:>6.1} MB  evicted {}",
            s * 100.0,
            v.current_level(),
            ms.unwrap_or(f64::NAN),
            v.cache_bytes() as f64 / 1e6,
            v.stats.evicted
        );
    }
    println!(
        "peak cache {:.1} MB vs budget {} MB (overview tiles are pinned and may exceed it slightly); {} tiles decoded, avg {:.1} ms each",
        peak as f64 / 1e6,
        o.cache_mb,
        v.stats.decoded,
        v.stats.decode_ms_sum / v.stats.decoded.max(1) as f64
    );
}
