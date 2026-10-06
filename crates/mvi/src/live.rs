// :mmd — a live HTML preview of the buffer in the browser, rendered by mmd.
// The page follows what you type (not just what you save) and scrolls to the
// cursor line; double-clicking a block in the browser moves the cursor there.

use crate::editor::Editor;

pub struct Live {
    server: mmd::Server,
    synced: Option<(usize, u64)>,
    line: usize,
}

impl Editor {
    /// `:mmd` starts the preview and opens it; `:mmd stop` ends it.
    pub fn mmd_command(&mut self, arg: &str) {
        match arg {
            "stop" | "off" | "close" => {
                self.live = None;
                return self.info("mmd: preview stopped");
            }
            "" => {}
            _ => return self.err("usage: :mmd [stop]"),
        }
        if self.live.is_none() {
            match mmd::Server::start(0, None, "", mmd::Options::default()) {
                Ok(server) => self.live = Some(Live { server, synced: None, line: 0 }),
                Err(e) => return self.err(format!("mmd: {}", e)),
            }
        }
        self.sync_mmd();
        let Some(l) = &self.live else { return };
        let url = l.server.url();
        match mmd::open_browser(&url) {
            Ok(()) => self.info(format!("mmd: {} (live; :mmd stop)", url)),
            Err(e) => self.err(format!("{} ({})", url, e)),
        }
    }

    pub fn mmd_active(&self) -> bool {
        self.live.is_some()
    }

    /// Push the buffer, its place and any double-clicked line between the
    /// editor and the preview. Cheap when nothing changed.
    pub fn sync_mmd(&mut self) {
        let Some(mut l) = self.live.take() else { return };
        let (cur, b) = (self.cur, &self.bufs[self.cur]);
        if l.synced != Some((cur, b.version)) {
            l.server.set_base(b.path.as_ref().and_then(|p| p.canonicalize().ok()).and_then(|p| p.parent().map(|d| d.to_path_buf())));
            l.server.set_title(&b.name);
            l.server.set_source(&b.lines.join("\n"));
            l.synced = Some((cur, b.version));
            l.line = 0;
        }
        let line = b.cy + 1;
        if line != l.line {
            l.line = line;
            l.server.scroll_to(line);
        }
        if let Some(g) = l.server.take_goto() {
            let y = g.saturating_sub(1).min(self.bb().lines.len() - 1);
            self.set_cursor((y, 0));
            l.line = y + 1;
        }
        self.live = Some(l);
    }
}
