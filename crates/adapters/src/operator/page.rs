//! The text of a command and whether it found something.

/// Whether a check found something worth a non-zero exit code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Finding {
    /// Nothing to report.
    #[default]
    Clean,
    /// At least one thing needs the operator's attention.
    Found,
}

impl From<bool> for Finding {
    /// `Found` for `true`, `Clean` for `false`.
    fn from(found: bool) -> Self {
        let verdicts = [Self::Clean, Self::Found];
        verdicts
            .get(usize::from(found))
            .copied()
            .unwrap_or_default()
    }
}

/// What a command prints and whether it found something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    text: String,
    finding: Finding,
}

impl Rendered {
    /// The text to print, ending in a newline.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the check found something.
    #[must_use]
    pub const fn finding(&self) -> Finding {
        self.finding
    }
}

/// The lines of a command, and whether one of them is a finding.
#[derive(Debug, Default)]
pub(super) struct Page {
    lines: Vec<String>,
    finding: Finding,
}

impl Page {
    /// A line that is information.
    pub(super) fn line(&mut self, text: impl Into<String>) {
        self.lines.push(text.into());
    }

    /// A line that is a finding.
    pub(super) fn finding(&mut self, text: impl Into<String>) {
        self.flag(Finding::Found, text);
    }

    /// A line that is a finding when `found` is `Found` (or true) and information otherwise.
    pub(super) fn flag(&mut self, found: impl Into<Finding>, text: impl Into<String>) {
        self.finding = self.finding.max(found.into());
        self.line(text);
    }

    /// Each of `texts` as a line, indented by `margin`.
    pub(super) fn indented<Text: AsRef<str>>(
        &mut self,
        margin: &str,
        texts: impl IntoIterator<Item = Text>,
    ) {
        let indented = texts
            .into_iter()
            .map(|text| format!("{margin}{}", text.as_ref()));
        self.lines.extend(indented);
    }

    pub(super) fn finish(self) -> Rendered {
        Rendered {
            text: format!("{}\n", self.lines.join("\n")),
            finding: self.finding,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_with_only_information_is_clean() {
        let mut page = Page::default();
        page.line("hello");
        let done = page.finish();
        assert_eq!(done.finding(), Finding::Clean);
        assert_eq!(done.text(), "hello\n");
    }

    #[test]
    fn a_finding_makes_the_page_found_and_indented_lines_keep_their_margin() {
        let mut page = Page::default();
        page.finding("broken");
        page.indented("  ", ["a", "b"]);
        let done = page.finish();
        assert_eq!(done.finding(), Finding::Found);
        assert_eq!(done.text(), "broken\n  a\n  b\n");
    }
}
