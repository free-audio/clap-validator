use std::fmt::{Display, Write};
use textwrap::core::display_width;
use yansi::Paint;

/// A pretty-printed report that consists of a header, a body with multiple items, and an optional footer.
/// The body items can be tables, text, or sub-reports.
#[derive(Debug, Default)]
pub struct Report {
    pub header: String,
    pub footer: Vec<String>,
    pub items: Vec<ReportItem>,
}

#[derive(Debug)]
pub enum ReportItem {
    Table(Vec<(String, String)>),
    Text(String),
    Child(Report),
}

impl Report {
    fn print_width(&self, width: usize) -> String {
        let mut result = String::new();

        let pipe = "│".dim();
        let bar = "─".dim();
        let ctl = "┌".dim();
        let cbl = "└".dim();

        // Print the header text
        writeln!(result, "{}{} {}", ctl, bar, self.header.bold()).ok();

        // Print the body
        for item in &self.items {
            match item {
                ReportItem::Text(text) => {
                    for line in pretty_wrap(text, width.saturating_sub(2)) {
                        writeln!(result, "{} {}", pipe, line).ok();
                    }
                }

                ReportItem::Child(child) => {
                    writeln!(result, "{} ", pipe).ok();

                    let child = child.print_width(width.saturating_sub(2));
                    for line in child.lines() {
                        writeln!(result, "{} {}", pipe, line).ok();
                    }
                }

                ReportItem::Table(rows) => {
                    let max_key_len = rows.iter().map(|(k, _)| display_width(k)).max().unwrap_or(0);

                    for (key, value) in rows {
                        for (index, line) in pretty_wrap(value, width.saturating_sub(2 + max_key_len))
                            .into_iter()
                            .enumerate()
                        {
                            let pad = if index == 0 {
                                format!("{:width$}", key, width = max_key_len)
                            } else {
                                " ".repeat(max_key_len)
                            };

                            writeln!(result, "{} {} {}", pipe, pad.dim().italic(), line).ok();
                        }
                    }
                }
            }
        }

        // Print the footer line
        write!(result, "{}{}{} ", cbl, bar, bar).ok();

        // Print footer text
        for (i, footer) in self.footer.iter().enumerate() {
            if i > 0 {
                write!(result, " {} ", bar).ok();
            }

            write!(result, "{}", footer).ok();
        }

        result
    }
}

impl Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.print_width(match textwrap::termwidth() {
                w if w > 40 => w - 10,
                w => w,
            },)
        )
    }
}

pub fn pluralize(count: usize, singular: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{} {}s", count, singular)
    }
}

pub fn pretty_wrap(text: &str, width: usize) -> Vec<std::borrow::Cow<'_, str>> {
    textwrap::wrap(
        text,
        textwrap::Options::new(width)
            .break_words(true)
            .wrap_algorithm(textwrap::WrapAlgorithm::OptimalFit(Default::default())),
    )
}
