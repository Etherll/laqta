//! File-name patterns.

use std::fmt;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

/// Candidate paths in order: `<dir>/<yyyy-mm>/Laqta_<yyyy-mm-dd_HH-mm-ss>.png`, then `_2`, `_3`, …
/// before `.png` (up to 999). Pure: it never checks the file system.
pub fn capture_candidates(dir: &Path, now: &jiff::civil::DateTime) -> impl Iterator<Item = PathBuf> {
    let folder = dir.join(format!("{:04}-{:02}", now.year(), now.month()));
    let stem =
        format!("Laqta_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", now.year(), now.month(), now.day(), now.hour(), now.minute(), now.second());
    (1u32..=999).map(move |n| if n == 1 { folder.join(format!("{stem}.png")) } else { folder.join(format!("{stem}_{n}.png")) })
}

/// Default screenshot file name, without the extension.
pub const DEFAULT_PATTERN: &str = "Laqta_%y-%mo-%d_%h-%mi-%s";
/// Default subfolder under the screenshots directory.
pub const DEFAULT_SUBFOLDER_PATTERN: &str = "%y-%mo";

/// Longest file name `file_name` returns, in characters.
const MAX_NAME_CHARS: usize = 200;

/// Everything a name pattern can read. The caller fills it in: nothing here reads the clock, the
/// environment or the file system.
pub struct NameContext<'a> {
    pub now: jiff::civil::DateTime,
    pub unix_seconds: i64,
    pub window_title: &'a str,
    pub process_name: &'a str,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Value of the `%i` family.
    pub counter: u64,
    pub user_name: &'a str,
    pub user_login: &'a str,
    pub computer_name: &'a str,
    /// Reads the lines of the file a `%rf{path}` names. The caller does the reading, so this module
    /// stays free of I/O. An empty result draws nothing.
    pub read_lines: &'a dyn Fn(&str) -> Vec<String>,
    /// Random source for the `%r…` tokens and `%guid`.
    pub rng: &'a mut dyn FnMut() -> u64,
}

impl fmt::Debug for NameContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NameContext")
            .field("now", &self.now)
            .field("unix_seconds", &self.unix_seconds)
            .field("counter", &self.counter)
            .finish_non_exhaustive()
    }
}

/// The file paths the `%rf{path}` tokens in `pattern` name, in order, without repeats. The caller
/// reads them and answers `NameContext::read_lines` from what it read.
pub fn random_line_files(pattern: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut rest = pattern;
    while let Some(at) = rest.find("%rf{") {
        let after = &rest[at + 4..];
        let Some(end) = after.find('}') else { break };
        let path = after[..end].to_owned();
        if !path.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
        rest = &after[end + 1..];
    }
    paths
}

/// Expands the `%` tokens in `pattern`. The longest token name at each `%` wins, and a `%` that
/// starts no known token stays literal. A `{…}` right after a token is its argument when the token
/// takes one and the contents are valid: a number in range (`%rn{3}`, `%i{4}`), a base and an
/// optional pad width (`%ib{16,4}`), or a path (`%rf{names.txt}`). Otherwise the `{` stays literal.
/// A `%pm` anywhere in the pattern makes `%h` 12-hour.
pub fn expand(pattern: &str, ctx: &mut NameContext<'_>) -> String {
    let twelve_hour = pattern.contains("%pm");
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let body = &rest[at + 1..];
        let Some((name, token)) = longest_token(body) else {
            out.push('%');
            rest = body;
            continue;
        };
        let tail = &body[name.len()..];
        let (arg, after) = match read_argument(token, tail) {
            Some((arg, used)) => (arg, &tail[used..]),
            None => (Argument::Absent, tail),
        };
        out.push_str(&render(token, arg, twelve_hour, ctx));
        rest = after;
    }
    out.push_str(rest);
    out
}

/// [`expand`], then made safe as a file name: `\ / : * ? " < > |` and control characters become
/// `_`, the result is cut to 200 characters, trailing dots and spaces are dropped, and an empty
/// result becomes `Laqta`.
pub fn file_name(pattern: &str, ctx: &mut NameContext<'_>) -> String {
    let expanded = expand(pattern, ctx);
    let safe: String =
        expanded.chars().map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c }).take(MAX_NAME_CHARS).collect();
    let trimmed = safe.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() { "Laqta".to_owned() } else { trimmed.to_owned() }
}

/// Candidate paths in order: `<dir>/<sub>/<stem>.<ext>`, then `_2`, `_3`, … up to 999. An empty
/// `sub` means no subfolder. Pure: it never checks the file system.
pub fn candidates(dir: &Path, sub: &str, stem: &str, ext: &str) -> impl Iterator<Item = PathBuf> {
    let folder = if sub.is_empty() { dir.to_path_buf() } else { dir.join(sub) };
    let stem = stem.to_owned();
    let ext = ext.to_owned();
    (1u32..=999).map(move |n| if n == 1 { folder.join(format!("{stem}.{ext}")) } else { folder.join(format!("{stem}_{n}.{ext}")) })
}

#[derive(Clone, Copy)]
enum Token {
    Title,
    ProcessName,
    Year,
    Year2,
    Month,
    MonthName,
    Weekday,
    IsoWeek,
    Day,
    Hour,
    Minute,
    Second,
    Millisecond,
    Meridiem,
    Unix,
    Counter,
    CounterBase36,
    CounterBase36Upper,
    CounterBase62,
    CounterBase62Inverse,
    CounterBase,
    CounterBaseAlnum,
    CounterHex,
    CounterHexUpper,
    RandomDigit,
    RandomAlnum,
    RandomUnambiguous,
    RandomHex,
    RandomHexUpper,
    Guid,
    GuidUpper,
    Adjective,
    Animal,
    Emoji,
    FileLine,
    Width,
    Height,
    UserName,
    UserLogin,
    ComputerName,
    Newline,
}

/// Token names (without the `%`) and what they expand to.
const TOKENS: &[(&str, Token)] = &[
    ("t", Token::Title),
    ("pn", Token::ProcessName),
    ("y", Token::Year),
    ("yy", Token::Year2),
    ("mo", Token::Month),
    ("mon", Token::MonthName),
    ("mon2", Token::MonthName),
    ("w", Token::Weekday),
    ("w2", Token::Weekday),
    ("wy", Token::IsoWeek),
    ("d", Token::Day),
    ("h", Token::Hour),
    ("mi", Token::Minute),
    ("s", Token::Second),
    ("ms", Token::Millisecond),
    ("pm", Token::Meridiem),
    ("unix", Token::Unix),
    ("i", Token::Counter),
    ("ia", Token::CounterBase36),
    ("iA", Token::CounterBase36Upper),
    ("iAa", Token::CounterBase62),
    ("iaA", Token::CounterBase62Inverse),
    ("ib", Token::CounterBase),
    ("iB", Token::CounterBaseAlnum),
    ("ix", Token::CounterHex),
    ("iX", Token::CounterHexUpper),
    ("rn", Token::RandomDigit),
    ("ra", Token::RandomAlnum),
    ("rna", Token::RandomUnambiguous),
    ("rx", Token::RandomHex),
    ("rX", Token::RandomHexUpper),
    ("guid", Token::Guid),
    ("GUID", Token::GuidUpper),
    ("radjective", Token::Adjective),
    ("ranimal", Token::Animal),
    ("remoji", Token::Emoji),
    ("rf", Token::FileLine),
    ("width", Token::Width),
    ("height", Token::Height),
    ("un", Token::UserName),
    ("uln", Token::UserLogin),
    ("cn", Token::ComputerName),
    ("n", Token::Newline),
];

const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

const ADJECTIVES: &[&str] = &[
    "Brave", "Calm", "Clever", "Cosmic", "Curious", "Daring", "Eager", "Fancy", "Gentle", "Golden", "Happy", "Jolly", "Kind", "Lively",
    "Lucky", "Mighty", "Modest", "Nimble", "Noble", "Patient", "Peaceful", "Playful", "Proud", "Quick", "Quiet", "Radiant", "Rapid",
    "Rustic", "Serene", "Shiny", "Silent", "Smooth", "Snappy", "Solid", "Sunny", "Swift", "Tidy", "Tranquil", "Vivid", "Warm", "Wise",
    "Witty", "Zesty",
];

const ANIMALS: &[&str] = &[
    "Otter", "Falcon", "Badger", "Heron", "Lynx", "Panda", "Koala", "Gecko", "Ibex", "Marmot", "Narwhal", "Ocelot", "Penguin", "Quokka",
    "Raven", "Salmon", "Tiger", "Walrus", "Yak", "Zebra", "Bison", "Camel", "Dolphin", "Eagle", "Ferret", "Gazelle", "Hedgehog", "Iguana",
    "Jaguar", "Kiwi", "Lemur", "Moose", "Newt", "Owl", "Puffin", "Rhino", "Seal", "Toucan", "Vulture", "Wombat",
];

const EMOJI: &[&str] = &[
    "😀", "😃", "😄", "😁", "😎", "🤓", "🙂", "😺", "🐶", "🐱", "🦊", "🐻", "🐼", "🐨", "🐸", "🐵", "🦉", "🐧", "🐢", "🌟", "🌈", "🍀",
    "🌸", "🌻", "🍎", "🍋", "🍕", "🚀", "🎈", "🎉", "🎯", "🎲", "🎵",
];

/// ShareX's `Alphanumeric`: digits, then upper case, then lower case.
const ALPHANUMERIC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// ShareX's `AlphanumericInverse`: digits, then lower case, then upper case.
const ALPHANUMERIC_INVERSE: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DECIMAL: &[u8] = b"0123456789";
const HEX: &[u8] = b"0123456789abcdef";
const HEX_UPPER: &[u8] = b"0123456789ABCDEF";
/// Letters and digits without the look-alikes I, l, O, o, 0 and 1.
const UNAMBIGUOUS: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ";

/// What the `{…}` after a token holds.
#[derive(Clone, Copy)]
enum Argument<'s> {
    /// No `{…}`, or one the token does not take.
    Absent,
    /// A repeat count for the random tokens, a zero-pad width for the counters.
    Count(u32),
    /// `%ib{base}` or `%ib{base,digits}`: a base from 2 to 62, and the zero-pad width when given.
    Base { base: u32, digits: Option<u32> },
    /// The file `%rf{path}` reads.
    Path(&'s str),
}

impl Argument<'_> {
    fn count(self) -> Option<u32> {
        match self {
            Argument::Count(n) => Some(n),
            _ => None,
        }
    }
}

/// The tokens whose `{N}` is one number, and the values it accepts: a repeat count for the random
/// tokens, a zero-pad width for the counters.
fn argument_range(token: Token) -> Option<RangeInclusive<u32>> {
    match token {
        Token::RandomDigit
        | Token::RandomAlnum
        | Token::RandomUnambiguous
        | Token::RandomHex
        | Token::RandomHexUpper
        | Token::Counter
        | Token::CounterBase36
        | Token::CounterBase36Upper
        | Token::CounterBase62
        | Token::CounterBase62Inverse
        | Token::CounterHex
        | Token::CounterHexUpper => Some(1..=100),
        _ => None,
    }
}

/// The longest token name that `body` (the text after a `%`) starts with.
fn longest_token(body: &str) -> Option<(&'static str, Token)> {
    TOKENS.iter().copied().filter(|&(name, _)| body.starts_with(name)).max_by_key(|&(name, _)| name.len())
}

/// `s` as a number in `range`, when it is only digits and in range.
fn number_in(s: &str, range: RangeInclusive<u32>) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: u32 = s.parse().ok()?;
    range.contains(&value).then_some(value)
}

/// The `{…}` at the start of `s`, when `token` takes one and its contents are valid: the argument
/// and the number of bytes it spans.
fn read_argument(token: Token, s: &str) -> Option<(Argument<'_>, usize)> {
    let body = s.strip_prefix('{')?;
    let end = body.find('}')?;
    let inner = &body[..end];
    let arg = match token {
        Token::FileLine if !inner.is_empty() => Argument::Path(inner),
        Token::CounterBase | Token::CounterBaseAlnum => match inner.split_once(',') {
            Some((base, digits)) => Argument::Base { base: number_in(base, 2..=62)?, digits: Some(number_in(digits, 1..=100)?) },
            None => Argument::Base { base: number_in(inner, 2..=62)?, digits: None },
        },
        _ => Argument::Count(number_in(inner, argument_range(token)?)?),
    };
    Some((arg, end + 2))
}

fn render(token: Token, arg: Argument<'_>, twelve_hour: bool, ctx: &mut NameContext<'_>) -> String {
    let now = ctx.now;
    let repeat = arg.count().unwrap_or(1);
    match token {
        Token::Title => ctx.window_title.to_owned(),
        Token::ProcessName => ctx.process_name.to_owned(),
        Token::Year => format!("{:04}", now.year()),
        Token::Year2 => format!("{:02}", now.year().rem_euclid(100)),
        Token::Month => format!("{:02}", now.month()),
        Token::MonthName => MONTHS[month_index(now)].to_owned(),
        Token::Weekday => WEEKDAYS[weekday_index(now)].to_owned(),
        Token::IsoWeek => format!("{:02}", now.iso_week_date().week()),
        Token::Day => format!("{:02}", now.day()),
        Token::Hour => {
            let hour = if twelve_hour {
                match now.hour() % 12 {
                    0 => 12,
                    h => h,
                }
            } else {
                now.hour()
            };
            format!("{hour:02}")
        }
        Token::Minute => format!("{:02}", now.minute()),
        Token::Second => format!("{:02}", now.second()),
        Token::Millisecond => format!("{:03}", now.subsec_nanosecond() / 1_000_000),
        Token::Meridiem => String::from(if now.hour() < 12 { "AM" } else { "PM" }),
        Token::Unix => ctx.unix_seconds.to_string(),
        Token::Counter => pad(radix(ctx.counter, DECIMAL), arg.count()),
        Token::CounterBase36 => pad(radix(ctx.counter, &ALPHANUMERIC_INVERSE[..36]), arg.count()),
        Token::CounterBase36Upper => pad(radix(ctx.counter, &ALPHANUMERIC[..36]), arg.count()),
        Token::CounterBase62 => pad(radix(ctx.counter, ALPHANUMERIC), arg.count()),
        Token::CounterBase62Inverse => pad(radix(ctx.counter, ALPHANUMERIC_INVERSE), arg.count()),
        Token::CounterBase => based(ctx.counter, ALPHANUMERIC_INVERSE, arg),
        Token::CounterBaseAlnum => based(ctx.counter, ALPHANUMERIC, arg),
        Token::CounterHex => pad(radix(ctx.counter, HEX), arg.count()),
        Token::CounterHexUpper => pad(radix(ctx.counter, HEX_UPPER), arg.count()),
        Token::RandomDigit => random_chars(ctx, DECIMAL, repeat),
        Token::RandomAlnum => random_chars(ctx, ALPHANUMERIC_INVERSE, repeat),
        Token::RandomUnambiguous => random_chars(ctx, UNAMBIGUOUS, repeat),
        Token::RandomHex => random_chars(ctx, HEX, repeat),
        Token::RandomHexUpper => random_chars(ctx, HEX_UPPER, repeat),
        Token::Guid => guid(ctx, HEX),
        Token::GuidUpper => guid(ctx, HEX_UPPER),
        Token::Adjective => pick(ctx, ADJECTIVES).to_owned(),
        Token::Animal => pick(ctx, ANIMALS).to_owned(),
        Token::Emoji => pick(ctx, EMOJI).to_owned(),
        Token::FileLine => match arg {
            Argument::Path(path) => {
                let lines = (ctx.read_lines)(path);
                if lines.is_empty() { String::new() } else { lines[pick_index(ctx, lines.len())].clone() }
            }
            _ => String::from("%rf"),
        },
        Token::Width => ctx.width.map_or_else(String::new, |w| w.to_string()),
        Token::Height => ctx.height.map_or_else(String::new, |h| h.to_string()),
        Token::UserName => ctx.user_name.to_owned(),
        Token::UserLogin => ctx.user_login.to_owned(),
        Token::ComputerName => ctx.computer_name.to_owned(),
        Token::Newline => String::from("\n"),
    }
}

fn month_index(now: jiff::civil::DateTime) -> usize {
    (now.month() - 1) as usize
}

/// Monday is 0.
fn weekday_index(now: jiff::civil::DateTime) -> usize {
    now.weekday().to_monday_zero_offset() as usize
}

/// `n` written with the symbols of `alphabet`, one per digit, so its length is the base.
fn radix(mut n: u64, alphabet: &[u8]) -> String {
    let base = alphabet.len() as u64;
    let mut digits = Vec::new();
    loop {
        digits.push(char::from(alphabet[(n % base) as usize]));
        n /= base;
        if n == 0 {
            break;
        }
    }
    digits.iter().rev().collect()
}

/// `%ib` and `%iB`: the counter in the base the argument names (10 without one), zero-padded to
/// the argument's digits. `alphabet` gives the digit symbols in order.
fn based(counter: u64, alphabet: &[u8], arg: Argument<'_>) -> String {
    let (base, digits) = match arg {
        Argument::Base { base, digits } => (base, digits),
        _ => (10, None),
    };
    pad(radix(counter, &alphabet[..base as usize]), digits)
}

/// Left-pads `digits` with zeros to the `{N}` width, when there is one.
fn pad(digits: String, width: Option<u32>) -> String {
    match width {
        Some(w) => format!("{digits:0>width$}", width = w as usize),
        None => digits,
    }
}

fn random_chars(ctx: &mut NameContext<'_>, alphabet: &[u8], count: u32) -> String {
    (0..count).map(|_| char::from(alphabet[pick_index(ctx, alphabet.len())])).collect()
}

/// A random version-4 GUID, with the hex digits of `alphabet`: lowercase or uppercase.
fn guid(ctx: &mut NameContext<'_>, alphabet: &[u8]) -> String {
    let mut bytes = [0u8; 16];
    let (first, second) = bytes.split_at_mut(8);
    first.copy_from_slice(&(ctx.rng)().to_be_bytes());
    second.copy_from_slice(&(ctx.rng)().to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String =
        bytes.iter().flat_map(|&b| [alphabet[usize::from(b >> 4)], alphabet[usize::from(b & 0x0f)]]).map(char::from).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

fn pick(ctx: &mut NameContext<'_>, items: &[&'static str]) -> &'static str {
    items[pick_index(ctx, items.len())]
}

/// An index below `len`, which must be non-zero.
fn pick_index(ctx: &mut NameContext<'_>, len: usize) -> usize {
    ((ctx.rng)() % len as u64) as usize
}

/// What the upload-info tokens (`$url` and the rest) read. An empty string is an absent value, so
/// `$shorturl` falls back to `url` when nothing was shortened.
#[derive(Clone, Debug)]
pub struct UploadInfo<'a> {
    pub url: &'a str,
    pub short_url: &'a str,
    pub thumbnail_url: &'a str,
    pub deletion_url: &'a str,
    /// The raw response body the host returned.
    pub result: &'a str,
    pub file_path: Option<&'a Path>,
    pub file_name: &'a str,
    pub thumbnail_path: Option<&'a Path>,
    pub upload_time: jiff::civil::DateTime,
}

/// The upload-info token names, without the `$`. `upload_info` takes the longest one that matches.
const UPLOAD_TOKENS: [&str; 13] = [
    "url",
    "shorturl",
    "thumbnailurl",
    "deletionurl",
    "result",
    "filename",
    "filenamenoext",
    "filepath",
    "folderpath",
    "foldername",
    "thumbnailfilename",
    "thumbnailfilenamenoext",
    "uploadtime",
];

/// Expands the upload-info tokens in `template` in one pass, so a value that contains `$url` is never
/// expanded again. A `$` that starts no token stays literal. Pure: it reads nothing but `info`.
pub fn upload_info(template: &str, info: &UploadInfo<'_>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let body = &rest[at + 1..];
        match UPLOAD_TOKENS.iter().copied().filter(|token| body.starts_with(*token)).max_by_key(|token| token.len()) {
            Some(token) => {
                out.push_str(&upload_value(token, info));
                rest = &body[token.len()..];
            }
            None => {
                out.push('$');
                rest = body;
            }
        }
    }
    out.push_str(rest);
    out
}

fn upload_value(token: &str, info: &UploadInfo<'_>) -> String {
    match token {
        "url" => info.url.to_owned(),
        "shorturl" => {
            if info.short_url.is_empty() {
                info.url.to_owned()
            } else {
                info.short_url.to_owned()
            }
        }
        "thumbnailurl" => info.thumbnail_url.to_owned(),
        "deletionurl" => info.deletion_url.to_owned(),
        "result" => info.result.to_owned(),
        "filename" => info.file_name.to_owned(),
        "filenamenoext" => name_part(Some(Path::new(info.file_name)), true),
        "filepath" => text_part(info.file_path),
        "folderpath" => text_part(info.file_path.and_then(Path::parent)),
        "foldername" => name_part(info.file_path.and_then(Path::parent), false),
        "thumbnailfilename" => name_part(info.thumbnail_path, false),
        "thumbnailfilenamenoext" => name_part(info.thumbnail_path, true),
        "uploadtime" => time_text(info.upload_time),
        _ => String::new(),
    }
}

/// The last component of `path`, or its stem without the extension. Empty when there is no path.
fn name_part(path: Option<&Path>, stem: bool) -> String {
    let Some(path) = path else { return String::new() };
    let part = if stem { path.file_stem() } else { path.file_name() };
    part.map_or_else(String::new, |part| part.to_string_lossy().into_owned())
}

fn text_part(path: Option<&Path>) -> String {
    path.map_or_else(String::new, |path| path.display().to_string())
}

/// `yyyy-MM-dd HH:mm:ss`, the form `$uploadtime` takes.
fn time_text(time: jiff::civil::DateTime) -> String {
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", time.year(), time.month(), time.day(), time.hour(), time.minute(), time.second())
}

#[cfg(test)]
mod upload_info_tests {
    use super::*;

    fn info<'a>(file_path: Option<&'a Path>, thumbnail_path: Option<&'a Path>) -> UploadInfo<'a> {
        UploadInfo {
            url: "https://h.example/a1",
            short_url: "",
            thumbnail_url: "https://h.example/t1",
            deletion_url: "https://h.example/d1",
            result: "{\"ok\":true}",
            file_path,
            file_name: "Laqta_2026-11-07_14-03-22.png",
            thumbnail_path,
            upload_time: jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0),
        }
    }

    #[test]
    fn every_token_takes_its_value() {
        let file = Path::new("/shots/2026-11/Laqta_2026-11-07_14-03-22.png");
        let thumb = Path::new("/shots/2026-11/Laqta_2026-11-07_14-03-22-thumbnail.png");
        let mut value = info(Some(file), Some(thumb));
        value.short_url = "https://h.example/s";
        let template = "$url|$shorturl|$thumbnailurl|$deletionurl|$result|$filename|$filenamenoext|$filepath|$folderpath|$foldername|$thumbnailfilename|$thumbnailfilenamenoext|$uploadtime";
        assert_eq!(
            upload_info(template, &value),
            "https://h.example/a1|https://h.example/s|https://h.example/t1|https://h.example/d1|{\"ok\":true}|Laqta_2026-11-07_14-03-22.png|Laqta_2026-11-07_14-03-22|/shots/2026-11/Laqta_2026-11-07_14-03-22.png|/shots/2026-11|2026-11|Laqta_2026-11-07_14-03-22-thumbnail.png|Laqta_2026-11-07_14-03-22-thumbnail|2026-11-07 14:03:22"
        );
    }

    #[test]
    fn longest_name_wins() {
        let file = Path::new("/shots/a.png");
        let thumb = Path::new("/shots/a-thumbnail.png");
        let value = info(Some(file), Some(thumb));
        assert_eq!(upload_info("$thumbnailfilenamenoext", &value), "a-thumbnail");
        assert_eq!(upload_info("$thumbnailfilename", &value), "a-thumbnail.png");
        assert_eq!(upload_info("$filenamenoext", &value), "Laqta_2026-11-07_14-03-22");
        assert_eq!(upload_info("$shorturl$url", &value), "https://h.example/a1https://h.example/a1");
    }

    #[test]
    fn shorturl_falls_back_to_url_when_empty() {
        let value = info(None, None);
        assert_eq!(upload_info("$shorturl", &value), "https://h.example/a1");
    }

    #[test]
    fn absent_paths_are_empty() {
        let value = info(None, None);
        assert_eq!(upload_info("[$filepath|$folderpath|$foldername|$thumbnailfilename]", &value), "[|||]");
    }

    #[test]
    fn unknown_dollar_stays_literal() {
        let value = info(None, None);
        assert_eq!(upload_info("$nope $ $urlx", &value), "$nope $ https://h.example/a1x");
        assert_eq!(upload_info("cost $", &value), "cost $");
    }

    #[test]
    fn values_are_not_expanded_again() {
        let mut value = info(None, None);
        value.url = "https://h.example/?q=$url";
        assert_eq!(upload_info("$url", &value), "https://h.example/?q=$url");
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn random_line_files_lists_each_rf_path_once() {
        assert_eq!(random_line_files("a%rf{x.txt}b%rf{y.txt}%rf{x.txt}%rf{}%rf{open"), vec!["x.txt".to_string(), "y.txt".to_string()]);
    }
    use super::*;

    /// 2026-11-07 14:03:22.045: a Saturday in ISO week 45.
    fn instant() -> jiff::civil::DateTime {
        jiff::civil::date(2026, 11, 7).at(14, 3, 22, 45_000_000)
    }

    fn at_hour(hour: i8) -> jiff::civil::DateTime {
        jiff::civil::date(2026, 11, 7).at(hour, 3, 22, 0)
    }

    /// Random source that returns 1, 2, 3, …
    fn counting() -> impl FnMut() -> u64 {
        let mut n = 0;
        move || {
            n += 1;
            n
        }
    }

    /// Every file reads as empty.
    fn no_files(_: &str) -> Vec<String> {
        Vec::new()
    }

    fn fixture<'a>(
        now: jiff::civil::DateTime,
        read_lines: &'a dyn Fn(&str) -> Vec<String>,
        rng: &'a mut dyn FnMut() -> u64,
    ) -> NameContext<'a> {
        NameContext {
            now,
            unix_seconds: 1_794_060_202,
            window_title: "Editor - notes.txt",
            process_name: "code",
            width: Some(1920),
            height: Some(1080),
            counter: 255,
            user_name: "Ada Lovelace",
            user_login: "ada",
            computer_name: "WORKSTATION",
            read_lines,
            rng,
        }
    }

    /// `pattern` at `instant()`, with counting random numbers and no file lines.
    fn run(pattern: &str) -> String {
        run_at(instant(), pattern)
    }

    fn run_at(now: jiff::civil::DateTime, pattern: &str) -> String {
        let mut rng = counting();
        let read = no_files;
        let mut ctx = fixture(now, &read, &mut rng);
        expand(pattern, &mut ctx)
    }

    fn run_with_lines(pattern: &str, lines: &[String]) -> String {
        let read = |_: &str| lines.to_vec();
        let mut rng = counting();
        let mut ctx = fixture(instant(), &read, &mut rng);
        expand(pattern, &mut ctx)
    }

    fn file(pattern: &str) -> String {
        let mut rng = counting();
        let read = no_files;
        let mut ctx = fixture(instant(), &read, &mut rng);
        file_name(pattern, &mut ctx)
    }

    #[test]
    fn token_table_is_the_43_documented_tokens() {
        let mut names: Vec<&str> = TOKENS.iter().map(|&(name, _)| name).collect();
        assert_eq!(names.len(), 43);
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 43);
    }

    #[test]
    fn date_and_time_tokens() {
        let cases = [
            ("%y", "2026"),
            ("%yy", "26"),
            ("%mo", "11"),
            ("%mon", "November"),
            ("%mon2", "November"),
            ("%w", "Saturday"),
            ("%w2", "Saturday"),
            ("%wy", "45"),
            ("%d", "07"),
            ("%h", "14"),
            ("%mi", "03"),
            ("%s", "22"),
            ("%ms", "045"),
            ("%pm", "PM"),
            ("%unix", "1794060202"),
        ];
        for (pattern, want) in cases {
            assert_eq!(run(pattern), want, "{pattern}");
        }
    }

    #[test]
    fn twelve_hour_clock_needs_the_pm_token() {
        assert_eq!(run_at(at_hour(14), "%h %pm"), "02 PM");
        assert_eq!(run_at(at_hour(0), "%h %pm"), "12 AM");
        assert_eq!(run_at(at_hour(12), "%h %pm"), "12 PM");
        assert_eq!(run_at(at_hour(9), "%h %pm"), "09 AM");
        assert_eq!(run_at(at_hour(0), "%h"), "00");
        assert_eq!(run_at(at_hour(14), "%h"), "14");
    }

    #[test]
    fn longest_token_wins() {
        assert_eq!(run("%mon2"), "November");
        assert_eq!(run("%mon"), "November");
        assert_eq!(run("%mo"), "11");
        assert_eq!(run("%iAa"), "47");
        assert_eq!(run("%ia"), "73");
        assert_eq!(run("%i"), "255");
        assert_eq!(run("%unix"), "1794060202");
        assert_eq!(run("%un"), "Ada Lovelace");
        assert_eq!(run("%uln"), "ada");
        assert_eq!(run("%wy"), "45");
        assert_eq!(run("%w2"), "Saturday");
        assert_eq!(run("%w"), "Saturday");
        assert_eq!(run("%width"), "1920");
        assert_eq!(run("%h"), "14");
        assert_eq!(run("%yy"), "26");
        assert_eq!(run("%y"), "2026");
        assert_eq!(run("%ms"), "045");
        assert_eq!(run("%mi"), "03");
        assert_eq!(run("%pn"), "code");
        assert_eq!(run("%pm"), "PM");
        assert_eq!(run("%rna"), "3");
        assert_eq!(run("%ra"), "1");
        assert_eq!(run("%rn"), "1");
        assert_eq!(run("%rx"), "1");
        assert_eq!(run("%radjective"), ADJECTIVES[1]);
        assert_eq!(run("%ranimal"), ANIMALS[1]);
        assert_eq!(run("%remoji"), EMOJI[1]);
    }

    #[test]
    fn window_machine_and_newline_tokens() {
        assert_eq!(run("%t"), "Editor - notes.txt");
        assert_eq!(run("%pn"), "code");
        assert_eq!(run("%width x %height"), "1920 x 1080");
        assert_eq!(run("%un|%uln|%cn"), "Ada Lovelace|ada|WORKSTATION");
        assert_eq!(run("a%nb"), "a\nb");
        let read = no_files;
        let mut rng = counting();
        let mut ctx = fixture(instant(), &read, &mut rng);
        ctx.width = None;
        assert_eq!(expand("[%width]", &mut ctx), "[]");
    }

    #[test]
    fn guid_is_a_version_4_guid_from_the_random_source() {
        assert_eq!(run("%guid"), "00000000-0000-4001-8000-000000000002");
    }

    #[test]
    fn upper_case_variants_use_the_upper_case_alphabet() {
        assert_eq!(run("%iX"), "FF");
        assert_eq!(run("%iX{4}"), "00FF");
        assert_eq!(run("%iA"), "73");
        assert_eq!(run("%iA{3}"), "073");
        assert_eq!(run("%iaA"), "47");
        assert_eq!(run("%iB"), "255");
        assert_eq!(run("%iB{62}"), "47");
        assert_eq!(run("%iB{16,4}"), "00FF");
    }

    #[test]
    fn random_hex_and_guid_follow_the_case() {
        // Every draw is all ones, so each digit is the top of its alphabet.
        let read = no_files;
        let mut all_ones = || u64::MAX;
        let mut ctx = fixture(instant(), &read, &mut all_ones);
        assert_eq!(
            expand("%rx %rX %rX{3} %guid %GUID", &mut ctx),
            "f F FFF ffffffff-ffff-4fff-bfff-ffffffffffff FFFFFFFF-FFFF-4FFF-BFFF-FFFFFFFFFFFF"
        );
    }

    #[test]
    fn file_line_is_drawn_only_when_there_are_lines() {
        let lines = vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()];
        assert_eq!(run_with_lines("%rf{names.txt}", &lines), "beta");
        assert_eq!(run_with_lines("%rf{names.txt}", &[]), "");
        // An empty list draws nothing, so the next random token gets the first value.
        assert_eq!(run_with_lines("%rf{names.txt}%rn", &[]), "1");
        // Without a path `%rf` is not a token: it stays as written.
        assert_eq!(run_with_lines("%rf", &lines), "%rf");
    }

    #[test]
    fn file_line_reads_the_named_path() {
        let read = |path: &str| -> Vec<String> { if path == "names.txt" { vec!["alpha".to_owned()] } else { Vec::new() } };
        let mut rng = counting();
        let mut ctx = fixture(instant(), &read, &mut rng);
        assert_eq!(expand("[%rf{names.txt}]", &mut ctx), "[alpha]");
        assert_eq!(expand("[%rf{other.txt}]", &mut ctx), "[]");
        assert_eq!(expand("%rf{}", &mut ctx), "%rf{}");
    }

    #[test]
    fn counter_arguments_pad_and_set_the_base() {
        assert_eq!(run("%i{5}"), "00255");
        assert_eq!(run("%ia{4}"), "0073");
        assert_eq!(run("%iAa{3}"), "047");
        assert_eq!(run("%ix"), "ff");
        assert_eq!(run("%ix{4}"), "00ff");
        assert_eq!(run("%ib"), "255");
        assert_eq!(run("%ib{2}"), "11111111");
        assert_eq!(run("%ib{16}"), "ff");
        assert_eq!(run("%ib{36}"), "73");
        assert_eq!(run("%ib{62}"), "47");
        assert_eq!(run("%ib{16,4}"), "00ff");
        assert_eq!(run("%iB{62}"), "47");
    }

    #[test]
    fn random_arguments_repeat_the_token() {
        assert_eq!(run("%rn{3}"), "123");
        assert_eq!(run("%rn{3}x"), "123x");
        assert_eq!(run("%ra{4}"), "1234");
        assert_eq!(run("%rna{3}"), "345");
        assert_eq!(run("%rx{2}"), "12");
    }

    #[test]
    fn invalid_arguments_stay_literal() {
        assert_eq!(run("%rn{0}"), "1{0}");
        assert_eq!(run("%rn{101}"), "1{101}");
        assert_eq!(run("%rn{x}"), "1{x}");
        assert_eq!(run("%rn{3"), "1{3");
        assert_eq!(run("%i{}"), "255{}");
        assert_eq!(run("%i{101}"), "255{101}");
        assert_eq!(run("%i{99999999999}"), "255{99999999999}");
        assert_eq!(run("%ib{1}"), "255{1}");
        assert_eq!(run("%ib{63}"), "255{63}");
        assert_eq!(run("%ib{16,0}"), "255{16,0}");
        assert_eq!(run("%ib{16,101}"), "255{16,101}");
        assert_eq!(run("%ib{,4}"), "255{,4}");
        assert_eq!(run("%iB{1}"), "255{1}");
        // Tokens without an argument never take a `{N}`.
        assert_eq!(run("%h{3}"), "14{3}");
        assert_eq!(run("%radjective{3}"), "Calm{3}");
    }

    #[test]
    fn unknown_percent_stays_literal() {
        assert_eq!(run("%q"), "%q");
        assert_eq!(run("100%"), "100%");
        assert_eq!(run("%%"), "%%");
        assert_eq!(run("%%y"), "%2026");
        assert_eq!(run("%xh"), "%xh");
        assert_eq!(run("%Mo"), "%Mo");
        assert_eq!(run("%iz"), "255z");
        assert_eq!(run("لقطة_%y"), "لقطة_2026");
        assert_eq!(run("%y_ملف"), "2026_ملف");
    }

    #[test]
    fn default_patterns() {
        assert_eq!(run(DEFAULT_PATTERN), "Laqta_2026-11-07_14-03-22");
        assert_eq!(run(DEFAULT_SUBFOLDER_PATTERN), "2026-11");
    }

    #[test]
    fn file_name_replaces_invalid_characters() {
        assert_eq!(file(r#"a/b\c:d*e?f"g<h>i|j"#), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(file("x%ny\tz"), "x_y_z");
    }

    #[test]
    fn file_name_trims_trailing_dots_and_spaces() {
        assert_eq!(file("name. . "), "name");
    }

    #[test]
    fn file_name_falls_back_to_laqta() {
        assert_eq!(file(""), "Laqta");
        assert_eq!(file("..."), "Laqta");
        // A file that gives no lines leaves the name empty; bare `%rf` is not a token.
        assert_eq!(file("%rf{names.txt}"), "Laqta");
        assert_eq!(file("%rf"), "%rf");
    }

    #[test]
    fn file_name_caps_at_200_characters() {
        assert_eq!(file(&"a".repeat(300)), "a".repeat(200));
        assert_eq!(file(&"ق".repeat(300)), "ق".repeat(200));
        assert_eq!(file(&"%remoji".repeat(210)).chars().count(), 200);
        // The cap comes before the trim, so a space cut at the limit does not survive.
        assert_eq!(file(&format!("{} b", "a".repeat(199))), "a".repeat(199));
    }

    #[test]
    fn candidates_match_capture_candidates() {
        let dir = Path::new("/shots");
        let expected: Vec<PathBuf> = capture_candidates(dir, &instant()).collect();
        let actual: Vec<PathBuf> = candidates(dir, &run(DEFAULT_SUBFOLDER_PATTERN), &run(DEFAULT_PATTERN), "png").collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn candidates_without_subfolder() {
        let all: Vec<PathBuf> = candidates(Path::new("/out"), "", "Laqta", "png").collect();
        assert_eq!(all.len(), 999);
        assert_eq!(all[0], PathBuf::from("/out/Laqta.png"));
        assert_eq!(all[1], PathBuf::from("/out/Laqta_2.png"));
        assert_eq!(all[998], PathBuf::from("/out/Laqta_999.png"));
    }

    #[test]
    fn candidates_in_order() {
        let now = jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0);
        let all: Vec<_> = capture_candidates(Path::new("/shots"), &now).collect();
        assert_eq!(all.len(), 999);
        assert_eq!(all[0], PathBuf::from("/shots/2026-11/Laqta_2026-11-07_14-03-22.png"));
        assert_eq!(all[1], PathBuf::from("/shots/2026-11/Laqta_2026-11-07_14-03-22_2.png"));
        assert_eq!(all[998], PathBuf::from("/shots/2026-11/Laqta_2026-11-07_14-03-22_999.png"));
    }
}
