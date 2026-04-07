//! Render Markdown to ANSI for the terminal (bold, italics, lists, headings, code).

use colored::Colorize;
use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd, TextMergeStream,
};

#[derive(Clone, Copy)]
enum Inline {
    Strong,
    Emphasis,
    Strikethrough,
}

fn style_text(
    text: &str,
    stack: &[Inline],
    in_link: bool,
    in_heading: Option<HeadingLevel>,
    in_code_block: bool,
    blockquote_depth: u32,
) -> String {
    if in_code_block {
        // Add left border for each line inside code blocks
        let lines: Vec<&str> = text.split('\n').collect();
        let styled: Vec<String> = lines
            .iter()
            .map(|line| {
                format!(
                    "  {} {}",
                    "│".truecolor(60, 60, 70),
                    line.truecolor(200, 200, 210)
                )
            })
            .collect();
        return styled.join("\n");
    }
    if let Some(level) = in_heading {
        let s = text.to_string();
        return match level {
            HeadingLevel::H1 => format!(
                "{} {}",
                "▌".truecolor(100, 149, 237),
                s.truecolor(255, 255, 255).bold()
            ),
            HeadingLevel::H2 => format!(
                "{} {}",
                "▎".truecolor(100, 149, 237),
                s.cyan().bold()
            ),
            _ => s.bold().to_string(),
        };
    }
    let mut acc = text.to_string();
    for i in stack.iter().rev() {
        acc = match i {
            Inline::Strong => acc.bold().to_string(),
            Inline::Emphasis => acc.italic().to_string(),
            Inline::Strikethrough => acc.dimmed().to_string(),
        };
    }
    if in_link {
        acc = acc.cyan().underline().to_string();
    }
    if blockquote_depth > 0 {
        acc = acc.dimmed().to_string();
    }
    acc
}

struct ListFrame {
    ordered: bool,
    next_num: u64,
}

fn ensure_blank_line(out: &mut String) {
    if out.is_empty() {
        return;
    }
    if out.ends_with("\n\n") {
        return;
    }
    if out.ends_with('\n') {
        out.push('\n');
    } else {
        out.push_str("\n\n");
    }
}

/// Convert Markdown to text with ANSI styling.
pub fn to_terminal(src: &str) -> String {
    let src = src.trim_end();
    if src.is_empty() {
        return String::new();
    }

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);

    let parser = TextMergeStream::new(Parser::new_ext(src, opts));

    let mut out = String::new();
    let mut inline_stack: Vec<Inline> = Vec::new();
    let mut list_frames: Vec<ListFrame> = Vec::new();
    let mut link_stack: Vec<String> = Vec::new();
    let mut in_heading: Option<HeadingLevel> = None;
    let mut in_code_block = false;
    let mut blockquote_depth = 0u32;
    let mut footnote_skip = 0u32;
    let mut meta_skip = 0u32;
    let mut table_col = 0usize;

    for event in parser {
        if footnote_skip > 0 {
            match &event {
                Event::Start(Tag::FootnoteDefinition(_)) => footnote_skip += 1,
                Event::End(TagEnd::FootnoteDefinition) => footnote_skip -= 1,
                _ => {}
            }
            continue;
        }
        if meta_skip > 0 {
            match &event {
                Event::Start(Tag::MetadataBlock(_)) => meta_skip += 1,
                Event::End(TagEnd::MetadataBlock(_)) => meta_skip -= 1,
                _ => {}
            }
            continue;
        }

        match event {
            Event::Start(Tag::FootnoteDefinition(_)) => {
                footnote_skip = 1;
            }
            Event::Start(Tag::MetadataBlock(_)) => {
                meta_skip = 1;
            }

            Event::Start(Tag::Heading { level, .. }) => {
                ensure_blank_line(&mut out);
                in_heading = Some(level);
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = None;
                out.push('\n');
            }

            Event::Start(Tag::Paragraph) => {
                ensure_blank_line(&mut out);
            }
            Event::End(TagEnd::Paragraph) => {
                out.push('\n');
            }

            Event::Start(Tag::BlockQuote(_)) => {
                ensure_blank_line(&mut out);
                blockquote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                blockquote_depth = blockquote_depth.saturating_sub(1);
                out.push('\n');
            }

            Event::Start(Tag::List(start)) => {
                if !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
                let ordered = start.is_some();
                let next_num = start.unwrap_or(1);
                list_frames.push(ListFrame { ordered, next_num });
            }
            Event::End(TagEnd::List(_)) => {
                list_frames.pop();
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }

            Event::Start(Tag::Item) => {
                let depth = list_frames.len().saturating_sub(1);
                out.push_str(&" ".repeat(depth * 2));
                if let Some(frame) = list_frames.last_mut() {
                    if frame.ordered {
                        let n = frame.next_num;
                        frame.next_num = frame.next_num.saturating_add(1);
                        out.push_str(&format!("{n}. ").cyan().to_string());
                    } else {
                        out.push_str(&format!("{} ", "•".cyan()));
                    }
                } else {
                    out.push_str(&format!("{} ", "•".cyan()));
                }
            }
            Event::End(TagEnd::Item) => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }

            Event::Start(Tag::Strong) => inline_stack.push(Inline::Strong),
            Event::End(TagEnd::Strong) => {
                inline_stack.pop();
            }
            Event::Start(Tag::Emphasis) => inline_stack.push(Inline::Emphasis),
            Event::End(TagEnd::Emphasis) => {
                inline_stack.pop();
            }
            Event::Start(Tag::Strikethrough) => inline_stack.push(Inline::Strikethrough),
            Event::End(TagEnd::Strikethrough) => {
                inline_stack.pop();
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                ensure_blank_line(&mut out);
                in_code_block = true;
                if let CodeBlockKind::Fenced(lang) = &kind {
                    let lang_str = lang.split_whitespace().next().unwrap_or("");
                    if !lang_str.is_empty() {
                        let label = format!(" {} ", lang_str);
                        out.push_str(&format!(
                            "  {}{}{}",
                            "╭─".truecolor(60, 60, 70),
                            label.truecolor(140, 140, 160),
                            format!("{}\n", "─".repeat(40usize.saturating_sub(label.len()))).truecolor(60, 60, 70),
                        ));
                    } else {
                        out.push_str(&format!(
                            "  {}\n",
                            format!("╭{}", "─".repeat(42)).truecolor(60, 60, 70),
                        ));
                    }
                } else {
                    out.push_str(&format!(
                        "  {}\n",
                        format!("╭{}", "─".repeat(42)).truecolor(60, 60, 70),
                    ));
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                out.push_str(&format!(
                    "  {}\n",
                    format!("╰{}", "─".repeat(42)).truecolor(60, 60, 70),
                ));
            }

            Event::Start(Tag::Link { dest_url, .. }) => {
                link_stack.push(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(url) = link_stack.pop() {
                    out.push(' ');
                    out.push_str(&format!("{}", format!("({url})").dimmed()));
                }
            }

            Event::Start(Tag::Image { dest_url, title, .. }) => {
                let label = if title.is_empty() {
                    dest_url.to_string()
                } else {
                    format!("{title}")
                };
                out.push_str(&format!("{}", format!("[image: {label}]").dimmed()));
            }
            Event::End(TagEnd::Image) => {}

            Event::Start(Tag::Table(_)) => {
                ensure_blank_line(&mut out);
                table_col = 0;
            }
            Event::End(TagEnd::Table) => {
                out.push('\n');
            }
            Event::Start(Tag::TableHead) => {}
            Event::End(TagEnd::TableHead) => {
                out.push('\n');
                out.push_str(&format!("{}", "───".dimmed()));
                out.push('\n');
                table_col = 0;
            }
            Event::Start(Tag::TableRow) => {
                table_col = 0;
            }
            Event::End(TagEnd::TableRow) => {
                out.push('\n');
            }
            Event::Start(Tag::TableCell) => {
                if table_col > 0 {
                    out.push_str(&format!("{}", " │ ".dimmed()));
                }
            }
            Event::End(TagEnd::TableCell) => {
                table_col += 1;
            }

            Event::Text(text) => {
                out.push_str(&style_text(
                    &text,
                    &inline_stack,
                    !link_stack.is_empty(),
                    in_heading,
                    in_code_block,
                    blockquote_depth,
                ));
            }

            Event::Code(code) => {
                out.push_str(&format!(
                    "{}{}{}",
                    "`".truecolor(90, 90, 100),
                    code.truecolor(220, 180, 100),
                    "`".truecolor(90, 90, 100)
                ));
            }

            Event::InlineMath(s) | Event::DisplayMath(s) => {
                out.push_str(&format!(" {}", s).dimmed());
            }

            Event::Html(s) | Event::InlineHtml(s) => {
                out.push_str(&s);
            }

            Event::FootnoteReference(label) => {
                out.push_str(&format!("{}", format!("[^{label}]").dimmed()));
            }

            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push('\n'),

            Event::Rule => {
                ensure_blank_line(&mut out);
                out.push_str(&format!("{}\n", "─".repeat(40).truecolor(50, 60, 80)));
            }

            Event::TaskListMarker(checked) => {
                let mark = if checked { "☑" } else { "☐" };
                out.push_str(&format!("{mark} "));
            }

            Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock) => {}

            Event::Start(Tag::DefinitionList)
            | Event::End(TagEnd::DefinitionList)
            | Event::Start(Tag::DefinitionListTitle)
            | Event::End(TagEnd::DefinitionListTitle)
            | Event::Start(Tag::DefinitionListDefinition)
            | Event::End(TagEnd::DefinitionListDefinition) => {}

            // Balanced by skip logic above; included for exhaustiveness.
            Event::End(TagEnd::FootnoteDefinition) | Event::End(TagEnd::MetadataBlock(_)) => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_list_no_raw_stars() {
        let s = to_terminal("**Name:** allux\n\n* one\n* two\n");
        assert!(!s.contains("**"));
        assert!(!s.contains("* one"));
    }
}
