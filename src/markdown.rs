use pulldown_cmark::{Event, Parser, Tag, TagEnd};

const BOLD: &str = "\x1b[1m";
const ITALIC: &str = "\x1b[3m";
const CODE: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

#[derive(Default)]
pub struct Renderer {
    line: String,
    block: String,
    fence: Option<char>,
    started: bool,
}

impl Renderer {
    pub fn push(&mut self, text: &str) -> String {
        let mut output = String::new();
        for character in text.chars() {
            self.line.push(character);
            if character == '\n' {
                let line = std::mem::take(&mut self.line);
                self.end_line(&line, &mut output);
            }
        }
        output
    }

    pub fn finish(&mut self) -> String {
        let line = std::mem::take(&mut self.line);
        self.block.push_str(&line);
        self.fence = None;
        self.flush()
    }

    fn end_line(&mut self, line: &str, output: &mut String) {
        let trimmed = line.trim();
        if let Some(marker) = trimmed
            .chars()
            .next()
            .filter(|&marker| marker == '`' || marker == '~')
            && trimmed.starts_with(&marker.to_string().repeat(3))
        {
            self.fence = match self.fence {
                None => Some(marker),
                Some(open) if open == marker => None,
                open => open,
            };
        }

        if trimmed.is_empty() && self.fence.is_none() {
            output.push_str(&self.flush());
        } else {
            self.block.push_str(line);
        }
    }

    fn flush(&mut self) -> String {
        let block = std::mem::take(&mut self.block);
        let rendered = render(&block);
        if rendered.is_empty() {
            return rendered;
        }
        if std::mem::replace(&mut self.started, true) {
            format!("\n\n{rendered}")
        } else {
            rendered
        }
    }
}

#[derive(Default)]
struct Writer {
    output: String,
    styles: Vec<&'static str>,
    lists: Vec<Option<u64>>,
    item_start: bool,
    code_block: Option<String>,
    links: Vec<(String, usize)>,
}

impl Writer {
    fn start_block(&mut self) {
        if self.output.is_empty() || std::mem::take(&mut self.item_start) {
            return;
        }
        let newlines = if self.lists.is_empty() { 2 } else { 1 };
        let trailing = self.output.len() - self.output.trim_end_matches('\n').len();
        for _ in trailing..newlines {
            self.output.push('\n');
        }
    }

    fn push_style(&mut self, style: &'static str) {
        self.styles.push(style);
        self.output.push_str(style);
    }

    fn pop_style(&mut self) {
        self.styles.pop();
        self.output.push_str(RESET);
        for style in &self.styles {
            self.output.push_str(style);
        }
    }

    fn text(&mut self, text: &str) {
        self.item_start = false;
        match &mut self.code_block {
            Some(code) => code.push_str(text),
            None => self.output.push_str(text),
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => self.text(&text),
            Event::Code(code) => {
                self.item_start = false;
                self.push_style(CODE);
                self.output.push_str(&code);
                self.pop_style();
            }
            Event::SoftBreak | Event::HardBreak => self.text("\n"),
            Event::Rule => {
                self.start_block();
                self.output.push_str("---");
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.start_block(),
            Tag::Heading { .. } => {
                self.start_block();
                self.push_style(BOLD);
            }
            Tag::CodeBlock(_) => {
                self.start_block();
                self.code_block = Some(String::new());
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.start_block();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                if !self.output.is_empty() && !self.output.ends_with('\n') {
                    self.output.push('\n');
                }
                let indent = "  ".repeat(self.lists.len().saturating_sub(1));
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        *number += 1;
                        format!("{}. ", *number - 1)
                    }
                    _ => "• ".to_string(),
                };
                self.output.push_str(&format!("{indent}{marker}"));
                self.item_start = true;
            }
            Tag::Strong => self.push_style(BOLD),
            Tag::Emphasis => self.push_style(ITALIC),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.links.push((dest_url.to_string(), self.output.len()));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) | TagEnd::Strong | TagEnd::Emphasis => self.pop_style(),
            TagEnd::CodeBlock => {
                let code = self.code_block.take().unwrap_or_default();
                self.push_style(CODE);
                self.output.push_str(code.trim_end_matches('\n'));
                self.pop_style();
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Link | TagEnd::Image => {
                let Some((url, start)) = self.links.pop() else {
                    return;
                };
                if self.output[start..] != url {
                    self.output.push_str(&format!(" ({url})"));
                }
            }
            _ => {}
        }
    }
}

fn render(markdown: &str) -> String {
    let mut writer = Writer::default();
    for event in Parser::new(markdown) {
        writer.event(event);
    }
    writer.output.trim_end_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(text: &str) -> String {
        let mut renderer = Renderer::default();
        let mut output = renderer.push(text);
        output.push_str(&renderer.finish());
        output
    }

    #[test]
    fn renders_headings_and_emphasis() {
        assert_eq!(
            render("# Title\n\nSome **bold** and *italic* text."),
            format!("{BOLD}Title{RESET}\n\nSome {BOLD}bold{RESET} and {ITALIC}italic{RESET} text.")
        );
    }

    #[test]
    fn keeps_outer_styles_after_nested_ones() {
        assert_eq!(
            render("**bold *both* bold**"),
            format!("{BOLD}bold {ITALIC}both{RESET}{BOLD} bold{RESET}")
        );
    }

    #[test]
    fn renders_code() {
        assert_eq!(
            render("Run `ls`:\n\n```sh\nls\npwd\n```"),
            format!("Run {CODE}ls{RESET}:\n\n{CODE}ls\npwd{RESET}")
        );
    }

    #[test]
    fn renders_lists() {
        assert_eq!(
            render("- one\n- two\n  - nested\n\n3. three\n4. four"),
            "• one\n• two\n  • nested\n\n3. three\n4. four"
        );
    }

    #[test]
    fn shows_link_targets() {
        assert_eq!(
            render("See [docs](https://example.com) or <https://example.org>."),
            "See docs (https://example.com) or https://example.org."
        );
    }

    #[test]
    fn keeps_line_breaks_and_tables() {
        assert_eq!(
            render("| a | b |\n|---|---|\n| 1 | 2 |"),
            "| a | b |\n|---|---|\n| 1 | 2 |"
        );
    }

    #[test]
    fn renders_each_block_once_it_is_complete() {
        let mut renderer = Renderer::default();

        assert_eq!(renderer.push("Some **bo"), "");
        assert_eq!(renderer.push("ld**\n"), "");
        assert_eq!(
            renderer.push("\n```\na\n\nb"),
            format!("Some {BOLD}bold{RESET}")
        );
        assert_eq!(
            renderer.push("\n```\n\nEnd"),
            format!("\n\n{CODE}a\n\nb{RESET}")
        );
        assert_eq!(renderer.finish(), "\n\nEnd");
    }
}
