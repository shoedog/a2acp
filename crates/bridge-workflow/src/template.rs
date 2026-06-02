//! Single-pass `{{var}}` template rendering. One left-to-right scan: each `{{token}}`
//! is replaced by `vars[token]` (or left verbatim if unknown). A substituted VALUE is
//! never re-scanned, so an upstream output containing `{{x}}` cannot be re-expanded.
//! UTF-8 safe: only ever slices/pushes `&str` (no `byte as char`), so multibyte prompt
//! text (em-dashes, smart quotes, accents) is preserved.
use std::collections::HashMap;

pub fn render(template: &str, vars: &HashMap<&str, &str>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);                  // verbatim prefix (str slice = UTF-8 safe)
        let after = &rest[open + 2..];
        match after.find("}}") {
            Some(close) => {
                let token = &after[..close];
                match vars.get(token) {
                    Some(v) => out.push_str(v),        // value is NOT re-scanned
                    None => { out.push_str("{{"); out.push_str(token); out.push_str("}}"); } // unknown verbatim
                }
                rest = &after[close + 2..];
            }
            None => { out.push_str("{{"); rest = after; } // a lone "{{" with no close → literal
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    fn vars<'a>(p: &[(&'a str, &'a str)]) -> HashMap<&'a str, &'a str> { p.iter().cloned().collect() }

    #[test]
    fn substitutes_known_tokens() {
        let out = render("review {{input}} via {{codex}}", &vars(&[("input","DIFF"),("codex","OK")]));
        assert_eq!(out, "review DIFF via OK");
    }
    #[test]
    fn unknown_token_left_verbatim() {
        assert_eq!(render("a {{ghost}} b", &vars(&[("input","x")])), "a {{ghost}} b");
    }
    #[test]
    fn single_pass_no_reexpansion() {
        // codex's output literally contains "{{claude}}". A naive sequential replace would
        // expand it when {{claude}} is substituted next. Single-pass must NOT.
        let out = render("{{codex}}|{{claude}}", &vars(&[("codex","see {{claude}}"),("claude","REAL")]));
        assert_eq!(out, "see {{claude}}|REAL");
    }
    #[test]
    fn preserves_utf8() {
        // multibyte chars (em-dash, smart quote, accent) outside {{}} must survive intact.
        let out = render("café — \u{201c}{{x}}\u{201d}", &vars(&[("x","ø")]));
        assert_eq!(out, "café — \u{201c}ø\u{201d}");
    }
}
