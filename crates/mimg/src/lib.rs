// Turns downloaded image bytes into something the terminal can show: PNG goes
// through untouched (the terminal decodes it), JPEG is decoded, shrunk to a
// sane size and zlib-compressed. Anything else is reported as unsupported.

use mtui::kitty::{self, Format, Img, Placement};
use mtui::screen::Screen;
use std::collections::HashMap;
use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
use zune_jpeg::JpegDecoder;

/// Widest image we send; the terminal scales it to the cells anyway.
const MAX_W: usize = 800;
/// Largest download worth sending as-is.
pub const MAX_PNG: usize = 6 << 20;

pub fn prepare(b: &[u8]) -> Result<Img, String> {
    if b.starts_with(b"\x89PNG") {
        let (w, h) = mtui::kitty::png_size(b).ok_or("bad PNG")?;
        if b.len() > MAX_PNG {
            return Err("PNG too large".into());
        }
        return Ok(Img { w, h, format: Format::Png, zlib: false, data: b.to_vec() });
    }
    if b.starts_with(&[0xff, 0xd8]) {
        return jpeg(b);
    }
    Err("unsupported image type".into())
}

fn jpeg(b: &[u8]) -> Result<Img, String> {
    let mut d = JpegDecoder::new_with_options(ZCursor::new(b), DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB));
    let px = d.decode().map_err(|e| format!("JPEG: {:?}", e))?;
    let info = d.info().ok_or("JPEG: no header")?;
    let (w, h) = (info.width as usize, info.height as usize);
    if px.len() != w * h * 3 {
        return Err("JPEG: unexpected pixel layout".into());
    }
    let (w, h, px) = if w > MAX_W { shrink(&px, w, h, MAX_W) } else { (w, h, px) };
    let data = miniz_oxide::deflate::compress_to_vec_zlib(&px, 1);
    Ok(Img { w: w as u32, h: h as u32, format: Format::Rgb, zlib: true, data })
}

/// Box-filter RGB down to `nw` pixels wide.
fn shrink(px: &[u8], w: usize, h: usize, nw: usize) -> (usize, usize, Vec<u8>) {
    let nh = (h * nw / w).max(1);
    let mut out = vec![0u8; nw * nh * 3];
    for y in 0..nh {
        let (y0, y1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1).min(h));
        for x in 0..nw {
            let (x0, x1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1).min(w));
            let mut s = [0u32; 3];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = (yy * w + xx) * 3;
                    s[0] += px[i] as u32;
                    s[1] += px[i + 1] as u32;
                    s[2] += px[i + 2] as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (y * nw + x) * 3;
            for c in 0..3 {
                out[o + c] = (s[c] / n) as u8;
            }
        }
    }
    (nw, nh, out)
}

pub enum Slot {
    Loading,
    Ready { id: u32, w: u32, h: u32 },
    Failed(String),
}

/// Images the app has asked for, by key (usually the URL): which are being
/// fetched, which are stored in the terminal, and which failed.
pub struct Gallery {
    pub enabled: bool,
    next_id: u32,
    slots: HashMap<String, Slot>,
    cell: (usize, usize),
}

impl Gallery {
    /// `allowed` is the app's own setting; the terminal must also support it.
    pub fn new(allowed: bool) -> Gallery {
        Gallery { enabled: allowed && kitty::supported(), next_id: 1, slots: HashMap::new(), cell: mtui::term::cell_px() }
    }

    pub fn slot(&self, key: &str) -> Option<&Slot> {
        self.slots.get(key)
    }

    /// Note that `key` is being fetched. False if it was already requested.
    pub fn request(&mut self, key: &str) -> bool {
        if !self.enabled || self.slots.contains_key(key) {
            return false;
        }
        self.slots.insert(key.to_string(), Slot::Loading);
        true
    }

    /// Store a fetched image in the terminal.
    pub fn arrived(&mut self, key: &str, bytes: Result<Vec<u8>, String>, scr: &Screen) {
        self.cell = mtui::term::cell_px();
        let slot = match bytes.and_then(|b| prepare(&b)) {
            Ok(img) => {
                let id = self.next_id;
                self.next_id += 1;
                scr.raw(&kitty::transmit(id, &img));
                Slot::Ready { id, w: img.w, h: img.h }
            }
            Err(e) => Slot::Failed(e),
        };
        self.slots.insert(key.to_string(), slot);
    }

    /// Size in cells of a ready image, at most `max_c` x `max_r`.
    pub fn size(&self, key: &str, max_c: usize, max_r: usize) -> Option<(usize, usize)> {
        match self.slots.get(key) {
            Some(Slot::Ready { w, h, .. }) => Some(kitty::fit(*w, *h, max_c, max_r, self.cell)),
            _ => None,
        }
    }

    /// The placement showing rows `i0..i1` (of `total`) of `key` at (x, y).
    pub fn place(&self, key: &str, pid: u32, (x, y): (usize, usize), cols: usize, total: usize, i0: usize, i1: usize) -> Option<Placement> {
        let Some(Slot::Ready { id, w, h }) = self.slots.get(key) else { return None };
        let crop = if i0 == 0 && i1 == total { (0, 0, 0, 0) } else { (0, (*h as usize * i0 / total) as u32, *w, ((*h as usize * (i1 - i0)) / total).max(1) as u32) };
        Some(Placement { id: *id, pid, x, y, cols, rows: i1 - i0, crop })
    }
}

impl Gallery {
    /// The key of the image stored under kitty id `id` (from `Screen::image_at`).
    pub fn key_of(&self, id: u32) -> Option<&str> {
        self.slots.iter().find(|(_, s)| matches!(s, Slot::Ready { id: i, .. } if *i == id)).map(|(k, _)| k.as_str())
    }

    /// A placement showing `key` as large as fits in a `w` x `h` cell area
    /// at the top left of the screen, centered (for a fullscreen viewer).
    pub fn place_full(&self, key: &str, pid: u32, (w, h): (usize, usize)) -> Option<Placement> {
        let Some(Slot::Ready { w: iw, h: ih, .. }) = self.slots.get(key) else { return None };
        let (cols, rows) = kitty::fit_fill(*iw, *ih, w, h, self.cell);
        self.place(key, pid, (w.saturating_sub(cols) / 2, h.saturating_sub(rows) / 2), cols, rows, 0, rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_of_id() {
        let mut g = Gallery::new(true);
        g.slots.insert("k".into(), Slot::Ready { id: 7, w: 1, h: 1 });
        assert_eq!((g.key_of(7), g.key_of(8)), (Some("k"), None));
    }

    #[test]
    fn full() {
        let mut g = Gallery::new(true);
        g.slots.insert("k".into(), Slot::Ready { id: 3, w: 100, h: 100 });
        let p = g.place_full("k", 1, (100, 20)).unwrap();
        assert_eq!(p.y, 0);
        assert!(p.rows == 20 && p.x == (100 - p.cols) / 2);
    }

    #[test]
    fn shrinks() {
        let px: Vec<u8> = (0..4 * 4 * 3).map(|i| (i % 7) as u8).collect();
        let (w, h, o) = shrink(&px, 4, 4, 2);
        assert_eq!((w, h, o.len()), (2, 2, 12));
        let flat = vec![200u8; 4 * 2 * 3];
        assert!(shrink(&flat, 4, 2, 2).2.iter().all(|&v| v == 200));
    }

    #[test]
    fn crops() {
        let mut g = Gallery::new(true);
        g.slots.insert("k".into(), Slot::Ready { id: 3, w: 100, h: 80 });
        let p = g.place("k", 1, (5, 6), 10, 8, 2, 6).unwrap();
        assert_eq!((p.rows, p.crop, p.id), (4, (0, 20, 100, 40), 3));
        assert_eq!(g.place("k", 1, (0, 0), 10, 8, 0, 8).unwrap().crop, (0, 0, 0, 0));
    }

    #[test]
    fn rejects_other_formats() {
        assert!(prepare(b"GIF89a....").is_err());
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x01rest";
        let i = prepare(png).unwrap();
        assert_eq!((i.w, i.h, i.format), (2, 1, Format::Png));
    }
}
