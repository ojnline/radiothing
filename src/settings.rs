use std::{collections::HashMap, convert::TryFrom, error::Error, fmt::Display, path::Path};

#[derive(Clone, Debug)]
pub enum Field {
    String(String),
    Number(f64),
    ParseError,
}

impl Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Field::String(s) => write!(f, "\"{}\"", s),
            Field::Number(n) => write!(f, "{}", n),
            Field::ParseError => write!(f, "Parse Error"),
        }
    }
}

macro_rules! field_try_into {
    ($type:ty, $string:stmt, $number:stmt) => {
        impl TryFrom<Field> for $type {
            type Error = ();

            fn try_from(field: Field) -> Result<$type, Self::Error> {
                match field {
                    Field::String(s) => return { $string }(s),
                    Field::Number(n) => return { $number }(n),
                    Field::ParseError => Err(()),
                }
            }
        }
    };
}

// mwuah
field_try_into! {f64, |s: String| s.parse::<f64>().map_err(|_| ()), |n| Ok(n)}
field_try_into! {f32, |s: String| s.parse::<f32>().map_err(|_| ()), |n| Ok(n as f32)}
field_try_into! {u64, |s: String| s.parse::<u64>().map_err(|_| ()), |n| Ok(n as u64)}
field_try_into! {u32, |s: String| s.parse::<u32>().map_err(|_| ()), |n| Ok(n as u32)}
field_try_into! {String, |s| Ok(s), |n: f64| Ok(n.to_string())}
field_try_into! {bool,
    |s: String| match s.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(())},
    |n: f64| {
        if n == 1.0 { Ok(true) }
        else
        if n == 0.0 { Ok(false)}
        else        { Err(())}
    }
}

macro_rules! field_from {
    ($type:ty, $from:stmt) => {
        impl From<$type> for Field {
            fn from(val: $type) -> Self {
                return { $from }(val);
            }
        }
    };
}

field_from! {f64, |v| Field::Number(v)}
field_from! {f32, |v| Field::Number(v as f64)}
field_from! {u64, |v| Field::Number(v as f64)}
field_from! {u32, |v| Field::Number(v as f64)}
field_from! {String, |v| Field::String(v)}
field_from! {bool, |v| Field::String(
    match v {
        true => "true".to_string(),
        false => "false".to_string(),
    }
)}

struct CharCursor<'a> {
    inner: std::str::CharIndices<'a>,
    cur: (usize, char),

    cur_line: usize,
    cur_column: usize,
}

impl<'a> CharCursor<'a> {
    fn new(string: &'a str) -> Self {
        let mut inner = string.char_indices();

        Self {
            cur: inner.next().unwrap_or((0, '\0')),
            inner,
            cur_line: 0,
            cur_column: 0,
        }
    }
    fn next(&mut self) -> (usize, char) {
        match self.cur.1 {
            '\n' => {
                self.cur_line += 1;
                self.cur_column = 0;
            }
            // null means end of buffer / end of the iterator
            // return early with the last value
            '\0' => return self.cur,
            _ => self.cur_column += 1,
        }

        if let Some(next) = self.inner.next() {
            self.cur = next;
        } else {
            self.cur.1 = '\0'
        }

        self.cur
    }
    fn current(&self) -> (usize, char) {
        self.cur
    }
    fn line(&self) -> usize {
        self.cur_line
    }
    fn col(&self) -> usize {
        self.cur_column
    }
    // consumes any caracters in array and stops on one that isn't in 'what'
    fn consume_all(&mut self, what: fn(char) -> bool) {
        loop {
            match self.current().1 {
                '\0' => break,
                other => {
                    if what(other) {
                        self.next();
                        continue;
                    }
                }
            }
            break;
        }
    }
    // consumes all characters until encountering the 'until' character
    fn consume_until(&mut self, until: char) {
        loop {
            match self.current().1 {
                '\0' => break,
                other => {
                    if other == until {
                        break;
                    }
                    self.next();
                }
            }
        }
    }
    fn err(&self, desc: String) -> ParseError {
        ParseError {
            line: self.cur_line,
            col: self.cur_column,
            desc,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ParseError {
    line: usize,
    col: usize,
    desc: String,
}

impl Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}:{} {}", self.line, self.col, self.desc)
    }
}

impl Error for ParseError {}

#[derive(Clone, Debug)]
pub enum SerializeError {
    InvalidField { field_name: String },
    FmtError(std::fmt::Error),
}

impl Display for SerializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SerializeError::InvalidField { field_name } => writeln!(
                f,
                "Tried serializing field '{}' which had an invalid value.",
                field_name
            ),
            SerializeError::FmtError(e) => e.fmt(f),
        }
    }
}

impl Error for SerializeError {}

#[derive(Clone, Debug)]
pub struct Settings {
    fields: HashMap<String, Field>,
}

impl Settings {
    pub fn new(string: &str) -> (Self, Vec<ParseError>) {
        let mut fields = HashMap::new();

        let mut errors = Vec::new();
        let mut cursor = CharCursor::new(string);

        macro_rules! err {
            ($simple:literal) => {
                errors.push(cursor.err($simple.to_owned()));
            };
            ($($arg:tt)*) => {{
                let desc = format!($($arg)*);
                errors.push(cursor.err(desc));
            }}
        }

        // # COMMENT \n
        // NAME = VALUE \n

        'statements: loop {
            // if true, the resulting name and field are not added to the hashmap
            // currently only because the name is invalid
            let mut skip = false;

            cursor.consume_all(|c| c.is_whitespace());

            match cursor.current().1 {
                '\0' => break 'statements,
                // COMMENT
                '#' => {
                    cursor.consume_until('\n');
                    continue;
                }
                _ => {}
            }

            // NAME - most unicode is allowed except whitespace, unicode control, punctuation (except '_' and '-')
            let name =
            // is a loop only so we can early break here in stable rust, see https://github.com/rust-lang/rust/issues/48594
            'name: loop {
                let name_start = cursor.current().0;
                let mut name_end = 0;
                let human_name_start = (cursor.line(), cursor.col());

                // this skips whitespace but we want to error on whitespace so it is done later on the full string 
                loop {

                    let current = cursor.current();

                    match current.1 {
                        '=' => break,
                        '\n' | '\0' => {
                            err!("Unexpected end of statement, maybe missing a '=' or comment with '#'");
                            cursor.next();
                            continue 'statements;
                        },
                        _ => {}
                    }

                    if !current.1.is_whitespace() {
                        name_end = current.0 + current.1.len_utf8();
                    }

                    cursor.next();
                }

                let slice = &string.as_bytes()[name_start..name_end];
                let string = std::str::from_utf8(slice).unwrap();

                for c in string.chars() {
                    if (c != '_' && c != '-') && (c.is_whitespace() || c.is_control() || c.is_ascii_punctuation()) {
                        errors.push(ParseError{
                            line: human_name_start.0,
                            col: human_name_start.1,
                            desc: format!("Encountered unexpected character '{}' while matching a name.", c),
                        });
                        // the name is invalid but we need the parser to be in the correct state after this (assuming the rest of the declaration is correct)
                        skip = true;
                        break 'name String::new();
                    }
                }

                break 'name string.to_owned();
            };

            // '='
            cursor.next();
            cursor.consume_all(|c| c.is_whitespace());

            // VALUE
            let value =
            // is a loop only so we can early break here in stable rust, see https://github.com/rust-lang/rust/issues/48594
            'value: loop {
                match cursor.current().1 {
                    // value must be a string, go until we find an unescaped '"'
                    '"' => {
                        let mut string = String::new();
                        loop {
                            match cursor.next().1 {
                                '\\' => {
                                    let escape = match cursor.next().1 {
                                    'n' => '\n',
                                    'r' => '\r',
                                    't' => '\t',
                                    '0' => '\0',
                                    '\\' => '\\',
                                    '\'' => '\'',
                                    '\"' => '\"',
                                    other => {
                                        err!("Unknown escape sequence '\\{}'.", other);
                                        break 'value Field::ParseError;
                                    }
                                };
                                string.push(escape);
                                },
                                '"' => {
                                    cursor.next();
                                    break;
                                },
                                '\n' | '\0' => {
                                    err!("Unclosed string.");
                                    break 'value Field::ParseError;
                                },
                                other => string.push(other),
                            }
                        }

                        break 'value Field::String(string);
                    },
                    '\n' | '\0' => {
                        err!("Expected value assignment to '{}'.", name);
                        break 'value Field::ParseError;
                    },
                    // value must be a number, go and accumulate value until we reach a '.' then accumulate backwards
                    _ => {
                        if !cursor.current().1.is_ascii_digit() && cursor.current().1 != '.' {
                            let start = cursor.current().0;
                            cursor.consume_until('\n');
                            let end = cursor.current().0;

                            let slice = &string.as_bytes()[start..end];
                            err!("Expected a number or a string while parsing a value, found '{}'.", std::str::from_utf8(slice).unwrap().trim_end());
                            break 'value Field::ParseError;
                        }

                        let mut number = 0.0;
                        'number: loop {
                            let current = cursor.current().1;
                            match current {
                                '.' => {
                                    let mut decimal_number = 0.0;
                                    let mut multiplier = 0.1;
                                    loop {
                                        let next = cursor.next().1;
                                        match next {
                                            '\n' | '#' | '\0' | _ if next.is_whitespace() => {
                                                number += decimal_number;
                                                break 'number;
                                            },
                                            other => {
                                                if !other.is_ascii_digit() {
                                                    err!("Expected an ascii digit while parsing the decimal part of a number, found '{}'.", other);
                                                    break 'value Field::ParseError;
                                                }
                                                decimal_number +=
                                                    other.to_digit(10).unwrap() as f64 * multiplier;
                                                multiplier *= 0.1;
                                            }
                                        }
                                    }
                                }
                                '\n' | '#' | '\0' | _ if current.is_whitespace() => break 'number,
                                other => {
                                    if !other.is_ascii_digit() {
                                        err!("Expected an ascii digit or '.' while parsing a number, found '{}'.", other);
                                        break 'value Field::ParseError;
                                    }

                                    number *= 10.0;
                                    number += other.to_digit(10).unwrap() as f64;
                                }
                            }
                            cursor.next();
                        }

                        break 'value Field::Number(number);
                    }
                }
            };

            if !skip {
                fields.insert(name, value);
            }
        }

        (Self { fields }, errors)
    }
    pub fn new_from_file(path: impl AsRef<Path>) -> std::io::Result<(Self, Vec<ParseError>)> {
        let string = std::fs::read_to_string(path)?;
        let new = Self::new(string.as_str());
        Ok(new)
    }
    pub fn serialize(&self) -> Result<String, SerializeError> {
        let mut output_string = String::new();
        for (name, value) in &self.fields {
            use std::fmt::Write;
            match value {
                Field::String(string) => writeln!(&mut output_string, "{} = \"{}\"", name, string)
                    .map_err(|e| SerializeError::FmtError(e))?,
                Field::Number(number) => writeln!(&mut output_string, "{} = {}", name, number)
                    .map_err(|e| SerializeError::FmtError(e))?,
                Field::ParseError => {
                    return Err(SerializeError::InvalidField {
                        field_name: name.clone(),
                    })
                }
            }
        }

        Ok(output_string)
    }
    pub fn get_hashmap(&self) -> &HashMap<String, Field> {
        &self.fields
    }
    pub fn get_hashmap_mut(&mut self) -> &mut HashMap<String, Field> {
        &mut self.fields
    }
    pub fn into_hashmap(self) -> HashMap<String, Field> {
        self.fields
    }
    pub fn get_field(&self, name: &str) -> Option<&Field> {
        self.fields.get(name)
    }
    pub fn get<T: TryFrom<Field>>(&self, name: &str) -> Option<T> {
        self.fields
            .get(name)
            .and_then(|f| TryFrom::try_from(f.clone()).ok())
    }
    pub fn set<T: Into<Field>>(&mut self, name: &str, t: T) {
        // to clone the name only if there is no previous entry we would need the unstable hash_raw_entry
        // https://github.com/rust-lang/rust/issues/56167
        self.fields.insert(name.to_string(), t.into());
    }
}

#[test]
fn correct() {
    let s = r#"
    #empty line
    
    #comment #another hash
    name = "aa"
    -a_ = 2
    shorhand_decimal = .1
    aaaaa = .1 # more edge cases >:
    c = 32.0
    0Bbia3 = "test"
    far     =       42
                offset =             0      # comment
    squished="" #comment on same line
    escaped="\n\r\t\0\'\""
    úňíčöďë = "yes"
    🍎 = "🚀"
    t̷̢̩͈̪̼͈̪̳͔̗̺̺̿͗̋̎̐̈́͌̋͜h̵̡͈̪͚͙̥̤͎̯̼̟̉̍͐̊̿̊͠͝i̶̘̠̬̬͓̗̬̠̬͇̮̞̥͆ś̷̢̛̈̋̃̍̎͆̈́̈̔͌̀̇͝_̶̛̦̘̩͚̦̯̣̮̼̲̩͌̿͑̿͛̔̋͐̀͜i̴̡̛͔̟̣̲̗̦̼͎̗͇͉̠͒͆̈́̈́̉̐́̾͊͊̋̚͘͠s̵̢̨̡̘̣̤̮̤̪̫̀̾̀̄̓̕̕_̴̢̧̲̗̘͍͍̞̱͚̞̦͌̒̓f̴̧̜͗͊̍̋̉̑̕͘͠į̶̫̞͇̭̦̝̼̖͓̥̦̇͊ņ̵̻͍͎̦̖̯̹̞̮̞̇̇̌̏̆̑͆̈́́̂͗͐͜e̴͇͊ = "this_is_fine" #any unicode identifier is allowed except whitespace, control, punctuation (except '_' and '-')
    "#;

    let (_settings, errors) = Settings::new(s);
    for e in &errors {
        println!("{}", e);
    }
    assert!(errors.is_empty());
}

#[test]
fn error() {
    let s = "invalid name with spaces = 1";
    let (_settings, errors) = Settings::new(s);
    assert!(!errors.is_empty());

    let s = "number_broke = 1.1.1";
    let (_settings, errors) = Settings::new(s);
    assert!(!errors.is_empty());

    let s = "forgot=";
    let (_settings, errors) = Settings::new(s);
    assert!(!errors.is_empty());

    let s = "forgot";
    let (_settings, errors) = Settings::new(s);
    assert!(!errors.is_empty());

    let s = "aaaaaaaaaaaaaa''2";
    let (_settings, errors) = Settings::new(s);
    assert!(!errors.is_empty());

    // zalgo with whitespace
    let s = "ẗ̷̜̳͚́͋̃h̵͇̞̩̟̫̅͜i̵̛̘̳̻̍́s̷̢̞͙̿̓͌̉̇̓͑͘ ̵̦̜̌̈́̈́͒̉̒͐̇i̵̯͆͌ś̵̢̛̗̣̜͇͚̼̺̗̳̘͙̲̮̰̈́̅̉̒̑̀͗̈́́̍̊͊ ̵̨̝̮̗͕̽̌̚͝f̵̦̼͎͒̀̈̆̇̌́̚͝ị̷̡̢̧̰̳̲̼̝̺̠̙͓̞̐́͆̌̎̚ņ̶̫̭̘̯̪̰̥͐̃̔̀̌͒̔̈́͂̈͒̈́͜͠͝e̸̜̣̦̲̫͕̎̋̽͒ = \"this is fine\"";
    let (_settings, errors) = Settings::new(s);
    assert!(!errors.is_empty());
}
