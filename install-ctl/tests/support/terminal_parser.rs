//! Maintained VT100 terminal state used by the bounded real-PTY adapter.

pub struct TerminalScreen {
    parser: vt100::Parser,
}

impl TerminalScreen {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            parser: vt100::Parser::new(height, width, 0),
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.parser.set_size(height, width);
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn contains(&self, text: &str) -> bool {
        self.contents().contains(text)
    }

    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    #[cfg(test)]
    fn row(&self, row: u16) -> String {
        let (_, cols) = self.parser.screen().size();
        (0..cols)
            .map(|col| {
                self.parser
                    .screen()
                    .cell(row, col)
                    .map_or_else(String::new, vt100::Cell::contents)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalScreen;

    #[test]
    fn deferred_wrap_preserves_the_next_row_after_a_full_line() {
        let mut screen = TerminalScreen::new(4, 2);
        screen.feed(b"----|Art");
        assert_eq!(screen.row(0), "----");
        assert_eq!(screen.row(1), "|Art");
    }
}
