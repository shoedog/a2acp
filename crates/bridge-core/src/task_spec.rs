#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSpec {
    pub task_type: String,
    pub title: Option<String>,
    pub body: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub content: String,
    pub subsections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskSpecError {
    NoTaskType,
    UnknownType { got: String },
    MissingSection { task_type: String, section: String },
    EmptySection { task_type: String, section: String },
    MissingTitle { task_type: String },
    EmptyTitle { task_type: String },
    Parse(String),
}

impl TaskSpec {
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|section| section.name.eq_ignore_ascii_case(name))
    }
}

pub fn parse(raw: &str) -> Result<TaskSpec, TaskSpecError> {
    let normalized = raw.replace("\r\n", "\n");
    let (task_type, body) = parse_frontmatter(&normalized)?;
    let (title, sections) = parse_body(&body);

    Ok(TaskSpec {
        task_type,
        title,
        body,
        sections,
    })
}

fn parse_frontmatter(input: &str) -> Result<(String, String), TaskSpecError> {
    if !input.starts_with("---\n") {
        return Err(TaskSpecError::NoTaskType);
    }

    let mut task_type = None;
    let mut offset = 4;

    for line in input[4..].split_inclusive('\n') {
        let line_content = line.strip_suffix('\n').unwrap_or(line);
        if line_content == "---" {
            let body_start = offset + line.len();
            let value = task_type
                .filter(|value: &String| !value.trim().is_empty())
                .ok_or(TaskSpecError::NoTaskType)?;
            return Ok((value, input[body_start..].to_string()));
        }

        parse_frontmatter_line(line_content, &mut task_type)?;
        offset += line.len();
    }

    Err(TaskSpecError::Parse("unclosed front-matter".to_string()))
}

fn parse_frontmatter_line(
    line: &str,
    task_type: &mut Option<String>,
) -> Result<(), TaskSpecError> {
    if line.trim().is_empty() || line.starts_with('#') {
        return Ok(());
    }

    if matches!(line.chars().next(), Some(c) if c.is_whitespace()) {
        return Err(TaskSpecError::Parse(
            "nested front-matter is not supported".to_string(),
        ));
    }

    let (key, value) = line
        .split_once(':')
        .ok_or_else(|| TaskSpecError::Parse("invalid front-matter line".to_string()))?;
    let key = key.trim();
    let value = value.trim();

    if key.is_empty() {
        return Err(TaskSpecError::Parse("empty front-matter key".to_string()));
    }

    if value.is_empty() {
        if key == "task-type" {
            *task_type = None;
            return Ok(());
        }
        return Err(TaskSpecError::Parse(
            "nested front-matter is not supported".to_string(),
        ));
    }

    if key == "task-type" {
        *task_type = Some(value.to_string());
    }

    Ok(())
}

fn parse_body(body: &str) -> (Option<String>, Vec<Section>) {
    let mut title = None;
    let mut sections = Vec::new();
    let mut current_section: Option<Section> = None;
    let mut current_subsection: Option<Section> = None;
    let mut fence: Option<&'static str> = None;

    for line in body.split_inclusive('\n') {
        let line_content = line.strip_suffix('\n').unwrap_or(line);

        if let Some(marker) = fence {
            append_content(&mut current_section, &mut current_subsection, line);
            if fence_marker(line_content) == Some(marker) {
                fence = None;
            }
            continue;
        }

        if let Some(marker) = fence_marker(line_content) {
            append_content(&mut current_section, &mut current_subsection, line);
            fence = Some(marker);
            continue;
        }

        if let Some(name) = line_content.strip_prefix("## ") {
            finish_subsection(&mut current_section, &mut current_subsection);
            finish_section(&mut sections, &mut current_section);
            current_section = Some(Section {
                name: name.trim().to_string(),
                content: String::new(),
                subsections: Vec::new(),
            });
            continue;
        }

        if let Some(name) = line_content.strip_prefix("### ") {
            if current_section.is_some() {
                finish_subsection(&mut current_section, &mut current_subsection);
                current_subsection = Some(Section {
                    name: name.trim().to_string(),
                    content: String::new(),
                    subsections: Vec::new(),
                });
            }
            continue;
        }

        if title.is_none() {
            if let Some(value) = line_content.strip_prefix("# ") {
                title = Some(value.trim().to_string());
            }
        }

        append_content(&mut current_section, &mut current_subsection, line);
    }

    finish_subsection(&mut current_section, &mut current_subsection);
    finish_section(&mut sections, &mut current_section);

    (title, sections)
}

fn fence_marker(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn append_content(
    current_section: &mut Option<Section>,
    current_subsection: &mut Option<Section>,
    line: &str,
) {
    if let Some(subsection) = current_subsection {
        subsection.content.push_str(line);
    } else if let Some(section) = current_section {
        section.content.push_str(line);
    }
}

fn finish_subsection(
    current_section: &mut Option<Section>,
    current_subsection: &mut Option<Section>,
) {
    if let (Some(section), Some(subsection)) = (current_section, current_subsection.take()) {
        section.subsections.push(subsection);
    }
}

fn finish_section(sections: &mut Vec<Section>, current_section: &mut Option<Section>) {
    if let Some(section) = current_section.take() {
        sections.push(section);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_title_sections() {
        let s = parse(
            "---\ntask-type: implement\n---\n# Add foo\n\n## Files\n- a.rs\n\n## Description\ndo it",
        )
        .unwrap();
        assert_eq!(s.task_type, "implement");
        assert_eq!(s.title.as_deref(), Some("Add foo"));
        assert_eq!(s.section("Files").unwrap().content.trim(), "- a.rs");
        assert!(s.body.starts_with("# Add foo"));
        assert!(!s.body.contains("task-type:"));
    }

    #[test]
    fn heading_inside_code_fence_is_not_a_section() {
        let s =
            parse("---\ntask-type: freeform\n---\n## Description\n```\n## not a section\n```\nx")
                .unwrap();
        assert!(s.section("not a section").is_none());
        assert!(s
            .section("Description")
            .unwrap()
            .content
            .contains("## not a section"));
    }

    #[test]
    fn crlf_frontmatter_and_missing_frontmatter() {
        assert_eq!(
            parse("---\r\ntask-type: freeform\r\n---\r\n# t")
                .unwrap()
                .task_type,
            "freeform"
        );
        assert!(matches!(parse("# no frontmatter"), Err(TaskSpecError::NoTaskType)));
        assert!(matches!(
            parse("---\ntask-type: x\n# unclosed"),
            Err(TaskSpecError::Parse(_))
        ));
    }

    #[test]
    fn subsections_nest() {
        let s = parse("---\ntask-type: freeform\n---\n## Description\n### Context\nc").unwrap();
        assert_eq!(
            s.section("Description").unwrap().subsections[0].name,
            "Context"
        );
        assert_eq!(
            s.section("Description").unwrap().subsections[0]
                .content
                .trim(),
            "c"
        );
    }

    #[test]
    fn list_or_nested_frontmatter_is_parse_error() {
        assert!(matches!(
            parse("---\ntask-type: freeform\n  - x\n---\n# t"),
            Err(TaskSpecError::Parse(_))
        ));
        assert!(matches!(
            parse("---\ntask-type: freeform\nk:\n  a: b\n---\n# t"),
            Err(TaskSpecError::Parse(_))
        ));
    }

    #[test]
    fn hash_without_space_is_not_heading() {
        let s = parse("---\ntask-type: freeform\n---\n##x\n\n## Description\nbody").unwrap();
        assert!(s.section("x").is_none());
        assert_eq!(s.sections.len(), 1);
        assert_eq!(s.section("Description").unwrap().content.trim(), "body");
    }

    #[test]
    fn tilde_and_infostring_fences() {
        let s = parse(
            "---\ntask-type: freeform\n---\n## Description\n~~~\n## not tilde\n~~~\n```rust\n## not rust\n```\n## Next\nn",
        )
        .unwrap();

        let description = s.section("Description").unwrap();
        assert!(description.content.contains("## not tilde"));
        assert!(description.content.contains("## not rust"));
        assert!(s.section("not tilde").is_none());
        assert!(s.section("not rust").is_none());
        assert_eq!(s.section("Next").unwrap().content.trim(), "n");
    }
}
