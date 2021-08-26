use std::fmt;

use crossterm::{self, cursor, style, terminal};

use crate::mode::{HeaderInfo, Output, ReadLine, SelectMenu};

pub enum Color {
    White,
    Red,
    Green,
    Blue,
    Yellow,
}
impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::White => f.write_str("\x1b[38;5;15m"),
            Self::Red => f.write_str("\x1b[38;5;1m"),
            Self::Green => f.write_str("\x1b[38;5;2m"),
            Self::Blue => f.write_str("\x1b[38;5;4m"),
            Self::Yellow => f.write_str("\x1b[38;5;3m"),
        }
    }
}

pub trait SelectEntryDraw {
    fn draw(&self, drawer: &mut Drawer, hovered: bool, full: bool) -> usize;
}

pub struct Drawer {
    buf: Vec<u8>,
    pub viewport_size: (u16, u16),
}

impl Drawer {
    pub fn new(mut buf: Vec<u8>, viewport_size: (u16, u16)) -> Self {
        buf.clear();
        Self { buf, viewport_size }
    }

    pub fn take_buf(self) -> Vec<u8> {
        self.buf
    }

    pub fn clear_to_bottom(&mut self) {
        crossterm::queue!(
            self.buf,
            style::SetBackgroundColor(style::Color::Black),
            terminal::Clear(terminal::ClearType::FromCursorDown),
        )
        .unwrap();
    }

    pub fn header(&mut self, info: HeaderInfo, spinner_state: u8) {
        let background_color = style::Color::DarkYellow;
        let foreground_color = style::Color::Black;

        let spinner = ['-', '\\', '|', '/'];
        let spinner = match info.waiting_response {
            true => spinner[spinner_state as usize % spinner.len()],
            false => ' ',
        };

        crossterm::queue!(
            self.buf,
            cursor::MoveTo(0, 0),
            style::SetBackgroundColor(background_color),
            style::SetForegroundColor(foreground_color),
            style::Print(' '),
            style::Print(spinner),
            style::Print(' '),
            style::SetBackgroundColor(foreground_color),
            style::SetForegroundColor(background_color),
            style::Print(' '),
            style::Print(info.name),
            style::Print(' '),
            style::SetBackgroundColor(background_color),
            terminal::Clear(terminal::ClearType::UntilNewLine),
            cursor::MoveToNextLine(1),
            style::ResetColor,
        )
        .unwrap();
    }

    pub fn str(&mut self, line: &str) {
        self.buf.extend_from_slice(line.as_bytes());
    }

    pub fn fmt(&mut self, args: fmt::Arguments) {
        use std::io::Write;
        self.buf.write_fmt(args).unwrap();
    }

    pub fn next_line(&mut self) {
        crossterm::queue!(
            self.buf,
            terminal::Clear(terminal::ClearType::UntilNewLine),
            cursor::MoveToNextLine(1),
        )
        .unwrap();
    }

    pub fn output(&mut self, output: &Output) -> usize {
        let tab_bytes = [b' '; 4];
        let mut utf8_buf = [0; 4];

        let mut line_count = 0;
        for line in output.lines_from_scroll() {
            let mut x = 0;
            for c in line.chars() {
                match c {
                    '\t' => {
                        self.buf.extend_from_slice(&tab_bytes);
                        x += tab_bytes.len();
                    }
                    _ => {
                        let bytes = c.encode_utf8(&mut utf8_buf).as_bytes();
                        self.buf.extend_from_slice(bytes);
                        x += 1;
                    }
                }

                if x >= self.viewport_size.0 as _ {
                    x -= self.viewport_size.0 as usize;
                    line_count += 1;
                }
            }

            crossterm::queue!(
                self.buf,
                terminal::Clear(terminal::ClearType::UntilNewLine),
                cursor::MoveToNextLine(1),
            )
            .unwrap();

            line_count += 1;
            if line_count >= self.viewport_size.1 as _ {
                break;
            }
        }

        line_count
    }

    pub fn readline(&mut self, readline: &ReadLine) {
        crossterm::queue!(
            self.buf,
            style::SetBackgroundColor(style::Color::Black),
            style::SetForegroundColor(style::Color::White),
            style::Print(readline.input()),
            style::SetBackgroundColor(style::Color::DarkRed),
            style::Print(' '),
            style::SetBackgroundColor(style::Color::Black),
        )
        .unwrap();
    }

    pub fn select_menu<'entries, I, E>(
        &mut self,
        select: &SelectMenu,
        header_height: u16,
        show_full_hovered_entry: bool,
        entries: I,
    ) where
        I: 'entries + Iterator<Item = &'entries E>,
        E: 'entries + SelectEntryDraw,
    {
        let cursor_index = select.cursor();

        crossterm::queue!(
            self.buf,
            style::SetBackgroundColor(style::Color::Black),
            style::SetForegroundColor(style::Color::White),
        )
        .unwrap();

        let mut line_count = 0;
        let max_line_count =
            self.viewport_size.1.saturating_sub(1 + header_height) as usize;

        for (i, entry) in entries.enumerate().skip(select.scroll()) {
            let hovered = i == cursor_index;
            if hovered {
                crossterm::queue!(
                    self.buf,
                    style::SetBackgroundColor(style::Color::DarkMagenta),
                )
                .unwrap();
            }

            line_count +=
                entry.draw(self, hovered, hovered && show_full_hovered_entry);

            crossterm::queue!(
                self.buf,
                terminal::Clear(terminal::ClearType::UntilNewLine),
                cursor::MoveToNextLine(1),
            )
            .unwrap();

            if hovered {
                crossterm::queue!(
                    self.buf,
                    style::SetBackgroundColor(style::Color::Black),
                )
                .unwrap();
            }

            if line_count >= max_line_count {
                break;
            }
        }
    }
}

