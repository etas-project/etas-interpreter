use std::fmt::{self, Write};

use etas_builtin::text::input::TextInput;

use super::{StringValue, chunks::TextLeaves};

impl fmt::Display for StringValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in TextLeaves::new(&self.0) {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

impl fmt::Debug for StringValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(text) = self.contiguous() {
            return fmt::Debug::fmt(text, f);
        }
        f.write_char('"')?;
        for chunk in TextLeaves::new(&self.0) {
            let mut start = 0;
            for (offset, ch) in chunk.char_indices() {
                let escaped = ch.escape_debug();
                // Rust string Debug leaves single quotes literal; all other
                // escaping and Unicode classification comes from char itself.
                if ch == '\'' || escaped.len() == 1 {
                    continue;
                }
                f.write_str(&chunk[start..offset])?;
                fmt::Display::fmt(&escaped, f)?;
                start = offset + ch.len_utf8();
            }
            f.write_str(&chunk[start..])?;
        }
        f.write_char('"')
    }
}

impl serde::Serialize for StringValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.contiguous() {
            Some(text) => serializer.serialize_str(text),
            None => serializer.collect_str(self),
        }
    }
}

#[cfg(test)]
mod tests;
