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
pub struct SchemaDef {
    pub task_type: &'static str,
    pub summary: &'static str,
    pub sections: &'static [SectionDef],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionDef {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
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

const TASK_TYPES: &[&str] = &[
    "freeform",
    "implement",
    "code-review",
    "spec-review",
    "plan-review",
    "design",
];

const FREEFORM_SECTIONS: &[SectionDef] = &[];

const IMPLEMENT_SECTIONS: &[SectionDef] = &[
    SectionDef {
        name: "Description",
        description: "Describe the requested implementation work and the problem it solves. Include any important context the agent needs before editing.",
        required: true,
    },
    SectionDef {
        name: "Acceptance Criteria",
        description: "List the concrete outcomes that must be true when the task is complete. Prefer observable behavior, tests, or verification steps.",
        required: true,
    },
    SectionDef {
        name: "Files",
        description: "Name files, directories, or ownership areas that are relevant to the implementation. This is guidance only and is not a path-existence check.",
        required: false,
    },
    SectionDef {
        name: "Spec Refs",
        description: "Link or name specifications, plans, ADRs, issues, or other source material the task should follow.",
        required: false,
    },
    SectionDef {
        name: "Commit Message",
        description: "Provide the preferred commit message for the resulting change. If omitted, later implement flow falls back to its configured sources.",
        required: false,
    },
];

const REVIEW_SECTIONS: &[SectionDef] = &[
    SectionDef {
        name: "Description",
        description: "Describe the change, document, or plan to review and the review goal. Include any known risk areas or context.",
        required: true,
    },
    SectionDef {
        name: "Acceptance Criteria",
        description: "List what a useful review must cover. Include expected outputs such as findings, approval criteria, or required checks.",
        required: true,
    },
    SectionDef {
        name: "Files",
        description: "Name files, directories, or ownership areas that should receive review attention. This is guidance only and is not a path-existence check.",
        required: false,
    },
    SectionDef {
        name: "Spec Refs",
        description: "Link or name specifications, plans, ADRs, issues, or other source material the review should use as authority.",
        required: false,
    },
];

const DESIGN_SECTIONS: &[SectionDef] = &[
    SectionDef {
        name: "Description",
        description: "Describe the design problem, desired outcome, and relevant constraints. Include enough context for clean-room design work.",
        required: true,
    },
    SectionDef {
        name: "Acceptance Criteria",
        description: "List the properties a satisfactory design must satisfy. Prefer tradeoffs, invariants, and decision points that can be evaluated.",
        required: true,
    },
    SectionDef {
        name: "Spec Refs",
        description: "Link or name specifications, plans, ADRs, issues, or other source material the design should follow.",
        required: false,
    },
];

static SCHEMAS: &[SchemaDef] = &[
    SchemaDef {
        task_type: "freeform",
        summary: "Back-compat task body with no required sections; the entire body is the task and the title is optional.",
        sections: FREEFORM_SECTIONS,
    },
    SchemaDef {
        task_type: "implement",
        summary: "Implementation task with explicit success criteria and optional file, spec, and commit-message guidance.",
        sections: IMPLEMENT_SECTIONS,
    },
    SchemaDef {
        task_type: "code-review",
        summary: "Code review task with explicit review criteria and optional file and spec guidance.",
        sections: REVIEW_SECTIONS,
    },
    SchemaDef {
        task_type: "spec-review",
        summary: "Specification review task with explicit review criteria and optional file and spec guidance.",
        sections: REVIEW_SECTIONS,
    },
    SchemaDef {
        task_type: "plan-review",
        summary: "Plan review task with explicit review criteria and optional file and spec guidance.",
        sections: REVIEW_SECTIONS,
    },
    SchemaDef {
        task_type: "design",
        summary: "Design task with explicit criteria and optional spec guidance.",
        sections: DESIGN_SECTIONS,
    },
];

impl TaskSpec {
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|section| section.name.eq_ignore_ascii_case(name))
    }
}

pub fn task_types() -> &'static [&'static str] {
    TASK_TYPES
}

pub fn schema(t: &str) -> Option<&'static SchemaDef> {
    SCHEMAS.iter().find(|schema| schema.task_type == t)
}

pub fn validate(spec: &TaskSpec) -> Result<(), TaskSpecError> {
    let schema = schema(&spec.task_type).ok_or_else(|| TaskSpecError::UnknownType {
        got: spec.task_type.clone(),
    })?;

    if schema.task_type != "freeform" {
        match spec.title.as_deref() {
            None => {
                return Err(TaskSpecError::MissingTitle {
                    task_type: spec.task_type.clone(),
                });
            }
            Some(title) if is_blank(title) => {
                return Err(TaskSpecError::EmptyTitle {
                    task_type: spec.task_type.clone(),
                });
            }
            Some(_) => {}
        }
    }

    for section in schema.sections.iter().filter(|section| section.required) {
        let parsed = spec.section(section.name).ok_or_else(|| TaskSpecError::MissingSection {
            task_type: spec.task_type.clone(),
            section: section.name.to_string(),
        })?;
        if is_blank(&parsed.content) {
            return Err(TaskSpecError::EmptySection {
                task_type: spec.task_type.clone(),
                section: section.name.to_string(),
            });
        }
    }

    Ok(())
}

pub fn fields(spec: &TaskSpec) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    fields.push(("title".to_string(), spec.title.clone().unwrap_or_default()));
    fields.push(("type".to_string(), spec.task_type.clone()));

    for section in &spec.sections {
        push_section_fields(&mut fields, "", section);
    }

    fields
}

pub fn body(spec: &TaskSpec) -> &str {
    &spec.body
}

pub fn template(t: &str) -> Option<String> {
    let schema = schema(t)?;
    let mut out = format!("---\ntask-type: {}\n---\n# <title>\n", schema.task_type);

    for section in schema.sections {
        let requirement = if section.required { "REQUIRED" } else { "OPTIONAL" };
        out.push_str(&format!(
            "\n## {}\n<!-- {}: {} -->\n",
            section.name, requirement, section.description
        ));
    }

    out.push_str(
        "\n## <Your Own Section>\n<!-- OPTIONAL/EXTENSION: Add task-specific context not covered by the schema. -->\n",
    );
    Some(out)
}

fn push_section_fields(fields: &mut Vec<(String, String)>, prefix: &str, section: &Section) {
    let name = normalize_field_name(&section.name);
    let key = if prefix.is_empty() {
        name
    } else {
        format!("{prefix}.{name}")
    };
    fields.push((key.clone(), section.content.clone()));

    for subsection in &section.subsections {
        push_section_fields(fields, &key, subsection);
    }
}

fn normalize_field_name(name: &str) -> String {
    let mut out = String::new();
    let mut previous_was_separator = false;

    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator {
            out.push('_');
            previous_was_separator = true;
        }
    }

    out.trim_matches('_').to_string()
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

fn strip_html_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + "<!--".len()..];
        let Some(end) = after_start.find("-->") else {
            return out;
        };
        rest = &after_start[end + "-->".len()..];
    }

    out.push_str(rest);
    out
}

fn is_blank(content: &str) -> bool {
    strip_html_comments(content).trim().is_empty()
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

impl std::fmt::Display for TaskSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskSpecError::NoTaskType => write!(
                f,
                "missing `task-type` front-matter; valid types: {}; {}",
                valid_types_display(),
                task_spec_hint()
            ),
            TaskSpecError::UnknownType { got } => write!(
                f,
                "unknown task-type `{}`; valid types: {}; {}",
                sanitize_task_type(got),
                valid_types_display(),
                task_spec_hint()
            ),
            TaskSpecError::MissingSection { task_type, section } => write!(
                f,
                "missing required section `{}` for task-type `{}`; {}",
                known_section_name(task_type, section),
                known_task_type_name(task_type),
                task_spec_hint()
            ),
            TaskSpecError::EmptySection { task_type, section } => write!(
                f,
                "empty required section `{}` for task-type `{}`; {}",
                known_section_name(task_type, section),
                known_task_type_name(task_type),
                task_spec_hint()
            ),
            TaskSpecError::MissingTitle { task_type } => write!(
                f,
                "missing required title for task-type `{}`; {}",
                known_task_type_name(task_type),
                task_spec_hint()
            ),
            TaskSpecError::EmptyTitle { task_type } => write!(
                f,
                "empty required title for task-type `{}`; {}",
                known_task_type_name(task_type),
                task_spec_hint()
            ),
            TaskSpecError::Parse(_) => write!(f, "malformed task-spec (see `task-spec schema`)"),
        }
    }
}

impl std::error::Error for TaskSpecError {}

fn valid_types_display() -> String {
    task_types().join(", ")
}

fn task_spec_hint() -> &'static str {
    "run `a2a-bridge task-spec schema <type>` or `a2a-bridge task-spec template <type>`"
}

fn sanitize_task_type(got: &str) -> String {
    const MAX_LEN: usize = 32;
    if got.is_empty() || !got.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return "invalid".to_string();
    }
    got.chars().take(MAX_LEN).collect()
}

fn known_task_type_name(task_type: &str) -> &'static str {
    schema(task_type)
        .map(|schema| schema.task_type)
        .unwrap_or("task")
}

fn known_section_name(task_type: &str, section: &str) -> &'static str {
    schema(task_type)
        .and_then(|schema| {
            schema
                .sections
                .iter()
                .find(|def| def.name.eq_ignore_ascii_case(section))
        })
        .map(|def| def.name)
        .unwrap_or("required section")
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

    #[test]
    fn validate_per_type() {
        for task_type in [
            "implement",
            "code-review",
            "spec-review",
            "plan-review",
            "design",
        ] {
            let spec = parse(&format!(
                "---\ntask-type: {task_type}\n---\n# Work\n\n## Description\nDo it.\n\n## Acceptance Criteria\n- It works.\n"
            ))
            .unwrap();
            assert!(validate(&spec).is_ok(), "{task_type} should validate");
        }

        let missing = parse(
            "---\ntask-type: implement\n---\n# Work\n\n## Description\nDo it.\n",
        )
        .unwrap();
        assert!(matches!(
            validate(&missing),
            Err(TaskSpecError::MissingSection { section, .. }) if section == "Acceptance Criteria"
        ));

        let unknown = parse("---\ntask-type: nope\n---\n# Work\n").unwrap();
        assert!(matches!(
            validate(&unknown),
            Err(TaskSpecError::UnknownType { .. })
        ));

        let freeform = parse("---\ntask-type: freeform\n---\nanything").unwrap();
        assert!(validate(&freeform).is_ok());
    }

    #[test]
    fn comment_only_section_is_empty() {
        let spec = parse(
            "---\ntask-type: implement\n---\n# Work\n\n## Description\n<!-- todo -->\n\n## Acceptance Criteria\n- It works.\n",
        )
        .unwrap();

        assert!(matches!(
            validate(&spec),
            Err(TaskSpecError::EmptySection { section, .. }) if section == "Description"
        ));
    }

    #[test]
    fn wire_display_is_bridge_authored_no_echo() {
        let msg = TaskSpecError::UnknownType {
            got: "../etc/passwd\nINJECT".into(),
        }
        .to_string();

        assert!(msg.contains("task-spec schema"));
        assert!(msg.contains("implement"));
        assert!(!msg.contains("passwd"));
        assert!(!msg.contains("INJECT"));
    }

    #[test]
    fn display_covers_notasktype_and_missingtitle() {
        let no_task_type = TaskSpecError::NoTaskType.to_string();
        assert!(no_task_type.contains("task-spec schema"));
        assert!(no_task_type.contains("freeform"));

        let missing_title = parse(
            "---\ntask-type: implement\n---\n## Description\nDo it.\n\n## Acceptance Criteria\n- It works.\n",
        )
        .unwrap();
        let err = validate(&missing_title).unwrap_err();
        assert!(matches!(err, TaskSpecError::MissingTitle { .. }));
        assert!(err.to_string().contains("task-spec schema"));

        let empty_title = parse(
            "---\ntask-type: implement\n---\n#   \n\n## Description\nDo it.\n\n## Acceptance Criteria\n- It works.\n",
        )
        .unwrap();
        let err = validate(&empty_title).unwrap_err();
        assert!(matches!(err, TaskSpecError::EmptyTitle { .. }));
        assert!(err.to_string().contains("task-spec schema"));
    }

    #[test]
    fn fields_flatten_and_body() {
        let s = parse(
            "---\ntask-type: implement\n---\n# T\n## Description\n### Context\nc\n## Files\n- a.rs",
        )
        .unwrap();
        let f: std::collections::HashMap<_, _> = fields(&s).into_iter().collect();

        assert_eq!(f.get("title").map(String::as_str), Some("T"));
        assert!(f.get("files").unwrap().contains("a.rs"));
        assert_eq!(f.get("description.context").map(|s| s.trim()), Some("c"));
        assert_eq!(body(&s), s.body.as_str());
    }

    #[test]
    fn template_round_trips() {
        let t = template("implement").unwrap();

        assert!(t.contains("task-type: implement"));
        assert!(t.contains("## Acceptance Criteria"));
        assert!(t.contains("OPTIONAL"));
        assert!(parse(&t).is_ok());
    }
}
