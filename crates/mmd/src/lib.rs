//! A minimal markdown renderer. `render` turns markdown into HTML (blocks
//! carry `data-line` source positions), `page` wraps it in a printable page,
//! and `Server` is a live-reloading preview that an editor can drive.

mod fmt;
mod md;
mod page;
mod serve;

pub use fmt::format_range;
pub use md::{esc, render, Doc, Heading, Options};
pub use page::page;
pub use serve::{open_browser, Server};

/// A standalone printable HTML document for `src`.
pub fn to_html(src: &str, title: Option<&str>, o: &Options) -> String {
    let doc = render(src, o);
    let t = title.map(String::from).or_else(|| doc.title.clone()).unwrap_or_else(|| "Document".into());
    page(&doc, &t, false, 0)
}
