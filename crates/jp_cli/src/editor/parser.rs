use std::fmt;

const CONVERSATION_MARKER: &str = "\n<!-- CONVERSATION_MARKER -->\n";
const CONFIG_HEADER: &str = "\n# Active Configuration\n";
pub(super) const CUT_MARKER: &str = indoc::indoc!(
    "
    ---------------------------------------8<---------------------------------------
    --------------------- EVERYTHING BELOW THIS LINE IS IGNORED --------------------
    --------------------------------------->8---------------------------------------"
);

#[derive(Debug, PartialEq)]
pub enum Error {
    MissingConfigHeader,
    MissingConfigCodeBlock,
}

#[derive(Debug, PartialEq, Default)]
pub struct QueryDocument<'s> {
    pub query: &'s str,
    pub meta: QueryMetaSection<'s>,
}

#[derive(Debug, PartialEq, Default)]
pub struct QueryMetaSection<'s> {
    pub config: QueryConfigSection<'s>,
    pub history: QueryHistorySection<'s>,
}

#[derive(Debug, PartialEq, Default)]
pub struct QueryConfigSection<'s> {
    pub value: &'s str,
    pub error: Option<&'s str>,
}

#[derive(Debug, PartialEq, Default)]
pub struct QueryHistorySection<'s> {
    pub value: &'s str,
}

impl fmt::Display for QueryDocument<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.query)?;

        if self.meta != QueryMetaSection::default() {
            f.write_str("\n\n")?;
            f.write_str(CUT_MARKER)?;
            f.write_str("\n")?;

            write!(f, "{}", self.meta)?;
        }

        Ok(())
    }
}

impl From<QueryDocument<'_>> for String {
    fn from(doc: QueryDocument<'_>) -> Self {
        doc.to_string()
    }
}

impl fmt::Display for QueryMetaSection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.config)?;
        if !self.history.value.trim().is_empty() {
            write!(f, "{}{}", CONVERSATION_MARKER, self.history)?;
        }

        Ok(())
    }
}

impl fmt::Display for QueryConfigSection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\n# Active Configuration\n\n")?;

        if let Some(error) = self.error {
            if error.starts_with("> ERROR:") {
                writeln!(f, "{error}")?;
            } else {
                let mut lines = error.lines();
                let first = lines.next().unwrap_or("");
                writeln!(f, "> ERROR: {first}")?;
                for line in lines {
                    writeln!(f, "> {line}")?;
                }
            }
        } else {
            indoc::writedoc!(
                f,
                "
                > NOTE: You can edit this configuration to apply it to the current conversation.
                >       If the configuration is invalid, the editor will re-open.
                >
                >       Only the most relevant configuration is shown here, but you can add any
                >       configuration properties you want to apply.
                "
            )?;
        }

        f.write_str("\n```toml\n")?;
        f.write_str(self.value)?;
        if !self.value.is_empty() && !self.value.ends_with('\n') {
            f.write_str("\n")?;
        }
        f.write_str("```\n")?;

        Ok(())
    }
}

impl fmt::Display for QueryHistorySection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl<'s> TryFrom<&'s str> for QueryDocument<'s> {
    type Error = Error;

    fn try_from(content: &'s str) -> Result<Self, Self::Error> {
        let (query, tail) = content.split_once(CUT_MARKER).unwrap_or((content, ""));

        let meta = if tail.trim().is_empty() {
            QueryMetaSection::default()
        } else {
            QueryMetaSection::try_from(tail)?
        };

        Ok(QueryDocument {
            query: query.trim(),
            meta,
        })
    }
}

impl<'s> TryFrom<&'s str> for QueryMetaSection<'s> {
    type Error = Error;

    fn try_from(s: &'s str) -> Result<Self, Self::Error> {
        let (config, tail) = s.rsplit_once(CONVERSATION_MARKER).unwrap_or((s, ""));
        let config = QueryConfigSection::try_from(config)?;
        let history = QueryHistorySection { value: tail.trim() };

        Ok(Self { config, history })
    }
}

impl<'s> TryFrom<&'s str> for QueryConfigSection<'s> {
    type Error = Error;

    fn try_from(s: &'s str) -> Result<Self, Self::Error> {
        // Remove the header.
        let s = s
            .split_once(CONFIG_HEADER)
            .map(|(_, b)| b.trim())
            .ok_or(Error::MissingConfigHeader)?;

        // Find the error message, if any.
        let mut n = 0;
        let error = if s.starts_with("> ERROR:") {
            for (i, c) in s.char_indices() {
                if c == '\n'
                    && s.get(i + c.len_utf8()..)
                        .is_none_or(|v| !v.starts_with('>'))
                {
                    break;
                }

                n = i + c.len_utf8();
            }

            Some(s[..n].trim())
        } else {
            None
        };

        // Get the config code block content.
        let value = s[n..]
            .trim()
            .split_once("```toml")
            .and_then(|(_, b)| b.rsplit_once("```").map(|(a, _)| a.trim()))
            .ok_or(Error::MissingConfigCodeBlock)?
            .trim();

        Ok(Self { value, error })
    }
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
