//! The canvas: one RGBA8 image in tiles of [`TILE`] pixels, each an `Arc`.
//!
//! Cloning a canvas clones the tile table, not the pixels, so an undo
//! snapshot costs a pointer per tile; a tile is copied the first time an
//! edit after the snapshot writes to it (`Arc::make_mut`). Which tiles an
//! edit changed is then a pointer comparison ([`Canvas::changed_tiles`]),
//! which is what the encoder (a tile no stroke touched keeps its stored
//! blocks) and the live preview (the touched rectangle only) read.

use std::sync::Arc;

use crate::error::Error;
use crate::rect::Rect;

/// A tile's side, in pixels (edge tiles are smaller).
pub const TILE: u32 = 256;

/// Largest width or height.
pub const MAX_SIZE: u32 = 8192;

/// Checks a canvas size.
pub fn check_size(width: u32, height: u32) -> Result<(), Error> {
    if width == 0 || height == 0 {
        return Err(Error::Size(format!("{width}x{height} has no pixels")));
    }
    if width > MAX_SIZE || height > MAX_SIZE {
        return Err(Error::Size(format!(
            "{width}x{height} is over the {MAX_SIZE} pixel limit"
        )));
    }
    Ok(())
}

/// A tiled RGBA8 image (straight alpha, sRGB-encoded colour).
#[derive(Clone, Debug)]
pub struct Canvas {
    width: u32,
    height: u32,
    cols: u32,
    tiles: Vec<Arc<Vec<u8>>>,
}

impl Canvas {
    /// A canvas of one colour.
    pub fn new(width: u32, height: u32, fill: [u8; 4]) -> Result<Canvas, Error> {
        check_size(width, height)?;
        let cols = width.div_ceil(TILE);
        let rows = height.div_ceil(TILE);
        let mut tiles = Vec::with_capacity((cols * rows) as usize);
        for ty in 0..rows {
            for tx in 0..cols {
                let tw = (width - tx * TILE).min(TILE);
                let th = (height - ty * TILE).min(TILE);
                let mut v = Vec::with_capacity((tw * th * 4) as usize);
                for _ in 0..tw * th {
                    v.extend_from_slice(&fill);
                }
                tiles.push(Arc::new(v));
            }
        }
        Ok(Canvas {
            width,
            height,
            cols,
            tiles,
        })
    }

    /// A canvas of `width * height * 4` bytes, row-major `R,G,B,A`.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Canvas, Error> {
        check_size(width, height)?;
        let need = width as usize * height as usize * 4;
        if rgba.len() != need {
            return Err(Error::Rect(format!(
                "{width}x{height} needs {need} bytes, got {}",
                rgba.len()
            )));
        }
        let mut c = Canvas::new(width, height, [0; 4])?;
        c.write_rect(c.bounds(), rgba)?;
        Ok(c)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The whole canvas.
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Tiles per row.
    pub fn tile_cols(&self) -> u32 {
        self.cols
    }

    /// Tile rows.
    pub fn tile_rows(&self) -> u32 {
        self.height.div_ceil(TILE)
    }

    fn index(&self, tx: u32, ty: u32) -> usize {
        (ty * self.cols + tx) as usize
    }

    /// The pixels a tile covers.
    pub fn tile_rect(&self, tx: u32, ty: u32) -> Rect {
        let x = tx * TILE;
        let y = ty * TILE;
        Rect::new(
            x,
            y,
            self.width.saturating_sub(x).min(TILE),
            self.height.saturating_sub(y).min(TILE),
        )
    }

    /// The tiles `r` (clipped to the canvas) overlaps, as `(tx, ty)`.
    pub fn tiles_in(&self, r: Rect) -> Vec<(u32, u32)> {
        let r = r.intersect(self.bounds());
        if r.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for ty in r.y / TILE..=(r.bottom() - 1) / TILE {
            for tx in r.x / TILE..=(r.right() - 1) / TILE {
                out.push((tx, ty));
            }
        }
        out
    }

    /// The pixel at (`x`, `y`); transparent black outside.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let (tx, ty) = (x / TILE, y / TILE);
        let tw = self.tile_rect(tx, ty).w;
        let o = (((y % TILE) * tw + (x % TILE)) * 4) as usize;
        let t = &self.tiles[self.index(tx, ty)];
        [t[o], t[o + 1], t[o + 2], t[o + 3]]
    }

    /// Writes one pixel (ignored outside the canvas).
    pub fn set_pixel(&mut self, x: u32, y: u32, px: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let (tx, ty) = (x / TILE, y / TILE);
        let tw = self.tile_rect(tx, ty).w;
        let o = (((y % TILE) * tw + (x % TILE)) * 4) as usize;
        let i = self.index(tx, ty);
        let t = Arc::make_mut(&mut self.tiles[i]);
        t[o..o + 4].copy_from_slice(&px);
    }

    /// `r` clipped to the canvas and its pixels, row-major.
    pub fn read_rect(&self, r: Rect) -> (Rect, Vec<u8>) {
        let r = r.intersect(self.bounds());
        let mut out = Vec::with_capacity(r.w as usize * r.h as usize * 4);
        for y in r.y..r.bottom() {
            let mut x = r.x;
            while x < r.right() {
                let (tx, ty) = (x / TILE, y / TILE);
                let tile = self.tile_rect(tx, ty);
                let end = ((tx + 1) * TILE).min(r.right());
                let n = (end - x) as usize * 4;
                let o = (((y % TILE) * tile.w + (x % TILE)) * 4) as usize;
                out.extend_from_slice(&self.tiles[self.index(tx, ty)][o..o + n]);
                x = end;
            }
        }
        (r, out)
    }

    /// Every pixel, row-major.
    pub fn to_rgba(&self) -> Vec<u8> {
        self.read_rect(self.bounds()).1
    }

    /// Writes `rgba` (`r.w * r.h * 4` bytes) into `r`, which must lie
    /// inside the canvas.
    pub fn write_rect(&mut self, r: Rect, rgba: &[u8]) -> Result<(), Error> {
        if !self.bounds().encloses(r) {
            return Err(Error::Rect(format!(
                "{}x{} at {},{} is outside the {}x{} canvas",
                r.w, r.h, r.x, r.y, self.width, self.height
            )));
        }
        let need = r.w as usize * r.h as usize * 4;
        if rgba.len() != need {
            return Err(Error::Rect(format!(
                "{}x{} needs {need} bytes, got {}",
                r.w,
                r.h,
                rgba.len()
            )));
        }
        let mut src = 0usize;
        for y in r.y..r.bottom() {
            let mut x = r.x;
            while x < r.right() {
                let (tx, ty) = (x / TILE, y / TILE);
                let tile = self.tile_rect(tx, ty);
                let end = ((tx + 1) * TILE).min(r.right());
                let n = (end - x) as usize * 4;
                let o = (((y % TILE) * tile.w + (x % TILE)) * 4) as usize;
                let i = self.index(tx, ty);
                let t = Arc::make_mut(&mut self.tiles[i]);
                t[o..o + n].copy_from_slice(&rgba[src..src + n]);
                src += n;
                x = end;
            }
        }
        Ok(())
    }

    /// Whether tile (`tx`, `ty`) is the same allocation in both canvases.
    pub fn shares_tile(&self, other: &Canvas, tx: u32, ty: u32) -> bool {
        self.width == other.width
            && self.height == other.height
            && tx < self.cols
            && ty < self.tile_rows()
            && Arc::ptr_eq(&self.tiles[self.index(tx, ty)], &other.tiles[other.index(tx, ty)])
    }

    /// The tiles whose allocation differs from `other`'s: all of them for
    /// a canvas of another size. A tile written to and then written back
    /// to its old bytes still counts; an undo that restores a snapshot
    /// does not.
    pub fn changed_tiles(&self, other: &Canvas) -> Vec<(u32, u32)> {
        let all = || self.tiles_in(self.bounds());
        if self.width != other.width || self.height != other.height {
            return all();
        }
        all()
            .into_iter()
            .filter(|&(tx, ty)| !self.shares_tile(other, tx, ty))
            .collect()
    }

    /// How many tiles this canvas shares with `other`.
    pub fn shared_tile_count(&self, other: &Canvas) -> usize {
        let total = self.tiles_in(self.bounds());
        total.len() - self.changed_tiles(other).len()
    }

    /// The union of the changed tiles' rectangles.
    pub fn changed_rect(&self, other: &Canvas) -> Rect {
        self.changed_tiles(other)
            .into_iter()
            .fold(Rect::default(), |acc, (tx, ty)| acc.union(self.tile_rect(tx, ty)))
    }
}
