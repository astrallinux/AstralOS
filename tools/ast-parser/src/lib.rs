use std::collections::HashMap;

/// Represents a parsed value in the .ast format.
#[derive(Debug, Clone, PartialEq)]
pub enum AstValue {
    /// A string value (e.g., "enabled", "nvenc")
    String(String),
    /// A boolean value (true/false)
    Boolean(bool),
    /// An enabled/disabled state
    State(State),
    /// An array of string values (e.g., ["nvenc", "x265"])
    Array(Vec<String>),
    /// A nested block of key-value pairs
    Block(AstBlock),
}

/// Represents an enabled or disabled state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Enabled,
    Disabled,
}

impl State {
    /// Parse a state from a string.
    fn from_str(s: &str) -> Option<State> {
        match s.to_lowercase().as_str() {
            "enabled" => Some(State::Enabled),
            "disabled" => Some(State::Disabled),
            _ => None,
        }
    }
}

/// A block in the AST, containing a name and key-value pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct AstBlock {
    /// The name of the block (e.g., "PACKAGES", "ffmpeg")
    pub name: String,
    /// The key-value pairs inside the block
    pub entries: HashMap<String, AstValue>,
}

impl AstBlock {
    /// Create a new empty block with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            entries: HashMap::new(),
        }
    }

    /// Insert a key-value pair into the block.
    pub fn insert(&mut self, key: impl Into<String>, value: AstValue) {
        self.entries.insert(key.into(), value);
    }

    /// Get a value by key.
    pub fn get(&self, key: &str) -> Option<&AstValue> {
        self.entries.get(key)
    }
}

/// Parser for the .ast configuration format.
pub struct AstParser {
    input: Vec<char>,
    position: usize,
    line: usize,
    column: usize,
}

impl AstParser {
    /// Create a new parser for the given input string.
    pub fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            position: 0,
            line: 1,
            column: 1,
        }
    }

    /// Parse the entire input and return a vector of top-level blocks.
    pub fn parse(&mut self) -> Result<Vec<AstBlock>, String> {
        let mut blocks = Vec::new();

        self.skip_whitespace_and_comments();

        while !self.is_at_end() {
            let block = self.parse_block()?;
            blocks.push(block);
            self.skip_whitespace_and_comments();
        }

        Ok(blocks)
    }

    /// Parse a single block: NAME { ... }
    fn parse_block(&mut self) -> Result<AstBlock, String> {
        let name = self.parse_identifier()?;
        self.skip_whitespace_and_comments();

        self.expect_char('{')?;
        self.skip_whitespace_and_comments();

        self.parse_block_body(name)
    }

    /// Parse the body of a block (after the opening {), given the block name.
    fn parse_block_body(&mut self, name: String) -> Result<AstBlock, String> {
        let mut block = AstBlock::new(name);

        while !self.is_at_end() && !self.check_char('}') {
            let (key, value) = self.parse_entry()?;
            block.insert(key, value);
            self.skip_whitespace_and_comments();
        }

        self.expect_char('}')?;
        Ok(block)
    }

    /// Parse a key-value entry or nested block inside a block.
    fn parse_entry(&mut self) -> Result<(String, AstValue), String> {
        let key = self.parse_identifier()?;
        self.skip_whitespace_and_comments();

        // Check if this is a nested block (key { ... }) or key-value (key = value)
        if self.check_char('{') {
            // Nested block: key { ... }
            self.advance(); // consume '{'
            self.skip_whitespace_and_comments();
            let block = self.parse_block_body(key.clone())?;
            Ok((key, AstValue::Block(block)))
        } else {
            // Key-value pair: key = value
            self.expect_char('=')?;
            self.skip_whitespace_and_comments();

            let value = self.parse_value()?;
            Ok((key, value))
        }
    }

    /// Parse a value (string, boolean, state, array, or nested block).
    fn parse_value(&mut self) -> Result<AstValue, String> {
        self.skip_whitespace_and_comments();

        if self.check_char('[') {
            self.parse_array()
        } else if self.check_char('{') {
            let block = self.parse_block()?;
            Ok(AstValue::Block(block))
        } else if self.check_char('"') {
            let s = self.parse_string()?;
            Ok(AstValue::String(s))
        } else {
            let ident = self.parse_identifier()?;
            self.parse_identifier_value(ident)
        }
    }

    /// Parse an array value: ["item1", "item2", ...]
    fn parse_array(&mut self) -> Result<AstValue, String> {
        self.expect_char('[')?;
        self.skip_whitespace_and_comments();

        let mut items = Vec::new();

        while !self.is_at_end() && !self.check_char(']') {
            let item = self.parse_string()?;
            items.push(item);
            self.skip_whitespace_and_comments();

            if self.check_char(',') {
                self.advance();
                self.skip_whitespace_and_comments();
            } else if !self.check_char(']') {
                return Err(self.error("Expected ',' or ']' in array"));
            }
        }

        self.expect_char(']')?;
        Ok(AstValue::Array(items))
    }

    /// Parse an identifier and determine its value type.
    fn parse_identifier_value(&mut self, ident: String) -> Result<AstValue, String> {
        // Check for boolean
        if ident.eq_ignore_ascii_case("true") {
            return Ok(AstValue::Boolean(true));
        }
        if ident.eq_ignore_ascii_case("false") {
            return Ok(AstValue::Boolean(false));
        }

        // Check for state (enabled/disabled)
        if let Some(state) = State::from_str(&ident) {
            return Ok(AstValue::State(state));
        }

        // Otherwise treat as string
        Ok(AstValue::String(ident))
    }

    /// Parse an identifier (alphanumeric, underscore, and hyphen, can start with digit).
    fn parse_identifier(&mut self) -> Result<String, String> {
        self.skip_whitespace_and_comments();

        if self.is_at_end() || (!self.current_char().is_alphanumeric() && self.current_char() != '_' && self.current_char() != '-') {
            return Err(self.error("Expected identifier"));
        }

        let mut ident = String::new();
        while !self.is_at_end() && (self.current_char().is_alphanumeric() || self.current_char() == '_' || self.current_char() == '-') {
            ident.push(self.advance());
        }

        if ident.is_empty() {
            return Err(self.error("Expected identifier"));
        }

        Ok(ident)
    }

    /// Parse a quoted string.
    fn parse_string(&mut self) -> Result<String, String> {
        self.skip_whitespace_and_comments();

        if !self.check_char('"') {
            // Also accept unquoted strings for array items
            return self.parse_identifier();
        }

        self.advance(); // consume opening quote
        let mut result = String::new();

        while !self.is_at_end() && !self.check_char('"') {
            if self.current_char() == '\\' {
                self.advance();
                if self.is_at_end() {
                    return Err(self.error("Unterminated escape sequence"));
                }
                match self.current_char() {
                    '"' => result.push('"'),
                    '\\' => result.push('\\'),
                    'n' => result.push('\n'),
                    't' => result.push('\t'),
                    'r' => result.push('\r'),
                    c => return Err(self.error(&format!("Unknown escape sequence: \\{}", c))),
                }
                self.advance();
            } else {
                result.push(self.advance());
            }
        }

        if self.is_at_end() {
            return Err(self.error("Unterminated string"));
        }

        self.advance(); // consume closing quote
        Ok(result)
    }

    /// Skip whitespace and comments.
    fn skip_whitespace_and_comments(&mut self) {
        while !self.is_at_end() {
            self.skip_whitespace();

            if self.check_char('#') {
                self.skip_comment();
            } else {
                break;
            }
        }
    }

    /// Skip whitespace characters.
    fn skip_whitespace(&mut self) {
        while !self.is_at_end() && self.current_char().is_whitespace() {
            self.advance();
        }
    }

    /// Skip a comment line.
    fn skip_comment(&mut self) {
        while !self.is_at_end() && self.current_char() != '\n' {
            self.advance();
        }
    }

    /// Expect a specific character, returning an error if not found.
    fn expect_char(&mut self, expected: char) -> Result<(), String> {
        self.skip_whitespace_and_comments();
        if self.check_char(expected) {
            self.advance();
            Ok(())
        } else {
            Err(self.error(&format!("Expected '{}'", expected)))
        }
    }

    /// Check if the current character matches the given one.
    fn check_char(&self, c: char) -> bool {
        !self.is_at_end() && self.current_char() == c
    }

    /// Get the current character without advancing.
    fn current_char(&self) -> char {
        self.input[self.position]
    }

    /// Advance the position and return the consumed character.
    fn advance(&mut self) -> char {
        let c = self.current_char();
        self.position += 1;
        if c == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        c
    }

    /// Check if we've reached the end of input.
    fn is_at_end(&self) -> bool {
        self.position >= self.input.len()
    }

    /// Create an error message with current position info.
    fn error(&self, msg: &str) -> String {
        format!("Line {}, column {}: {}", self.line, self.column, msg)
    }
}

/// Convenience function to parse a string into AST blocks.
pub fn parse(input: &str) -> Result<Vec<AstBlock>, String> {
    let mut parser = AstParser::new(input);
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_block() {
        let input = r#"
PACKAGES {
    firefox = enabled
}
"#;
        let blocks = parse(input).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].name, "PACKAGES");
        assert_eq!(blocks[0].get("firefox"), Some(&AstValue::State(State::Enabled)));
    }

    #[test]
    fn test_parse_nested_block() {
        let input = r#"
PACKAGES {
    ffmpeg {
        state = enabled
        useflags = ["nvenc", "x265"]
    }
}
"#;
        let blocks = parse(input).unwrap();
        assert_eq!(blocks.len(), 1);
        let packages = &blocks[0];
        let ffmpeg = packages.get("ffmpeg").unwrap();
        match ffmpeg {
            AstValue::Block(block) => {
                assert_eq!(block.name, "ffmpeg");
                assert_eq!(block.get("state"), Some(&AstValue::State(State::Enabled)));
                match block.get("useflags") {
                    Some(AstValue::Array(items)) => {
                        assert_eq!(items, &["nvenc", "x265"]);
                    }
                    _ => panic!("Expected array for useflags"),
                }
            }
            _ => panic!("Expected nested block"),
        }
    }

    #[test]
    fn test_parse_boolean() {
        let input = r#"
TEST {
    flag1 = true
    flag2 = false
}
"#;
        let blocks = parse(input).unwrap();
        let test = &blocks[0];
        assert_eq!(test.get("flag1"), Some(&AstValue::Boolean(true)));
        assert_eq!(test.get("flag2"), Some(&AstValue::Boolean(false)));
    }

    #[test]
    fn test_parse_string() {
        let input = r#"
TEST {
    name = "firefox"
    version = 120
}
"#;
        let blocks = parse(input).unwrap();
        let test = &blocks[0];
        assert_eq!(test.get("name"), Some(&AstValue::String("firefox".to_string())));
        assert_eq!(test.get("version"), Some(&AstValue::String("120".to_string())));
    }

    #[test]
    fn test_ignore_comments() {
        let input = r#"
# This is a comment
PACKAGES {
    # Another comment
    firefox = enabled
}
"#;
        let blocks = parse(input).unwrap();
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn test_empty_block() {
        let input = r#"
EMPTY {
}
"#;
        let blocks = parse(input).unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].entries.is_empty());
    }

    #[test]
    fn test_multiple_top_level_blocks() {
        let input = r#"
BLOCK1 {
    a = 1
}
BLOCK2 {
    b = 2
}
"#;
        let blocks = parse(input).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].name, "BLOCK1");
        assert_eq!(blocks[1].name, "BLOCK2");
    }

    #[test]
    fn test_error_unexpected_eof() {
        let input = "PACKAGES {";
        let result = parse(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_error_missing_brace() {
        let input = "PACKAGES { firefox = enabled";
        let result = parse(input);
        assert!(result.is_err());
    }
}