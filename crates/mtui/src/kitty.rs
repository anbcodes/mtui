// Kitty graphics protocol: escape-sequence builders for transmitting images
// once and placing (and cropping) them over cells. Also works in ghostty and
// WezTerm. Decoding lives in `mimg`; this only speaks the protocol.

use crate::base64;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once anything is transmitted, so exit knows to clean up.
pub static USED: AtomicBool = AtomicBool::new(false);

/// Whether the terminal is known to speak the protocol. `MTUI_IMAGES=0`
/// turns it off and `=1` forces it on (e.g. through an ssh session that
/// loses the terminal's own environment).
pub fn supported() -> bool {
    match std::env::var("MTUI_IMAGES").as_deref() {
        Ok("0") | Ok("off") | Ok("no") => return false,
        Ok("1") | Ok("on") | Ok("yes") => return true,
        _ => {}
    }
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    // tmux swallows the sequences unless passthrough is set up.
    if !env("TMUX").is_empty() {
        return false;
    }
    env("TERM").contains("kitty") || !env("KITTY_WINDOW_ID").is_empty() || matches!(env("TERM_PROGRAM").as_str(), "ghostty" | "WezTerm") || env("TERM") == "xterm-ghostty"
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Format {
    /// A PNG file; the terminal decodes it.
    Png,
    /// Packed 8-bit RGB.
    Rgb,
}

pub struct Img {
    pub w: u32,
    pub h: u32,
    pub format: Format,
    /// Raw data is zlib-compressed (`o=z`).
    pub zlib: bool,
    pub data: Vec<u8>,
}

/// Where an image goes: cell position, size in cells, and the pixel
/// rectangle (x, y, w, h) of the source to show (all zeros = everything).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    pub id: u32,
    pub pid: u32,
    pub x: usize,
    pub y: usize,
    pub cols: usize,
    pub rows: usize,
    pub crop: (u32, u32, u32, u32),
}

/// Pixel size of a PNG from its header.
pub fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
        return None;
    }
    let n = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    Some((n(16), n(20)))
}

/// The escape sequences that store `img` under `id` without showing it.
pub fn transmit(id: u32, img: &Img) -> Vec<u8> {
    USED.store(true, Ordering::Relaxed);
    let b64 = base64::encode(&img.data);
    let mut out = Vec::with_capacity(b64.len() + 256);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    for (i, c) in chunks.iter().enumerate() {
        let more = if i + 1 < chunks.len() { 1 } else { 0 };
        if i == 0 {
            let fmt = match img.format {
                Format::Png => "f=100".to_string(),
                Format::Rgb => format!("f=24,s={},v={}", img.w, img.h),
            };
            out.extend_from_slice(format!("\x1b_Ga=t,{},i={},q=2{},m={};", fmt, id, if img.zlib { ",o=z" } else { "" }, more).as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b_Gq=2,m={};", more).as_bytes());
        }
        out.extend_from_slice(c);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Show an image at the cursor position (the caller moves the cursor).
pub fn place(p: &Placement) -> Vec<u8> {
    let (cx, cy, cw, ch) = p.crop;
    let crop = if cw > 0 && ch > 0 { format!(",x={},y={},w={},h={}", cx, cy, cw, ch) } else { String::new() };
    format!("\x1b_Ga=p,i={},p={},c={},r={}{},C=1,q=2\x1b\\", p.id, p.pid, p.cols, p.rows, crop).into_bytes()
}

/// Remove one placement but keep the image data for later.
pub fn unplace(p: &Placement) -> Vec<u8> {
    format!("\x1b_Ga=d,d=i,i={},p={},q=2\x1b\\", p.id, p.pid).into_bytes()
}

/// Free an image and all its placements.
pub fn free(id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", id).into_bytes()
}

pub const FREE_ALL: &[u8] = b"\x1b_Ga=d,d=A,q=2\x1b\\";

/// Size in cells for an image of `iw`x`ih` pixels shown at most `max_c` x
/// `max_r` cells, keeping the aspect ratio given the cell size in pixels.
pub fn fit(iw: u32, ih: u32, max_c: usize, max_r: usize, cell: (usize, usize)) -> (usize, usize) {
    let (cw, ch) = (cell.0.max(1) as f64, cell.1.max(1) as f64);
    let (iw, ih) = (iw.max(1) as f64, ih.max(1) as f64);
    // Never upscale past the image's own pixel size.
    let mut c = (max_c as f64).min(iw / cw).max(1.0);
    let mut r = c * cw * ih / (iw * ch);
    if r > max_r as f64 {
        r = max_r as f64;
        c = (r * ch * iw / (ih * cw)).max(1.0);
    }
    ((c.round() as usize).clamp(1, max_c.max(1)), (r.round() as usize).clamp(1, max_r.max(1)))
}

/// Like `fit`, but scales up to fill the area (fullscreen viewing).
pub fn fit_fill(iw: u32, ih: u32, max_c: usize, max_r: usize, cell: (usize, usize)) -> (usize, usize) {
    let (cw, ch) = (cell.0.max(1) as f64, cell.1.max(1) as f64);
    let (iw, ih) = (iw.max(1) as f64, ih.max(1) as f64);
    let mut c = max_c as f64;
    let mut r = c * cw * ih / (iw * ch);
    if r > max_r as f64 {
        r = max_r as f64;
        c = r * ch * iw / (ih * cw);
    }
    ((c.round() as usize).clamp(1, max_c.max(1)), (r.round() as usize).clamp(1, max_r.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills() {
        assert_eq!(fit_fill(80, 40, 100, 40, (8, 16)), (100, 25));
        assert_eq!(fit_fill(40, 80, 100, 10, (8, 16)), (10, 10));
    }

    #[test]
    fn sizes() {
        // 800x400 image, 8x16 cells: 100x25 cells at native size.
        assert_eq!(fit(800, 400, 100, 40, (8, 16)), (100, 25));
        assert_eq!(fit(800, 400, 50, 40, (8, 16)), (50, 13));
        assert_eq!(fit(400, 800, 50, 10, (8, 16)), (10, 10));
        // small images stay small
        assert_eq!(fit(80, 32, 100, 40, (8, 16)), (10, 2));
    }

    #[test]
    fn seqs() {
        let img = Img { w: 1, h: 1, format: Format::Rgb, zlib: false, data: vec![1, 2, 3] };
        let s = String::from_utf8(transmit(7, &img)).unwrap();
        assert_eq!(s, "\x1b_Ga=t,f=24,s=1,v=1,i=7,q=2,m=0;AQID\x1b\\");
        let big = Img { data: vec![0; 5000], ..img };
        let s = String::from_utf8(transmit(7, &big)).unwrap();
        assert_eq!(s.matches("\x1b_G").count(), 2);
        assert!(s.contains("m=1;") && s.contains("\x1b_Gq=2,m=0;"));
        let p = Placement { id: 7, pid: 1, x: 0, y: 0, cols: 10, rows: 5, crop: (0, 8, 100, 50) };
        assert_eq!(String::from_utf8(place(&p)).unwrap(), "\x1b_Ga=p,i=7,p=1,c=10,r=5,x=0,y=8,w=100,h=50,C=1,q=2\x1b\\");
    }
}
