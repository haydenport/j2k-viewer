//! Camera and tile-pyramid geometry. Pure functions, no GPU or threads.
//!
//! Coordinates: "image px" are full-resolution pixels; "screen px" are physical window pixels.
//! Pyramid level `L` has pixels that are `2^L` image px wide. Level `L` is cut into
//! `TILE x TILE` texel tiles, so a tile covers `TILE << L` image px per side.

use decoder::{Info, Rect};

/// Edge length of a display tile, in texels of its own level.
pub const TILE: u32 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub level: u8,
    pub tx: u32,
    pub ty: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Image point shown at the centre of the viewport.
    pub cx: f64,
    pub cy: f64,
    /// Screen px per image px.
    pub scale: f64,
    pub vw: f64,
    pub vh: f64,
}

pub const MAX_SCALE: f64 = 64.0;

impl Camera {
    pub fn fit(img_w: u32, img_h: u32, vw: f64, vh: f64) -> Self {
        let scale = (vw / f64::from(img_w)).min(vh / f64::from(img_h)).min(1.0);
        Self {
            cx: f64::from(img_w) / 2.0,
            cy: f64::from(img_h) / 2.0,
            scale,
            vw,
            vh,
        }
    }

    pub fn min_scale(img_w: u32, img_h: u32, vw: f64, vh: f64) -> f64 {
        (vw / f64::from(img_w)).min(vh / f64::from(img_h)) * 0.5
    }

    pub fn image_to_screen(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.cx) * self.scale + self.vw / 2.0,
            (y - self.cy) * self.scale + self.vh / 2.0,
        )
    }

    pub fn screen_to_image(&self, sx: f64, sy: f64) -> (f64, f64) {
        (
            (sx - self.vw / 2.0) / self.scale + self.cx,
            (sy - self.vh / 2.0) / self.scale + self.cy,
        )
    }

    /// Zoom by `factor` keeping the image point under screen position `(sx, sy)` fixed.
    pub fn zoom_at(&mut self, sx: f64, sy: f64, factor: f64, min_scale: f64) {
        let (ix, iy) = self.screen_to_image(sx, sy);
        self.scale = (self.scale * factor).clamp(min_scale, MAX_SCALE);
        self.cx = ix - (sx - self.vw / 2.0) / self.scale;
        self.cy = iy - (sy - self.vh / 2.0) / self.scale;
    }

    /// Drag the image by `(dx, dy)` screen px.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.cx -= dx / self.scale;
        self.cy -= dy / self.scale;
    }

    /// Keep the centre inside the image so it can't be lost off-screen.
    pub fn clamp_centre(&mut self, img_w: u32, img_h: u32) {
        self.cx = self.cx.clamp(0.0, f64::from(img_w));
        self.cy = self.cy.clamp(0.0, f64::from(img_h));
    }

    /// Visible image rectangle `(x0, y0, x1, y1)` in image px (unclipped).
    pub fn visible(&self) -> (f64, f64, f64, f64) {
        let (x0, y0) = self.screen_to_image(0.0, 0.0);
        let (x1, y1) = self.screen_to_image(self.vw, self.vh);
        (x0, y0, x1, y1)
    }
}

/// Coarsest level whose texels are no larger than a screen pixel: `floor(log2(1 / scale))`.
pub fn level_for_scale(scale: f64, max_level: u8) -> u8 {
    if scale >= 1.0 {
        return 0;
    }
    ((1.0 / scale).log2().floor() as i64).clamp(0, i64::from(max_level)) as u8
}

/// Size of pyramid level `level` in texels.
pub fn level_dims(w: u32, h: u32, level: u8) -> (u32, u32) {
    (w.div_ceil(1 << level), h.div_ceil(1 << level))
}

/// Number of tiles across and down at `level`.
pub fn tile_grid(info: &Info, level: u8) -> (u32, u32) {
    let (lw, lh) = level_dims(info.width, info.height, level);
    (lw.div_ceil(TILE), lh.div_ceil(TILE))
}

/// Full-resolution region to decode for `key`, clipped to the image. Origin is always a
/// multiple of `1 << level`, as the decoder requires.
pub fn tile_region(info: &Info, key: TileKey) -> Option<Rect> {
    let (gx, gy) = tile_grid(info, key.level);
    if key.tx >= gx || key.ty >= gy {
        return None;
    }
    let span = u64::from(TILE) << key.level;
    let (x, y) = (u64::from(key.tx) * span, u64::from(key.ty) * span);
    let w = span.min(u64::from(info.width) - x) as u32;
    let h = span.min(u64::from(info.height) - y) as u32;
    Some(Rect {
        x: x as u32,
        y: y as u32,
        w,
        h,
    })
}

/// Image-px rectangle `(x0, y0, x1, y1)` that a decoded tile of `tex_w x tex_h` texels covers.
pub fn tile_image_rect(key: TileKey, tex_w: u32, tex_h: u32) -> (f64, f64, f64, f64) {
    let span = f64::from(TILE << key.level);
    let (x0, y0) = (f64::from(key.tx) * span, f64::from(key.ty) * span);
    let texel = f64::from(1u32 << key.level);
    (
        x0,
        y0,
        x0 + f64::from(tex_w) * texel,
        y0 + f64::from(tex_h) * texel,
    )
}

/// Tiles of `level` intersecting the camera's view (grown by `margin` tiles), nearest to the
/// view centre first.
pub fn visible_tiles(cam: &Camera, info: &Info, level: u8, margin: u32) -> Vec<TileKey> {
    let (gx, gy) = tile_grid(info, level);
    let span = f64::from(TILE << level);
    let (vx0, vy0, vx1, vy1) = cam.visible();
    let m = i64::from(margin);
    let lo = |v: f64| ((v / span).floor() as i64 - m).max(0);
    let hi = |v: f64, grid: u32| ((v / span).floor() as i64 + m).min(i64::from(grid) - 1);
    let mut out = Vec::new();
    // Inclusive ranges are empty when start > end, which covers views entirely off the image.
    for ty in lo(vy0)..=hi(vy1, gy) {
        for tx in lo(vx0)..=hi(vx1, gx) {
            out.push(TileKey {
                level,
                tx: tx as u32,
                ty: ty as u32,
            });
        }
    }
    let dist = |k: &TileKey| {
        let (cx, cy) = (
            (f64::from(k.tx) + 0.5) * span,
            (f64::from(k.ty) + 0.5) * span,
        );
        (cx - cam.cx).powi(2) + (cy - cam.cy).powi(2)
    };
    out.sort_by(|a, b| dist(a).total_cmp(&dist(b)));
    out
}

/// Every tile of `level`, nearest to the image centre first.
pub fn all_tiles(info: &Info, level: u8) -> Vec<TileKey> {
    let (gx, gy) = tile_grid(info, level);
    let span = f64::from(TILE << level);
    let (cx, cy) = (f64::from(info.width) / 2.0, f64::from(info.height) / 2.0);
    let mut out: Vec<TileKey> = (0..gy)
        .flat_map(|ty| (0..gx).map(move |tx| TileKey { level, tx, ty }))
        .collect();
    out.sort_by(|a, b| {
        let d = |k: &TileKey| {
            ((f64::from(k.tx) + 0.5) * span - cx).powi(2)
                + ((f64::from(k.ty) + 0.5) * span - cy).powi(2)
        };
        d(a).total_cmp(&d(b))
    });
    out
}

/// Pyramid level that always stays resident as a fallback: the first level small enough that the
/// whole image fits in a few tiles (longest side <= 2048 texels), or the deepest available.
pub fn overview_level(info: &Info) -> u8 {
    let longest = info.width.max(info.height);
    (0..=info.levels)
        .find(|&l| longest.div_ceil(1 << l) <= 2048)
        .unwrap_or(info.levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(w: u32, h: u32, levels: u8) -> Info {
        Info {
            width: w,
            height: h,
            tile_width: 1024,
            tile_height: 1024,
            levels,
        }
    }

    #[test]
    fn level_selection() {
        assert_eq!(level_for_scale(8.0, 5), 0);
        assert_eq!(level_for_scale(1.0, 5), 0);
        assert_eq!(level_for_scale(0.9, 5), 0);
        assert_eq!(level_for_scale(0.5, 5), 1);
        assert_eq!(level_for_scale(0.26, 5), 1);
        assert_eq!(level_for_scale(0.25, 5), 2);
        assert_eq!(level_for_scale(0.04, 5), 4);
        assert_eq!(level_for_scale(0.001, 5), 5); // clamped to the deepest level
    }

    #[test]
    fn zoom_keeps_cursor_point_fixed() {
        let mut c = Camera {
            cx: 1000.0,
            cy: 700.0,
            scale: 0.5,
            vw: 800.0,
            vh: 600.0,
        };
        let before = c.screen_to_image(123.0, 456.0);
        c.zoom_at(123.0, 456.0, 3.0, 0.001);
        let after = c.screen_to_image(123.0, 456.0);
        assert!((before.0 - after.0).abs() < 1e-9 && (before.1 - after.1).abs() < 1e-9);
        assert!((c.scale - 1.5).abs() < 1e-12);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut c = Camera {
            cx: 0.0,
            cy: 0.0,
            scale: 1.0,
            vw: 100.0,
            vh: 100.0,
        };
        c.zoom_at(50.0, 50.0, 1e9, 0.01);
        assert_eq!(c.scale, MAX_SCALE);
        c.zoom_at(50.0, 50.0, 1e-12, 0.01);
        assert_eq!(c.scale, 0.01);
    }

    #[test]
    fn pan_moves_image_with_cursor() {
        let mut c = Camera {
            cx: 100.0,
            cy: 100.0,
            scale: 2.0,
            vw: 100.0,
            vh: 100.0,
        };
        let p = c.image_to_screen(120.0, 90.0);
        c.pan(10.0, -4.0);
        let q = c.image_to_screen(120.0, 90.0);
        assert!((q.0 - p.0 - 10.0).abs() < 1e-9 && (q.1 - p.1 + 4.0).abs() < 1e-9);
    }

    #[test]
    fn fit_shows_whole_image_and_never_upscales() {
        let c = Camera::fit(47104, 39377, 1920.0, 1080.0);
        assert!((c.scale - 1080.0 / 39377.0).abs() < 1e-12);
        let small = Camera::fit(100, 100, 1920.0, 1080.0);
        assert_eq!(small.scale, 1.0);
    }

    #[test]
    fn tile_regions_are_aligned_and_clipped() {
        let i = info(47104, 39377, 5);
        let r = tile_region(
            &i,
            TileKey {
                level: 0,
                tx: 1,
                ty: 2,
            },
        )
        .unwrap();
        assert_eq!(
            r,
            Rect {
                x: 512,
                y: 1024,
                w: 512,
                h: 512
            }
        );
        // Level 3 tile spans 4096 image px and its origin is a multiple of 8.
        let r = tile_region(
            &i,
            TileKey {
                level: 3,
                tx: 2,
                ty: 1,
            },
        )
        .unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (8192, 4096, 4096, 4096));
        // Bottom edge is clipped: 39377 - 9*4096 = 2513.
        let (_, gy) = tile_grid(&i, 3);
        let last = tile_region(
            &i,
            TileKey {
                level: 3,
                tx: 0,
                ty: gy - 1,
            },
        )
        .unwrap();
        assert_eq!(last.y + last.h, 39377);
        assert!(last.h < 4096);
        assert!(tile_region(
            &i,
            TileKey {
                level: 3,
                tx: 0,
                ty: gy
            }
        )
        .is_none());
        for k in [
            TileKey {
                level: 5,
                tx: 1,
                ty: 1,
            },
            TileKey {
                level: 2,
                tx: 7,
                ty: 3,
            },
        ] {
            let r = tile_region(&i, k).unwrap();
            assert_eq!(r.x % (1 << k.level), 0);
            assert_eq!(r.y % (1 << k.level), 0);
        }
    }

    #[test]
    fn tiles_cover_the_image_exactly_once() {
        let i = info(3000, 1700, 3);
        for level in 0..=3u8 {
            let (gx, gy) = tile_grid(&i, level);
            let mut area = 0u64;
            for ty in 0..gy {
                for tx in 0..gx {
                    let r = tile_region(&i, TileKey { level, tx, ty }).unwrap();
                    area += u64::from(r.w) * u64::from(r.h);
                }
            }
            assert_eq!(area, 3000 * 1700, "level {level}");
        }
    }

    #[test]
    fn visible_tiles_cover_view_and_sort_by_distance() {
        let i = info(47104, 39377, 5);
        let cam = Camera {
            cx: 20000.0,
            cy: 15000.0,
            scale: 1.0,
            vw: 1920.0,
            vh: 1080.0,
        };
        let t = visible_tiles(&cam, &i, 0, 0);
        // x: 19040..20960 -> tiles 37..40 (4); y: 14460..15540 -> tiles 28..30 (3)
        assert_eq!(t.len(), 4 * 3);
        assert!(t
            .iter()
            .all(|k| (37..=40).contains(&k.tx) && (28..=30).contains(&k.ty)));
        let d = |k: &TileKey| {
            ((f64::from(k.tx) + 0.5) * 512.0 - 20000.0).powi(2)
                + ((f64::from(k.ty) + 0.5) * 512.0 - 15000.0).powi(2)
        };
        assert!(t.windows(2).all(|w| d(&w[0]) <= d(&w[1])));
        // A one-tile margin grows the set.
        assert_eq!(visible_tiles(&cam, &i, 0, 1).len(), 6 * 5);
    }

    #[test]
    fn visible_tiles_clip_to_image_and_handle_offscreen() {
        let i = info(1000, 1000, 2);
        let cam = Camera {
            cx: 500.0,
            cy: 500.0,
            scale: 0.25,
            vw: 4000.0,
            vh: 4000.0,
        };
        // View much larger than the image: level 2 is 250x250 => a single tile.
        assert_eq!(visible_tiles(&cam, &i, 2, 3).len(), 1);
        let away = Camera {
            cx: -50000.0,
            cy: -50000.0,
            scale: 1.0,
            vw: 100.0,
            vh: 100.0,
        };
        assert!(visible_tiles(&away, &i, 0, 0).is_empty());
        let far = Camera {
            cx: 90000.0,
            cy: 90000.0,
            scale: 1.0,
            vw: 100.0,
            vh: 100.0,
        };
        assert!(visible_tiles(&far, &i, 0, 0).is_empty());
    }

    #[test]
    fn overview_is_small_but_available() {
        assert_eq!(overview_level(&info(47104, 39377, 5)), 5); // 1472 px
        assert_eq!(overview_level(&info(16237, 10483, 6)), 3); // 2030 px
        assert_eq!(overview_level(&info(1500, 900, 5)), 0);
        assert_eq!(overview_level(&info(200000, 1000, 3)), 3); // deepest available
    }

    #[test]
    fn tile_rect_matches_region_for_full_tiles() {
        let i = info(47104, 39377, 5);
        let k = TileKey {
            level: 2,
            tx: 3,
            ty: 4,
        };
        let r = tile_region(&i, k).unwrap();
        let (x0, y0, x1, y1) = tile_image_rect(k, TILE, TILE);
        assert_eq!(
            (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32),
            (r.x, r.y, r.w, r.h)
        );
    }
}
