//! Config-owned, admission-frozen prompt attachments. No delivery or execution authority.
use super::ConfigError;
use std::path::Path;

pub const MAX_SKILL_BYTES: usize = 32 * 1024;
pub const MAX_SELECTION_BYTES: usize = 64 * 1024;

#[derive(serde::Deserialize)]
pub struct SkillDeclaration {
    #[serde(default, deserialize_with = "string_or_empty")]
    id: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    path: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    delivery: String,
    #[serde(flatten)]
    unsupported: std::collections::BTreeMap<String, toml::Value>,
}

// Defer schema refusals to validation so serde does not attach the raw config
// (including source paths) to a skill-field diagnostic. Wrong types become invalid
// empty values, never an accepted coercion. Unknown fields are refused, not ignored.
fn string_or_empty<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    use serde::Deserialize;
    Ok(match toml::Value::deserialize(deserializer)? {
        toml::Value::String(value) => value,
        _ => String::new(),
    })
}
impl std::fmt::Debug for SkillDeclaration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillDeclaration")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Prompt,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Checkout,
    External,
}
#[derive(Debug, Clone, Copy)]
pub enum ExecutionRoute {
    Direct,
    Implement,
    WorkflowPendingComposition,
}
#[derive(Debug)]
pub struct SkillMetadata<'a> {
    pub id: &'a str,
    pub sha256: &'a str,
    pub bytes: usize,
    pub delivery: Delivery,
    pub origin: Origin,
    pub status: String,
}
pub struct FrozenSkill {
    id: String,
    source: String,
    sha256: String,
    origin: Origin,
}
impl FrozenSkill {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn byte_count(&self) -> usize {
        self.source.len()
    }
    pub fn delivery(&self) -> Delivery {
        Delivery::Prompt
    }
    pub fn origin(&self) -> Origin {
        self.origin
    }
    pub fn preview(&self, route: ExecutionRoute) -> SkillMetadata<'_> {
        SkillMetadata {
            id: &self.id,
            sha256: &self.sha256,
            bytes: self.byte_count(),
            delivery: self.delivery(),
            origin: self.origin,
            status: format!(
                "not_delivered({})",
                match route {
                    ExecutionRoute::Direct => "direct",
                    ExecutionRoute::Implement => "implement",
                    ExecutionRoute::WorkflowPendingComposition => "workflow_pending_composition",
                }
            ),
        }
    }
}
impl std::fmt::Debug for FrozenSkill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.preview(ExecutionRoute::Direct).fmt(f)
    }
}
#[derive(Debug, Default)]
pub struct FrozenSkillSelection {
    skills: Vec<FrozenSkill>,
}
impl FrozenSkillSelection {
    pub fn skills(&self) -> &[FrozenSkill] {
        &self.skills
    }
}
fn error(message: &str) -> ConfigError {
    ConfigError::Registry(format!("skills: {message}"))
}
pub fn validate_declarations(declarations: &[SkillDeclaration]) -> Result<(), ConfigError> {
    let mut ids = std::collections::HashSet::new();
    for declaration in declarations {
        if !declaration.unsupported.is_empty() {
            return Err(error(
                "unsupported skill declaration field; expected id, path, delivery",
            ));
        }
        if declaration.id.trim().is_empty() {
            return Err(error("id must be a nonempty string"));
        }
        if declaration.path.trim().is_empty() {
            return Err(error("path must be a nonempty string"));
        }
        if declaration.delivery != "prompt" {
            return Err(error("unsupported delivery; explicitly select prompt"));
        }
        if !ids.insert(&declaration.id) {
            return Err(error("duplicate skill id within agent selection"));
        }
    }
    Ok(())
}

/// Admission is the only constructor of frozen attachments. No paths survive admission.
/// Sources carry the declaring config's prompt trust; prefer operator-controlled files
/// outside untrusted checkouts. Preview is metadata only and does not imply delivery.
pub fn admit(
    declarations: &[SkillDeclaration],
    config: &Path,
    checkout: &Path,
) -> Result<FrozenSkillSelection, ConfigError> {
    validate_declarations(declarations)?;
    if declarations.is_empty() {
        return Ok(FrozenSkillSelection::default());
    }
    admit_platform(declarations, config, checkout)
}

#[cfg(not(unix))]
fn admit_platform(
    _declarations: &[SkillDeclaration],
    _config: &Path,
    _checkout: &Path,
) -> Result<FrozenSkillSelection, ConfigError> {
    Err(error("safe skill opening is unsupported on this platform"))
}

#[cfg(unix)]
fn admit_platform(
    declarations: &[SkillDeclaration],
    config: &Path,
    checkout: &Path,
) -> Result<FrozenSkillSelection, ConfigError> {
    if !config.is_absolute() || !checkout.is_absolute() {
        return Err(error(
            "declaring config and checkout context must be absolute",
        ));
    }
    let base = config
        .parent()
        .ok_or_else(|| error("config has no parent"))?;
    let checkout = checkout
        .canonicalize()
        .map_err(|_| error("checkout context unavailable"))?;
    let mut skills = Vec::with_capacity(declarations.len());
    let mut total = 0;
    for declaration in declarations {
        let path = base.join(&declaration.path);
        let file = open_source(&path)?;
        let bytes = read_bounded(file)?;
        total += bytes.len();
        if total > MAX_SELECTION_BYTES {
            return Err(error("selection exceeds 64 KiB"));
        }
        let source = String::from_utf8(bytes).map_err(|_| error("source must be UTF-8"))?;
        validate_frontmatter(&source)?;
        let digest = ring::digest::digest(&ring::digest::SHA256, source.as_bytes());
        let sha256 = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        // Normalize parent components only AFTER the no-follow traversal has succeeded.
        // Thus a symlink followed by '..' can never disappear before safe opening.
        let mut location = std::path::PathBuf::new();
        for component in path.components() {
            if component == std::path::Component::ParentDir {
                location.pop();
            } else {
                location.push(component);
            }
        }
        let origin = if location.starts_with(&checkout) {
            Origin::Checkout
        } else {
            Origin::External
        };
        skills.push(FrozenSkill {
            id: declaration.id.clone(),
            source,
            sha256,
            origin,
        });
    }
    Ok(FrozenSkillSelection { skills })
}

#[cfg(unix)]
fn open_source(path: &Path) -> Result<std::fs::File, ConfigError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    // Retain each directory descriptor while opening its child. No check-then-open
    // race, canonicalization of the source, or follow-link fallback is permitted.
    let mut file = std::fs::File::open("/").map_err(|_| error("source root unavailable"))?;
    let mut components = path
        .components()
        .filter(|c| *c != std::path::Component::RootDir)
        .peekable();
    while let Some(component) = components.next() {
        let name = std::ffi::CString::new(component.as_os_str().as_bytes())
            .map_err(|_| error("invalid source path component"))?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if components.peek().is_some() {
                libc::O_DIRECTORY
            } else {
                0
            };
        // SAFETY: live parent descriptor, NUL-terminated name, no creation flags.
        let fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(error(
                "source open refused (missing, symlinked or inaccessible component)",
            ));
        }
        // SAFETY: successful openat returns a new descriptor owned only by this File.
        file = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    if !file
        .metadata()
        .map_err(|_| error("opened source metadata unavailable"))?
        .is_file()
    {
        return Err(error("opened source must be a regular file"));
    }
    Ok(file)
}

#[cfg(unix)]
fn read_bounded(reader: impl std::io::Read) -> Result<Vec<u8>, ConfigError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader
        .take((MAX_SKILL_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| error("source read failed"))?;
    if bytes.len() > MAX_SKILL_BYTES {
        return Err(error("source exceeds 32 KiB"));
    }
    Ok(bytes)
}

/// Deliberately bounded frontmatter grammar, not a general YAML implementation.
/// Validation borrows lines; the original UTF-8 source is frozen without normalization.
fn validate_frontmatter(source: &str) -> Result<(), ConfigError> {
    let mut lines = source
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    if lines.next() != Some("---") {
        return Err(error("frontmatter must start with a --- line"));
    }
    let mut header = Vec::new();
    let mut closed = false;
    for line in lines {
        if line == "---" {
            closed = true;
            break;
        }
        if line.chars().any(|c| c.is_control() && c != '\t') {
            return Err(error("unsupported frontmatter control character"));
        }
        header.push(line);
    }
    if !closed {
        return Err(error("frontmatter closing --- line missing"));
    }
    let mut name = false;
    let mut description = false;
    let mut index = 0;
    while index < header.len() {
        let line = header[index];
        index += 1;
        reject_tab_indent(line)?;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with(' ') {
            return Err(error("unsupported nested map indentation"));
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| error("unsupported top-level construct; expected key: value"))?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(error("unsupported top-level key construct"));
        }
        if !value.is_empty() && !value.starts_with([' ', '\t']) {
            return Err(error("top-level key requires key: value separation"));
        }
        let required = match key {
            "name" => Some(&mut name),
            "description" => Some(&mut description),
            _ => None,
        };
        let is_required = required.is_some();
        if let Some(seen) = required {
            if *seen {
                return Err(error("duplicate required frontmatter key"));
            }
            *seen = true;
        }
        let value = value.trim_start();
        let nonempty = if value.starts_with(['|', '>']) {
            if key != "description" {
                return Err(error("unsupported block scalar outside description"));
            }
            let indicator = strip_comment(value).trim_end();
            if !matches!(indicator, "|" | "|-" | "|+" | ">" | ">-" | ">+") {
                return Err(error("unsupported block scalar indicator"));
            }
            let mut indent = None;
            let mut nonempty = false;
            while index < header.len() {
                let content = header[index];
                reject_tab_indent(content)?;
                if content.trim().is_empty() {
                    index += 1;
                    continue;
                }
                let spaces = content.bytes().take_while(|b| *b == b' ').count();
                if spaces == 0 {
                    break;
                }
                let expected = *indent.get_or_insert(spaces);
                if spaces < expected {
                    return Err(error("unsupported block scalar indentation"));
                }
                nonempty |= !content.trim().is_empty();
                index += 1;
            }
            nonempty
        } else {
            let scalar = parse_scalar(value, is_required)?;
            !scalar.trim().is_empty()
        };
        if is_required && !nonempty {
            return Err(error("required frontmatter strings must be nonempty"));
        }
    }
    if !name {
        return Err(error("frontmatter name string is required"));
    }
    if !description {
        return Err(error("frontmatter description string is required"));
    }
    Ok(())
}

fn reject_tab_indent(line: &str) -> Result<(), ConfigError> {
    if line
        .chars()
        .take_while(|c| c.is_whitespace())
        .any(|c| c == '\t')
    {
        return Err(error("unsupported tab indentation"));
    }
    Ok(())
}

fn strip_comment(value: &str) -> &str {
    let mut whitespace = true;
    for (index, c) in value.char_indices() {
        if c == '#' && whitespace {
            return &value[..index];
        }
        whitespace = c.is_whitespace();
    }
    value
}

fn parse_scalar(value: &str, require_string: bool) -> Result<String, ConfigError> {
    if value.starts_with(['\'', '"']) {
        return parse_quoted(value);
    }
    let value = strip_comment(value).trim_end();
    match value.chars().next() {
        None => return Err(error("unsupported empty value or nested map")),
        Some('[' | '{' | ']' | '}') => return Err(error("unsupported flow collection")),
        Some('&') => return Err(error("unsupported anchor")),
        Some('*') => return Err(error("unsupported alias")),
        Some('!') => return Err(error("unsupported tag")),
        Some('|' | '>') => return Err(error("unsupported block scalar")),
        Some('@' | '`' | '%' | ',') => return Err(error("unsupported scalar indicator")),
        _ => {}
    }
    if value
        .strip_prefix(['-', '?'])
        .is_some_and(|tail| tail.is_empty() || tail.starts_with([' ', '\t']))
    {
        return Err(error("unsupported sequence or complex key"));
    }
    if value.ends_with(':')
        || value
            .as_bytes()
            .windows(2)
            .any(|w| w[0] == b':' && w[1].is_ascii_whitespace())
    {
        return Err(error("unsupported nested map in scalar"));
    }
    // YAML 1.2 core implicit scalars are not strings. Match their syntax exactly:
    // prefixes/case variants that merely resemble a number or keyword remain strings.
    let numeric = value.bytes().any(|b| b.is_ascii_digit())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
        && value.parse::<f64>().is_ok();
    let based_integer = value
        .strip_prefix("0x")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        || value.strip_prefix("0o").is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|b| matches!(b, b'0'..=b'7'))
        });
    let infinity = matches!(
        value.strip_prefix(['+', '-']).unwrap_or(value),
        ".inf" | ".Inf" | ".INF"
    );
    if require_string
        && (numeric
            || based_integer
            || infinity
            || matches!(
                value,
                "null"
                    | "Null"
                    | "NULL"
                    | "~"
                    | "true"
                    | "True"
                    | "TRUE"
                    | "false"
                    | "False"
                    | "FALSE"
                    | ".nan"
                    | ".NaN"
                    | ".NAN"
            ))
    {
        return Err(error(
            "required frontmatter value must be a string; quote non-string scalars",
        ));
    }
    Ok(value.to_owned())
}

fn parse_quoted(value: &str) -> Result<String, ConfigError> {
    let quote = value.chars().next().expect("quoted scalar");
    let mut chars = value[1..].char_indices().peekable();
    let mut out = String::new();
    while let Some((index, c)) = chars.next() {
        if c == quote {
            if quote == '\'' && chars.peek().is_some_and(|(_, c)| *c == '\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            let tail = &value[index + 2..];
            if !tail.is_empty() && !tail.starts_with([' ', '\t']) {
                return Err(error("unsupported trailing quoted scalar content"));
            }
            if !strip_comment(tail).trim().is_empty() {
                return Err(error("unsupported trailing quoted scalar content"));
            }
            return Ok(out);
        }
        if c != '\\' || quote == '\'' {
            out.push(c);
            continue;
        }
        let (_, escaped) = chars
            .next()
            .ok_or_else(|| error("invalid quoted scalar escape"))?;
        let decoded = match escaped {
            '0' => '\0',
            'a' => '\u{7}',
            'b' => '\u{8}',
            't' | '\t' => '\t',
            'n' => '\n',
            'v' => '\u{b}',
            'f' => '\u{c}',
            'r' => '\r',
            'e' => '\u{1b}',
            ' ' => ' ',
            '"' => '"',
            '/' => '/',
            '\\' => '\\',
            'N' => '\u{85}',
            '_' => '\u{a0}',
            'L' => '\u{2028}',
            'P' => '\u{2029}',
            'x' | 'u' | 'U' => {
                let count = match escaped {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let mut code = 0u32;
                for _ in 0..count {
                    let digit = chars
                        .next()
                        .and_then(|(_, c)| c.to_digit(16))
                        .ok_or_else(|| error("invalid Unicode escape"))?;
                    code = code
                        .checked_mul(16)
                        .and_then(|v| v.checked_add(digit))
                        .ok_or_else(|| error("invalid Unicode escape"))?;
                }
                char::from_u32(code).ok_or_else(|| error("invalid Unicode escape"))?
            }
            _ => return Err(error("unsupported quoted scalar escape")),
        };
        out.push(decoded);
    }
    Err(error("unterminated quoted scalar"))
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "skills_config_tests.rs"]
mod config_tests;
